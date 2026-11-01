//! 实机测试共用：起一个真 `llama-server` 路由器（随包推理引擎，ADR-143）。
//!
//! 引擎包目录取 `LUNA_LLAMA_BIN_DIR`，模型由调用方写进 preset。路由器随 [`Router`] 的
//! `Drop` 以 SIGTERM 收掉（被 SIGKILL 时子实例会 abort 并留下崩溃报告，P1 实测）。

#![allow(dead_code)]

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use luna_provider::{LocalEngineEndpoint, LocalEngineSource};

const KEY: &str = "live-test-key";

/// 一个跑着的路由器。
pub struct Router {
    child: Child,
    port: u16,
}

impl Drop for Router {
    fn drop(&mut self) {
        let _ = Command::new("kill")
            .args(["-TERM", &self.child.id().to_string()])
            .status();
        let _ = self.child.wait();
    }
}

impl LocalEngineSource for Router {
    fn endpoint(&self) -> Option<LocalEngineEndpoint> {
        Some(LocalEngineEndpoint {
            base_url: format!("http://127.0.0.1:{}", self.port),
            api_key: KEY.into(),
        })
    }
}

/// 环境变量里的路径（支持 `~`）。缺了直接 panic，说清缺哪个。
pub fn path(var: &str) -> PathBuf {
    let raw = std::env::var(var).unwrap_or_else(|_| panic!("缺环境变量 {var}"));
    PathBuf::from(raw.replacen('~', &std::env::var("HOME").unwrap(), 1))
}

/// 以给定的 preset 正文（`[*]` 段之后的各模型节）起路由器并等它监听。
pub fn start(models: &str, models_max: u32) -> Arc<Router> {
    let work = std::env::temp_dir().join(format!(
        "luna-provider-live-{}-{}",
        std::process::id(),
        Instant::now().elapsed().as_nanos()
    ));
    std::fs::create_dir_all(&work).unwrap();
    let preset = work.join("preset.ini");
    std::fs::write(
        &preset,
        format!("[*]\nflash-attn = auto\nnp = 1\n\n{models}"),
    )
    .unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let child = Command::new(path("LUNA_LLAMA_BIN_DIR").join("llama-server"))
        .args(["--models-preset"])
        .arg(&preset)
        .args([
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--no-webui",
            "--models-max",
            &models_max.to_string(),
        ])
        .env("LLAMA_API_KEY", KEY)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let router = Arc::new(Router { child, port });
    let deadline = Instant::now() + Duration::from_secs(30);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(Instant::now() < deadline, "路由器 30 s 内没起来");
        std::thread::sleep(Duration::from_millis(200));
    }
    router
}

/// 只带一个对话模型的路由器（`LUNA_LLAMA_CHAT_GGUF`，请求里的 model 叫 `chat`）。
/// 上下文取 `LUNA_LLAMA_CTX`（缺省 8192；主题生成探针按 16k 设计时给 16384）。
pub fn start_chat() -> Arc<Router> {
    let ctx = std::env::var("LUNA_LLAMA_CTX").unwrap_or_else(|_| "8192".into());
    start(
        &format!(
            "[chat]\nmodel = {}\nctx-size = {ctx}\nreasoning = off\n",
            path("LUNA_LLAMA_CHAT_GGUF").display()
        ),
        1,
    )
}
