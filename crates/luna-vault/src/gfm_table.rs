//! GFM 表格的纯文本处理（表格 AI 动作，PRD-06-011 / 对标方案 T6）：
//!
//! - [`extract_table`]：从模型输出里取出**一张合法的 GFM 表**（模型常常在表前后加一句话、
//!   或包一层代码围栏）；取不到就是 `None`，调用方据此不落地并说明原因——宁可不给，
//!   不把一段说明文字当表格替换进正文。
//! - [`table_cells`]：表格源码 → 各行单元格文字（含表头），供逐行填写时给模型看这一行。
//! - [`replace_column`]：只改某一列的正文格，其余字节原样（ADR-138 ②「只改受影响的行」
//!   的同一纪律：AI 填一列，diff 里就只有那一列）。
//!
//! 行切分与前端 `table-model.ts` 同一口径：未转义的 `|` 为界，首尾管道可省。

/// 一行表格源码 → 各格原始片段在行内的字节区间（不含两侧空白的修剪）。
fn segments(line: &str) -> Vec<(usize, usize)> {
    let bytes = line.as_bytes();
    let mut pipes = Vec::new();
    let mut escapes = 0usize;
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'|' && escapes.is_multiple_of(2) {
            pipes.push(i);
        }
        escapes = if *b == b'\\' { escapes + 1 } else { 0 };
    }
    if pipes.is_empty() {
        return vec![(0, line.len())];
    }
    let mut out = Vec::new();
    if !line[..pipes[0]].trim().is_empty() {
        out.push((0, pipes[0]));
    }
    for pair in pipes.windows(2) {
        out.push((pair[0] + 1, pair[1]));
    }
    let last = pipes[pipes.len() - 1];
    if !line[last + 1..].trim().is_empty() {
        out.push((last + 1, line.len()));
    }
    out
}

fn is_delimiter_row(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.contains('-')
        && trimmed
            .chars()
            .all(|c| matches!(c, '|' | ':' | '-' | ' ' | '\t'))
}

fn cells(line: &str) -> Vec<String> {
    segments(line)
        .into_iter()
        .map(|(from, to)| line[from..to].trim().to_owned())
        .collect()
}

/// 从一段文字里取出第一张合法 GFM 表（表头行 + 分隔行 + 至少一行正文），行与行之间的列数
/// 按表头补齐 / 截断；前后的说明文字与代码围栏一律丢掉。取不到返回 `None`。
#[must_use]
pub fn extract_table(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    for start in 0..lines.len().saturating_sub(1) {
        let header = lines[start];
        if !header.contains('|') || !is_delimiter_row(lines[start + 1]) {
            continue;
        }
        let columns = cells(header).len();
        if columns < 2 || cells(lines[start + 1]).len() != columns {
            continue;
        }
        let body: Vec<&str> = lines[start + 2..]
            .iter()
            .take_while(|line| line.contains('|') && !line.trim().is_empty())
            .copied()
            .collect();
        if body.is_empty() {
            continue;
        }
        let row = |line: &str| {
            let mut values = cells(line);
            values.resize(columns, String::new());
            format!("| {} |", values.join(" | "))
        };
        let mut out = vec![row(header), row(lines[start + 1])];
        out.extend(body.into_iter().map(row));
        return Some(out.join("\n"));
    }
    None
}

/// 表格源码 → 各行单元格文字（第 0 行是表头，分隔行不在其中）。不是表格返回 `None`。
#[must_use]
pub fn table_cells(source: &str) -> Option<Vec<Vec<String>>> {
    let lines: Vec<&str> = source.lines().collect();
    if lines.len() < 2 || !is_delimiter_row(lines[1]) {
        return None;
    }
    let mut out = vec![cells(lines[0])];
    out.extend(lines[2..].iter().map(|line| cells(line)));
    Some(out)
}

