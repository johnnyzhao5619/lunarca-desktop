//! 「整理列表层级」纯函数（PRD-06-056）：折叠单行空嵌套列表链。
//!
//! 外部 AI 工具导出的会议纪要常含 `* * 文字` / `* * * * * * 文字` 这类**单行空嵌套
//! 列表链**（CommonMark 合法：只含一个子列表的列表项，直到最深层才有文字）。渲染端按
//! 规范显示是正确行为，已裁决**不为畸形内容改渲染**（TDD-15 §3.4a）；治本走内容侧，
//! 且必须是**显式动作**（预览 → 确认 → 写盘），绝不静默改用户文件（PRD-06-056）。
//!
//! 归一规则（裁决已定）：
//! - 一行行首的**连续无序标记链**（`([-*+] ){2,}` 紧跟非空白）折叠为**单个标记**，
//!   保留链首的标记字符与原有前导空白。
//! - **刻意不做**「链长→缩进层级」映射（`* * * a` → 4 空格缩进）：深链无父级时映射出
//!   的 ≥4 空格缩进会被 CommonMark 解析成缩进代码块，修复变成新的损坏。
//! - 跳过 frontmatter（定位与 [`crate::frontmatter::body_of`] 同口径，§4.4）与围栏代码块
//!   （``` 与 ~~~，含 info string；嵌套/未闭合围栏按 CommonMark 规则处理）。
//! - 有序标记链（`1. 1. `）与多行式空嵌套（`*` 独占一行 + 子项在下一行）不在 v1 范围。
//! - 除被归一行的前缀外，输出与输入**逐字节一致**（round-trip：不改行尾、不改其它行、
//!   不 trim——TDD-06 §4.4 同一纪律）。

use crate::frontmatter::body_of;

/// 一次整理的完整结果：归一后的全文 + 逐行改动清单。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListTidy {
    /// 归一后的全文。`changes` 为空时与输入逐字节相同。
    pub content: String,
    /// 被归一的行（升序）。空 = 无可归一行，调用方不应写盘。
    pub changes: Vec<ListTidyChange>,
}

/// 一行的归一前后对照（确认卡的展示单元）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListTidyChange {
    /// 1-based 行号，相对**整篇文件**（含 frontmatter，与编辑器行号同口径）。
    pub line: u32,
    /// 归一前的行内容（不含行尾 `\n`/`\r\n`）。
    pub before: String,
    /// 归一后的行内容（不含行尾 `\n`/`\r\n`）。
    pub after: String,
}

/// 围栏代码块状态：开栏字符（`` ` `` 或 `~`）与开栏长度（闭栏须同字符且不短于它）。
struct Fence {
    marker: u8,
    len: usize,
}

/// 折叠全文中的单行空嵌套列表链。纯函数、零 IO；写盘与事件归调用方（IPC 层）。
#[must_use]
pub fn tidy_list_levels(content: &str) -> ListTidy {
    // frontmatter 定位复用 §4.4 的唯一口径（`body_of`）：其覆盖的字节区间整体跳过。
    let frontmatter_len = content.len() - body_of(content).len();
    let mut out = String::with_capacity(content.len());
    let mut changes = Vec::new();
    let mut fence: Option<Fence> = None;
    let mut offset = 0usize;
    for (idx, raw_line) in content.split_inclusive('\n').enumerate() {
        let line_start = offset;
        offset += raw_line.len();
        if line_start < frontmatter_len {
            out.push_str(raw_line);
            continue;
        }
        let body_line = raw_line
            .strip_suffix('\n')
            .map(|rest| rest.strip_suffix('\r').unwrap_or(rest))
            .unwrap_or(raw_line);
        match &fence {
            Some(open) => {
                if closes_fence(body_line, open) {
                    fence = None;
                }
                out.push_str(raw_line);
                continue;
            }
            None => {
                if let Some(opened) = opens_fence(body_line) {
                    fence = Some(opened);
                    out.push_str(raw_line);
                    continue;
                }
            }
        }
        match collapse_marker_chain(body_line) {
            Some(after) => {
                let terminator = &raw_line[body_line.len()..];
                changes.push(ListTidyChange {
                    line: u32::try_from(idx + 1).unwrap_or(u32::MAX),
                    before: body_line.to_owned(),
                    after: after.clone(),
                });
                out.push_str(&after);
                out.push_str(terminator);
            }
            None => out.push_str(raw_line),
        }
    }
    ListTidy {
        content: out,
        changes,
    }
}

