//! 网页版对一个已打开的云端库的视图（TDD-17 §5.1 / §5.6）：清单在内存里的形状、增量更新（含本页
//! 自己的提交）、路径树、「最近」、标题与路径搜索、撞名判定、链接与附件的解析。
//!
//! 解密后的清单只在这里（Worker 的 WASM 内存），主线程只拿到树与搜索结果这类命令结果。判定全部复用
//! `luna-sync-core` 的口径：隐藏路径、标题、名字比较键、「这条链接指向哪一篇」与按嵌入写法找文件都与
//! 桌面同一份实现（ADR-177 ⑥）。纯逻辑、不依赖 wasm-bindgen，原生单测直接覆盖。

use std::collections::{BTreeMap, HashMap};

use luna_sync_core::links::{
    self, extension_of, find_file, is_hidden_rel, normalize_vault_rel, parent_dir, percent_decode,
    relative_link_path, split_wikilink_target, title_of, LinkIndex, WikiLinkCandidate,
    IMAGE_EXTENSIONS,
};
use luna_sync_core::manifest::EntryPlain;
use luna_sync_core::path::name_key;
use serde::Serialize;
use uuid::Uuid;

/// 一个文件此刻的头：rev 与解开的条目。
#[derive(Debug, Clone)]
pub struct FileHead {
    pub rev: i64,
    pub entry: EntryPlain,
    /// 补齐后的大小（清单里服务端可见的那个数；改名原样带上）。
    pub size_padded: i64,
}

/// 清单响应里的一条（`manifest` / `changes` 同形）解开之后的样子。
#[derive(Debug, Clone)]
pub enum Change {
    Put {
        file_id: Uuid,
        rev: i64,
        entry: EntryPlain,
        size_padded: i64,
    },
    Delete {
        file_id: Uuid,
        rev: i64,
    },
}

/// 已打开的库：纪元、读到的序号与全部文件头。
#[derive(Debug, Default)]
pub struct VaultView {
    epoch: String,
    head_seq: i64,
    files: HashMap<Uuid, FileHead>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct TreeNode {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub file_id: Option<String>,
    pub size: Option<u64>,
    pub mtime_ms: Option<i64>,
    pub children: Vec<TreeNode>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct FileSummary {
    pub file_id: String,
    pub path: String,
    pub name: String,
    pub size: u64,
    pub mtime_ms: Option<i64>,
}

/// 嵌入 `![[…]]` 的解析结果。笔记嵌入只给出指向哪一篇——正文由调用方另取。
#[derive(Debug, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EmbedTarget {
    Missing,
    File {
        path: String,
        file_id: String,
        link: String,
        image: bool,
    },
    Note {
        path: String,
        file_id: String,
    },
}

fn is_note(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".md")
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

impl VaultView {
    /// 全量清单：替换掉此前的一切。
    pub fn reset(&mut self, epoch: String, at_seq: i64, changes: Vec<Change>) {
        self.epoch = epoch;
        self.head_seq = at_seq;
        self.files.clear();
        self.apply(changes);
    }

    /// 增量变更（`changes?since=`）：同一文件只认更新的 rev。返回变了的 `file_id`。
    pub fn apply(&mut self, changes: Vec<Change>) -> Vec<String> {
        let mut changed = Vec::new();
        for change in changes {
            match change {
                Change::Put {
                    file_id,
                    rev,
                    entry,
                    size_padded,
                } => {
                    if self.files.get(&file_id).is_some_and(|head| head.rev >= rev) {
                        continue;
                    }
                    self.files.insert(
                        file_id,
                        FileHead {
                            rev,
                            entry,
                            size_padded,
                        },
                    );
                    changed.push(file_id.to_string());
                }
                Change::Delete { file_id, rev } => {
                    if self.files.get(&file_id).is_some_and(|head| head.rev < rev) {
                        self.files.remove(&file_id);
                        changed.push(file_id.to_string());
                    }
                }
            }
        }
        changed
    }

