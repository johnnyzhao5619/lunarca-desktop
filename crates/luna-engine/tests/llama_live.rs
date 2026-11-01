//! 实机：用真 `llama-server` 与真 GGUF 验证路由器监管（ADR-143 ⑤）。默认 `#[ignore]`。
//!
//! 跑法（目录换成本机的；模型取一个对话模型即可，越小越快）：
//!
//! ```text
//! LUNA_LLAMA_BIN_DIR=~/Dev/lunarca-p0/bin/llama-b11429 \
//! LUNA_LLAMA_TEST_GGUF=~/Dev/lunarca-p0/models/qwen3.5-0.8b-q8_0.gguf \
//! cargo test -p luna-engine --test llama_live -- --ignored --test-threads=1
//! ```
//!
//! 必须串行：用例之间共享「本机不应残留 llama-server」这条断言。

use std::path::PathBuf;
use std::time::{Duration, Instant};

use luna_engine::llama::{
    plan, render_preset, EngineState, FitParamsEstimator, FitRequest, FitVerdict, LaunchSpec,
    LlamaServerSupervisor, ModelRole, PresetModel,
};

const MODEL_ID: &str = "test-chat";

struct Env {
    bin_dir: PathBuf,
    gguf: PathBuf,
    work: PathBuf,
}

fn env() -> Env {
    let expand = |v: String| PathBuf::from(v.replacen('~', &std::env::var("HOME").unwrap(), 1));
    let bin_dir = expand(std::env::var("LUNA_LLAMA_BIN_DIR").expect("LUNA_LLAMA_BIN_DIR"));
    let gguf = expand(std::env::var("LUNA_LLAMA_TEST_GGUF").expect("LUNA_LLAMA_TEST_GGUF"));
    let work = std::env::temp_dir().join(format!("luna-llama-live-{}", std::process::id()));
    std::fs::create_dir_all(&work).unwrap();
    Env {
        bin_dir,
        gguf,
        work,
    }
}

fn spec(env: &Env, sleep_idle_seconds: u32) -> LaunchSpec {
    let model = PresetModel {
        id: MODEL_ID.into(),
        path: env.gguf.clone(),
        role: ModelRole::Chat { thinking: true },
        ctx: 4096,
        kv: luna_engine::llama::KvType::F16,
    };
    LaunchSpec {
        server_bin: env.bin_dir.join("llama-server"),
        preset_ini: render_preset(sleep_idle_seconds, &[model]).unwrap(),
        preset_path: env.work.join("preset.ini"),
        pid_path: env.work.join("router.pid"),
        models_max: 1,
    }
}

