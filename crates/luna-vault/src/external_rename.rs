//! 库外改名的修复（PRD-06-030 ⑤，TDD-06 §3 ⑧；笔记库对标轮 V3a）。
//!
//! 在访达、`git mv` 或应用没开时把笔记改了名，指向旧名的链接就断了。Lunarca 按内容认出
//! 这次改名（「消失的那篇」与「新出现的那篇」内容指纹相同），算出还有多少处链接指向旧名，
//! **只提示、不自动改**（PRD-06-030 ⑤ 用户裁决 Q4：默认询问）；用户点「修复」后走与应用内
//! 改名同一个改写器（[`crate::relink`]），只是被移动的那篇已经在新位置了。

use crate::relink::{LinkWorld, Moves};
use crate::{Result, Vault};
use std::collections::HashMap;

/// 一次被认出的库外改名，以及它留下的断链。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalRename {
    pub from: String,
    pub to: String,
    /// 有几篇笔记里还有指向旧名的链接（含被改名那篇自己的相对链接）。
    pub notes: usize,
    pub links: usize,
}

/// 一次修复的结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExternalRepair {
    /// 改写了的笔记。
    pub rewritten: Vec<String>,
    pub links: usize,
}

fn is_note(path: &str) -> bool {
    path.to_lowercase().ends_with(".md")
}

/// 只有一篇笔记持有的指纹 → 那篇笔记。
fn unique_by_hash(items: &[(String, String)]) -> HashMap<&str, &str> {
    let mut by_hash: HashMap<&str, Vec<&str>> = HashMap::new();
    for (path, hash) in items.iter().filter(|(path, _)| is_note(path)) {
        by_hash
            .entry(hash.as_str())
            .or_default()
            .push(path.as_str());
    }
    by_hash
        .into_iter()
        .filter_map(|(hash, paths)| (paths.len() == 1).then(|| (hash, paths[0])))
        .collect()
}

/// 按内容指纹把「消失的笔记」与「新出现的笔记」配对：两边各自只有一篇持有这个指纹才算——
/// 两篇空笔记、两份同样的模板指纹相同，猜错一对就会把链接改去不相干的地方。
#[must_use]
pub fn pair_external_renames(
    vanished: &[(String, String)],
    appeared: &[(String, String)],
) -> Vec<(String, String)> {
    let gone = unique_by_hash(vanished);
    let came = unique_by_hash(appeared);
    let mut out: Vec<(String, String)> = gone
        .iter()
        .filter_map(|(hash, from)| {
            came.get(hash)
                .map(|to| ((*from).to_owned(), (*to).to_owned()))
        })
        .filter(|(from, to)| from != to)
        .collect();
    out.sort();
    out
}

impl Vault {
    /// 一篇笔记在索引里的内容指纹（外部变更同步索引之前取）。
    pub fn indexed_hash(&self, rel: &str) -> Result<Option<String>> {
        self.index().note_hash(rel)
    }

