//! 增量扫描（TDD-06 §4.1 修订，PRD-06-004；笔记库对标轮 V1）。
//!
//! 此前每次启动都 `clear()` 索引再整库重读：1 万篇的库实测每次启动 50 秒、期间应用不可用，
//! 后台那条全程攥着 Vault 锁、不可取消，任一目录读不出时索引已被清空。现在拆成四步，前三步
//! 都不碰 Vault 锁，调用方在批与批之间可以放锁、查取消：
//!
//! 1. [`walk`]：列出全部 `.md` 与它们的修改时间。不跟随符号链接（防环），读不了的子目录跳过
//!    并记下，**不让整次失败**；
//! 2. [`plan`]：与索引里的指纹比对——修改时间相同即视为没变；新出现的、时间变了的进待读清单；
//!    索引里有、磁盘上没有的进待删清单；
//! 3. [`read_batch`]：读一批待读的文件。时间变了而内容指纹没变（同步盘下载回来会整体改时间）
//!    只记新时间，不重建；
//! 4. [`crate::Vault::apply_scan_batch`]：一批一个事务写进索引。
//!
//! 只有索引格式升级（`SCHEMA_VERSION` 变化）才整库重建，由打开索引时处理，不在这里。

use crate::hashing::ContentHash;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// 磁盘上的一篇笔记：库相对路径与修改时间（毫秒）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskNote {
    pub path: String,
    pub modified_ms: i64,
}

/// 一次遍历的结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Walk {
    pub notes: Vec<DiskNote>,
    /// 读不了的子目录（库相对路径）：跳过并如实报告，进体检，不让整次扫描失败。
    pub skipped_dirs: Vec<String>,
    /// iCloud 未下载占位（真名的库相对路径，[`crate::icloud`]）：不进索引，但链接解析当作在。
    pub placeholders: Vec<String>,
}

/// 列出库里全部 `.md`。点开头的文件与目录（`.luna`、`.trash`、`.obsidian`…）跳过；符号链接
/// 一律不跟随——指回祖先目录的链接会让递归永不结束。根目录本身读不了才算失败。
pub fn walk(root: &Path) -> std::io::Result<Walk> {
    let mut out = Walk::default();
    let entries = std::fs::read_dir(root)?;
    walk_entries(root, entries, &mut out);
    out.notes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

fn walk_entries(root: &Path, entries: std::fs::ReadDir, out: &mut Walk) {
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            if let Some(real) = crate::icloud::placeholder_real_name(&name) {
                let real_abs = entry.path().with_file_name(real);
                if !real_abs.exists() {
                    if let Ok(rel) = real_abs.strip_prefix(root) {
                        out.placeholders
                            .push(rel.to_string_lossy().replace('\\', "/"));
                    }
                }
            }
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        let rel = || {
            path.strip_prefix(root)
                .map(|rel| rel.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default()
        };
        if kind.is_dir() {
            match std::fs::read_dir(&path) {
                Ok(children) => walk_entries(root, children, out),
                Err(_) => out.skipped_dirs.push(rel()),
            }
        } else if kind.is_file() && name.to_lowercase().ends_with(".md") {
            let modified_ms = entry
                .metadata()
                .ok()
                .and_then(|meta| meta.modified().ok())
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_millis() as i64);
            out.notes.push(DiskNote {
                path: rel(),
                modified_ms,
            });
        }
    }
}

/// 一次扫描要做的事。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// 新出现的或修改时间变了的：要读。
    pub to_read: Vec<DiskNote>,
    /// 索引里有、磁盘上已经没有的：要从索引删掉。
    pub to_remove: Vec<String>,
    /// 修改时间没变、直接跳过的篇数。
    pub unchanged: usize,
}

