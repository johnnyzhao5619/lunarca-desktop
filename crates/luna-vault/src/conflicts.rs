//! 同步盘与 git 留下的冲突副本（TDD-06 §4.1「同步盘痕迹」①；笔记库对标轮 V3b）。
//!
//! 两台设备同时改了同一篇，同步盘不会合并，而是多出一份「副本」；git 合并冲突则把两边的
//! 内容连同 `<<<<<<<` 标记一起写进正文。用户常常根本没发现——副本静静躺在文件夹里，
//! 搜索时出现两篇几乎一样的笔记。
//!
//! 这里只**认出来**：确定性的按文件名（Syncthing `.sync-conflict-`、Dropbox「conflicted copy」/
//! 「冲突副本」）与正文标记（git）；iCloud 的「名字 2.md」可能就是用户自己起的名字，只在
//! 「同目录有原名、两份修改时间相近、内容高度相似」三条同时成立时才算。**不自动合并、不自动
//! 删除**：由用户对比之后选保留哪份（[`Vault::resolve_conflict_copy`]）。

use crate::{DeleteMode, Result, Vault};
use std::path::Path;

/// 冲突副本从哪来。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictKind {
    Syncthing,
    Dropbox,
    ICloud,
    /// 正文里有未解决的 git 合并冲突标记（不是另一份文件）。
    GitMarkers,
}

/// 一份冲突副本。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConflictCopy {
    pub path: String,
    pub kind: ConflictKind,
    /// 它是哪一篇的副本（同目录的原文路径）；git 冲突标记在原文自己身上，为 `None`。
    pub original: Option<String>,
}

/// iCloud 副本与原文的修改时间相差不超过这么久才算「相近」（毫秒）。
const ICLOUD_MTIME_WINDOW_MS: i64 = 24 * 60 * 60 * 1000;
/// iCloud 副本与原文至少这么多行相同才算「高度相似」。
const ICLOUD_SIMILARITY: f64 = 0.5;

fn split_dir(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(at) => (&path[..=at], &path[at + 1..]),
        None => ("", path),
    }
}

/// 按文件名能确定的副本：（来源, 原文路径）。iCloud 的「名字 2.md」只给候选，要再验。
#[must_use]
pub fn conflict_by_name(path: &str) -> Option<(ConflictKind, String)> {
    let (dir, file) = split_dir(path);
    let stem = file.strip_suffix(".md")?;
    if let Some(at) = stem.find(".sync-conflict-") {
        let rest = &stem[at + ".sync-conflict-".len()..];
        if rest.len() >= 15 && rest.as_bytes()[..8].iter().all(u8::is_ascii_digit) {
            return Some((ConflictKind::Syncthing, format!("{dir}{}.md", &stem[..at])));
        }
    }
    for (open, close) in [(" (", ')'), ("（", '）')] {
        if let Some(at) = stem.rfind(open) {
            let inner = &stem[at + open.len()..];
            if inner.ends_with(close)
                && (inner.contains("conflicted copy") || inner.contains("冲突副本"))
            {
                return Some((
                    ConflictKind::Dropbox,
                    format!("{dir}{}.md", stem[..at].trim_end()),
                ));
            }
        }
    }
    let (base, number) = stem.rsplit_once(' ')?;
    if !base.is_empty()
        && !number.is_empty()
        && number.len() <= 2
        && number.bytes().all(|b| b.is_ascii_digit())
        && number != "0"
        && number != "1"
    {
        return Some((ConflictKind::ICloud, format!("{dir}{base}.md")));
    }
    None
}

/// 正文里有没有完整的一组 git 冲突标记（`<<<<<<<` → `=======` → `>>>>>>>`），代码块里的不算。
#[must_use]
pub fn has_git_conflict_markers(content: &str) -> bool {
    let mut fence: Option<&str> = None;
    let mut stage = 0;
    for line in content.lines() {
        let trimmed = line.trim_start();
        if let Some(open) = fence {
            if trimmed.starts_with(open) {
                fence = None;
            }
            continue;
        }
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fence = Some(&trimmed[..3]);
            continue;
        }
        stage = match stage {
            0 if line.starts_with("<<<<<<< ") || line == "<<<<<<<" => 1,
            1 if line == "=======" => 2,
            2 if line.starts_with(">>>>>>> ") || line == ">>>>>>>" => return true,
            other => other,
        };
    }
    false
}

