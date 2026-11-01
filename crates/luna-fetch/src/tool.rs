//! 外部工具发现与状态探测（TDD-03 §11.1）。
//!
//! **为什么不能只写 `Command::new("yt-dlp")`**：macOS 上从 Finder/Dock 启动的 GUI 应用继承的是
//! launchd 的最小 `PATH`（`/usr/bin:/bin:/usr/sbin:/sbin`），既没有 `/opt/homebrew/bin` 也没有
//! `/usr/local/bin`——用户明明 `brew install yt-dlp` 成功了，应用里却"找不到"。终端里 `cargo tauri
//! dev` 跑起来的开发构建反而正常（继承了 shell 的 PATH），于是这类缺陷只在打包后的真机出现。
//! 故发现顺序为：**用户显式指定 → Lunarca 受管安装 → 常见包管理器落点 → PATH 逐项**。
//! 受管那一档来自"应用代为下载安装"（ADR-023 修订，[`crate::install`]）——它排在系统各处
//! 之前，因为装了它即表示用户选择让 Lunarca 管理版本，而它是安装那刻的 latest。
//!
//! 同一套发现逻辑也用于 ffmpeg 与 JS 运行时——它们在场与否分别决定视频档能否合并出高分辨率
//! （见 [`crate::format`]）与站点取流挑战能否解开，是**能力分档**而不是错误。
//!
//! ## 为什么"装在哪儿"与"怎么装"同住一个模块
//! 本模块另有 [`install_guide`]：一件工具在**当前平台**上该怎么装。它与发现顺序是同一件事的
//! 两面——候选目录列的正是"用这个包管理器装完会落在哪"，引导给的正是"用哪个包管理器装"。
//! 两者若各写各的，就会出现"引导让你 `winget install`、而候选目录里没有 winget 的落点"
//! 这种自相矛盾（装完了应用还是说没装）。故它们并排放，且**一起**按平台分叉。
//! [`ExternalTool`] 与 [`locate_executable`] 同一条：几件工具共用一处知识，才会有一致的行为，
//! 而不是每处各写一份、各踩一次同一个坑。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::FetchError;

/// `PATH` 里可能缺失、但包管理器实际安装到的**绝对**位置（顺序 = 优先级）。
///
/// > 假设：覆盖 Homebrew（Apple Silicon / Intel）、pipx/pip --user、MacPorts 这四种主流装法即够
/// >（理由：yt-dlp 官方文档给出的正是 brew 与 pipx 两条路；MacPorts 顺带一条几乎零成本。
/// > 装在别处的用户走设置里的显式路径，那是唯一需要"自己说清楚"的场景）。
#[cfg(not(windows))]
const EXTRA_BIN_DIRS: &[&str] = &[
    "/opt/homebrew/bin", // Homebrew（Apple Silicon）
    "/usr/local/bin",    // Homebrew（Intel）/ 手动安装
    "/opt/local/bin",    // MacPorts
    "/usr/bin",          // 系统包管理器（Linux）
];

/// 同上（Windows）：Chocolatey 的机器级 shim 目录。
///
/// **Windows 此前一个候选都没有**——上面那四条全是 Unix 绝对路径，在 Windows 上永远不存在，
/// 于是发现完全落在 `PATH` 上。表面看没问题（Windows 的 GUI 进程从 Explorer 继承用户 `PATH`，
/// 不像 macOS 那样被 launchd 削成最小集），**但"装完点重新检测、不必重启"这条承诺在 Windows 上
/// 是假的**：`winget` / `scoop` 刚把 shim 目录加进注册表里的 `PATH`，已经在跑的进程拿到的仍是
/// 旧环境块，要等重启应用才看得见。列出落点即可当场找到——引导里让用户用哪个包管理器装，
/// 这里就必须有它的落点（见模块头注）。
#[cfg(windows)]
const EXTRA_BIN_DIRS: &[&str] = &[r"C:\ProgramData\chocolatey\bin"];

/// 用户 home 下的候选相对目录（pipx / pip --user / 手动放置）。
#[cfg(not(windows))]
const HOME_BIN_DIRS: &[&str] = &[".local/bin", "bin"];

/// 同上（Windows）：winget 的 Links 目录与 scoop 的 shims，均在用户 profile 下。
#[cfg(windows)]
const HOME_BIN_DIRS: &[&str] = &[
    r"AppData\Local\Microsoft\WinGet\Links", // winget（Win11 自带，本项目引导的首选）
    r"scoop\shims",                          // scoop
    r".local\bin",                           // pipx / pip --user
];

/// 抓取工具的可执行文件名（Windows 需 `.exe`）。
#[cfg(windows)]
const YT_DLP_NAMES: &[&str] = &["yt-dlp.exe"];
/// 抓取工具的可执行文件名。
#[cfg(not(windows))]
const YT_DLP_NAMES: &[&str] = &["yt-dlp"];

/// 合并工具的可执行文件名。
#[cfg(windows)]
const FFMPEG_NAMES: &[&str] = &["ffmpeg.exe"];
/// 合并工具的可执行文件名。
#[cfg(not(windows))]
const FFMPEG_NAMES: &[&str] = &["ffmpeg"];

