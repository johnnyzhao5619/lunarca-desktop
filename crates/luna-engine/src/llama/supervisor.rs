//! 路由器进程的监管（ADR-143 ③⑤）。
//!
//! 只做进程层面的事：写 preset、拉起、等就绪、带超时的健康探针、退出后按
//! [`RestartPolicy`] 退避重启或熔断、优雅退出。决策逻辑全在 [`super::policy`]（可测），
//! 这里是薄薄一层 I/O。
//!
//! 几条从 P0 带来的硬规矩：
//! - 只监听 127.0.0.1、`--port 0`；密钥**只经环境变量** `LLAMA_API_KEY`——经命令行 /
//!   preset 传的不下发子实例，子端口会无鉴权暴露（P0-2）。
//! - 退出走 SIGTERM，3 s 后才 SIGKILL：路由器被强杀时子实例会 abort 并在用户的诊断
//!   报告里留一份崩溃（P0-7）。
//! - 日志原文不出本模块（可能含请求片段，TDD-12 日志脱敏）：只按类别上报。
//! - 上次运行崩溃留下的路由器，按 pid 文件找回并结束——否则它会一直占着内存。

use std::io::BufRead as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, watch};

use super::policy::{
    classify_line, parse_listening, HealthTracker, RestartDecision, RestartPolicy, Sentinel,
};

/// 拉起一次路由器需要的全部输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchSpec {
    /// 随包 `llama-server` 的绝对路径。
    pub server_bin: PathBuf,
    /// 已渲染的 preset（[`super::preset::render_preset`]）。
    pub preset_ini: String,
    /// preset 落盘位置（应用数据目录内）。
    pub preset_path: PathBuf,
    /// pid 文件位置（找回上次崩溃留下的路由器）。
    pub pid_path: PathBuf,
    /// 同时常驻的模型数（路由器参数，纯 LRU）。
    pub models_max: u32,
}

/// 对外可见的状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineState {
    /// 没在跑（未启动或已停止）。
    Stopped,
    /// 正在拉起 / 等就绪。
    Starting,
    /// 就绪。
    Ready {
        /// 实际监听端口。
        port: u16,
    },
    /// 意外退出，等待重启。
    Restarting {
        /// 已连续失败次数。
        failures: u32,
        /// 多久后重启（毫秒）。
        delay_ms: u64,
    },
    /// 熔断：不再自动重启，等用户手动重启。
    Tripped,
}

/// 状态快照：状态 + 最近一次故障的类别（不含原始日志）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineStatus {
    /// 当前状态。
    pub state: EngineState,
    /// 最近一次故障类别。
    pub last_failure: Option<Failure>,
}

/// 故障类别。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// 拉不起来（二进制缺失 / 权限 / preset 写不进去）。
    Spawn(String),
    /// 规定时间内没有监听或健康探针不通。
    StartTimeout,
    /// 进程退出（退出码；被信号杀时为 `None`）。
    Exited(Option<i32>),
    /// 健康探针连续不通（进程挂起）。
    Unresponsive,
    /// 日志哨兵。
    Sentinel(Sentinel),
}

const LISTEN_TIMEOUT: Duration = Duration::from_secs(30);
const READY_TIMEOUT: Duration = Duration::from_secs(60);
const HEALTH_INTERVAL: Duration = Duration::from_secs(5);
const HEALTH_TIMEOUT: Duration = Duration::from_secs(2);
const EXIT_POLL: Duration = Duration::from_millis(250);
const TERM_GRACE: Duration = Duration::from_secs(3);

/// 路由器监管器。克隆廉价（内部 `Arc`）。
#[derive(Clone)]
pub struct LlamaServerSupervisor {
    shared: Arc<Shared>,
}

struct Shared {
    api_key: String,
    client: reqwest::Client,
    status: watch::Sender<EngineStatus>,
    run: Mutex<Option<RunHandle>>,
}

struct RunHandle {
    stop: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
    spec: LaunchSpec,
}

impl Default for LlamaServerSupervisor {
    fn default() -> Self {
        Self::new()
    }
}

impl LlamaServerSupervisor {
    /// 新监管器，生成本次运行的随机密钥（256 bit，系统 CSPRNG）。
    #[must_use]
    pub fn new() -> Self {
        let api_key = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let (status, _) = watch::channel(EngineStatus {
            state: EngineState::Stopped,
            last_failure: None,
        });
        Self {
            shared: Arc::new(Shared {
                api_key,
                client: reqwest::Client::new(),
                status,
                run: Mutex::new(None),
            }),
        }
    }

