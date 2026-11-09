//! 从系统拖进文件树（PRD-07-008，笔记库对标轮 W5a）：把外部文件**复制**进库里的某个文件夹。
//!
//! - 拖进来的是副本，原文件不动（与 Obsidian 同口径；访达里按住 ⌘ 才是移动，这里不做）。
//! - 重名走库里既有的唯一化规则（`名字-1.md`，与附件搬迁、回收站「保留两份」同一个
//!   [`Vault::unique_vault_path`]）；**绝不覆盖**——笔记经 [`Vault::publish_new`] 的 `link(2)`
//!   原子落点，附件经 `create_new` 占名，落点被抢就换下一个名字。
//! - 笔记进索引、登记自写；附件按字节复制。
//! - 不收：文件夹、点开头的文件、本来就在这个库里的文件（那是移动，该在树里拖）、不是 UTF-8 的
//!   `.md`。逐个给理由，调用方如实告诉用户哪几个没进来。

use std::path::Path;

use crate::vault::Vault;
use crate::{Result, VaultError};

/// 一个文件没被复制进来的理由。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportSkip {
    /// 是文件夹，或不是普通文件。
    NotAFile,
    /// 点开头的文件（Lunarca 把它们当作「不是用户内容」，V4b）。
    Hidden,
    /// 本来就在这个库里。
    InsideVault,
    /// `.md` 但不是 UTF-8 文本。
    NotUtf8,
    /// 读或写失败（权限、磁盘满……）。
    Failed,
}

/// 一次拖入的结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportReport {
    /// 复制进来的文件（库相对路径，按拖入顺序）。
    pub imported: Vec<String>,
    /// 没进来的：（文件名, 理由）。
    pub skipped: Vec<(String, ImportSkip)>,
}

fn is_markdown(name: &str) -> bool {
    name.to_lowercase().ends_with(".md")
}

impl Vault {
    /// 把外部文件复制进 `folder`（库相对路径，`""` = 库根）。
    ///
    /// # Errors
    /// `folder` 不合法（越界、点开头）或不是库里已有的文件夹。单个文件的失败不算整次失败，
    /// 记进 [`ImportReport::skipped`]。
    pub fn import_files(&self, sources: &[impl AsRef<Path>], folder: &str) -> Result<ImportReport> {
        let folder = folder.trim_matches('/');
        if !folder.is_empty()
            && crate::pathutil::external_rel_path(folder).as_deref() != Some(folder)
        {
            return Err(VaultError::PathEscape(folder.into()));
        }
        if !self.root().join(folder).is_dir() {
            return Err(VaultError::NotFound(folder.into()));
        }
        let mut report = ImportReport::default();
        for source in sources {
            let source = source.as_ref();
            let name = source
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            match self.import_one(source, &name, folder) {
                Ok(rel) => report.imported.push(rel),
                Err(skip) => report.skipped.push((name, skip)),
            }
        }
        Ok(report)
    }