/// yt-dlp 可用的 JS 运行时（`(yt-dlp 认的名字, 可执行文件名)`，按 **yt-dlp 自己的优先级**排序）。
///
/// 为什么必须有这一档：YouTube 的取流地址要先解一段 JS 挑战，yt-dlp 已把「无 JS 运行时的
/// 提取」标为 deprecated——没有运行时它会**静默回落**到不需要挑战的播放器客户端，而那些
/// 客户端的媒体地址随时可能被站点以 `HTTP 403` 拒绝（2026-08-23 真机缺陷的直接现场）。
///
/// > 假设：只列 deno / node / bun 三者（理由：yt-dlp 同样支持 quickjs，但它的可执行文件名
/// > 随构建而异——`qjs` / `quickjs` / 发行版自定，猜错会把"没装"报成"装了却跑不通"。
/// > 这三个各自只有一个公认名字，判定是确定的）。
#[cfg(windows)]
const JS_RUNTIMES: &[(&str, &str)] = &[
    ("deno", "deno.exe"),
    ("node", "node.exe"),
    ("bun", "bun.exe"),
];
/// 同上（非 Windows）。
#[cfg(not(windows))]
const JS_RUNTIMES: &[(&str, &str)] = &[("deno", "deno"), ("node", "node"), ("bun", "bun")];

/// Lunarca 依赖的**进程外外部工具**（发现顺序与安装引导共用的枚举）。
///
/// 枚举放这里，界面就不必自己维护一张"工具 → 安装命令"的表——
/// 那张表一旦落在前端，就会立刻分裂成中英两份、再分裂成 macOS 与 Windows 两份。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExternalTool {
    /// 网络媒体抓取（Lunarca 可代装，见 [`crate::install`]）。
    YtDlp,
    /// 音视频流合并（只引导不代装：上游不发布官方二进制，ADR-023）。
    Ffmpeg,
    /// yt-dlp 解站点 JS 挑战所需的运行时（引导装 Deno，见 [`JS_RUNTIMES`]）。
    JsRuntime,
}

impl ExternalTool {
    /// 全部工具（界面按此顺序逐件呈现；新增一件必须加进来，`every_tool_has_a_guide` 守护）。
    pub const ALL: &'static [Self] = &[Self::YtDlp, Self::Ffmpeg, Self::JsRuntime];

    /// 稳定 id（**IPC 契约**，前端按它取文案与图标；改动即破坏前端）。
    ///
    /// 与 [`crate::FetchMediaKind::as_str`] 同一条口径：`luna-fetch` 保持零新增依赖（不引 specta），
    /// 故枚举以稳定字符串过界，而不是让每处调用方自己拼一个名字。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::YtDlp => "yt_dlp",
            Self::Ffmpeg => "ffmpeg",
            Self::JsRuntime => "js_runtime",
        }
    }

    /// 官方安装说明页。**恒有**——一行命令那条路对"没装 Homebrew 的 macOS 用户"和
    /// "老 Win10 没有 winget 的用户"都是死路，官网是唯一恒通的出路。
    #[must_use]
    pub const fn docs_url(self) -> &'static str {
        match self {
            Self::YtDlp => "https://github.com/yt-dlp/yt-dlp/wiki/Installation",
            Self::Ffmpeg => "https://ffmpeg.org/download.html",
            Self::JsRuntime => "https://docs.deno.com/runtime/getting_started/installation/",
        }
    }

    /// 上游项目的许可证（SPDX；「开源组件与许可证」页逐件写明，ADR-168 ⑤）。JS 运行时三种
    /// （Deno / Node / Bun）都是 MIT。ffmpeg 的项目许可是 LGPL，个别构建启用了 GPL 部件。
    #[must_use]
    pub const fn license(self) -> &'static str {
        match self {
            Self::YtDlp => "Unlicense",
            Self::Ffmpeg => "LGPL-2.1-or-later",
            Self::JsRuntime => "MIT",
        }
    }

    /// 上游项目主页（不带协议，展示用）。JS 运行时给的是 Lunarca 代装的那一种（Deno）；在用的
    /// 若是用户自己装的 Node / Bun，见 [`js_runtime_source`]。
    #[must_use]
    pub const fn source(self) -> &'static str {
        match self {
            Self::YtDlp => "github.com/yt-dlp/yt-dlp",
            Self::Ffmpeg => "ffmpeg.org",
            Self::JsRuntime => "github.com/denoland/deno",
        }
    }

    /// 本平台包管理器里的包名。
    #[cfg(target_os = "macos")]
    #[must_use]
    const fn package(self) -> Option<&'static str> {
        Some(match self {
            Self::YtDlp => "yt-dlp",
            Self::Ffmpeg => "ffmpeg",
            // 三种运行时里只引导装 Deno：它是 yt-dlp 的默认首选，且是单文件零依赖的那一个。
            Self::JsRuntime => "deno",
        })
    }

    /// 本平台包管理器里的包 id（winget 的 `Publisher.Package` 形态）。
    #[cfg(target_os = "windows")]
    #[must_use]
    const fn package(self) -> Option<&'static str> {
        Some(match self {
            Self::YtDlp => "yt-dlp.yt-dlp",
            Self::Ffmpeg => "Gyan.FFmpeg",
            Self::JsRuntime => "DenoLand.Deno",
        })
    }

    /// 其余平台不给一行命令（发行版包管理器各不相同，猜错的代价是把用户带进死胡同）。
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    #[must_use]
    const fn package(self) -> Option<&'static str> {
        let _ = self;
        None
    }
}