    pub fn set_head_seq(&mut self, seq: i64) {
        self.head_seq = self.head_seq.max(seq);
    }

    #[must_use]
    pub fn epoch(&self) -> &str {
        &self.epoch
    }

    #[must_use]
    pub fn head_seq(&self) -> i64 {
        self.head_seq
    }

    #[must_use]
    pub fn head(&self, file_id: &Uuid) -> Option<&FileHead> {
        self.files.get(file_id)
    }

    /// 用户看得见的文件（隐藏路径与桌面同一口径：任一段以 `.` 开头的不是用户内容）。
    fn visible(&self) -> impl Iterator<Item = (&Uuid, &FileHead)> {
        self.files
            .iter()
            .filter(|(_, head)| !is_hidden_rel(&head.entry.path))
    }

    fn summary(file_id: &Uuid, head: &FileHead) -> FileSummary {
        FileSummary {
            file_id: file_id.to_string(),
            path: head.entry.path.clone(),
            name: file_name(&head.entry.path).to_owned(),
            size: head.entry.size,
            mtime_ms: head.entry.mtime_ms,
        }
    }

    /// 路径树（目录由路径推出：空文件夹不同步，PRD-21-011）。同层按路径字典序给出，显示顺序由宿主
    /// 用与桌面同一个排序函数排。
    #[must_use]
    pub fn tree(&self) -> Vec<TreeNode> {
        #[derive(Default)]
        struct Dir {
            dirs: BTreeMap<String, Dir>,
            files: BTreeMap<String, (Uuid, u64, Option<i64>)>,
        }
        let mut root = Dir::default();
        for (file_id, head) in self.visible() {
            let path = &head.entry.path;
            let mut parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
            let Some(name) = parts.pop() else { continue };
            let mut dir = &mut root;
            for part in parts {
                dir = dir.dirs.entry(part.to_owned()).or_default();
            }
            dir.files.insert(
                name.to_owned(),
                (*file_id, head.entry.size, head.entry.mtime_ms),
            );
        }
        fn emit(dir: &Dir, prefix: &str) -> Vec<TreeNode> {
            let join = |name: &str| {
                if prefix.is_empty() {
                    name.to_owned()
                } else {
                    format!("{prefix}/{name}")
                }
            };
            let mut out: Vec<TreeNode> = dir
                .dirs
                .iter()
                .map(|(name, child)| {
                    let path = join(name);
                    TreeNode {
                        name: name.clone(),
                        children: emit(child, &path),
                        path,
                        is_dir: true,
                        file_id: None,
                        size: None,
                        mtime_ms: None,
                    }
                })
                .collect();
            out.extend(
                dir.files
                    .iter()
                    .map(|(name, (file_id, size, mtime))| TreeNode {
                        name: name.clone(),
                        path: join(name),
                        is_dir: false,
                        file_id: Some(file_id.to_string()),
                        size: Some(*size),
                        mtime_ms: *mtime,
                        children: Vec::new(),
                    }),
            );
            out
        }
        emit(&root, "")
    }

    /// 「最近」：笔记按修改时间倒序（PRD-21-011）；修改时间缺席的排在最后。
    #[must_use]
    pub fn recent(&self, limit: usize) -> Vec<FileSummary> {
        let mut notes: Vec<(&Uuid, &FileHead)> = self
            .visible()
            .filter(|(_, head)| is_note(&head.entry.path))
            .collect();
        notes.sort_by(|a, b| {
            b.1.entry
                .mtime_ms
                .cmp(&a.1.entry.mtime_ms)
                .then_with(|| a.1.entry.path.cmp(&b.1.entry.path))
        });
        notes
            .into_iter()
            .take(limit)
            .map(|(id, head)| Self::summary(id, head))
            .collect()
    }