/// 比对磁盘与索引。`force` = 全部重读（「重建索引」按钮），仍然不先清空。
#[must_use]
pub fn plan(walk: &Walk, indexed: &HashMap<String, (i64, String)>, force: bool) -> Plan {
    let mut out = Plan::default();
    let mut seen = HashSet::with_capacity(walk.notes.len());
    for note in &walk.notes {
        seen.insert(note.path.as_str());
        match indexed.get(&note.path) {
            Some((modified, _)) if !force && *modified == note.modified_ms => out.unchanged += 1,
            _ => out.to_read.push(note.clone()),
        }
    }
    // 读不了的目录里的笔记不算「消失」：目录暂时没权限，不该把它们从索引里抹掉。
    let skipped: Vec<String> = walk
        .skipped_dirs
        .iter()
        .map(|dir| format!("{dir}/"))
        .collect();
    out.to_remove = indexed
        .keys()
        .filter(|path| !seen.contains(path.as_str()))
        .filter(|path| !skipped.iter().any(|dir| path.starts_with(dir.as_str())))
        .cloned()
        .collect();
    out.to_remove.sort();
    out
}

/// 写进索引的一项。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    /// 内容变了（或新篇）：整篇重建。
    Upsert {
        path: String,
        content: String,
        modified_ms: i64,
    },
    /// 只有修改时间变了。
    Touch { path: String, modified_ms: i64 },
    /// 磁盘上已经没有了。
    Remove { path: String },
}

/// 读一批待读的文件（不碰 Vault 锁）。读不出来的（期间被删、不是 UTF-8）跳过，下一次扫描再看。
#[must_use]
pub fn read_batch(
    root: &Path,
    batch: &[DiskNote],
    indexed: &HashMap<String, (i64, String)>,
) -> Vec<Item> {
    batch
        .iter()
        .filter_map(|note| {
            let content = std::fs::read_to_string(root.join(&note.path)).ok()?;
            let same_content = indexed.get(&note.path).is_some_and(|(_, hash)| {
                ContentHash::of(content.as_bytes()).as_str() == hash.as_str()
            });
            Some(if same_content {
                Item::Touch {
                    path: note.path.clone(),
                    modified_ms: note.modified_ms,
                }
            } else {
                Item::Upsert {
                    path: note.path.clone(),
                    content,
                    modified_ms: note.modified_ms,
                }
            })
        })
        .collect()
}

/// 索引落后磁盘多少（[`crate::Vault::index_drift`]）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Drift {
    /// 新出现的或修改时间变了的篇数。
    pub changed: usize,
    /// 索引里有、磁盘上已经没有的篇数。
    pub removed: usize,
}

impl Drift {
    /// 索引与磁盘一致。
    #[must_use]
    pub fn is_current(&self) -> bool {
        self.changed == 0 && self.removed == 0
    }
}

impl crate::Vault {
    /// 索引落后磁盘多少（CLI 在应用没开时如实提示「索引可能不是最新」，ADR-141 ④）：与增量
    /// 扫描同一份 [`walk`] + [`plan`]，只比对、不读正文、不写索引。
    pub fn index_drift(&self) -> crate::Result<Drift> {
        let walk = walk(self.root())?;
        let plan = plan(&walk, &self.index().fingerprints()?, false);
        Ok(Drift {
            changed: plan.to_read.len(),
            removed: plan.to_remove.len(),
        })
    }

    /// 索引里每篇笔记的指纹（短暂用一下索引）。
    pub fn index_fingerprints(&self) -> crate::Result<HashMap<String, (i64, String)>> {
        self.index().fingerprints()
    }

    /// 把一批扫描结果写进索引：一个事务，要么整批进、要么整批不进。
    pub fn apply_scan_batch(&self, items: &[Item]) -> crate::Result<()> {
        self.index().in_transaction(|| {
            for item in items {
                match item {
                    Item::Upsert {
                        path,
                        content,
                        modified_ms,
                    } => self.index().index_note(path, content, *modified_ms)?,
                    Item::Touch { path, modified_ms } => {
                        self.index().touch_modified(path, *modified_ms)?;
                    }
                    Item::Remove { path } => self.index().remove_note(path)?,
                }
            }
            Ok(())
        })
    }
}

