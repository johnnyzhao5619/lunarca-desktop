//! 抓取失败的**稳定错误码**与 stderr 分类（TDD-03 §11.4）。
//!
//! 抓取跨越应用无法控制的三方（站点改版、区域限制、登录墙、代理不通），失败是常态而非异常，
//! 故错误必须**可读且可行动**：稳定 code 交给前端本地化为「这条视频需要登录，Lunarca 不支持」
//! 一类的人话，而不是把 yt-dlp 的英文 traceback 原样糊到界面上。
//!
//! 隐私铁律：分类只读 stderr 的**关键词**，`FetchError` 绝不携带 URL、cookie 或 stderr 原文
//! （日志亦然，TDD-12）——诊断信息经 [`FetchError::code`] 与站点名表达即已足够。

use std::fmt;

/// 抓取错误（稳定 code 面向 IPC / i18n，`Display` 面向脱敏日志）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FetchError {
    /// 未找到 yt-dlp 可执行文件（未安装 / 不在候选位置 / 设置里的路径不存在）。
    ToolMissing,
    /// 找到了但跑不起来（权限不足 / 架构不符 / 文件损坏）。
    ToolUnusable,
    /// yt-dlp 版本过旧——站点提取器已失效（yt-dlp 自陈 "update" / nsig 提取失败）。
    ToolOutdated,
    /// 站点**拒绝了媒体下载本身**（`HTTP 403`）：元信息取到了、取流地址也拿到了，
    /// 唯独取字节时被拒。这是站点反爬策略变动的典型现场——工具版本落后于策略，
    /// 或提取时缺 JS 运行时而回落到了一个被站点封掉的播放器客户端。
    HostRejected,
    /// 缺 JS 运行时（deno / node / bun）：yt-dlp 解不了站点的 JS 挑战，
    /// 于是"能看见的格式"里没有一个是我们要的。**装个运行时**是唯一有效动作，
    /// 与"装 ffmpeg"（[`Self::NoCompatibleFormat`] 的常规成因）是两回事，不能混为一谈。
    JsRuntimeMissing,
    /// URL 不是 http(s)，或形状上不可能是媒体页（拒绝 `file://` 等本地 scheme）。
    UnsupportedUrl,
    /// 站点不支持 / 页面无可提取媒体。
    UnsupportedSite,
    /// 需要登录、会员、年龄验证或已设为私享。
    LoginRequired,
    /// 区域限制。
    GeoBlocked,
    /// 视频已下架 / 被删除。
    Unavailable,
    /// 正在直播——无限长流，转写没有终点，故诚实拒绝而不是下到磁盘满。
    LiveStream,
    /// 网络不通 / 代理失败 / 超时。
    Network,
    /// 无可用的兼容格式（拿不到 Lunarca 可解码的容器；通常需要 ffmpeg 合并）。
    NoCompatibleFormat,
    /// 目标磁盘剩余空间不足以放下这份媒体。
    InsufficientDisk,
    /// 严格本地模式已开启，联网抓取被硬拦截（AGENTS.md §7）。
    StrictLocalBlocked,
    /// 用户取消。
    Cancelled,
    /// 下载进程异常退出，且 stderr 无可识别指纹（保留原始退出码作诊断线索）。
    ToolFailed {
        /// 子进程退出码（`None` = 被信号终止）。
        exit_code: Option<i32>,
    },
    /// 下载"成功"但没有产出可用文件（yt-dlp 未打印落点 / 文件不存在）。
    NoOutput,
    /// 探测输出不是预期的 JSON 形状。
    BadProbeOutput,
    /// 本地 IO 失败（建临时目录 / 移动文件）。
    Io(String),
}