/// 两份正文有多少比例的行相同（按较长那份算，空行不计）。
fn line_similarity(a: &str, b: &str) -> f64 {
    let lines = |text: &str| -> Vec<String> {
        text.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect()
    };
    let (left, mut right) = (lines(a), lines(b));
    let longest = left.len().max(right.len());
    if longest == 0 {
        return 1.0;
    }
    let mut same = 0usize;
    for line in &left {
        if let Some(at) = right.iter().position(|other| other == line) {
            right.swap_remove(at);
            same += 1;
        }
    }
    same as f64 / longest as f64
}

fn mtime_ms(path: &Path) -> Option<i64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let ms = modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis();
    i64::try_from(ms).ok()
}

impl Vault {
    /// 这一篇是不是冲突副本（含正文里的 git 冲突标记）。
    pub fn conflict_copy_of(&self, path: &str) -> Result<Option<ConflictCopy>> {
        let abs = self.root().join(path);
        let Ok(content) = std::fs::read_to_string(&abs) else {
            return Ok(None);
        };
        if let Some(copy) = self.named_conflict(path, &abs, &content) {
            return Ok(Some(copy));
        }
        Ok(has_git_conflict_markers(&content).then(|| ConflictCopy {
            path: path.to_owned(),
            kind: ConflictKind::GitMarkers,
            original: None,
        }))
    }

    fn named_conflict(&self, path: &str, abs: &Path, content: &str) -> Option<ConflictCopy> {
        let (kind, original) = conflict_by_name(path)?;
        let original_abs = self.root().join(&original);
        if !original_abs.is_file() {
            return None;
        }
        if kind == ConflictKind::ICloud {
            let close = match (mtime_ms(abs), mtime_ms(&original_abs)) {
                (Some(a), Some(b)) => (a - b).abs() <= ICLOUD_MTIME_WINDOW_MS,
                _ => false,
            };
            let original_content = std::fs::read_to_string(&original_abs).ok()?;
            if !close || line_similarity(content, &original_content) < ICLOUD_SIMILARITY {
                return None;
            }
        }
        Some(ConflictCopy {
            path: path.to_owned(),
            kind,
            original: Some(original),
        })
    }

