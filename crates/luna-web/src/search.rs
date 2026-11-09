//! 网页版的全文搜索与标签（PRD-21-041 / 042，TDD-17 §5.6）：第一次用时 Worker 把全部 `.md` 正文下载下来交给
//! 这里，在 WASM 内存里建索引；索引不落盘，关闭页面即丢弃。
//!
//! 判定一律复用桌面那一份：查询语法、哪些词算命中、命中落在哪一行哪几个字（`luna_sync_core::search`）；标签怎么
//! 读、`tag:` 过滤器盖住哪些标签（`luna_sync_core::tags`）。桌面的 SQLite 粗筛在这里换成内存里的折叠副本——
//! 它只负责「可能命中」，命中与片段由 `match_note` 对着原文定。

use std::collections::{BTreeMap, HashMap};

use luna_sync_core::links::title_of;
use luna_sync_core::search::{
    fold, match_note, Filter, FilterField, LineMatch, SearchQuery, MAX_LINES_PER_NOTE,
    MAX_RESULT_NOTES,
};
use luna_sync_core::tags::{extract_tags, tag_covers, TagSource, TaggedTerm};
use serde::Serialize;
use serde_json::json;
use uuid::Uuid;
use wasm_bindgen::prelude::*;

use crate::view::{FileSummary, VaultView};
use crate::{code, parse_uuid, VaultSession};

/// 索引里的一篇：它是哪一版、正文、检索用的折叠副本与标签。
#[derive(Debug)]
struct IndexedNote {
    rev: i64,
    path: String,
    size: u64,
    mtime_ms: Option<i64>,
    content_hash: String,
    text: String,
    /// 整篇的折叠副本（不分大小写）：粗筛用，命中与否最终由 `match_note` 定。
    folded: String,
    tags: Vec<TaggedTerm>,
}

impl IndexedNote {
    fn summary(&self, file_id: &Uuid) -> FileSummary {
        FileSummary {
            file_id: file_id.to_string(),
            path: self.path.clone(),
            name: self
                .path
                .rsplit('/')
                .next()
                .unwrap_or(&self.path)
                .to_owned(),
            size: self.size,
            mtime_ms: self.mtime_ms,
        }
    }
}

/// 一篇的命中。
#[derive(Debug, Serialize, PartialEq)]
pub struct NoteHit {
    pub file_id: String,
    pub path: String,
    /// 文件名（含扩展名）。
    pub name: String,
    pub title: String,
    pub mtime_ms: Option<i64>,
    pub match_count: u32,
    pub lines: Vec<LineMatch>,
    pub lines_truncated: bool,
}

/// 一次搜索的结果。
#[derive(Debug, Default, Serialize, PartialEq)]
pub struct SearchHits {
    pub notes: Vec<NoteHit>,
    pub total_matches: u32,
    /// 命中的笔记超过上限，只给了最近的那些。
    pub truncated: bool,
}

/// 标签列表的一行（与桌面标签面板同一种归并：不分 ASCII 大小写、计笔记数）。
#[derive(Debug, Serialize, PartialEq)]
pub struct TagCount {
    pub tag: String,
    pub count: u32,
    /// 只有 AI 标签（`luna.ai_tags`）用到它。
    pub ai_only: bool,
}

/// 内存里的全文索引（纯逻辑；`wasm_bindgen` 外壳见 [`TextIndex`]）。
#[derive(Debug, Default)]
pub struct Index {
    notes: HashMap<Uuid, IndexedNote>,
}

impl Index {
    /// 对照此刻的清单：去掉已不在的笔记；只换了路径（改名、移动）的就地更新；返回要（重新）下载正文的笔记，
    /// 小的在前（进度先走得动）。
    pub fn sync(&mut self, view: &VaultView) -> Vec<(Uuid, u64)> {
        let mut alive: HashMap<Uuid, ()> = HashMap::new();
        let mut wanted = Vec::new();
        for (id, head) in view.notes() {
            alive.insert(*id, ());
            match self.notes.get_mut(id) {
                Some(note) if note.content_hash == head.entry.content_hash => {
                    note.rev = head.rev;
                    note.path.clone_from(&head.entry.path);
                    note.mtime_ms = head.entry.mtime_ms;
                }
                _ => wanted.push((*id, head.entry.size)),
            }
        }
        self.notes.retain(|id, _| alive.contains_key(id));
        wanted.sort_by_key(|(id, size)| (*size, *id));
        wanted
    }