impl FetchError {
    /// 稳定错误码（前端据此选本地化文案；**契约，改动即破坏 i18n**）。
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::ToolMissing => "FETCH_TOOL_MISSING",
            Self::ToolUnusable => "FETCH_TOOL_UNUSABLE",
            Self::ToolOutdated => "FETCH_TOOL_OUTDATED",
            Self::HostRejected => "FETCH_HOST_REJECTED",
            Self::JsRuntimeMissing => "FETCH_JS_RUNTIME_MISSING",
            Self::UnsupportedUrl => "FETCH_UNSUPPORTED_URL",
            Self::UnsupportedSite => "FETCH_UNSUPPORTED_SITE",
            Self::LoginRequired => "FETCH_LOGIN_REQUIRED",
            Self::GeoBlocked => "FETCH_GEO_BLOCKED",
            Self::Unavailable => "FETCH_UNAVAILABLE",
            Self::LiveStream => "FETCH_LIVE_STREAM",
            Self::Network => "FETCH_NETWORK",
            Self::NoCompatibleFormat => "FETCH_NO_COMPATIBLE_FORMAT",
            Self::InsufficientDisk => "FETCH_INSUFFICIENT_DISK",
            Self::StrictLocalBlocked => "STRICT_LOCAL_BLOCKED",
            Self::Cancelled => "FETCH_CANCELLED",
            Self::ToolFailed { .. } => "FETCH_TOOL_FAILED",
            Self::NoOutput => "FETCH_NO_OUTPUT",
            Self::BadProbeOutput => "FETCH_BAD_PROBE_OUTPUT",
            Self::Io(_) => "FETCH_IO",
        }
    }

    /// 用户改环境（装工具 / 关严格本地 / 换网络）后**同一 URL 有望成功** → 值得给「重试」。
    /// 站点侧的硬事实（已下架 / 区域限制 / 直播 / 登录墙）重试只会重复失败，不该诱导用户空等。
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::ToolMissing
                | Self::ToolUnusable
                | Self::ToolOutdated
                // 这两条都是**环境可改**：更新工具 / 装个 JS 运行时之后同一条链接有望成功，
                // 故值得给「重试」——与"已下架 / 区域限制"那类站点侧硬事实不同。
                | Self::HostRejected
                | Self::JsRuntimeMissing
                | Self::Network
                | Self::InsufficientDisk
                | Self::StrictLocalBlocked
                | Self::ToolFailed { .. }
                | Self::Io(_)
        )
    }
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ToolFailed { exit_code: Some(c) } => write!(f, "{} (exit {c})", self.code()),
            Self::Io(detail) => write!(f, "{}: {detail}", self.code()),
            _ => f.write_str(self.code()),
        }
    }
}

impl std::error::Error for FetchError {}

impl From<std::io::Error> for FetchError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err.to_string())
    }
}

/// 全部变体的样本（码表、判据、守护测试的**共同基础**）。
///
/// 新增 `FetchError` 变体时必须加进来——`every_code_is_unique_and_prefixed` 与
/// `retryable_for_code_round_trips_every_variant` 两条守护都建立在它之上，漏加即少一条覆盖。
/// 带字段的变体用占位值（判据只看变体本身，不看载荷）。
#[must_use]
pub fn all_variants() -> Vec<FetchError> {
    vec![
        FetchError::ToolMissing,
        FetchError::ToolUnusable,
        FetchError::ToolOutdated,
        FetchError::HostRejected,
        FetchError::JsRuntimeMissing,
        FetchError::UnsupportedUrl,
        FetchError::UnsupportedSite,
        FetchError::LoginRequired,
        FetchError::GeoBlocked,
        FetchError::Unavailable,
        FetchError::LiveStream,
        FetchError::Network,
        FetchError::NoCompatibleFormat,
        FetchError::InsufficientDisk,
        FetchError::StrictLocalBlocked,
        FetchError::Cancelled,
        FetchError::ToolFailed { exit_code: None },
        FetchError::NoOutput,
        FetchError::BadProbeOutput,
        FetchError::Io(String::new()),
    ]
}

/// 稳定错误码 → **是否值得重试**（`None` = 这不是抓取错误码，调用方自行决定默认）。
///
/// 队列的失败终态只携带错误码串（DTO 脱敏，不带结构化错误），故需要这条**反向查询**把
/// 「码」还原成判据。容忍带尾注的形态（`FETCH_TOOL_FAILED (exit 1)` / `FETCH_IO: …`），
/// 只取首段——`Display` 会附退出码与 IO 细节。
#[must_use]
pub fn retryable_for_code(code: &str) -> Option<bool> {
    let head = code.trim().split([' ', ':', '(']).next()?;
    all_variants()
        .into_iter()
        .find(|err| err.code() == head)
        .map(|err| err.is_retryable())
}

