//! 抓取工具的**受管安装**：解析上游发布物、选平台资产、校验和（纯逻辑，TDD-03 §11.8）。
//!
//! ## 为什么应用可以代为安装（ADR-023 修订）
//! 初版把「打包随应用分发」的缺点（版本随发版冻结 → 下载成功率单调下降）当成了否定整个
//! 「应用代劳」方向的理由——**那是两件事**。按需从上游取 **latest** 恰恰没有版本落后问题：
//! 它每次装的都是当天最新版，甚至比 `brew`（用户得记得 `brew upgrade`）更不容易过期。
//! License 上也更干净：把 GPLv3 打包物塞进 `.app` **分发**会触发我们的分发义务；
//! 代用户从上游**下载**只是自动化了他本可以自己做的那一步。
//!
//! 三条使之可行的上游事实：
//! - `yt-dlp_macos` 等是 **PyInstaller 自包含单文件**（不需要系统 Python、不需要安装器）；
//! - 每个 release 自带 `SHA2-256SUMS`，故**不锁版本也能校验**（模型权重那套硬编码 sha256
//!   的做法在这里行不通——yt-dlp 一年发数十版）；
//! - 各平台都有对应单文件，落到应用私有目录 + `chmod +x` 即可用，**不碰系统**。
//!
//! 本模块只做**纯判定**（选哪个资产、期望什么校验和）；下载、进度事件、落盘与 `chmod`
//! 在 `luna-app`（IO 与事件属应用层）。

use crate::error::FetchError;

/// 上游 latest release 的查询地址（GitHub API）。
pub const LATEST_RELEASE_API: &str = "https://api.github.com/repos/yt-dlp/yt-dlp/releases/latest";

/// 校验和清单在 release 资产中的文件名。
pub const CHECKSUM_ASSET: &str = "SHA2-256SUMS";

/// 受管 **JS 运行时**（Deno）的 latest release 查询地址。
///
/// ## 为什么它也够格被代装（ADR-023 的门槛，逐条核对过）
/// - **官方仓库发官方二进制** ✓（`denoland/deno` 的 release 资产）；
/// - **每个资产自带官方校验和** ✓——形态与 yt-dlp 不同：不是一份整包清单，而是**每个资产
///   一个 `<资产名>.sha256sum`**，但内容仍是标准 `sha256sum` 格式，故 [`checksum_for`] 原样复用；
/// - **自包含单文件** ✓——核实过 v2.9.5 的中央目录：zip 里**只有一个条目**（`deno`，deflate），
///   解压出来落应用私有目录 + `chmod +x` 即可用，不碰系统；
/// - **MIT** ✓，与 AGPL 兼容。
///
/// 与 yt-dlp 唯一的实现差别就是**多一步解压**（yt-dlp 的发布物是裸可执行文件）。
pub const JS_RUNTIME_LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/denoland/deno/releases/latest";

/// 本平台应当下载的资产名（**编译期**按目标平台/架构定，不做运行时猜测）。
///
/// > 假设：macOS 取 `yt-dlp_macos`（universal2，覆盖 Apple Silicon 与 Intel）；
/// > Windows 按架构取 `yt-dlp.exe` / `yt-dlp_arm64.exe`；Linux 同理（理由：这些是上游
/// > 的自包含单文件档，不需要 Python 环境；`_legacy` 档只服务 macOS < 10.15，
/// > 而项目基线远高于它，故不纳入）。
#[must_use]
pub const fn platform_asset() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "yt-dlp_macos"
    }
    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    {
        "yt-dlp_arm64.exe"
    }
    #[cfg(all(target_os = "windows", not(target_arch = "aarch64")))]
    {
        "yt-dlp.exe"
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        "yt-dlp_linux_aarch64"
    }
    #[cfg(all(target_os = "linux", not(target_arch = "aarch64")))]
    {
        "yt-dlp_linux"
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        "yt-dlp"
    }
}

/// 本平台应当下载的 **Deno** 资产名（编译期按目标三元组定，不做运行时猜测）。
///
/// > 假设：只覆盖 x86_64 / aarch64 两种架构（理由：上游发布物本就只有这两种，且项目基线
/// > 已是 Apple Silicon 与 x64 Windows；其余目标平台不代装，界面退回引导）。
#[must_use]
pub const fn js_runtime_platform_asset() -> &'static str {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        "deno-aarch64-apple-darwin.zip"
    }
    #[cfg(all(target_os = "macos", not(target_arch = "aarch64")))]
    {
        "deno-x86_64-apple-darwin.zip"
    }
    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    {
        "deno-aarch64-pc-windows-msvc.zip"
    }
    #[cfg(all(target_os = "windows", not(target_arch = "aarch64")))]
    {
        "deno-x86_64-pc-windows-msvc.zip"
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        "deno-aarch64-unknown-linux-gnu.zip"
    }
    #[cfg(all(target_os = "linux", not(target_arch = "aarch64")))]
    {
        "deno-x86_64-unknown-linux-gnu.zip"
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        ""
    }
}