/// 一次同步跑完的增量扫描的结果（库内调用与测试用；应用层在后台分批跑同样的四步）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// 扫描之后索引里的笔记数。
    pub indexed: usize,
    /// 这次实际重读了几篇。
    pub read: usize,
    pub removed: usize,
    pub skipped_dirs: Vec<String>,
}

/// 每批多少篇：够大以摊薄事务开销，够小以让应用层在批与批之间及时放锁、查取消。
pub const BATCH: usize = 128;

#[cfg(test)]
mod tests {
    use super::*;

    fn disk(path: &str, modified_ms: i64) -> DiskNote {
        DiskNote {
            path: path.to_owned(),
            modified_ms,
        }
    }

    #[test]
    fn plan_reads_only_new_and_changed_and_removes_vanished() {
        let walk = Walk {
            notes: vec![disk("a.md", 1), disk("b.md", 5), disk("c.md", 3)],
            skipped_dirs: Vec::new(),
            placeholders: Vec::new(),
        };
        let indexed: HashMap<String, (i64, String)> = [
            ("a.md".to_owned(), (1, "h".to_owned())),
            ("b.md".to_owned(), (2, "h".to_owned())),
            ("gone.md".to_owned(), (1, "h".to_owned())),
        ]
        .into();
        let out = plan(&walk, &indexed, false);
        assert_eq!(out.to_read, vec![disk("b.md", 5), disk("c.md", 3)]);
        assert_eq!(out.to_remove, vec!["gone.md".to_owned()]);
        assert_eq!(out.unchanged, 1);
        assert_eq!(
            plan(&walk, &indexed, true).to_read.len(),
            3,
            "重建 = 全部重读"
        );
    }

    #[test]
    fn notes_under_an_unreadable_folder_are_not_treated_as_deleted() {
        let walk = Walk {
            notes: vec![disk("a.md", 1)],
            skipped_dirs: vec!["锁住".to_owned()],
            placeholders: Vec::new(),
        };
        let indexed: HashMap<String, (i64, String)> = [
            ("a.md".to_owned(), (1, "h".to_owned())),
            ("锁住/b.md".to_owned(), (1, "h".to_owned())),
        ]
        .into();
        assert!(plan(&walk, &indexed, false).to_remove.is_empty());
    }

    #[test]
    fn same_content_with_a_new_time_is_only_touched() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.md"), "正文").unwrap();
        std::fs::write(dir.path().join("b.md"), "新的").unwrap();
        let indexed: HashMap<String, (i64, String)> = [
            (
                "a.md".to_owned(),
                (1, ContentHash::of("正文".as_bytes()).to_string()),
            ),
            (
                "b.md".to_owned(),
                (1, ContentHash::of("旧的".as_bytes()).to_string()),
            ),
        ]
        .into();
        let items = read_batch(
            dir.path(),
            &[disk("a.md", 9), disk("b.md", 9), disk("没了.md", 9)],
            &indexed,
        );
        assert_eq!(
            items,
            vec![
                Item::Touch {
                    path: "a.md".to_owned(),
                    modified_ms: 9
                },
                Item::Upsert {
                    path: "b.md".to_owned(),
                    content: "新的".to_owned(),
                    modified_ms: 9
                },
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn walk_never_follows_symlinks_and_survives_unreadable_folders() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::fs::write(root.join("a/b/笔记.md"), "x").unwrap();
        // 指回祖先的链接：跟随的话递归永不结束。
        std::os::unix::fs::symlink(root.join("a"), root.join("a/b/回环")).unwrap();
        std::fs::create_dir(root.join("锁住")).unwrap();
        std::fs::write(root.join("锁住/c.md"), "y").unwrap();
        std::fs::set_permissions(root.join("锁住"), std::fs::Permissions::from_mode(0o000))
            .unwrap();
        let out = walk(root).unwrap();
        std::fs::set_permissions(root.join("锁住"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        assert_eq!(
            out.notes
                .iter()
                .map(|n| n.path.as_str())
                .collect::<Vec<_>>(),
            vec!["a/b/笔记.md"]
        );
        assert_eq!(out.skipped_dirs, vec!["锁住".to_owned()]);
    }
}
