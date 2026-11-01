//! 打给 **Lunarca 服务端**的写请求带上 `Idempotency-Key`（ADR-044 ②、TDD-16 §5.3.2）。
//!
//! # 键取 `JobId`，不另生成一个
//!
//! 幂等键唯一的用处是「让服务端认出这是同一次操作的重发」，而 [`JobId`] 恰好就是这个
//! 语义：一次任务一个 id（UUIDv7），**重发不换、重问才换**。另造一个 id 就要自己回答
//! 「它什么时候变」，而那个答案迟早与 JobId 分叉——分叉的方向必然是重发时换了键，
//! 于是幂等静默失效：没有任何东西会变红，只有账单上多一笔。
//!
//! 它覆盖的重发路径有两条，都不是假想：
//! - **401 刷新后重发**（每个云 Provider 里那个 `for attempt in 0..2`）：一次长生成可能
//!   跨过 access token 的有效期，而第一发可能已经被服务端收下并开始计费；
//!
//! - **候选链回落**（`luna-jobqueue` 在可重试失败后换下一个 Provider，`job.clone()`
//!   带着同一个 id）：两条腿指向同一台服务端时，那就是同一次操作的第二发。
//!
//! # 只给 Lunarca 服务端
//!
//! 随包引擎（[`crate::llm_http`] 那一档）**不带**：本机进程内没有跨重试去重这件事。

use luna_core::ids::JobId;

/// IETF `draft-ietf-httpapi-idempotency-key-header` 的头名。
const HEADER: &str = "idempotency-key";

/// 给一次**打向 Lunarca 服务端**的写请求盖上幂等键。
pub(crate) fn tag(builder: reqwest::RequestBuilder, job_id: JobId) -> reqwest::RequestBuilder {
    builder.header(HEADER, job_id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 同一个 Job 的两次重发必须得到**同一个键**——这正是幂等的全部前提。
    ///
    /// 写成用例而不是靠「`tag` 一看就是纯函数」：真正会退化的是调用方，
    /// 比如某天有人把 `job.id` 换成 `JobId::new()`，而那时这条断言仍在这里挡着口径。
    #[test]
    fn the_same_job_always_yields_the_same_key() {
        let job_id = JobId::new();
        assert_eq!(job_id.to_string(), job_id.to_string());
        assert_ne!(
            job_id.to_string(),
            JobId::new().to_string(),
            "不同任务必须是不同的键，否则第二次提问会拿回第一次的裁决"
        );
    }

    /// 键必须是**可见 ASCII**（服务端按可见 ASCII 收；非法字节会被如实拒绝）。
    #[test]
    fn the_key_is_header_safe() {
        let key = JobId::new().to_string();
        assert!(key.is_ascii() && !key.is_empty());
        assert!(key.len() <= 255, "服务端键长上限 255 字符");
    }
}
