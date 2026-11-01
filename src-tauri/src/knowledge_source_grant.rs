//! 录音/转写直接作为主题来源的范围授权回执（PRD-16-048/058 / TDD-06 §3.9.3）。
//!
//! 回执是**设备私有**的：落在应用数据目录，不进 Vault（库可能同步到别的设备，那台设备的
//! 用户并没有确认过）。每条回执绑定库身份、策略版本与**算力落点类别**（ADR-136 ②）——
//! 同一路径换了库、策略版本升级、或 AI 从本机换到云端 / 组织服务端之后，旧回执都不再生效，
//! 须重新确认一次：接收方变了，用户同意过的就不再是同一件事。缺回执即未授权：旧用户升级后
//! 不静默扩大来源范围。
//!
//! 「暂不」也记下来（同样绑库身份与落点）：选了「暂不」之后不再逐条录音地问。
//!
//! 版本区间：更早的文件格式（`schema` 小于当前）没有落点信息，一律视为「从未问过」并允许
//! 覆写——那是本应用自己写的旧格式，用户会在下一次录音时被重新问一次；更新的格式或读不出的
//! 文件一律当作未授权，且拒绝覆写，免得把一份看不懂的文件抹掉。

use serde::{Deserialize, Serialize};
use std::path::Path;

const FILE_NAME: &str = "knowledge-source-grants.json";
const FILE_SCHEMA: u32 = 2;