    /// 标题与路径搜索（PRD-21-040）：名字比较键（NFC + 小写）上的包含匹配，与桌面 `[[` 补全同一套
    /// 排序——标题以它开头的排前、短的排前、层级浅的排前，最后按路径。
    #[must_use]
    pub fn search(&self, query: &str, limit: usize) -> Vec<FileSummary> {
        let wanted = name_key(query.trim());
        if wanted.is_empty() {
            return Vec::new();
        }
        let mut hits: Vec<(&Uuid, &FileHead, String)> = self
            .visible()
            .filter_map(|(id, head)| {
                let title = name_key(&title_of(&head.entry.path));
                (title.contains(&wanted) || name_key(&head.entry.path).contains(&wanted))
                    .then_some((id, head, title))
            })
            .collect();
        hits.sort_by(|a, b| {
            let key = |hit: &(&Uuid, &FileHead, String)| {
                (
                    !hit.2.starts_with(&wanted),
                    hit.2.chars().count(),
                    hit.1.entry.path.matches('/').count(),
                    hit.1.entry.path.clone(),
                )
            };
            key(a).cmp(&key(b))
        });
        hits.into_iter()
            .take(limit)
            .map(|(id, head, _)| Self::summary(id, head))
            .collect()
    }

    /// 这个路径是不是已被别的文件占着（名字比较键相等即撞名，ADR-148 ⑥；`except` 是要写入的那个文件
    /// 自己）。隐藏文件同样算占用。
    #[must_use]
    pub fn path_taken(&self, path: &str, except: Option<&Uuid>) -> bool {
        let wanted = name_key(path);
        self.files
            .iter()
            .any(|(id, head)| Some(id) != except && name_key(&head.entry.path) == wanted)
    }

    /// 按库内路径找文件（名字比较键相等）。
    #[must_use]
    pub fn by_path(&self, path: &str) -> Option<FileSummary> {
        let wanted = name_key(path);
        self.visible()
            .find(|(_, head)| name_key(&head.entry.path) == wanted)
            .map(|(id, head)| Self::summary(id, head))
    }

    fn note_rows(&self) -> Vec<(String, String)> {
        self.visible()
            .filter(|(_, head)| is_note(&head.entry.path))
            .map(|(_, head)| (head.entry.path.clone(), title_of(&head.entry.path)))
            .collect()
    }

    /// `[[` 补全候选（与桌面同一个 [`links::wikilink_candidates`]；网页上没有正文，别名不参与）。
    #[must_use]
    pub fn wikilink_candidates(
        &self,
        src: &str,
        query: &str,
        limit: usize,
    ) -> Vec<WikiLinkCandidate> {
        links::wikilink_candidates(&self.note_rows(), &[], src, query, limit)
    }

    /// 一批 wikilink 目标各指向哪一篇（与桌面 `resolve_wikilinks` 同一个 [`LinkIndex`]；网页上没有
    /// 笔记正文，frontmatter 别名不参与）。只有片段的 `[[#标题]]` 指向本篇。
    #[must_use]
    pub fn resolve_wikilinks(&self, src: &str, targets: &[String]) -> Vec<Option<String>> {
        let rows = self.note_rows();
        let index = LinkIndex::from_rows(&rows, &[]);
        targets
            .iter()
            .map(|target| match split_wikilink_target(target).0 {
                "" => Some(src.to_owned()),
                note => index.resolve(src, note).map(str::to_owned),
            })
            .collect()
    }

    /// 一条标准 md 链接（`[标题](../甲.md#片段)`）指向哪一篇；片段在网页上不定位。
    #[must_use]
    pub fn resolve_link(&self, src: &str, href: &str) -> Option<String> {
        let target = href.split_once('#').map_or(href, |(path, _)| path);
        if target.trim().is_empty() {
            return None;
        }
        let rows = self.note_rows();
        let index = LinkIndex::from_rows(&rows, &[]);
        index
            .resolve(src, target)
            .map(str::to_owned)
            // 笔记相对的附件链接（`[报告](../附件/x.pdf)`）：指向库里的文件就打开它的附件页。
            .or_else(|| self.resolve_asset(src, target).map(|file| file.path))
    }

