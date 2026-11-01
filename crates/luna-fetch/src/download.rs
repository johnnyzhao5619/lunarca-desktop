//! 抓取执行：子进程 + 确定性进度 + 真取消（TDD-03 §11.3）。
//!
//! 三条硬要求，每条都对应一个"不这样做就会骗用户"的场景：
//! ① **进度必须是真字节数**——`--progress-template` 让 yt-dlp 逐行吐机器可读进度，
//!    而不是靠解析它给人看的进度条（那串 ANSI 回车覆盖的文本不是契约）；
//! ② **落点必须由工具自报**——`--print after_move:filepath`。我们能定文件名 stem，但扩展名
//!    取决于站点最终给的容器；靠"扫目录猜哪个是产物"会在断点续传残留 `.part` 时挑错文件；
//! ③ **取消必须 kill 子进程**——只丢弃 future 的话 yt-dlp 会把 2GB 视频下完，那是假承诺
//!    （与 PRD-05-010「取消是真 abort」同一条纪律）。

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt as _, BufReader};

use crate::error::{classify_stderr, FetchError};
use crate::format::{format_selector, FetchMediaKind};
use crate::progress::{parse_progress_line, FetchProgress, PROGRESS_PREFIX, PROGRESS_TEMPLATE};
use crate::tool::FetchTool;
use crate::url;

/// 取消轮询间隔：kill 的响应上限。1s 足够"点了取消就停"的手感，又不忙等。
const CANCEL_POLL: Duration = Duration::from_secs(1);

/// 一次抓取请求。
#[derive(Clone, Debug)]
pub struct FetchRequest {
    /// 目标地址（已过 [`url::normalize`] 或来自探测结果）。
    pub url: String,
    /// 抓音轨还是连画面（随任务走，不读全局开关）。
    pub media: FetchMediaKind,
    /// 落地目录（调用方给的**任务专属临时目录**；成败一律由调用方清理）。
    pub dir: PathBuf,
    /// 文件名 stem（不含扩展名；由调用方按笔记命名规则净化后给出）。
    pub stem: String,
}

/// 抓取结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchOutcome {
    /// 落地的媒体文件绝对路径。
    pub path: PathBuf,
}

