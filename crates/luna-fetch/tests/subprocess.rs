//! 子进程编排的端到端测试（**假 yt-dlp**，不联网、不依赖真实工具）。
//!
//! 为什么必须有这一层：`crates/luna-fetch` 的单测全是纯函数，而
//! [`luna_fetch::download`] / [`luna_fetch::probe`] / [`luna_fetch::tool::status`] 做的恰恰是
//! **最容易错且错了最难发现**的事——管道读取会不会死锁、进度模板与解析器是否真的对得上、
//! 落点行能否从工具的闲聊里分辨出来、取消是不是真的 kill 掉了进程。这些在纯函数测试里
//! 一个都验证不到，而真机验证又要求装 yt-dlp + 联网 + 一个不会下架的视频。
//!
//! 做法：用一个 shell 脚本冒充 yt-dlp，按我们**自己下的参数**产出对应输出。于是这层测的是
//! 「我们的编排」而不是「yt-dlp 的行为」——后者归真机验收（PRD-05 §6.2）。
//!
//! 假工具是 `#!/bin/sh` 脚本，只在 Unix 上可执行；Windows 的编排另需 `.cmd` 版假工具（未做）。
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use luna_fetch::{FetchError, FetchMediaKind, FetchRequest, FetchTool};

/// 注入给「非超时」用例的探测超时：**死锁守卫，不是性能断言**——假工具跑完只需毫秒，
/// 这个值大到只可能因实现真的卡住才触发。
///
/// **绝不继承生产超时**：`probe_classifies_failure_from_stderr` 原先直接吃
/// `probe::PROBE_TIMEOUT`（45 秒，按真实站点提取耗时定的）。单独跑 1.4 秒稳过；整仓
/// `cargo test --workspace` 并行跑时机器被编译 + 十几个测试二进制打满，假工具没在 45 秒内
/// 被调度完，于是 `probe` 走了超时分支返回 `Network`，断言炸在与超时**毫无关系**的
/// `ToolOutdated` 上（2026-08-07）。分类用例要断言的是「stderr 分类对不对」，不是
/// 「这台机器 45 秒内调度得完子进程吗」——故超时改由测试注入：分类用例给足到不可能触发，
/// 超时分支交给专门的用例（短超时 + 长睡假工具）去覆盖，两个方向都与机器负载无关。
const GENEROUS_TIMEOUT: Duration = Duration::from_secs(600);

/// 注入给「超时」用例的探测超时：配长睡的假工具，无论机器多闲都不可能跑赢它。
const INSTANT_TIMEOUT: Duration = Duration::from_millis(100);

/// 写一个可执行的假工具脚本，返回其路径。
fn fake_tool(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod +x");
    }
    path
}

fn tool_at(path: PathBuf) -> FetchTool {
    FetchTool {
        path,
        ffmpeg: None,
        proxy: None,
        js_runtime: None,
    }
}

fn request(dir: &Path, url: &str) -> FetchRequest {
    FetchRequest {
        url: url.to_owned(),
        media: FetchMediaKind::Audio,
        dir: dir.join("work"),
        stem: "会议纪要".into(),
    }
}

/// 快乐路径：逐行进度被解析、落点被识别、产物路径原样交出。
///
/// 假工具刻意在进度行之间夹一句**非路径的闲聊**（yt-dlp 真会这么干），验证它不会被当成落点。
#[tokio::test]
async fn download_parses_progress_and_takes_reported_filepath() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = r#"
# 解析出 -P 指定的落地目录与 -o 模板的 stem
while [ $# -gt 0 ]; do
  case "$1" in
    -P) OUTDIR="$2"; shift 2 ;;
    -o) TEMPLATE="$2"; shift 2 ;;
    *) shift ;;
  esac
