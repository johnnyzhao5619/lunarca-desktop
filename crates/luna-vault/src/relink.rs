//! 改名重构：按「指向谁」改写链接（PRD-06-030 2026-10-04 修订，TDD-06 §3 改名重构）。
//!
//! 此前的改写器按「文件名 == 旧名」改 wikilink、按路径全等改 md 链接，与图谱 / 点击用的
//! [`luna_sync_core::links::LinkIndex`] 是两套口径：库里有两篇 `foo` 时改一篇会把指向另一篇的链接
//! 一起改掉；同名移动不改写，链接却可能悄悄改指别处；附件改名只认 `](旧)` 一种写法。
//!
//! 这里只有一条规则：**移动前解析一次、移动后再解析一次，指向变了或名字变了的才改**。
//! - 解析与点击、反链同一份（笔记走 `LinkIndex`，附件走嵌入的按文件名找）；
//! - 断链（移动前就指不到）一律不动；
//! - 指向没变的链接只在「目标被改了名」时才换成新名字（`[[foo]]` → `[[Foo]]`），否则一个
//!   字节都不碰；裸名会改指别处时写成能指回原处的路径（`[[目录/foo]]`）；
//! - 只换目标那一段：片段、`|` 别名、md 标题与尖括号原样保留（区间来自 [`scan_links`]）。
//!

use crate::parse::{scan_links, LinkKind};
use crate::pathutil::encode_link_target;
use crate::vault::PathPairs;
use luna_sync_core::links::title_of;
use luna_sync_core::links::LinkIndex;
use luna_sync_core::links::{canonical_link_target, percent_decode, relative_link_path};
use std::collections::{HashMap, HashSet};

/// 一次移动前（或后）的库：笔记（路径, 标题）、别名（路径, 别名）与全部文件。
pub(crate) struct LinkWorld {
    notes: PathPairs,
    aliases: PathPairs,
    files: Vec<String>,
}

impl LinkWorld {
    pub(crate) fn new(notes: PathPairs, aliases: PathPairs, files: Vec<String>) -> Self {
        Self {
            notes,
            aliases,
            files,
        }
    }

    /// 移动**已经发生**之后反推移动前的库（库外改名，PRD-06-030 ⑤）：把这批移动倒着套一遍。
    pub(crate) fn reverted(&self, moves: &Moves) -> Self {
        self.moved(&Moves::new(
            moves
                .pairs()
                .iter()
                .map(|(old, new)| (new.clone(), old.clone())),
        ))
    }

    /// 把一批移动套上去：路径换新、标题跟新文件名走。
    fn moved(&self, moves: &Moves) -> Self {
        Self {
            notes: self
                .notes
                .iter()
                .map(|(path, title)| match moves.forward(path) {
                    Some(new) => (new.to_owned(), title_of(new)),
                    None => (path.clone(), title.clone()),
                })
                .collect(),
            aliases: self
                .aliases
                .iter()
                .map(|(path, alias)| {
                    (
                        moves.forward(path).unwrap_or(path).to_owned(),
                        alias.clone(),
                    )
                })
                .collect(),
            files: self
                .files
                .iter()
                .map(|path| moves.forward(path).unwrap_or(path).to_owned())
                .collect(),
        }
    }
}

/// 一批移动（旧路径 → 新路径，笔记与附件都在内）。查表不分大小写：链接解析不分。
pub(crate) struct Moves {
    pairs: Vec<(String, String)>,
    forward: HashMap<String, String>,
}

impl Moves {
    pub(crate) fn new(pairs: impl IntoIterator<Item = (String, String)>) -> Self {
        let pairs: Vec<(String, String)> = pairs.into_iter().collect();
        let forward = pairs
            .iter()
            .map(|(from, to)| (from.to_lowercase(), to.clone()))
            .collect();
        Self { pairs, forward }
    }

    /// 这批移动（旧 → 新）。
    pub(crate) fn pairs(&self) -> &[(String, String)] {
        &self.pairs
    }