/// Deno 的校验和资产名：**每个资产一个** `<资产名>.sha256sum`（不是整包清单）。
#[must_use]
pub fn js_runtime_checksum_asset(asset: &str) -> String {
    format!("{asset}.sha256sum")
}

/// 受管 JS 运行时的落点（与 yt-dlp 同住 `tools/`，故"删模型腾空间"同样误伤不到）。
#[must_use]
pub const fn managed_js_runtime_rel_path() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "tools/deno.exe"
    }
    #[cfg(not(target_os = "windows"))]
    {
        "tools/deno"
    }
}

/// 受管工具在 `models_root` 下的相对落点；P1 安全搬移实现后必须与模型权重同批搬移。
///
/// 放 `tools/` 而非模型目录：它是**可执行程序**不是权重，混放会让「删除模型释放空间」
/// 一类操作误伤工具。
#[must_use]
pub const fn managed_rel_path() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "tools/yt-dlp.exe"
    }
    #[cfg(not(target_os = "windows"))]
    {
        "tools/yt-dlp"
    }
}

/// 记录已装版本的旁文件名（供界面显示"已安装 2026.07.04"与判断是否有更新）。
pub const VERSION_SIDECAR: &str = "tools/yt-dlp.version";

/// 受管 JS 运行时（Deno）的版本旁文件：与 yt-dlp 同一种记法，「检查更新」据此比较（ADR-167 ⑤）。
pub const JS_RUNTIME_VERSION_SIDECAR: &str = "tools/deno.version";

/// 一次安装所需的全部上游事实（由 [`parse_release`] 从 release JSON 解出）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallPlan {
    /// 上游版本号（release tag，如 `2026.07.04`）。
    pub version: String,
    /// 平台资产的下载地址。
    pub asset_url: String,
    /// 平台资产名（与 `SHA2-256SUMS` 里的行对应）。
    pub asset_name: String,
    /// 资产字节数（进度条总量；`0` = 上游未给）。
    pub size_bytes: u64,
    /// 校验和清单的下载地址。
    pub checksums_url: String,
}

/// 解析 GitHub release JSON（**纯函数**）。
///
/// `asset_name` 是本平台的发布物，`checksum_asset` 是它的校验和文件——两者都由调用方给出，
/// 因为两件受管工具的上游形态不同：yt-dlp 是"裸可执行 + 整包清单 `SHA2-256SUMS`"，
/// Deno 是"zip + 逐资产 `<资产名>.sha256sum`"。**共用同一条解析与校验纪律**，只是名字不同。
///
/// # Errors
/// 非 JSON、缺 `tag_name`、找不到本平台资产或校验和文件时返回 [`FetchError::BadProbeOutput`]
/// ——安装是"要么拿到可校验的完整计划，要么不动手"，绝不在缺校验和的情况下继续。
pub fn parse_release(
    json: &str,
    asset_name: &str,
    checksum_asset: &str,
) -> Result<InstallPlan, FetchError> {
    let root: serde_json::Value =
        serde_json::from_str(json).map_err(|_| FetchError::BadProbeOutput)?;
    let version = root
        .get("tag_name")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or(FetchError::BadProbeOutput)?
        .to_owned();
    let assets = root
        .get("assets")
        .and_then(serde_json::Value::as_array)
        .ok_or(FetchError::BadProbeOutput)?;

    let find = |name: &str| {
        assets
            .iter()
            .find(|asset| asset.get("name").and_then(serde_json::Value::as_str) == Some(name))
    };
    let url_of = |asset: &serde_json::Value| {
        asset
            .get("browser_download_url")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    };

    let asset = find(asset_name).ok_or(FetchError::BadProbeOutput)?;
    let checksums = find(checksum_asset).ok_or(FetchError::BadProbeOutput)?;
    Ok(InstallPlan {
        version,
        asset_url: url_of(asset).ok_or(FetchError::BadProbeOutput)?,
        asset_name: asset_name.to_owned(),
        size_bytes: asset
            .get("size")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
        checksums_url: url_of(checksums).ok_or(FetchError::BadProbeOutput)?,
    })
}