/// 执行抓取。`on_progress` 每收到一行进度即调用；`is_cancelled` 被轮询用于 kill 子进程。
///
/// # Errors
/// URL 非法 / 工具失败（stderr 分类）/ 无产出 / 被取消时返回对应 [`FetchError`]。
pub async fn download(
    tool: &FetchTool,
    request: &FetchRequest,
    on_progress: &(dyn Fn(FetchProgress) + Sync),
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<FetchOutcome, FetchError> {
    let url = url::normalize(&request.url)?;
    std::fs::create_dir_all(&request.dir)?;

    let mut child = tokio::process::Command::new(&tool.path)
        .args(download_args(tool, request, &url))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // 进程随句柄一同回收：应用退出/任务 panic 都不留孤儿下载进程。
        .kill_on_drop(true)
        .spawn()
        .map_err(|err| match err.kind() {
            std::io::ErrorKind::NotFound => FetchError::ToolMissing,
            std::io::ErrorKind::PermissionDenied => FetchError::ToolUnusable,
            _ => FetchError::Io(err.to_string()),
        })?;

    let stdout = child.stdout.take().ok_or(FetchError::ToolUnusable)?;
    let stderr = child.stderr.take().ok_or(FetchError::ToolUnusable)?;
    // stderr 必须**并发**读走：yt-dlp 的告警会填满管道缓冲区，只在结束后读会死锁。
    let stderr_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        let mut collected = String::new();
        while let Ok(Some(line)) = lines.next_line().await {
            // 只留末尾若干行：stderr 可能极长，而分类只看关键词。
            if collected.len() > 8 * 1024 {
                collected.clear();
            }
            collected.push_str(&line);
            collected.push('\n');
        }
        collected
    });

    let mut filepath: Option<PathBuf> = None;
    // stdout 上除进度与落点之外的话（`--match-filter` 的 skip 说明就在这里，而非 stderr）
    // 也要留作错误分类的依据——否则"直播被过滤器挡下"只会表现为一句无信息的"没有产出"。
    let mut stdout_notes = String::new();
    let mut lines = BufReader::new(stdout).lines();
    loop {
        // 与取消轮询竞速：既不阻塞在读行上错过取消，也不忙等。
        let next = tokio::time::timeout(CANCEL_POLL, lines.next_line()).await;
        if is_cancelled() {
            let _ = child.kill().await;
            stderr_task.abort();
            return Err(FetchError::Cancelled);
        }
        match next {
            // 超时 = 这一秒没有新行（下载中很正常），继续轮询。
            Err(_) => continue,
            Ok(Err(err)) => {
                let _ = child.kill().await;
                stderr_task.abort();
                return Err(FetchError::Io(err.to_string()));
            }
            Ok(Ok(None)) => break,
            Ok(Ok(Some(line))) => {
                let line = line.trim();
                if let Some(progress) = parse_progress_line(line) {
                    on_progress(progress);
                } else if line.is_empty() || line.starts_with(PROGRESS_PREFIX) {
                    // 空行 / 残缺的进度行：忽略。
                } else if is_absolute_path(line) {
                    // `--print after_move:filepath` 的落点。**取最后一个**：需要合并的视频
                    // 会先打印各分离流、最后才是合并产物。
                    // 判"绝对路径形状"而不是"非进度行"——yt-dlp 也会往 stdout 写
                    // 「…does not pass filter…」这类说明，把它当路径会让真正的落点被覆盖，
                    // 最终以一句无信息的"没有产出"收场。
                    filepath = Some(PathBuf::from(line));
                } else if stdout_notes.len() < 4 * 1024 {
                    stdout_notes.push_str(line);
                    stdout_notes.push('\n');
                }
            }
        }
    }

    let status = child
        .wait()
        .await
        .map_err(|e| FetchError::Io(e.to_string()))?;
    let stderr = stderr_task.await.unwrap_or_default();
    if is_cancelled() {
        return Err(FetchError::Cancelled);
    }
    // 分类时把 stdout 的说明与 stderr 合看：过滤器拦下的直播只在 stdout 说话。
    let diagnostics = format!("{stdout_notes}{stderr}");
    if !status.success() {
        return Err(classify_stderr(&diagnostics, status.code()));
    }
    // 工具报成功却没有可用文件：可能是过滤器把它挡下了（直播），也可能确实没产出——
    // 先按诊断文本分类，只有认不出来时才落到"没有产出"。
    let path = filepath
        .filter(|p| p.is_file())
        .or_else(|| newest_media_in(&request.dir));
    match path {
        Some(path) => Ok(FetchOutcome { path }),
        None => Err(match classify_stderr(&diagnostics, status.code()) {
            // 诊断文本认不出具体原因（成功退出 + 无产出）→ 诚实报"没有产出"。
            FetchError::ToolFailed { .. } => FetchError::NoOutput,
            classified => classified,
        }),
    }
}

/// 是否长得像绝对路径（Unix `/…` / Windows `C:\…` 或 UNC `\\…`）。
fn is_absolute_path(line: &str) -> bool {
    if line.starts_with('/') || line.starts_with("\\\\") {
        return true;
    }
    let bytes = line.as_bytes();
    bytes.len() > 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
}

