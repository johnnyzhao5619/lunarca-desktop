//! 任务级取消（PRD-05-010 / TDD-01 §5.5）。
//!
//! 一件用户可见的「任务」通常由**多个 Job 顺序组成**（导入 = 转写 Job + 逐句翻译 Job…）。
//! [`CancelToken`] 只描述单个 Job，故需要一层任务级令牌把「用户点了取消」传播到**当前在跑
//! 的那个 Job**，并让后续 Job 一出生就已取消。

use crate::provider::CancelToken;
use std::sync::Mutex;

/// 任务级取消令牌（PRD-05-010）。
///
/// **为什么不是「select! 掉 future」**：丢弃 future 只是不再等待，whisper 仍在阻塞线程池里
/// 把一小时的视频解码完——用户点了「取消」而 CPU 还在满转，是假承诺。故本令牌持有**当前在跑
/// 的 Job 的 `CancelToken`**，取消时一路传播到解码边界的 `is_cancelled()` 轮询（TDD-01 §5.5）。
///
/// 单 worker 顺序处理 → 同一时刻至多一个 Job 在跑，故只需一个槽位；`arm` 在每次提交 Job 后
/// 调用，若此前已请求取消则**立即**取消新令牌（消除「取消请求落在两个 Job 之间」的窗口）。
#[derive(Debug, Default)]
pub struct TaskCancel {
    /// 任务级标志。用 [`CancelToken`]（本身即 `Arc<AtomicBool>`）而非裸 `AtomicBool`——
    /// 于是 [`TaskCancel::flag`] 能交出一份 `Clone + Send + 'static` 的只读视图，
    /// 让**阻塞线程池里的解码循环**也能轮询它，而不必把整个任务令牌搬进闭包。
    requested: CancelToken,
    live: Mutex<Option<CancelToken>>,
}

impl TaskCancel {
    /// 请求取消：置位 + 取消当前在跑的 Job（幂等）。
    pub fn cancel(&self) {
        self.requested.cancel();
        if let Ok(slot) = self.live.lock() {
            if let Some(token) = slot.as_ref() {
                token.cancel();
            }
        }
    }

    /// 是否已请求取消（各阶段边界据此提前返回）。
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.requested.is_cancelled()
    }

    /// 可跨线程携带的**只读**取消视图（`Clone + Send + 'static`）。
    ///
    /// 给「只需要知道要不要停」的阻塞活用——典型是 symphonia 的逐包解码循环：
    /// 一个 GB 级视频解码可跑数分钟，若这段不可中断，「取消」就要等解码跑完才生效。
    #[must_use]
    pub fn flag(&self) -> CancelToken {
        self.requested.clone()
    }

    /// 登记当前 Job 的取消令牌；若取消已在此前被请求，立即取消之。
    pub fn arm(&self, token: CancelToken) {
        if self.requested.is_cancelled() {
            token.cancel();
        }
        if let Ok(mut slot) = self.live.lock() {
            *slot = Some(token);
        }
    }

    /// 取消已请求 → `Err(CANCELLED)`，供各阶段边界一行短路。
    ///
    /// # Errors
    /// 已请求取消时返回 `CANCELLED` 错误码（worker 据此置 `Cancelled` 而非 `Failed`）。
    pub fn bail_if_cancelled(&self) -> Result<(), String> {
        if self.is_cancelled() {
            return Err(CANCELLED_CODE.to_owned());
        }
        Ok(())
    }
}

/// 取消的错误码（worker 单一判定点据此把终态归为 `Cancelled`）。
pub const CANCELLED_CODE: &str = "CANCELLED";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arm_after_cancel_immediately_cancels_the_new_job_token() {
        // 取消请求恰好落在两个 Job 之间：新 Job 的令牌必须在登记时立刻被取消，
        // 否则「已点取消却又起了一段新解码」。
        let cancel = TaskCancel::default();
        cancel.cancel();
        let token = CancelToken::new();
        cancel.arm(token.clone());
        assert!(token.is_cancelled(), "登记时已取消的任务应立即取消新令牌");
    }

    #[test]
    fn arm_before_cancel_propagates_to_the_live_job() {
        let cancel = TaskCancel::default();
        let token = CancelToken::new();
        cancel.arm(token.clone());
        assert!(!token.is_cancelled());
        cancel.cancel();
        assert!(token.is_cancelled(), "取消应传播到当前在跑的 Job");
        assert!(cancel.bail_if_cancelled().is_err());
    }
}
