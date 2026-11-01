//! 监管器的纯决策部分（ADR-143 ⑤）：端口解析、日志哨兵、重启退避与熔断、健康判定。
//!
//! 与进程 I/O 分开，是为了让「什么时候重启、什么时候放弃」能被确定性地测试——
//! 真进程只在 `#[ignore]` 的实机测试里跑。

use std::time::{Duration, Instant};

/// 从路由器的一行日志里取监听端口（`--port 0` 时由系统分配）。
///
/// 行形如 `main: server is listening on http://127.0.0.1:52811`；只认 127.0.0.1，
/// 别的地址意味着参数被改坏了，宁可起不来也不对外监听。
#[must_use]
pub fn parse_listening(line: &str) -> Option<u16> {
    let rest = line.split("listening on http://127.0.0.1:").nth(1)?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok().filter(|port| *port != 0)
}

/// 日志哨兵：只按**类别**上报，原始行从不进日志（可能含请求片段，TDD-12 日志脱敏）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sentinel {
    /// Metal / 计算后端报错：进程可能「活着但废了」。
    ComputeError,
    /// 权重文件损坏或不完整：重启无用，须标记条目并提示重下（P0-7）。
    CorruptModel,
    /// preset / 参数被拒（编程错误或引擎升级后参数改名）。
    BadArguments,
}

/// 对一行日志分类；无关行返回 `None`。
#[must_use]
pub fn classify_line(line: &str) -> Option<Sentinel> {
    if line.contains("model is corrupted or incomplete")
        || line.contains("not within the file bounds")
    {
        Some(Sentinel::CorruptModel)
    } else if line.contains("Compute error")
        || (line.contains("command buffer") && line.contains("failed"))
    {
        Some(Sentinel::ComputeError)
    } else if line.contains("invalid argument") || line.contains("error while handling argument") {
        Some(Sentinel::BadArguments)
    } else {
        None
    }
}

/// 重启策略：指数退避 1 s → 30 s；**连续** 5 次没撑过稳定期就熔断。
#[derive(Debug, Clone)]
pub struct RestartPolicy {
    consecutive: u32,
    ready_since: Option<Instant>,
}

/// 进程退出后的决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartDecision {
    /// 等这么久再拉起。
    RetryAfter(Duration),
    /// 熔断：不再自动拉起，等用户在界面上手动重启。
    Trip,
}

/// 连续失败多少次熔断。
pub const MAX_CONSECUTIVE_FAILURES: u32 = 5;
/// 就绪后撑过多久算「稳定」，清零失败计数。
pub const STABLE_AFTER: Duration = Duration::from_secs(60);

impl Default for RestartPolicy {
    fn default() -> Self {
        Self::new()
    }
}

impl RestartPolicy {
    /// 新策略。
    #[must_use]
    pub fn new() -> Self {
        Self {
            consecutive: 0,
            ready_since: None,
        }
    }

    /// 进程就绪（`/health` 首次 200）。
    pub fn on_ready(&mut self, now: Instant) {
        self.ready_since = Some(now);
    }

    /// 进程意外退出（或被判死后杀掉）。
    pub fn on_exit(&mut self, now: Instant) -> RestartDecision {
        if self
            .ready_since
            .take()
            .is_some_and(|since| now.duration_since(since) >= STABLE_AFTER)
        {
            self.consecutive = 0;
        }
        self.consecutive += 1;
        if self.consecutive >= MAX_CONSECUTIVE_FAILURES {
            return RestartDecision::Trip;
        }
        let secs = 1u64 << (self.consecutive - 1).min(5);
        RestartDecision::RetryAfter(Duration::from_secs(secs.min(30)))
    }

    /// 用户手动重启：清零，重新给满次数。
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// 已连续失败的次数（状态上报用）。
    #[must_use]
    pub fn failures(&self) -> u32 {
        self.consecutive
    }
}

