//! 创建副本（PRD-06-006，笔记库对标轮 V5a）：在原文件旁边复制一份。
//!
//! **命名**：`名字 副本.md`，再有就 `名字 副本2.md`、`名字 副本3.md`……后缀由调用方按界面语言
//! 给（访达同样按系统语言命名）。数字前**不加空格**是刻意的：`名字 2.md` 正是 iCloud 冲突副本
//! 的形状（[`crate::conflicts`]），内容相同、时间相近的副本会被误标成冲突副本。
//!
//! 笔记走 [`Vault::write`]（留快照、进索引），并去掉 `luna.desktop_note`——钉在桌面上的是那一篇，
//! 不是它的副本，照抄会钉出两张一样的便签；附件按字节复制。文件夹不支持。

use std::path::Path;

use crate::vault::Vault;
use crate::{Result, VaultError};

/// 副本的库相对路径（在 `exists` 判定为空位的第一个名字上）。
fn duplicate_name(rel: &str, suffix: &str, exists: impl Fn(&str) -> bool) -> String {
    let (dir, file) = rel.rsplit_once('/').map_or(("", rel), |(d, f)| (d, f));
    let (stem, ext) = match file.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, Some(ext)),
        _ => (file, None),
    };
    let join = |name: &str| -> String {
        let file = ext.map_or_else(|| name.to_owned(), |ext| format!("{name}.{ext}"));
        if dir.is_empty() {
            file
        } else {
            format!("{dir}/{file}")
        }
    };
    let first = join(&format!("{stem} {suffix}"));
    if !exists(&first) {
        return first;
    }
    (2..)
        .map(|n| join(&format!("{stem} {suffix}{n}")))
        .find(|candidate| !exists(candidate))
        .expect("总能找到空位")
}

impl Vault {
    /// 在原文件旁边复制一份，返回副本的库相对路径。
    ///
    /// # Errors
    /// 原文件不存在、是文件夹或读写失败。
    pub fn duplicate(&self, rel: &str, suffix: &str) -> Result<String> {
        let rel = crate::vault::normalize_rel(rel);
        let source = self.root().join(&rel);
        if !source.is_file() {
            return Err(VaultError::NotFound(rel.into()));
        }
        let target = duplicate_name(&rel, suffix, |candidate| {
            self.root().join(Path::new(candidate)).exists()
        });
        if rel.to_lowercase().ends_with(".md") {
            let content = self.read(&rel)?.content;
            let content = if crate::frontmatter::read_luna(&content)
                .desktop_note
                .is_some()
            {
                let mut meta = crate::frontmatter::read_luna(&content);
                meta.desktop_note = None;
                crate::frontmatter::update_luna(&content, &meta)?
            } else {
                content
            };
            self.write(&target, &content)?;
        } else {
            std::fs::copy(&source, self.root().join(&target))?;
        }
        Ok(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_avoid_the_icloud_conflict_shape() {
        let taken = ["项目/计划 副本.md", "项目/计划 副本2.md"];
        let exists = |p: &str| taken.contains(&p);
        assert_eq!(duplicate_name("计划.md", "副本", |_| false), "计划 副本.md");
        assert_eq!(
            duplicate_name("项目/计划.md", "副本", exists),
            "项目/计划 副本3.md"
        );
        assert_eq!(duplicate_name("图.png", "copy", |_| false), "图 copy.png");
        assert_eq!(duplicate_name("README", "copy", |_| false), "README copy");
        assert_eq!(
            duplicate_name(".gitignore", "copy", |_| false),
            ".gitignore copy"
        );
        assert!(crate::conflicts::conflict_by_name("项目/计划 副本2.md").is_none());
    }

    #[test]
    fn a_duplicate_is_indexed_but_not_pinned_to_the_desktop() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        vault.write("便签.md", "正文\n").unwrap();
        vault
            .set_desktop_note("便签.md", Some(serde_json::from_str("{}").unwrap()))
            .unwrap();
        let copy = vault.duplicate("便签.md", "副本").unwrap();
        assert_eq!(copy, "便签 副本.md");
        let content = vault.read(&copy).unwrap().content;
        assert!(content.contains("正文"));
        assert!(crate::frontmatter::read_luna(&content)
            .desktop_note
            .is_none());
        assert!(
            crate::frontmatter::read_luna(&vault.read("便签.md").unwrap().content)
                .desktop_note
                .is_some()
        );
        assert!(vault.duplicate("没有.md", "副本").is_err());
        std::fs::create_dir(dir.path().join("夹")).unwrap();
        assert!(vault.duplicate("夹", "副本").is_err());
    }
}