/// 单元格里放得下的写法：换行写成 `<br>`，未转义的 `|` 转义。
#[must_use]
pub fn cell_text(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut escapes = 0usize;
    for ch in value.trim().chars() {
        match ch {
            '\n' => out.push_str("<br>"),
            '\r' => {}
            '|' if escapes.is_multiple_of(2) => out.push_str("\\|"),
            _ => out.push(ch),
        }
        escapes = if ch == '\\' { escapes + 1 } else { 0 };
    }
    out
}

/// 把第 `column` 列的正文格依次换成 `values`（第 0 个值给第 1 行正文），只改这些格，
/// 其余字节原样；短行缺的格在行尾补齐。不是表格、列号越界或值的个数不对返回 `None`。
#[must_use]
pub fn replace_column(source: &str, column: usize, values: &[String]) -> Option<String> {
    let lines: Vec<&str> = source.lines().collect();
    if lines.len() < 2 || !is_delimiter_row(lines[1]) || column >= cells(lines[0]).len() {
        return None;
    }
    if values.len() != lines.len() - 2 {
        return None;
    }
    let mut out: Vec<String> = lines[..2].iter().map(|line| (*line).to_owned()).collect();
    for (line, value) in lines[2..].iter().zip(values) {
        let value = cell_text(value);
        let segs = segments(line);
        if let Some((from, to)) = segs.get(column) {
            let raw = &line[*from..*to];
            // 原格两侧的空白照留；空格（`|  |`）留左右各一个空格，值写在中间。
            let (lead, trail) = if raw.trim().is_empty() {
                (raw.len().min(1), usize::from(raw.len() >= 2))
            } else {
                (
                    raw.len() - raw.trim_start().len(),
                    raw.len() - raw.trim_end().len(),
                )
            };
            out.push(format!(
                "{}{}{}",
                &line[..from + lead],
                value,
                &line[to - trail..]
            ));
        } else {
            let trimmed = line.trim_end();
            let closed = trimmed.ends_with('|') && !trimmed.ends_with("\\|");
            let mut next = if closed {
                trimmed.to_owned()
            } else {
                format!("{trimmed} |")
            };
            for c in segs.len()..=column {
                next.push_str(&format!(
                    " {} |",
                    if c == column { value.as_str() } else { "" }
                ));
            }
            out.push(next);
        }
    }
    Some(out.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_the_table_from_chatty_output() {
        let output = "好的，整理如下：\n\n```markdown\n| 名称 | 数量 |\n|---|---|\n| 苹果 | 3 |\n| 梨 |\n```\n希望有帮助。";
        assert_eq!(
            extract_table(output).as_deref(),
            Some("| 名称 | 数量 |\n| --- | --- |\n| 苹果 | 3 |\n| 梨 |  |")
        );
    }

    #[test]
    fn refuses_output_without_a_real_table() {
        assert_eq!(extract_table("抱歉，这段文字看不出表格结构。"), None);
        assert_eq!(extract_table("| 只有表头 | 没有正文 |\n|---|---|"), None);
        assert_eq!(extract_table("| 单列 |\n|---|\n| x |"), None);
    }

    #[test]
    fn reads_cells_and_replaces_only_one_column() {
        let source = "| 产品 | 卖点 | 价 |\n|:--|---|--:|\n| 灯 |  | 9 |\n| 椅 | 旧 | 99 |\n| 桌 |";
        assert_eq!(
            table_cells(source).unwrap(),
            vec![
                vec!["产品", "卖点", "价"],
                vec!["灯", "", "9"],
                vec!["椅", "旧", "99"],
                vec!["桌"]
            ]
        );
        let values = vec!["很亮".to_owned(), "a|b".to_owned(), "两行\n文字".to_owned()];
        assert_eq!(
            replace_column(source, 1, &values).as_deref(),
            Some("| 产品 | 卖点 | 价 |\n|:--|---|--:|\n| 灯 | 很亮 | 9 |\n| 椅 | a\\|b | 99 |\n| 桌 | 两行<br>文字 |")
        );
        assert_eq!(replace_column(source, 3, &values), None);
        assert_eq!(replace_column(source, 1, &values[..1]), None);
    }
}