    /// 记下一篇的正文（这一版的头取自清单）；这篇已不在清单里就不记。
    pub fn add(&mut self, view: &VaultView, file_id: Uuid, text: String) {
        let Some(head) = view.head(&file_id) else {
            return;
        };
        let tags = extract_tags(&text);
        self.notes.insert(
            file_id,
            IndexedNote {
                rev: head.rev,
                path: head.entry.path.clone(),
                size: head.entry.size,
                mtime_ms: head.entry.mtime_ms,
                content_hash: head.entry.content_hash.clone(),
                folded: fold(&text, false),
                text,
                tags,
            },
        );
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.notes.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
    }

    /// 最近修改的在前，再按路径。
    fn newest_first(&self) -> Vec<(&Uuid, &IndexedNote)> {
        let mut notes: Vec<(&Uuid, &IndexedNote)> = self.notes.iter().collect();
        notes.sort_by(|a, b| {
            b.1.mtime_ms
                .cmp(&a.1.mtime_ms)
                .then_with(|| a.1.path.cmp(&b.1.path))
        });
        notes
    }

    /// 全文搜索：与桌面搜索视图同一套语法与命中定位；结果按修改时间倒序（桌面的候选同样按修改时间取）。
    /// `type:` 过滤器网页上不认（笔记类型的判定在桌面的库里），按没写处理。
    #[must_use]
    pub fn search(&self, input: &str, match_case: bool) -> SearchHits {
        let query = SearchQuery::parse(input, match_case);
        if query.is_empty() {
            return SearchHits::default();
        }
        let needles: Vec<String> = query.positive_terms().map(|t| fold(t, false)).collect();
        let mut hits = SearchHits::default();
        for (id, note) in self.newest_first() {
            if !needles
                .iter()
                .all(|needle| note.folded.contains(needle.as_str()))
            {
                continue;
            }
            if !query
                .filters
                .iter()
                .all(|filter| filter_admits(filter, note))
            {
                continue;
            }
            let Some(body) = match_note(&note.text, &query, MAX_LINES_PER_NOTE) else {
                continue;
            };
            if hits.notes.len() >= MAX_RESULT_NOTES {
                hits.truncated = true;
                break;
            }
            hits.total_matches += body.match_count;
            hits.notes.push(NoteHit {
                file_id: id.to_string(),
                path: note.path.clone(),
                name: note
                    .path
                    .rsplit('/')
                    .next()
                    .unwrap_or(&note.path)
                    .to_owned(),
                title: title_of(&note.path),
                mtime_ms: note.mtime_ms,
                match_count: body.match_count,
                lines: body.lines,
                lines_truncated: body.lines_truncated,
            });
        }
        hits
    }

    /// 全库标签与各自的笔记数：不分 ASCII 大小写归成一组，显示组里（按字节）最大的那种写法，笔记多的在前、
    /// 再按标签——与桌面标签面板同一种归并。
    #[must_use]
    pub fn tags(&self) -> Vec<TagCount> {
        let mut groups: BTreeMap<String, (String, u32, bool)> = BTreeMap::new();
        for note in self.notes.values() {
            for term in &note.tags {
                let group = groups
                    .entry(term.tag.to_ascii_lowercase())
                    .or_insert_with(|| (term.tag.clone(), 0, false));
                if term.tag > group.0 {
                    group.0.clone_from(&term.tag);
                }
                group.1 += 1;
                group.2 |= term.source == TagSource::User;
            }
        }
        let mut out: Vec<TagCount> = groups
            .into_values()
            .map(|(tag, count, user)| TagCount {
                tag,
                count,
                ai_only: !user,
            })
            .collect();
        out.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.tag.cmp(&b.tag)));
        out
    }

    /// 带这个标签（含它的子标签：`工作` 也列出 `工作/周报`）的笔记，最近修改的在前。
    #[must_use]
    pub fn tag_notes(&self, tag: &str) -> Vec<FileSummary> {
        self.newest_first()
            .into_iter()
            .filter(|(_, note)| note.tags.iter().any(|term| tag_covers(tag, &term.tag)))
            .map(|(id, note)| note.summary(id))
            .collect()
    }
}

