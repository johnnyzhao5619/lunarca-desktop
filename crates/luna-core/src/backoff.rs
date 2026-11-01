//! 失败退避：**同一个目标连续失败之后，一段时间内不再去碰它**。
//!
//! # 为什么是共享原语
//!
//! 三处需要同一条策略，且各自漏做的代价都很具体：
//! - 系统钥匙链读取（`luna-config` 的设置密钥、`luna-cloud` 的账户凭据）：读不到时 macOS
//!   每次都弹一个要求输入钥匙串密码的**模态框**。而这两条都在界面路径上——密钥设置行
//!   每次渲染问一次「设没设」，算力落点每次设置变更重算一次——于是用户点几下设置就被
//!   连续打断，点「拒绝」之后立刻又来。
//! - 网络 Provider（`luna-provider` 的熔断）：端点挂掉时每一次请求各等一次连接超时。
//!
//! 各写一份的下场不是"多写几行"，而是三处阈值与恢复语义各自演化，而它们本该是同一条。
//!
//! # 不主动探测
//!
//! 退避到点后**不补一次探测**，只是放行下一个真实请求（Half-Open）。对云端点而言，一次
//! 自发的探测就是一次没有用户动作能解释的云网络行为，会当场推翻 ADR-037 的运行期不变量。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 按目标（key）记录连续失败并在阈值后进入静默期。
///
/// 线程安全且内部可变——持有方通常是个 `&self` 方法密集的 store / provider。
#[derive(Debug)]
pub struct Backoff {
    trip_after: u32,
    quiet: Duration,
    state: Mutex<HashMap<String, Entry>>,
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    consecutive_failures: u32,
    quiet_since: Option<Instant>,
}

impl Backoff {
    /// `trip_after` 次连续失败后静默 `quiet`。
    ///
    /// `trip_after = 1` 表示「一次失败就静默」——钥匙链取这一档：那不是抖动，是用户拒绝了
    /// 授权或钥匙串锁着，再问一次只会再被拒绝一次。网络端点取 3：一次瞬时抖动（切网、
    /// 休眠唤醒）不该让它被判死。
    #[must_use]
    pub fn new(trip_after: u32, quiet: Duration) -> Self {
        Self {
            trip_after: trip_after.max(1),
            quiet,
            state: Mutex::new(HashMap::new()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        // 锁中毒不该让这个目标从此不可用：退化为「当作没在静默」，最坏是多试一次。
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 此刻该不该跳过对这个目标的尝试。**不做任何 IO**。
    ///
    /// 静默期到点即自行解除（Half-Open）：下一个真实请求被放行，由它的结果决定去留。
    /// 没有这一步，目标恢复之后要等到进程重启才会被再次尝试。
    #[must_use]
    pub fn should_skip(&self, key: &str) -> bool {
        let mut state = self.lock();
        let Some(entry) = state.get_mut(key) else {
            return false;
        };
        match entry.quiet_since {
            Some(since) if since.elapsed() < self.quiet => true,
            Some(_) => {
                entry.quiet_since = None;
                false
            }
            None => false,
        }
    }

    /// 静默期还剩多久（未在静默中则为 `None`）。仅供说明文案用。
    #[must_use]
    pub fn remaining(&self, key: &str) -> Option<Duration> {
        let state = self.lock();
        let entry = state.get(key)?;
        let since = entry.quiet_since?;
        self.quiet.checked_sub(since.elapsed())
    }

    /// 记一次成功——目标是活的，清空该目标的一切记录。
    pub fn record_success(&self, key: &str) {
        self.lock().remove(key);
    }

    /// 记一次失败；达到阈值即进入静默期。
    pub fn record_failure(&self, key: &str) {
        let mut state = self.lock();
        let entry = state.entry(key.to_owned()).or_insert(Entry {
            consecutive_failures: 0,
            quiet_since: None,
        });
        entry.consecutive_failures = entry.consecutive_failures.saturating_add(1);
        if entry.consecutive_failures >= self.trip_after && entry.quiet_since.is_none() {
            entry.quiet_since = Some(Instant::now());
        }
    }

    /// 连续失败次数（诊断/文案用）。
    #[must_use]
    pub fn failures(&self, key: &str) -> u32 {
        self.lock()
            .get(key)
            .map_or(0, |entry| entry.consecutive_failures)
    }

    /// 把某个目标的静默期人为提前到点。
    ///
    /// 只在本 crate 的测试里存在——「到点自解除」的语义在这里验一次就够，下游 crate
    /// 各自验自己的那部分（什么算失败、失败之后做什么），不必再穿透到时间。
    #[cfg(test)]
    fn expire_for_test(&self, key: &str) {
        if let Some(entry) = self.lock().get_mut(key) {
            entry.quiet_since = Instant::now().checked_sub(self.quiet + Duration::from_secs(1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 四条状态转移一起钉住——每一条反了都对应一种具体的坏行为。
    #[test]
    fn it_goes_quiet_after_the_threshold_and_recovers_by_itself() {
        let backoff = Backoff::new(3, Duration::from_secs(30));
        let key = "endpoint";

        assert!(
            !backoff.should_skip(key),
            "初始必须放行，否则首次使用即失败"
        );

        backoff.record_failure(key);
        assert!(
            !backoff.should_skip(key),
            "未达阈值就静默——一次抖动（切网、休眠唤醒）就要等一个静默期"
        );

        backoff.record_failure(key);
        backoff.record_failure(key);
        assert!(backoff.should_skip(key), "达到阈值仍不静默——重试风暴照旧");
        assert!(backoff.remaining(key).is_some());
        assert_eq!(backoff.failures(key), 3);

        backoff.record_success(key);
        assert!(!backoff.should_skip(key), "成功之后仍在静默");
        assert_eq!(backoff.failures(key), 0);
    }

    /// 静默期到点自行解除；不解除意味着目标恢复后要重启进程才会被再次尝试。
    #[test]
    fn the_quiet_period_expires_on_its_own() {
        let backoff = Backoff::new(1, Duration::from_secs(60));
        backoff.record_failure("k");
        assert!(backoff.should_skip("k"));

        backoff.expire_for_test("k");
        assert!(
            !backoff.should_skip("k"),
            "静默期不会到点——目标恢复后要重启才会被再次尝试"
        );
    }

    /// 各目标互不影响：一个条目读不到，不该把另一个也拖进静默。
    #[test]
    fn targets_are_tracked_independently() {
        let backoff = Backoff::new(1, Duration::from_secs(60));
        backoff.record_failure("a");
        assert!(backoff.should_skip("a"));
        assert!(!backoff.should_skip("b"), "一个目标失败就把别的也静默了");
    }
}
