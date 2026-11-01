//! 稳定标识符（零 IO）。
//!
//! `JobId` / `SegmentId` / `StreamId` 用 **UUIDv7**（时间有序，利于队列排序与
//! 持久化恢复的稳定排序，TDD-01 §2.1）；`ProviderId` / `ModelId` / `PresetId`
//! 是稳定字符串 id（如 `"local-whisper"` / `"whisper-large-v3-turbo"`）。

use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

macro_rules! uuid_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(
            Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        pub struct $name(Uuid);

        impl $name {
            /// 生成一个新的、时间有序的 UUIDv7。
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }

            /// 底层 UUID。
            #[must_use]
            pub fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }
    };
}

uuid_id!(JobId, "一次 AI 任务的稳定标识（UUIDv7，时间有序）。");
uuid_id!(
    SegmentId,
    "分段的稳定标识（UUIDv7）；边车内部标识，不写入 md（TDD-06 §2.2）。"
);
uuid_id!(StreamId, "流式采集会话的稳定标识（UUIDv7）。");

macro_rules! string_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        pub struct $name(String);

        impl $name {
            /// 由任意可转为 `String` 的值构造。
            #[must_use]
            pub fn new(s: impl Into<String>) -> Self {
                Self(s.into())
            }

            /// 底层字符串切片。
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_owned())
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

string_id!(
    ProviderId,
    "Provider 稳定 id（如 \"local-whisper\" / \"local-llm\"）。"
);
string_id!(
    ModelId,
    "模型稳定 id（如 \"whisper-large-v3-turbo\" / \"qwen3:4b\"）。"
);
string_id!(
    PresetId,
    "提示词预设 / 模板 id（PromptPreset，TDD-01 §2.5）。"
);
string_id!(
    UploadId,
    "计算服务侧一次音频上传的 id（远程批式转写，ADR-036 ①）。是 String 而非 UUID ——      这个 id 由服务端生成、格式归服务端所有；钉成 UUID 等于把某一版实现的选择写进客户端     领域模型，私有化部署方换一套 id 方案时客户端会在解析阶段失败，而那条失败信息与     『上传坏了』毫无区别。"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_id_is_v7_and_distinct() {
        let a = JobId::new();
        let b = JobId::new();
        assert_ne!(a, b);
        assert_eq!(a.as_uuid().get_version_num(), 7);
    }

    #[test]
    fn job_id_serde_round_trip() {
        let a = JobId::new();
        let json = serde_json::to_string(&a).unwrap();
        let back: JobId = serde_json::from_str(&json).unwrap();
        assert_eq!(a, back);
    }

    #[test]
    fn string_id_round_trips_and_displays() {
        let p = ProviderId::from("local-whisper");
        assert_eq!(p.as_str(), "local-whisper");
        assert_eq!(p.to_string(), "local-whisper");
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(json, "\"local-whisper\"");
        let back: ProviderId = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }
}