    fn forward(&self, old: &str) -> Option<&str> {
        self.forward.get(&old.to_lowercase()).map(String::as_str)
    }
}

/// 不是笔记的文件扩展名（`[[v1.0 方案]]` 这种含点笔记名不算，与图谱同一判法）。
fn file_extension(name: &str) -> Option<&str> {
    let file = name.rsplit('/').next().unwrap_or(name);
    let (stem, ext) = file.rsplit_once('.')?;
    let ext_like = !stem.is_empty()
        && (1..=6).contains(&ext.len())
        && ext.chars().all(|c| c.is_ascii_alphanumeric());
    (ext_like && !ext.eq_ignore_ascii_case("md")).then_some(ext)
}

fn strip_md(name: &str) -> &str {
    if name.len() > 3 && name[name.len() - 3..].eq_ignore_ascii_case(".md") {
        &name[..name.len() - 3]
    } else {
        name
    }
}

fn last_segment(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// 「这条链接指向哪个文件」的一份答案（一个库状态一份）。
struct Resolver<'a> {
    index: LinkIndex<'a>,
    files: &'a [String],
    by_lower: HashMap<String, &'a str>,
}

impl<'a> Resolver<'a> {
    fn new(world: &'a LinkWorld) -> Self {
        Self {
            index: LinkIndex::from_rows(&world.notes, &world.aliases),
            files: &world.files,
            by_lower: world
                .files
                .iter()
                .map(|path| (path.to_lowercase(), path.as_str()))
                .collect(),
        }
    }

    fn resolve(&self, src: &str, kind: LinkKind, written: &str) -> Option<String> {
        let written = written.trim();
        match kind {
            LinkKind::Wiki { .. } if file_extension(written).is_some() => {
                luna_sync_core::links::find_file(self.files, src, written)
            }
            LinkKind::Wiki { .. } => self.index.resolve(src, written).map(str::to_owned),
            // md 链接是**相对源笔记**的路径：先按源目录归一化找真实文件，库根同名文件不得抢先；
            // 不带扩展名的写法（`[x](foo)`）才回落到按名字认。
            LinkKind::Markdown => {
                let canonical = canonical_link_target(src, written);
                if let Some(path) = self.by_lower.get(&canonical.to_lowercase()) {
                    return Some((*path).to_owned());
                }
                let decoded = percent_decode(written);
                (file_extension(&decoded).is_none() && !decoded.to_lowercase().ends_with(".md"))
                    .then(|| self.index.resolve(src, written).map(str::to_owned))
                    .flatten()
            }
        }
    }
}

/// 这些笔记里的链接各指向哪个文件——与改名重构、点击、嵌入同一份解析（孤儿附件检测用，
/// PRD-06-012 ②）。`![[图.png]]` 按文件名全库找、md 链接按源笔记目录找，HTML 媒体标签的
/// `src` 按 md 链接口径认。`sources` 是「(按哪个路径解析, 正文)」：回收站与快照里的旧正文按
/// 它原来的位置解析。
pub(crate) fn resolved_targets<'c>(
    world: &LinkWorld,
    sources: impl IntoIterator<Item = (&'c str, &'c str)>,
) -> HashSet<String> {
    let resolver = Resolver::new(world);
    let mut out = HashSet::new();
    for (src, content) in sources {
        for link in scan_links(content) {
            if let Some(path) = resolver.resolve(src, link.kind, &content[link.target.clone()]) {
                out.insert(path);
            }
        }
        for written in crate::parse::html_media_sources(content) {
            if let Some(path) = resolver.resolve(src, LinkKind::Markdown, written) {
                out.insert(path);
            }
        }
    }
    out
}

/// 一条链接在移动后应写成什么；`None` = 不动。
struct Rewrite<'a> {
    old: &'a Resolver<'a>,
    new: &'a Resolver<'a>,
    old_src: &'a str,
    new_src: &'a str,
}