done
STEM=$(printf '%s' "$TEMPLATE" | sed 's/\.%(ext)s$//')
mkdir -p "$OUTDIR"
printf 'LUNA_FETCH\t0\t8388608\tNA\n'
printf '[download] Destination: whatever\n'
printf 'LUNA_FETCH\t4194304\t8388608\tNA\n'
printf 'LUNA_FETCH\t8388608\t8388608\tNA\n'
: > "$OUTDIR/$STEM.m4a"
printf '%s\n' "$OUTDIR/$STEM.m4a"
exit 0
"#;
    let tool = tool_at(fake_tool(dir.path(), "yt-dlp", script));
    let req = request(dir.path(), "https://example.com/v");

    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    let outcome = luna_fetch::download(
        &tool,
        &req,
        &move |p| sink.lock().expect("lock").push(p),
        &|| false,
    )
    .await
    .expect("download ok");

    assert_eq!(
        outcome.path,
        req.dir.join("会议纪要.m4a"),
        "落点必须取工具自报的那个（不是扫目录猜的）"
    );
    let progress = seen.lock().expect("lock").clone();
    assert_eq!(progress.len(), 3, "三行进度都要解析出来：{progress:?}");
    assert_eq!(progress[1].downloaded, 4_194_304);
    assert_eq!(progress[2].total, Some(8_388_608));
}

/// stdout 上只有闲聊、没有落点行时回落"扫目录挑最新媒体"，且跳过 `.part` 残留。
#[tokio::test]
async fn download_falls_back_to_scanning_when_no_filepath_printed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = r#"
while [ $# -gt 0 ]; do
  case "$1" in
    -P) OUTDIR="$2"; shift 2 ;;
    *) shift ;;
  esac
done
mkdir -p "$OUTDIR"
: > "$OUTDIR/leftover.m4a.part"
: > "$OUTDIR/actual.m4a"
printf '[download] 100%% of 1.00MiB\n'
exit 0
"#;
    let tool = tool_at(fake_tool(dir.path(), "yt-dlp", script));
    let req = request(dir.path(), "https://example.com/v");
    let outcome = luna_fetch::download(&tool, &req, &|_| {}, &|| false)
        .await
        .expect("download ok");
    assert_eq!(
        outcome.path.file_name().and_then(|s| s.to_str()),
        Some("actual.m4a"),
        "绝不能挑中 .part 残留（那是残缺文件）"
    );
}

/// 工具成功退出却没有产出任何文件 → 诚实报 `NO_OUTPUT`，不让下游对着不存在的路径解码。
#[tokio::test]
async fn download_reports_no_output_when_nothing_produced() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = tool_at(fake_tool(dir.path(), "yt-dlp", "exit 0"));
    let req = request(dir.path(), "https://example.com/v");
    let err = luna_fetch::download(&tool, &req, &|_| {}, &|| false)
        .await
        .expect_err("应失败");
    assert_eq!(err, FetchError::NoOutput);
}

/// **过滤器挡下直播时的诚实归因**：`--match-filter "!is_live"` 的 skip 说明走 **stdout**，
/// 若不把它并入分类依据，这一情形只会表现为无信息的 `NO_OUTPUT`。
#[tokio::test]
async fn download_classifies_match_filter_skip_as_live_stream() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = r#"
printf '[download] skipping ..: does not pass filter (!is_live), test skipping ..\n'
exit 0
"#;
    let tool = tool_at(fake_tool(dir.path(), "yt-dlp", script));
    let req = request(dir.path(), "https://example.com/live");
    let err = luna_fetch::download(&tool, &req, &|_| {}, &|| false)
        .await
        .expect_err("应失败");
    assert_eq!(
        err,
        FetchError::LiveStream,
        "成功退出 + 无产出 + 过滤器说明 → 必须归因为直播，而不是含糊的 NO_OUTPUT"
    );
}

