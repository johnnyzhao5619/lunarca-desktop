//! 关系图谱构建（PRD-06-034 / TDD-06 §3，Obsidian Graph View 对标）。
//!
//! 纯逻辑：输入派生索引三底表（笔记 / 链接 / 标签），输出节点 + 边的图结构，
//! headless 可测。链接目标解析口径与反链一致（路径或文件名 stem，大小写不敏感）：
//! - **笔记节点**：每篇已索引笔记一个节点（id = 相对路径）。
//! - **标签节点**：每个标签一个节点（id = `tag:<tag>`），笔记 → 标签建边——
//!   共享标签的笔记经标签节点二跳相连，这正是 AI 关系标签驱动图谱连通性的通道。
//! - **幽灵节点**（Obsidian「未创建笔记」对标）：链接指向的目标不存在时以
//!   淡显占位节点呈现（id = `ghost:<目标小写>`），提示知识网络的缺口。
//! - 非 `.md` 扩展的链接目标（图片/音频附件等）不入图（图谱只表达笔记间关系）。

// 「这条链接指向哪一篇」的唯一实现，与网页版共用（`luna_sync_core::links`，ADR-177 ⑥）。
use luna_sync_core::links::LinkIndex;
use luna_sync_core::path::name_key;
use std::collections::{BTreeMap, BTreeSet};

/// 一行笔记底表（路径 / 标题 / 两轴事实）。
///
/// 从三元组升级成具名结构是因为成熟度轴（ADR-086）让它长到了第四个字段——
/// `(String, String, String, bool)` 这种形状的第四个位置，下一个人只能靠数括号来读。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphNoteRow {
    /// 相对路径。
    pub path: String,
    /// 标题（= 文件名 stem）。
    pub title: String,
    /// 笔记类型 kebab 键（来源轴）。
    pub note_type: String,
    /// 是不是**记录态**（成熟度轴，`notes.stage`）。构图与索引口径**不因它改变**——
    /// 记录态的图谱过滤是前端视图态（PRD-06-052），节点只携带事实。
    pub draft: bool,
}

/// 图底表行（由 [`crate::index::Index::graph_rows`] 一次拉取）。
#[derive(Clone, Debug, Default)]
pub struct GraphRows {
    /// 笔记底表。
    pub notes: Vec<GraphNoteRow>,
    /// (源笔记路径, 链接目标原文)。
    pub links: Vec<(String, String)>,
    /// (笔记路径, 标签)。
    pub tags: Vec<(String, String)>,
    /// (笔记路径, 别名)：frontmatter `aliases`，链接按别名也认得出这一篇（对标方案 C2）。
    pub aliases: Vec<(String, String)>,
}

/// 节点类别。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphNodeKind {
    /// 已存在的笔记。
    Note,
    /// 标签（聚合节点）。
    Tag,
    /// 未创建笔记（链接目标不存在，Obsidian「未解析链接」对标）。
    Ghost,
}

/// 一个图谱节点。
#[derive(Clone, Debug, PartialEq)]
pub struct GraphNode {
    /// 稳定 id：笔记 = 相对路径；标签 = `tag:<tag>`；幽灵 = `ghost:<目标小写>`。
    pub id: String,
    /// 展示名：笔记 = 标题（文件名 stem）；标签 = `#<tag>`；幽灵 = 链接目标原文。
    pub label: String,
    /// 节点类别。
    pub kind: GraphNodeKind,
    /// 笔记类型串（仅笔记节点；`note` / `recording` / `chat` …，供着色分组）。
    pub note_type: Option<String>,
    /// 这一篇是**记录态**吗（ADR-086；标签/幽灵节点恒 `false`）。
    ///
    /// 只是**事实**，不是过滤：记录态默认隐藏、可开是**前端视图态**（与资料的类型
    /// 过滤器同形态，PRD-06-052），构图与索引口径逐字不变——孤立判定仍按全图度数。
    pub draft: bool,
    /// 连接度（入边 + 出边数，供节点尺寸映射）。
    pub degree: u32,
}