impl Rewrite<'_> {
    fn text(&self, kind: LinkKind, angle: bool, written: &str, moves: &Moves) -> Option<String> {
        let old_target = self.old.resolve(self.old_src, kind, written)?;
        let moved_target = moves.forward(&old_target);
        let new_target = moved_target.unwrap_or(&old_target);
        let target_moved = moved_target.is_some();
        // md 路径链接只在源或目标挪了位置时才可能失效；wikilink 按名字认，别人挪进来也会抢走它。
        if kind == LinkKind::Markdown && !target_moved && self.old_src == self.new_src {
            return None;
        }
        let still_points =
            self.new.resolve(self.new_src, kind, written).as_deref() == Some(new_target);
        match kind {
            LinkKind::Markdown => {
                let decoded = percent_decode(written.trim());
                let name_current = !target_moved
                    || strip_md(last_segment(&decoded)) == strip_md(last_segment(new_target));
                if still_points && name_current {
                    return None;
                }
                let mut path = relative_link_path(self.new_src, new_target);
                // 原写法省略了 `.md`（`[x](foo)`）就照样省略。
                if file_extension(new_target).is_none() && !decoded.to_lowercase().ends_with(".md")
                {
                    path = strip_md(&path).to_owned();
                }
                Some(if angle {
                    path
                } else {
                    encode_link_target(&path)
                })
            }
            LinkKind::Wiki { .. } => {
                self.wiki_text(written, &old_target, new_target, target_moved, still_points)
            }
        }
    }

    fn wiki_text(
        &self,
        written: &str,
        old_target: &str,
        new_target: &str,
        target_moved: bool,
        still_points: bool,
    ) -> Option<String> {
        let kind = LinkKind::Wiki { embed: false };
        let is_file = file_extension(new_target).is_some();
        let with_path = written.contains('/');
        let keeps_md = written.to_lowercase().ends_with(".md");
        let name_of = |path: &str| {
            if is_file {
                last_segment(path).to_owned()
            } else {
                title_of(path)
            }
        };
        let written_name = strip_md(last_segment(written));
        // 按别名写的（`[[例会]]` 指向「周会」）：别名不随改名变，只要还指得到就不碰。
        let by_alias = !with_path
            && !is_file
            && written_name.to_lowercase() != name_of(old_target).to_lowercase();
        if by_alias && still_points {
            return None;
        }
        let new_name = name_of(new_target);
        if still_points && (!target_moved || by_alias || written_name == strip_md(&new_name)) {
            return None;
        }
        let suffix = if keeps_md { ".md" } else { "" };
        let full_path = if is_file {
            new_target.to_owned()
        } else {
            format!("{}{suffix}", strip_md(new_target))
        };
        let mut candidates = Vec::with_capacity(3);
        if !with_path && !by_alias {
            candidates.push(format!("{}{suffix}", strip_md(&new_name)));
        }
        candidates.push(full_path);
        // 库根的笔记「路径」与名字同形（`foo`），按名字认会落到别处；带上 `.md` 才是精确路径。
        if !is_file {
            candidates.push(new_target.to_owned());
        }
        candidates.into_iter().find(|candidate| {
            self.new.resolve(self.new_src, kind, candidate).as_deref() == Some(new_target)
        })
    }
}

/// 一篇笔记在这批移动之后的新全文与改了几处；没有要改的为 `None`。
fn relink_note(content: &str, rewrite: &Rewrite<'_>, moves: &Moves) -> Option<(String, usize)> {
    let mut edits = Vec::new();
    for link in scan_links(content) {
        let written = &content[link.target.clone()];
        if let Some(text) = rewrite.text(link.kind, link.angle, written, moves) {
            if text != written {
                edits.push((link.target, text));
            }
        }
    }
    if edits.is_empty() {
        return None;
    }
    let count = edits.len();
    let mut out = content.to_owned();
    for (range, text) in edits.into_iter().rev() {
        out.replace_range(range, &text);
    }
    Some((out, count))
}