/// 站点侧失败：stderr 被读走（不因管道缓冲区填满而死锁）并分类为登录墙。
///
/// 脚本刻意往 stderr 写**大量**内容——只在进程结束后才读 stderr 的实现会在这里挂死。
///
/// **噪声由一次 `awk` 吐出，而不是 4000 次 shell 循环**：本文件的测试并发跑，每个都在
/// 起子进程；原来的 `while` 循环在本机 0.2s，在 macOS runner 上撞满过超时守卫，把一次与
/// 本测试毫无关系的改动判成红（2026-08-04）。**一个会随机变红的门禁与没有门禁是一回事**，
/// 故改为单进程生成同样体量的字节——守卫要抓的是「实现不读 stderr 就死锁」，不是
/// 「shell 循环跑得快不快」。
///
/// **守卫值也一并从 20s 提到 [`GENEROUS_TIMEOUT`]**：当时留 20s 并断言"它现在只可能因
/// 真死锁而触发"，而 2026-08-07 复现 flake 时把机器压到 loadavg 100+，这条**又**在没有任何
/// 死锁的情况下红了一次。死锁是"永远不结束"，任何有限值都够抓；20s 却同时兼了一句
/// 谁也没要求过的性能断言。守卫只该回答"卡死了吗"，不该回答"这台机器忙不忙"。
#[tokio::test]
async fn download_drains_large_stderr_and_classifies_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = r#"
awk 'BEGIN { for (i = 0; i < 4000; i++)
  printf "WARNING: noise line %d padding padding padding padding padding\n", i }' >&2
printf 'ERROR: [youtube] xx: Private video. Sign in if you have been granted access\n' >&2
exit 1
"#;
    let tool = tool_at(fake_tool(dir.path(), "yt-dlp", script));
    let req = request(dir.path(), "https://example.com/private");
    let err = tokio::time::timeout(
        GENEROUS_TIMEOUT,
        luna_fetch::download(&tool, &req, &|_| {}, &|| false),
    )
    .await
    .expect("绝不能死锁在 stderr 管道上")
    .expect_err("应失败");
    assert_eq!(err, FetchError::LoginRequired);
}

/// **取消是真 kill**：假工具会睡很久，取消后必须立刻返回 `Cancelled` 而不是等它跑完。
#[tokio::test]
async fn cancel_kills_the_child_instead_of_waiting_it_out() {
    let dir = tempfile::tempdir().expect("tempdir");
    // 先吐一行进度让测试确知进程已起，再长睡——若取消只是"丢弃 future"，这里会等满 120s。
    let script = r#"
printf 'LUNA_FETCH\t1024\tNA\tNA\n'
sleep 120
exit 0
"#;
    let tool = tool_at(fake_tool(dir.path(), "yt-dlp", script));
    let req = request(dir.path(), "https://example.com/slow");

    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancelled);
    let started = Arc::new(AtomicBool::new(false));
    let started_probe = Arc::clone(&started);

    let began = std::time::Instant::now();
    // 闭包须先绑定到具名变量：直接写在 `join!` 的实参位置会让临时值在 await 点之前被丢弃。
    let on_progress = move |_| started_probe.store(true, Ordering::SeqCst);
    let is_cancelled = move || flag.load(Ordering::SeqCst);
    let (result, _) = tokio::join!(
        luna_fetch::download(&tool, &req, &on_progress, &is_cancelled),
        async {
            // 等第一行进度到达（确认子进程真的在跑）再请求取消。
            for _ in 0..100 {
                if started.load(Ordering::SeqCst) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            cancelled.store(true, Ordering::SeqCst);
        }
    );
    assert_eq!(result.expect_err("应被取消"), FetchError::Cancelled);
    assert!(
        began.elapsed() < Duration::from_secs(30),
        "取消必须当即返回（实测 {:?}）——只丢弃 future 的话会等满 sleep",
        began.elapsed()
    );
}

/// 工具路径不存在 → `ToolMissing`（而不是一个含糊的 IO 错误）。
#[tokio::test]
async fn download_maps_missing_executable_to_tool_missing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = tool_at(dir.path().join("nope"));
    let req = request(dir.path(), "https://example.com/v");
    assert_eq!(
        luna_fetch::download(&tool, &req, &|_| {}, &|| false)
            .await
            .expect_err("应失败"),
        FetchError::ToolMissing
    );
}