/// 组装下载参数（**纯函数**，可单测——参数表是这条链路最容易悄悄写错的地方）。
#[must_use]
pub fn download_args(tool: &FetchTool, request: &FetchRequest, url: &str) -> Vec<String> {
    let mut args = tool.base_args();
    args.extend([
        // 逐行、机器可读的进度（见 progress.rs 的模板契约）。
        "--newline".to_owned(),
        "--progress".to_owned(),
        "--progress-template".to_owned(),
        PROGRESS_TEMPLATE.to_owned(),
        // 落点自报：合并/后处理都结束、文件搬到最终位置之后的那个路径。
        "--print".to_owned(),
        "after_move:filepath".to_owned(),
        // `--print` 默认隐含 --simulate；`after_move` 属后段阶段故不隐含，但显式写上更稳
        // （yt-dlp 各版本对"哪些 WHEN 算后段"的判定曾有出入，而"只模拟不下载"是灾难性静默失败）。
        "--no-simulate".to_owned(),
        // 单条媒体：即便 URL 带 `?list=`，也只抓这一条——播放列表由探测清单显式展开成多件任务，
        // 绝不让一条链接静默变成 200 件下载。
        "--no-playlist".to_owned(),
        // 不写站点字幕/缩略图/描述：Lunarca 的产物是自己转写的时间轴文稿（见 §11.5 裁决）。
        "--no-write-subs".to_owned(),
        "--no-write-auto-subs".to_owned(),
        "--no-write-thumbnail".to_owned(),
        "--no-write-info-json".to_owned(),
        // 直播的**后端硬护栏**：探测阶段已把直播条目标出并在界面上禁选，但"探测完才开播"
        // 是真实竞态（首播倒计时结束）。过滤器让 yt-dlp 自己拒绝，不依赖前端传对标记；
        // 被拦下的情形经 stdout 说明分类回 `FETCH_LIVE_STREAM`，而不是含糊的"没有产出"。
        "--match-filter".to_owned(),
        "!is_live".to_owned(),
        "--no-live-from-start".to_owned(),
        "-f".to_owned(),
        format_selector(request.media, tool.ffmpeg.is_some()),
        "-P".to_owned(),
        request.dir.to_string_lossy().into_owned(),
        "-o".to_owned(),
        format!("{}.%(ext)s", request.stem),
        "--".to_owned(),
        url.to_owned(),
    ]);
    args
}