    fn import_one(
        &self,
        source: &Path,
        name: &str,
        folder: &str,
    ) -> std::result::Result<String, ImportSkip> {
        if name.is_empty() || !source.is_file() {
            return Err(ImportSkip::NotAFile);
        }
        if name.starts_with('.') {
            return Err(ImportSkip::Hidden);
        }
        if std::fs::canonicalize(source).is_ok_and(|real| real.starts_with(self.root())) {
            return Err(ImportSkip::InsideVault);
        }
        let wanted = luna_sync_core::attachment::join_vault_rel(folder, name);
        if is_markdown(name) {
            let content = std::fs::read(source).map_err(|_| ImportSkip::Failed)?;
            let content = String::from_utf8(content).map_err(|_| ImportSkip::NotUtf8)?;
            // 落点被抢（`unique_vault_path` 判空之后、发布之前有人写了同名）就再取一次。
            loop {
                let target = self.unique_vault_path(&wanted);
                match self.publish_new(&target, &content) {
                    Ok(()) => return Ok(target),
                    Err(VaultError::AlreadyExists(_)) => {}
                    Err(_) => return Err(ImportSkip::Failed),
                }
            }
        }
        let mut input = std::fs::File::open(source).map_err(|_| ImportSkip::Failed)?;
        loop {
            let target = self.unique_vault_path(&wanted);
            let abs = self.root().join(&target);
            let mut output = match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&abs)
            {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(_) => return Err(ImportSkip::Failed),
            };
            if std::io::copy(&mut input, &mut output).is_err() {
                // 复制到一半的文件不留在库里。
                drop(output);
                let _ = std::fs::remove_file(&abs);
                return Err(ImportSkip::Failed);
            }
            return Ok(target);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault() -> (tempfile::TempDir, Vault) {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        (dir, vault)
    }

    #[test]
    fn copies_notes_and_files_into_the_folder_without_overwriting() {
        let (_dir, vault) = vault();
        std::fs::create_dir_all(vault.root().join("项目")).unwrap();
        std::fs::write(vault.root().join("项目/计划.md"), "原来的").unwrap();
        let outside = tempfile::tempdir().unwrap();
        let note = outside.path().join("计划.md");
        let image = outside.path().join("图.png");
        std::fs::write(&note, "# 外面的计划\n\n季度复盘\n").unwrap();
        std::fs::write(&image, [0u8, 1, 2, 3]).unwrap();

        let report = vault.import_files(&[&note, &image], "项目").unwrap();
        assert_eq!(report.imported, vec!["项目/计划-1.md", "项目/图.png"]);
        assert!(report.skipped.is_empty());
        assert_eq!(
            std::fs::read_to_string(vault.root().join("项目/计划.md")).unwrap(),
            "原来的"
        );
        assert_eq!(
            std::fs::read(vault.root().join("项目/图.png")).unwrap(),
            [0u8, 1, 2, 3]
        );
        // 笔记进了索引，搜得到；原文件还在。
        let hits = vault
            .search_matches(&luna_sync_core::search::SearchQuery::parse(
                "季度复盘",
                false,
            ))
            .unwrap();
        assert_eq!(hits.notes[0].path, "项目/计划-1.md");
        assert!(note.exists());
    }

    #[test]
    fn skips_folders_hidden_files_vault_files_and_non_utf8_notes() {
        let (_dir, vault) = vault();
        std::fs::write(vault.root().join("已在库里.md"), "x").unwrap();
        let outside = tempfile::tempdir().unwrap();
        let folder = outside.path().join("一个文件夹");
        std::fs::create_dir_all(&folder).unwrap();
        let hidden = outside.path().join(".env");
        std::fs::write(&hidden, "SECRET=1").unwrap();
        let binary = outside.path().join("乱码.md");
        std::fs::write(&binary, [0xffu8, 0xfe, 0x00]).unwrap();
        let inside = vault.root().join("已在库里.md");

        let report = vault
            .import_files(&[&folder, &hidden, &binary, &inside], "")
            .unwrap();
        assert!(report.imported.is_empty());
        assert_eq!(
            report.skipped,
            vec![
                ("一个文件夹".to_owned(), ImportSkip::NotAFile),
                (".env".to_owned(), ImportSkip::Hidden),
                ("乱码.md".to_owned(), ImportSkip::NotUtf8),
                ("已在库里.md".to_owned(), ImportSkip::InsideVault),
            ]
        );
    }

    #[test]
    fn the_target_folder_must_be_an_existing_visible_folder_inside_the_vault() {
        let (_dir, vault) = vault();
        let none: [&Path; 0] = [];
        assert!(matches!(
            vault.import_files(&none, "../外面"),
            Err(VaultError::PathEscape(_))
        ));
        assert!(matches!(
            vault.import_files(&none, ".luna"),
            Err(VaultError::PathEscape(_))
        ));
        assert!(matches!(
            vault.import_files(&none, "没有这个"),
            Err(VaultError::NotFound(_))
        ));
        assert!(vault.import_files(&none, "").is_ok());
    }
}
