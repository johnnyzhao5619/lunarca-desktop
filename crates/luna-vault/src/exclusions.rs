//! 排除路径（PRD-06-062，笔记库对标轮 V4a）：用户划出的「降噪区」。
//!
//! **语义（用户 2026-10-05 裁决）**：排除只是**不打扰**——被排除的笔记与附件不出现在搜索、
//! 快速切换器、图谱、语义索引、知识库的自动候选、相关笔记，附件不被列为「未被引用」；但
//! **链接维护照常**：仍进派生索引的链接表，改名照样改写排除区里的链接，反链照常显示。
//! 用户点名（`@` 某篇、直接打开、正文链接）永远优先于排除。
//!
//! 规则随库存储于 `.luna/excluded_paths.json`（与 `graph_scope.json` 同一判据：路径是库相对的，
//! 规则理应随库迁移，Obsidian「Excluded files」同构）。每次使用**现读**——清单很短，而缓存会让
//! 另一台设备同步过来的改动要等重开库才生效。条目可以是文件夹（含子孙）或单个文件，
//! 匹配口径与 [`crate::graph::scope_covers`] 同一（边界严格：`归档` 不覆盖 `归档2`）。

use crate::graph::{scope_covers, GraphNodeKind, VaultGraph};
use crate::vault::Vault;
use crate::Result;

/// 规则文件名（`.luna/<name>.json`）。前端据此认出广播里的那一次变更。
pub const EXCLUDED_PATHS_META: &str = "excluded_paths";

/// 一次操作里用的排除规则快照。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Exclusions {
    rules: Vec<String>,
}

impl Exclusions {
    #[must_use]
    pub fn new(mut rules: Vec<String>) -> Self {
        rules.sort();
        rules.dedup();
        Self { rules }
    }

    /// `path` 是否落在任一排除规则内。
    #[must_use]
    pub fn covers(&self, path: &str) -> bool {
        self.covering(path).is_some()
    }

    /// 覆盖 `path` 的那条规则（多条都覆盖时取最上层的那条）。
    #[must_use]
    pub fn covering(&self, path: &str) -> Option<&str> {
        self.rules
            .iter()
            .filter(|rule| scope_covers(rule, path))
            .min_by_key(|rule| rule.len())
            .map(String::as_str)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    #[must_use]
    pub fn rules(&self) -> &[String] {
        &self.rules
    }

    /// 从图谱里拿掉被排除的笔记节点及其边；只因它们才存在的标签 / 幽灵节点随之消失，
    /// 留下的节点按剩余的边重算连接度。没被排除的孤立笔记照旧保留（它本来就是孤立的）。
    #[must_use]
    pub fn prune_graph(&self, graph: VaultGraph) -> VaultGraph {
        if self.is_empty() {
            return graph;
        }
        let removed: std::collections::HashSet<String> = graph
            .nodes
            .iter()
            .filter(|node| node.kind == GraphNodeKind::Note && self.covers(&node.id))
            .map(|node| node.id.clone())
            .collect();
        if removed.is_empty() {
            return graph;
        }
        let edges: Vec<_> = graph
            .edges
            .into_iter()
            .filter(|edge| !removed.contains(&edge.source) && !removed.contains(&edge.target))
            .collect();
        let mut degree: std::collections::HashMap<&str, u32> = std::collections::HashMap::new();
        for edge in &edges {
            *degree.entry(edge.source.as_str()).or_default() += 1;
            *degree.entry(edge.target.as_str()).or_default() += 1;
        }
        let nodes = graph
            .nodes
            .into_iter()
            .filter(|node| !removed.contains(&node.id))
            .filter_map(|mut node| {
                let count = degree.get(node.id.as_str()).copied().unwrap_or(0);
                if node.kind != GraphNodeKind::Note && count == 0 {
                    return None;
                }
                node.degree = count;
                Some(node)
            })
            .collect();
        VaultGraph { nodes, edges }
    }
}

impl Vault {
    /// 当前的排除规则（排序去重）。规则文件读不了时按「没有排除」处理：排除是降噪，读坏了
    /// 宁可多显示，也不能悄悄藏掉用户的笔记。
    #[must_use]
    pub fn exclusions(&self) -> Exclusions {
        Exclusions::new(
            crate::meta::read_meta(self.root(), EXCLUDED_PATHS_META).unwrap_or_default(),
        )
    }

