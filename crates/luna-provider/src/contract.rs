//! 契约检查器：对**任意** `AIProvider` 的事件流校验流式不变量（TDD-01 §8）。
//!
//! 新增 Provider（whisper / 随包引擎 / openai-compat …）必须通过同一套不变量：
//! ① `committed` 段**单调**（起点非递减、发出即不回退）；② 终态**唯一且为最后一个事件**
//! （`Finished`/`Failed`/`Cancelled` 之后无更多事件）。

use futures::StreamExt;
use luna_core::{JobEvent, JobEventStream, PartialPayload, Segment};

/// 终态类别。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalKind {
    /// 正常完成。
    Finished,
    /// 失败。
    Failed,
    /// 已取消。
    Cancelled,
}

/// 驱动流后的汇总（供测试断言）。
#[derive(Default)]
pub struct StreamSummary {
    /// 事件总数。
    pub events: usize,
    /// 累计已定稿段（按到达顺序）。
    pub committed: Vec<Segment>,
    /// 首个终态（若有）。
    pub terminal: Option<TerminalKind>,
    /// 是否在终态后还收到事件（违反不变量②）。
    pub saw_event_after_terminal: bool,
    /// committed 段起点是否始终非递减（不变量①）。
    pub committed_monotonic: bool,
}

/// 驱动整条流到结束，收集并校验不变量。
pub async fn check_invariants(mut stream: JobEventStream) -> StreamSummary {
    let mut summary = StreamSummary {
        committed_monotonic: true,
        ..Default::default()
    };

    while let Some(ev) = stream.next().await {
        if summary.terminal.is_some() {
            summary.saw_event_after_terminal = true;
        }
        match ev {
            JobEvent::Partial(PartialPayload::Asr {
                committed_delta, ..
            }) => {
                for seg in committed_delta {
                    if let Some(last) = summary.committed.last() {
                        if seg.start < last.start {
                            summary.committed_monotonic = false;
                        }
                    }
                    summary.committed.push(seg);
                }
            }
            JobEvent::Finished(_) => {
                summary.terminal = summary.terminal.or(Some(TerminalKind::Finished));
            }
            JobEvent::Failed(_) => {
                summary.terminal = summary.terminal.or(Some(TerminalKind::Failed));
            }
            JobEvent::Cancelled => {
                summary.terminal = summary.terminal.or(Some(TerminalKind::Cancelled));
            }
            _ => {}
        }
        summary.events += 1;
    }

    summary
}
