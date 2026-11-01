//! 抓取前的**元信息探测**（TDD-03 §11.2）。
//!
//! 为什么必须先探测再下载：粘错一条链接、或那是个 3 小时的直播、或那是个 200 集的播放列表，
//! 用户都应该**在花掉带宽与磁盘之前**看见标题与时长。这也是唯一能在下载前拦住直播
//! （无终点流）与识别播放列表（一次入队几十件）的时机。
//!
//! 实现：`yt-dlp -J --flat-playlist`（不下载、单行 JSON）。`--flat-playlist` 让播放列表只出
//! 条目清单而不逐条深挖——200 集逐条提取要几分钟，而清单确认只需要标题与时长。
//!
//! 隐私：探测结果里的 URL 会随任务进续跑档（设备本地），但日志与事件一律只出脱敏主机名。

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::FetchError;
use crate::tool::FetchTool;
use crate::url;

/// 单条可抓取媒体。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FetchEntry {
    /// 站点侧 id（播放列表内去重与稳定 key）。
    pub id: String,
    /// 标题（→ 队列标签 + 笔记名，经 `safe_media_stem` 净化后落盘）。
    pub title: String,
    /// 该条目的可抓取地址（播放列表条目的 `url` 字段；单视频即原 URL）。
    pub url: String,
    /// 时长（毫秒）；`None` = 站点未给（`--flat-playlist` 下常见）。
    pub duration_ms: Option<u64>,
    /// 上传者 / 频道（确认卡上"是不是我要的那个视频"的第二判据）。
    pub uploader: Option<String>,
    /// 预估体积（字节）；`None` = 未知（据此做磁盘预检，未知则跳过预检不阻断）。
    pub filesize: Option<u64>,
    /// 正在直播 / 待首播——**不可抓取**（无终点），入队前一律拒绝。
    pub is_live: bool,
}

/// 探测结果。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FetchProbe {
    /// 站点主机名（脱敏展示"来自 youtube.com"）。
    pub site: String,
    /// 播放列表标题（`None` = 单条媒体）。
    pub playlist_title: Option<String>,
    /// 条目清单（单条媒体即长度 1；播放列表已按 [`PLAYLIST_LIMIT`] 截断）。
    pub entries: Vec<FetchEntry>,
    /// 因超过 [`PLAYLIST_LIMIT`] 而未列出的条目数（>0 时界面须如实告知，绝不静默截断）。
    pub truncated: u32,
}

impl FetchProbe {
    /// 是否为播放列表（决定界面给单卡还是给可勾选清单）。
    #[must_use]
    pub fn is_playlist(&self) -> bool {
        self.playlist_title.is_some() || self.entries.len() > 1
    }
}

/// 播放列表条目上限。
///
/// > 假设：50 条足以覆盖"一门课程/一个专辑"的真实用量（理由：更大的量级是批量抓取而非
/// > 转写工作流，那既拖垮单 worker 队列，也把工具推向站点 ToS 的灰区。超出部分**如实告知**
/// > 而不是静默丢弃，用户可自行分批）。
pub const PLAYLIST_LIMIT: usize = 50;

/// 探测超时：站点提取慢是常态（反爬挑战、多次跳转），但超过这个数就该让用户重试而不是干等。
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(45);

/// 探测一条 URL（超时取 [`PROBE_TIMEOUT`]）。
///
/// # Errors
/// URL 非法、工具失败（分类见 [`crate::error::classify_stderr`]）、超时、
/// 输出非预期 JSON 时返回对应 [`FetchError`]。
pub async fn probe(tool: &FetchTool, raw_url: &str) -> Result<FetchProbe, FetchError> {
    probe_with_timeout(tool, raw_url, PROBE_TIMEOUT).await
}

