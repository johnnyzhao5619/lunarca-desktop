//! iCloud 未下载占位（TDD-06 §4.1「同步盘痕迹」②；笔记库对标轮 V3c）。
//!
//! macOS 13 及更早，开着「优化 Mac 存储空间」时，iCloud 把没下载到本机的文件换成一个点开头的
//! 占位：`周报.md` → `.周报.md.icloud`。点文件一律被当成隐藏文件跳过，于是笔记看起来「消失」、
//! 指向它的链接被判成断链。这里认出占位：树上显示成「未下载」，链接解析把它当作在，打开它时
//! 说清楚并请系统下载。
//!
//! macOS 14 起 iCloud 不再生成占位：未下载的文件保留原名，读取时由系统自动下载，无需处理。
//! 最低支持 10.15，所以这条仍要做。

use std::path::Path;

/// `.名字.icloud` → `名字`（不是占位返回 `None`）。
#[must_use]
pub fn placeholder_real_name(file_name: &str) -> Option<&str> {
    let inner = file_name.strip_prefix('.')?.strip_suffix(".icloud")?;
    (!inner.is_empty() && !inner.starts_with('.')).then_some(inner)
}

/// 一个库相对路径对应的占位文件（绝对路径）。
#[must_use]
pub fn placeholder_for(root: &Path, rel: &str) -> std::path::PathBuf {
    let (dir, name) = match rel.rfind('/') {
        Some(at) => (&rel[..at], &rel[at + 1..]),
        None => ("", rel),
    };
    let placeholder = format!(".{name}.icloud");
    if dir.is_empty() {
        root.join(placeholder)
    } else {
        root.join(dir).join(placeholder)
    }
}

impl crate::Vault {
    /// 这个路径的文件不在本机、但 iCloud 占位在（还没下载）。
    #[must_use]
    pub fn is_cloud_placeholder(&self, rel: &str) -> bool {
        !self.root().join(rel).exists() && placeholder_for(self.root(), rel).is_file()
    }

    /// 最近一次看到的全部占位（库相对的真名路径）：链接解析把它们当作在。由建树与启动扫描
    /// 顺手刷新，不为它单独遍历全库。
    ///
    /// 同步里只在云端的文件（ADR-156）同理算在：指向一段按需录音的 `![[…]]` 不是断链。
    pub(crate) fn cloud_placeholders(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .placeholders
            .lock()
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default();
        out.extend(self.sync_cloud_only_paths());
        out
    }

