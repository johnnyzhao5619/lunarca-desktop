//! 抓取工具的受管安装执行（下载 → 校验 → `chmod +x` → 原子落位，ADR-023 修订）。
//!
//! 纯判定（选资产 / 解析 release / 取校验和）在 [`luna_fetch::install`]；本模块只做 IO 与事件
//! ——与模型下载（`install_luna_managed_model`）同一分层，故进度也复用同一套
//! `model://download` 事件，前端不必为工具再造一条进度通道。
//!
//! 三条与模型下载不同的纪律：
//! ① **校验和不能硬编码**（yt-dlp 一年数十版）→ 每次装都从**同一 release** 取 `SHA2-256SUMS`；
//! ② 产物是**可执行文件** → 落位后 `chmod +x`，且**先校验后改名**（校验失败的字节永不出现在
//!    最终路径上，绝不给出一个"半个工具"）；
//! ③ 落**应用私有目录**（`<models_root>/tools/`）而非系统路径 → 不需要管理员权限、
//!    不碰用户的 PATH、卸载 Lunarca 即一并消失。
//!
//! 隐私与安全：这是**用户显式点击**才发生的联网行为（AGENTS.md §7），受严格本地模式硬拦截；
//! 只访问 GitHub 固定端点，不带任何用户数据。

use std::path::{Path, PathBuf};

use luna_fetch::install;
use tauri_specta::Event as _;

use crate::ipc::ModelDownloadProgress;

/// 受管工具安装/更新的进度与结果对外用的 id（前端据此把进度条挂到工具那一行）。
pub const TOOL_ID: &str = "yt-dlp";

/// 受管 **JS 运行时**的进度 id。
///
/// **两件工具必须各用各的 id**：合并安装是"一次点击、两个下载"，共用一个 id 会让进度条
/// 在第二件开始时莫名归零，而界面无法说出此刻在下的是哪一件（下载 38 MB 与解压 77 MB
/// 是两段可感知的等待，不解释就是"卡住了"）。
pub const JS_RUNTIME_TOOL_ID: &str = "deno";

/// GitHub API 要求带 User-Agent，否则 403。用应用名而非伪装浏览器（诚实标识）。
const USER_AGENT: &str = concat!(
    "Lunarca/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/johnnyzhao5619/lunarca-desktop)"
);

/// 版本探测缓存（进程内）。
///
/// **为什么必须缓存**：`yt-dlp_macos` 是 PyInstaller onefile，每次启动都要解压整个
/// Python 运行时——实测 `--version` 恒需 ~8 秒。而"打开从链接导入对话框"与"进设置页"
/// 都要看工具状态，不缓存就是每次让用户对着"正在检测…"等 8 秒。
///
/// 缓存键是**文件身份**（路径 + 修改时间 + 大小）而不是路径本身：重装、更新、或用户
/// 换了一份工具时键自然变化，缓存自动失效——不需要谁记得去清它。
static PROBE_CACHE: std::sync::Mutex<Option<(ToolIdentity, luna_fetch::ToolStatus)>> =
    std::sync::Mutex::new(None);

/// 被探测文件的身份（路径 + mtime + 体积）。
type ToolIdentity = (PathBuf, Option<std::time::SystemTime>, u64);

/// 取某个工具文件的身份；文件不在时为 `None`。
fn tool_identity(path: &Path) -> Option<ToolIdentity> {
    let meta = std::fs::metadata(path).ok()?;
    Some((path.to_path_buf(), meta.modified().ok(), meta.len()))
}

/// 带缓存的工具状态探测（界面用）。`force` = 用户点了「重新检测」，无视缓存。
///
/// 缓存只对"探到了版本"的结果生效：超时或失败**不缓存**——那两种状态该在用户重试时
/// 重新判断，把一次偶发的超时钉死成"这工具坏了"正是初版的病根。
pub async fn cached_status(
    explicit: Option<&Path>,
    models_root: &Path,
    force: bool,
) -> luna_fetch::ToolStatus {
    let managed = managed_path(models_root);
    let managed = managed.as_path();
    let managed_js = managed_js_runtime_path(models_root);
    let managed_js = managed_js.as_path();
    let located = luna_fetch::tool::locate_yt_dlp(explicit, Some(managed));
    // 选中的就是受管那份、且凭证仍然有效 → **直接给结果，一秒不等**。
    // 官方 yt-dlp 是 PyInstaller onefile，跑一次 `--version` 恒需约 8 秒（每次启动都要
    // 解压运行时）；而这份二进制是我们自己装的、装完当场验过，没有理由每次开界面再验一遍。
    if !force && located.as_deref() == Some(managed) {
        if let Some(version) = verified_version(models_root) {
            let ffmpeg_path = luna_fetch::tool::locate_ffmpeg();
            return luna_fetch::ToolStatus {
                tool_path: located,
                version: Some(version),
                probe_timed_out: false,
                ffmpeg: ffmpeg_path.is_some(),
                ffmpeg_path,
                // 与 ffmpeg 同一条：本地文件系统查找，毫秒级，不值得也不该缓存
                //（用户刚装完 deno 就该在下一次开界面时看到它）。
                js_runtime: luna_fetch::tool::locate_js_runtime(Some(managed_js)),
            };
        }
    }
    let identity = located.as_deref().and_then(tool_identity);
    if !force {
        if let (Some(identity), Ok(cache)) = (identity.as_ref(), PROBE_CACHE.lock()) {
            if let Some((cached_id, status)) = cache.as_ref() {
                if cached_id == identity {
                    return status.clone();
                }
            }
        }
    }
    let status = luna_fetch::tool::status(explicit, Some(managed), Some(managed_js)).await;
    if status.version.is_some() {
        if let (Some(identity), Ok(mut cache)) = (identity, PROBE_CACHE.lock()) {
            *cache = Some((identity, status.clone()));
        }
    }
    status
}