/// yt-dlp 的诊断输出 → 稳定错误码（**纯函数**，可单测）。
///
/// yt-dlp 没有细分退出码（失败一律 1），唯一的结构化线索是它吐的文本，故这里做**关键词分类**。
/// 判定顺序即优先级：先认「工具该更新」（它会连带表现为提取失败，误判成"站点不支持"会让用户
/// 去怀疑站点而不是升级工具），再认站点侧硬事实，最后才落到网络与兜底。
/// 匹配一律在小写化后进行，且只认 yt-dlp 长期稳定的措辞片段。
///
/// **判据只取非警告行**：我们刻意不再传 `--no-warnings`（缺 JS 运行时这类关键线索只写在
/// 警告里，见 [`FetchTool::base_args`](crate::tool::FetchTool::base_args)），而警告文本里
/// 到处是 `--cookies-from-browser` 一类的建议措辞——若与错误行混在一起匹配，一次纯粹的
/// 网络失败会被"警告里提了 cookies"劫持成「需要登录」。故：**错误行定性，警告行只做细化**。
#[must_use]
pub fn classify_stderr(diagnostics: &str, exit_code: Option<i32>) -> FetchError {
    let lower = diagnostics.to_ascii_lowercase();
    // 警告行（`WARNING: …`）不参与定性。stdout 上的说明（`--match-filter` 的 skip）无前缀，
    // 故自然留在错误集里——它正是"直播被我们自己的过滤器拦下"的唯一证据。
    let fatal: String = lower
        .lines()
        .filter(|line| !line.trim_start().starts_with("warning:"))
        .collect::<Vec<_>>()
        .join("\n");
    let has = |needle: &str| fatal.contains(needle);
    // 警告只用于**细化已定性的失败**（同一句失败的两种成因该给两种动作），绝不单独定性。
    let warned = |needle: &str| lower.contains(needle);

    // ① 工具自陈过旧 / 签名提取失效（YouTube 反爬变动的典型指纹）。
    if has("please update") || has("yt-dlp is out of date") || has("update to nightly") {
        return FetchError::ToolOutdated;
    }
    if has("nsig extraction failed") || has("unable to extract nsig") || has("signature extraction")
    {
        return FetchError::ToolOutdated;
    }
    // **认不得我们下的选项，同样是"太旧"**：`--js-runtimes` 是 yt-dlp 较新版本才有的
    // （见 [`crate::tool`]）。一份老到没有它的工具会以 `no such option` 退出——若不认这条指纹，
    // 用户拿到的又是那句无信息的"下载失败，请稍后重试"，而正确的动作恰恰仍是升级。
    // 这不是兼容层（我们不为老版本分叉参数），是把"太旧"如实说出口。
    if has("no such option") || has("unrecognized arguments") {
        return FetchError::ToolOutdated;
    }
    // ② 登录 / 会员 / 年龄 / 私享（同一类用户动作：Lunarca 不代持凭据，见 §11.5 裁决）。
    if has("sign in to confirm")
        || has("login required")
        || has("members-only")
        || has("private video")
        || has("this video is private")
        || has("age-restricted")
        || has("confirm your age")
        || has("cookies")
    {
        return FetchError::LoginRequired;
    }
    // ③ 区域限制。措辞在两种主流形态间摇摆（"not available in your country" 与
    // "has not made this video available in your country"），故只认稳定的公共尾串。
    if has("in your country") || has("geo restricted") || has("geo-restricted") {
        return FetchError::GeoBlocked;
    }
    // ④ 已下架 / 不存在。
    if has("video unavailable")
        || has("has been removed")
        || has("has been terminated")
        || has("no longer available")
        || has("404")
    {
        return FetchError::Unavailable;
    }
    // ⑤ 直播（探测阶段已拦一道；这里兜住"探测后才开播"的竞态）。
    // `does not pass filter` = 我们自己下的 `--match-filter "!is_live"` 生效了，
    // 它在 stdout 而非 stderr，故调用方须把两路诊断合并后再送进来。
    if has("is live")
        || has("live event will begin")
        || has("premieres in")
        || has("does not pass filter")
    {
        return FetchError::LiveStream;
    }
    // ⑥ 无可用格式。常规成因是"需要 ffmpeg 合并却没装"，但**缺 JS 运行时会先表现成同一句话**
    // ——站点把需要解挑战才能拿到的格式全藏了，于是我们要的容器一个都不在清单里。
    // 这时说"装上 ffmpeg 通常能解决"是**错的建议**，用户装完 ffmpeg 仍会失败。
    if has("requested format is not available") || has("no video formats found") {
        return if warned(NO_JS_RUNTIME) {
            FetchError::JsRuntimeMissing
        } else {
            FetchError::NoCompatibleFormat
        };
    }
    // ⑦ 站点不支持 / 页面不是媒体页。
    if has("unsupported url") || has("no suitable extractor") || has("unable to extract") {
        return FetchError::UnsupportedSite;
    }
    // ⑧ 站点在**取字节这一步**拒绝了我们（403）。元信息与取流地址都拿到了，唯独下载被拒
    // ——这是站点反爬策略变动的典型现场，2026-08-23 真机缺陷即此：工具停在 2026.07.04，
    // 而上游早已把被站点封掉的那个播放器客户端从默认表里移除。它**不是**网络故障，
    // 把它归到"检查网络/配代理"只会让用户在正确的动作（更新工具）之外白费力气。
    //
    // 指纹**只认 `http error 403` 这个状态码**，不认外层的 `unable to download video data`
    // ——后者是 yt-dlp 对"取字节失败"的通用外壳，里面同样可能包着 `<urlopen error …>`
    // （那是真的网络不通，该走 ⑨ 让用户去查代理）。拿外壳当指纹会把一类网络故障
    // 误导成"去更新工具"。
    if has("http error 403") {
        return FetchError::HostRejected;
    }
    // ⑨ 网络 / 代理（放在站点判定之后：站点侧消息里也常带 "urlopen error"）。
    if has("urlopen error")
        || has("connection refused")
        || has("connection reset")
        || has("temporary failure in name resolution")
        || has("timed out")
        || has("proxy")
        || has("network is unreachable")
        || has("ssl")
    {
        return FetchError::Network;
    }
    // ⑩ 磁盘。
    if has("no space left") {
        return FetchError::InsufficientDisk;
    }
    // ⑪ 兜底之前的最后一问：如果连原因都认不出、而 yt-dlp 又抱怨过没有 JS 运行时，
    // 那句抱怨就是我们手上**最可行动**的线索——总好过一句"下载失败，请稍后重试"。
    if warned(NO_JS_RUNTIME) {
        return FetchError::JsRuntimeMissing;
    }
    FetchError::ToolFailed { exit_code }
}