    /// 笔记相对的图片 / 附件地址（`![](assets/x.png)`、`../附件/x.png`）→ 那个文件。越出库根、
    /// 指不到都为 `None`（与桌面「笔记目录 + 相对路径，规范化后必须仍在库内」同一口径）。
    #[must_use]
    pub fn resolve_asset(&self, note: &str, src: &str) -> Option<FileSummary> {
        let decoded = percent_decode(src.trim());
        let path = normalize_vault_rel(parent_dir(note), &decoded)?;
        self.by_path(&path)
    }

    /// 一批嵌入目标各指向什么：附件按桌面同一套「先相对、再库根、再按文件名」找，笔记按 wikilink 解析。
    #[must_use]
    pub fn resolve_embeds(&self, src: &str, targets: &[String]) -> Vec<EmbedTarget> {
        let rows = self.note_rows();
        let index = LinkIndex::from_rows(&rows, &[]);
        let files: Vec<String> = self
            .visible()
            .map(|(_, head)| head.entry.path.clone())
            .collect();
        targets
            .iter()
            .map(|target| {
                let (note, _) = split_wikilink_target(target.trim());
                let note = note.trim();
                match extension_of(note) {
                    Some(ext) if ext != "md" => find_file(&files, src, note)
                        .and_then(|path| self.by_path(&path))
                        .map_or(EmbedTarget::Missing, |hit| EmbedTarget::File {
                            link: relative_link_path(src, &hit.path),
                            image: IMAGE_EXTENSIONS.contains(&ext.as_str()),
                            path: hit.path,
                            file_id: hit.file_id,
                        }),
                    _ => {
                        let path = if note.is_empty() {
                            Some(src.to_owned())
                        } else {
                            index.resolve(src, note).map(str::to_owned)
                        };
                        path.and_then(|path| self.by_path(&path)).map_or(
                            EmbedTarget::Missing,
                            |hit| EmbedTarget::Note {
                                path: hit.path,
                                file_id: hit.file_id,
                            },
                        )
                    }
                }
            })
            .collect()
    }
}

impl VaultView {
    /// 用户看得见的笔记（`.md`，隐藏路径之外）：全文索引与标签的范围（TDD-17 §5.6）。
    pub fn notes(&self) -> impl Iterator<Item = (&Uuid, &FileHead)> {
        self.visible().filter(|(_, head)| is_note(&head.entry.path))
    }

    /// 按路径找文件，**含隐藏路径**（库级设置 `.luna/settings.toml` 这类机器文件）；名字比较键相等。
    #[must_use]
    pub fn by_path_any(&self, path: &str) -> Option<(&Uuid, &FileHead)> {
        let wanted = name_key(path);
        self.files
            .iter()
            .find(|(_, head)| name_key(&head.entry.path) == wanted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, mtime: i64) -> EntryPlain {
        EntryPlain {
            v: 1,
            path: path.to_owned(),
            size: 10,
            mtime_ms: Some(mtime),
            content_hash: String::new(),
            chunks: Vec::new(),
        }
    }

    fn view(paths: &[&str]) -> (VaultView, Vec<Uuid>) {
        let mut view = VaultView::default();
        let ids: Vec<Uuid> = paths.iter().map(|_| Uuid::now_v7()).collect();
        let changes = paths
            .iter()
            .zip(&ids)
            .enumerate()
            .map(|(i, (path, id))| Change::Put {
                file_id: *id,
                rev: 1,
                entry: entry(path, i64::try_from(i).unwrap()),
                size_padded: 4096,
            })
            .collect();
        view.reset("e1".into(), 7, changes);
        (view, ids)
    }

    #[test]
    fn the_tree_hides_dot_paths_and_nests_folders() {
        let (view, _) = view(&[
            "工作/周报.md",
            ".luna/bookmarks.json",
            "根.md",
            "工作/子/图.png",
        ]);
        let tree = view.tree();
        let names: Vec<&str> = tree.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, ["工作", "根.md"]);
        assert_eq!(tree[0].children[0].path, "工作/子");
        assert_eq!(tree[0].children[0].children[0].path, "工作/子/图.png");
        assert_eq!(tree[0].children[1].path, "工作/周报.md");
    }

