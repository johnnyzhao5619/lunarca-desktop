//! 行级三路合并（ADR-148 ②、TDD-06 §4.2）。纯函数，可 headless 重测。
//!
//! 基线 = 两边分叉前的共同版本，我方 / 对方 = 各自改过的版本。两边改动互不重叠才给出结果；
//! 任何一处两边改得不一样，就如实报冲突——不猜、不拼、不往文件里写冲突标记。
//!
//! - **正文按行比**。行内容相同即同一行（行尾 `\r\n` 与 `\n` 不算差别）。相邻两行分别被两边
//!   改动也算重叠（diff3 同口径），宁可交给人。输出沿用我方的换行风格：取自我方的行原样搬，
//!   取自对方的行只换行尾。
//! - **frontmatter 按顶层键比**，键的识别与属性面板同一处（`frontmatter::key_of`）。不同键
//!   各自生效，同一个键两边改得不一样才冲突；每个键连同续行与其后的空行、注释原样搬运，不重新
//!   序列化 YAML（ADR-139 ④）。三边有无 frontmatter 不一致、或任何一边有重复键，整篇按行比。
//! - **末尾换行**单独三路：一边加了或去了，就按那一边。

use crate::diff::lcs_pairs;
use std::collections::HashMap;

/// 一次三路合并的结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MergeOutcome {
    /// 两边改动不重叠，这是合并后的全文。
    Merged(String),
    /// 有 `regions` 处两边改得不一样，没有给出结果。
    Conflict { regions: u32 },
    /// 改动范围超出比对预算（与版本对比的降级同一个预算），没有比。
    TooLarge,
}

/// 合并 `ours` 与 `theirs` 相对共同基线 `base` 的改动。
#[must_use]
pub fn merge3(base: &str, ours: &str, theirs: &str) -> MergeOutcome {
    if ours == theirs || base == theirs {
        return MergeOutcome::Merged(ours.to_owned());
    }
    if base == ours {
        return MergeOutcome::Merged(theirs.to_owned());
    }
    let eol = newline_of(ours);
    use crate::frontmatter::frontmatter_parts as parts;
    if let (Some(b), Some(o), Some(t)) = (parts(base), parts(ours), parts(theirs)) {
        if let Some(front) = merge_frontmatter(b[1], o[1], t[1]) {
            let body = merge_lines(b[3], o[3], t[3], eol);
            return finish([
                Part::Text(o[0].to_owned()),
                front,
                Part::Text(o[2].to_owned()),
                body,
            ]);
        }
    }
    finish([merge_lines(base, ours, theirs, eol)])
}

/// 一段的合并结果；整篇由若干段拼成。
enum Part {
    Text(String),
    Conflict(u32),
    TooLarge,
}

fn finish<const N: usize>(parts: [Part; N]) -> MergeOutcome {
    let mut text = String::new();
    let mut regions = 0;
    for part in parts {
        match part {
            Part::TooLarge => return MergeOutcome::TooLarge,
            Part::Conflict(n) => regions += n,
            Part::Text(piece) => text.push_str(&piece),
        }
    }
    if regions > 0 {
        MergeOutcome::Conflict { regions }
    } else {
        MergeOutcome::Merged(text)
    }
}

/// 全文用的换行风格（第一处换行说了算；没有换行时用 `\n`）。
fn newline_of(content: &str) -> &'static str {
    match content.find('\n') {
        Some(i) if i > 0 && content.as_bytes()[i - 1] == b'\r' => "\r\n",
        _ => "\n",
    }
}

// ─── 正文：行级 diff3 ─────────────────────────────────────────────────────────

struct Lines<'a> {
    /// 每行原字节（含行尾；最后一行可能没有）。
    segs: Vec<&'a str>,
    /// 去掉行尾的内容：比对只看它。
    keys: Vec<&'a str>,
    final_newline: bool,
}