/// 某一种 JS 运行时的项目主页（不带协议）：「开源组件与许可证」页按**在用的那一种**写来源，
/// 不把用户自己装的 Node 写成 Deno 的仓库。三种都是 MIT，许可证不随之变。
#[must_use]
pub fn js_runtime_source(name: &str) -> &'static str {
    match name {
        "node" => "nodejs.org",
        "bun" => "bun.sh",
        _ => ExternalTool::JsRuntime.source(),
    }
}

/// 本平台默认引导使用的包管理器：`(展示名, 安装命令动词)`。
///
/// **这一处此前根本不存在，而界面上的命令是写死在 i18n 里的 `brew install …`**——
/// Windows 用户复制过去必然失败，且面板里没有第二条路。命令是**环境事实**不是文案：
/// 它随平台变、不随语言变，放 i18n 就必然分裂成"语言 × 平台"份，且没有任何门禁能发现
/// 其中一份是错的。故它与工具发现同住 Rust（ADR-003 前端零业务）。
///
/// > 假设：macOS 取 Homebrew、Windows 取 winget（理由：前者是 macOS 事实上的默认；后者
/// > 随 Win11 与近期 Win10 内置，对用户是**零前置**——比 Homebrew 在 macOS 上还友好。
/// > 两者都不在场时用户仍有官网那条路，故不为 scoop/choco/MacPorts 再列一份）。
#[cfg(target_os = "macos")]
const PACKAGE_MANAGER: Option<(&str, &str)> = Some(("Homebrew", "brew install"));
/// 同上（Windows）。
#[cfg(target_os = "windows")]
const PACKAGE_MANAGER: Option<(&str, &str)> = Some(("winget", "winget install"));
/// 同上（其余平台：不给命令，只给官网）。
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const PACKAGE_MANAGER: Option<(&str, &str)> = None;

/// 一件工具在**当前平台**上的安装引导。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstallGuide {
    /// 包管理器展示名（`None` = 本平台不给一行命令）。
    pub manager: Option<String>,
    /// 可复制的一行安装命令（`None` 同上）。
    pub command: Option<String>,
    /// 官方安装说明页（**恒有**）。
    pub docs_url: String,
}

/// 取一件工具在当前平台的安装引导（**纯函数**，可单测）。
#[must_use]
pub fn install_guide(tool: ExternalTool) -> InstallGuide {
    let (manager, command) = match (PACKAGE_MANAGER, tool.package()) {
        (Some((manager, verb)), Some(package)) => {
            (Some(manager.to_owned()), Some(format!("{verb} {package}")))
        }
        _ => (None, None),
    };
    InstallGuide {
        manager,
        command,
        docs_url: tool.docs_url().to_owned(),
    }
}

/// 抓取工具的可用性快照（前端据此决定给输入框还是给安装引导）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolStatus {
    /// yt-dlp 绝对路径（`None` = 未找到）。
    pub tool_path: Option<PathBuf>,
    /// yt-dlp 自报版本（`yt-dlp --version`，通常形如 `2026.07.20`）；未跑通时为 `None`。
    pub version: Option<String>,
    /// 版本探测**超时**了（`None` 版本的一种，且与"坏了"是两回事）。
    ///
    /// 真机教训：官方的 `yt-dlp_macos` 是 **PyInstaller onefile**，每次启动都要把整个
    /// Python 运行时解压出来——实测 `--version` **恒需 ~8 秒**（不是首次才慢）。初版按
    /// "本地进程自报版本应该很快"设了 3 秒超时，于是一份**完全可用**的工具被恒定误判为
    /// 「无法运行（可能是权限不足或文件损坏）」，用户反馈"装完还是让我装、过一会儿又不可用"。
    /// 超时与真失败必须分开：前者该说"再等等/重试"，后者才该说"重装"。
    pub probe_timed_out: bool,
    /// ffmpeg 是否在场——决定视频档能否取「分离流 + 合并」的高分辨率（[`crate::format`]）。
    pub ffmpeg: bool,
    /// ffmpeg 的绝对路径（`None` = 未找到）。工具管理面板据此显示"装在哪儿"。
    pub ffmpeg_path: Option<PathBuf>,
    /// 找到的 JS 运行时（`None` = 一个都没有）。缺它不是错误而是**降级**：站点提取会退到
    /// 不需要 JS 挑战的客户端，成功率随站点策略摇摆（见 [`JS_RUNTIMES`]）。
    pub js_runtime: Option<JsRuntime>,
}

/// 一个可用的 JS 运行时（名字 + 绝对路径）。
///
/// 名字是**传给 yt-dlp `--js-runtimes` 的那个**（`deno` / `node` / `bun`），路径必须是绝对的
/// ——GUI 应用的 `PATH` 陷阱对它同样成立（见本模块顶部）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct JsRuntime {
    /// yt-dlp 认的运行时名。
    pub name: String,
    /// 可执行文件绝对路径。
    pub path: PathBuf,
}

impl ToolStatus {
    /// 是否可以开始抓取（找到且能跑）。
    #[must_use]
    pub fn ready(&self) -> bool {
        self.tool_path.is_some() && self.version.is_some()
    }
}

