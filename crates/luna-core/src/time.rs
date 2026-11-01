//! 时间戳（零 IO：核心不读时钟，由上层注入）。

use serde::{Deserialize, Serialize};

/// 毫秒级时间戳（自 Unix epoch）。
///
/// 核心层**不读系统时钟**（保持纯净/可测）；`created_at` / `finished_at` 等由
/// 持有时钟的调度层在构造时戳入。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Timestamp(pub i64);

impl Timestamp {
    /// epoch 起点（占位默认值）。
    pub const ZERO: Self = Timestamp(0);

    /// 由毫秒数构造。
    #[must_use]
    pub fn from_millis(ms: i64) -> Self {
        Self(ms)
    }

    /// 毫秒数。
    #[must_use]
    pub fn as_millis(self) -> i64 {
        self.0
    }
}