    #[test]
    fn newer_revs_win_and_tombstones_remove() {
        let (mut view, ids) = view(&["a.md"]);
        assert!(view
            .apply(vec![Change::Put {
                file_id: ids[0],
                rev: 1,
                entry: entry("旧.md", 0),
                size_padded: 4096,
            }])
            .is_empty());
        assert_eq!(
            view.apply(vec![Change::Delete {
                file_id: ids[0],
                rev: 2
            }]),
            [ids[0].to_string()]
        );
        assert!(view.tree().is_empty());
    }

    #[test]
    fn search_matches_titles_and_paths_by_name_key_with_prefix_first() {
        let (view, _) = view(&[
            "归档/周报 Week.md",
            "周报.md",
            "资料/WEEK 计划.md",
            "其他.md",
        ]);
        let hits: Vec<String> = view
            .search("week", 10)
            .into_iter()
            .map(|h| h.path)
            .collect();
        assert_eq!(hits, ["资料/WEEK 计划.md", "归档/周报 Week.md"]);
        let hits: Vec<String> = view
            .search("周报", 10)
            .into_iter()
            .map(|h| h.path)
            .collect();
        assert_eq!(hits, ["周报.md", "归档/周报 Week.md"]);
        assert!(view.search("  ", 10).is_empty());
    }

    #[test]
    fn paths_collide_by_name_key_except_for_the_file_itself() {
        let (view, ids) = view(&["工作/周报.md", ".luna/x.json"]);
        assert!(view.path_taken("工作/周报.MD", None));
        assert!(view.path_taken("工作/周报.md", Some(&Uuid::now_v7())));
        assert!(!view.path_taken("工作/周报.md", Some(&ids[0])));
        assert!(
            view.path_taken(".LUNA/x.json", None),
            "隐藏文件同样占着路径"
        );
        assert!(!view.path_taken("工作/月报.md", None));
    }

    #[test]
    fn recent_lists_notes_newest_first() {
        let (view, _) = view(&["a.md", "b.png", "c.md"]);
        let hits: Vec<String> = view.recent(10).into_iter().map(|h| h.path).collect();
        assert_eq!(hits, ["c.md", "a.md"]);
    }

    #[test]
    fn links_resolve_with_the_desktop_rules() {
        let (view, _) = view(&[
            "笔记/甲.md",
            "笔记/乙.md",
            "乙.md",
            "附件/图 1.png",
            "笔记/附件/图.png",
        ]);
        assert_eq!(
            view.resolve_wikilinks("笔记/甲.md", &["乙".into(), "#标题".into(), "丙".into()]),
            [Some("笔记/乙.md".into()), Some("笔记/甲.md".into()), None]
        );
        assert_eq!(
            view.resolve_link("笔记/甲.md", "../乙.md#L3").as_deref(),
            Some("乙.md")
        );
        assert_eq!(
            view.resolve_link("笔记/甲.md", "../附件/图%201.png")
                .as_deref(),
            Some("附件/图 1.png")
        );
        assert_eq!(view.resolve_link("笔记/甲.md", "../附件/没有.png"), None);
        assert_eq!(
            view.resolve_asset("笔记/甲.md", "../附件/图%201.png")
                .map(|h| h.path)
                .as_deref(),
            Some("附件/图 1.png")
        );
        assert!(view.resolve_asset("笔记/甲.md", "../../外面.png").is_none());
        let embeds = view.resolve_embeds(
            "笔记/甲.md",
            &["图.png".into(), "乙".into(), "无.png".into()],
        );
        assert!(
            matches!(&embeds[0], EmbedTarget::File { path, image: true, .. } if path == "笔记/附件/图.png")
        );
        assert!(matches!(&embeds[1], EmbedTarget::Note { path, .. } if path == "笔记/乙.md"));
        assert_eq!(embeds[2], EmbedTarget::Missing);
    }
}