/// 候选路径生成（**纯函数**，可单测）：显式路径 → home 候选 → 系统候选 → `PATH` 逐项。
///
/// 只负责"按什么顺序看哪些位置"，是否存在交给调用方的谓词——真机的文件系统不进单测。
#[must_use]
pub fn candidates(
    names: &[&str],
    explicit: Option<&Path>,
    managed: Option<&Path>,
    home: Option<&Path>,
    path_var: Option<&str>,
) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut push = |p: PathBuf| {
        if !out.contains(&p) {
            out.push(p);
        }
    };
    // 用户显式指定优先且**原样使用**：他可能装在任何地方，我们没有资格改写。
    if let Some(explicit) = explicit.filter(|p| !p.as_os_str().is_empty()) {
        push(explicit.to_path_buf());
    }
    // **Lunarca 受管的那份**（应用代装，ADR-023 修订）排在系统各处之前：装了它说明用户选择让
    // Lunarca 管理工具版本，而它是"点安装那一刻的 latest"；系统里 brew 装的那份可能几个月
    // 没 `brew upgrade`。显式设置仍可覆盖，且界面如实显示当前在用的是哪一份。
    if let Some(managed) = managed.filter(|p| !p.as_os_str().is_empty()) {
        push(managed.to_path_buf());
    }
    if let Some(home) = home {
        for dir in HOME_BIN_DIRS {
            for name in names {
                push(home.join(dir).join(name));
            }
        }
    }
    for dir in EXTRA_BIN_DIRS {
        for name in names {
            push(Path::new(dir).join(name));
        }
    }
    // PATH 放最后：GUI 下它最贫瘠，终端下它与上面多有重合（已按 `contains` 去重）。
    if let Some(path_var) = path_var {
        for dir in std::env::split_paths(path_var) {
            if dir.as_os_str().is_empty() {
                continue;
            }
            for name in names {
                push(dir.join(name));
            }
        }
    }
    out
}

/// 在候选里挑第一个**存在且是文件**的路径。
fn first_existing(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates.iter().find(|p| p.is_file()).cloned()
}

/// 定位 yt-dlp（`explicit` 来自 `transcribe.fetch_tool_path`，空则自动发现）。
#[must_use]
pub fn locate_yt_dlp(explicit: Option<&Path>, managed: Option<&Path>) -> Option<PathBuf> {
    let home = home_dir();
    let path_var = std::env::var("PATH").ok();
    first_existing(&candidates(
        YT_DLP_NAMES,
        explicit,
        managed,
        home.as_deref(),
        path_var.as_deref(),
    ))
}

/// **通用的可执行文件定位**（任何外部工具适用，不限于抓取域）。
///
/// 之所以放在这里而不是各处自写一份：本模块顶部那段「GUI 应用继承的是 launchd 最小 `PATH`」
/// 的知识对**每一件**外部工具都成立——裸 `Command::new("ffmpeg")` 从 Finder 启动时同样会
/// 找不到 brew 装的那份。发现逻辑只有一处，几件工具才会有一致的行为。
#[must_use]
pub fn locate_executable(names: &[&str]) -> Option<PathBuf> {
    let home = home_dir();
    let path_var = std::env::var("PATH").ok();
    first_existing(&candidates(
        names,
        None,
        None,
        home.as_deref(),
        path_var.as_deref(),
    ))
}

/// 定位 ffmpeg（只影响视频档的格式选择，缺失不是错误）。
#[must_use]
pub fn locate_ffmpeg() -> Option<PathBuf> {
    let home = home_dir();
    let path_var = std::env::var("PATH").ok();
    first_existing(&candidates(
        FFMPEG_NAMES,
        None,
        // ffmpeg 暂无受管安装（它不是单文件自包含档，见 ADR-023）。
        None,
        home.as_deref(),
        path_var.as_deref(),
    ))
}

/// 定位一个 JS 运行时（**受管那份优先**，其余按 [`JS_RUNTIMES`] 的优先级取第一个装了的）。
///
/// 与 ffmpeg 同理：缺失不是错误，是能力分档。但与 ffmpeg 不同的是，它影响的不是画质而是
/// **能不能取到流**，故界面上的措辞要更强（「建议安装」而非 ffmpeg 的「可选」）。
///
/// `managed` 是 Lunarca 代装的那份 Deno（[`crate::install::managed_js_runtime_rel_path`]）。
/// 它排第一，理由与 yt-dlp 那份完全相同：装了它即表示用户选择让 Lunarca 管这件事。
/// **同时它也是"合并安装"成立的前提**——代装完之后必须立刻能被发现，否则用户点了
/// 「让 Lunarca 安装」、磁盘上多了 77 MB，界面却还写着"建议安装"。
#[must_use]
pub fn locate_js_runtime(managed: Option<&Path>) -> Option<JsRuntime> {
    if let Some(managed) = managed.filter(|p| p.is_file()) {
        return Some(JsRuntime {
            // 受管那份就是 Deno（我们只代装它），故名字是确定的，不必去问它自己。
            name: JS_RUNTIMES[0].0.to_owned(),
            path: managed.to_path_buf(),
        });
    }
    JS_RUNTIMES.iter().find_map(|(name, exe)| {
        locate_executable(&[exe]).map(|path| JsRuntime {
            name: (*name).to_owned(),
            path,
        })
    })
}

/// 用户 home（`HOME` / Windows `USERPROFILE`）。
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

