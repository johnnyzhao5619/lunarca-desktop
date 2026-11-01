//! 极简 SSE（Server-Sent Events）行解码器。
//!
//! 只做"把字节流切成一个个事件"这一件事（纯逻辑、可单测）；协议特定的载荷解析在各
//! Provider（OpenAI 兼容 delta 在 [`crate::llm_http`]，Lunarca 原生帧在
//! [`crate::cloud_llm`]）。
//!
//! **`event:` 名必须带出来。** OpenAI 兼容流不发它（一律匿名 `data:`），而 Lunarca
//! 原生推理面用的是具名事件——其中 `delta` 与 `thinking` 的载荷**形状完全相同**
//! （都是 `{"text":…}`）。丢掉事件名就只能靠形状猜，两者必然混为一谈，后果是模型的
//! 推理过程被当作正文写进结果（`llm_http` 头注释里那条 Qwen3 thinking 的老账）。
//!
//! 只此一份解码器：为具名事件另写一个，是「两处各写一遍必然分叉」的标准形态。

/// 一条 SSE 负载。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SsePayload {
    /// 一个事件。`event` = `event:` 字段（匿名流为 `None`），`json` = `data:` 的文本部分。
    Data {
        /// 事件名；OpenAI 兼容流不发 `event:`，故为 `None`。
        event: Option<String>,
        /// `data:` 后的文本（通常是 JSON）。
        json: String,
    },
    /// `data: [DONE]` 终止标记（OpenAI 兼容流的收尾约定）。
    Done,
}

impl SsePayload {
    /// 构造匿名事件（测试与 OpenAI 兼容流用）。
    #[must_use]
    pub fn data(json: impl Into<String>) -> Self {
        Self::Data {
            event: None,
            json: json.into(),
        }
    }
}

/// 累积字节、按事件产出负载（跨 chunk 的半行会被缓冲到下次）。
#[derive(Default)]
pub struct SseDecoder {
    buf: String,
    /// 当前事件已读到的 `event:` 名；产出一条 `data:` 后清空（SSE 的字段作用域是
    /// 「到下一个 `data:` 为止」，不是整条流）。
    pending_event: Option<String>,
}

impl SseDecoder {
    /// 新建空解码器。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 喂入一段文本，返回本次得到的完整负载（不含半行）。
    pub fn push(&mut self, chunk: &str) -> Vec<SsePayload> {
        self.buf.push_str(chunk);
        let mut out = Vec::new();
        while let Some(pos) = self.buf.find('\n') {
            let line = self.buf[..pos].trim_end_matches('\r').trim().to_string();
            self.buf.drain(..=pos);
            if line.is_empty() {
                // 事件分隔空行：**同时清掉尚未被 `data:` 消费的事件名**。
                // 不清的话，一个只有 `event:` 没有 `data:` 的事件（如心跳）会把自己的名字
                // 借给下一条真载荷。
                self.pending_event = None;
                continue;
            }
            if let Some(rest) = line.strip_prefix("event:") {
                self.pending_event = Some(rest.trim().to_owned());
                continue;
            }
            if let Some(rest) = line.strip_prefix("data:") {
                let rest = rest.trim();
                if rest == "[DONE]" {
                    out.push(SsePayload::Done);
                } else {
                    out.push(SsePayload::Data {
                        event: self.pending_event.take(),
                        json: rest.to_string(),
                    });
                }
            }
            // 其它 SSE 字段（`id:` / `retry:` / `:` 注释）忽略。
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_complete_lines() {
        let mut d = SseDecoder::new();
        let out = d.push("data: {\"a\":1}\n\ndata: [DONE]\n\n");
        assert_eq!(out, vec![SsePayload::data("{\"a\":1}"), SsePayload::Done]);
    }

    #[test]
    fn buffers_partial_line_across_chunks() {
        let mut d = SseDecoder::new();
        assert!(d.push("data: {\"par").is_empty()); // 半行，缓冲
        let out = d.push("tial\":true}\n");
        assert_eq!(out, vec![SsePayload::data("{\"partial\":true}")]);
    }

    #[test]
    fn ignores_comments_and_blank_lines() {
        let mut d = SseDecoder::new();
        let out = d.push(": comment\nid: 7\n\ndata: x\n");
        assert_eq!(out, vec![SsePayload::data("x")]);
    }

    /// 具名事件必须带着名字出来——`delta` 与 `thinking` 的载荷形状完全相同，
    /// 靠形状分不开。分不开的后果是模型的推理过程被当成正文写进结果。
    #[test]
    fn keeps_the_event_name() {
        let mut d = SseDecoder::new();
        let out = d.push(
            "event: delta\ndata: {\"text\":\"你\"}\n\nevent: thinking\ndata: {\"text\":\"嗯\"}\n\n",
        );
        assert_eq!(
            out,
            vec![
                SsePayload::Data {
                    event: Some("delta".into()),
                    json: "{\"text\":\"你\"}".into()
                },
                SsePayload::Data {
                    event: Some("thinking".into()),
                    json: "{\"text\":\"嗯\"}".into()
                },
            ]
        );
    }

    /// 事件名跨 chunk 到达时同样要接上（`event:` 与它的 `data:` 可能被切在两段里）。
    #[test]
    fn event_name_survives_a_chunk_boundary() {
        let mut d = SseDecoder::new();
        assert!(d.push("event: acce").is_empty());
        let out = d.push("pted\ndata: {\"job_id\":\"x\"}\n");
        assert_eq!(
            out,
            vec![SsePayload::Data {
                event: Some("accepted".into()),
                json: "{\"job_id\":\"x\"}".into()
            }]
        );
    }

    /// 只有 `event:` 而没有 `data:` 的事件（心跳）**不得**把名字借给下一条。
    ///
    /// 借出去的表现是：一次 keep-alive 之后，紧跟的那条 `delta` 被当成上一个事件名
    /// 处理——正文可能因此被丢弃或被当成错误帧。
    #[test]
    fn a_nameless_payload_does_not_inherit_a_stale_event_name() {
        let mut d = SseDecoder::new();
        let out = d.push("event: ping\n\ndata: {\"text\":\"正文\"}\n");
        assert_eq!(out, vec![SsePayload::data("{\"text\":\"正文\"}")]);
    }
}
