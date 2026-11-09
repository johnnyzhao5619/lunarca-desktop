//! 一次提交（`POST /sync/v1/vaults/{id}/commits`，TDD-16 §9）的条目、请求体与逐条结果。
//!
//! 条目密文绑定它将成为的 rev（[`next_rev`]：基线 + 1，新文件为 1）——拿旧版本的密文冒充新版本
//! 打不开。桌面（`luna-sync` 引擎 → `luna-cloud` 发出）与网页版（`luna-web` → Worker 原样发出）
//! 用的是这同一份类型与序列化：请求体只由 [`body_json`] 写出，两端逐字节相同。

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use serde::{Deserialize, Serialize, Serializer};
use uuid::Uuid;

use crate::crypto::VaultKeys;
use crate::manifest::{seal_entry, PreparedFile};

/// 以 `base_rev` 为基线提交成功后，这个文件的新 rev（新文件没有基线，即 1）。
#[must_use]
pub fn next_rev(base_rev: Option<i64>) -> i64 {
    base_rev.unwrap_or(0) + 1
}

/// 提交里的一个文件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommitItem {
    pub file_id: Uuid,
    pub base_rev: Option<i64>,
    pub change: CommitChange,
}

/// 一个条目的改动：写入新版本，或墓碑。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CommitChange {
    Put {
        #[serde(serialize_with = "base64_bytes")]
        entry_ct: Vec<u8>,
        size_padded: i64,
        chunk_ids: Vec<String>,
    },
    Delete,
}

fn base64_bytes<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&STANDARD.encode(bytes))
}

/// 写入一个版本：条目封在 [`next_rev`] 上。
#[must_use]
pub fn put(
    keys: &VaultKeys,
    file_id: Uuid,
    base_rev: Option<i64>,
    file: &PreparedFile,
) -> CommitItem {
    CommitItem {
        file_id,
        base_rev,
        change: CommitChange::Put {
            entry_ct: seal_entry(keys, file_id, next_rev(base_rev), &file.entry),
            size_padded: file.size_padded,
            chunk_ids: file.entry.chunks.clone(),
        },
    }
}

/// 墓碑（ADR-148 ⑤：带当前 rev 提交）。
#[must_use]
pub fn delete(file_id: Uuid, base_rev: i64) -> CommitItem {
    CommitItem {
        file_id,
        base_rev: Some(base_rev),
        change: CommitChange::Delete,
    }
}

/// 一条的结果：提交了；基线过期（带服务端此刻的 rev，没有这个文件的头为 `None`）；引用了服务端还没有的
/// 块；超过单文件上限（ADR-156 ④）。一条失败不影响同批其余条目。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum CommitResult {
    Committed {
        file_id: Uuid,
        rev: i64,
        seq: i64,
    },
    StaleBase {
        file_id: Uuid,
        current_rev: Option<i64>,
    },
    MissingBlobs {
        file_id: Uuid,
        chunk_ids: Vec<String>,
    },
    TooLarge {
        file_id: Uuid,
        max_bytes: i64,
    },
}

/// 响应体：库此刻的序号与逐条结果（与请求同序）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitReply {
    pub head_seq: i64,
    pub results: Vec<CommitResult>,
}

/// 请求体：`{"items":[…]}`。条目密文是标准 base64（带填充）。
#[must_use]
pub fn body_json(items: &[CommitItem]) -> String {
    #[derive(Serialize)]
    struct Body<'a> {
        items: &'a [CommitItem],
    }
    serde_json::to_string(&Body { items }).expect("commit items always serialize")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{Cmk, KeyRing};
    use crate::manifest::{open_entry, prepare};

    fn ring() -> KeyRing {
        KeyRing::new(Uuid::nil(), &[Cmk::from_bytes(1, [9; 32])]).unwrap()
    }

    #[test]
    fn a_put_seals_its_entry_on_the_rev_it_will_become() {
        let ring = ring();
        let file_id = Uuid::now_v7();
        let file = prepare(ring.current(), "甲.md", b"hello", Some(5));
        let item = put(ring.current(), file_id, Some(3), &file);
        let CommitChange::Put { entry_ct, .. } = &item.change else {
            panic!("put");
        };
        assert_eq!(open_entry(&ring, file_id, 4, entry_ct).unwrap(), file.entry);
        assert!(open_entry(&ring, file_id, 3, entry_ct).is_err());
        assert_eq!(next_rev(None), 1);
    }

    #[test]
    fn the_reply_parses_every_outcome() {
        let id = "00000000-0000-0000-0000-000000000001";
        let reply: CommitReply = serde_json::from_str(&format!(
            r#"{{"head_seq":6,"results":[{{"outcome":"committed","file_id":"{id}","rev":3,"seq":6}},
            {{"outcome":"stale_base","file_id":"{id}","current_rev":null}},
            {{"outcome":"missing_blobs","file_id":"{id}","chunk_ids":["b"]}},
            {{"outcome":"too_large","file_id":"{id}","max_bytes":9}}]}}"#
        ))
        .unwrap();
        let file_id = Uuid::parse_str(id).unwrap();
        assert_eq!(
            reply.results,
            [
                CommitResult::Committed {
                    file_id,
                    rev: 3,
                    seq: 6
                },
                CommitResult::StaleBase {
                    file_id,
                    current_rev: None
                },
                CommitResult::MissingBlobs {
                    file_id,
                    chunk_ids: vec!["b".into()]
                },
                CommitResult::TooLarge {
                    file_id,
                    max_bytes: 9
                },
            ]
        );
    }

    #[test]
    fn the_body_has_the_wire_shape() {
        let file_id = Uuid::nil();
        let item = CommitItem {
            file_id,
            base_rev: None,
            change: CommitChange::Put {
                entry_ct: vec![1, 2, 3],
                size_padded: 4096,
                chunk_ids: vec!["ab".into()],
            },
        };
        assert_eq!(
            body_json(&[item, delete(file_id, 7)]),
            concat!(
                r#"{"items":[{"file_id":"00000000-0000-0000-0000-000000000000","base_rev":null,"#,
                r#""change":{"kind":"put","entry_ct":"AQID","size_padded":4096,"chunk_ids":["ab"]}},"#,
                r#"{"file_id":"00000000-0000-0000-0000-000000000000","base_rev":7,"change":{"kind":"delete"}}]}"#
            )
        );
    }
}