/// 一条过滤器放不放行这一篇（与桌面 SQL 里的口径一致：路径 / 文件名是不分大小写的包含，标签含子标签）。
fn filter_admits(filter: &Filter, note: &IndexedNote) -> bool {
    let wanted = filter.value.to_lowercase();
    let hit = match filter.field {
        FilterField::Path => note.path.to_lowercase().contains(&wanted),
        FilterField::File => title_of(&note.path).to_lowercase().contains(&wanted),
        FilterField::Tag => note
            .tags
            .iter()
            .any(|term| tag_covers(&filter.value, &term.tag)),
        FilterField::Type => return true,
    };
    hit != filter.negated
}

/// 全文索引（Worker 里每个打开的库一份；`free()` 即丢弃）。
#[wasm_bindgen]
#[derive(Default)]
pub struct TextIndex(Index);

#[wasm_bindgen]
impl TextIndex {
    #[wasm_bindgen(constructor)]
    #[must_use]
    pub fn new() -> TextIndex {
        Self::default()
    }

    /// 对照此刻的清单，返回要下载正文的笔记：`[{ file_id, size }]` JSON。
    #[must_use]
    pub fn sync(&mut self, session: &VaultSession) -> String {
        let wanted: Vec<serde_json::Value> = self
            .0
            .sync(&session.view)
            .into_iter()
            .map(|(id, size)| json!({ "file_id": id.to_string(), "size": size }))
            .collect();
        json!(wanted).to_string()
    }

    /// 记下一篇的正文。
    ///
    /// # Errors
    /// `invalid_id`。
    pub fn add(
        &mut self,
        session: &VaultSession,
        file_id: &str,
        text: String,
    ) -> Result<(), JsError> {
        self.0.add(&session.view, parse_uuid(file_id)?, text);
        Ok(())
    }

    /// 已索引的篇数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// 全文搜索：JSON（见 [`SearchHits`]）。
    #[must_use]
    pub fn search(&self, query: &str, match_case: bool) -> String {
        json!(self.0.search(query, match_case)).to_string()
    }

    /// 标签列表：JSON（见 [`TagCount`]）。
    #[must_use]
    pub fn tags(&self) -> String {
        json!(self.0.tags()).to_string()
    }

    /// 带这个标签的笔记：`FileSummary` 的 JSON 数组。
    ///
    /// # Errors
    /// `invalid_tag`（空的）。
    pub fn tag_notes(&self, tag: &str) -> Result<String, JsError> {
        if tag.trim_start_matches('#').trim().is_empty() {
            return Err(code("invalid_tag"));
        }
        Ok(json!(self.0.tag_notes(tag)).to_string())
    }
}

#[wasm_bindgen]
impl VaultSession {
    /// 为这个库新建一份空的全文索引。
    #[must_use]
    pub fn new_text_index(&self) -> TextIndex {
        TextIndex::new()
    }
}

/// 一篇笔记的标签（与桌面派生索引同一读法）：`[{ tag, source }]` JSON，`source` 为 `user` / `ai`。互通测试用。
#[wasm_bindgen]
#[must_use]
pub fn note_tags(content: &str) -> String {
    let terms: Vec<serde_json::Value> = extract_tags(content)
        .into_iter()
        .map(|term| json!({ "tag": term.tag, "source": term.source.key() }))
        .collect();
    json!(terms).to_string()
}