    /// 增删一条排除规则，返回更新后的完整清单（前端据此就地刷新）。
    ///
    /// # Errors
    /// 路径为空（根即全库，不是一条排除规则）、越界（`..` / 绝对路径）或写入失败。
    pub fn set_excluded(&self, path: &str, excluded: bool) -> Result<Vec<String>> {
        let path = Self::validate_scope_folder(path)?;
        let mut rules = self.exclusions().rules;
        if excluded {
            rules.push(path);
        } else {
            rules.retain(|rule| rule != &path);
        }
        let rules = Exclusions::new(rules).rules;
        crate::meta::write_meta(self.root(), EXCLUDED_PATHS_META, &rules)?;
        Ok(rules)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{GraphEdge, GraphEdgeKind, GraphNode};

    fn node(id: &str, kind: GraphNodeKind, degree: u32) -> GraphNode {
        GraphNode {
            id: id.to_owned(),
            label: id.to_owned(),
            kind,
            note_type: None,
            draft: false,
            degree,
        }
    }

    fn edge(source: &str, target: &str, kind: GraphEdgeKind) -> GraphEdge {
        GraphEdge {
            source: source.to_owned(),
            target: target.to_owned(),
            kind,
        }
    }

    #[test]
    fn covers_folders_and_single_files_with_strict_boundaries() {
        let rules = Exclusions::new(vec!["归档".into(), "日记/私密.md".into()]);
        assert!(rules.covers("归档"));
        assert!(rules.covers("归档/2024/a.md"));
        assert!(!rules.covers("归档2/a.md"));
        assert!(rules.covers("日记/私密.md"));
        assert!(!rules.covers("日记/公开.md"));
    }

    #[test]
    fn pruning_the_graph_drops_excluded_notes_and_what_only_they_brought() {
        let graph = VaultGraph {
            nodes: vec![
                node("a.md", GraphNodeKind::Note, 2),
                node("归档/b.md", GraphNodeKind::Note, 3),
                node("tag:旧", GraphNodeKind::Tag, 1),
                node("tag:共用", GraphNodeKind::Tag, 2),
                node("ghost:没写", GraphNodeKind::Ghost, 1),
                node("c.md", GraphNodeKind::Note, 0),
            ],
            edges: vec![
                edge("a.md", "归档/b.md", GraphEdgeKind::Link),
                edge("归档/b.md", "tag:旧", GraphEdgeKind::Tag),
                edge("归档/b.md", "tag:共用", GraphEdgeKind::Tag),
                edge("a.md", "tag:共用", GraphEdgeKind::Tag),
                edge("归档/b.md", "ghost:没写", GraphEdgeKind::Link),
            ],
        };
        let pruned = Exclusions::new(vec!["归档".into()]).prune_graph(graph);
        let ids: Vec<_> = pruned
            .nodes
            .iter()
            .map(|n| (n.id.as_str(), n.degree))
            .collect();
        assert_eq!(ids, vec![("a.md", 1), ("tag:共用", 1), ("c.md", 0)]);
        assert_eq!(
            pruned.edges,
            vec![edge("a.md", "tag:共用", GraphEdgeKind::Tag)]
        );
    }

    #[test]
    fn rules_are_stored_with_the_vault_and_validated() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        assert!(vault.exclusions().is_empty());
        assert_eq!(vault.set_excluded("/归档/", true).unwrap(), vec!["归档"]);
        assert_eq!(
            vault.set_excluded("日记/私密.md", true).unwrap(),
            vec!["归档", "日记/私密.md"]
        );
        assert_eq!(
            vault.set_excluded("归档", true).unwrap().len(),
            2,
            "重复添加不重复记"
        );
        assert!(dir.path().join(".luna/excluded_paths.json").is_file());
        assert!(vault.set_excluded("", true).is_err());
        assert!(vault.set_excluded("../外面", true).is_err());
        assert_eq!(
            vault.set_excluded("归档", false).unwrap(),
            vec!["日记/私密.md"]
        );
    }

    #[test]
    fn excluded_notes_stay_out_of_the_way_but_links_are_still_maintained() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        vault.write("计划.md", "季度目标\n").unwrap();
        vault
            .write("归档/旧计划.md", "季度目标，见 [[计划]]\n")
            .unwrap();
        vault.set_excluded("归档", true).unwrap();

        let found = vault
            .search_matches(&luna_sync_core::search::SearchQuery::parse(
                "季度目标",
                false,
            ))
            .unwrap();
        let paths: Vec<_> = found.notes.iter().map(|note| note.path.as_str()).collect();
        assert_eq!(paths, vec!["计划.md"]);
        let switcher: Vec<_> = vault
            .switcher_entries()
            .unwrap()
            .into_iter()
            .map(|entry| entry.path)
            .collect();
        assert_eq!(switcher, vec!["计划.md"]);
        assert!(vault
            .graph()
            .unwrap()
            .nodes
            .iter()
            .all(|n| !n.id.starts_with("归档")));
        // 文件树照常列出，但每个节点带上覆盖它的规则（树行淡显与右键勾选态据此）。
        let tree = vault.tree().unwrap();
        let folder = tree.children.iter().find(|n| n.path == "归档").unwrap();
        assert_eq!(folder.excluded_by.as_deref(), Some("归档"));
        assert_eq!(folder.children[0].excluded_by.as_deref(), Some("归档"));
        let kept = tree.children.iter().find(|n| n.path == "计划.md").unwrap();
        assert_eq!(kept.excluded_by, None);

        // 链接维护照常：反链看得到排除区里的引用，改名改写它。
        assert_eq!(vault.backlinks("计划.md").unwrap(), vec!["归档/旧计划.md"]);
        vault.move_("计划.md", "目标.md").unwrap();
        let old = std::fs::read_to_string(dir.path().join("归档/旧计划.md")).unwrap();
        assert!(old.contains("[[目标]]"), "{old}");
    }

    #[test]
    fn an_unreadable_rule_file_means_nothing_is_hidden() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        std::fs::create_dir_all(dir.path().join(".luna")).unwrap();
        std::fs::write(dir.path().join(".luna/excluded_paths.json"), "{坏的").unwrap();
        assert!(vault.exclusions().is_empty());
    }
}