    /// 仍然成立的改名对：新名字那篇在、旧名字那篇不在。提示之后用户可能又动过。
    fn live_pairs(&self, pairs: &[(String, String)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .filter(|(from, to)| self.root().join(to).is_file() && !self.root().join(from).exists())
            .cloned()
            .collect()
    }

    fn external_rename_plan(
        &self,
        world: &LinkWorld,
        pairs: &[(String, String)],
    ) -> Result<Vec<crate::relink::PlannedRewrite>> {
        let moves = Moves::new(pairs.to_vec());
        let before = world.reverted(&moves);
        self.plan_relink_after_move(&before, &moves)
    }

    /// 这些库外改名各留下多少处指向旧名的链接；没有断链的不列（没什么可修）。
    pub fn external_rename_impact(
        &self,
        pairs: &[(String, String)],
    ) -> Result<Vec<ExternalRename>> {
        let world = self.link_world()?;
        let mut out = Vec::new();
        for pair in self.live_pairs(pairs) {
            let plan = self.external_rename_plan(&world, std::slice::from_ref(&pair))?;
            let links: usize = plan.iter().map(|edit| edit.links).sum();
            if links > 0 {
                out.push(ExternalRename {
                    from: pair.0,
                    to: pair.1,
                    notes: plan.len(),
                    links,
                });
            }
        }
        Ok(out)
    }

    /// 修复：把指向旧名的链接改成指向新名（与应用内改名同一个改写器）。先查后写——有一篇
    /// 写不进去就整次不做；写到一半失败把已改的逐篇写回。版本历史随改名迁移。
    pub fn repair_external_renames(&self, pairs: &[(String, String)]) -> Result<ExternalRepair> {
        let pairs = self.live_pairs(pairs);
        if pairs.is_empty() {
            return Ok(ExternalRepair::default());
        }
        let world = self.link_world()?;
        let plan = self.external_rename_plan(&world, &pairs)?;
        self.ensure_writable(plan.iter().map(|edit| edit.new_rel.as_str()))?;
        let mut written: Vec<&crate::relink::PlannedRewrite> = Vec::new();
        for edit in &plan {
            if let Err(error) = self.write_planned(edit) {
                let unrestored: Vec<String> = written
                    .iter()
                    .rev()
                    .filter(|done| {
                        crate::atomic::write_atomic(
                            &self.root().join(&done.new_rel),
                            done.before.as_bytes(),
                        )
                        .is_err()
                    })
                    .map(|done| done.new_rel.clone())
                    .collect();
                return Err(crate::vault::rollback_error(error, unrestored));
            }
            written.push(edit);
        }
        crate::atomic::flush_durable(self.root())?;
        for (from, to) in &pairs {
            let _ = self.snapshot_store().rename_chain(from, to);
            let _ = self.remap_bookmarks(from, to);
        }
        Ok(ExternalRepair {
            rewritten: plan.iter().map(|edit| edit.new_rel.clone()).collect(),
            links: plan.iter().map(|edit| edit.links).sum(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(path: &str, hash: &str) -> (String, String) {
        (path.to_owned(), hash.to_owned())
    }

    #[test]
    fn pairs_only_unique_fingerprints_of_notes() {
        let vanished = [
            h("旧.md", "A"),
            h("空1.md", "E"),
            h("空2.md", "E"),
            h("图.png", "P"),
        ];
        let appeared = [h("新.md", "A"), h("空3.md", "E"), h("图2.png", "P")];
        assert_eq!(
            pair_external_renames(&vanished, &appeared),
            vec![("旧.md".to_owned(), "新.md".to_owned())]
        );
    }

    /// 在库外把一篇被引用的笔记改名、换了目录：提示有几处断链；修复后引用方指向新位置，
    /// 被移动那篇自己的相对链接也修好；版本历史随改名迁移。
    #[test]
    fn an_outside_rename_is_measured_and_repaired_with_the_same_rewriter() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let vault = Vault::open(root).unwrap();
        vault
            .write("甲/目标.md", "第一版\n[邻居](邻居.md)")
            .unwrap();
        vault
            .write("甲/目标.md", "第二版\n[邻居](邻居.md)")
            .unwrap();
        vault.write("甲/邻居.md", "x").unwrap();
        vault
            .write("引用.md", "[[目标]] 和 [t](甲/目标.md)")
            .unwrap();
        std::fs::create_dir_all(root.join("乙")).unwrap();
        std::fs::rename(root.join("甲/目标.md"), root.join("乙/改名了.md")).unwrap();
        vault.sync_external_note("甲/目标.md").unwrap();
        vault.sync_external_note("乙/改名了.md").unwrap();

        let pair = vec![("甲/目标.md".to_owned(), "乙/改名了.md".to_owned())];
        let impact = vault.external_rename_impact(&pair).unwrap();
        assert_eq!(impact.len(), 1);
        assert_eq!(impact[0].links, 3, "{impact:?}");
        assert_eq!(impact[0].notes, 2);

        let repair = vault.repair_external_renames(&pair).unwrap();
        assert_eq!(repair.links, 3);
        let referrer = std::fs::read_to_string(root.join("引用.md")).unwrap();
        assert_eq!(referrer, "[[改名了]] 和 [t](乙/改名了.md)");
        let moved = std::fs::read_to_string(root.join("乙/改名了.md")).unwrap();
        assert!(moved.contains("[邻居](../甲/邻居.md)"), "{moved}");
        assert!(
            !vault.snapshots("乙/改名了.md").is_empty(),
            "版本历史随改名迁移"
        );
        assert!(
            vault.external_rename_impact(&pair).unwrap().is_empty(),
            "修好之后没有断链"
        );
    }

    #[test]
    fn a_rename_that_was_undone_is_no_longer_reported() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let vault = Vault::open(root).unwrap();
        vault.write("a.md", "x").unwrap();
        vault.write("b.md", "[[a]]").unwrap();
        let pair = vec![("a.md".to_owned(), "c.md".to_owned())];
        assert!(
            vault.external_rename_impact(&pair).unwrap().is_empty(),
            "c.md 不在：不成立"
        );
        assert_eq!(
            vault.repair_external_renames(&pair).unwrap(),
            ExternalRepair::default()
        );
    }
}