/// 探测工具状态（跑一次 `--version`，**不联网**；超时取 [`PROBE_TIMEOUT`]）。
///
/// 找到文件但跑不起来（权限 / 架构 / 损坏）时 `version` 为 `None`——这与"没装"是两种不同的
/// 用户处境，界面上的引导也不同，故不合并。
pub async fn status(
    explicit: Option<&Path>,
    managed: Option<&Path>,
    managed_js: Option<&Path>,
) -> ToolStatus {
    status_with_timeout(explicit, managed, managed_js, PROBE_TIMEOUT).await
}

/// 同 [`status`]，但**版本探测的超时由调用方给定**。
///
/// 与 [`crate::probe::probe_with_timeout`] 同一理由：25 秒是按 PyInstaller onefile 的真实
/// 解压耗时定的，假工具却是毫秒级——测试继承生产值，就等于把「非零退出是坏了、不是超时」
/// 这条断言押在"机器 25 秒内调度得完子进程"上。给生产多一个旋钮不是目的，可测才是。
pub async fn status_with_timeout(
    explicit: Option<&Path>,
    managed: Option<&Path>,
    managed_js: Option<&Path>,
    version_timeout: std::time::Duration,
) -> ToolStatus {
    let tool_path = locate_yt_dlp(explicit, managed);
    let (version, probe_timed_out) = match tool_path.as_deref() {
        Some(path) => probe_version(path, version_timeout).await,
        None => (None, false),
    };
    let ffmpeg_path = locate_ffmpeg();
    ToolStatus {
        tool_path,
        version,
        probe_timed_out,
        ffmpeg: ffmpeg_path.is_some(),
        ffmpeg_path,
        js_runtime: locate_js_runtime(managed_js),
    }
}

/// 版本探测超时。
///
/// **绝不能按"本地进程应该很快"来定**：`yt-dlp_macos` 是 PyInstaller onefile，
/// 每次启动都要解压整个 Python 运行时，实测 `--version` 恒需 ~8 秒。初版取 3 秒，
/// 把一份完全可用的工具恒定判成"跑不起来"。25 秒给慢机器与高负载留足余量；
/// 之所以敢给这么长，是因为调用方对结果做了缓存（应用生命周期内只付一次）。
pub const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(25);

/// 跑 `<tool> --version` 取版本串。
///
/// 返回 `(版本, 是否超时)`：**超时不等于坏**——它可能只是机器忙或首次解压慢，
/// 界面该给"重试"而不是"重装"。
async fn probe_version(tool: &Path, timeout: std::time::Duration) -> (Option<String>, bool) {
    let output = tokio::process::Command::new(tool)
        .arg("--version")
        .kill_on_drop(true)
        .output();
    let Ok(output) = tokio::time::timeout(timeout, output).await else {
        return (None, true);
    };
    let Ok(output) = output else {
        return (None, false); // 起不来（权限/架构/损坏）
    };
    if !output.status.success() {
        return (None, false);
    }
    let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    ((!version.is_empty()).then_some(version), false)
}

/// 已就绪的抓取工具（探测/下载入口一律先过此校验，故下游无需重复判空）。
#[derive(Clone, Debug)]
pub struct FetchTool {
    /// yt-dlp 绝对路径。
    pub path: PathBuf,
    /// ffmpeg 的绝对路径（在场才能取「分离流 + 合并」的高分辨率；`None` = 没找到）。
    pub ffmpeg: Option<PathBuf>,
    /// 代理（`transcribe.fetch_proxy`，空 = 不显式指定）。
    pub proxy: Option<String>,
    /// 已定位的 JS 运行时（`None` = 一个都没装 → 站点提取降级，见 [`JS_RUNTIMES`]）。
    pub js_runtime: Option<JsRuntime>,
}

impl FetchTool {
    /// 由设置解析出可用工具。
    ///
    /// # Errors
    /// 未找到返回 [`FetchError::ToolMissing`]；找到但 `--version` 跑不通返回
    /// [`FetchError::ToolUnusable`]（两者的用户动作不同：装 vs 修）。
    pub fn resolve(
        explicit: Option<&Path>,
        managed: Option<&Path>,
        managed_js: Option<&Path>,
        proxy: Option<&str>,
    ) -> Result<Self, FetchError> {
        // **只定位、不探版本**：探一次要 ~8 秒（PyInstaller onefile 每次启动都解压运行时），
        // 而每条抓取任务都要先 resolve —— 那是纯粹的白等。工具真坏了，下载会立刻失败并由
        // `classify_stderr` 给出准确原因；提前探测的价值只在**界面**（`status`，带缓存）。
        let path = locate_yt_dlp(explicit, managed).ok_or(FetchError::ToolMissing)?;
        Ok(Self {
            path,
            ffmpeg: locate_ffmpeg(),
            proxy: proxy
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(str::to_owned),
            js_runtime: locate_js_runtime(managed_js),
        })
    }