/// 上一次「检查更新」的结论（进程内，ADR-168 ⑧）：组件 id（`yt_dlp` / `js_runtime`）→ 有没有新版。
///
/// 不落盘、不后台刷新：只有用户点了「检查更新」（应用页或组件面板，两处写同一份）才会有值，
/// 重启就忘。装上、更新、卸载那一件都把它记回「没有新版」——否则刚更新完按钮还写着「更新」。
static UPDATES: std::sync::Mutex<Vec<(&'static str, bool)>> = std::sync::Mutex::new(Vec::new());

/// 记下一件组件有没有新版。
pub fn record_update(component: install::PlannedComponent, available: bool) {
    if let Ok(mut updates) = UPDATES.lock() {
        let id = component.as_str();
        updates.retain(|(known, _)| *known != id);
        updates.push((id, available));
    }
}

/// 上一次检查说这件组件有新版（没查过一律为否）。
#[must_use]
pub fn known_update(component: install::PlannedComponent) -> bool {
    UPDATES.lock().is_ok_and(|updates| {
        updates
            .iter()
            .any(|(id, available)| *id == component.as_str() && *available)
    })
}

/// 查 Lunarca 下载的那几件有没有新版并记下（**一次联网**，只在用户点「检查更新」时调；严格本地
/// 由调用方拦）。不是 Lunarca 装的不查——用户自己的 Homebrew 由他自己 upgrade。
///
/// # Errors
/// 联网失败或上游发布物不符预期时返回稳定错误码。
pub async fn check_updates(models_root: &Path) -> Result<(), String> {
    if managed_path(models_root).is_file() {
        let (_, available) = check_update(models_root).await?;
        record_update(install::PlannedComponent::YtDlp, available);
    }
    if let Some((_, available)) = check_js_runtime_update(models_root).await? {
        record_update(install::PlannedComponent::JsRuntime, available);
    }
    Ok(())
}

/// 受管工具的绝对落点。
#[must_use]
pub fn managed_path(models_root: &Path) -> PathBuf {
    models_root.join(install::managed_rel_path())
}

/// 受管 JS 运行时（Deno）的绝对落点。
#[must_use]
pub fn managed_js_runtime_path(models_root: &Path) -> PathBuf {
    models_root.join(install::managed_js_runtime_rel_path())
}

/// 已装版本（展示用：界面上的"由 Lunarca 管理 X"与「安装 / 重新安装」的措辞）。
#[must_use]
pub fn installed_version(models_root: &Path) -> Option<String> {
    // 二进制不在了就当没装——旁文件可能残留（用户手删了工具但没删版本记录）。
    if !managed_path(models_root).is_file() {
        return None;
    }
    std::fs::read_to_string(models_root.join(install::VERSION_SIDECAR))
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

/// 受管 JS 运行时的已装版本（二进制不在了就当没装；本轮之前装的那份没有记录，也是 `None`）。
#[must_use]
pub fn installed_js_runtime_version(models_root: &Path) -> Option<String> {
    if !managed_js_runtime_path(models_root).is_file() {
        return None;
    }
    std::fs::read_to_string(models_root.join(install::JS_RUNTIME_VERSION_SIDECAR))
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

/// 查受管 JS 运行时有没有更新：不是 Lunarca 装的（用户自己的 node / bun / deno）不查、返回
/// `None`；否则返回 `(上游最新版, 是否比已装的新)`。没有版本记录的旧安装一律算可更新。
///
/// # Errors
/// 联网失败或上游发布物不符预期时返回稳定错误码。
pub async fn check_js_runtime_update(models_root: &Path) -> Result<Option<(String, bool)>, String> {
    if !managed_js_runtime_path(models_root).is_file() {
        return Ok(None);
    }
    let asset = install::js_runtime_platform_asset();
    if asset.is_empty() {
        return Ok(None);
    }
    let plan = latest_plan(
        &http_client()?,
        install::JS_RUNTIME_LATEST_RELEASE_API,
        asset,
        &install::js_runtime_checksum_asset(asset),
    )
    .await?;
    let installed = installed_js_runtime_version(models_root);
    let available = install::update_available(installed.as_deref(), &plan.version);
    Ok(Some((plan.version, available)))
}

/// **探测凭证**：这份受管二进制曾经跑通过 `--version`，且之后没被替换过。
///
/// 这是把 8 秒等待降到零的关键。版本旁文件**只在 `--version` 跑通后才写**，写的是
/// **工具自报的版本**——于是"旁文件在场"本身就是一张"这份二进制确认可用"的凭证，
/// 界面不必为了拿版本号再跑一次 PyInstaller 的解压启动（TDD-03 §11.1c）。
///
/// 绑定靠**时序**而非另存哈希：安装流程是"落二进制 → 探测 → 写旁文件"，故旁文件恒
/// **不早于**二进制；若有人替换了二进制（mtime 变新），凭证自然失效并触发重新探测。
fn verified_version(models_root: &Path) -> Option<String> {
    let binary = managed_path(models_root);
    let sidecar = models_root.join(install::VERSION_SIDECAR);
    let binary_at = std::fs::metadata(&binary).ok()?.modified().ok()?;
    let sidecar_at = std::fs::metadata(&sidecar).ok()?.modified().ok()?;
    if sidecar_at < binary_at {
        return None; // 二进制比凭证新 = 被换过，凭证作废
    }
    std::fs::read_to_string(&sidecar)
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

/// 安装或更新受管抓取工具。返回装好的版本号。
///
/// 步骤：查 latest release → 取 `SHA2-256SUMS` → 下载平台资产（带进度）→ **校验 sha256**
/// → `chmod +x` → 原子改名 → 写版本旁文件。任一步失败都不会留下可执行的半成品。
///
/// # Errors
/// 联网失败、上游 JSON 不符预期、缺校验和、摘要不匹配、落盘失败时返回稳定错误码。
pub async fn install_managed(app: &tauri::AppHandle, models_root: &Path) -> Result<String, String> {
    let client = http_client()?;

    // 1. 上游 latest release（不锁版本——这正是"应用代装"优于"随应用打包"的地方）。
    let plan = latest_tool_plan(&client).await?;

    // 2. 校验和清单（小文件，先拿——拿不到就不下正文，绝不"先装上再说"）。
    let manifest = client
        .get(&plan.checksums_url)
        .send()
        .await
        .map_err(|e| format!("FETCH_TOOL_NETWORK: {e}"))?
        .text()
        .await
        .map_err(|e| format!("FETCH_TOOL_NETWORK: {e}"))?;
    let expected = install::checksum_for(&manifest, &plan.asset_name)
        .ok_or_else(|| "FETCH_TOOL_NO_CHECKSUM: 上游未提供该文件的校验和".to_owned())?;

    // 3. 下载到临时文件（同目录，保证改名是同卷原子操作）。
    let final_path = managed_path(models_root);
    let parent = final_path
        .parent()
        .ok_or_else(|| "FETCH_TOOL_INSTALL_FAILED: 落点无父目录".to_owned())?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|e| format!("FETCH_TOOL_INSTALL_FAILED: {e}"))?;
    let part_path = parent.join(format!("{}.part", plan.asset_name));

    let outcome = download_and_verify(app, &client, &plan, &part_path, &expected, TOOL_ID).await;
    if let Err(err) = outcome {
        let _ = tokio::fs::remove_file(&part_path).await;
        return Err(err);
    }

    // 4. 可执行位 + 原子落位（先 chmod 再改名：最终路径一出现就是可用状态）。
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut perms = tokio::fs::metadata(&part_path)
            .await
            .map_err(|e| format!("FETCH_TOOL_INSTALL_FAILED: {e}"))?
            .permissions();
        perms.set_mode(0o755);
        tokio::fs::set_permissions(&part_path, perms)
            .await
            .map_err(|e| format!("FETCH_TOOL_INSTALL_FAILED: {e}"))?;
    }
    tokio::fs::rename(&part_path, &final_path)
        .await
        .map_err(|e| format!("FETCH_TOOL_INSTALL_FAILED: {e}"))?;
    // **装完立刻验一次**（`--version`）：PyInstaller onefile 跑一次要 ~8 秒，把这份等待付在
    // "用户刚看完进度条、正等着结果"的这一刻——它是被解释的，而不是留给下次打开对话框时
    // 的一段无声空转。跑通了才写凭证，于是**此后每次开界面都是零等待**（`verified_version`）。
    let probed = luna_fetch::tool::status(Some(&final_path), None, None).await;
    match probed.version.as_deref() {
        Some(version) => {
            // 凭证写的是**工具自报的版本**而非 release tag：前者才是"它确实跑通了并且是这个版本"。
            if let Err(e) =
                tokio::fs::write(models_root.join(install::VERSION_SIDECAR), version).await
            {
                eprintln!("[fetch-tool] 版本凭证写入失败（下次开界面会重新验一遍）：{e}");
            }
        }
        None => {
            // 装下来了却跑不通：**不写凭证**（凭证的语义就是"验过"），让界面照常走探测分支
            // 并按超时/损坏分别引导。不在此报错——文件确实装好了，能不能跑是另一件事。
            eprintln!("[fetch-tool] 已安装但首次校验未通过（超时或无法执行），界面将提示重试");
        }
    }
    // 写进内存缓存，安装完成即刻就是"就绪"态，不会再退回安装引导页。
    let _ = cached_status(None, models_root, true).await;
    record_update(install::PlannedComponent::YtDlp, false);

    let _ = ModelDownloadProgress {
        model_id: TOOL_ID.to_owned(),
        downloaded_bytes: 0.0,
        total_bytes: 0.0,
        done: true,
        paused: false,
    }
    .emit(app);
    Ok(plan.version)
}

/// 带诚实 UA 的 HTTP 客户端（GitHub API 拒绝无 UA 的请求）。
fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| format!("FETCH_TOOL_INSTALL_FAILED: {e}"))
}