/// 一篇笔记的改写计划：移动之前算好、动磁盘之前就知道要写什么（TDD-06 §3 改名重构 ⑤）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PlannedRewrite {
    /// 移动前的路径（从这里读到 `before`）。
    pub old_rel: String,
    /// 移动后的路径（`after` 写到这里）。
    pub new_rel: String,
    pub before: String,
    pub after: String,
    /// 改了几处链接。
    pub links: usize,
}

impl crate::Vault {
    /// 这批移动可能牵动到的笔记（新路径）：被移动的笔记本身，加上链接末段名字等于某个被移动
    /// 文件的旧名、新名（别人挪进来可能抢走裸名）或其别名的笔记。改名不再逐篇读全库
    /// （TDD-06 §3 改名重构 ①）；索引查不出来时退回全库，宁慢不漏。
    fn relink_candidates(&self, moves: &Moves) -> crate::Result<Vec<String>> {
        let mut names = std::collections::HashSet::new();
        let mut moved_notes = std::collections::BTreeSet::new();
        for (old, new) in moves.pairs() {
            names.insert(crate::index::link_name_key(old));
            names.insert(crate::index::link_name_key(new));
            if old.to_lowercase().ends_with(".md") {
                moved_notes.insert(old.clone());
            }
        }
        let old_notes: Vec<String> = moved_notes.iter().cloned().collect();
        let sources = self.index().aliases_of(&old_notes).and_then(|aliases| {
            names.extend(aliases.iter().map(|alias| alias.trim().to_lowercase()));
            self.index().sources_linking_to_names(&names)
        });
        let Ok(sources) = sources else {
            return self.walk_markdown();
        };
        let mut out = moved_notes;
        out.extend(
            sources
                .into_iter()
                .filter(|src| self.root().join(src).is_file()),
        );
        Ok(out.into_iter().collect())
    }

    /// 移动之前的库（必须在动磁盘之前取：之后索引与文件都已是新的）。
    pub(crate) fn link_world(&self) -> crate::Result<LinkWorld> {
        let (notes, aliases) = self.link_rows()?;
        Ok(LinkWorld::new(notes, aliases, self.walk_files()?))
    }

    /// 这批移动会改写哪些笔记、各改成什么（**只读**，在动磁盘之前调用）。读的是移动前的
    /// 位置；系统维护的知识页按 [`crate::Vault::relink_allowed`] 跳过。
    pub(crate) fn plan_relink(
        &self,
        before: &LinkWorld,
        moves: &Moves,
    ) -> crate::Result<Vec<PlannedRewrite>> {
        self.plan_relink_reading(before, moves, false)
    }

    /// 同上，但移动**已经发生**（库外改名，PRD-06-030 ⑤）：被移动的笔记从新位置读。
    pub(crate) fn plan_relink_after_move(
        &self,
        before: &LinkWorld,
        moves: &Moves,
    ) -> crate::Result<Vec<PlannedRewrite>> {
        self.plan_relink_reading(before, moves, true)
    }