/// 边类别。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphEdgeKind {
    /// 笔记间链接（wikilink / 标准 md 相对链接）。
    Link,
    /// 笔记 → 标签归属。
    Tag,
}

/// 一条图谱边（`source`/`target` 为节点 id）。
#[derive(Clone, Debug, PartialEq)]
pub struct GraphEdge {
    pub source: String,
    pub target: String,
    pub kind: GraphEdgeKind,
}

/// 全库关系图谱。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VaultGraph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

/// 链接目标是否可能指向笔记：无扩展（wikilink stem）或 `.md`；
/// 其它扩展（图片/音频/pdf…）是附件引用，不入图。点后缀仅当形如真实扩展名
/// （1–6 位 ASCII 字母数字）才按扩展语义处理——`[[v1.0 方案]]` 这类含点笔记名不误伤。
fn is_note_target(dst: &str) -> bool {
    let file = dst.rsplit('/').next().unwrap_or(dst);
    match file.rsplit_once('.') {
        Some((stem, ext)) => {
            let ext_like =
                (1..=6).contains(&ext.len()) && ext.chars().all(|c| c.is_ascii_alphanumeric());
            !ext_like || (!stem.is_empty() && ext.eq_ignore_ascii_case("md"))
        }
        None => true,
    }
}

/// 由索引底表构建全库图谱。节点/边确定性排序（笔记按路径、标签按名、幽灵按 id；
/// 边按 (source, target)），保证幂等输出便于测试与前端增量比对。
#[must_use]
pub fn build_graph(rows: &GraphRows) -> VaultGraph {
    let mut sorted_notes: Vec<&GraphNoteRow> = rows.notes.iter().collect();
    sorted_notes.sort_by(|a, b| a.path.cmp(&b.path));
    let index = LinkIndex::build(
        sorted_notes
            .iter()
            .map(|note| (note.path.as_str(), note.title.as_str())),
    )
    .with_aliases(
        rows.aliases
            .iter()
            .map(|(path, alias)| (path.as_str(), alias.as_str())),
    );

    // 链接边：解析目标 → 笔记或幽灵；去重、剔自环。
    let mut link_edges: BTreeSet<(String, String)> = BTreeSet::new();
    let mut ghosts: BTreeMap<String, String> = BTreeMap::new(); // id → label
    for (src, dst) in &rows.links {
        if !index.contains_path(src) || !is_note_target(dst) {
            continue;
        }
        let target_id = match index.resolve(src, dst) {
            Some(path) => path.to_owned(),
            None => {
                let id = format!("ghost:{}", name_key(dst.trim()));
                ghosts
                    .entry(id.clone())
                    .or_insert_with(|| dst.trim().to_owned());
                id
            }
        };
        if target_id.eq_ignore_ascii_case(src) {
            continue;
        }
        link_edges.insert((src.clone(), target_id));
    }

    // 标签边：笔记 → tag:<tag>。
    //
    // **大小写归并**（与 `Index::list_tags` / 标签过滤同一口径）：模型给两篇笔记写出
    // `Data_Security` 与 `data_security` 是常态，按原样建节点会让**同一个概念在图谱上裂成
    // 两个点**——两篇本该经它二跳相连的笔记于是各自孤立（真机第一轮亲眼所见）。
    // 展示取**字典序首见**的那种原形（`BTreeMap` 保证确定性，与本模块其余部分同纪律）。
    let mut display: BTreeMap<String, String> = BTreeMap::new();
    for (_, tag) in &rows.tags {
        if tag.is_empty() {
            continue;
        }
        display
            .entry(tag.to_lowercase())
            .or_insert_with(|| tag.clone());
    }
    let mut tag_edges: BTreeSet<(String, String)> = BTreeSet::new();
    let mut tag_names: BTreeSet<String> = BTreeSet::new();
    for (note, tag) in &rows.tags {
        if !index.contains_path(note) || tag.is_empty() {
            continue;
        }
        let canonical = display
            .get(&tag.to_lowercase())
            .cloned()
            .unwrap_or_else(|| tag.clone());
        tag_names.insert(canonical.clone());
        tag_edges.insert((note.clone(), format!("tag:{canonical}")));
    }

    // 连接度统计。
    let mut degree: BTreeMap<&str, u32> = BTreeMap::new();
    let mut edges: Vec<GraphEdge> = Vec::with_capacity(link_edges.len() + tag_edges.len());
    for (source, target) in &link_edges {
        edges.push(GraphEdge {
            source: source.clone(),
            target: target.clone(),
            kind: GraphEdgeKind::Link,
        });
    }
    for (source, target) in &tag_edges {
        edges.push(GraphEdge {
            source: source.clone(),
            target: target.clone(),
            kind: GraphEdgeKind::Tag,
        });
    }
    for edge in &edges {
        *degree.entry(edge.source.as_str()).or_default() += 1;
        *degree.entry(edge.target.as_str()).or_default() += 1;
    }

    let mut nodes: Vec<GraphNode> = Vec::new();
    for note in &sorted_notes {
        nodes.push(GraphNode {
            id: note.path.clone(),
            label: note.title.clone(),
            kind: GraphNodeKind::Note,
            note_type: Some(note.note_type.clone()),
            draft: note.draft,
            degree: degree.get(note.path.as_str()).copied().unwrap_or(0),
        });
    }
    for tag in &tag_names {
        let id = format!("tag:{tag}");
        nodes.push(GraphNode {
            degree: degree.get(id.as_str()).copied().unwrap_or(0),
            label: format!("#{tag}"),
            kind: GraphNodeKind::Tag,
            note_type: None,
            draft: false,
            id,
        });
    }
    for (id, label) in &ghosts {
        nodes.push(GraphNode {
            degree: degree.get(id.as_str()).copied().unwrap_or(0),
            id: id.clone(),
            label: label.clone(),
            kind: GraphNodeKind::Ghost,
            note_type: None,
            draft: false,
        });
    }

    VaultGraph { nodes, edges }
}