/// 查某个上游 latest release 并解析成安装计划（**一次联网**，安装与查更新共用同一条）。
async fn latest_plan(
    client: &reqwest::Client,
    api: &str,
    asset: &str,
    checksum_asset: &str,
) -> Result<install::InstallPlan, String> {
    let release = client
        .get(api)
        .send()
        .await
        .map_err(|e| format!("FETCH_TOOL_NETWORK: {e}"))?;
    if !release.status().is_success() {
        return Err(format!("FETCH_TOOL_NETWORK: HTTP {}", release.status()));
    }
    let release = release
        .text()
        .await
        .map_err(|e| format!("FETCH_TOOL_NETWORK: {e}"))?;
    install::parse_release(&release, asset, checksum_asset)
        .map_err(|e| format!("FETCH_TOOL_{}", e.code()))
}

/// yt-dlp 的 latest 计划（裸可执行 + 整包 `SHA2-256SUMS`）。
async fn latest_tool_plan(client: &reqwest::Client) -> Result<install::InstallPlan, String> {
    latest_plan(
        client,
        install::LATEST_RELEASE_API,
        install::platform_asset(),
        install::CHECKSUM_ASSET,
    )
    .await
}

/// 查上游有没有更新，返回 `(上游最新版, 是否比已装的新)`。
///
/// **为什么必须有这条、而且必须是用户显式触发的**：受管安装把版本**冻结在安装那一刻**，
/// 而 yt-dlp 的站点提取器是跟着站点反爬策略每周变的——2026-08-23 的真机缺陷正是如此：
/// 一份 7 周前装好、当时完全可用的 2026.07.04，在上游把被站点封掉的播放器客户端移出默认表
/// 之后，只会以一句 `HTTP 403` 收场。`install::update_available` 这条判据一直躺在
/// `luna-fetch` 里没有任何调用方——**没有更新通道的受管安装，就是把用户钉死在安装那天**。
///
/// 同时它**不做后台轮询**：默认离线是铁律（AGENTS.md §7），联网只发生在用户点「检查更新」
/// 或点「安装 / 更新」的那一刻，且严格本地模式下由调用方硬拦截。
///
/// # Errors
/// 联网失败或上游发布物不符预期时返回稳定错误码。
pub async fn check_update(models_root: &Path) -> Result<(String, bool), String> {
    let plan = latest_tool_plan(&http_client()?).await?;
    let installed = installed_version(models_root);
    let available = install::update_available(installed.as_deref(), &plan.version);
    Ok((plan.version, available))
}