/// 该行是否开一个围栏代码块（CommonMark：≤3 空格缩进 + ≥3 个 `` ` `` 或 `~`；
/// backtick 围栏的 info string 不得再含 `` ` ``）。
fn opens_fence(line: &str) -> Option<Fence> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let marker = *rest.as_bytes().first()?;
    if marker != b'`' && marker != b'~' {
        return None;
    }
    let len = rest.bytes().take_while(|b| *b == marker).count();
    if len < 3 {
        return None;
    }
    let info = &rest[len..];
    if marker == b'`' && info.contains('`') {
        return None;
    }
    Some(Fence { marker, len })
}

/// 该行是否闭合 `open` 围栏（同字符、不短于开栏、除尾随空白外无其它内容）。
fn closes_fence(line: &str, open: &Fence) -> bool {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return false;
    }
    let rest = &line[indent..];
    let len = rest.bytes().take_while(|b| *b == open.marker).count();
    len >= open.len && rest[len..].trim().is_empty()
}

/// 折叠一行行首的无序标记链。命中（`([-*+] ){2,}` 紧跟非空白）返回归一后的行
/// （保留前导空白与链首标记字符），未命中返回 `None`。
fn collapse_marker_chain(line: &str) -> Option<String> {
    let indent_len = line.len() - line.trim_start_matches([' ', '\t']).len();
    let rest = &line.as_bytes()[indent_len..];
    let mut pairs = 0usize;
    let mut cursor = 0usize;
    while matches!(rest.get(cursor), Some(b'-' | b'*' | b'+'))
        && rest.get(cursor + 1) == Some(&b' ')
    {
        pairs += 1;
        cursor += 2;
    }
    if pairs < 2 {
        return None;
    }
    // 链后必须**紧跟非空白**：`* * ` 之后是空白或行尾时，这行没有内容可救，不动。
    let tail = &line[indent_len + cursor..];
    if tail.chars().next().is_none_or(char::is_whitespace) {
        return None;
    }
    let first_marker = rest[0] as char;
    Some(format!("{}{first_marker} {tail}", &line[..indent_len]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试助手：只取 (行号, before → after) 三元组，断言不必逐字段展开结构体。
    fn rows(tidy: &ListTidy) -> Vec<(u32, String, String)> {
        tidy.changes
            .iter()
            .map(|c| (c.line, c.before.clone(), c.after.clone()))
            .collect()
    }

    #[test]
    fn collapses_chains_of_various_lengths() {
        // 2 / 6 / 10 连链都折叠成单标记；行号 1-based。
        let input = "* * 二连\n* * * * * * 六连\n* * * * * * * * * * 十连\n";
        let tidy = tidy_list_levels(input);
        assert_eq!(tidy.content, "* 二连\n* 六连\n* 十连\n");
        assert_eq!(
            rows(&tidy),
            vec![
                (1, "* * 二连".into(), "* 二连".into()),
                (2, "* * * * * * 六连".into(), "* 六连".into()),
                (3, "* * * * * * * * * * 十连".into(), "* 十连".into()),
            ]
        );
    }

    #[test]
    fn keeps_leading_whitespace() {
        let tidy = tidy_list_levels("  * * 缩进两格\n\t- - 制表符缩进\n");
        assert_eq!(tidy.content, "  * 缩进两格\n\t- 制表符缩进\n");
    }

    #[test]
    fn keeps_first_marker_char_of_the_chain() {
        // `- - ` / `+ + ` / 混合 `- * `：保留的都是**链首**那个字符。
        let tidy = tidy_list_levels("- - 甲\n+ + 乙\n- * 丙\n* - + 丁\n");
        assert_eq!(tidy.content, "- 甲\n+ 乙\n- 丙\n* 丁\n");
    }

    #[test]
    fn skips_frontmatter() {
        // frontmatter 里的 `* * `（如 YAML 注释/字符串）一个字节不动。
        let input = "---\ntitle: x\n# * * 注释\n---\n* * 正文\n";
        let tidy = tidy_list_levels(input);
        assert_eq!(tidy.content, "---\ntitle: x\n# * * 注释\n---\n* 正文\n");
        assert_eq!(rows(&tidy), vec![(5, "* * 正文".into(), "* 正文".into())]);
    }

    #[test]
    fn skips_fenced_code_blocks() {
        let input = "```md\n* * 栏内\n```\n* * 栏外\n~~~text info\n* * 波浪栏内\n~~~\n";
        let tidy = tidy_list_levels(input);
        assert_eq!(
            tidy.content,
            "```md\n* * 栏内\n```\n* 栏外\n~~~text info\n* * 波浪栏内\n~~~\n"
        );
        assert_eq!(rows(&tidy), vec![(4, "* * 栏外".into(), "* 栏外".into())]);
    }

    #[test]
    fn unclosed_fence_covers_rest_of_file() {
        // CommonMark：未闭合围栏一直开到文末——其后所有行都是代码，不归一。
        let tidy = tidy_list_levels("```\n* * 未闭合\n* * 仍在栏内\n");
        assert!(tidy.changes.is_empty());
        assert_eq!(tidy.content, "```\n* * 未闭合\n* * 仍在栏内\n");
    }

    #[test]
    fn nested_fence_of_other_marker_is_content() {
        // ~~~ 栏内出现 ``` 只是内容，不会「就近闭合」（闭栏须同字符同长起）。
        let input = "~~~\n```\n* * 仍在栏内\n~~~\n* * 栏外\n";
        let tidy = tidy_list_levels(input);
        assert_eq!(rows(&tidy), vec![(5, "* * 栏外".into(), "* 栏外".into())]);
    }

    #[test]
    fn shorter_close_does_not_end_longer_fence() {
        // 五连开栏须 ≥5 连闭合；三连那行只是内容。
        let tidy = tidy_list_levels("`````\n```\n* * 仍在栏内\n`````\n* * 栏外\n");
        assert_eq!(rows(&tidy), vec![(5, "* * 栏外".into(), "* 栏外".into())]);
    }

    #[test]
    fn mid_line_chain_is_untouched() {
        // 链必须在行首（前导空白后）；行中间的 `* * ` 是正文。
        let tidy = tidy_list_levels("正文里的 * * 不动\n> * * 引用块里也不动\n");
        assert!(tidy.changes.is_empty());
    }

    #[test]
    fn single_marker_and_task_boxes_are_untouched() {
        // 单标记（正常列表）、任务框 `- [ ]`、强调 `* *bold*`（第二个 `*` 后无空格）都不动。
        let tidy = tidy_list_levels("- 正常项\n- [ ] 待办\n- [x] 已办\n* *强调* 文本\n");
        assert!(tidy.changes.is_empty());
    }

    #[test]
    fn chain_followed_by_task_box_collapses_to_task_item() {
        let tidy = tidy_list_levels("- - [ ] 深链待办\n");
        assert_eq!(tidy.content, "- [ ] 深链待办\n");
    }

    #[test]
    fn chain_with_no_content_is_untouched() {
        // `紧跟非空白` 是硬条件：链后是行尾或空白时这行没有内容可救，不动。
        let tidy = tidy_list_levels("* * \n* *  隔两格\n* *\t制表符\n");
        assert!(tidy.changes.is_empty());
    }

    #[test]
    fn preserves_crlf_and_missing_final_newline() {
        // CRLF 行尾原样保留；末行无换行也原样——round-trip 是逐字节承诺。
        let tidy = tidy_list_levels("* * 甲\r\n普通行\r\n* * 尾行无换行");
        assert_eq!(tidy.content, "* 甲\r\n普通行\r\n* 尾行无换行");
        assert_eq!(
            rows(&tidy),
            vec![
                (1, "* * 甲".into(), "* 甲".into()),
                (3, "* * 尾行无换行".into(), "* 尾行无换行".into()),
            ]
        );
    }

    #[test]
    fn no_change_means_byte_identical_output() {
        for input in ["", "只有一行\n", "# 标题\n\n段落，无列表。"] {
            let tidy = tidy_list_levels(input);
            assert!(tidy.changes.is_empty(), "不该有改动：{input:?}");
            assert_eq!(tidy.content, input, "零改动须逐字节相同");
        }
    }

    #[test]
    fn unclosed_frontmatter_is_body() {
        // §4.4 同口径（`find_frontmatter`）：无闭合分隔符 = 无 frontmatter，整体当正文——
        // 于是其中的链照常归一（保守口径的另一面：绝不把正文误当头部跳过）。
        let tidy = tidy_list_levels("---\n未闭合\n* * 这行是正文\n");
        assert_eq!(
            rows(&tidy),
            vec![(3, "* * 这行是正文".into(), "* 这行是正文".into())]
        );
    }
}
