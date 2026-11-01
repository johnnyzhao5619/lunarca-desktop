//! 孤儿附件（PRD-06-012 ②，TDD-06 §4.6；笔记库对标轮 V2）：库里没有任何笔记引用的文件。
//!
//! **只列不删**：列出来、按体积倒序、给合计，用户勾选后按当前删除策略移走（可还原）。
//!
//! 「被引用」的口径与改名重构同一个解析器（[`crate::relink::resolved_targets`]）：`![[图.png]]`
//! 按文件名全库找、md 链接按源笔记目录找、HTML 媒体标签的 `src`、frontmatter `media[]` 与
//! 转写边车。此前删除跟随用的是「按源目录拼路径」的简化口径，`![[图.png]]` 引用放在
//! `assets/` 里的图会被它看漏——孤儿检测照抄就会把在用的图当孤儿，这是最坏的误判。
//!
//! 引用源不只是库里现在的笔记：**库内回收站里的笔记**（可还原）与**每篇笔记的版本快照**
//! （可回滚）引用的文件也不算孤儿——否则还原之后图就断了。旧正文按它原来的路径解析。

use crate::{Result, Vault};
use std::collections::HashSet;

/// 一个孤儿文件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrphanAsset {
    /// 库相对路径。
    pub path: String,
    pub bytes: u64,
}

/// 一次「移走孤儿」的结果。
#[derive(Debug, Default)]
pub struct OrphanRemoval {
    /// 已移走（或本来就不在了）。
    pub removed: Vec<String>,
    /// 列出来之后又被引用上了（或是笔记本身）：没动。
    pub kept: Vec<String>,
    /// 移走失败（路径, 原因）。
    pub failed: Vec<(String, crate::VaultError)>,
}

fn is_note(path: &str) -> bool {
    path.to_lowercase().ends_with(".md")
}

impl Vault {
    /// 库里没有任何笔记引用的文件（不含笔记本身），按体积倒序。
    pub fn orphan_assets(&self) -> Result<Vec<OrphanAsset>> {
        let referenced = self.referenced_files()?;
        // 排除路径里的附件不列（V4a）：用户划出那块就是不想被打扰；被引用的判定照旧全库算。
        let excluded = self.exclusions();
        let mut out: Vec<OrphanAsset> = self
            .walk_files()?
            .into_iter()
            .filter(|path| !is_note(path) && !referenced.contains(path))
            .filter(|path| !excluded.covers(path))
            // iCloud 还没下载的文件不在本机，谈不上占用，也不该被移走（[`crate::icloud`]）。
            .filter(|path| !self.is_cloud_placeholder(path) && !self.is_sync_cloud_only(path))
            .map(|path| OrphanAsset {
                bytes: std::fs::metadata(self.root().join(&path)).map_or(0, |m| m.len()),
                path,
            })
            .collect();
        out.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.path.cmp(&b.path)));
        Ok(out)
    }

    /// 移走用户勾选的孤儿：**删除那一刻重判一次**——列出来之后若有笔记引用了它（或勾的是
    /// 一篇笔记），原样留下。走当前删除策略，与删除笔记同一条路（可还原）。
    pub fn remove_orphan_assets(
        &self,
        paths: &[String],
        mode: crate::DeleteMode,
    ) -> Result<OrphanRemoval> {
        let referenced = self.referenced_files()?;
        let excluded = self.exclusions();
        let mut out = OrphanRemoval::default();
        for path in paths {
            let path = crate::vault::normalize_rel(path);
            if is_note(&path) || referenced.contains(&path) || excluded.covers(&path) {
                out.kept.push(path);
            } else if !self.root().join(&path).is_file() {
                out.removed.push(path);
            } else {
                match self.delete(&path, mode) {
                    Ok(()) => out.removed.push(path),
                    Err(error) => out.failed.push((path, error)),
                }
            }
        }
        Ok(out)
    }

    /// 全部引用源指向的文件（库内笔记、库内回收站里的笔记、所有版本快照）。
    fn referenced_files(&self) -> Result<HashSet<String>> {
        // (按哪个路径解析, 正文)
        let mut sources: Vec<(String, String)> = Vec::new();
        let mut note_paths: Vec<String> = Vec::new();
        for rel in self.walk_markdown()? {
            if let Ok(content) = std::fs::read_to_string(self.root().join(&rel)) {
                sources.push((rel.clone(), content));
            }
            note_paths.push(rel);
        }
        let trash = self.vault_trash_dir();
        for entry in self.load_trash_index() {
            let abs = trash.join(&entry.id);
            if abs.is_dir() {
                let mut inside = Vec::new();
                let _ = crate::vault::walk_md(&abs, &abs, &mut inside);
                for sub in inside {
                    let original = format!("{}/{sub}", entry.original_path);
                    if let Ok(content) = std::fs::read_to_string(abs.join(&sub)) {
                        sources.push((original.clone(), content));
                    }
                    note_paths.push(original);
                }
            } else if is_note(&entry.original_path) {
                if let Ok(content) = std::fs::read_to_string(&abs) {
                    sources.push((entry.original_path.clone(), content));
                }
                note_paths.push(entry.original_path);
            }
        }
        for path in &note_paths {
            let mut seen = HashSet::new();
            for meta in self.snapshots(path) {
                if seen.insert(meta.hash.clone()) {
                    if let Ok(content) = self.read_snapshot(path, &meta.id) {
                        sources.push((path.clone(), content));
                    }
                }
            }
        }

        let world = self.link_world()?;
        let mut referenced = crate::relink::resolved_targets(
            &world,
            sources
                .iter()
                .map(|(src, content)| (src.as_str(), content.as_str())),
        );
        for (_, content) in &sources {
            for media in crate::frontmatter::read_luna(content).media {
                referenced.insert(crate::vault::normalize_rel(&media.path));
                if let Some(segments) = media.segments.as_deref() {
                    referenced.insert(crate::vault::normalize_rel(segments));
                }
            }
        }
        Ok(referenced)
    }
}