/// **一次点击装齐抓取所需的一切**（PRD-05-007 修订，2026-08-23；2026-10-08 起先核对计划）。
///
/// 为什么合并而不是让用户装两次：抓取要能用，实际需要**两件**东西——yt-dlp 本身，
/// 以及它解站点 JS 挑战所需的运行时。把它们摆成两件事，用户就要理解"yt-dlp 是什么、
/// JS 运行时又是什么、我为什么两个都要装"——而这两个问题对"我只想把这条视频转成文字"
/// 的人毫无意义。现在一颗按钮装齐，**已经装了 node / bun / deno 的机器自动跳过**
/// （不重复占那 77 MB），装完立刻可被发现（受管那份在发现顺序里排第一）。
///
/// **yt-dlp 成功即算成功**：运行时那一半失败（网络抖动、上游改名）不回滚、也不把整次安装
/// 判死——yt-dlp 已经能用了，说它失败是撒谎。失败原因如实带回给界面，那一行仍显示引导。
///
/// # Errors
/// yt-dlp 那一半失败时返回其稳定错误码；运行时那一半的失败在返回值里，不作为错误抛出。
pub async fn install_toolchain(
    app: &tauri::AppHandle,
    models_root: &Path,
    confirmed: &[String],
) -> Result<ToolchainInstall, String> {
    // 执行前核对用户看过的那份计划（PRD-05-007 2026-10-08 修订）：从看计划到点安装之间，
    // 用户可能刚自己装了 node，也可能刚卸了受管 Deno——环境变了就让界面重新给出计划，
    // 而不是按旧计划装、或悄悄多装一件用户没看见的东西。
    let plan = toolchain_plan(models_root);
    if !plan_matches(&plan, confirmed) {
        return Err(PLAN_CHANGED.to_owned());
    }
    let tool_version = install_managed(app, models_root).await?;

    if !plan.contains(&install::PlannedComponent::JsRuntime) {
        return Ok(ToolchainInstall {
            tool_version,
            js_runtime_version: None,
            js_runtime_error: None,
        });
    }
    match install_managed_js_runtime(app, models_root).await {
        Ok(version) => Ok(ToolchainInstall {
            tool_version,
            js_runtime_version: Some(version),
            js_runtime_error: None,
        }),
        Err(error) => Ok(ToolchainInstall {
            tool_version,
            js_runtime_version: None,
            js_runtime_error: Some(error),
        }),
    }
}

