//! 行级差异（PRD-06-041 版本对比）。零依赖纯函数，可 headless 重测。
//!
//! 版本历史此前只能「读一段全文再决定要不要回滚」——两屏文字肉眼比对，等于没有对比。
//! 本模块给出**统一（unified）行差异**：相同行、新增行、删除行，各带新旧行号。
//!
//! 算法 = **公共前后缀裁剪 + 中段 LCS**。裁剪把"改了三行的一万行文档"压到三行的规模；
//! 中段仍过大时（[`LCS_CELL_BUDGET`]）**诚实降级**为「整段删除 + 整段新增」并置
//! [`LineDiff::degraded`]，绝不为了看起来更聪明而吃掉几百 MB 内存。

/// 一行差异的类别。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffKind {
    /// 两侧相同。
    Equal,
    /// 仅新版本有（新增）。
    Added,
    /// 仅旧版本有（删除）。
    Removed,
}

/// 一行差异。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffKind,
    /// 旧版本行号（1-based；新增行为 `None`）。
    pub old_line: Option<u32>,
    /// 新版本行号（1-based；删除行为 `None`）。
    pub new_line: Option<u32>,
    pub text: String,
}

/// 一次对比的结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LineDiff {
    pub lines: Vec<DiffLine>,
    pub added: u32,
    pub removed: u32,
    /// 中段过大，已降级为整段替换（对比粒度变粗，UI 须如实告知）。
    pub degraded: bool,
}

/// LCS 动态规划的格子预算（约 4 MB `u32`）。超出即降级，不做无上限分配。
const LCS_CELL_BUDGET: usize = 1_000_000;

/// 计算旧→新的统一行差异。
#[must_use]
pub fn diff_lines(old: &str, new: &str) -> LineDiff {
    let old_lines: Vec<&str> = split_lines(old);
    let new_lines: Vec<&str> = split_lines(new);

    let mut out = LineDiff::default();

    // 公共前缀。
    let mut head = 0;
    while head < old_lines.len() && head < new_lines.len() && old_lines[head] == new_lines[head] {
        push(
            &mut out,
            DiffKind::Equal,
            Some(head),
            Some(head),
            old_lines[head],
        );
        head += 1;
    }
    // 公共后缀（不与前缀重叠）。
    let mut tail = 0;
    while tail < old_lines.len() - head
        && tail < new_lines.len() - head
        && old_lines[old_lines.len() - 1 - tail] == new_lines[new_lines.len() - 1 - tail]
    {
        tail += 1;
    }

    let old_mid = &old_lines[head..old_lines.len() - tail];
    let new_mid = &new_lines[head..new_lines.len() - tail];

    if old_mid.len().saturating_mul(new_mid.len()) > LCS_CELL_BUDGET {
        out.degraded = true;
        for (i, line) in old_mid.iter().enumerate() {
            push(&mut out, DiffKind::Removed, Some(head + i), None, line);
        }
        for (i, line) in new_mid.iter().enumerate() {
            push(&mut out, DiffKind::Added, None, Some(head + i), line);
        }
    } else {
        emit_lcs(&mut out, old_mid, new_mid, head, head);
    }

    for i in 0..tail {
        let old_idx = old_lines.len() - tail + i;
        let new_idx = new_lines.len() - tail + i;
        push(
            &mut out,
            DiffKind::Equal,
            Some(old_idx),
            Some(new_idx),
            old_lines[old_idx],
        );
    }
    out
}

/// 按 `\n` 切行并剥掉 `\r`；末尾换行不产出一条空行（否则每篇笔记都多一行伪差异）。
fn split_lines(text: &str) -> Vec<&str> {
    let trimmed = text.strip_suffix('\n').unwrap_or(text);
    if trimmed.is_empty() && text.is_empty() {
        return Vec::new();
    }
    trimmed
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect()
}

fn push(
    out: &mut LineDiff,
    kind: DiffKind,
    old_idx: Option<usize>,
    new_idx: Option<usize>,
    text: &str,
) {
    match kind {
        DiffKind::Added => out.added += 1,
        DiffKind::Removed => out.removed += 1,
        DiffKind::Equal => {}
    }
    out.lines.push(DiffLine {
        kind,
        old_line: old_idx.map(|i| (i + 1) as u32),
        new_line: new_idx.map(|i| (i + 1) as u32),
        text: text.to_string(),
    });
}

/// 后缀最长公共子序列表：`table[i * cols + j]` = `old[i..]` 与 `new[j..]` 的 LCS 长度，
/// `cols = new.len() + 1`。差异回溯与三路合并的行配对（[`lcs_pairs`]）共用这一张表。
fn lcs_table<T: PartialEq>(old: &[T], new: &[T]) -> Vec<u32> {
    let cols = new.len() + 1;
    let mut table = vec![0u32; (old.len() + 1) * cols];
    for i in (0..old.len()).rev() {
        for j in (0..new.len()).rev() {
            table[i * cols + j] = if old[i] == new[j] {
                table[(i + 1) * cols + j + 1] + 1
            } else {
                table[(i + 1) * cols + j].max(table[i * cols + j + 1])
            };
        }
    }
    table
}