/// yt-dlp 抱怨"一个可用的 JS 运行时都没有"时的稳定措辞片段（它只写在 `WARNING:` 行里）。
const NO_JS_RUNTIME: &str = "no supported javascript runtime";

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个变体的 code 必须唯一——它是 i18n 的键，撞号会让两种失败显示同一句话。
    #[test]
    fn every_code_is_unique_and_prefixed() {
        let all = all_variants();
        let mut codes: Vec<&str> = all.iter().map(FetchError::code).collect();
        let total = codes.len();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), total, "错误码必须唯一");
        // 严格本地是全应用共享的既有码，不加 FETCH_ 前缀；其余一律同族前缀便于前端分派。
        assert!(codes
            .iter()
            .all(|c| c.starts_with("FETCH_") || *c == "STRICT_LOCAL_BLOCKED"));
    }

    /// 版本过旧的指纹必须先于「站点不支持」命中——否则用户会去怀疑 bilibili 改版，
    /// 而真正的动作是升级 yt-dlp。
    #[test]
    fn outdated_tool_wins_over_extractor_failure() {
        let stderr = "ERROR: [youtube] abc: Unable to extract nsig function; \
                      please report this issue. Please update yt-dlp to nightly";
        assert_eq!(classify_stderr(stderr, Some(1)), FetchError::ToolOutdated);
    }

    /// 老到不认识 `--js-runtimes` 的工具会以 `no such option` 退出——那也是"太旧"，
    /// 必须这么说，而不是又落回一句无信息的"下载失败，请稍后重试"。
    #[test]
    fn unknown_option_means_the_tool_is_too_old() {
        let stderr = "Usage: yt-dlp [OPTIONS] URL\n\
                      yt-dlp: error: no such option: --js-runtimes";
        assert_eq!(classify_stderr(stderr, Some(2)), FetchError::ToolOutdated);
    }

    #[test]
    fn classifies_site_side_facts() {
        assert_eq!(
            classify_stderr(
                "ERROR: [youtube] xx: Private video. Sign in if you've been granted access",
                Some(1)
            ),
            FetchError::LoginRequired
        );
        assert_eq!(
            classify_stderr(
                "ERROR: [youtube] xx: Video unavailable. This video has been removed",
                Some(1)
            ),
            FetchError::Unavailable
        );
        assert_eq!(
            classify_stderr(
                "ERROR: The uploader has not made this video available in your country",
                Some(1)
            ),
            FetchError::GeoBlocked
        );
        assert_eq!(
            classify_stderr("ERROR: Unsupported URL: https://example.com/page", Some(1)),
            FetchError::UnsupportedSite
        );
        assert_eq!(
            classify_stderr("ERROR: requested format is not available", Some(1)),
            FetchError::NoCompatibleFormat
        );
    }

    /// 网络判定必须**让位**给站点侧消息：站点错误里也常夹着 urlopen/ssl 字样，
    /// 若网络判定抢先，用户会被引导去检查代理而不是"这条视频要登录"。
    #[test]
    fn network_classification_yields_to_site_facts() {
        let stderr = "ERROR: unable to download webpage: <urlopen error [Errno 61] \
                      Connection refused>";
        assert_eq!(classify_stderr(stderr, Some(1)), FetchError::Network);

        let mixed = "ERROR: [bilibili] 1: Private video; <urlopen error timed out>";
        assert_eq!(classify_stderr(mixed, Some(1)), FetchError::LoginRequired);
    }

    /// 我们自下的 `--match-filter "!is_live"` 拦下条目时的说明走 stdout，且措辞是
    /// 「does not pass filter」——认不出它就会把"这是直播"降格成含糊的"没有产出"。
    #[test]
    fn match_filter_skip_is_recognized_as_live() {
        let stdout = "[download] skipping abc: does not pass filter (!is_live), test skipping";
        assert_eq!(classify_stderr(stdout, Some(0)), FetchError::LiveStream);
    }

    /// 2026-08-23 真机缺陷的回归门禁：元信息取到了、下载被站点 403 拒了。
    /// 它此前落到无指纹的 `ToolFailed`，界面因此只会说"下载失败，请稍后重试"——
    /// 而真正的动作是**更新 yt-dlp**。
    #[test]
    fn media_download_403_is_a_host_rejection_not_an_unknown_failure() {
        let stderr = "ERROR: unable to download video data: HTTP Error 403: Forbidden";
        assert_eq!(classify_stderr(stderr, Some(1)), FetchError::HostRejected);
        assert!(
            FetchError::HostRejected.is_retryable(),
            "更新工具后同一条链接有望成功"
        );
        // 同一句外壳里包着的若是真的网络不通，判据必须落回「网络」——指引用户去查代理，
        // 而不是去更新一个没有问题的工具。
        let networky = "ERROR: unable to download video data: <urlopen error timed out>";
        assert_eq!(classify_stderr(networky, Some(1)), FetchError::Network);
    }

    /// 警告行**不得定性**：yt-dlp 的建议措辞里到处是 `--cookies-from-browser`，
    /// 与错误行混在一起匹配会把一次纯网络失败劫持成「需要登录」（去掉 `--no-warnings` 后
    /// 这些警告才真正流进分类器，故这条守护与那个改动同生）。
    #[test]
    fn warning_lines_never_decide_the_verdict() {
        let mixed = "WARNING: [youtube] falling back; consider --cookies-from-browser chrome\n\
                     ERROR: unable to download webpage: <urlopen error timed out>";
        assert_eq!(classify_stderr(mixed, Some(1)), FetchError::Network);
    }

    /// 缺 JS 运行时会先表现成"没有可用格式"——此时建议"装 ffmpeg"是错的，
    /// 用户装完仍会失败。两种成因必须给两种动作。
    #[test]
    fn missing_js_runtime_outranks_the_generic_no_format_advice() {
        let with_runtime = "ERROR: [youtube] x: Requested format is not available";
        assert_eq!(
            classify_stderr(with_runtime, Some(1)),
            FetchError::NoCompatibleFormat
        );

        let without = "WARNING: [youtube] No supported JavaScript runtime could be found. \
                       Only deno is enabled by default\n\
                       ERROR: [youtube] x: Requested format is not available";
        assert_eq!(
            classify_stderr(without, Some(1)),
            FetchError::JsRuntimeMissing
        );
    }

    /// 认不出原因、但工具抱怨过没有 JS 运行时时，那句抱怨就是最可行动的线索——
    /// 好过一句无信息的"稍后重试"。
    #[test]
    fn unrecognized_failure_prefers_the_js_runtime_hint_over_the_generic_fallback() {
        let noise = "WARNING: [youtube] No supported JavaScript runtime could be found.\n\
                     ERROR: something entirely new";
        assert_eq!(
            classify_stderr(noise, Some(1)),
            FetchError::JsRuntimeMissing
        );
    }

    #[test]
    fn unknown_stderr_falls_back_to_exit_code() {
        assert_eq!(
            classify_stderr("something entirely new", Some(2)),
            FetchError::ToolFailed { exit_code: Some(2) }
        );
    }

    /// 「重试有意义」的判定：环境类失败可重试，站点侧硬事实不可——否则界面会给出
    /// 一个注定再次失败的按钮。
    #[test]
    fn retryable_covers_environment_not_site_facts() {
        assert!(FetchError::Network.is_retryable());
        assert!(FetchError::ToolMissing.is_retryable());
        assert!(FetchError::StrictLocalBlocked.is_retryable());
        assert!(!FetchError::LiveStream.is_retryable());
        assert!(!FetchError::GeoBlocked.is_retryable());
        assert!(!FetchError::LoginRequired.is_retryable());
        assert!(!FetchError::Cancelled.is_retryable());
    }

    /// 码 → 判据的**反向查询必须对每个变体都成立**：队列的失败终态只带错误码串，
    /// 若某个变体查不回来，它在界面上就会落到"未知码 → 默认可重试"，于是登录墙也会
    /// 长出一个注定失败的「重试」按钮。
    #[test]
    fn retryable_for_code_round_trips_every_variant() {
        for err in all_variants() {
            assert_eq!(
                retryable_for_code(err.code()),
                Some(err.is_retryable()),
                "{} 查不回判据",
                err.code()
            );
        }
        // 带尾注的形态（`Display` 会附退出码 / IO 细节）同样要认出来。
        assert_eq!(retryable_for_code("FETCH_TOOL_FAILED (exit 1)"), Some(true));
        assert_eq!(
            retryable_for_code("FETCH_IO: permission denied"),
            Some(true)
        );
        // 非抓取错误码返回 None，由调用方决定默认（队列侧按可重试处理）。
        assert_eq!(retryable_for_code("DECODE_FAILED"), None);
        assert_eq!(retryable_for_code(""), None);
    }

    /// `Display` 只吐 code（+ 退出码），绝不含 URL/stderr 原文——它会进日志。
    #[test]
    fn display_stays_redacted() {
        assert_eq!(
            FetchError::ToolFailed { exit_code: Some(1) }.to_string(),
            "FETCH_TOOL_FAILED (exit 1)"
        );
        assert_eq!(FetchError::LiveStream.to_string(), "FETCH_LIVE_STREAM");
    }
}