/// 从 `SHA2-256SUMS` 清单里取某个资产的十六进制 sha256（**纯函数**）。
///
/// 清单是标准 `sha256sum` 格式（`<64 hex>  <文件名>`，二进制模式下文件名前可能带 `*`）。
/// 找不到该文件名、或摘要不是 64 位十六进制时返回 `None`——**宁可拒绝安装也不跳过校验**。
#[must_use]
pub fn checksum_for(manifest: &str, asset_name: &str) -> Option<String> {
    manifest.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let digest = parts.next()?;
        let name = parts.next()?.trim_start_matches('*');
        if name != asset_name {
            return None;
        }
        let valid = digest.len() == 64 && digest.chars().all(|c| c.is_ascii_hexdigit());
        valid.then(|| digest.to_ascii_lowercase())
    })
}

/// 「让 Lunarca 安装」这一次点击会下载的一件组件（安装计划的一行，PRD-05-007 2026-10-08 修订）。
///
/// 计划在**点击之前**给出：用户要能看见"将装哪几件、从哪来、装到哪、大概多大"，而不是读一段
/// 固定写着"两件都会装"的说明——本机已有 node / bun / deno 时运行时那件根本不装。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlannedComponent {
    /// yt-dlp 本体（每次安装都会下载 latest，已有受管那份时即覆盖）。
    YtDlp,
    /// 受管 JS 运行时（Deno），仅当本机一个 JS 运行时都没有、且本平台有官方档时才在计划里。
    JsRuntime,
}

impl PlannedComponent {
    /// 稳定 id（**IPC 契约**）：与 [`crate::tool::ExternalTool::as_str`] 同一取值空间，
    /// 前端可按同一个 id 取文案与安装引导。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::YtDlp => "yt_dlp",
            Self::JsRuntime => "js_runtime",
        }
    }

    /// 上游来源（展示用的仓库地址，不带协议）——下载只访问它的 release 端点。
    #[must_use]
    pub const fn source(self) -> &'static str {
        match self {
            Self::YtDlp => "github.com/yt-dlp/yt-dlp",
            Self::JsRuntime => "github.com/denoland/deno",
        }
    }

    /// 典型下载体积（字节，**约数**）：上游每版浮动，精确值只有在查到 release 之后才知道，
    /// 而查 release 本身就是一次联网——计划阶段不为一个数字先联网。下载开始后进度条给精确总量。
    #[must_use]
    pub const fn typical_download_bytes(self) -> u64 {
        match self {
            Self::YtDlp | Self::JsRuntime => 38 * 1024 * 1024,
        }
    }

    /// 典型磁盘占用（字节，约数）：yt-dlp 是裸可执行，下载即占用；Deno 下的是 zip，解压后约 77 MB。
    #[must_use]
    pub const fn typical_disk_bytes(self) -> u64 {
        match self {
            Self::YtDlp => 38 * 1024 * 1024,
            Self::JsRuntime => 77 * 1024 * 1024,
        }
    }
}

/// 一次「让 Lunarca 安装」的计划（**纯函数**，执行与预览共用这一个判定）。
///
/// `js_runtime_present` = 本机已能找到任一 JS 运行时（系统的或受管的）；
/// `js_runtime_installable` = 本平台有官方 Deno 档（[`js_runtime_platform_asset`] 非空）。
/// 计划与执行若各写一份判定，预览说"只装 yt-dlp"而实际又装了 Deno 的分叉迟早出现。
#[must_use]
pub fn plan_toolchain(
    js_runtime_present: bool,
    js_runtime_installable: bool,
) -> Vec<PlannedComponent> {
    let mut plan = vec![PlannedComponent::YtDlp];
    if !js_runtime_present && js_runtime_installable {
        plan.push(PlannedComponent::JsRuntime);
    }
    plan
}