/// 同 [`probe`]，但**超时由调用方给定**。
///
/// 存在的理由是**可测**，不是给生产多一个旋钮：45 秒是按真实站点提取的耗时定的，
/// 而假工具跑完只需毫秒——测试若继承生产值，它断言的就不再是"stderr 分类对不对"，
/// 而是"这台机器 45 秒内调度得完子进程吗"。整仓 `cargo test --workspace` 并行跑时
/// 这个隐含前提被打破过：子进程没在 45 秒内跑完，`probe` 返回超时（`Network`），
/// 于是一条与超时毫无关系的分类用例变红（2026-08-07）。
/// **一个会随机变红的门禁与没有门禁是一回事**，故把超时做成入参：分类用例给足余量、
/// 超时用例给极短值配长睡的假工具——两个方向都与机器负载无关。
///
/// # Errors
/// 同 [`probe`]；超过 `timeout` 未出结果时返回 [`FetchError::Network`]（可重试）。
pub async fn probe_with_timeout(
    tool: &FetchTool,
    raw_url: &str,
    timeout: Duration,
) -> Result<FetchProbe, FetchError> {
    let url = url::normalize(raw_url)?;
    let mut args = tool.base_args();
    args.extend([
        "-J".to_owned(),
        // 扁平清单：只要条目标题与时长，不逐条深挖（200 集逐条提取要几分钟）。
        "--flat-playlist".to_owned(),
        // **不加 `--playlist-items` 截断**：那样拿回来的条数就是我们要的上限本身，
        // 于是"还有多少条没列出"只能瞎猜（曾经的写法固定报"还有 1 条"，而真实可能是 150 条）。
        // 扁平模式对整列表也只是一次提取，取全量再自己截断，`truncated` 才是真数。
        // `--` 终止选项解析：即便 URL 校验被绕过，也不会被当成选项。
        "--".to_owned(),
        url.clone(),
    ]);

    let output = tokio::process::Command::new(&tool.path)
        .args(&args)
        .kill_on_drop(true)
        .output();
    let output = match tokio::time::timeout(timeout, output).await {
        Ok(Ok(output)) => output,
        Ok(Err(err)) => return Err(FetchError::Io(err.to_string())),
        Err(_) => return Err(FetchError::Network),
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(crate::error::classify_stderr(&stderr, output.status.code()));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_probe(&stdout, &url)
}

/// 解析 `-J` 输出（**纯函数**，可单测——真实站点不进单测）。
///
/// # Errors
/// 非 JSON / 无任何条目时返回 [`FetchError::BadProbeOutput`]。
pub fn parse_probe(stdout: &str, request_url: &str) -> Result<FetchProbe, FetchError> {
    let root: serde_json::Value =
        serde_json::from_str(stdout.trim()).map_err(|_| FetchError::BadProbeOutput)?;
    let site = url::host_of(request_url);

    // 播放列表：`_type: "playlist"` 且带 entries。单视频：无 entries 的扁平对象。
    let is_playlist = root.get("_type").and_then(serde_json::Value::as_str) == Some("playlist")
        || root.get("entries").is_some();
    if !is_playlist {
        let entry = parse_entry(&root, request_url).ok_or(FetchError::BadProbeOutput)?;
        return Ok(FetchProbe {
            site,
            playlist_title: None,
            entries: vec![entry],
            truncated: 0,
        });
    }

    let raw_entries = root
        .get("entries")
        .and_then(serde_json::Value::as_array)
        .ok_or(FetchError::BadProbeOutput)?;
    // 嵌套播放列表（列表里套列表）会让 entry 没有可播地址 → 一律过滤，不假装能抓。
    let mut entries: Vec<FetchEntry> = raw_entries
        .iter()
        .filter(|e| e.get("_type").and_then(serde_json::Value::as_str) != Some("playlist"))
        .filter_map(|e| parse_entry(e, request_url))
        .collect();
    if entries.is_empty() {
        return Err(FetchError::BadProbeOutput);
    }
    // 站点自报总数可能大于我们拿到的条数（超大频道 yt-dlp 侧也会分页）→ 取较大者作总量，
    // 宁可把"还有多少条"报得更全，也不让用户以为清单就是全部。
    let declared = root
        .get("playlist_count")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as usize;
    let total = declared.max(entries.len());
    let truncated = total.saturating_sub(PLAYLIST_LIMIT) as u32;
    entries.truncate(PLAYLIST_LIMIT);
    Ok(FetchProbe {
        site,
        playlist_title: root
            .get("title")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        entries,
        truncated,
    })
}

/// 单条目解析；缺少可抓取地址即视为不可用条目（返回 `None`，由调用方过滤）。
fn parse_entry(value: &serde_json::Value, request_url: &str) -> Option<FetchEntry> {
    let str_of = |key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    // 抓取地址优先取条目自带的（播放列表条目），否则用用户输入的原 URL（单视频）。
    // `webpage_url` 是 yt-dlp 归一化后的规范地址，优于 `url`（后者在扁平模式下也是页面地址，
    // 但在单视频模式下可能是直链且带时效签名）。
    let target = str_of("webpage_url")
        .or_else(|| str_of("url"))
        .unwrap_or_else(|| request_url.to_owned());
    // 直链（带签名的 media 地址）不能作为抓取入口：它会过期，且绕过提取器。
    let target = if target.starts_with("http") {
        target
    } else {
        request_url.to_owned()
    };
    let id = str_of("id").unwrap_or_else(|| target.clone());
    // 标题缺失（部分站点扁平模式不给）时回落 id——绝不留空标题，那会产出一篇无名笔记。
    let title = str_of("title").unwrap_or_else(|| id.clone());
    // duration 是秒（可能是浮点）。
    let duration_ms = value
        .get("duration")
        .and_then(serde_json::Value::as_f64)
        .filter(|d| d.is_finite() && *d > 0.0)
        .map(|d| (d * 1000.0) as u64);
    let filesize = ["filesize", "filesize_approx"]
        .iter()
        .find_map(|key| value.get(*key).and_then(serde_json::Value::as_u64))
        .filter(|size| *size > 0);
    // `is_live` 为真、或 `live_status` 明确处于直播/待播 → 不可抓取。
    let live_status = value.get("live_status").and_then(serde_json::Value::as_str);
    let is_live = value
        .get("is_live")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
        || matches!(live_status, Some("is_live" | "is_upcoming" | "post_live"));
    Some(FetchEntry {
        id,
        title,
        url: target,
        duration_ms,
        uploader: str_of("uploader").or_else(|| str_of("channel")),
        filesize,
        is_live,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SINGLE: &str = r#"{
        "id": "dQw4w9WgXcQ",
        "title": "会议纪要 2026",
        "webpage_url": "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
        "duration": 212.5,
        "uploader": "Lunarca Channel",
        "filesize_approx": 8388608,
        "live_status": "not_live"
    }"#;

    #[test]
    fn parses_single_video_with_metadata() {
        let probe = parse_probe(SINGLE, "https://youtu.be/dQw4w9WgXcQ").unwrap();
        assert!(!probe.is_playlist());
        assert_eq!(probe.site, "youtu.be", "站点取自用户输入的地址");
        let entry = &probe.entries[0];
        assert_eq!(entry.title, "会议纪要 2026");
        assert_eq!(entry.duration_ms, Some(212_500), "秒→毫秒");
        assert_eq!(entry.uploader.as_deref(), Some("Lunarca Channel"));
        assert_eq!(entry.filesize, Some(8_388_608));
        assert!(!entry.is_live);
        assert_eq!(
            entry.url, "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            "抓取用 yt-dlp 归一化后的规范地址"
        );
    }

    /// 直播与待首播必须被识别——它们没有终点，下载会一直写到磁盘满。
    #[test]
    fn detects_live_and_upcoming() {
        for status in ["is_live", "is_upcoming", "post_live"] {
            let json = format!(
                r#"{{"id":"x","title":"t","webpage_url":"https://a.com/x","live_status":"{status}"}}"#
            );
            let probe = parse_probe(&json, "https://a.com/x").unwrap();
            assert!(probe.entries[0].is_live, "{status} 应判为直播");
        }
        let flag = r#"{"id":"x","title":"t","webpage_url":"https://a.com/x","is_live":true}"#;
        assert!(parse_probe(flag, "https://a.com/x").unwrap().entries[0].is_live);
    }

    #[test]
    fn parses_playlist_and_reports_truncation_honestly() {
        let entries: Vec<String> = (1..=PLAYLIST_LIMIT + 3)
            .map(|i| {
                format!(
                    r#"{{"id":"v{i}","title":"第 {i} 讲","url":"https://a.com/v{i}","duration":60}}"#
                )
            })
            .collect();
        let json = format!(
            r#"{{"_type":"playlist","title":"课程","entries":[{}]}}"#,
            entries.join(",")
        );
        let probe = parse_probe(&json, "https://a.com/list").unwrap();
        assert!(probe.is_playlist());
        assert_eq!(probe.playlist_title.as_deref(), Some("课程"));
        assert_eq!(probe.entries.len(), PLAYLIST_LIMIT, "按上限截断");
        assert_eq!(probe.truncated, 3, "未列出的条数必须如实报出");
        assert_eq!(probe.entries[0].title, "第 1 讲");
    }

    /// 单条目的播放列表（YouTube 的 `?list=` 单集）不该被当成"多选清单"以外的特殊情况——
    /// 有 playlist 标题即按列表呈现，让用户看清自己抓的是这一条还是整个列表。
    #[test]
    fn playlist_with_one_entry_is_still_a_playlist() {
        let json = r#"{"_type":"playlist","title":"合辑","entries":[
            {"id":"a","title":"唯一一集","url":"https://a.com/a"}
        ]}"#;
        let probe = parse_probe(json, "https://a.com/list").unwrap();
        assert!(probe.is_playlist());
        assert_eq!(probe.entries.len(), 1);
    }

    /// 嵌套播放列表条目没有可抓地址，必须过滤掉而不是产出一件注定失败的任务。
    #[test]
    fn nested_playlist_entries_are_dropped() {
        let json = r#"{"_type":"playlist","title":"频道","entries":[
            {"_type":"playlist","title":"子列表","id":"sub"},
            {"id":"a","title":"真视频","url":"https://a.com/a"}
        ]}"#;
        let probe = parse_probe(json, "https://a.com/c").unwrap();
        assert_eq!(probe.entries.len(), 1);
        assert_eq!(probe.entries[0].id, "a");
    }

    /// 标题缺失时回落 id——绝不产出无名笔记。
    #[test]
    fn missing_title_falls_back_to_id() {
        let json = r#"{"id":"BV1xx","webpage_url":"https://b.com/BV1xx"}"#;
        let probe = parse_probe(json, "https://b.com/BV1xx").unwrap();
        assert_eq!(probe.entries[0].title, "BV1xx");
    }

    /// 时长为 0 / 负 / 非数（直播常见）时按"未知"处理，不做出 0 秒这种谎报。
    #[test]
    fn non_positive_duration_is_unknown() {
        for d in ["0", "-1", "null", "\"abc\""] {
            let json = format!(
                r#"{{"id":"x","title":"t","webpage_url":"https://a.com/x","duration":{d}}}"#
            );
            assert_eq!(
                parse_probe(&json, "https://a.com/x").unwrap().entries[0].duration_ms,
                None
            );
        }
    }

    #[test]
    fn rejects_non_json_and_empty_playlists() {
        assert_eq!(
            parse_probe("not json", "https://a.com/x"),
            Err(FetchError::BadProbeOutput)
        );
        assert_eq!(
            parse_probe(r#"{"_type":"playlist","entries":[]}"#, "https://a.com/x"),
            Err(FetchError::BadProbeOutput)
        );
    }
}