    /// 每次调用共享的基础参数：禁自更新、不读用户配置文件、显式代理、显式 JS 运行时。
    ///
    /// `--ignore-config` 是安全与可复现的双重要求：用户 `~/.config/yt-dlp/config` 里的
    /// `--exec`、`-o`、`--postprocessor-args` 会静默改变我们的落点与行为，而 Lunarca 承诺
    /// "落到 Vault 里这一份"。`--no-update` 则杜绝工具在我们背后自我升级（联网行为必须显式）。
    ///
    /// **刻意不传 `--no-warnings`**：yt-dlp 把「我没找到 JS 运行时、于是回落到一个不需要
    /// 解挑战的播放器客户端」这件事**只写在警告里**。屏蔽掉它，一次真实失败在我们眼里就只剩
    /// 一句 `HTTP Error 403`——分类器认不出，用户拿到的是无信息的"稍后重试"，而正确的动作是
    /// 更新工具或装个运行时（2026-08-23 真机缺陷）。警告只进
    /// [`classify_stderr`](crate::error::classify_stderr)，既不回显也不入日志（TDD-12），
    /// 故留着它没有隐私代价，却是唯一的诊断依据。
    #[must_use]
    pub fn base_args(&self) -> Vec<String> {
        let mut args: Vec<String> = vec![
            "--ignore-config".into(),
            "--no-update".into(),
            "--no-colors".into(),
            "--no-progress".into(),
        ];
        if let Some(proxy) = &self.proxy {
            args.push("--proxy".into());
            args.push(proxy.clone());
        }
        if let Some(ffmpeg) = &self.ffmpeg {
            // 显式告诉 yt-dlp ffmpeg 在哪：GUI 应用继承的是 launchd 的最小 PATH（不含
            // `/opt/homebrew/bin`），我们找得到 ffmpeg、选了要合并的格式，yt-dlp 自己却找不到，
            // 于是分离的音画流不合并（ADR-167 ⑤ 核对时发现）。
            args.push("--ffmpeg-location".into());
            args.push(ffmpeg.display().to_string());
        }
        if let Some(runtime) = &self.js_runtime {
            // 先 `--no-js-runtimes` 清空默认（yt-dlp 默认只启用 deno 且**去 PATH 里找**），
            // 再显式启用我们已经解析到绝对路径的那一个——GUI 应用的最小 PATH 里通常一个都没有，
            // 于是"用户明明装了 node"却仍被判定为无运行时（与本模块顶部同一条陷阱）。
            args.push("--no-js-runtimes".into());
            args.push("--js-runtimes".into());
            args.push(format!("{}:{}", runtime.name, runtime.path.display()));
        }
        args
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_js_runtime_source_follows_the_runtime_in_use() {
        assert_eq!(js_runtime_source("node"), "nodejs.org");
        assert_eq!(js_runtime_source("bun"), "bun.sh");
        assert_eq!(js_runtime_source("deno"), ExternalTool::JsRuntime.source());
        // 三种运行时都是 MIT：许可证不必随在用的那一种变。
        assert_eq!(ExternalTool::JsRuntime.license(), "MIT");
    }

    /// 发现顺序即优先级；`/opt/homebrew/bin` 必须在 `PATH` 之前被看到——GUI 下 PATH 恰恰
    /// 没有它，这条顺序就是"brew 装了却找不到"缺陷的根治点。
    // Unix 路径写法与 `:` PATH 分隔符；Windows 的候选次序另需用例（未做）。
    #[cfg(unix)]
    #[test]
    fn candidate_order_puts_explicit_then_package_dirs_then_path() {
        let list = candidates(
            &["yt-dlp"],
            Some(Path::new("/custom/yt-dlp")),
            Some(Path::new("/Models/tools/yt-dlp")),
            Some(Path::new("/Users/me")),
            Some("/usr/bin:/sbin"),
        );
        let as_str: Vec<String> = list.iter().map(|p| p.display().to_string()).collect();
        assert_eq!(as_str[0], "/custom/yt-dlp", "显式路径最优先");
        let brew = as_str
            .iter()
            .position(|p| p == "/opt/homebrew/bin/yt-dlp")
            .expect("含 Homebrew 落点");
        let home = as_str
            .iter()
            .position(|p| p == "/Users/me/.local/bin/yt-dlp")
            .expect("含 pipx 落点");
        let sbin = as_str
            .iter()
            .position(|p| p == "/sbin/yt-dlp")
            .expect("含 PATH 逐项");
        assert!(home < brew, "home 候选先于系统候选");
        assert!(brew < sbin, "包管理器落点先于 PATH（GUI 下 PATH 最贫瘠）");
    }

    /// **受管那份排在系统各处之前**（ADR-023 修订）：装了 Lunarca 管理的工具即表示用户选择让
    /// 应用管版本，而它是安装那刻的 latest；brew 里那份可能几个月没 upgrade。显式设置仍最优先。
    // Unix 路径写法与 `:` PATH 分隔符；Windows 的候选次序另需用例（未做）。
    #[cfg(unix)]
    #[test]
    fn managed_install_outranks_system_locations_but_not_explicit() {
        let list = candidates(
            &["yt-dlp"],
            Some(Path::new("/custom/yt-dlp")),
            Some(Path::new("/Models/tools/yt-dlp")),
            Some(Path::new("/Users/me")),
            Some("/opt/homebrew/bin"),
        );
        let as_str: Vec<String> = list.iter().map(|p| p.display().to_string()).collect();
        let idx = |needle: &str| {
            as_str
                .iter()
                .position(|p| p == needle)
                .unwrap_or_else(|| panic!("缺候选 {needle}"))
        };
        assert_eq!(as_str[0], "/custom/yt-dlp", "显式设置最优先");
        assert!(
            idx("/Models/tools/yt-dlp") < idx("/Users/me/.local/bin/yt-dlp"),
            "受管先于 home 候选"
        );
        assert!(
            idx("/Models/tools/yt-dlp") < idx("/opt/homebrew/bin/yt-dlp"),
            "受管先于包管理器落点"
        );
    }

    /// PATH 与固定候选重合时不得产出重复项（否则 `is_file()` 会被多探几次）。
    #[test]
    fn candidates_deduplicate_across_sources() {
        let list = candidates(
            &["yt-dlp"],
            Some(Path::new("/usr/local/bin/yt-dlp")),
            None,
            None,
            Some("/usr/local/bin:/opt/homebrew/bin"),
        );
        let count = list
            .iter()
            .filter(|p| p.as_path() == Path::new("/usr/local/bin/yt-dlp"))
            .count();
        assert_eq!(count, 1, "同一路径只出现一次");
    }

    /// 空的显式路径（设置项默认值）不得产出 `""` 这个荒谬候选。
    #[test]
    fn empty_explicit_path_is_ignored() {
        let list = candidates(
            &["yt-dlp"],
            Some(Path::new("")),
            Some(Path::new("")),
            None,
            None,
        );
        assert!(list.iter().all(|p| !p.as_os_str().is_empty()));
    }

    /// PATH 里的空项（`"a::b"` 的中段）同样不得变成相对候选。
    // Unix 路径写法与 `:` PATH 分隔符；Windows 的候选次序另需用例（未做）。
    #[cfg(unix)]
    #[test]
    fn empty_path_entries_are_skipped() {
        let list = candidates(&["yt-dlp"], None, None, None, Some("/a::/b"));
        assert!(
            list.iter().all(|p| p.is_absolute()),
            "空 PATH 项会产出相对路径 `yt-dlp`，那会在当前工作目录里找工具"
        );
    }

    #[test]
    fn status_ready_requires_both_path_and_version() {
        let missing = ToolStatus {
            tool_path: None,
            version: None,
            probe_timed_out: false,
            ffmpeg: false,
            ffmpeg_path: None,
            js_runtime: None,
        };
        assert!(!missing.ready());
        let unusable = ToolStatus {
            tool_path: Some(PathBuf::from("/x/yt-dlp")),
            version: None,
            ..missing.clone()
        };
        assert!(!unusable.ready(), "跑不起来 ≠ 就绪");
        let ok = ToolStatus {
            version: Some("2026.07.20".into()),
            ..unusable
        };
        assert!(ok.ready());
        // JS 运行时**不进就绪判据**：缺它是降级不是故障，把它算进 `ready` 会让
        // 「从链接导入」整个对话框退回安装引导页，而绝大多数站点根本不需要它。
        let no_runtime = ToolStatus {
            js_runtime: None,
            ..ok
        };
        assert!(no_runtime.ready());
    }

    /// 基础参数必须恒含 `--ignore-config`（用户配置文件可改落点/加 --exec）与 `--no-update`。
    #[test]
    fn base_args_pin_config_and_updates_and_carry_proxy() {
        let tool = FetchTool {
            path: PathBuf::from("/x/yt-dlp"),
            ffmpeg: None,
            proxy: Some("socks5://127.0.0.1:1080".into()),
            js_runtime: None,
        };
        let args = tool.base_args();
        assert!(args.contains(&"--ignore-config".to_string()));
        assert!(args.contains(&"--no-update".to_string()));
        let idx = args.iter().position(|a| a == "--proxy").expect("代理在场");
        assert_eq!(args[idx + 1], "socks5://127.0.0.1:1080");

        let no_proxy = FetchTool {
            proxy: None,
            ..tool
        };
        assert!(!no_proxy.base_args().contains(&"--proxy".to_string()));
    }

    /// **绝不再传 `--no-warnings`**：它屏蔽掉的正是"缺 JS 运行时 → 回落 → 被站点 403"
    /// 这条唯一线索，于是一次可解释的失败被压成无信息的"稍后重试"（2026-08-23 真机缺陷）。
    #[test]
    fn base_args_never_silence_warnings() {
        let tool = FetchTool {
            path: PathBuf::from("/x/yt-dlp"),
            ffmpeg: None,
            proxy: None,
            js_runtime: None,
        };
        assert!(
            !tool.base_args().contains(&"--no-warnings".to_string()),
            "警告是失败分类的唯一依据，不得屏蔽"
        );
    }

    /// 找到运行时就**显式按绝对路径**启用它：先清默认（yt-dlp 只启用 deno 且去 PATH 找），
    /// 再指名道姓——否则 GUI 下的最小 PATH 会让"装了也等于没装"。
    #[test]
    fn base_args_pin_the_located_js_runtime_by_absolute_path() {
        let tool = FetchTool {
            path: PathBuf::from("/x/yt-dlp"),
            ffmpeg: None,
            proxy: None,
            js_runtime: Some(JsRuntime {
                name: "node".into(),
                path: PathBuf::from("/opt/homebrew/bin/node"),
            }),
        };
        let args = tool.base_args();
        let clear = args
            .iter()
            .position(|a| a == "--no-js-runtimes")
            .expect("先清空默认");
        let enable = args
            .iter()
            .position(|a| a == "--js-runtimes")
            .expect("再显式启用");
        assert!(
            clear < enable,
            "清空必须在启用之前，否则默认的 deno 仍占优先级"
        );
        assert_eq!(args[enable + 1], "node:/opt/homebrew/bin/node");
    }

    /// 找到 ffmpeg 就把它的绝对路径交给 yt-dlp：它自己只会去 PATH 里找，从访达启动时找不到。
    #[test]
    fn base_args_pass_the_located_ffmpeg_by_absolute_path() {
        let tool = FetchTool {
            path: PathBuf::from("/x/yt-dlp"),
            ffmpeg: Some(PathBuf::from("/opt/homebrew/bin/ffmpeg")),
            proxy: None,
            js_runtime: None,
        };
        let args = tool.base_args();
        let at = args
            .iter()
            .position(|a| a == "--ffmpeg-location")
            .expect("带上 ffmpeg 的位置");
        assert_eq!(args[at + 1], "/opt/homebrew/bin/ffmpeg");
        let without = FetchTool {
            ffmpeg: None,
            ..tool
        };
        assert!(!without
            .base_args()
            .contains(&"--ffmpeg-location".to_owned()));
    }

    /// 一个运行时都没有时**不动 yt-dlp 的默认**：传 `--no-js-runtimes` 只会把"用户 PATH 里
    /// 恰好有 deno"这条最后的可能也掐掉。
    #[test]
    fn base_args_leave_defaults_alone_without_a_runtime() {
        let tool = FetchTool {
            path: PathBuf::from("/x/yt-dlp"),
            ffmpeg: None,
            proxy: None,
            js_runtime: None,
        };
        let args = tool.base_args();
        assert!(!args.iter().any(|a| a == "--no-js-runtimes"));
        assert!(!args.iter().any(|a| a == "--js-runtimes"));
    }

    /// 受管那份 JS 运行时必须**优先于系统里的任何一份**被发现——这是"合并安装"成立的前提：
    /// 代装完之后要立刻可见，否则用户点了安装、磁盘多了 77 MB，界面还写着"建议安装"。
    #[test]
    fn managed_js_runtime_outranks_system_ones() {
        let dir = tempfile::tempdir().expect("tempdir");
        let managed = dir.path().join("deno");
        std::fs::write(&managed, b"#!/bin/sh\n").expect("write");
        let found = locate_js_runtime(Some(&managed)).expect("找到受管那份");
        assert_eq!(found.path, managed);
        assert_eq!(found.name, "deno", "名字必须是 yt-dlp 认的那个");

        // 受管路径不存在时回落到系统发现（不因为传了个路径就当它在场）。
        let missing = dir.path().join("not-installed");
        assert!(locate_js_runtime(Some(&missing)).is_none_or(|rt| rt.path != missing));
    }

    /// **每件工具都必须有引导**：新增一件却忘了给它安装说明，界面上就是一行"未安装"
    /// 加一个死胡同。官网恒有；而只要本平台有包管理器，就每件都得有包名——漏一个的表现是
    /// 那一件悄悄退化成"只给官网"，没有任何门禁会红。
    #[test]
    fn every_tool_has_a_guide_on_this_platform() {
        for tool in ExternalTool::ALL {
            let guide = install_guide(*tool);
            assert!(
                guide.docs_url.starts_with("https://"),
                "{tool:?} 的官方说明页必须是 https"
            );
            assert_eq!(
                guide.command.is_some(),
                PACKAGE_MANAGER.is_some(),
                "{tool:?}：本平台有包管理器就必须有一行命令，没有就一条都不给"
            );
            if let Some(command) = &guide.command {
                let manager = guide.manager.as_deref().expect("有命令必有管理器名");
                assert!(!manager.is_empty());
                assert!(command.contains(" install "), "命令形态不对：{command}");
                assert!(!command.ends_with("install "), "包名为空：{command}");
            }
        }
    }

    /// 命令是**环境事实**（随平台变、不随语言变），故它的唯一事实源在这里而不是 i18n。
    /// 这条断言把"当前平台该用哪个包管理器"钉死——写成 `brew` 的那份曾经让 Windows 用户
    /// 复制到一条必然失败的命令。
    #[test]
    fn install_command_matches_this_platform_package_manager() {
        let guide = install_guide(ExternalTool::Ffmpeg);
        if cfg!(target_os = "macos") {
            assert_eq!(guide.command.as_deref(), Some("brew install ffmpeg"));
            assert_eq!(guide.manager.as_deref(), Some("Homebrew"));
        } else if cfg!(target_os = "windows") {
            assert_eq!(guide.command.as_deref(), Some("winget install Gyan.FFmpeg"));
            assert_eq!(guide.manager.as_deref(), Some("winget"));
        } else {
            assert_eq!(guide.command, None, "不猜发行版包管理器，只给官网");
        }
    }

    /// 工具 id 是 IPC 契约（前端按它选文案与图标），改名即破坏前端；且必须两两不同。
    #[test]
    fn external_tool_ids_are_stable_and_unique() {
        let ids: Vec<&str> = ExternalTool::ALL.iter().map(|t| t.as_str()).collect();
        assert_eq!(ids, ["yt_dlp", "ffmpeg", "js_runtime"]);
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "id 必须唯一");
    }

    /// 运行时优先级即 [`JS_RUNTIMES`] 的书写顺序（与 yt-dlp 自己的一致），且名字是
    /// **yt-dlp 认的那个**——写错名字会让 `--js-runtimes` 直接报参数错误。
    #[test]
    fn js_runtime_table_follows_yt_dlp_priority() {
        let names: Vec<&str> = JS_RUNTIMES.iter().map(|(name, _)| *name).collect();
        assert_eq!(names, ["deno", "node", "bun"]);
        for (name, exe) in JS_RUNTIMES {
            assert!(exe.starts_with(name), "可执行名应以运行时名为词干：{exe}");
        }
    }
}