/// 健康探针判定：进程还在但 `/health` 连续不响应（SIGSTOP 类挂起，P0-7）就判死。
#[derive(Debug, Clone, Default)]
pub struct HealthTracker {
    misses: u32,
}

/// 连续多少次探针失败判死。
pub const MAX_HEALTH_MISSES: u32 = 3;

impl HealthTracker {
    /// 记一次探针结果；返回 `true` = 该判死并重启了。
    pub fn record(&mut self, ok: bool) -> bool {
        if ok {
            self.misses = 0;
            false
        } else {
            self.misses += 1;
            self.misses >= MAX_HEALTH_MISSES
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_listening_lines() {
        assert_eq!(
            parse_listening("0.00.123.456 I main: server is listening on http://127.0.0.1:52811 - starting the main loop"),
            Some(52_811)
        );
        assert_eq!(
            parse_listening("listening on http://127.0.0.1:49755"),
            Some(49_755)
        );
        assert_eq!(parse_listening("listening on http://0.0.0.0:8080"), None);
        assert_eq!(parse_listening("listening on http://127.0.0.1:0"), None);
        assert_eq!(parse_listening("loading model"), None);
    }

    #[test]
    fn classifies_sentinels_from_p0_logs() {
        assert_eq!(
            classify_line("E llama_model_load: error loading model: tensor 'token_embd.weight' data is not within the file bounds, model is corrupted or incomplete"),
            Some(Sentinel::CorruptModel)
        );
        assert_eq!(
            classify_line("ggml_metal: Compute error."),
            Some(Sentinel::ComputeError)
        );
        assert_eq!(
            classify_line("ggml_metal_graph_compute: command buffer 0 failed with status 5"),
            Some(Sentinel::ComputeError)
        );
        assert_eq!(
            classify_line("error: invalid argument: --embeddings"),
            Some(Sentinel::BadArguments)
        );
        assert_eq!(
            classify_line("slot release: id 0 | task 81 | stop processing"),
            None
        );
    }

    #[test]
    fn backs_off_exponentially_then_trips() {
        let t0 = Instant::now();
        let mut p = RestartPolicy::new();
        let got: Vec<_> = (0..5).map(|_| p.on_exit(t0)).collect();
        assert_eq!(
            got,
            vec![
                RestartDecision::RetryAfter(Duration::from_secs(1)),
                RestartDecision::RetryAfter(Duration::from_secs(2)),
                RestartDecision::RetryAfter(Duration::from_secs(4)),
                RestartDecision::RetryAfter(Duration::from_secs(8)),
                RestartDecision::Trip,
            ]
        );
    }

    #[test]
    fn a_stable_run_clears_the_failure_count() {
        let t0 = Instant::now();
        let mut p = RestartPolicy::new();
        for _ in 0..4 {
            p.on_exit(t0);
        }
        p.on_ready(t0);
        // 稳定运行超过 60 s 后再挂：从第一次算起
        assert_eq!(
            p.on_exit(t0 + STABLE_AFTER),
            RestartDecision::RetryAfter(Duration::from_secs(1))
        );
        // 就绪但没撑过稳定期：继续累计
        p.on_ready(t0);
        assert_eq!(
            p.on_exit(t0 + Duration::from_secs(5)),
            RestartDecision::RetryAfter(Duration::from_secs(2))
        );
    }

    #[test]
    fn manual_reset_restores_the_budget() {
        let t0 = Instant::now();
        let mut p = RestartPolicy::new();
        for _ in 0..5 {
            p.on_exit(t0);
        }
        p.reset();
        assert_eq!(p.failures(), 0);
        assert_eq!(
            p.on_exit(t0),
            RestartDecision::RetryAfter(Duration::from_secs(1))
        );
    }

    #[test]
    fn three_consecutive_health_misses_declare_death() {
        let mut h = HealthTracker::default();
        assert!(!h.record(false));
        assert!(!h.record(false));
        assert!(!h.record(true));
        assert!(!h.record(false));
        assert!(!h.record(false));
        assert!(h.record(false));
    }
}