/// 兜底：目录里最新的媒体文件（`--print` 未生效时用）。
///
/// 排除 `.part`/`.ytdl` 等中间文件——挑到它们会让下游解码一个残缺文件。
fn newest_media_in(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if ext.is_empty() || matches!(ext.as_str(), "part" | "ytdl" | "temp") {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        if best
            .as_ref()
            .is_none_or(|(best_at, _)| modified >= *best_at)
        {
            best = Some((modified, path));
        }
    }
    best.map(|(_, path)| path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(ffmpeg: bool) -> FetchTool {
        FetchTool {
            path: PathBuf::from("/x/yt-dlp"),
            ffmpeg: ffmpeg.then(|| PathBuf::from("/x/ffmpeg")),
            proxy: None,
            js_runtime: None,
        }
    }

    fn request() -> FetchRequest {
        FetchRequest {
            url: "https://a.com/v".into(),
            media: FetchMediaKind::Audio,
            dir: PathBuf::from("/tmp/luna-fetch/1"),
            stem: "会议纪要".into(),
        }
    }

    /// 参数表的四条硬要求：机器可读进度 / 自报落点 / 单条不展开列表 / URL 在 `--` 之后。
    #[test]
    fn download_args_cover_the_four_hard_requirements() {
        let args = download_args(&tool(false), &request(), "https://a.com/v");
        let joined = args.join(" ");
        assert!(joined.contains("--progress-template"), "{joined}");
        assert!(joined.contains(PROGRESS_TEMPLATE), "{joined}");
        assert!(joined.contains("after_move:filepath"), "{joined}");
        assert!(joined.contains("--no-simulate"), "缺它可能只模拟不下载");
        assert!(
            joined.contains("--no-playlist"),
            "带 ?list= 的链接不得静默展开成整列表"
        );
        let dash = args.iter().position(|a| a == "--").expect("含 -- 分隔");
        assert_eq!(args[dash + 1], "https://a.com/v");
        assert_eq!(dash + 2, args.len(), "URL 必须是最后一个参数");
    }

    /// 输出模板只定 stem，扩展名交给站点（`%(ext)s`）——我们不知道最终容器是什么。
    #[test]
    fn output_template_fixes_stem_and_delegates_extension() {
        let args = download_args(&tool(true), &request(), "https://a.com/v");
        let idx = args.iter().position(|a| a == "-o").expect("含 -o");
        assert_eq!(args[idx + 1], "会议纪要.%(ext)s");
        let p = args.iter().position(|a| a == "-P").expect("含 -P");
        assert_eq!(args[p + 1], "/tmp/luna-fetch/1");
    }

    /// 站点自带字幕/缩略图/描述一律不落盘（产物只有我们自己转写的文稿 + 媒体）。
    #[test]
    fn download_args_refuse_sidecar_artifacts() {
        let args = download_args(&tool(true), &request(), "https://a.com/v");
        for flag in [
            "--no-write-subs",
            "--no-write-auto-subs",
            "--no-write-thumbnail",
            "--no-write-info-json",
        ] {
            assert!(args.iter().any(|a| a == flag), "缺 {flag}");
        }
    }

    /// 格式串随 ffmpeg 在场变化（能力分档），且恒经 `-f` 传入。
    #[test]
    fn format_selector_is_wired_and_tiered() {
        let with = download_args(&tool(true), &request(), "https://a.com/v");
        let video_req = FetchRequest {
            media: FetchMediaKind::Video,
            ..request()
        };
        let video_with = download_args(&tool(true), &video_req, "https://a.com/v");
        let video_without = download_args(&tool(false), &video_req, "https://a.com/v");
        let f = |args: &[String]| {
            let i = args.iter().position(|a| a == "-f").expect("含 -f");
            args[i + 1].clone()
        };
        assert!(!f(&with).contains('+'), "音频档不合并");
        assert!(f(&video_with).contains('+'), "有 ffmpeg 的视频档合并");
        assert!(!f(&video_without).contains('+'), "无 ffmpeg 不选合并组合");
    }

    /// 兜底扫目录必须跳过 `.part` 残留——挑到它等于把残缺文件交给解码器。
    #[test]
    fn newest_media_skips_partial_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.m4a"), b"x").expect("write");
        std::fs::write(dir.path().join("b.m4a.part"), b"x").expect("write");
        std::fs::write(dir.path().join("noext"), b"x").expect("write");
        let found = newest_media_in(dir.path()).expect("found");
        assert_eq!(found.extension().and_then(|e| e.to_str()), Some("m4a"));
        assert!(!found.to_string_lossy().contains("part"));
    }

    #[test]
    fn newest_media_returns_none_for_empty_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(newest_media_in(dir.path()).is_none());
    }

    /// 直播的后端硬护栏必须在参数里（前端标记只管禁选，挡不住"探测完才开播"）。
    #[test]
    fn download_args_carry_the_live_match_filter() {
        let args = download_args(&tool(true), &request(), "https://a.com/v");
        let idx = args
            .iter()
            .position(|a| a == "--match-filter")
            .expect("含 --match-filter");
        assert_eq!(args[idx + 1], "!is_live");
    }

    /// 落点识别必须只认绝对路径形状——yt-dlp 也往 stdout 写说明文字，
    /// 把它当路径会覆盖真正的落点，最后表现为"没有产出"。
    #[test]
    fn absolute_path_shape_distinguishes_filepath_from_chatter() {
        assert!(is_absolute_path("/tmp/luna/会议.m4a"));
        assert!(is_absolute_path(r"C:\Users\me\a.mp4"));
        assert!(is_absolute_path(r"\\server\share\a.mp4"));
        assert!(!is_absolute_path(
            "[download] skipping ..: does not pass filter (!is_live)"
        ));
        assert!(!is_absolute_path("会议.m4a"));
        assert!(!is_absolute_path(""));
    }
}