    /// 同步里只在云端、此刻本机确实没有的文件。
    pub(crate) fn sync_cloud_only_paths(&self) -> Vec<String> {
        self.sync_cloud_only
            .lock()
            .map(|set| {
                set.iter()
                    .filter(|rel| !self.root().join(rel).exists())
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 换上同步服务给的仅云端清单（ADR-156），返回进出清单的路径（文件树据此刷新）。
    pub fn set_sync_cloud_only(&self, paths: impl IntoIterator<Item = String>) -> Vec<String> {
        let next: std::collections::HashSet<String> = paths.into_iter().collect();
        let Ok(mut set) = self.sync_cloud_only.lock() else {
            return Vec::new();
        };
        let changed = set.symmetric_difference(&next).cloned().collect();
        *set = next;
        changed
    }

    /// 这个路径是同步里只在云端、本机没有的文件。
    #[must_use]
    pub fn is_sync_cloud_only(&self, rel: &str) -> bool {
        !self.root().join(rel).exists()
            && self
                .sync_cloud_only
                .lock()
                .is_ok_and(|set| set.contains(rel))
    }

    /// 把仅云端的文件按原位置挂到树上（所在目录本机不存在就补出来）。
    pub(crate) fn insert_sync_cloud_only(&self, tree: &mut crate::vault::NoteTree) {
        for rel in self.sync_cloud_only_paths() {
            let parts: Vec<&str> = rel.split('/').collect();
            let Some((name, dirs)) = parts.split_last() else {
                continue;
            };
            let mut node = &mut *tree;
            let mut prefix = String::new();
            for dir in dirs {
                prefix = if prefix.is_empty() {
                    (*dir).to_owned()
                } else {
                    format!("{prefix}/{dir}")
                };
                let at = match node
                    .children
                    .iter()
                    .position(|c| c.is_dir && c.name == *dir)
                {
                    Some(at) => at,
                    None => {
                        node.children
                            .push(crate::vault::NoteTree::virtual_dir(dir, &prefix));
                        node.children.len() - 1
                    }
                };
                node = &mut node.children[at];
            }
            if node.children.iter().any(|c| c.path == rel) {
                continue;
            }
            node.children
                .push(crate::vault::NoteTree::sync_cloud_only_file(name, &rel));
        }
    }

    /// 换上新看到的占位清单（全库遍历的结果）。
    pub fn set_cloud_placeholders(&self, paths: impl IntoIterator<Item = String>) {
        if let Ok(mut set) = self.placeholders.lock() {
            *set = paths.into_iter().collect();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 同步里只在云端的录音（ADR-156）：按原位置挂上树（目录本机还不存在也补出来）、标成仅云端；
    /// 指向它的嵌入不是断链、它不是孤儿；本机有了真文件之后不再重复列出。
    #[test]
    fn a_sync_cloud_only_file_is_listed_and_resolved_until_it_lands() {
        use crate::Vault;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let vault = Vault::open(root).unwrap();
        vault.write("录音/会议.md", "![[s-p01.wav]]").unwrap();
        assert_eq!(
            vault.set_sync_cloud_only(["录音/assets/s-p01.wav".to_owned()]),
            vec!["录音/assets/s-p01.wav".to_owned()]
        );

        let tree = vault.tree().unwrap();
        let rec = tree.children.iter().find(|c| c.name == "录音").unwrap();
        let assets = rec.children.iter().find(|c| c.name == "assets").unwrap();
        assert!(assets.is_dir);
        let file = &assets.children[0];
        assert_eq!(
            (
                file.path.as_str(),
                file.sync_cloud_only,
                file.cloud_placeholder
            ),
            ("录音/assets/s-p01.wav", true, false)
        );
        assert!(vault
            .walk_files()
            .unwrap()
            .contains(&"录音/assets/s-p01.wav".to_owned()));
        assert!(vault.orphan_assets().unwrap().is_empty());
        assert!(vault.is_sync_cloud_only("录音/assets/s-p01.wav"));

        std::fs::create_dir_all(root.join("录音/assets")).unwrap();
        std::fs::write(root.join("录音/assets/s-p01.wav"), b"RIFF").unwrap();
        assert!(!vault.is_sync_cloud_only("录音/assets/s-p01.wav"));
        let tree = vault.tree().unwrap();
        let rec = tree.children.iter().find(|c| c.name == "录音").unwrap();
        let assets = rec.children.iter().find(|c| c.name == "assets").unwrap();
        assert_eq!(assets.children.len(), 1);
        assert!(!assets.children[0].sync_cloud_only);
    }

    #[test]
    fn only_dot_name_dot_icloud_is_a_placeholder() {
        assert_eq!(placeholder_real_name(".周报.md.icloud"), Some("周报.md"));
        assert_eq!(placeholder_real_name(".图.png.icloud"), Some("图.png"));
        assert_eq!(placeholder_real_name("周报.md.icloud"), None);
        assert_eq!(placeholder_real_name(".icloud"), None);
        assert_eq!(placeholder_real_name("..icloud"), None);
        assert_eq!(placeholder_real_name(".DS_Store"), None);
    }

    #[test]
    fn placeholder_sits_next_to_where_the_file_would_be() {
        let root = Path::new("/库");
        assert_eq!(
            placeholder_for(root, "项目/周报.md"),
            Path::new("/库/项目/.周报.md.icloud")
        );
        assert_eq!(
            placeholder_for(root, "周报.md"),
            Path::new("/库/.周报.md.icloud")
        );
    }

    /// 占位在树上按真名出现、标成未下载；指向它的 `[[名字]]` 与 `![[图.png]]` 不算断链；
    /// 打开它说「还在 iCloud 上」而不是「找不到」；真文件下载回来后不再重复列出。
    #[test]
    fn a_placeholder_is_listed_resolved_and_explained() {
        use crate::Vault;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let vault = Vault::open(root).unwrap();
        std::fs::create_dir_all(root.join("项目")).unwrap();
        std::fs::write(root.join("项目/.周报.md.icloud"), b"bplist").unwrap();
        std::fs::write(root.join("项目/.图.png.icloud"), b"bplist").unwrap();
        vault.write("引用.md", "[[周报]] ![[图.png]]").unwrap();

        let tree = vault.tree().unwrap();
        let project = tree.children.iter().find(|c| c.name == "项目").unwrap();
        let listed: Vec<(&str, bool)> = project
            .children
            .iter()
            .map(|c| (c.path.as_str(), c.cloud_placeholder))
            .collect();
        assert_eq!(listed, vec![("项目/周报.md", true), ("项目/图.png", true)]);

        let resolved = vault
            .resolve_wikilinks("引用.md", &["周报".to_owned()])
            .unwrap();
        assert_eq!(resolved, vec![Some("项目/周报.md".to_owned())]);
        assert!(
            vault.orphan_assets().unwrap().is_empty(),
            "没下载的图不是孤儿"
        );

        assert!(matches!(
            vault.read("项目/周报.md"),
            Err(crate::VaultError::CloudPlaceholder(_))
        ));
        assert!(matches!(
            vault.read("项目/不存在.md"),
            Err(crate::VaultError::NotFound(_))
        ));

        std::fs::write(root.join("项目/周报.md"), "下载回来了").unwrap();
        let tree = vault.tree().unwrap();
        let project = tree.children.iter().find(|c| c.name == "项目").unwrap();
        let weekly: Vec<_> = project
            .children
            .iter()
            .filter(|c| c.name == "周报.md")
            .collect();
        assert_eq!(weekly.len(), 1);
        assert!(!weekly[0].cloud_placeholder);
    }
}
