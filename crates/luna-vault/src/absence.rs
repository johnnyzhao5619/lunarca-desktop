//! 「不见了」不等于「删了」（ADR-148；同步算法护栏）。
//!
//! 一个文件此刻在磁盘上找不到，原因可能是用户删了它，也可能是：库所在的卷没挂上、库被换成了
//! 另一份（身份文件对不上）、iCloud 只留了占位、或者上层目录这一刻读不了。后几种若被当成删除，
//! 同步会把云端的副本也删掉——Obsidian / Dropbox 都有过这类事故。所以「判成删除」只有一条路：
//! 库根在、身份对得上、离它最近的现存祖先目录读得了、没有占位，而文件确实不在。其余一律
//! 「说不准」，调用方不得据此产生删除。
//!
//! 判定集中在这一个函数里：同步引擎与将来的扫描各写一份，迟早有一份漏判。

use std::path::Path;

/// 一个文件此刻不在的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Absence {
    /// 文件在（不是缺席）。
    Present,
    /// 确认被删了：可以据此产生删除。
    Deleted,
    /// 说不准：不得产生删除。
    Unknown(UnknownAbsence),
}

/// 说不准的几种情形。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnknownAbsence {
    /// 库根不在或不是目录（卷没挂上、库被挪走）。
    RootUnavailable,
    /// `.luna/vault-id` 缺失或与期望的不同（这是另一份库，或身份文件被删了）。
    IdentityMismatch,
    /// 离文件最近的现存祖先目录读不了（权限、网络盘断开）。
    ParentUnreadable,
    /// iCloud 只留了占位，文件还在云上。
    CloudPlaceholder,
    /// 路径形状不对（绝对路径、`..`、空）。
    InvalidPath,
}

/// 判定库相对路径 `rel`（`/` 分隔）的缺席原因。`expected_vault_id` 给出时要求 `.luna/vault-id`
/// 与之相同（同步绑定的那一份库）。
#[must_use]
pub fn classify_absence(root: &Path, rel: &str, expected_vault_id: Option<&str>) -> Absence {
    if rel.is_empty()
        || rel.starts_with('/')
        || rel
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Absence::Unknown(UnknownAbsence::InvalidPath);
    }
    if !root.is_dir() {
        return Absence::Unknown(UnknownAbsence::RootUnavailable);
    }
    if let Some(expected) = expected_vault_id {
        let found = crate::identity::read(&root.join(".luna"));
        let matches = found.is_some_and(|id| {
            crate::identity::VaultId::parse(expected).is_some_and(|want| want == id)
        });
        if !matches {
            return Absence::Unknown(UnknownAbsence::IdentityMismatch);
        }
    }
    let path = root.join(rel);
    if std::fs::symlink_metadata(&path).is_ok() {
        return Absence::Present;
    }
    if crate::icloud::placeholder_for(root, rel).is_file() {
        return Absence::Unknown(UnknownAbsence::CloudPlaceholder);
    }
    // 离它最近的现存祖先（整个目录被删时，父目录也不在）。
    let mut ancestor = path.parent();
    while let Some(dir) = ancestor {
        if dir.exists() {
            return if std::fs::read_dir(dir).is_ok() {
                Absence::Deleted
            } else {
                Absence::Unknown(UnknownAbsence::ParentUnreadable)
            };
        }
        if dir == root {
            break;
        }
        ancestor = dir.parent();
    }
    Absence::Unknown(UnknownAbsence::RootUnavailable)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".luna")).unwrap();
        let id = crate::identity::ensure(&dir.path().join(".luna")).unwrap();
        (dir, id.to_string())
    }

    #[test]
    fn only_an_observed_disappearance_counts_as_deleted() {
        let (dir, id) = vault();
        let root = dir.path();
        std::fs::create_dir_all(root.join("notes/deep")).unwrap();
        std::fs::write(root.join("notes/a.md"), "x").unwrap();
        assert_eq!(
            classify_absence(root, "notes/a.md", Some(&id)),
            Absence::Present
        );
        assert_eq!(
            classify_absence(root, "notes/gone.md", Some(&id)),
            Absence::Deleted
        );
        // 整个目录都没了：最近的现存祖先读得了，仍是删除。
        assert_eq!(
            classify_absence(root, "old/folder/b.md", Some(&id)),
            Absence::Deleted
        );
        // 不要求身份时同样成立。
        assert_eq!(
            classify_absence(root, "notes/gone.md", None),
            Absence::Deleted
        );
    }

    #[test]
    fn an_unmounted_or_swapped_vault_never_deletes() {
        let (dir, id) = vault();
        let root = dir.path();
        assert_eq!(
            classify_absence(&root.join("missing-volume"), "a.md", Some(&id)),
            Absence::Unknown(UnknownAbsence::RootUnavailable)
        );
        let other = uuid::Uuid::now_v7().to_string();
        assert_eq!(
            classify_absence(root, "a.md", Some(&other)),
            Absence::Unknown(UnknownAbsence::IdentityMismatch)
        );
        std::fs::remove_file(root.join(".luna").join(crate::identity::VAULT_ID_FILE)).unwrap();
        assert_eq!(
            classify_absence(root, "a.md", Some(&id)),
            Absence::Unknown(UnknownAbsence::IdentityMismatch)
        );
    }

    #[test]
    fn a_cloud_placeholder_is_not_a_deletion() {
        let (dir, id) = vault();
        let root = dir.path();
        std::fs::write(root.join(".a.md.icloud"), "").unwrap();
        assert_eq!(
            classify_absence(root, "a.md", Some(&id)),
            Absence::Unknown(UnknownAbsence::CloudPlaceholder)
        );
    }

    #[test]
    fn hostile_paths_are_refused() {
        let (dir, id) = vault();
        for rel in ["", "/etc/passwd", "../x.md", "a//b.md", "a/./b.md"] {
            assert_eq!(
                classify_absence(dir.path(), rel, Some(&id)),
                Absence::Unknown(UnknownAbsence::InvalidPath),
                "{rel}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_parent_is_not_a_deletion() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, id) = vault();
        let root = dir.path();
        let locked = root.join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let readable_anyway = std::fs::read_dir(&locked).is_ok(); // root 跑测试时权限不生效
        let got = classify_absence(root, "locked/a.md", Some(&id));
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        if !readable_anyway {
            assert_eq!(got, Absence::Unknown(UnknownAbsence::ParentUnreadable));
        }
    }
}