    /// 当前状态。
    #[must_use]
    pub fn status(&self) -> EngineStatus {
        self.shared.status.borrow().clone()
    }

    /// 订阅状态变化。
    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<EngineStatus> {
        self.shared.status.subscribe()
    }

    /// 就绪时的 `(base_url, api_key)`，供 Provider 组请求。
    #[must_use]
    pub fn endpoint(&self) -> Option<(String, String)> {
        match self.shared.status.borrow().state {
            EngineState::Ready { port } => Some((
                format!("http://127.0.0.1:{port}"),
                self.shared.api_key.clone(),
            )),
            _ => None,
        }
    }

    /// 当前（或上一次）启动用的参数。编排层据此判断「已装模型没变」时不必重启——
    /// 重启会把已加载的模型全部丢掉，下一次请求重付冷启动。
    #[must_use]
    pub fn current_spec(&self) -> Option<LaunchSpec> {
        self.shared
            .run
            .lock()
            .ok()
            .and_then(|run| run.as_ref().map(|r| r.spec.clone()))
    }

    /// 按 `spec` 启动（已在跑则先停）。立即返回；用 [`Self::subscribe`] 等就绪。
    pub async fn start(&self, spec: LaunchSpec) {
        self.stop().await;
        let (pid_path, server_bin) = (spec.pid_path.clone(), spec.server_bin.clone());
        let _ = tokio::task::spawn_blocking(move || reap_stale(&pid_path, &server_bin)).await;
        let (stop_tx, stop_rx) = watch::channel(false);
        let shared = Arc::clone(&self.shared);
        let task_spec = spec.clone();
        let task = tokio::spawn(async move { supervise(shared, task_spec, stop_rx).await });
        if let Ok(mut run) = self.shared.run.lock() {
            *run = Some(RunHandle {
                stop: stop_tx,
                task,
                spec,
            });
        }
    }

    /// 用上次的参数手动重启（熔断后界面上的「重启」）：重启次数清零。
    pub async fn restart(&self) {
        let spec = self
            .shared
            .run
            .lock()
            .ok()
            .and_then(|run| run.as_ref().map(|r| r.spec.clone()));
        if let Some(spec) = spec {
            self.start(spec).await;
        }
    }

    /// 停止并等进程退出（SIGTERM → 3 s → SIGKILL）。没在跑时什么也不做。
    pub async fn stop(&self) {
        let handle = self.shared.run.lock().ok().and_then(|mut run| run.take());
        if let Some(handle) = handle {
            let _ = handle.stop.send(true);
            let _ = handle.task.await;
        }
    }
}

/// 一次运行中的子进程与它的日志事件。
struct Running {
    child: Arc<Mutex<Child>>,
    events: mpsc::UnboundedReceiver<LineEvent>,
    port: u16,
}

enum LineEvent {
    Port(u16),
    Sentinel(Sentinel),
}

async fn supervise(shared: Arc<Shared>, spec: LaunchSpec, mut stop: watch::Receiver<bool>) {
    let mut policy = RestartPolicy::new();
    loop {
        set_state(&shared, EngineState::Starting, None);
        let failure = match launch(&shared, &spec, &mut stop).await {
            Launch::Stopped => {
                set_state(&shared, EngineState::Stopped, None);
                return;
            }
            Launch::Failed(failure) => failure,
            Launch::Ready(mut running) => {
                policy.on_ready(Instant::now());
                set_state(&shared, EngineState::Ready { port: running.port }, None);
                let outcome = watch_running(&shared, &mut running, &mut stop).await;
                terminate(&running.child).await;
                let _ = std::fs::remove_file(&spec.pid_path);
                match outcome {
                    None => {
                        set_state(&shared, EngineState::Stopped, None);
                        return;
                    }
                    Some(failure) => failure,
                }
            }
        };
        // 参数被拒、文件损坏：重启也一样失败，直接熔断，别在用户机器上空转五次。
        let decision = if matches!(
            failure,
            Failure::Sentinel(Sentinel::BadArguments | Sentinel::CorruptModel)
        ) {
            RestartDecision::Trip
        } else {
            policy.on_exit(Instant::now())
        };
        match decision {
            RestartDecision::RetryAfter(delay) => {
                set_state(
                    &shared,
                    EngineState::Restarting {
                        failures: policy.failures(),
                        delay_ms: u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
                    },
                    Some(failure),
                );
                tokio::select! {
                    () = tokio::time::sleep(delay) => {}
                    _ = stop.changed() => {
                        set_state(&shared, EngineState::Stopped, None);
                        return;
                    }
                }
            }
            RestartDecision::Trip => {
                set_state(&shared, EngineState::Tripped, Some(failure));
                let _ = stop.changed().await;
                set_state(&shared, EngineState::Stopped, None);
                return;
            }
        }
    }
}