/// 录音/转写来源授权的策略版本。影响范围的说法变了（例如纳入新的来源类型）就升一版，
/// 已有回执随之失效、须重新确认。
pub(crate) const TRANSCRIPT_SOURCE_POLICY_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Grant {
    vault_identity: String,
    policy_version: u32,
    /// 同意时 AI 的执行落点类别（[`compute_class`]）。
    compute: String,
    granted_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Decline {
    vault_identity: String,
    compute: String,
    declined_at_ms: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantFile {
    schema: u32,
    grants: Vec<Grant>,
    declines: Vec<Decline>,
}

/// 只读版本号，判断是旧格式、当前格式还是更新的格式。
#[derive(Deserialize)]
struct SchemaProbe {
    schema: u32,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum GrantStoreError {
    #[error("grant store unreadable")]
    Unreadable,
    #[error("grant store write failed: {0}")]
    Write(String),
}

/// 这台设备对这个库、在这个落点下的状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TranscriptSourceState {
    Granted,
    Declined,
    /// 从未问过，或同意 / 暂不是在另一个落点下给的（须重新问）。
    Unasked,
}

/// 算力落点的**类别**：本机（含严格本地）一类，官方云一类，组织服务端一类。同意绑定的是
/// 「录音内容会交给谁处理」，同一类别里换个模型不必重问。
#[must_use]
pub(crate) fn compute_class(location: crate::cloud::ComputeLocation) -> &'static str {
    use crate::cloud::ComputeLocation as L;
    match location {
        L::StrictLocal | L::OnDevice => "device",
        L::OfficialCloud => "official_cloud",
        L::Organization => "organization",
    }
}

fn load(dir: &Path) -> Result<GrantFile, GrantStoreError> {
    match std::fs::read(dir.join(FILE_NAME)) {
        Ok(bytes) => {
            let probe: SchemaProbe =
                serde_json::from_slice(&bytes).map_err(|_| GrantStoreError::Unreadable)?;
            if probe.schema < FILE_SCHEMA {
                // 本应用写过的旧格式：没有落点信息，按「从未问过」处理。
                return Ok(GrantFile::default());
            }
            if probe.schema != FILE_SCHEMA {
                return Err(GrantStoreError::Unreadable);
            }
            serde_json::from_slice(&bytes).map_err(|_| GrantStoreError::Unreadable)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(GrantFile::default()),
        Err(_) => Err(GrantStoreError::Unreadable),
    }
}

fn save(dir: &Path, mut file: GrantFile) -> Result<(), GrantStoreError> {
    file.schema = FILE_SCHEMA;
    std::fs::create_dir_all(dir).map_err(|error| GrantStoreError::Write(error.to_string()))?;
    let bytes = serde_json::to_vec_pretty(&file)
        .map_err(|error| GrantStoreError::Write(error.to_string()))?;
    luna_vault::atomic::write_atomic(&dir.join(FILE_NAME), &bytes)
        .map_err(|error| GrantStoreError::Write(error.to_string()))
}

/// 这台设备对这个库、在当前落点类别下的授权状态。文件读不出时按「暂不」处理：不授权，
/// 也不在录音旁反复询问一件记不下来的事（写入会被拒绝，用户点了「允许」只会得到一个错误）。
pub(crate) fn transcript_source_state(
    dir: &Path,
    vault_identity: &str,
    compute: &str,
) -> TranscriptSourceState {
    let Ok(file) = load(dir) else {
        return TranscriptSourceState::Declined;
    };
    if file.grants.iter().any(|grant| {
        grant.vault_identity == vault_identity
            && grant.policy_version == TRANSCRIPT_SOURCE_POLICY_VERSION
            && grant.compute == compute
    }) {
        return TranscriptSourceState::Granted;
    }
    if file
        .declines
        .iter()
        .any(|decline| decline.vault_identity == vault_identity && decline.compute == compute)
    {
        return TranscriptSourceState::Declined;
    }
    TranscriptSourceState::Unasked
}

/// 记下同意、撤回或「暂不」。三者对这个库互斥：写一条就清掉该库的其余记录（含其他落点
/// 与旧策略版本的）。文件读不出时拒绝写。
pub(crate) fn set_transcript_source_state(
    dir: &Path,
    vault_identity: &str,
    compute: &str,
    state: TranscriptSourceState,
    now_ms: i64,
) -> Result<(), GrantStoreError> {
    let mut file = load(dir)?;
    file.grants
        .retain(|grant| grant.vault_identity != vault_identity);
    file.declines
        .retain(|decline| decline.vault_identity != vault_identity);
    match state {
        TranscriptSourceState::Granted => file.grants.push(Grant {
            vault_identity: vault_identity.to_owned(),
            policy_version: TRANSCRIPT_SOURCE_POLICY_VERSION,
            compute: compute.to_owned(),
            granted_at_ms: now_ms,
        }),
        TranscriptSourceState::Declined => file.declines.push(Decline {
            vault_identity: vault_identity.to_owned(),
            compute: compute.to_owned(),
            declined_at_ms: now_ms,
        }),
        TranscriptSourceState::Unasked => {}
    }
    save(dir, file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use TranscriptSourceState as S;

    fn transcripts_granted(dir: &Path, vault_identity: &str, compute: &str) -> bool {
        transcript_source_state(dir, vault_identity, compute) == S::Granted
    }

    #[test]
    fn grant_is_per_vault_per_compute_class_and_revocable() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            transcript_source_state(dir.path(), "vault-a", "device"),
            S::Unasked,
            "缺回执即未授权"
        );
        set_transcript_source_state(dir.path(), "vault-a", "device", S::Granted, 1).unwrap();
        assert!(transcripts_granted(dir.path(), "vault-a", "device"));
        assert!(
            !transcripts_granted(dir.path(), "vault-b", "device"),
            "不跨库沿用"
        );
        assert_eq!(
            transcript_source_state(dir.path(), "vault-a", "official_cloud"),
            S::Unasked,
            "换到云端须重新确认：接收方变了"
        );
        set_transcript_source_state(dir.path(), "vault-a", "device", S::Unasked, 2).unwrap();
        assert!(!transcripts_granted(dir.path(), "vault-a", "device"));
    }

    #[test]
    fn a_decline_silences_the_prompt_for_that_compute_class_only() {
        let dir = tempfile::tempdir().unwrap();
        set_transcript_source_state(dir.path(), "vault-a", "device", S::Declined, 1).unwrap();
        assert_eq!(
            transcript_source_state(dir.path(), "vault-a", "device"),
            S::Declined
        );
        assert!(!transcripts_granted(dir.path(), "vault-a", "device"));
        assert_eq!(
            transcript_source_state(dir.path(), "vault-a", "organization"),
            S::Unasked
        );
        // 之后改主意同意：「暂不」随之清掉。
        set_transcript_source_state(dir.path(), "vault-a", "device", S::Granted, 2).unwrap();
        assert_eq!(
            transcript_source_state(dir.path(), "vault-a", "device"),
            S::Granted
        );
    }

    #[test]
    fn older_formats_read_as_unasked_newer_or_unreadable_files_are_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        // 旧格式（schema 1，无落点）：视为从未问过，可被覆写。
        std::fs::write(
            dir.path().join(FILE_NAME),
            br#"{"schema":1,"grants":[{"vault_identity":"vault-a","policy_version":1,"granted_at_ms":1}]}"#,
        )
        .unwrap();
        assert_eq!(
            transcript_source_state(dir.path(), "vault-a", "device"),
            S::Unasked
        );
        set_transcript_source_state(dir.path(), "vault-a", "device", S::Granted, 2).unwrap();
        assert!(transcripts_granted(dir.path(), "vault-a", "device"));
        // 更新的格式或看不懂的文件：不授权，也不覆写。
        for bytes in [
            &br#"{"schema":3,"grants":[],"declines":[]}"#[..],
            b"{ not json",
        ] {
            std::fs::write(dir.path().join(FILE_NAME), bytes).unwrap();
            assert!(!transcripts_granted(dir.path(), "vault-a", "device"));
            assert!(
                set_transcript_source_state(dir.path(), "vault-a", "device", S::Granted, 3)
                    .is_err(),
                "看不懂的文件不覆写"
            );
            assert_eq!(std::fs::read(dir.path().join(FILE_NAME)).unwrap(), bytes);
        }
    }

    #[test]
    fn a_newer_policy_version_needs_a_new_confirmation() {
        let dir = tempfile::tempdir().unwrap();
        save(
            dir.path(),
            GrantFile {
                schema: FILE_SCHEMA,
                grants: vec![Grant {
                    vault_identity: "vault-a".into(),
                    policy_version: TRANSCRIPT_SOURCE_POLICY_VERSION + 1,
                    compute: "device".into(),
                    granted_at_ms: 1,
                }],
                declines: Vec::new(),
            },
        )
        .unwrap();
        assert!(!transcripts_granted(dir.path(), "vault-a", "device"));
    }
}