/// 探测：`-J` 的单行 JSON 被解析成清单，且 URL 校验先于子进程执行。
#[tokio::test]
async fn probe_reads_json_and_validates_url_first() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = r#"
printf '{"id":"abc","title":"会议分享","webpage_url":"https://example.com/v","duration":212.5,"uploader":"Lunarca"}\n'
exit 0
"#;
    let tool = tool_at(fake_tool(dir.path(), "yt-dlp", script));

    let probe = luna_fetch::probe_with_timeout(&tool, "https://example.com/v", GENEROUS_TIMEOUT)
        .await
        .expect("probe ok");
    assert_eq!(probe.entries.len(), 1);
    assert_eq!(probe.entries[0].title, "会议分享");
    assert_eq!(probe.entries[0].duration_ms, Some(212_500));
    assert!(!probe.is_playlist());

    // 非 http(s) 在**进程启动之前**就被拒（假工具一旦被执行就会返回上面那份 JSON，
    // 故拿到 UnsupportedUrl 即证明校验发生在前）。
    assert_eq!(
        luna_fetch::probe_with_timeout(&tool, "file:///etc/passwd", GENEROUS_TIMEOUT)
            .await
            .expect_err("应拒绝"),
        FetchError::UnsupportedUrl
    );
}

/// 探测失败按 stderr 分类（此处：版本过旧优先于"提取失败"）。
///
/// 超时由测试注入且给足余量（见 [`GENEROUS_TIMEOUT`]）——这条断言只该受 stderr 内容左右。
#[tokio::test]
async fn probe_classifies_failure_from_stderr() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = r#"
printf 'ERROR: Unable to extract nsig function; please update yt-dlp\n' >&2
exit 1
"#;
    let tool = tool_at(fake_tool(dir.path(), "yt-dlp", script));
    assert_eq!(
        luna_fetch::probe_with_timeout(&tool, "https://example.com/v", GENEROUS_TIMEOUT)
            .await
            .expect_err("应失败"),
        FetchError::ToolOutdated
    );
}

/// 探测超时归到 `Network`（**可重试**）而不是含糊的工具失败：站点提取本就慢，超时说明
/// "这次没成"而非"工具坏了"，界面该给「重试」而不是「重装」。
///
/// 假工具长睡 120 秒、注入超时 100 毫秒 → 无论机器多闲都必然走超时分支（与
/// [`probe_classifies_failure_from_stderr`] 恰好相反的方向，两者都不看机器负载）。
#[tokio::test]
async fn probe_timeout_is_reported_as_retryable_network_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tool = tool_at(fake_tool(dir.path(), "yt-dlp", "sleep 120\nexit 0"));
    let began = std::time::Instant::now();
    let err = luna_fetch::probe_with_timeout(&tool, "https://example.com/v", INSTANT_TIMEOUT)
        .await
        .expect_err("应超时");
    assert_eq!(err, FetchError::Network);
    assert!(err.is_retryable(), "超时该给「重试」，不是「重装」");
    assert!(
        began.elapsed() < Duration::from_secs(60),
        "超时必须当即返回（实测 {:?}）——等满子进程的 sleep 就说明超时根本没生效",
        began.elapsed()
    );
}

/// 工具状态探测：`--version` 成功 → 就绪；退出码非 0 → 找到了但跑不起来（两种处境不同）。
#[tokio::test]
async fn status_distinguishes_usable_from_broken_tool() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ok = fake_tool(dir.path(), "yt-dlp-ok", "printf '2026.07.20\\n'; exit 0");
    let status =
        luna_fetch::tool::status_with_timeout(Some(&ok), None, None, GENEROUS_TIMEOUT).await;
    assert!(status.ready());
    assert_eq!(status.version.as_deref(), Some("2026.07.20"));

    let broken = fake_tool(dir.path(), "yt-dlp-broken", "exit 127");
    let status =
        luna_fetch::tool::status_with_timeout(Some(&broken), None, None, GENEROUS_TIMEOUT).await;
    assert_eq!(status.tool_path.as_deref(), Some(broken.as_path()));
    assert!(status.version.is_none(), "跑不起来 → 无版本");
    assert!(!status.ready());

    // 路径根本不存在 → 连 tool_path 都没有（= 没装，与"装了但坏"分立）。
    let absent = luna_fetch::tool::status_with_timeout(
        Some(&dir.path().join("nope")),
        None,
        None,
        GENEROUS_TIMEOUT,
    )
    .await;
    assert!(absent.tool_path.is_none());
}

