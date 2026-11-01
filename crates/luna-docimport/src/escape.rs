//! 去掉 Markdown 里语法上不必要的反斜杠转义（ADR-164 ①）。
//!
//! HTML → Markdown 的序列化器把 `.` `!` `(` `)` `#` `+` `-` `{` `}` `>` 一律转义（`her\.`）：
//! 渲染无差，但资料笔记是要在别的编辑器里直接读的文件，满屏反斜杠读不下去。这里只留真正会
//! 改变语义的那几处；`*` `_` `` ` `` `[` `]` `<` `|` `\` 原样保留；代码块与行内代码一律不动。

/// 只留必要的转义。
#[must_use]
pub fn tidy_escapes(markdown: &str) -> String {
    let mut out = String::with_capacity(markdown.len());
    let mut fence: Option<String> = None;
    for (index, line) in markdown.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let trimmed = line.trim_start();
        if let Some(marker) = &fence {
            if trimmed.starts_with(marker.as_str()) {
                fence = None;
            }
            out.push_str(line);
            continue;
        }
        if let Some(marker) = fence_marker(trimmed) {
            fence = Some(marker);
            out.push_str(line);
            continue;
        }
        tidy_line(line, trimmed.starts_with('|'), &mut out);
    }
    out
}

/// 围栏代码块的起始标记（三个以上的 `` ` `` 或 `~`）。
fn fence_marker(trimmed: &str) -> Option<String> {
    for ch in ['`', '~'] {
        let count = trimmed.chars().take_while(|c| *c == ch).count();
        if count >= 3 {
            return Some(ch.to_string().repeat(count));
        }
    }
    None
}

/// `table_row`：这一行是 GFM 表格行（以 `|` 开头），`\\|` 必须留着。
fn tidy_line(line: &str, table_row: bool, out: &mut String) {
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    // 已输出到本行的内容（判断「行首」与「前一个字符」用的是输出后的样子）。
    let mut emitted = String::new();
    while i < chars.len() {
        let c = chars[i];
        // 行内代码：原样跳过整段（与开头同样长度的反引号收尾）。
        if c == '`' {
            let run = chars[i..].iter().take_while(|c| **c == '`').count();
            let close = (i + run..chars.len())
                .find(|&j| chars[j..].iter().take_while(|c| **c == '`').count() == run);
            let end = close.map_or(i + run, |j| j + run);
            emitted.extend(&chars[i..end]);
            i = end;
            continue;
        }
        if c == '\\' && i + 1 < chars.len() {
            let next = chars[i + 1];
            let keep = if next == '|' {
                table_row
            } else {
                next == '\\' || keep_escape(&emitted, next, chars.get(i + 2).copied())
            };
            if keep {
                emitted.push(c);
                emitted.push(next);
            } else {
                emitted.push(next);
            }
            i += 2;
            continue;
        }
        emitted.push(c);
        i += 1;
    }
    out.push_str(&emitted);
}

/// `before` = 本行已输出的内容，`ch` = 被转义的字符，`after` = 它后面那个字符。
fn keep_escape(before: &str, ch: char, after: Option<char>) -> bool {
    let at_line_start = before.trim_start().is_empty();
    match ch {
        // 行首的数字后面跟 `.` 会变成有序列表。
        '.' => {
            let lead = before.trim_start();
            !lead.is_empty() && lead.chars().all(|c| c.is_ascii_digit())
        }
        // `![` 是图片。
        '!' => after == Some('['),
        // `](` 是链接目标。
        '(' => before.ends_with(']'),
        ')' => false,
        // 行首的 `#` 是标题，`>` 是引用。
        '#' | '>' => at_line_start,
        // 行首的 `+ ` / `- ` 是列表。
        '+' | '-' => at_line_start && matches!(after, None | Some(' ')),
        '{' | '}' => false,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_needless_escapes_in_prose() {
        assert_eq!(
            tidy_escapes(r"I’m filling in for her\. Hi\! \(see\) \{x\} a\-b a \# b"),
            "I’m filling in for her. Hi! (see) {x} a-b a # b"
        );
    }

    /// `\!\[` 里的 `[` 本身已转义，`!` 不会开出图片，它的转义可以去掉。
    #[test]
    fn keeps_escapes_that_change_meaning() {
        let md = "1\\. 不是列表\n\\# 不是标题\n\\> 不是引用\n\\- 不是列表\n\\!\\[不是图片\\]\n\\[x\\]\\(y\\)\n\\*星号\\* \\_下划线\\_ \\| 竖线 \\\\ 反斜杠";
        assert_eq!(
            tidy_escapes(md),
            "1\\. 不是列表\n\\# 不是标题\n\\> 不是引用\n\\- 不是列表\n!\\[不是图片\\]\n\\[x\\]\\(y)\n\\*星号\\* \\_下划线\\_ | 竖线 \\\\ 反斜杠"
        );
    }

    #[test]
    fn code_is_left_alone() {
        let md = "前 `a\\.b` 后\\.\n```\nx\\.y\n```\n尾\\.";
        assert_eq!(tidy_escapes(md), "前 `a\\.b` 后.\n```\nx\\.y\n```\n尾.");
    }

    #[test]
    fn pipes_outside_tables_lose_their_escape() {
        assert_eq!(tidy_escapes("David Chen \\| Author"), "David Chen | Author");
    }

    #[test]
    fn tables_keep_pipe_escapes() {
        assert_eq!(tidy_escapes("| a \\| b | c\\. |"), "| a \\| b | c. |");
    }
}
