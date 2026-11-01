//! 安全封禁（ADR-159 ⑦，PRD-18-012）：主题目录与模板目录共用的那一段。
//!
//! 装过这个 id 的客户端在本机**停用**它，不删文件；原因原样显示给用户。下一版目录里不再有这一条即
//! 解除封禁，客户端自动恢复。撤回（下架）与封禁是两件事：撤回 = 下一版不再列出这一条，已装的照常用
//! （PRD-13-035）；封禁 = 列进 `revoked`，已装的也停用。两类目录的版本内容都是 `{ entries, revoked }`，
//! 签进同一份目录信封。

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::ContentId;

/// 封禁原因的最大长度（字符）：它原样显示在用户的「已停用」那一行，要一句话说完。
pub const REVOCATION_REASON_MAX_CHARS: usize = 200;

/// 一条安全封禁。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Revocation {
    pub id: ContentId,
    pub reason: String,
}

/// 封禁列表的问题清单（空 = 合格）：要写原因（一句话、不超过 [`REVOCATION_REASON_MAX_CHARS`]），同一个
/// id 不能封两次，也不能一边封禁一边上架（`listed` = 本版本 `entries` 里的 id）。`noun` 是条目在问题
/// 里的叫法（`theme`、`template pack`）。
pub(crate) fn revocation_problems(
    listed: &HashSet<&str>,
    revoked: &[Revocation],
    noun: &str,
) -> Vec<String> {
    let mut problems = Vec::new();
    let mut seen = HashSet::new();
    for (index, revocation) in revoked.iter().enumerate() {
        let id = revocation.id.as_str();
        let reason = revocation.reason.trim();
        if reason.is_empty() || reason.chars().count() > REVOCATION_REASON_MAX_CHARS {
            problems.push(format!(
                "revoked #{index}: reason must be 1-{REVOCATION_REASON_MAX_CHARS} characters (it is shown to users)"
            ));
        }
        if listed.contains(id) {
            problems.push(format!(
                "revoked #{index}: {noun} id {id} is also listed in entries; remove it from entries to ban it"
            ));
        }
        if !seen.insert(id) {
            problems.push(format!(
                "revoked #{index}: {noun} id {id} is revoked more than once"
            ));
        }
    }
    problems
}