/// 一篇笔记里的命中：`null` 或 `{ match_count, lines }` JSON。互通测试用。
#[wasm_bindgen]
#[must_use]
pub fn match_note_json(content: &str, query: &str, match_case: bool) -> String {
    match match_note(
        content,
        &SearchQuery::parse(query, match_case),
        MAX_LINES_PER_NOTE,
    ) {
        Some(body) => json!({ "match_count": body.match_count, "lines": body.lines }).to_string(),
        None => "null".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::Change;
    use luna_sync_core::manifest::EntryPlain;

    fn entry(path: &str, mtime: i64, hash: &str) -> EntryPlain {
        EntryPlain {
            v: 1,
            path: path.to_owned(),
            size: 10,
            mtime_ms: Some(mtime),
            content_hash: hash.to_owned(),
            chunks: Vec::new(),
        }
    }

    fn put(id: Uuid, rev: i64, path: &str, mtime: i64, hash: &str) -> Change {
        Change::Put {
            file_id: id,
            rev,
            entry: entry(path, mtime, hash),
            size_padded: 4096,
        }
    }

    #[test]
    fn the_index_follows_the_manifest_and_searches_with_the_desktop_rules() {
        let ids: Vec<Uuid> = (0..4).map(|_| Uuid::now_v7()).collect();
        let mut view = VaultView::default();
        view.reset(
            "e".into(),
            1,
            vec![
                put(ids[0], 1, "周报.md", 3, "a"),
                put(ids[1], 1, "韩文/인사.md", 2, "b"),
                put(ids[2], 1, "图.png", 1, "c"),
                put(ids[3], 1, ".luna/x.md", 1, "d"),
            ],
        );
        let mut index = Index::default();
        let wanted: Vec<Uuid> = index.sync(&view).into_iter().map(|(id, _)| id).collect();
        assert_eq!(wanted.len(), 2, "只要看得见的笔记：{wanted:?}");
        index.add(
            &view,
            ids[0],
            "---\ntags: [工作]\n---\n本周完成了网页版骨架 #工作/周报\n".into(),
        );
        index.add(&view, ids[1], "안녕하세요 #한국어\n회의 기록\n".into());
        assert!(index.sync(&view).is_empty());

        let hits = index.search("骨架", false);
        assert_eq!(hits.notes.len(), 1);
        assert_eq!(hits.notes[0].path, "周报.md");
        assert_eq!(hits.notes[0].lines[0].line, 4);
        assert_eq!(index.search("안녕", false).notes[0].path, "韩文/인사.md");
        assert!(
            index.search("tags", false).notes.is_empty(),
            "frontmatter 不搜"
        );
        assert_eq!(index.search("骨架 tag:工作", false).notes.len(), 1);
        assert!(index.search("骨架 -tag:工作", false).notes.is_empty());
        assert_eq!(index.search("기록 path:韩文", false).notes.len(), 1);

        let tags = index.tags();
        assert_eq!(
            tags.iter().map(|t| t.tag.as_str()).collect::<Vec<_>>(),
            ["工作", "工作/周报", "한국어"]
        );
        assert_eq!(index.tag_notes("工作").len(), 1);
        assert_eq!(index.tag_notes("한국어")[0].path, "韩文/인사.md");

        // 改名（内容哈希不变）就地更新路径，不重新下载；内容变了才要重新下载；删掉的就丢掉。
        view.apply(vec![
            put(ids[0], 2, "归档/周报.md", 3, "a"),
            put(ids[1], 2, "韩文/인사.md", 4, "b2"),
            Change::Delete {
                file_id: ids[1],
                rev: 3,
            },
        ]);
        assert!(index.sync(&view).is_empty());
        assert_eq!(index.search("骨架", false).notes[0].path, "归档/周报.md");
        assert_eq!(index.len(), 1);
    }

    #[test]
    fn tags_group_case_insensitively_like_the_desktop_panel() {
        let ids: Vec<Uuid> = (0..2).map(|_| Uuid::now_v7()).collect();
        let mut view = VaultView::default();
        view.reset(
            "e".into(),
            1,
            vec![
                put(ids[0], 1, "a.md", 1, "a"),
                put(ids[1], 1, "b.md", 2, "b"),
            ],
        );
        let mut index = Index::default();
        index.add(&view, ids[0], "#Data_Security".into());
        index.add(
            &view,
            ids[1],
            "---\nluna:\n  ai_tags: [data_security, 会议]\n---\n".into(),
        );
        let tags = index.tags();
        assert_eq!(tags[0].tag, "data_security");
        assert_eq!(tags[0].count, 2);
        assert!(!tags[0].ai_only);
        assert_eq!(tags[1].tag, "会议");
        assert!(tags[1].ai_only);
    }
}