    /// 库里全部按文件名认得出的冲突副本（文件树打标记用）。git 冲突标记要读正文，不在这里扫——
    /// 那一种在打开笔记时由 [`Vault::conflict_copy_of`] 说。
    pub fn conflict_copies(&self) -> Result<Vec<ConflictCopy>> {
        let mut out = Vec::new();
        for path in self.walk_markdown()? {
            if conflict_by_name(&path).is_none() {
                continue;
            }
            let abs = self.root().join(&path);
            let Ok(content) = std::fs::read_to_string(&abs) else {
                continue;
            };
            if let Some(copy) = self.named_conflict(&path, &abs, &content) {
                out.push(copy);
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }

    /// 选保留哪份：`keep_copy` = 用副本替换原文（原文先留一个版本，可从版本历史回滚），
    /// 否则保留原文。两种都把副本按当前删除策略移走（可还原）。
    pub fn resolve_conflict_copy(
        &self,
        path: &str,
        keep_copy: bool,
        mode: DeleteMode,
    ) -> Result<String> {
        let copy = self
            .conflict_copy_of(path)?
            .filter(|copy| copy.kind != ConflictKind::GitMarkers)
            .ok_or_else(|| crate::VaultError::NotFound(path.into()))?;
        let original = copy.original.unwrap_or_default();
        if keep_copy {
            let content = std::fs::read_to_string(self.root().join(path))?;
            self.write(&original, &content)?;
        }
        self.delete(path, mode)?;
        Ok(original)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_from_each_sync_tool_point_back_to_the_original() {
        assert_eq!(
            conflict_by_name("周报.sync-conflict-20261005-101010-ABCDEF7.md"),
            Some((ConflictKind::Syncthing, "周报.md".into()))
        );
        assert_eq!(
            conflict_by_name("项目/周报 (Pat's conflicted copy 2026-10-05).md"),
            Some((ConflictKind::Dropbox, "项目/周报.md".into()))
        );
        assert_eq!(
            conflict_by_name("周报（小王的冲突副本 2026-10-05）.md"),
            Some((ConflictKind::Dropbox, "周报.md".into()))
        );
        assert_eq!(
            conflict_by_name("项目/周报 2.md"),
            Some((ConflictKind::ICloud, "项目/周报.md".into()))
        );
        assert_eq!(conflict_by_name("周报.md"), None);
        assert_eq!(
            conflict_by_name("Chapter 1.md"),
            None,
            "「第 1 章」不是副本"
        );
        assert_eq!(
            conflict_by_name("图.sync-conflict-20261005-101010-A.png"),
            None
        );
    }

    #[test]
    fn git_markers_need_the_full_sequence_outside_code() {
        assert!(has_git_conflict_markers(
            "a\n<<<<<<< HEAD\n我的\n=======\n他的\n>>>>>>> main\nb"
        ));
        assert!(!has_git_conflict_markers("<<<<<<< HEAD\n只有开头"));
        assert!(!has_git_conflict_markers(
            "```\n<<<<<<< HEAD\nx\n=======\ny\n>>>>>>> main\n```"
        ));
    }

    /// 「名字 2.md」只有在原文就在旁边、内容几乎一样时才算 iCloud 副本；用户真的起名叫
    /// 「第 2 版」而内容不同的，不能被标成副本。
    #[test]
    fn an_icloud_duplicate_needs_a_similar_original_next_to_it() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        vault
            .write("周报.md", "# 周报\n\n本周进度\n下周计划\n风险")
            .unwrap();
        vault
            .write("周报 2.md", "# 周报\n\n本周进度\n下周计划\n风险\n补一句")
            .unwrap();
        vault.write("方案 2.md", "完全不同的另一份方案").unwrap();
        vault.write("方案.md", "# 方案\n\n第一版内容").unwrap();
        vault.write("孤单 2.md", "旁边没有原文").unwrap();
        let found: Vec<String> = vault
            .conflict_copies()
            .unwrap()
            .into_iter()
            .map(|c| c.path)
            .collect();
        assert_eq!(found, vec!["周报 2.md".to_owned()]);
    }

    #[test]
    fn keeping_the_copy_replaces_the_original_with_a_version_to_roll_back_to() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let vault = Vault::open(root).unwrap();
        vault.write("周报.md", "原文").unwrap();
        vault
            .write("周报.sync-conflict-20261005-101010-ABCDEF7.md", "副本")
            .unwrap();
        let copy = "周报.sync-conflict-20261005-101010-ABCDEF7.md";
        assert_eq!(
            vault.conflict_copy_of(copy).unwrap().map(|c| c.kind),
            Some(ConflictKind::Syncthing)
        );
        let original = vault
            .resolve_conflict_copy(copy, true, DeleteMode::VaultTrash)
            .unwrap();
        assert_eq!(original, "周报.md");
        assert_eq!(
            std::fs::read_to_string(root.join("周报.md")).unwrap(),
            "副本"
        );
        assert!(!root.join(copy).exists());
        assert!(!vault.snapshots("周报.md").is_empty(), "原文留了版本");
    }

    #[test]
    fn keeping_the_original_only_moves_the_copy_away() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let vault = Vault::open(root).unwrap();
        vault.write("周报.md", "原文").unwrap();
        vault
            .write("周报 (Pat's conflicted copy 2026-10-05).md", "副本")
            .unwrap();
        vault
            .resolve_conflict_copy(
                "周报 (Pat's conflicted copy 2026-10-05).md",
                false,
                DeleteMode::VaultTrash,
            )
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("周报.md")).unwrap(),
            "原文"
        );
        assert!(!root
            .join("周报 (Pat's conflicted copy 2026-10-05).md")
            .exists());
    }
}