enum Launch {
    Ready(Running),
    Failed(Failure),
    Stopped,
}

async fn launch(shared: &Shared, spec: &LaunchSpec, stop: &mut watch::Receiver<bool>) -> Launch {
    if let Err(e) = write_private(&spec.preset_path, spec.preset_ini.as_bytes()) {
        return Launch::Failed(Failure::Spawn(format!("write preset: {}", e.kind())));
    }
    let mut cmd = Command::new(&spec.server_bin);
    cmd.arg("--models-preset")
        .arg(&spec.preset_path)
        .args(["--host", "127.0.0.1", "--port", "0", "--no-webui"])
        .args(["--models-max", &spec.models_max.to_string()])
        .env("LLAMA_API_KEY", &shared.api_key)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => return Launch::Failed(Failure::Spawn(e.kind().to_string())),
    };
    let _ = std::fs::write(&spec.pid_path, child.id().to_string());
    let (tx, mut events) = mpsc::unbounded_channel();
    for pipe in [
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
    ]
    .into_iter()
    .flatten()
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            // 逐行只做分类，原文就地丢弃。
            for line in std::io::BufReader::new(pipe).lines().map_while(Result::ok) {
                let event = parse_listening(&line)
                    .map(LineEvent::Port)
                    .or_else(|| classify_line(&line).map(LineEvent::Sentinel));
                if let Some(event) = event {
                    if tx.send(event).is_err() {
                        break;
                    }
                }
            }
        });
    }
    drop(tx);
    let child = Arc::new(Mutex::new(child));

    // ① 等监听行
    let deadline = Instant::now() + LISTEN_TIMEOUT;
    let port = loop {
        if let Some(code) = exited(&child) {
            return Launch::Failed(
                drain_sentinel(&mut events).map_or(Failure::Exited(code), Failure::Sentinel),
            );
        }
        if Instant::now() >= deadline {
            terminate(&child).await;
            return Launch::Failed(Failure::StartTimeout);
        }
        tokio::select! {
            event = events.recv() => match event {
                Some(LineEvent::Port(port)) => break port,
                Some(LineEvent::Sentinel(s @ (Sentinel::BadArguments | Sentinel::CorruptModel))) => {
                    terminate(&child).await;
                    return Launch::Failed(Failure::Sentinel(s));
                }
                Some(LineEvent::Sentinel(_)) => {}
                None => tokio::time::sleep(EXIT_POLL).await,
            },
            () = tokio::time::sleep(EXIT_POLL) => {}
            _ = stop.changed() => {
                terminate(&child).await;
                return Launch::Stopped;
            }
        }
    };

    // ② 等健康探针
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        if probe(shared, port).await {
            return Launch::Ready(Running {
                child,
                events,
                port,
            });
        }
        if let Some(code) = exited(&child) {
            return Launch::Failed(Failure::Exited(code));
        }
        if Instant::now() >= deadline {
            terminate(&child).await;
            return Launch::Failed(Failure::StartTimeout);
        }
        tokio::select! {
            () = tokio::time::sleep(Duration::from_millis(100)) => {}
            _ = stop.changed() => {
                terminate(&child).await;
                return Launch::Stopped;
            }
        }
    }
}

