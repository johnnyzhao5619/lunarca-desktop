//! 统一结果模型 `JobResult`（ResultSchema）。零 IO。事实源：TDD-01 §2.2。
//!
//! 关键统一：转写/转译/批量/二次处理全部产出 `Segments`，导出层（TDD-10）只面向
//! `Segments`/`Text`，无需为每个功能各写一套（DRY）。

use crate::chat::ChatMessage;
use crate::ids::{JobId, ModelId, ProviderId, SegmentId};
use crate::time::Timestamp;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// 词级时间戳（P2 预留；从 MVP 到 P1 缺省，回放/导出按行级）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WordTiming {
    /// 词文本。
    pub text: String,
    /// 起点（相对会话绝对时钟）。
    pub start: Duration,
    /// 止点。
    pub end: Duration,
}

/// 一个分段：转写/转译统一用"分段 + 时间戳"表达。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    /// 稳定 id（边车内部标识，不写入 md；TDD-06 §2.2）。
    pub id: SegmentId,
    /// 起点（绝对时间）。
    pub start: Duration,
    /// 止点（绝对时间）。
    pub end: Duration,
    /// 正文。
    pub text: String,
    /// 译文（转写+转译并存时）。
    pub translation: Option<String>,
    /// 说话人（diarization，P2 预留）。
    pub speaker: Option<String>,
    /// 置信度。
    pub confidence: Option<f32>,
    /// 词级时间戳（P2 预留，缺省 `None`）。
    pub words: Option<Vec<WordTiming>>,
}

/// 结果载荷。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultPayload {
    /// 有时间维度（转写/转译/批量/二次处理）。
    Segments(Vec<Segment>),
    /// 无时间维度（总结/优化/翻译）。
    Text(String),
    /// 对话回复。
    Chat(ChatMessage),
    /// 一批文本的向量（嵌入，ADR-027）。外层 `Vec` 与请求的 `texts` **严格一一对应**，
    /// 内层是该段的向量。
    ///
    /// 这是 ADR-022「此刻需要才引入」的一次兑现：批次 4 之前没有任何消费者需要"一批数值"
    /// 这种载荷形态，故此前只有三种；现在需要了才加，而不是预先留一个空位。
    ///
    /// **不进导出/回放管线**：那两条只面向 `Segments`/`Text`（TDD-10），向量是派生数据、
    /// 对用户不可读——把它塞进导出只会让「导出格式」多一个没人要的分支。
    Embeddings(Vec<Vec<f32>>),
}

/// 输出触顶时**是谁的上限先到**。
///
/// # 为什么要分归属而不是一个 bool
///
/// 三种触顶对用户是三件不同的事，能做的动作也不同：调用方自己定的额度可以由用户
/// 直接调大；上游的机器保护上限只能换模型；部署级绝对上限得找管理员。只报「被截断了」
/// 等于让所有人都去猜该动哪一个。
///
/// 命名刻意不叫 `Truncated*` 之外的短名：`luna-jobqueue` 的 `chat_session` 里已有一个
/// `truncated`，指的是**输入上下文被裁剪**——同一条链路上两个相反方向的「截断」共用名字，
/// 读的人一定会串。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TruncatedBy {
    /// 调用方自己给出的 `max_tokens` 到顶。
    Caller,
    /// 选中上游的机器保护上限到顶。
    Upstream,
    /// 部署级绝对输出上限到顶（不按账户、不按能力，纯防失控）。
    Absolute,
}

impl TruncatedBy {
    /// 全部取值（供枚举遍历，与 serde 口径同源）。
    pub const ALL: [Self; 3] = [Self::Caller, Self::Upstream, Self::Absolute];

    /// snake 键——**唯一事实源**：serde 契约、服务端终态帧、蒸馏笔记 frontmatter 三边共用。
    ///
    /// 存在的理由是这个字符串已经在两个地方各写过一遍（wire 解析与落盘），第三个消费者
    /// 出现时再抄一遍就必然漂移。
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Caller => "caller",
            Self::Upstream => "upstream",
            Self::Absolute => "absolute",
        }
    }

    /// 由 snake 键还原；**认不出即 `None`，绝不猜一个归属**——猜错会把「上游机器上限」
    /// 说成「你自己的额度设小了」，把用户引去改一个改了也没用的设置。
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|by| by.key() == key)
    }
}

/// 纯金额标量（最小货币单位，如分）；**币种在外层统一约束**（TDD-04 §2.2）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Money {
    /// 最小货币单位数量。
    pub minor_units: i64,
}

/// 一次任务的用量（计量 ≠ 计费，ADR-016）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    /// ASR 音频时长（秒）。
    pub audio_seconds: Option<f32>,
    /// 输入 token。
    pub tokens_in: Option<u32>,
    /// 输出 token。
    pub tokens_out: Option<u32>,
    /// 估算费用（本地模型为 0/None；估算非计费）。
    pub est_cost: Option<Money>,
}

/// 可观测耗时（排队/首字/总耗时）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timings {
    /// 排队耗时（ms）。
    pub queued_ms: u64,
    /// 首字延迟（ms，流式）。
    pub first_token_ms: Option<u64>,
    /// 总耗时（ms）。
    pub total_ms: u64,
}

/// 统一结果。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JobResult {
    /// 对应 Job。
    pub job_id: JobId,
    /// 执行 Provider。
    pub provider_id: ProviderId,
    /// 所用模型。
    pub model_id: Option<ModelId>,
    /// 结果载荷。
    pub payload: ResultPayload,
    /// 用量。
    pub usage: Usage,
    /// 耗时。
    pub timings: Timings,
    /// 输出触顶导致的提前收尾及其归属；`None` = 正文自然写完。
    ///
    /// **触顶是「完成 + 标注」，不是失败**：撞上限时把整个任务判成额度耗尽会把已经
    /// 生成好的正文一起丢掉，而用户的额度可能只用了四分之一。事实必须有地方落，
    /// 落盘标注与界面提示才有依据。
    ///
    /// `None` 是正当的领域取值（绝大多数任务本就没触顶），不是给旧数据留的过渡位。
    pub output_truncated: Option<TruncatedBy>,
    /// 完成时间（由调度层戳入）。
    pub finished_at: Timestamp,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_payload_round_trip() {
        let payload = ResultPayload::Segments(vec![Segment {
            id: SegmentId::new(),
            start: Duration::from_millis(1200),
            end: Duration::from_millis(3400),
            text: "大家好".into(),
            translation: Some("Hello everyone".into()),
            speaker: None,
            confidence: Some(0.92),
            words: None,
        }]);
        let json = serde_json::to_string(&payload).unwrap();
        let back: ResultPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(payload, back);
    }

    #[test]
    fn text_payload_tag_is_snake_case() {
        let json = serde_json::to_string(&ResultPayload::Text("hi".into())).unwrap();
        assert_eq!(json, "{\"text\":\"hi\"}");
    }
}