async fn wait_ready(sup: &LlamaServerSupervisor, within: Duration) -> u16 {
    let mut rx = sup.subscribe();
    let deadline = Instant::now() + within;
    loop {
        if let EngineState::Ready { port } = rx.borrow().state {
            return port;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(
            !left.is_zero(),
            "not ready within {within:?}: {:?}",
            sup.status()
        );
        let _ = tokio::time::timeout(left, rx.changed()).await;
    }
}

async fn chat(sup: &LlamaServerSupervisor) -> String {
    let (base, key) = sup.endpoint().expect("endpoint");
    let body = serde_json::json!({
        "model": MODEL_ID,
        "messages": [{"role": "user", "content": "1+1 等于几？只回答数字。"}],
        "max_tokens": 8,
        "temperature": 0,
    });
    let resp: serde_json::Value = reqwest::Client::new()
        .post(format!("{base}/v1/chat/completions"))
        .bearer_auth(key)
        .json(&body)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    resp["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

fn pids_of(bin: &std::path::Path) -> Vec<u32> {
    let out = std::process::Command::new("pgrep")
        .arg("-f")
        .arg(bin)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .filter_map(|p| p.parse().ok())
        .collect()
}

fn router_pid(env: &Env) -> u32 {
    std::fs::read_to_string(env.work.join("router.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

fn kill9(pid: u32) {
    std::process::Command::new("/bin/kill")
        .args(["-9", &pid.to_string()])
        .status()
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "需要随包 llama-server 与一个 GGUF（见文件头）"]
async fn starts_serves_and_stops_without_orphans() {
    let env = env();
    let sup = LlamaServerSupervisor::new();
    sup.start(spec(&env, 600)).await;
    wait_ready(&sup, Duration::from_secs(30)).await;
    let answer = chat(&sup).await;
    assert!(answer.contains('2'), "answer: {answer:?}");
    assert!(!answer.contains("<think>"));

    // 不带密钥直连路由器必须被拒（密钥经环境变量下发，ADR-143 ③）
    let (base, _) = sup.endpoint().unwrap();
    let anon = reqwest::get(format!("{base}/v1/models")).await.unwrap();
    assert_eq!(anon.status(), 401);

    sup.stop().await;
    assert_eq!(sup.status().state, EngineState::Stopped);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(
        pids_of(&env.bin_dir.join("llama-server")).is_empty(),
        "orphans left"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "需要随包 llama-server 与一个 GGUF（见文件头）"]
async fn router_killed_is_restarted_within_seconds() {
    let env = env();
    let sup = LlamaServerSupervisor::new();
    sup.start(spec(&env, 600)).await;
    let first = wait_ready(&sup, Duration::from_secs(30)).await;
    let mut rx = sup.subscribe();
    // P1 判据：从强杀那一刻起 ≤ 5 s 重新就绪（退出轮询 250 ms + 首次退避 1 s + 拉起）
    let t = Instant::now();
    kill9(router_pid(&env));
    loop {
        let state = rx.borrow().state.clone();
        if matches!(state, EngineState::Ready { port } if port != first) {
            break;
        }
        let left = Duration::from_secs(5).saturating_sub(t.elapsed());
        assert!(
            !left.is_zero(),
            "not recovered within 5 s: {:?}",
            sup.status()
        );
        let _ = tokio::time::timeout(left, rx.changed()).await;
    }
    eprintln!("router recovered in {:?}", t.elapsed());
    assert!(matches!(
        sup.status().last_failure,
        Some(luna_engine::llama::Failure::Exited(_))
    ));
    assert!(chat(&sup).await.contains('2'));
    sup.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "需要随包 llama-server 与一个 GGUF（见文件头）"]
async fn child_killed_is_reloaded_by_the_router() {
    let env = env();
    let sup = LlamaServerSupervisor::new();
    sup.start(spec(&env, 600)).await;
    wait_ready(&sup, Duration::from_secs(30)).await;
    chat(&sup).await;
    let router = router_pid(&env);
    let children: Vec<u32> = String::from_utf8_lossy(
        &std::process::Command::new("pgrep")
            .args(["-P", &router.to_string()])
            .output()
            .unwrap()
            .stdout,
    )
    .split_whitespace()
    .filter_map(|p| p.parse().ok())
    .collect();
    assert_eq!(children.len(), 1, "one loaded model = one child");
    kill9(children[0]);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let t = Instant::now();
    assert!(chat(&sup).await.contains('2'));
    eprintln!("child reloaded and answered in {:?}", t.elapsed());
    assert!(
        matches!(sup.status().state, EngineState::Ready { .. }),
        "router itself untouched"
    );
    sup.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "需要随包 llama-server 与一个 GGUF（见文件头）"]
async fn idle_sleep_wakes_on_first_request() {
    let env = env();
    let sup = LlamaServerSupervisor::new();
    sup.start(spec(&env, 3)).await;
    wait_ready(&sup, Duration::from_secs(30)).await;
    chat(&sup).await;
    tokio::time::sleep(Duration::from_secs(8)).await;
    // 休眠期间健康探针照常（/health 不重置空闲计时），状态仍是 Ready
    assert!(matches!(sup.status().state, EngineState::Ready { .. }));
    let t = Instant::now();
    assert!(chat(&sup).await.contains('2'));
    eprintln!("woke and answered in {:?}", t.elapsed());
    sup.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "需要随包 llama-server 与一个 GGUF（见文件头）"]
async fn stale_router_from_a_crashed_run_is_reaped() {
    let env = env();
    let bin = env.bin_dir.join("llama-server");
    // 模拟上次运行崩溃：一个我们的 llama-server 还活着，pid 文件还在
    std::fs::write(env.work.join("stale.ini"), "[*]\nnp = 1\n").unwrap();
    let mut stale = std::process::Command::new(&bin)
        .args([
            "--host",
            "127.0.0.1",
            "--port",
            "0",
            "--no-webui",
            "--models-preset",
        ])
        .arg(env.work.join("stale.ini"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    std::fs::write(env.work.join("router.pid"), stale.id().to_string()).unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    let sup = LlamaServerSupervisor::new();
    sup.start(spec(&env, 600)).await;
    wait_ready(&sup, Duration::from_secs(30)).await;
    assert!(
        !pids_of(&bin).contains(&stale.id()),
        "stale router still alive"
    );
    // 被监管器结束的是它；这里只负责收尸，免得留僵尸进程。
    let _ = stale.wait();
    sup.stop().await;
}

/// 预检用的估算器在真工具上可用，且对 P0-7 事故那组参数给出「放不下」。
#[test]
#[ignore = "需要随包 llama-fit-params 与一个 GGUF（见文件头）"]
fn fit_params_estimator_refuses_the_panic_shape() {
    let env = env();
    let est = FitParamsEstimator::new(env.bin_dir.join("llama-fit-params")).with_model(
        MODEL_ID,
        env.gguf.clone(),
        false,
    );
    let ok = plan(
        16 * 1024,
        2457,
        &[FitRequest {
            id: MODEL_ID.into(),
            ctx: 8192,
            min_ctx: 4096,
        }],
        &est,
    );
    assert!(
        matches!(ok[0], FitVerdict::Fits { ctx: 8192, .. }),
        "{ok:?}"
    );
    // 只给 1 GiB 额度、下限也设成 262 144：必须拒绝而不是硬上
    let refused = plan(
        1024 + 2457,
        2457,
        &[FitRequest {
            id: MODEL_ID.into(),
            ctx: 262_144,
            min_ctx: 262_144,
        }],
        &est,
    );
    assert!(
        matches!(refused[0], FitVerdict::Refused { .. }),
        "{refused:?}"
    );
}
