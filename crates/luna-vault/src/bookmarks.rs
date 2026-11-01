//! 书签（PRD-06-006，笔记库对标轮 V5b；ADR-140 ①：书签描述「库里的内容怎么组织」，随库走）。
//!
//! 存于 `.luna/bookmarks.json`（meta store），按加入的先后排列；笔记、附件、文件夹都可以加。
//! **跟着内容走**：应用内改名 / 移动（含撤销）与库外改名的修复都会改写书签里的路径，文件夹
//! 移动连带其下的书签。**删除不动书签**：读取时只列此刻还在的条目，于是从回收站还原之后书签
//! 自己回来；下一次增删书签时顺手把已经不在的剪掉。

use crate::graph::scope_covers;
use crate::vault::Vault;
use crate::Result;

const BOOKMARKS_META: &str = "bookmarks";

/// 一条书签。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VaultBookmark {
    pub path: String,
    pub is_dir: bool,
}

impl Vault {
    fn stored_bookmarks(&self) -> Result<Vec<String>> {
        crate::meta::read_meta(self.root(), BOOKMARKS_META)
    }

    fn exists_in_vault(&self, rel: &str) -> bool {
        self.root().join(rel).exists()
    }

    /// 此刻还在的书签（按加入的先后）。
    ///
    /// # Errors
    /// 书签文件损坏或读不了。
    pub fn bookmarks(&self) -> Result<Vec<VaultBookmark>> {
        Ok(self
            .stored_bookmarks()?
            .into_iter()
            .filter(|path| self.exists_in_vault(path))
            .map(|path| VaultBookmark {
                is_dir: self.root().join(&path).is_dir(),
                path,
            })
            .collect())
    }

    /// 加入 / 移除一条书签，返回更新后的书签（同 [`Self::bookmarks`]）。
    ///
    /// # Errors
    /// 路径为空或越界、要加的条目不存在、或写入失败。
    pub fn set_bookmarked(&self, path: &str, bookmarked: bool) -> Result<Vec<VaultBookmark>> {
        let path = Self::validate_scope_folder(path)?;
        if bookmarked && !self.exists_in_vault(&path) {
            return Err(crate::VaultError::NotFound(path.into()));
        }
        let mut stored: Vec<String> = self
            .stored_bookmarks()?
            .into_iter()
            .filter(|existing| existing != &path && self.exists_in_vault(existing))
            .collect();
        if bookmarked {
            stored.push(path);
        }
        crate::meta::write_meta(self.root(), BOOKMARKS_META, &stored)?;
        self.bookmarks()
    }

    /// 改名 / 移动之后改写书签里的路径（文件夹连带其下的条目）。书签是次要状态：写不进去
    /// 不让改名本身失败，调用方忽略错误即可。
    pub(crate) fn remap_bookmarks(&self, from: &str, to: &str) -> Result<()> {
        let stored = self.stored_bookmarks()?;
        if !stored.iter().any(|path| scope_covers(from, path)) {
            return Ok(());
        }
        let remapped: Vec<String> = stored
            .into_iter()
            .map(|path| {
                if scope_covers(from, &path) {
                    format!("{to}{}", &path[from.len()..])
                } else {
                    path
                }
            })
            .collect();
        crate::meta::write_meta(self.root(), BOOKMARKS_META, &remapped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(bookmarks: &[VaultBookmark]) -> Vec<(&str, bool)> {
        bookmarks
            .iter()
            .map(|b| (b.path.as_str(), b.is_dir))
            .collect()
    }

    #[test]
    fn bookmarks_follow_moves_and_survive_a_trip_through_the_trash() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        vault.write("项目/计划.md", "正文\n").unwrap();
        vault.write("随笔.md", "正文\n").unwrap();

        vault.set_bookmarked("随笔.md", true).unwrap();
        let list = vault.set_bookmarked("项目", true).unwrap();
        assert_eq!(paths(&list), vec![("随笔.md", false), ("项目", true)]);
        assert!(vault.set_bookmarked("没有.md", true).is_err());

        // 改名与移动文件夹：书签跟着走（文件夹连带其下）。
        vault.move_("随笔.md", "随想.md").unwrap();
        vault.move_("项目", "归档/项目").unwrap();
        assert_eq!(
            paths(&vault.bookmarks().unwrap()),
            vec![("随想.md", false), ("归档/项目", true)]
        );

        // 删除不动书签：只是暂时不列，还原回来又在。
        vault
            .delete("随想.md", crate::vault::DeleteMode::VaultTrash)
            .unwrap();
        assert_eq!(
            paths(&vault.bookmarks().unwrap()),
            vec![("归档/项目", true)]
        );
        let trashed = vault.list_trash();
        vault
            .restore(&trashed[0].id, crate::vault::RestoreConflict::Fail)
            .unwrap();
        assert_eq!(
            paths(&vault.bookmarks().unwrap()),
            vec![("随想.md", false), ("归档/项目", true)]
        );

        let list = vault.set_bookmarked("随想.md", false).unwrap();
        assert_eq!(paths(&list), vec![("归档/项目", true)]);
    }
}