/// 计划已变的稳定错误码：界面据此重新取计划、让用户再看一眼。
pub const PLAN_CHANGED: &str = "FETCH_TOOL_PLAN_CHANGED";

/// 本机此刻的安装计划（**不联网**：只在本地找 JS 运行时，体积是约数）。
///
/// 预览（`fetch_install_plan`）与执行（[`install_toolchain`]）共用这一个函数——
/// 两边各写一份判定，就会出现"计划说只装 yt-dlp、实际又装了 Deno"的分叉。
#[must_use]
pub fn toolchain_plan(models_root: &Path) -> Vec<install::PlannedComponent> {
    let present =
        luna_fetch::tool::locate_js_runtime(Some(&managed_js_runtime_path(models_root))).is_some();
    install::plan_toolchain(present, !install::js_runtime_platform_asset().is_empty())
}

/// 用户确认的计划（组件 id 列表）与当前计划是否一致（顺序无关，**纯函数**）。
fn plan_matches(plan: &[install::PlannedComponent], confirmed: &[String]) -> bool {
    plan.len() == confirmed.len()
        && plan
            .iter()
            .all(|component| confirmed.iter().any(|id| id == component.as_str()))
}

/// 一次合并安装的结果。
pub struct ToolchainInstall {
    /// 装好的 yt-dlp 版本。
    pub tool_version: String,
    /// 顺带装上的 JS 运行时版本（`None` = 本机已有一份，没必要再装）。
    pub js_runtime_version: Option<String>,
    /// 运行时那一半的失败原因（`None` = 没失败或没尝试）。
    pub js_runtime_error: Option<String>,
}

/// 安装受管 JS 运行时（Deno）：查 release → 取逐资产校验和 → 下载 zip → **校验** → 解压
/// 单条目 → `chmod +x` → 原子落位。返回装好的版本 tag。
///
/// 与 yt-dlp 那条链路**共用同一套纪律**（拿不到校验和就不动手、先校验后落位、落应用私有目录），
/// 唯一多出来的一步是解压——上游发的是 zip 而不是裸可执行文件。
///
/// # Errors
/// 联网失败、上游发布物不符预期、缺校验和、摘要不匹配、zip 形态异常、落盘失败时返回稳定错误码。
pub async fn install_managed_js_runtime(
    app: &tauri::AppHandle,
    models_root: &Path,
) -> Result<String, String> {
    let client = http_client()?;
    let asset = install::js_runtime_platform_asset();
    if asset.is_empty() {
        return Err("FETCH_TOOL_INSTALL_FAILED: 本平台没有可代装的 JS 运行时".to_owned());
    }
    let plan = latest_plan(
        &client,
        install::JS_RUNTIME_LATEST_RELEASE_API,
        asset,
        &install::js_runtime_checksum_asset(asset),
    )
    .await?;

    let manifest = client
        .get(&plan.checksums_url)
        .send()
        .await
        .map_err(|e| format!("FETCH_TOOL_NETWORK: {e}"))?
        .text()
        .await
        .map_err(|e| format!("FETCH_TOOL_NETWORK: {e}"))?;
    let expected = install::checksum_for(&manifest, &plan.asset_name)
        .ok_or_else(|| "FETCH_TOOL_NO_CHECKSUM: 上游未提供该文件的校验和".to_owned())?;

    let final_path = managed_js_runtime_path(models_root);
    let parent = final_path
        .parent()
        .ok_or_else(|| "FETCH_TOOL_INSTALL_FAILED: 落点无父目录".to_owned())?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|e| format!("FETCH_TOOL_INSTALL_FAILED: {e}"))?;
    let archive_path = parent.join(format!("{}.part", plan.asset_name));
    let staged_path = parent.join(format!("{}.staged", plan.asset_name));

    let outcome = async {
        download_and_verify(
            app,
            &client,
            &plan,
            &archive_path,
            &expected,
            JS_RUNTIME_TOOL_ID,
        )
        .await?;
        // 解压在阻塞线程池：zip 是同步 IO + 解压缩，77 MB 的 inflate 放在 async 线程上
        // 会把整个 runtime 卡住（与 ADR-009「实时回调不做重活」同一条心智）。
        let (from, to) = (archive_path.clone(), staged_path.clone());
        tokio::task::spawn_blocking(move || extract_single_executable(&from, &to))
            .await
            .map_err(|e| format!("FETCH_TOOL_INSTALL_FAILED: {e}"))?
    }
    .await;
    // 归档件无论成败都不该留下（77 MB 的 zip 留在磁盘上是纯粹的浪费）。
    let _ = tokio::fs::remove_file(&archive_path).await;
    if let Err(err) = outcome {
        let _ = tokio::fs::remove_file(&staged_path).await;
        return Err(err);
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut perms = tokio::fs::metadata(&staged_path)
            .await
            .map_err(|e| format!("FETCH_TOOL_INSTALL_FAILED: {e}"))?
            .permissions();
        perms.set_mode(0o755);
        tokio::fs::set_permissions(&staged_path, perms)
            .await
            .map_err(|e| format!("FETCH_TOOL_INSTALL_FAILED: {e}"))?;
    }
    tokio::fs::rename(&staged_path, &final_path)
        .await
        .map_err(|e| format!("FETCH_TOOL_INSTALL_FAILED: {e}"))?;
    // 记下装的是哪一版：「检查更新」据此比较（写不成不算安装失败，下次检查会当作可更新）。
    let _ = tokio::fs::write(
        models_root.join(install::JS_RUNTIME_VERSION_SIDECAR),
        &plan.version,
    )
    .await;
    record_update(install::PlannedComponent::JsRuntime, false);

    let _ = ModelDownloadProgress {
        model_id: JS_RUNTIME_TOOL_ID.to_owned(),
        downloaded_bytes: 0.0,
        total_bytes: 0.0,
        done: true,
        paused: false,
    }
    .emit(app);
    Ok(plan.version)
}