    fn plan_relink_reading(
        &self,
        before: &LinkWorld,
        moves: &Moves,
        already_moved: bool,
    ) -> crate::Result<Vec<PlannedRewrite>> {
        let after = before.moved(moves);
        let old = Resolver::new(before);
        let new = Resolver::new(&after);
        let mut plan = Vec::new();
        for old_rel in self.relink_candidates(moves)? {
            let new_rel = moves
                .forward(&old_rel)
                .map_or(old_rel.clone(), str::to_owned);
            let read_at = if already_moved { &new_rel } else { &old_rel };
            let Ok(content) = std::fs::read_to_string(self.root().join(read_at)) else {
                continue;
            };
            if !Self::relink_allowed(&content) {
                continue;
            }
            let rewrite = Rewrite {
                old: &old,
                new: &new,
                old_src: &old_rel,
                new_src: &new_rel,
            };
            if let Some((updated, links)) = relink_note(&content, &rewrite, moves) {
                let after = Self::finalize_relinked(&content, updated)?;
                plan.push(PlannedRewrite {
                    old_rel,
                    new_rel,
                    before: content,
                    after,
                    links,
                });
            }
        }
        Ok(plan)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world(notes: &[&str], files: &[&str]) -> LinkWorld {
        LinkWorld::new(
            notes
                .iter()
                .map(|p| ((*p).to_owned(), title_of(p)))
                .collect(),
            Vec::new(),
            notes.iter().chain(files).map(|p| (*p).to_owned()).collect(),
        )
    }

    /// 一篇笔记（`src`，移动后在 `new_src`）在 `moves` 之后的新全文。
    fn run(before: &LinkWorld, moves: &[(&str, &str)], src: &str, content: &str) -> Option<String> {
        let moves = Moves::new(
            moves
                .iter()
                .map(|(a, b)| ((*a).to_owned(), (*b).to_owned())),
        );
        let after = before.moved(&moves);
        let old = Resolver::new(before);
        let new = Resolver::new(&after);
        let new_src = moves.forward(src).unwrap_or(src).to_owned();
        let rewrite = Rewrite {
            old: &old,
            new: &new,
            old_src: src,
            new_src: &new_src,
        };
        relink_note(content, &rewrite, &moves).map(|(text, _)| text)
    }

    #[test]
    fn renames_both_link_kinds_keeping_fragment_alias_and_title() {
        let w = world(&["a.md", "ref.md"], &[]);
        let out = run(
            &w,
            &[("a.md", "b.md")],
            "ref.md",
            "[[a|别名]] [[a#计划]] ![[a#^k1|300]] [t](a.md#节) [u](a.md \"说明\")\n",
        );
        assert_eq!(
            out.as_deref(),
            Some("[[b|别名]] [[b#计划]] ![[b#^k1|300]] [t](b.md#节) [u](b.md \"说明\")\n")
        );
    }

    #[test]
    fn same_name_in_another_folder_is_left_alone() {
        // V-1：`b/x` 的 `[[foo]]` 解析到同目录的 `b/foo`，改名 `a/foo` 不得碰它。
        let w = world(&["a/foo.md", "b/foo.md", "b/x.md", "a/y.md"], &[]);
        assert_eq!(
            run(&w, &[("a/foo.md", "a/bar.md")], "b/x.md", "见 [[foo]]"),
            None
        );
        assert_eq!(
            run(&w, &[("a/foo.md", "a/bar.md")], "a/y.md", "见 [[foo]]").as_deref(),
            Some("见 [[bar]]")
        );
    }

    #[test]
    fn drift_after_a_same_name_move_is_pinned_with_a_path() {
        // V-1′：`a/x` 的 `[[foo]]` 原本指库根 `foo`；`b/foo` 挪进 `a/` 后，裸名会改指同目录那篇。
        let w = world(&["foo.md", "b/foo.md", "a/x.md", "c/y.md"], &[]);
        assert_eq!(
            run(&w, &[("b/foo.md", "a/foo.md")], "a/x.md", "见 [[foo]]").as_deref(),
            Some("见 [[foo.md]]")
        );
        // 解析没被抢走的不动。
        assert_eq!(
            run(&w, &[("b/foo.md", "a/foo.md")], "c/y.md", "见 [[foo]]"),
            None
        );
        // 指向被移动那篇的路径写法跟着换路径。
        assert_eq!(
            run(&w, &[("b/foo.md", "a/foo.md")], "c/y.md", "见 [[b/foo]]").as_deref(),
            Some("见 [[a/foo]]")
        );
    }

    #[test]
    fn code_is_never_rewritten() {
        // V-3：围栏（``` 与 ~~~）、缩进代码块、行内代码里的示例一个字节都不动。
        let w = world(&["a.md", "ref.md"], &[]);
        let content = "```md\n[[a]]\n```\n~~~\n[x](a.md)\n~~~\n\n    [[a]]\n\n用 `[[a]]` 写链接，正文 [[a]]\n";
        assert_eq!(
            run(&w, &[("a.md", "b.md")], "ref.md", content).as_deref(),
            Some("```md\n[[a]]\n```\n~~~\n[x](a.md)\n~~~\n\n    [[a]]\n\n用 `[[a]]` 写链接，正文 [[b]]\n")
        );
        // 列表里缩进四格的子项不是代码块。
        assert_eq!(
            run(&w, &[("a.md", "b.md")], "ref.md", "- 父\n    - 子 [[a]]\n").as_deref(),
            Some("- 父\n    - 子 [[b]]\n")
        );
    }

    #[test]
    fn alias_and_broken_links_are_left_alone() {
        let mut w = world(&["周会.md", "ref.md"], &[]);
        w.aliases = vec![("周会.md".to_owned(), "例会".to_owned())];
        let content = "[[例会]] [[周会]] [[不存在]] [x](没有.md)";
        assert_eq!(
            run(&w, &[("周会.md", "周例会.md")], "ref.md", content).as_deref(),
            Some("[[例会]] [[周例会]] [[不存在]] [x](没有.md)")
        );
    }

    #[test]
    fn case_only_rename_updates_the_written_name() {
        let w = world(&["foo.md", "ref.md"], &[]);
        assert_eq!(
            run(&w, &[("foo.md", "Foo.md")], "ref.md", "[[foo]] [x](foo.md)").as_deref(),
            Some("[[Foo]] [x](Foo.md)")
        );
    }

    #[test]
    fn asset_rename_covers_every_way_of_writing_it() {
        // V-4：此前只替换 `](旧)`，`![[图.png]]`、尖括号、带标题的写法都会断。
        let w = world(&["notes/ref.md"], &["notes/assets/图 1.png"]);
        let content = "![[图 1.png]] ![[notes/assets/图 1.png|300]] ![](assets/图%201.png) ![](<assets/图 1.png> \"说明\")";
        assert_eq!(
            run(&w, &[("notes/assets/图 1.png", "notes/assets/封面.png")], "notes/ref.md", content).as_deref(),
            Some("![[封面.png]] ![[notes/assets/封面.png|300]] ![](assets/封面.png) ![](<assets/封面.png> \"说明\")")
        );
    }

    #[test]
    fn a_moved_note_keeps_its_own_relative_links_working() {
        // 笔记换了文件夹：它自己写的相对 md 链接按新位置重算，wikilink 照样指得到就不动。
        let w = world(&["a/x.md", "a/y.md", "z.md"], &["a/assets/共享.png"]);
        let content = "[y](y.md) [[y]] [z](../z.md) ![](assets/共享.png)";
        assert_eq!(
            run(&w, &[("a/x.md", "b/c/x.md")], "a/x.md", content).as_deref(),
            Some("[y](../../a/y.md) [[y]] [z](../../z.md) ![](../../a/assets/共享.png)")
        );
    }

    #[test]
    fn folder_move_updates_links_into_and_out_of_it() {
        let w = world(&["proj/x.md", "y.md", "ref.md"], &["proj/assets/图.png"]);
        let moves = [
            ("proj/x.md", "archive/2026/proj/x.md"),
            ("proj/assets/图.png", "archive/2026/proj/assets/图.png"),
        ];
        // 外面指进来的（笔记与图片）跟着换路径。
        assert_eq!(
            run(
                &w,
                &moves,
                "ref.md",
                "[x](proj/x.md) ![](proj/assets/图.png) [[x]]"
            )
            .as_deref(),
            Some("[x](archive/2026/proj/x.md) ![](archive/2026/proj/assets/图.png) [[x]]")
        );
        // 里面指出去的按新深度重算；子树内部的相对链接不变。
        assert_eq!(
            run(&w, &moves, "proj/x.md", "[y](../y.md) ![](assets/图.png)").as_deref(),
            Some("[y](../../../y.md) ![](assets/图.png)")
        );
    }
}