fn lines(text: &str) -> Lines<'_> {
    let segs: Vec<&str> = text.split_inclusive('\n').collect();
    let keys = segs
        .iter()
        .map(|seg| {
            seg.strip_suffix('\n')
                .map_or(*seg, |line| line.strip_suffix('\r').unwrap_or(line))
        })
        .collect();
    Lines {
        segs,
        keys,
        final_newline: text.is_empty() || text.ends_with('\n'),
    }
}

/// 基线每一行在另一边的对应行（没有对应为 `None`）。
fn aligned(base_len: usize, pairs: &[(usize, usize)]) -> Vec<Option<usize>> {
    let mut map = vec![None; base_len];
    for &(b, other) in pairs {
        map[b] = Some(other);
    }
    map
}

/// 输出的一行：从哪一边取的、取第几行。
#[derive(Clone, Copy)]
enum Pick {
    Ours(usize),
    Theirs(usize),
}

fn merge_lines(base: &str, ours: &str, theirs: &str, eol: &str) -> Part {
    let (b, o, t) = (lines(base), lines(ours), lines(theirs));
    let (Some(pairs_o), Some(pairs_t)) = (lcs_pairs(&b.keys, &o.keys), lcs_pairs(&b.keys, &t.keys))
    else {
        return Part::TooLarge;
    };
    let (in_o, in_t) = (
        aligned(b.keys.len(), &pairs_o),
        aligned(b.keys.len(), &pairs_t),
    );

    let mut picks: Vec<Pick> = Vec::new();
    let mut regions = 0u32;
    let (mut ib, mut io, mut it) = (0usize, 0usize, 0usize);
    loop {
        // 稳定段：基线这几行在两边都原样、且位置对齐。
        let mut run = 0;
        while ib + run < b.keys.len()
            && in_o[ib + run] == Some(io + run)
            && in_t[ib + run] == Some(it + run)
        {
            run += 1;
        }
        if run > 0 {
            picks.extend((io..io + run).map(Pick::Ours));
            (ib, io, it) = (ib + run, io + run, it + run);
            continue;
        }
        if ib == b.keys.len() && io == o.keys.len() && it == t.keys.len() {
            break;
        }
        // 不稳定段一直到下一处两边都保留的基线行（没有就到结尾）。
        let (eb, eo, et) =
            match (ib..b.keys.len()).find(|&l| in_o[l].is_some() && in_t[l].is_some()) {
                Some(l) => (l, in_o[l].unwrap_or(io), in_t[l].unwrap_or(it)),
                None => (b.keys.len(), o.keys.len(), t.keys.len()),
            };
        let (kb, ko, kt) = (&b.keys[ib..eb], &o.keys[io..eo], &t.keys[it..et]);
        if ko == kb {
            picks.extend((it..et).map(Pick::Theirs));
        } else if kt == kb || ko == kt {
            picks.extend((io..eo).map(Pick::Ours));
        } else {
            regions += 1;
        }
        (ib, io, it) = (eb, eo, et);
    }
    if regions > 0 {
        return Part::Conflict(regions);
    }

    let mut text = String::new();
    for (k, pick) in picks.iter().enumerate() {
        let last = k + 1 == picks.len();
        match *pick {
            Pick::Ours(i) => {
                text.push_str(o.segs[i]);
                if !o.segs[i].ends_with('\n') && !last {
                    text.push_str(eol);
                }
            }
            Pick::Theirs(i) => {
                text.push_str(t.keys[i]);
                if t.segs[i].ends_with('\n') || !last {
                    text.push_str(eol);
                }
            }
        }
    }
    let final_newline = if o.final_newline == b.final_newline {
        t.final_newline
    } else {
        o.final_newline
    };
    if !text.is_empty() {
        if final_newline && !text.ends_with('\n') {
            text.push_str(eol);
        } else if !final_newline && text.ends_with('\n') {
            text.pop();
            if text.ends_with('\r') {
                text.pop();
            }
        }
    }
    Part::Text(text)
}

// ─── frontmatter：按顶层键 ─────────────────────────────────────────────────────