/// 从 zip 里取出**唯一那个**可执行条目，写到 `dest`。
///
/// **只接受"恰好一个条目"**（核实过上游 v2.9.5：中央目录里就一个 `deno`）。上游哪天改成
/// 多条目归档（加个 README、改成带目录的布局），我们宁可**明确失败**也不去猜哪个才是运行时
/// ——猜错的后果是把一个非可执行文件 `chmod +x` 后当运行时去跑，表现成一句莫名其妙的失败。
///
/// 同样拒绝带路径分隔符的条目名：那是 zip-slip 的形状，而我们只期待一个裸文件名。
fn extract_single_executable(archive: &Path, dest: &Path) -> Result<(), String> {
    use std::io::Read as _;

    let file =
        std::fs::File::open(archive).map_err(|e| format!("FETCH_TOOL_INSTALL_FAILED: {e}"))?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file))
        .map_err(|e| format!("FETCH_TOOL_INSTALL_FAILED: 归档无法读取（{e}）"))?;
    if zip.len() != 1 {
        return Err(format!(
            "FETCH_TOOL_INSTALL_FAILED: 归档里有 {} 个条目，预期恰好 1 个",
            zip.len()
        ));
    }
    let mut entry = zip
        .by_index(0)
        .map_err(|e| format!("FETCH_TOOL_INSTALL_FAILED: {e}"))?;
    // zip 9 起 `name()` 可失败（条目名不是合法 UTF-8 时报错）：同样按"条目名异常"拒绝。
    let name = entry
        .name()
        .map_err(|e| format!("FETCH_TOOL_INSTALL_FAILED: 归档条目名异常（{e}）"))?
        .into_owned();
    if name.contains('/') || name.contains('\\') {
        return Err(format!(
            "FETCH_TOOL_INSTALL_FAILED: 归档条目名异常（{name}）"
        ));
    }
    let mut out =
        std::fs::File::create(dest).map_err(|e| format!("FETCH_TOOL_INSTALL_FAILED: {e}"))?;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = entry
            .read(&mut buffer)
            .map_err(|e| format!("FETCH_TOOL_INSTALL_FAILED: {e}"))?;
        if read == 0 {
            break;
        }
        std::io::Write::write_all(&mut out, &buffer[..read])
            .map_err(|e| format!("FETCH_TOOL_INSTALL_FAILED: {e}"))?;
    }
    std::io::Write::flush(&mut out).map_err(|e| format!("FETCH_TOOL_INSTALL_FAILED: {e}"))?;
    Ok(())
}

/// 流式下载 + 边写边算摘要，最后比对（**不匹配即拒绝**，字节留在 `.part` 由调用方清理）。
async fn download_and_verify(
    app: &tauri::AppHandle,
    client: &reqwest::Client,
    plan: &install::InstallPlan,
    part_path: &Path,
    expected: &str,
    tool_id: &str,
) -> Result<(), String> {
    use futures::StreamExt as _;
    use sha2::Digest as _;
    use tokio::io::AsyncWriteExt as _;

    let mut hasher = sha2::Sha256::new();

    let response = client
        .get(&plan.asset_url)
        .send()
        .await
        .map_err(|e| format!("FETCH_TOOL_NETWORK: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("FETCH_TOOL_NETWORK: HTTP {}", response.status()));
    }
    let mut file = tokio::fs::File::create(part_path)
        .await
        .map_err(|e| format!("FETCH_TOOL_INSTALL_FAILED: {e}"))?;
    let mut downloaded: u64 = 0;
    let mut stream = response.bytes_stream();
    // 进度节流与抓取同理（整队列/整列表 JSON 广播不该每个 chunk 来一次）。
    let mut last_emit = std::time::Instant::now();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("FETCH_TOOL_NETWORK: {e}"))?;
        hasher.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("FETCH_TOOL_INSTALL_FAILED: {e}"))?;
        downloaded += chunk.len() as u64;
        if last_emit.elapsed() >= std::time::Duration::from_millis(250) {
            last_emit = std::time::Instant::now();
            let _ = ModelDownloadProgress {
                model_id: tool_id.to_owned(),
                downloaded_bytes: downloaded as f64,
                total_bytes: plan.size_bytes as f64,
                done: false,
                paused: false,
            }
            .emit(app);
        }
    }
    file.flush()
        .await
        .map_err(|e| format!("FETCH_TOOL_INSTALL_FAILED: {e}"))?;

    let actual = hex_lower(&hasher.finalize());
    if actual != expected {
        return Err("FETCH_TOOL_CHECKSUM_MISMATCH: 下载文件校验失败，已丢弃".to_owned());
    }
    Ok(())
}