// ── AI 标签生成范围（PRD-06-035c）─────────────────────────────────────────────
//
// 文件夹级排除的**唯一判据**：自动触发（保存/转写后）与全库/定向重扫的候选筛选
// 共用这两个纯函数——两处各判一遍必然漂移。规则本体随库存储（`.luna/graph_scope.json`，
// [`crate::Vault::graph_tag_exclusions`]），Obsidian「Excluded files」同构。

/// `path` 是否落在 `folder`（含其全部子孙）内。边界严格：`Chats` 覆盖
/// `Chats/a.md` 与 `Chats/sub/b.md`，**不**覆盖 `Chats2/a.md`。
#[must_use]
pub fn scope_covers(folder: &str, path: &str) -> bool {
    path == folder
        || path
            .strip_prefix(folder)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// `path` 是否被任一排除文件夹覆盖（AI 标签生成的排除判据）。
#[must_use]
pub fn is_tag_excluded(path: &str, excluded: &[String]) -> bool {
    excluded.iter().any(|folder| scope_covers(folder, path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(path: &str, title: &str, note_type: &str) -> GraphNoteRow {
        GraphNoteRow {
            path: path.into(),
            title: title.into(),
            note_type: note_type.into(),
            draft: false,
        }
    }

    fn rows() -> GraphRows {
        GraphRows {
            aliases: vec![],
            notes: vec![
                row("a.md", "a", "note"),
                row("dir/b.md", "b", "note"),
                row("rec.md", "rec", "recording"),
            ],
            links: vec![
                // wikilink stem 解析（大小写不敏感）。
                ("a.md".into(), "B".into()),
                // 标准 md 相对路径解析。
                ("rec.md".into(), "dir/b.md".into()),
                // 未创建笔记 → 幽灵节点。
                ("a.md".into(), "尚未创建".into()),
                // 附件目标不入图。
                ("a.md".into(), "assets/img.png".into()),
                // 自环剔除。
                ("a.md".into(), "a".into()),
            ],
            tags: vec![
                ("a.md".into(), "工作".into()),
                ("dir/b.md".into(), "工作".into()),
            ],
        }
    }

    #[test]
    fn builds_note_tag_ghost_nodes_and_resolves_links() {
        let g = build_graph(&rows());
        let ids: Vec<&str> = g.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["a.md", "dir/b.md", "rec.md", "tag:工作", "ghost:尚未创建"]
        );
        // stem 与路径两种解析都归到真实笔记节点；附件与自环不建边。
        let link_edges: Vec<(&str, &str)> = g
            .edges
            .iter()
            .filter(|e| e.kind == GraphEdgeKind::Link)
            .map(|e| (e.source.as_str(), e.target.as_str()))
            .collect();
        assert_eq!(
            link_edges,
            vec![
                ("a.md", "dir/b.md"),
                ("a.md", "ghost:尚未创建"),
                ("rec.md", "dir/b.md"),
            ]
        );
        // 标签节点经两条 tag 边连接两篇笔记（图谱连通性通道）。
        let tag_edges = g
            .edges
            .iter()
            .filter(|e| e.kind == GraphEdgeKind::Tag)
            .count();
        assert_eq!(tag_edges, 2);
    }

    #[test]
    fn degree_counts_all_edge_kinds() {
        let g = build_graph(&rows());
        let by_id = |id: &str| g.nodes.iter().find(|n| n.id == id).unwrap();
        // dir/b.md：两条入链 + 一条标签边。
        assert_eq!(by_id("dir/b.md").degree, 3);
        assert_eq!(by_id("tag:工作").degree, 2);
        assert_eq!(by_id("ghost:尚未创建").degree, 1);
        assert_eq!(by_id("rec.md").note_type.as_deref(), Some("recording"));
        assert_eq!(by_id("tag:工作").label, "#工作");
    }

    /// 成熟度轴（ADR-086）只让节点**多带一个事实**，构图口径逐字不变：记录态笔记
    /// 照常入图、照常建边、度数照算——「默认隐藏、可开」是前端视图态（PRD-06-052）。
    /// 在 Rust 侧就把它剔掉的话，孤立判定（PRD-06-043 全图度数）会跟着变，
    /// 而那是没人要求过的行为改变。
    #[test]
    fn draft_notes_stay_in_the_graph_and_only_carry_the_fact() {
        let mut r = rows();
        r.notes.push(GraphNoteRow {
            path: "草稿.md".into(),
            title: "草稿".into(),
            note_type: "note".into(),
            draft: true,
        });
        r.links.push(("a.md".into(), "草稿".into()));
        let g = build_graph(&r);
        let node = g
            .nodes
            .iter()
            .find(|n| n.id == "草稿.md")
            .expect("记录态节点");
        assert!(node.draft, "节点必须携带记录态事实");
        assert_eq!(node.degree, 1, "记录态照常建边、度数照算");
        assert!(
            g.nodes
                .iter()
                .filter(|n| n.kind != GraphNodeKind::Note)
                .all(|n| !n.draft),
            "标签/幽灵节点谈不上成熟度"
        );
        assert!(
            g.nodes.iter().any(|n| n.id == "a.md" && !n.draft),
            "普通笔记不得被误标"
        );
    }

    #[test]
    fn duplicate_links_dedupe_and_output_is_deterministic() {
        let mut r = rows();
        r.links.push(("a.md".into(), "dir/b.md".into())); // 与 stem 解析同目标 → 去重。
        let once = build_graph(&r);
        let twice = build_graph(&r);
        assert_eq!(once, twice);
        let a_to_b = once
            .edges
            .iter()
            .filter(|e| e.source == "a.md" && e.target == "dir/b.md")
            .count();
        assert_eq!(a_to_b, 1);
    }

    /// Lunarca 自己写出去的跨文件夹链接必须解析得回来（出处行 / 蒸馏来源区 / 归入源链接
    /// 全是 `../目录/名字%20带空格.md` 这一形态）。解析不回来的代价不是"少一条边"，而是
    /// **一篇存在的笔记被画成幽灵节点**——图谱凭空多出未创建笔记，知识体检（PRD-16-030）
    /// 还会把它当真断链报出来。
    #[test]
    fn generated_links_resolve_across_folders_and_percent_escapes() {
        let rows = GraphRows {
            aliases: vec![],
            notes: vec![
                row("Chats/对话.md", "对话", "chat"),
                row("Notes/项目 X.md", "项目 X", "note"),
                row("Notes/sub/深处.md", "深处", "note"),
            ],
            links: vec![
                // 出处行的真实形态：相对路径 + 空格转义。
                ("Chats/对话.md".into(), "../Notes/项目%20X.md".into()),
                // 同目录下探一层（相对源笔记所在目录归一化）。
                ("Notes/项目 X.md".into(), "sub/深处.md".into()),
                // 归入源链接的真实形态：**同目录、带 `.md`**——按字面既不是库内路径
                // （缺目录）也不是 stem（多了 `.md`），不归一化就永远是幽灵。
                ("Notes/sub/深处.md".into(), "../项目%20X.md".into()),
                // 真断链仍然是断链——修的是解析，不是把幽灵取消掉。
                ("Chats/对话.md".into(), "../Notes/根本没有.md".into()),
            ],
            tags: vec![],
        };
        let g = build_graph(&rows);
        let link_edges: Vec<(&str, &str)> = g
            .edges
            .iter()
            .filter(|e| e.kind == GraphEdgeKind::Link)
            .map(|e| (e.source.as_str(), e.target.as_str()))
            .collect();
        assert!(
            link_edges.contains(&("Chats/对话.md", "Notes/项目 X.md")),
            "转义 + 跨目录的自动链接必须解析到真实笔记：{link_edges:?}"
        );
        assert!(
            link_edges.contains(&("Notes/项目 X.md", "Notes/sub/深处.md")),
            "相对路径按源笔记目录归一化：{link_edges:?}"
        );
        assert!(
            link_edges.contains(&("Notes/sub/深处.md", "Notes/项目 X.md")),
            "同目录 `.md` 链接（归入源链接的形态）必须解析：{link_edges:?}"
        );
        let ghosts: Vec<&str> = g
            .nodes
            .iter()
            .filter(|n| n.kind == GraphNodeKind::Ghost)
            .map(|n| n.label.as_str())
            .collect();
        assert_eq!(ghosts, vec!["../Notes/根本没有.md"], "只剩真断链");
    }

    /// 同一个概念在图谱上必须是**一个**节点：模型给两篇笔记写出 `Data_Security` 与
    /// `data_security` 是常态，裂成两个点会让本该经它二跳相连的两篇各自孤立
    /// （真机第一轮亲眼所见）。口径与 `Index::list_tags` 的分组同源。
    #[test]
    fn tag_nodes_merge_case_variants_into_one() {
        let rows = GraphRows {
            aliases: vec![],
            notes: vec![row("a.md", "a", "note"), row("b.md", "b", "note")],
            links: vec![],
            tags: vec![
                ("a.md".into(), "Data_Security".into()),
                ("b.md".into(), "data_security".into()),
            ],
        };
        let g = build_graph(&rows);
        let tags: Vec<&str> = g
            .nodes
            .iter()
            .filter(|n| n.kind == GraphNodeKind::Tag)
            .map(|n| n.id.as_str())
            .collect();
        assert_eq!(tags.len(), 1, "两种大小写必须并成一个节点：{tags:?}");
        let node = g
            .nodes
            .iter()
            .find(|n| n.kind == GraphNodeKind::Tag)
            .expect("标签节点");
        assert_eq!(node.degree, 2, "两篇都应连到这一个节点上");
        // 两篇经标签节点二跳相连——这正是标签在图谱里的全部作用。
        assert_eq!(
            g.edges
                .iter()
                .filter(|e| e.kind == GraphEdgeKind::Tag)
                .count(),
            2
        );
    }

    #[test]
    fn aliases_resolve_after_paths_and_names() {
        let notes = [
            ("项目/周会纪要.md", "周会纪要"),
            ("Weekly.md", "Weekly"),
            ("归档/旧.md", "旧"),
        ];
        let aliases = [
            ("项目/周会纪要.md", "例会"),
            ("项目/周会纪要.md", "weekly"),
            ("不存在.md", "幽灵别名"),
        ];
        let index = LinkIndex::build(notes).with_aliases(aliases);
        assert_eq!(index.resolve("a.md", "例会"), Some("项目/周会纪要.md"));
        assert_eq!(index.resolve("a.md", "例会#结论"), Some("项目/周会纪要.md"));
        // 真叫这个名字的笔记优先于别人的别名。
        assert_eq!(index.resolve("a.md", "Weekly"), Some("Weekly.md"));
        // 指向库里没有的笔记的别名不认。
        assert_eq!(index.resolve("a.md", "幽灵别名"), None);
    }

    #[test]
    fn same_name_links_prefer_the_writers_folder_then_the_shallowest() {
        let notes = [
            ("周报.md", "周报"),
            ("项目/甲/周报.md", "周报"),
            ("项目/周报.md", "周报"),
            ("归档/计划.md", "计划"),
        ];
        let index = LinkIndex::build(notes);
        assert_eq!(
            index.resolve("项目/甲/会议.md", "周报"),
            Some("项目/甲/周报.md")
        );
        assert_eq!(index.resolve("项目/乙/会议.md", "周报"), Some("周报.md"));
        assert_eq!(index.resolve("会议.md", "项目/周报"), Some("项目/周报.md"));
        // 子目录里唯一的一篇：按名字就能找到，不要求写路径。
        assert_eq!(index.resolve("会议.md", "计划"), Some("归档/计划.md"));
        // 片段剥离之前写进索引的旧行仍能认出。
        assert_eq!(index.resolve("会议.md", "计划#下周"), Some("归档/计划.md"));
        assert_eq!(index.resolve("会议.md", "不存在"), None);
    }

    #[test]
    fn note_target_filter_accepts_md_and_stem_only() {
        assert!(is_note_target("会议纪要"));
        assert!(is_note_target("dir/计划.md"));
        assert!(is_note_target("v1.0 方案")); // 点后非扩展语义按文件名整体处理。
        assert!(!is_note_target("assets/img.png"));
        assert!(!is_note_target("录音/assets/a.wav"));
    }

    #[test]
    fn scope_covers_requires_path_component_boundary() {
        // PRD-06-035c：`Chats` 覆盖自身与子孙，绝不覆盖同前缀的兄弟目录——
        // 这条边界一旦松掉，「排除 Chats」会把 `Chats归档/` 一并静默排除。
        assert!(scope_covers("Chats", "Chats"));
        assert!(scope_covers("Chats", "Chats/a.md"));
        assert!(scope_covers("Chats", "Chats/sub/b.md"));
        assert!(!scope_covers("Chats", "Chats2/a.md"));
        assert!(!scope_covers("Chats", "Chats归档/a.md"));
        assert!(!scope_covers("Chats/sub", "Chats/a.md"));
    }

    #[test]
    fn is_tag_excluded_matches_any_rule() {
        let excluded = vec!["Chats".to_string(), "Archive/Old".to_string()];
        assert!(is_tag_excluded("Chats/对话.md", &excluded));
        assert!(is_tag_excluded("Archive/Old/旧稿.md", &excluded));
        assert!(!is_tag_excluded("Archive/新稿.md", &excluded));
        assert!(!is_tag_excluded("笔记.md", &excluded));
        assert!(!is_tag_excluded("笔记.md", &[]));
    }
}