/// frontmatter 内部切成「键 → 这个键占的原文」。第一个键之前的注释 / 空行归键 `""`。
/// 有重复键时返回 `None`（按键合并没有定义，交给行级合并）。
fn units(inner: &str) -> Option<Vec<(&str, &str)>> {
    let mut spans: Vec<(&str, usize, usize)> = Vec::new();
    let mut offset = 0;
    for raw in inner.split_inclusive('\n') {
        let end = offset + raw.len();
        let text = raw.trim_end_matches(['\n', '\r']);
        match crate::frontmatter::key_of(text) {
            Some((key, _)) => {
                if key.is_empty() || spans.iter().any(|(known, _, _)| *known == key) {
                    return None;
                }
                spans.push((key, offset, end));
            }
            None => match spans.last_mut() {
                Some(last) => last.2 = end,
                None => spans.push(("", offset, end)),
            },
        }
        offset = end;
    }
    Some(
        spans
            .into_iter()
            .map(|(key, s, e)| (key, &inner[s..e]))
            .collect(),
    )
}

fn by_key<'a>(units: &[(&'a str, &'a str)]) -> HashMap<&'a str, &'a str> {
    units.iter().copied().collect()
}

fn merge_frontmatter(base: &str, ours: &str, theirs: &str) -> Option<Part> {
    let (b, o, t) = (units(base)?, units(ours)?, units(theirs)?);
    let (bm, om, tm) = (by_key(&b), by_key(&o), by_key(&t));

    let mut regions = 0u32;
    let mut merged: HashMap<&str, &str> = HashMap::new();
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let all_keys = b.iter().chain(&o).chain(&t).map(|(key, _)| *key);
    for key in all_keys {
        if !seen.insert(key) {
            continue;
        }
        let (bv, ov, tv) = (bm.get(key), om.get(key), tm.get(key));
        let pick = if ov == bv {
            tv
        } else if tv == bv || ov == tv {
            ov
        } else {
            regions += 1;
            None
        };
        if let Some(text) = pick {
            merged.insert(key, text);
        }
    }
    if regions > 0 {
        return Some(Part::Conflict(regions));
    }

    // 顺序以我方为准；对方新加的键插在它在对方那里的前一个键之后。
    let mut order: Vec<&str> = o
        .iter()
        .map(|(key, _)| *key)
        .filter(|key| merged.contains_key(key))
        .collect();
    let mut previous: Option<&str> = None;
    for (key, _) in &t {
        if merged.contains_key(key) && !order.contains(key) {
            let at = previous
                .and_then(|p| order.iter().position(|known| *known == p))
                .map_or_else(|| usize::from(order.first() == Some(&"")), |i| i + 1);
            order.insert(at, *key);
        }
        if order.contains(key) {
            previous = Some(*key);
        }
    }
    Some(Part::Text(order.iter().map(|key| merged[key]).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn merged(base: &str, ours: &str, theirs: &str) -> String {
        match merge3(base, ours, theirs) {
            MergeOutcome::Merged(text) => text,
            other => panic!("应能合并，实际 {other:?}"),
        }
    }

    #[test]
    fn identical_or_one_sided_changes_take_the_changed_side() {
        assert_eq!(merged("a\n", "a\n", "b\n"), "b\n");
        assert_eq!(merged("a\n", "b\n", "a\n"), "b\n");
        assert_eq!(merged("a\n", "c\n", "c\n"), "c\n");
    }

    #[test]
    fn edits_in_different_paragraphs_both_survive() {
        let base = "一\n二\n三\n四\n五\n";
        let ours = "一\n二（我改）\n三\n四\n五\n";
        let theirs = "一\n二\n三\n四\n五（对方改）\n";
        assert_eq!(
            merged(base, ours, theirs),
            "一\n二（我改）\n三\n四\n五（对方改）\n"
        );
    }

    #[test]
    fn insertions_at_both_ends_both_survive() {
        let base = "正文\n";
        assert_eq!(
            merged(base, "正文\n我加的尾巴\n", "对方加的开头\n正文\n"),
            "对方加的开头\n正文\n我加的尾巴\n"
        );
    }

    #[test]
    fn same_line_changed_differently_is_a_conflict() {
        assert_eq!(
            merge3("a\nb\nc\n", "a\nB1\nc\n", "a\nB2\nc\n"),
            MergeOutcome::Conflict { regions: 1 }
        );
    }

    #[test]
    fn adjacent_lines_changed_by_each_side_count_as_overlap() {
        assert_eq!(
            merge3("a\nb\nc\n", "a\nB\nc\n", "a\nb\nC\n"),
            MergeOutcome::Conflict { regions: 1 }
        );
    }

    #[test]
    fn same_change_on_both_sides_is_not_a_conflict() {
        assert_eq!(
            merged("a\nb\nc\nd\n", "a\nX\nc\nD\n", "a\nX\nc\nd\n"),
            "a\nX\nc\nD\n"
        );
    }

    #[test]
    fn one_side_deletes_a_line_the_other_did_not_touch() {
        assert_eq!(
            merged("a\nb\nc\nd\ne\n", "a\nc\nd\ne\n", "a\nb\nc\nd\nE\n"),
            "a\nc\nd\nE\n"
        );
    }

    #[test]
    fn delete_versus_modify_is_a_conflict() {
        assert_eq!(
            merge3("a\nb\nc\n", "a\nc\n", "a\nB\nc\n"),
            MergeOutcome::Conflict { regions: 1 }
        );
    }

    #[test]
    fn our_line_endings_win_and_lines_from_theirs_are_re_terminated() {
        let base = "a\r\nb\r\nc\r\nd\r\n";
        let ours = "A\r\nb\r\nc\r\nd\r\n";
        let theirs = "a\nb\nc\nD\n"; // 对方的编辑器存成了 LF
        assert_eq!(merged(base, ours, theirs), "A\r\nb\r\nc\r\nD\r\n");
    }

    #[test]
    fn final_newline_follows_the_side_that_changed_it() {
        assert_eq!(merged("a\nb", "A\nb", "a\nb\n"), "A\nb\n");
        assert_eq!(merged("a\nb\n", "A\nb\n", "a\nb"), "A\nb");
    }

    #[test]
    fn appending_after_a_last_line_without_newline_keeps_lines_apart() {
        assert_eq!(merged("x\ny\nz", "X\ny\nz", "x\ny\nz\nw"), "X\ny\nz\nw");
    }

    #[test]
    fn frontmatter_keys_on_adjacent_lines_merge_independently() {
        let base = "---\ntitle: 周会\ndate: 2026-10-01\ntags: [a]\n---\n正文\n";
        let ours = "---\ntitle: '周会（改）'\ndate: 2026-10-01\ntags: [a]\n---\n正文\n";
        let theirs = "---\ntitle: 周会\ndate: 2026-10-02\ntags: [a]\n---\n正文\n";
        assert_eq!(
            merged(base, ours, theirs),
            "---\ntitle: '周会（改）'\ndate: 2026-10-02\ntags: [a]\n---\n正文\n"
        );
    }

    #[test]
    fn same_key_changed_differently_is_a_conflict() {
        let base = "---\nstatus: draft\n---\n";
        assert_eq!(
            merge3(
                base,
                "---\nstatus: done\n---\n",
                "---\nstatus: review\n---\n"
            ),
            MergeOutcome::Conflict { regions: 1 }
        );
    }

    #[test]
    fn a_key_added_by_theirs_lands_after_its_neighbour_and_block_lists_move_whole() {
        let base = "---\ntitle: x\ntags:\n  - a\n  - b\nluna:\n  schema: 1\n---\n正文\n";
        let ours = "---\ntitle: x\ntags:\n  - a\n  - b\nluna:\n  schema: 1\n---\n正文（我改）\n";
        let theirs =
            "---\ntitle: x\ntags:\n  - a\n  - b\nsource: 会议\nluna:\n  schema: 1\n---\n正文\n";
        assert_eq!(
            merged(base, ours, theirs),
            "---\ntitle: x\ntags:\n  - a\n  - b\nsource: 会议\nluna:\n  schema: 1\n---\n正文（我改）\n"
        );
    }

    #[test]
    fn deleting_an_untouched_key_sticks_but_delete_versus_edit_conflicts() {
        let base = "---\na: 1\nb: 2\n---\n";
        assert_eq!(
            merged(base, "---\nb: 2\n---\n", "---\na: 1\nb: 3\n---\n"),
            "---\nb: 3\n---\n"
        );
        assert_eq!(
            merge3(base, "---\nb: 2\n---\n", "---\na: 9\nb: 2\n---\n"),
            MergeOutcome::Conflict { regions: 1 }
        );
    }

    #[test]
    fn duplicate_keys_fall_back_to_line_merge() {
        let base = "---\nk: 1\nk: 2\nz: 0\n---\n正文\n";
        let ours = "---\nk: 1\nk: 2\nz: 0\n---\n正文（我改）\n";
        let theirs = "---\nK: 1\nk: 2\nz: 0\n---\n正文\n";
        assert_eq!(
            merged(base, ours, theirs),
            "---\nK: 1\nk: 2\nz: 0\n---\n正文（我改）\n"
        );
    }

    #[test]
    fn frontmatter_on_one_side_only_merges_as_plain_lines() {
        let base = "第一段\n\n第二段\n";
        let ours = "---\ntags: [x]\n---\n第一段\n\n第二段\n";
        let theirs = "第一段\n\n第二段（对方改）\n";
        assert_eq!(
            merged(base, ours, theirs),
            "---\ntags: [x]\n---\n第一段\n\n第二段（对方改）\n"
        );
    }

    #[test]
    fn oversized_rewrites_report_too_large_instead_of_guessing() {
        let base: String = (0..1200).map(|i| format!("b{i}\n")).collect();
        let ours: String = (0..1200).map(|i| format!("o{i}\n")).collect();
        let theirs: String = (0..1200).map(|i| format!("t{i}\n")).collect();
        assert_eq!(merge3(&base, &ours, &theirs), MergeOutcome::TooLarge);
    }

    /// 性质：两边各改一组行、组与组之间至少隔一行未改动时，合并结果 = 两边的改动都在。
    #[test]
    fn separated_random_edits_always_merge_to_both_sets_of_changes() {
        let mut seed: u64 = 0x5eed_1234;
        let mut next = |bound: usize| {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (seed >> 33) as usize % bound
        };
        for round in 0..300 {
            let n = 4 + next(40);
            let base: Vec<String> = (0..n).map(|i| format!("行{i}")).collect();
            // 每行三选一：不动 / 我方改 / 对方改；同侧改动的块之间、异侧改动之间都隔一行原样。
            let mut owner = vec![0u8; n];
            let mut i = 0;
            while i < n {
                let who = next(3) as u8;
                if who != 0 {
                    owner[i] = who;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            let side = |who: u8, tag: &str| -> String {
                base.iter()
                    .zip(&owner)
                    .map(|(line, o)| {
                        if *o == who {
                            format!("{line}{tag}\n")
                        } else {
                            format!("{line}\n")
                        }
                    })
                    .collect()
            };
            let both: String = base
                .iter()
                .zip(&owner)
                .map(|(line, o)| match o {
                    1 => format!("{line}我\n"),
                    2 => format!("{line}彼\n"),
                    _ => format!("{line}\n"),
                })
                .collect();
            let base_text: String = base.iter().map(|l| format!("{l}\n")).collect();
            assert_eq!(
                merge3(&base_text, &side(1, "我"), &side(2, "彼")),
                MergeOutcome::Merged(both),
                "第 {round} 轮"
            );
        }
    }
}
