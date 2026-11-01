//! 网络 Provider 的失败计数熔断器（TDD-01 §3.1）。
//!
//! # 为什么不主动探活
//!
//! TDD-01 §3.1 要求 `health()` 缓存 + 熔断、禁止每 Job 探活。**「缓存」在这里不是缓存一次
//! 探测的结果，而是压根不发探测**：主动探活对云端点意味着一次自发的云网络行为，
//! 而 ADR-037 的运行期不变量是「未登录时不自发产生任何云网络行为」——
//! 一个后台定时探活会当场把它推翻，且没有任何用户动作能解释那次请求。
//!
//! 故本熔断器**只记录真实请求的结果**：可用性在用户真的要用的时候暴露，失败累计到阈值后
//! 才拒绝派活。这既满足「禁止每 Job 探活」（不额外发请求），也满足「熔断参与健康度」。
//!
//! # 什么算失败
//!
//! 只有**端点不可达**（连不上 / 超时）才计入。服务端明确回答了的那些——额度用尽、
//! 未登录、模型不存在、上游 5xx——统统不算：那些恰恰证明端点是活的，而且各自有用户必须
//! 知道且能处理的动作。把它们计入熔断的后果是：用户加完额度回来，还要莫名其妙再等一个
//! 退避周期；而这段时间里他得到的错误是「服务不可用」，指向一个完全错误的修复方向。

use std::time::Duration;

use luna_core::{Backoff, HealthStatus, ProviderHealth};

/// 连续多少次不可达即熔断。
///
/// 取 3 而不是 1：一次瞬时抖动（切网、休眠唤醒）不该让端点被判死。钥匙链那两处取 1——
/// 那里的失败是用户拒绝授权或钥匙串锁着，不是抖动。
const TRIP_AFTER: u32 = 3;

/// 熔断后的退避时长；到点后下一个**真实请求**被放行（Half-Open），成功即复位。
///
/// 对齐 TDD-01 §3.1 的 ~30s 口径。
const BACKOFF: Duration = Duration::from_secs(30);

/// 单个端点的失败计数熔断。
///
/// 退避机制本身是共享原语（[`luna_core::Backoff`]，钥匙链读取用的是同一个）；本类型只
/// 负责这一层的语义：把它翻译成 `ProviderHealth`，并守住「什么算失败」那条边界。
#[derive(Debug)]
pub struct Breaker {
    inner: Backoff,
}

/// 本熔断器只跟踪一个目标（Provider 各持一个），故 key 恒定。
const ENDPOINT: &str = "endpoint";

impl Default for Breaker {
    fn default() -> Self {
        Self::new()
    }
}

impl Breaker {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Backoff::new(TRIP_AFTER, BACKOFF),
        }
    }

    /// 此刻的健康结论。**不发任何请求**。
    #[must_use]
    pub fn health(&self) -> ProviderHealth {
        if !self.inner.should_skip(ENDPOINT) {
            return ProviderHealth::ready();
        }
        ProviderHealth {
            status: HealthStatus::Unavailable,
            detail: Some(format!(
                "连续 {} 次连不上，暂停派发；约 {} 秒后自动重试",
                self.inner.failures(ENDPOINT),
                self.inner
                    .remaining(ENDPOINT)
                    .map_or(1, |left| left.as_secs().max(1))
            )),
        }
    }

    /// 一次真实请求成功抵达了端点（哪怕业务上失败）——端点是活的，复位。
    pub fn record_reachable(&self) {
        self.inner.record_success(ENDPOINT);
    }

    /// 一次真实请求**没能抵达端点**（连不上 / 超时）。
    pub fn record_unreachable(&self) {
        self.inner.record_failure(ENDPOINT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 四条状态转移一起钉住——**任何一条反了都会造成一种新的坏行为**。
    #[test]
    fn it_trips_after_repeated_unreachability_and_recovers_on_its_own() {
        let breaker = Breaker::new();
        assert!(
            breaker.health().is_ready(),
            "初始必须放行，否则首次使用即失败"
        );

        // 单次抖动（切网、休眠唤醒）不该判死端点。
        breaker.record_unreachable();
        assert!(
            breaker.health().is_ready(),
            "一次连不上就熔断——切一次网就要等一个退避周期"
        );

        for _ in 1..TRIP_AFTER {
            breaker.record_unreachable();
        }
        let tripped = breaker.health();
        assert!(
            !tripped.is_ready(),
            "连续失败后仍派活——每次都白等一次连接超时"
        );
        assert!(
            tripped.detail.is_some(),
            "不可用必须带原因——`ProviderHealth::detail` 的契约就是「不可用原因」，\
             留空会让诊断时只剩一个状态枚举"
        );

        // 端点恢复（一次真实请求成功抵达）即复位，不必等退避走完。
        breaker.record_reachable();
        assert!(breaker.health().is_ready(), "成功抵达后仍拒派活");
    }
}