#[cfg(test)]
mod tests {
    use crate::{DeleteMode, Vault};

    fn put(root: &std::path::Path, rel: &str, bytes: &[u8]) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn orphans(vault: &Vault) -> Vec<String> {
        vault
            .orphan_assets()
            .unwrap()
            .into_iter()
            .map(|asset| asset.path)
            .collect()
    }

    /// 每一类引用源都算数：`![[名字]]` 按文件名全库找到的、md 链接、HTML `src`、
    /// frontmatter `media[]`、只在回收站里的笔记引用的、只在旧版本里引用的。
    #[test]
    fn every_kind_of_reference_keeps_a_file_out_of_the_orphan_list() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let vault = Vault::open(root).unwrap();
        for name in ["wiki", "md", "html", "trash", "snap", "orphan"] {
            put(root, &format!("笔记/assets/{name}.png"), name.as_bytes());
        }
        put(root, "Recordings/assets/会.m4a", b"audio");
        vault
            .write(
                "笔记/甲.md",
                "![[wiki.png]] ![](assets/md.png) <img src=\"assets/html.png\">",
            )
            .unwrap();
        vault
            .write(
                "Recordings/会.md",
                "---\nluna:\n  type: recording\n  media:\n    - path: Recordings/assets/会.m4a\n---\n正文",
            )
            .unwrap();
        // 只被回收站里的笔记引用：两篇都引用它，删一篇（共享的不随删除走），再让另一篇不引用。
        vault.write("笔记/乙.md", "![](assets/trash.png)").unwrap();
        vault.write("笔记/丙.md", "![](assets/trash.png)").unwrap();
        vault.delete("笔记/乙.md", DeleteMode::VaultTrash).unwrap();
        vault.write("笔记/丙.md", "不再引用").unwrap();
        // 只在旧版本里引用：覆盖写会先留一个版本。
        vault.write("笔记/丁.md", "![](assets/snap.png)").unwrap();
        vault.write("笔记/丁.md", "改掉了").unwrap();

        assert_eq!(orphans(&vault), vec!["笔记/assets/orphan.png".to_owned()]);
        assert_eq!(vault.orphan_assets().unwrap()[0].bytes, 6);
    }

    /// 移走前重判：列出来之后被引用上的、勾错的笔记都不动；不在了的算已达成。
    #[test]
    fn removal_rechecks_references_at_the_moment_it_runs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let vault = Vault::open(root).unwrap();
        put(root, "a.png", b"a");
        put(root, "b.png", b"b");
        vault.write("笔记.md", "正文").unwrap();
        assert_eq!(
            orphans(&vault),
            vec!["a.png".to_owned(), "b.png".to_owned()]
        );
        vault.write("笔记.md", "![](b.png)").unwrap();
        let out = vault
            .remove_orphan_assets(
                &[
                    "a.png".into(),
                    "b.png".into(),
                    "笔记.md".into(),
                    "早没了.png".into(),
                ],
                DeleteMode::VaultTrash,
            )
            .unwrap();
        assert_eq!(
            out.removed,
            vec!["a.png".to_owned(), "早没了.png".to_owned()]
        );
        assert_eq!(out.kept, vec!["b.png".to_owned(), "笔记.md".to_owned()]);
        assert!(out.failed.is_empty());
        assert!(!root.join("a.png").exists());
        assert!(root.join("b.png").exists());
        assert!(root.join("笔记.md").exists());
    }
}
