//! 低风险整理计划的「影子资格」计数（ADR-136 ③ / PRD-16-059）。
//!
//! 自动写入在人工评测门槛通过前保持关闭；在那之前，Rust 判定为低风险的计划（每一条都是只引用
//! 正式笔记的新增）照常等用户确认，同时记下用户对它们的处置：被看到、被接受、被忽略、被否决、
//! 被撤销。这些比例是日后讨论「能不能先自动写这一类」的线上证据——它只是证据，不是放行开关。
//!
//! 只存**计数**与去重用的操作 ID / 结果指纹（随机 UUID 与哈希，不含任何正文、路径或标题），
//! 落在应用数据目录、绑库身份；不进 Vault（库可能同步到别处）、不联网。读不出或格式更新的文件
//! 一律不记也不覆写，计数宁可少记，不抹掉看不懂的文件。

use serde::{Deserialize, Serialize};
use std::path::Path;

const FILE_NAME: &str = "knowledge-shadow-tally.json";
const FILE_SCHEMA: u32 = 1;
/// 去重集合的上限：最久的先忘。计数本身不受影响，只是极久之前的同一计划可能被重复计一次。
const SEEN_LIMIT: usize = 500;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShadowEvent {
    /// 在「待你看」里被列出。
    Shown,
    Accepted,
    Ignored,
    Rejected,
    /// 接受之后又撤销了那次写入。
    Undone,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ShadowTally {
    pub shown: u32,
    pub accepted: u32,
    pub ignored: u32,
    pub rejected: u32,
    pub undone: u32,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct VaultTally {
    vault_identity: String,
    tally: ShadowTally,
    /// 已计过「被看到」与「被处置」的操作 ID（每份计划各只计一次）。
    shown_ops: Vec<String>,
    decided_ops: Vec<String>,
    /// 被接受的计划的结果指纹：撤销只凭写入前后指纹，靠它认出撤的是不是一份低风险计划。
    accepted_results: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TallyFile {
    schema: u32,
    vaults: Vec<VaultTally>,
}

fn load(dir: &Path) -> Option<TallyFile> {
    match std::fs::read(dir.join(FILE_NAME)) {
        Ok(bytes) => serde_json::from_slice::<TallyFile>(&bytes)
            .ok()
            .filter(|file| file.schema == FILE_SCHEMA),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(TallyFile {
            schema: FILE_SCHEMA,
            vaults: Vec::new(),
        }),
        Err(_) => None,
    }
}

fn remember(list: &mut Vec<String>, value: &str) -> bool {
    if list.iter().any(|known| known == value) {
        return false;
    }
    list.push(value.to_owned());
    if list.len() > SEEN_LIMIT {
        list.remove(0);
    }
    true
}

/// 记一次处置。`operation_id` 去重「看到 / 处置」；`result_hash` 用于接受与撤销的对应。
/// 同一份计划的同一类事件只计一次；读写失败安静放弃（这是证据，不是功能）。
pub(crate) fn record(
    dir: &Path,
    vault_identity: &str,
    event: ShadowEvent,
    operation_id: &str,
    result_hash: &str,
) {
    let Some(mut file) = load(dir) else {
        return;
    };
    let index = match file
        .vaults
        .iter()
        .position(|entry| entry.vault_identity == vault_identity)
    {
        Some(index) => index,
        None => {
            file.vaults.push(VaultTally {
                vault_identity: vault_identity.to_owned(),
                ..VaultTally::default()
            });
            file.vaults.len() - 1
        }
    };
    let entry = &mut file.vaults[index];
    let counted = match event {
        ShadowEvent::Shown => remember(&mut entry.shown_ops, operation_id),
        ShadowEvent::Accepted | ShadowEvent::Ignored | ShadowEvent::Rejected => {
            let first = remember(&mut entry.decided_ops, operation_id);
            if first && event == ShadowEvent::Accepted {
                remember(&mut entry.accepted_results, result_hash);
            }
            first
        }
        ShadowEvent::Undone => {
            let known = entry
                .accepted_results
                .iter()
                .position(|hash| hash == result_hash);
            if let Some(position) = known {
                entry.accepted_results.remove(position);
            }
            known.is_some()
        }
    };
    if !counted {
        return;
    }
    let tally = &mut entry.tally;
    let slot = match event {
        ShadowEvent::Shown => &mut tally.shown,
        ShadowEvent::Accepted => &mut tally.accepted,
        ShadowEvent::Ignored => &mut tally.ignored,
        ShadowEvent::Rejected => &mut tally.rejected,
        ShadowEvent::Undone => &mut tally.undone,
    };
    *slot = slot.saturating_add(1);
    let Ok(bytes) = serde_json::to_vec_pretty(&file) else {
        return;
    };
    if std::fs::create_dir_all(dir).is_ok() {
        if let Err(error) = luna_vault::atomic::write_atomic(&dir.join(FILE_NAME), &bytes) {
            eprintln!("[knowledge] 影子资格计数写入失败：{error}");
        }
    }
}

/// 这个库至今的计数（读不出即全零）。
#[cfg(test)]
pub(crate) fn tally(dir: &Path, vault_identity: &str) -> ShadowTally {
    load(dir)
        .and_then(|file| {
            file.vaults
                .into_iter()
                .find(|entry| entry.vault_identity == vault_identity)
        })
        .map(|entry| entry.tally)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ShadowEvent as E;

    #[test]
    fn each_plan_counts_once_per_kind_and_undo_matches_an_accepted_result() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        record(d, "vault-a", E::Shown, "op-1", "r1");
        record(d, "vault-a", E::Shown, "op-1", "r1");
        record(d, "vault-a", E::Shown, "op-2", "r2");
        record(d, "vault-a", E::Accepted, "op-1", "r1");
        record(d, "vault-a", E::Accepted, "op-1", "r1");
        record(d, "vault-a", E::Rejected, "op-2", "r2");
        // 撤销一份没被记为接受的结果：不计。
        record(d, "vault-a", E::Undone, "op-x", "r-unknown");
        record(d, "vault-a", E::Undone, "op-1", "r1");
        record(d, "vault-a", E::Undone, "op-1", "r1");
        assert_eq!(
            tally(d, "vault-a"),
            ShadowTally {
                shown: 2,
                accepted: 1,
                ignored: 0,
                rejected: 1,
                undone: 1,
            }
        );
        assert_eq!(tally(d, "vault-b"), ShadowTally::default(), "不跨库");
    }

    #[test]
    fn unreadable_or_newer_files_are_left_untouched() {
        let dir = tempfile::tempdir().unwrap();
        for bytes in [&br#"{"schema":2,"vaults":[]}"#[..], b"{ not json"] {
            std::fs::write(dir.path().join(FILE_NAME), bytes).unwrap();
            record(dir.path(), "vault-a", E::Shown, "op-1", "r1");
            assert_eq!(std::fs::read(dir.path().join(FILE_NAME)).unwrap(), bytes);
        }
    }

    #[test]
    fn the_file_holds_no_content_only_counts_and_opaque_ids() {
        let dir = tempfile::tempdir().unwrap();
        record(
            dir.path(),
            "vault-a",
            E::Shown,
            "019d1a08-7cf3",
            "sha256:abc",
        );
        let text = std::fs::read_to_string(dir.path().join(FILE_NAME)).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        let entry = &value["vaults"][0];
        let mut keys: Vec<&str> = entry
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "accepted_results",
                "decided_ops",
                "shown_ops",
                "tally",
                "vault_identity"
            ]
        );
    }
}