/// 两组行的最长公共子序列配对 `(old 下标, new 下标)`，升序，含公共前后缀。
///
/// 与 [`diff_lines`] 同一套「前后缀裁剪 + 中段 LCS」与同一个格子预算；中段超出预算时返回
/// `None`——三路合并据此如实说「太大，合并不了」，而不是退化成一个看似成功的粗粒度结果。
pub(crate) fn lcs_pairs<T: PartialEq>(old: &[T], new: &[T]) -> Option<Vec<(usize, usize)>> {
    let mut head = 0;
    while head < old.len() && head < new.len() && old[head] == new[head] {
        head += 1;
    }
    let mut tail = 0;
    while tail < old.len() - head
        && tail < new.len() - head
        && old[old.len() - 1 - tail] == new[new.len() - 1 - tail]
    {
        tail += 1;
    }
    let old_mid = &old[head..old.len() - tail];
    let new_mid = &new[head..new.len() - tail];
    if old_mid.len().saturating_mul(new_mid.len()) > LCS_CELL_BUDGET {
        return None;
    }

    let mut pairs: Vec<(usize, usize)> = (0..head).map(|k| (k, k)).collect();
    let table = lcs_table(old_mid, new_mid);
    let cols = new_mid.len() + 1;
    let (mut i, mut j) = (0usize, 0usize);
    while i < old_mid.len() && j < new_mid.len() {
        if old_mid[i] == new_mid[j] {
            pairs.push((head + i, head + j));
            i += 1;
            j += 1;
        } else if table[(i + 1) * cols + j] >= table[i * cols + j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    pairs.extend((0..tail).map(|k| (old.len() - tail + k, new.len() - tail + k)));
    Some(pairs)
}

/// 中段最长公共子序列回溯，产出统一差异（删除先于同位置的新增，读起来是"改成"）。
fn emit_lcs(out: &mut LineDiff, old: &[&str], new: &[&str], old_base: usize, new_base: usize) {
    let cols = new.len() + 1;
    let table = lcs_table(old, new);

    let (mut i, mut j) = (0usize, 0usize);
    while i < old.len() && j < new.len() {
        if old[i] == new[j] {
            push(
                out,
                DiffKind::Equal,
                Some(old_base + i),
                Some(new_base + j),
                old[i],
            );
            i += 1;
            j += 1;
        } else if table[(i + 1) * cols + j] >= table[i * cols + j + 1] {
            push(out, DiffKind::Removed, Some(old_base + i), None, old[i]);
            i += 1;
        } else {
            push(out, DiffKind::Added, None, Some(new_base + j), new[j]);
            j += 1;
        }
    }
    while i < old.len() {
        push(out, DiffKind::Removed, Some(old_base + i), None, old[i]);
        i += 1;
    }
    while j < new.len() {
        push(out, DiffKind::Added, None, Some(new_base + j), new[j]);
        j += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(diff: &LineDiff) -> Vec<(DiffKind, &str)> {
        diff.lines
            .iter()
            .map(|l| (l.kind, l.text.as_str()))
            .collect()
    }

    #[test]
    fn identical_text_has_no_changes() {
        let diff = diff_lines("a\nb\n", "a\nb\n");
        assert_eq!(diff.added, 0);
        assert_eq!(diff.removed, 0);
        assert_eq!(
            kinds(&diff),
            vec![(DiffKind::Equal, "a"), (DiffKind::Equal, "b")]
        );
    }

    #[test]
    fn single_line_edit_keeps_surrounding_context() {
        let diff = diff_lines("头\n旧\n尾\n", "头\n新\n尾\n");
        assert_eq!(
            kinds(&diff),
            vec![
                (DiffKind::Equal, "头"),
                (DiffKind::Removed, "旧"),
                (DiffKind::Added, "新"),
                (DiffKind::Equal, "尾"),
            ]
        );
        assert_eq!((diff.added, diff.removed), (1, 1));
    }

    #[test]
    fn line_numbers_track_both_sides() {
        let diff = diff_lines("a\nb\nc\n", "a\nc\n");
        let removed = diff
            .lines
            .iter()
            .find(|l| l.kind == DiffKind::Removed)
            .expect("有删除行");
        assert_eq!((removed.old_line, removed.new_line), (Some(2), None));
        let last = diff.lines.last().expect("有尾行");
        assert_eq!((last.old_line, last.new_line), (Some(3), Some(2)));
    }

    #[test]
    fn insertion_at_end_is_added_only() {
        let diff = diff_lines("a\n", "a\nb\n");
        assert_eq!(
            kinds(&diff),
            vec![(DiffKind::Equal, "a"), (DiffKind::Added, "b")]
        );
    }

    #[test]
    fn empty_old_marks_everything_added() {
        let diff = diff_lines("", "a\nb\n");
        assert_eq!(diff.added, 2);
        assert_eq!(diff.removed, 0);
    }

    #[test]
    fn crlf_does_not_show_up_as_a_difference() {
        let diff = diff_lines("a\r\nb\r\n", "a\nb\n");
        assert_eq!(diff.added, 0);
        assert_eq!(diff.removed, 0);
    }

    #[test]
    fn oversized_middle_degrades_instead_of_allocating() {
        let old: String = (0..1200).map(|i| format!("old-{i}\n")).collect();
        let new: String = (0..1200).map(|i| format!("new-{i}\n")).collect();
        let diff = diff_lines(&old, &new);
        assert!(diff.degraded);
        assert_eq!(diff.added, 1200);
        assert_eq!(diff.removed, 1200);
    }
}
