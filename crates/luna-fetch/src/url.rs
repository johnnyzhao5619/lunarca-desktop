//! 抓取入口的 URL 校验与脱敏（纯逻辑，TDD-03 §11.2）。
//!
//! 这是整条抓取链路唯一的**不可信输入**，故校验放在最前面且只放一处：
//! ① **仅 http/https**——`file://` 会让"抓取网络视频"变成读本机任意文件，
//!    `-` 开头的字符串会被 yt-dlp 当作选项解析（`--exec` 一类等于任意命令执行）；
//! ② **不做站点白名单**——yt-dlp 支持上千站点且持续增删，白名单只会让"明明能下的站点
//!    Lunarca 说不支持"，判据交给工具本身（拿不到媒体就诚实报 `UnsupportedSite`）。
//!
//! 隐私：URL 可能是私享链接、含一次性 token 的签名地址，故日志一律只出 [`redact`] 的
//! 主机名，绝不出完整 URL（TDD-12）。

use crate::error::FetchError;

/// 校验并归一化用户输入的媒体页地址。
///
/// 归一化只做**去首尾空白**——查询串一律保留：`?v=` 是 YouTube 的视频 id，`?t=` 是起播点，
/// 擅自裁剪等于把用户粘的链接改成另一条。
///
/// # Errors
/// 非 http(s)、空串、含控制字符或空白时返回 [`FetchError::UnsupportedUrl`]。
pub fn normalize(raw: &str) -> Result<String, FetchError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(FetchError::UnsupportedUrl);
    }
    // 内嵌空白/控制字符：既非合法 URL，也是"一次传两个参数"的注入形状。
    if trimmed.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(FetchError::UnsupportedUrl);
    }
    let lower = trimmed.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return Err(FetchError::UnsupportedUrl);
    }
    // scheme 之后必须有主机名（`https://` 光秃秃的不是页面地址）。
    let rest = &trimmed[lower.find("//").map_or(trimmed.len(), |i| i + 2)..];
    let host_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    if rest[..host_end].is_empty() {
        return Err(FetchError::UnsupportedUrl);
    }
    Ok(trimmed.to_owned())
}

/// 主机名（去 `www.` 前缀），用于**展示**「来自 youtube.com」与脱敏日志。
///
/// 输入未必已过 [`normalize`]（探测结果里的 entry URL 亦经此处），故形状不符时返回空串
/// 而非 panic。
#[must_use]
pub fn host_of(url: &str) -> String {
    let after_scheme = url.find("://").map_or(url, |i| &url[i + 3..]);
    let host_end = after_scheme
        .find(['/', '?', '#'])
        .unwrap_or(after_scheme.len());
    let host = &after_scheme[..host_end];
    // 去掉 userinfo 与端口（`user@host:443` → `host`）。
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host);
    host.strip_prefix("www.")
        .unwrap_or(host)
        .to_ascii_lowercase()
}

/// 日志安全的 URL 投影：**只留主机名**（`https://host/…`）。
///
/// 路径与查询串都可能是身份（私享 id、签名 token），故一并抹掉——诊断"下载失败"需要知道的
/// 是哪个站点，不是哪条视频。
#[must_use]
pub fn redact(url: &str) -> String {
    let host = host_of(url);
    if host.is_empty() {
        "<url>".to_owned()
    } else {
        format!("{host}/…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_http_and_https_and_keeps_query() {
        assert_eq!(
            normalize("  https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=30s  ").unwrap(),
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=30s",
            "查询串是视频身份与起播点，不得裁剪"
        );
        assert_eq!(
            normalize("http://example.com/a").unwrap(),
            "http://example.com/a"
        );
        // scheme 大小写不敏感（用户可能从富文本里粘到 HTTPS://）。
        assert_eq!(
            normalize("HTTPS://B.com/x").unwrap(),
            "HTTPS://B.com/x",
            "只用小写副本判 scheme，原样交给工具"
        );
    }

    /// 非 http(s) 一律拒绝——这是"抓网络视频"不会变成"读本机文件"的唯一护栏。
    #[test]
    fn rejects_non_http_schemes() {
        for bad in [
            "file:///etc/passwd",
            "ftp://host/x",
            "data:text/plain,hi",
            "javascript:alert(1)",
            "/Users/me/movie.mp4",
            "youtube.com/watch?v=1",
        ] {
            assert_eq!(
                normalize(bad),
                Err(FetchError::UnsupportedUrl),
                "必须拒绝：{bad}"
            );
        }
    }

    /// `-` 开头会被子进程当选项解析（`--exec` = 任意命令执行）；空白分隔等于偷塞第二个参数。
    #[test]
    fn rejects_option_shaped_and_whitespace_split_input() {
        assert_eq!(
            normalize("--exec=rm -rf /"),
            Err(FetchError::UnsupportedUrl)
        );
        assert_eq!(
            normalize("https://a.com/x --exec touch /tmp/pwn"),
            Err(FetchError::UnsupportedUrl)
        );
        assert_eq!(
            normalize("https://a.com/x\n--exec"),
            Err(FetchError::UnsupportedUrl)
        );
        assert_eq!(normalize("   "), Err(FetchError::UnsupportedUrl));
        assert_eq!(normalize("https://"), Err(FetchError::UnsupportedUrl));
    }

    #[test]
    fn host_strips_www_userinfo_and_port() {
        assert_eq!(
            host_of("https://www.bilibili.com/video/BV1"),
            "bilibili.com"
        );
        assert_eq!(host_of("https://user@Host.COM:8443/x?y=1"), "host.com");
        assert_eq!(host_of("not a url"), "not a url");
    }

    /// 脱敏必须抹掉路径与查询串——它们可能整个就是身份凭据。
    #[test]
    fn redact_keeps_only_host() {
        assert_eq!(
            redact("https://youtu.be/SECRET_ID?token=abc123"),
            "youtu.be/…"
        );
        assert_eq!(redact(""), "<url>");
    }
}