/// 运行期：返回 `None` = 被要求停止；`Some` = 判死（调用方负责收尸与重启决策）。
async fn watch_running(
    shared: &Shared,
    running: &mut Running,
    stop: &mut watch::Receiver<bool>,
) -> Option<Failure> {
    let mut health = HealthTracker::default();
    let mut tick = tokio::time::interval(HEALTH_INTERVAL);
    tick.tick().await;
    let mut pipes_open = true;
    loop {
        // 退出检查放在每轮开头，而不是作为 select 的一个分支：管道关闭后 `recv()` 每轮都
        // 立即返回，select 总选中它，计时分支永远赢不了——实测强杀路由器后要等健康探针
        // 连败三次（15 s）才发现。
        if let Some(code) = exited(&running.child) {
            return Some(Failure::Exited(code));
        }
        tokio::select! {
            _ = stop.changed() => return None,
            _ = tick.tick() => {
                if health.record(probe(shared, running.port).await) {
                    return Some(Failure::Unresponsive);
                }
            }
            event = running.events.recv(), if pipes_open => match event {
                Some(LineEvent::Sentinel(Sentinel::ComputeError)) => {
                    return Some(Failure::Sentinel(Sentinel::ComputeError));
                }
                Some(_) => {}
                None => pipes_open = false,
            },
            () = tokio::time::sleep(EXIT_POLL) => {}
        }
    }
}

async fn probe(shared: &Shared, port: u16) -> bool {
    matches!(
        shared
            .client
            .get(format!("http://127.0.0.1:{port}/health"))
            .bearer_auth(&shared.api_key)
            .timeout(HEALTH_TIMEOUT)
            .send()
            .await,
        Ok(resp) if resp.status().is_success()
    )
}

fn exited(child: &Mutex<Child>) -> Option<Option<i32>> {
    let mut child = child.lock().ok()?;
    match child.try_wait() {
        Ok(Some(status)) => Some(status.code()),
        Ok(None) => None,
        Err(_) => Some(None),
    }
}

fn drain_sentinel(events: &mut mpsc::UnboundedReceiver<LineEvent>) -> Option<Sentinel> {
    let mut found = None;
    while let Ok(event) = events.try_recv() {
        if let LineEvent::Sentinel(s) = event {
            found = Some(s);
        }
    }
    found
}

/// SIGTERM → 最多等 3 s → SIGKILL，并回收。只碰自己拉起的那个 pid。
async fn terminate(child: &Arc<Mutex<Child>>) {
    let child = Arc::clone(child);
    let _ = tokio::task::spawn_blocking(move || terminate_blocking(&child)).await;
}

fn terminate_blocking(child: &Mutex<Child>) {
    let Ok(mut child) = child.lock() else {
        return;
    };
    if matches!(child.try_wait(), Ok(Some(_))) {
        return;
    }
    send_term(child.id());
    let deadline = Instant::now() + TERM_GRACE;
    while Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
fn send_term(pid: u32) {
    // 不为一个信号引入 libc / unsafe（本 crate `forbid(unsafe_code)`）。
    let _ = Command::new("/bin/kill")
        .args(["-TERM", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(not(unix))]
fn send_term(_pid: u32) {
    // Windows 没有 SIGTERM；直接走 kill（TerminateProcess）。
}

/// 结束上次运行崩溃留下的路由器：pid 文件里的进程**仍在跑且可执行文件就是我们的
/// `llama-server`** 才动它，否则只删 pid 文件（pid 可能已被系统复用给别的进程）。
fn reap_stale(pid_path: &Path, server_bin: &Path) {
    let Some(pid) = std::fs::read_to_string(pid_path)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
    else {
        return;
    };
    if process_executable(pid).is_some_and(|exe| same_file(&exe, server_bin)) {
        send_term(pid);
        let deadline = Instant::now() + TERM_GRACE;
        while Instant::now() < deadline && process_executable(pid).is_some() {
            std::thread::sleep(Duration::from_millis(50));
        }
        if process_executable(pid).is_some() {
            #[cfg(unix)]
            let _ = Command::new("/bin/kill")
                .args(["-KILL", &pid.to_string()])
                .status();
        }
    }
    let _ = std::fs::remove_file(pid_path);
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

#[cfg(unix)]
fn process_executable(pid: u32) -> Option<PathBuf> {
    let out = Command::new("/bin/ps")
        .args(["-o", "comm=", "-p", &pid.to_string()])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let comm = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (!comm.is_empty()).then(|| PathBuf::from(comm))
}

#[cfg(not(unix))]
fn process_executable(_pid: u32) -> Option<PathBuf> {
    None
}

/// 写文件并（unix 上）收紧为仅本用户可读——preset 里有模型路径。
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn set_state(shared: &Shared, state: EngineState, failure: Option<Failure>) {
    shared.status.send_modify(|status| {
        status.state = state;
        if failure.is_some() {
            status.last_failure = failure;
        }
    });
}