/// 已装版本是否落后于上游（两者都是 `YYYY.MM.DD` 形态的 tag，字典序即时序）。
///
/// 形态不符（上游改了命名）时按"不同即有更新"处理——宁可多提示一次，也不让用户困在旧版。
#[must_use]
pub fn update_available(installed: Option<&str>, latest: &str) -> bool {
    match installed {
        None => true,
        Some(installed) => installed.trim() != latest.trim(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELEASE: &str = r#"{
        "tag_name": "2026.07.04",
        "assets": [
            {"name": "SHA2-256SUMS", "browser_download_url": "https://x/SHA2-256SUMS", "size": 1595},
            {"name": "yt-dlp_macos", "browser_download_url": "https://x/yt-dlp_macos", "size": 38256544},
            {"name": "yt-dlp.exe", "browser_download_url": "https://x/yt-dlp.exe", "size": 12},
            {"name": "yt-dlp_linux", "browser_download_url": "https://x/yt-dlp_linux", "size": 13},
            {"name": "yt-dlp_linux_aarch64", "browser_download_url": "https://x/aarch64", "size": 14},
            {"name": "yt-dlp_arm64.exe", "browser_download_url": "https://x/win-arm", "size": 15}
        ]
    }"#;

    /// yt-dlp 的解析入口（裸可执行 + 整包清单）。
    fn parse_yt_dlp(json: &str) -> Result<InstallPlan, FetchError> {
        parse_release(json, platform_asset(), CHECKSUM_ASSET)
    }

    #[test]
    fn parses_release_and_picks_this_platform_asset() {
        let plan = parse_yt_dlp(RELEASE).expect("plan");
        assert_eq!(plan.version, "2026.07.04");
        assert_eq!(plan.asset_name, platform_asset());
        assert!(
            plan.asset_url.ends_with(platform_asset()) || plan.asset_url.starts_with("https://x/")
        );
        assert_eq!(plan.checksums_url, "https://x/SHA2-256SUMS");
        assert!(plan.size_bytes > 0);
    }

    /// 计划只在确实要装时才列运行时：本机已有一份，或本平台没有官方档，都不能宣称"会一并装上"。
    #[test]
    fn plan_lists_js_runtime_only_when_missing_and_installable() {
        assert_eq!(
            plan_toolchain(false, true),
            vec![PlannedComponent::YtDlp, PlannedComponent::JsRuntime]
        );
        assert_eq!(plan_toolchain(true, true), vec![PlannedComponent::YtDlp]);
        assert_eq!(plan_toolchain(false, false), vec![PlannedComponent::YtDlp]);
    }

    /// 计划 id 与外部工具 id 同一取值空间（前端按同一个 id 取文案与安装引导）。
    #[test]
    fn planned_component_ids_are_external_tool_ids() {
        for component in [PlannedComponent::YtDlp, PlannedComponent::JsRuntime] {
            assert!(crate::tool::ExternalTool::ALL
                .iter()
                .any(|tool| tool.as_str() == component.as_str()));
        }
    }

    /// 缺校验和清单时**拒绝安装**——绝不在无法校验的情况下把一个可执行文件落到磁盘上。
    #[test]
    fn refuses_release_without_checksum_manifest() {
        let json = format!(
            r#"{{"tag_name":"1","assets":[{{"name":"{}","browser_download_url":"https://x/a","size":1}}]}}"#,
            platform_asset()
        );
        assert_eq!(parse_yt_dlp(&json), Err(FetchError::BadProbeOutput));
    }

    /// 上游没有本平台的资产时同样拒绝（而不是随便挑一个跑不起来的档）。
    #[test]
    fn refuses_release_without_platform_asset() {
        let json = r#"{"tag_name":"1","assets":[
            {"name":"SHA2-256SUMS","browser_download_url":"https://x/s","size":1},
            {"name":"yt-dlp_totally_other_platform","browser_download_url":"https://x/o","size":1}
        ]}"#;
        assert_eq!(parse_yt_dlp(json), Err(FetchError::BadProbeOutput));
    }

    #[test]
    fn refuses_malformed_release_json() {
        assert_eq!(parse_yt_dlp("not json"), Err(FetchError::BadProbeOutput));
        assert_eq!(parse_yt_dlp("{}"), Err(FetchError::BadProbeOutput));
        assert_eq!(
            parse_yt_dlp(r#"{"tag_name":"  "}"#),
            Err(FetchError::BadProbeOutput)
        );
    }

    /// Deno 那一档走**同一条**解析与校验纪律，只是资产名与校验和文件名不同：
    /// 逐资产 `<资产名>.sha256sum`，而不是整包清单。核实过上游 v2.9.5 的资产命名。
    #[test]
    fn parses_deno_release_with_per_asset_checksum() {
        let asset = js_runtime_platform_asset();
        let checksum = js_runtime_checksum_asset(asset);
        let json = format!(
            r#"{{"tag_name":"v2.9.5","assets":[
                {{"name":"{asset}","browser_download_url":"https://x/{asset}","size":38511993}},
                {{"name":"{checksum}","browser_download_url":"https://x/{checksum}","size":96}}
            ]}}"#
        );
        let plan = parse_release(&json, asset, &checksum).expect("plan");
        assert_eq!(plan.version, "v2.9.5");
        assert_eq!(plan.asset_name, asset);
        assert!(plan.size_bytes > 0);
        assert!(plan.checksums_url.ends_with(".sha256sum"));

        // 缺校验和文件同样**拒绝安装**——与 yt-dlp 同一条纪律，不因它是"另一件工具"而放松。
        let without = format!(
            r#"{{"tag_name":"v2.9.5","assets":[
                {{"name":"{asset}","browser_download_url":"https://x/{asset}","size":1}}
            ]}}"#
        );
        assert_eq!(
            parse_release(&without, asset, &checksum),
            Err(FetchError::BadProbeOutput)
        );
    }

    /// 逐资产校验和文件的内容就是标准 `sha256sum` 一行，故 [`checksum_for`] 原样复用
    /// ——核实过上游真实文件的字节形态（`<64 hex><两个空格><资产名>`）。
    #[test]
    fn per_asset_checksum_file_parses_with_the_same_reader() {
        let asset = js_runtime_platform_asset();
        let digest = "b796aadd131f6930560c1ee040cf0d6f53933fbb987464e9ff46bd7ea4830615";
        assert_eq!(
            checksum_for(&format!("{digest}  {asset}"), asset),
            Some(digest.to_owned())
        );
    }

    /// 两件受管工具同住 `tools/`，且落点互不相同（否则后装的会覆盖先装的）。
    #[test]
    fn managed_paths_share_the_tools_dir_without_colliding() {
        assert!(managed_js_runtime_rel_path().starts_with("tools/"));
        assert_ne!(managed_js_runtime_rel_path(), managed_rel_path());
        assert!(
            !js_runtime_platform_asset().is_empty(),
            "本平台必须有 Deno 资产"
        );
        assert!(js_runtime_platform_asset().ends_with(".zip"));
    }

    /// 校验和清单是标准 `sha256sum` 格式；二进制模式的 `*` 前缀要剥掉。
    #[test]
    fn reads_checksum_for_named_asset() {
        let digest = "a".repeat(64);
        let other = "b".repeat(64);
        let manifest = format!("{other}  yt-dlp\n{digest} *yt-dlp_macos\n{other}  yt-dlp.exe\n");
        assert_eq!(checksum_for(&manifest, "yt-dlp_macos"), Some(digest));
        assert_eq!(checksum_for(&manifest, "yt-dlp"), Some(other));
        assert_eq!(checksum_for(&manifest, "not-in-manifest"), None);
    }

    /// 摘要形态不对（长度/字符）时返回 `None`——半个校验等于没校验。
    #[test]
    fn rejects_malformed_digest_lines() {
        assert_eq!(checksum_for("deadbeef  yt-dlp_macos", "yt-dlp_macos"), None);
        assert_eq!(
            checksum_for(&format!("{}  yt-dlp_macos", "z".repeat(64)), "yt-dlp_macos"),
            None,
            "非十六进制字符"
        );
        assert_eq!(checksum_for("", "yt-dlp_macos"), None);
        assert_eq!(checksum_for("no-filename-column", "yt-dlp_macos"), None);
    }

    /// 摘要大小写不敏感，统一归一为小写（与 `sha2` 的 hex 输出对齐，否则恒判不匹配）。
    #[test]
    fn digest_is_normalised_to_lowercase() {
        let upper = "A".repeat(64);
        assert_eq!(
            checksum_for(&format!("{upper}  yt-dlp_macos"), "yt-dlp_macos"),
            Some("a".repeat(64))
        );
    }

    #[test]
    fn update_available_compares_tags() {
        assert!(update_available(None, "2026.07.04"), "未装 → 有更新");
        assert!(update_available(Some("2026.06.01"), "2026.07.04"));
        assert!(!update_available(Some("2026.07.04"), "2026.07.04"));
        assert!(
            !update_available(Some(" 2026.07.04 "), "2026.07.04"),
            "去空白后比较"
        );
        // 上游改了命名规则时按"不同即有更新"处理，不因形态不符而卡住用户。
        assert!(update_available(Some("2026.07.04"), "nightly-abc"));
    }

    /// 落点在 `tools/` 而非模型目录：它是可执行程序不是权重，混放会被"删模型腾空间"误伤。
    #[test]
    fn managed_path_lives_under_tools() {
        assert!(managed_rel_path().starts_with("tools/"));
        assert!(VERSION_SIDECAR.starts_with("tools/"));
        #[cfg(target_os = "windows")]
        assert!(managed_rel_path().ends_with(".exe"));
    }
}
