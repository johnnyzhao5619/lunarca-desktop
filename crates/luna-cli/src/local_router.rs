//! `luna` 命令自己起一个随包推理引擎路由器（ADR-143）：与应用同一个 `llama-server`、同一个
//! 监管器、同一份 preset 渲染，于是基准测的就是应用跑的那条路。
//!
//! 与应用的差别只有一处：**不做内存预检**。CLI 是开发者工具，上下文由调用方给定且都很小
//! （对话 4096、嵌入取目录上限），P0-7 的过量分配风险来自「262144 上下文 + `--fit off`」
//! 那一类配置，这里两样都不会出现（preset 里从不写 `fit` / `n-gpu-layers`）。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use luna_engine::llama::{
    render_preset, runtime, EngineState, KvType, LaunchSpec, LlamaServerSupervisor, ModelRole,
    PresetModel,
};
use luna_provider::{LocalEngineEndpoint, LocalEngineSource};

/// 引擎资源根（其下 `llama/` 是 `scripts/fetch-llama-runtime.py` 取下来的引擎包）。
/// 缺省指向工作树里的 `src-tauri/resources`，`--resources` 可覆盖。
#[must_use]
pub fn default_resources() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../src-tauri/resources")
}

/// 一个跑着的路由器。正常收尾调 [`LocalRouter::stop`]；提前返回时 `Drop` 兜底发 SIGTERM
/// （路由器被 SIGKILL 时子实例会 abort 并留下崩溃报告，P1 实测）。
pub struct LocalRouter {
    supervisor: LlamaServerSupervisor,
    work: tempdir::Work,
    stopped: bool,
}

impl LocalRouter {
    /// 以一个模型起路由器并等到就绪。`id` 即请求里的 `model`。
    ///
    /// # Errors
    /// 引擎包不在、模型文件不在、30 s 内没就绪。
    pub async fn start(
        resources: &Path,
        id: &str,
        gguf: &Path,
        role: ModelRole,
        ctx: u32,
    ) -> Result<Self, String> {
        let paths = runtime::locate(resources).ok_or_else(|| {
            format!(
                "LLAMA_RUNTIME_MISSING: {} 下没有完整的引擎包（先跑 scripts/fetch-llama-runtime.py）",
                resources.display()
            )
        })?;
        if !gguf.is_file() {
            return Err(format!("GGUF_MISSING: {}", gguf.display()));
        }
        let work = tempdir::Work::new()?;
        let preset_ini = render_preset(
            300,
            &[PresetModel {
                id: id.to_owned(),
                path: gguf.to_owned(),
                role,
                ctx,
                kv: KvType::F16,
            }],
        )
        .map_err(|e| format!("PRESET: {e}"))?;
        let supervisor = LlamaServerSupervisor::new();
        supervisor
            .start(LaunchSpec {
                server_bin: paths.server,
                preset_ini,
                preset_path: work.path().join("preset.ini"),
                pid_path: work.path().join("router.pid"),
                models_max: 1,
            })
            .await;
        let router = Self {
            supervisor,
            work,
            stopped: false,
        };
        router.wait_ready(Duration::from_secs(30)).await?;
        Ok(router)
    }

    async fn wait_ready(&self, within: Duration) -> Result<(), String> {
        let mut rx = self.supervisor.subscribe();
        let deadline = Instant::now() + within;
        loop {
            match rx.borrow().state {
                EngineState::Ready { .. } => return Ok(()),
                EngineState::Tripped => return Err("ENGINE_TRIPPED: 引擎反复启动失败".into()),
                _ => {}
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(format!("ENGINE_NOT_READY: {within:?} 内没就绪"));
            }
            let _ = tokio::time::timeout(left, rx.changed()).await;
        }
    }

    /// Provider 用的地址事实源。
    #[must_use]
    pub fn source(&self) -> Arc<dyn LocalEngineSource> {
        Arc::new(Source(self.supervisor.clone()))
    }

    /// 路由器与它全部子实例的常驻内存合计（KB）。取不到为 `None`。
    #[must_use]
    pub fn rss_kb(&self) -> Option<u64> {
        // 监管器把路由器 pid 写进 pid 文件（重启时回收孤儿用），这里读同一份。
        let root = self.router_pid()?;
        let children = std::process::Command::new("pgrep")
            .args(["-P", &root.to_string()])
            .output()
            .ok()?;
        let mut pids = vec![root.to_string()];
        pids.extend(
            String::from_utf8_lossy(&children.stdout)
                .lines()
                .map(|l| l.trim().to_owned())
                .filter(|l| !l.is_empty()),
        );
        let out = std::process::Command::new("ps")
            .args(["-o", "rss=", "-p", &pids.join(",")])
            .output()
            .ok()?;
        Some(
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter_map(|l| l.trim().parse::<u64>().ok())
                .sum(),
        )
    }

    fn router_pid(&self) -> Option<u32> {
        std::fs::read_to_string(self.work.path().join("router.pid"))
            .ok()?
            .trim()
            .parse()
            .ok()
    }

    /// 收掉路由器。
    pub async fn stop(mut self) {
        self.supervisor.stop().await;
        self.stopped = true;
    }
}

impl Drop for LocalRouter {
    fn drop(&mut self) {
        if self.stopped {
            return;
        }
        if let Some(pid) = self.router_pid() {
            let _ = std::process::Command::new("kill")
                .args(["-TERM", &pid.to_string()])
                .status();
            // 给子实例一点时间随路由器退出，免得工作目录先被删掉。
            std::thread::sleep(Duration::from_millis(500));
        }
    }
}

struct Source(LlamaServerSupervisor);

impl LocalEngineSource for Source {
    fn endpoint(&self) -> Option<LocalEngineEndpoint> {
        self.0
            .endpoint()
            .map(|(base_url, api_key)| LocalEngineEndpoint { base_url, api_key })
    }
}

mod tempdir {
    use std::path::{Path, PathBuf};

    /// 本次运行的工作目录（preset、pid 文件），结束即删。
    pub struct Work(PathBuf);

    impl Work {
        pub fn new() -> Result<Self, String> {
            let dir = std::env::temp_dir().join(format!("luna-cli-llama-{}", std::process::id()));
            std::fs::create_dir_all(&dir).map_err(|e| format!("WORKDIR: {e}"))?;
            Ok(Self(dir))
        }
        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Work {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