/// 摘要 → 小写十六进制（与 `SHA2-256SUMS` 的归一化口径一致）。
fn hex_lower(digest: &[u8]) -> String {
    use std::fmt::Write as _;
    digest.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

/// 卸载受管工具（删二进制 + 探测凭证）。用户改用自己装的那份、或想腾出 38 MB 时用。
///
/// 幂等：文件不存在不算失败——目标状态已达成。顺带清掉内存缓存，界面立即反映"没装了"。
///
/// # Errors
/// 删除失败（权限等）时返回错误。
pub fn uninstall_managed(models_root: &Path) -> Result<(), String> {
    record_update(install::PlannedComponent::YtDlp, false);
    remove_all(&[
        managed_path(models_root),
        models_root.join(install::VERSION_SIDECAR),
    ])
}

/// 卸载受管 JS 运行时（解压后约 77 MB，**必须给得掉**——装得上却删不掉是在替用户做主）。
///
/// 与 yt-dlp 的卸载分开：用户可能只想留其中一件（比如自己另装了 node，就没必要留这份 Deno）。
/// 同样幂等，同样只删我们自己放的那一份。
///
/// # Errors
/// 删除失败（权限等）时返回错误。
pub fn uninstall_managed_js_runtime(models_root: &Path) -> Result<(), String> {
    record_update(install::PlannedComponent::JsRuntime, false);
    remove_all(&[
        managed_js_runtime_path(models_root),
        models_root.join(install::JS_RUNTIME_VERSION_SIDECAR),
    ])
}

/// 幂等删除一组受管文件（不存在不算失败——目标状态已达成），并清掉探测缓存。
fn remove_all(paths: &[PathBuf]) -> Result<(), String> {
    for path in paths {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("FETCH_TOOL_UNINSTALL_FAILED: {e}")),
        }
    }
    if let Ok(mut cache) = PROBE_CACHE.lock() {
        *cache = None;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_update_is_known_only_after_a_check_and_forgotten_once_installed() {
        let js = install::PlannedComponent::JsRuntime;
        record_update(js, true);
        assert!(known_update(js));
        // 装上 / 更新 / 卸载都走同一条：记回没有新版，按钮不再写「更新」。
        record_update(js, false);
        assert!(!known_update(js));
    }

    /// 造一个 zip（`entries` = `(条目名, 内容)`）。
    fn make_zip(path: &Path, entries: &[(&str, &[u8])]) {
        use std::io::Write as _;
        let file = std::fs::File::create(path).expect("create zip");
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, body) in entries {
            writer.start_file(*name, options).expect("start entry");
            writer.write_all(body).expect("write entry");
        }
        writer.finish().expect("finish zip");
    }

    /// 快乐路径：上游那种"恰好一个条目"的归档，内容原样落到目标路径。
    #[test]
    fn extracts_the_single_entry_verbatim() {
        let dir = tempfile::tempdir().expect("tempdir");
        let archive = dir.path().join("deno.zip");
        let dest = dir.path().join("deno");
        make_zip(&archive, &[("deno", b"#!/bin/sh\necho deno\n")]);

        extract_single_executable(&archive, &dest).expect("extract");
        assert_eq!(
            std::fs::read(&dest).expect("read"),
            b"#!/bin/sh\necho deno\n"
        );
    }

    /// 上游哪天改成多条目布局，我们**明确失败**而不是猜哪个才是运行时——猜错的后果是
    /// 把一个非可执行文件 chmod +x 后当运行时去跑，表现成一句莫名其妙的失败。
    #[test]
    fn refuses_archives_that_are_not_exactly_one_entry() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dest = dir.path().join("out");

        let many = dir.path().join("many.zip");
        make_zip(&many, &[("deno", b"a"), ("README.md", b"b")]);
        let err = extract_single_executable(&many, &dest).expect_err("多条目必须失败");
        assert!(err.starts_with("FETCH_TOOL_INSTALL_FAILED"), "{err}");

        let empty = dir.path().join("empty.zip");
        make_zip(&empty, &[]);
        assert!(
            extract_single_executable(&empty, &dest).is_err(),
            "空归档同样失败"
        );
    }

    /// 条目名带路径分隔符是 zip-slip 的形状；我们只期待一个裸文件名，故一律拒绝。
    #[test]
    fn refuses_entry_names_with_path_separators() {
        let dir = tempfile::tempdir().expect("tempdir");
        let archive = dir.path().join("slip.zip");
        let dest = dir.path().join("out");
        make_zip(&archive, &[("../../etc/passwd", b"x")]);
        assert!(extract_single_executable(&archive, &dest).is_err());
        assert!(!dest.exists(), "拒绝时不得落任何字节");
    }

    /// 卸载受管运行时同样幂等，且**只删那一份**——绝不顺手把 yt-dlp 也删了。
    #[test]
    fn uninstalling_the_js_runtime_leaves_the_fetch_tool_alone() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("tools")).expect("mkdir");
        std::fs::write(managed_path(root), b"yt-dlp").expect("write tool");
        std::fs::write(managed_js_runtime_path(root), b"deno").expect("write runtime");

        assert!(uninstall_managed_js_runtime(root).is_ok());
        assert!(!managed_js_runtime_path(root).exists(), "运行时已删");
        assert!(managed_path(root).exists(), "yt-dlp 必须原样留着");
        assert!(uninstall_managed_js_runtime(root).is_ok(), "再卸一次仍 ok");
    }

    #[test]
    fn hex_lower_is_sha256sums_compatible() {
        assert_eq!(hex_lower(&[0x0a, 0xff, 0x10]), "0aff10");
        assert_eq!(hex_lower(&[]), "");
    }

    /// 探测凭证靠**时序**绑定二进制：旁文件不早于二进制才算数。
    /// 这是"零等待"的正确性根基——凭证若能在二进制被换掉后仍生效，界面就会显示一个早已
    /// 不存在的版本，甚至把一份跑不通的工具当成就绪。
    #[test]
    fn verified_version_expires_when_the_binary_is_replaced() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("tools")).expect("mkdir");
        let binary = managed_path(root);
        let sidecar = root.join(install::VERSION_SIDECAR);

        std::fs::write(&binary, b"#!/bin/sh\n").expect("write bin");
        // 安装流程的顺序是"落二进制 → 验 → 写凭证"，故凭证恒不早于二进制。
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&sidecar, "2026.07.04").expect("write sidecar");
        assert_eq!(verified_version(root).as_deref(), Some("2026.07.04"));

        // 有人换掉了二进制（mtime 变新）→ 凭证作废，界面须重新探测而不是信旧版本号。
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&binary, b"#!/bin/sh\n# newer\n").expect("replace bin");
        assert_eq!(verified_version(root), None, "二进制比凭证新 → 凭证失效");

        // 缺凭证（装下来了但首次校验没通过）同样为 None——凭证的语义就是"验过"。
        std::fs::remove_file(&sidecar).expect("rm sidecar");
        assert_eq!(verified_version(root), None);
    }

    /// 卸载是幂等的（重复点、或用户已手删都不该报错），且凭证随之清掉。
    #[test]
    fn uninstall_removes_binary_and_credential_idempotently() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        assert!(uninstall_managed(root).is_ok(), "未装时卸载即空操作");
        std::fs::create_dir_all(root.join("tools")).expect("mkdir");
        std::fs::write(managed_path(root), b"x").expect("write");
        std::fs::write(root.join(install::VERSION_SIDECAR), "1").expect("write");
        assert!(uninstall_managed(root).is_ok());
        assert!(!managed_path(root).exists(), "二进制已删");
        assert!(
            !root.join(install::VERSION_SIDECAR).exists(),
            "凭证一并删——留着它会让下次装完前的界面显示一个不存在的版本"
        );
        assert!(uninstall_managed(root).is_ok(), "再卸一次仍 ok");
    }

    /// 二进制不在了就当没装——旁文件残留（用户手删了工具）不该让界面显示"已安装"。
    #[test]
    fn installed_version_requires_the_binary_to_exist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("tools")).expect("mkdir");
        std::fs::write(root.join(install::VERSION_SIDECAR), "2026.07.04").expect("write");
        assert_eq!(installed_version(root), None, "只有版本记录、没有二进制");

        std::fs::write(managed_path(root), b"#!/bin/sh\n").expect("write bin");
        assert_eq!(installed_version(root).as_deref(), Some("2026.07.04"));
    }

    /// 确认的计划与当前计划顺序无关地逐件相等才算一致：多一件、少一件都要回到确认。
    #[test]
    fn plan_matches_requires_the_same_components() {
        use install::PlannedComponent::{JsRuntime, YtDlp};
        let ids = |list: &[&str]| list.iter().map(|id| (*id).to_owned()).collect::<Vec<_>>();
        assert!(plan_matches(
            &[YtDlp, JsRuntime],
            &ids(&["js_runtime", "yt_dlp"])
        ));
        assert!(
            !plan_matches(&[YtDlp], &ids(&["yt_dlp", "js_runtime"])),
            "运行时已出现"
        );
        assert!(
            !plan_matches(&[YtDlp, JsRuntime], &ids(&["yt_dlp"])),
            "运行时已消失"
        );
        assert!(!plan_matches(&[YtDlp], &[]), "没看过计划不能装");
    }
}