/// `FetchTool::resolve` **只定位不探版本**：探一次要 ~8 秒（PyInstaller onefile 每次启动
/// 都解压运行时），而每条抓取任务都先 resolve——那是纯粹的白等。文件在就交出去用，
/// 真坏了由下载的 stderr 分类给出准确原因（本文件另有覆盖）。
#[tokio::test]
async fn resolve_locates_without_paying_the_version_probe() {
    let dir = tempfile::tempdir().expect("tempdir");
    // 一个"跑不起来"的工具照样能被 resolve 交出——判定推迟到真正使用时。
    let broken = fake_tool(dir.path(), "yt-dlp-broken", "exit 1");
    let tool = FetchTool::resolve(Some(&broken), None, None, None).expect("定位成功");
    assert_eq!(tool.path, broken);
    // 文件不在才是 resolve 该拦的。
    assert_eq!(
        FetchTool::resolve(Some(&dir.path().join("nope")), None, None, None).expect_err("应失败"),
        FetchError::ToolMissing
    );
}

/// 探测把「超时」与「坏了」分开——真机上一份**完全可用**的 yt-dlp 曾因 3 秒超时被恒定
/// 判成"无法运行"，界面于是给出"重装"这个毫无帮助的指引。
///
/// 三种处境各验一次，且**都不看机器负载**：前两种给足超时（只可能因真卡住才触发），
/// 第三种给极短超时配长睡的假工具（只可能超时）。原先只验了 `probe_timed_out == false`
/// 的两条，`true` 那条——也就是这个字段存在的全部理由——从未被任何测试观察到。
#[tokio::test]
async fn status_reports_timeout_separately_from_breakage() {
    let dir = tempfile::tempdir().expect("tempdir");
    let broken = fake_tool(dir.path(), "yt-dlp-broken", "exit 127");
    let status =
        luna_fetch::tool::status_with_timeout(Some(&broken), None, None, GENEROUS_TIMEOUT).await;
    assert!(status.version.is_none());
    assert!(!status.probe_timed_out, "非零退出是坏了，不是超时");

    let ok = fake_tool(dir.path(), "yt-dlp-ok", "printf '2026.07.04\n'");
    let status =
        luna_fetch::tool::status_with_timeout(Some(&ok), None, None, GENEROUS_TIMEOUT).await;
    assert_eq!(status.version.as_deref(), Some("2026.07.04"));
    assert!(!status.probe_timed_out);

    // 慢到超时：同样是"没有版本"，但成因不同——界面据此给「重试」而不是「重装」。
    let slow = fake_tool(
        dir.path(),
        "yt-dlp-slow",
        "sleep 120\nprintf '2026.07.04\n'",
    );
    let began = std::time::Instant::now();
    let status =
        luna_fetch::tool::status_with_timeout(Some(&slow), None, None, INSTANT_TIMEOUT).await;
    assert!(status.version.is_none());
    assert!(status.probe_timed_out, "慢 ≠ 坏，必须如实标为超时");
    assert!(!status.ready(), "超时期间仍不可开抓（但引导词不同）");
    // 守卫（余量 600×：注入 100ms，负载下实测最慢 0.5s）——等满假工具那 120 秒长睡就说明
    // 注入的超时压根没生效，那是实现缺陷，须当场报出来而不是被一次「慢但绿」的跑盖过去。
    assert!(
        began.elapsed() < Duration::from_secs(60),
        "超时必须当即返回（实测 {:?}）",
        began.elapsed()
    );
}
