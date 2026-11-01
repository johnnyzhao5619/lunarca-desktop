//! 剪贴板 HTML → 可移植 Markdown。纯本地、零网络、不运行 HTML。
use dom_query::Document;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PasteWarning {
    FormattingSimplified,
    ComplexTable,
    MissingHeader,
    ImagesOmitted,
    UnsafeLinks,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HtmlPaste {
    pub markdown: String,
    pub warnings: Vec<PasteWarning>,
    pub block: bool,
    /// 无表头来源的显式首行表头版本；默认版本仍保留首行为数据。
    pub header_markdown: Option<String>,
    /// 仅当剪贴板全部有效内容是一张表时提供，供现有表格矩形填充。
    pub table_cells: Option<Vec<Vec<String>>>,
}

fn table_markdown(rows: &[Vec<String>], columns: usize, has_header: bool) -> String {
    let row_line = |row: &[String]| {
        format!(
            "| {} |",
            (0..columns)
                .map(|c| row.get(c).cloned().unwrap_or_default())
                .collect::<Vec<_>>()
                .join(" | ")
        )
    };
    let delimiter = format!("| {} |", vec!["---"; columns].join(" | "));
    let mut lines = Vec::new();
    if !has_header {
        lines.push(row_line(&vec![String::new(); columns]));
        lines.push(delimiter.clone());
    }
    for (index, row) in rows.iter().enumerate() {
        lines.push(row_line(row));
        if index == 0 && has_header {
            lines.push(delimiter.clone());
        }
    }
    lines.join("\n")
}

/// 相对路径（没有协议、不以 `/` 或 `\\` 开头）。
fn is_relative_src(src: &str) -> bool {
    let src = src.trim();
    !src.is_empty() && !src.contains(':') && !src.starts_with('/') && !src.starts_with('\\')
}

fn warn(warnings: &mut Vec<PasteWarning>, warning: PasteWarning) {
    if !warnings.contains(&warning) {
        warnings.push(warning);
    }
}

pub fn convert_html(html: &str) -> HtmlPaste {
    convert_html_with(html, false)
}

/// 同 [`convert_html`]，`keep_relative_images` 为真时保留 `src` 是相对路径的图片——只给由
/// Lunarca 自己生成、图片已落进附件目录的 HTML 用（文档导入，ADR-164 ⑥）。粘贴与本地网页文件
/// 仍走 [`convert_html`]：那里的相对路径指向库外，留着就是一条坏链。
pub fn convert_html_with(html: &str, keep_relative_images: bool) -> HtmlPaste {
    let doc = Document::from(html);
    let mut warnings = Vec::new();
    if !doc.select("span[style], font, svg, math").is_empty() {
        warn(&mut warnings, PasteWarning::FormattingSimplified);
    }
    doc.select("script, style, head, iframe, object, embed, template, noscript")
        .remove();
    for link in doc.select("a[href]").nodes() {
        let href = link.attr("href").unwrap_or_default();
        let lower = href.trim().to_ascii_lowercase();
        if lower.contains(':')
            && !["http:", "https:", "mailto:"]
                .iter()
                .any(|scheme| lower.starts_with(scheme))
        {
            link.remove_attr("href");
            warn(&mut warnings, PasteWarning::UnsafeLinks);
        }
    }
    // 不让复制网页悄悄触发远程图片下载；保留 alt，显式提示损失。
    for image in doc.select("img").nodes() {
        if keep_relative_images && image.attr("src").is_some_and(|src| is_relative_src(&src)) {
            continue;
        }
        let alt = image.attr("alt").unwrap_or_default();
        let escaped = alt
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        // 空替代文字要显式删掉：`replace_with_html("")` 在 dom_query 里不生效，远程图片会原样
        // 漏进 Markdown（真机 2026-10-19，剪藏网页时发现；粘贴同样受影响）。
        if escaped.trim().is_empty() {
            image.remove_from_parent();
        } else {
            image.replace_with_html(escaped);
        }
        warn(&mut warnings, PasteWarning::ImagesOmitted);
    }
    // 只包着图片的链接（「点图看大图」）要拆掉：序列化器会把整段 `<a><img></a>` 吞掉，图就没了
    // （真机 2026-10-19，剪藏新浪博客）。图片的 title 也去掉——带换行的 title 会写进图片语法里。
    for image in doc.select("img").nodes() {
        image.remove_attrs(&["title", "TITLE"]);
    }
    for link in doc.select("a").nodes() {
        if link.text().trim().is_empty() && !link.find(&["img"]).is_empty() {
            let inner = link.inner_html();
            link.replace_with_html(inner);
        }
    }
    let block = !doc
        .select("h1, h2, h3, h4, h5, h6, p, ul, ol, table, pre, blockquote")
        .is_empty();
    let mut tables = Vec::new();
    for table in doc.select("table").nodes() {
        if table.is("table table") {
            continue;
        }
        let complex = table.is("table:has(table), table:has([rowspan]), table:has([colspan])");
        let rows: Vec<Vec<String>> = table
            .find(&["tr"])
            .into_iter()
            .filter(|row| {
                row.ancestors_it(None)
                    .find(|parent| parent.is("table"))
                    .is_some_and(|parent| parent.id == table.id)
            })
            .map(|row| {
                row.children()
                    .into_iter()
                    .filter(|cell| cell.is("th, td"))
                    .map(|cell| {
                        cell.md(None)
                            .trim()
                            .lines()
                            .map(str::trim_end)
                            .collect::<Vec<_>>()
                            .join("<br>")
                    })
                    .collect()
            })
            .filter(|row: &Vec<String>| !row.is_empty())
            .collect();
        if rows.is_empty() {
            continue;
        }
        let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
        if complex || rows.iter().any(|row| row.len() != columns) {
            warn(&mut warnings, PasteWarning::ComplexTable);
        }
        let has_header = table
            .find(&["tr"])
            .first()
            .is_some_and(|row| row.children().into_iter().any(|cell| cell.is("th")));
        if !has_header {
            warn(&mut warnings, PasteWarning::MissingHeader);
        }
        let default_table = table_markdown(&rows, columns, has_header);
        let header_table = table_markdown(&rows, columns, true);
        let mut marker = format!("LunarcaTable{}Token", tables.len());
        while html.contains(&marker) {
            marker.push('X');
        }
        table.replace_with_html(format!("<p>{marker}</p>"));
        tables.push((marker, default_table, header_table, rows));
    }
    let mut markdown = doc.md(None).trim().to_string();
    let table_cells = if tables.len() == 1 && markdown == tables[0].0 {
        Some(tables[0].3.clone())
    } else {
        None
    };
    let mut header_markdown = warnings
        .contains(&PasteWarning::MissingHeader)
        .then(|| markdown.clone());
    for (marker, table, header_table, _) in tables {
        if let Some(alternative) = &mut header_markdown {
            *alternative = alternative.replace(&marker, &header_table);
        }
        markdown = markdown.replace(&marker, &table);
    }
    let block = block || markdown.contains('\n');
    HtmlPaste {
        markdown,
        warnings,
        block,
        header_markdown,
        table_cells,
    }
}

/// 选中的多行文字 → GFM 表（「转为表格」，PRD-06-011）：首行作表头，按**制表符**、**逗号**
/// （RFC 4180 引号）或**两个以上空格**（含全角空格）分列，依次尝试。每行列数一致、至少两列
/// 两行才算一张表，否则 `None`——看不出列就不猜，原文不动。单元格里的 `|` 转义为 `\|`。
#[must_use]
pub fn text_to_table(text: &str) -> Option<String> {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .collect();
    if lines.len() < 2 {
        return None;
    }
    let consistent = |rows: Option<Vec<Vec<String>>>| {
        let rows = rows?;
        let columns = rows.first()?.len();
        (columns >= 2 && rows.iter().all(|row| row.len() == columns)).then_some(rows)
    };
    let by = |split: fn(&str) -> Option<Vec<String>>| {
        consistent(lines.iter().map(|line| split(line)).collect())
    };
    let rows = by(split_tabs)
        .or_else(|| by(split_csv))
        .or_else(|| by(split_spaces))?;
    let columns = rows[0].len();
    let cells: Vec<Vec<String>> = rows
        .into_iter()
        .map(|row| {
            row.into_iter()
                .map(|cell| cell.trim().replace('|', "\\|"))
                .collect()
        })
        .collect();
    Some(table_markdown(&cells, columns, true))
}

fn split_tabs(line: &str) -> Option<Vec<String>> {
    Some(line.split('\t').map(str::to_owned).collect())
}

/// 一行 CSV：双引号包裹的字段可含逗号，`""` 是一个引号；引号不成对返回 `None`。
fn split_csv(line: &str) -> Option<Vec<String>> {
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' if !quoted && field.trim().is_empty() => {
                field.clear();
                quoted = true;
            }
            '"' if quoted => {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    quoted = false;
                }
            }
            ',' if !quoted => fields.push(std::mem::take(&mut field)),
            _ => field.push(ch),
        }
    }
    if quoted {
        return None;
    }
    fields.push(field);
    Some(fields)
}

/// 两个以上连续空格（或一个全角空格）为列界：对齐排版的纯文本表格。
fn split_spaces(line: &str) -> Option<Vec<String>> {
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut spaces = 0;
    for ch in line.trim().chars() {
        if ch == ' ' {
            spaces += 1;
            continue;
        }
        if spaces >= 2 || ch == '\u{3000}' {
            if !field.is_empty() {
                fields.push(std::mem::take(&mut field));
            }
        } else if spaces == 1 {
            field.push(' ');
        }
        spaces = 0;
        if ch != '\u{3000}' {
            field.push(ch);
        }
    }
    if !field.is_empty() {
        fields.push(field);
    }
    Some(fields)
}

/// TSV 支持带引号的多行单元格与双引号转义；歧义/不完整文本保持纯文本。
pub fn convert_tsv(text: &str) -> Option<HtmlPaste> {
    if !text.contains('\t') {
        return None;
    }
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' if cell.is_empty() && !quoted => quoted = true,
            '"' if quoted => {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    cell.push('"');
                } else {
                    quoted = false;
                }
            }
            '\t' if !quoted => row.push(std::mem::take(&mut cell)),
            '\n' | '\r' if !quoted => {
                if ch == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                row.push(std::mem::take(&mut cell));
                rows.push(std::mem::take(&mut row));
            }
            _ => cell.push(ch),
        }
    }
    if quoted {
        return None;
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        rows.push(row);
    }
    if rows.is_empty() || rows.iter().all(|row| row.len() < 2) {
        return None;
    }
    let escape = |cell: &str| {
        cell.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('\n', "<br>")
    };
    let html = format!(
        "<table>{}</table>",
        rows.iter()
            .map(|row| format!(
                "<tr>{}</tr>",
                row.iter()
                    .map(|cell| format!("<td>{}</td>", escape(cell)))
                    .collect::<String>()
            ))
            .collect::<String>()
    );
    Some(convert_html(&html))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_to_table_splits_tabs_csv_and_aligned_spaces() {
        assert_eq!(
            text_to_table("名称\t数量\n苹果\t3\n").as_deref(),
            Some("| 名称 | 数量 |\n| --- | --- |\n| 苹果 | 3 |")
        );
        assert_eq!(
            text_to_table("a,\"b, c\"\n1,\"说\"\"好\"\"\"").as_deref(),
            Some("| a | b, c |\n| --- | --- |\n| 1 | 说\"好\" |")
        );
        assert_eq!(
            text_to_table("Name    Score\nAda     99\nLin Lee  7").as_deref(),
            Some("| Name | Score |\n| --- | --- |\n| Ada | 99 |\n| Lin Lee | 7 |")
        );
        assert_eq!(
            text_to_table("a|b\tc\nd\te").as_deref(),
            Some("| a\\|b | c |\n| --- | --- |\n| d | e |")
        );
    }

    #[test]
    fn text_to_table_refuses_text_without_columns() {
        assert_eq!(text_to_table("一行\n又一行"), None);
        assert_eq!(text_to_table("a\tb"), None);
        assert_eq!(text_to_table("a\tb\nc\td\te"), None);
    }
    #[test]
    fn retains_structure_and_never_executes_or_embeds_active_html() {
        let paste = convert_html("<h2>标题</h2><p><strong>重点</strong> <a href='https://example.com'>来源</a></p><ul><li>一</li><li>二</li></ul><pre><code>let x = 1;</code></pre><script>secret()</script>");
        assert!(paste.markdown.contains("## 标题"));
        assert!(paste.markdown.contains("**重点**"));
        assert!(paste.markdown.contains("[来源](https://example.com)"));
        assert!(paste.markdown.contains("- 一"));
        assert!(paste.markdown.contains("```"));
        assert!(!paste.markdown.contains("secret"));
    }
    #[test]
    fn rectangular_table_preserves_empty_cells_pipes_and_inline_format() {
        let paste = convert_html("<table><tr><th>名称</th><th>数量</th></tr><tr><td><b>A|B</b></td><td></td></tr></table>");
        assert!(paste.warnings.is_empty());
        assert_eq!(
            paste.markdown,
            "| 名称 | 数量 |\n| --- | --- |\n| **A\\|B** |  |"
        );
    }
    #[test]
    fn no_header_inserts_blank_header_not_promoting_first_data_row() {
        let paste = convert_html("<table><tr><td>数据</td><td>1</td></tr></table>");
        assert_eq!(paste.markdown, "|  |  |\n| --- | --- |\n| 数据 | 1 |");
        assert_eq!(paste.warnings, vec![PasteWarning::MissingHeader]);
        assert_eq!(
            paste.header_markdown.as_deref(),
            Some("| 数据 | 1 |\n| --- | --- |")
        );
        assert_eq!(
            paste.table_cells,
            Some(vec![vec!["数据".to_owned(), "1".to_owned()]])
        );
    }
    #[test]
    fn merged_cells_images_and_unsafe_links_need_review() {
        let paste = convert_html("<table><tr><td colspan='2'>合并</td></tr><tr><td>甲</td><td>乙</td></tr></table><img src='https://example.com/x.png' alt='插图'><a href='javascript:run()'>文本</a>");
        assert!(paste.warnings.contains(&PasteWarning::ComplexTable));
        assert!(paste.warnings.contains(&PasteWarning::ImagesOmitted));
        assert!(paste.warnings.contains(&PasteWarning::UnsafeLinks));
        assert!(paste.markdown.contains("合并"));
        assert!(!paste.markdown.contains("javascript"));
        assert!(!paste.markdown.contains("https://example.com/x.png"));
    }
    #[test]
    fn tsv_preserves_quoted_multiline_empty_cells_and_literal_markup() {
        let paste = convert_tsv("\"甲\n乙\"\t\"<b>x</b>\"\t\r\n1\t\"a\"\"b\"\t3").unwrap();
        assert!(paste.markdown.contains("甲<br>乙"), "{:?}", paste.markdown);
        assert!(paste.markdown.contains("\\<b\\>x\\</b\\>"));
        assert!(paste.markdown.contains("a\\\"b"));
        assert_eq!(paste.warnings, vec![PasteWarning::MissingHeader]);
        assert!(paste.block);
        assert!(convert_tsv("\"未闭合\t1").is_none());
        assert!(convert_tsv("普通文字").is_none());
    }
    #[test]
    fn single_heading_is_a_block_and_word_style_loss_is_explicit() {
        assert!(convert_html("<h2>单行标题</h2>").block);
        let paste = convert_html("<span style='color:red'>正文</span>");
        assert_eq!(paste.markdown, "正文");
        assert_eq!(paste.warnings, vec![PasteWarning::FormattingSimplified]);
    }
    #[test]
    fn nested_table_data_is_not_duplicated_as_outer_rows() {
        let paste = convert_html("<table><tr><th>表头</th></tr><tr><td>外层<table><tr><td>内层唯一文本</td></tr></table></td></tr></table>");
        assert_eq!(paste.markdown.matches("内层唯一文本").count(), 1);
        assert!(paste.warnings.contains(&PasteWarning::ComplexTable));
        let nested_header = convert_html(
            "<table><tr><td>正文<table><tr><th>内层表头</th></tr></table></td></tr></table>",
        );
        assert!(nested_header
            .warnings
            .contains(&PasteWarning::MissingHeader));
    }

    #[test]
    fn single_table_projection_excludes_surrounding_content_and_keeps_literal_tsv() {
        let table =
            "<table><tr><th>名称</th><th>数量</th></tr><tr><td>A|B</td><td></td></tr></table>";
        let paste = convert_html(table);
        assert!(paste.header_markdown.is_none());
        assert_eq!(paste.table_cells.unwrap()[1], vec!["A\\|B", ""]);
        assert!(convert_html(&format!("<p>必须保留的正文</p>{table}"))
            .table_cells
            .is_none());
        assert!(convert_html(&format!("{table}{table}"))
            .table_cells
            .is_none());
        let tsv = convert_tsv("\"甲\n乙\"\t\"<b>x</b>\"\r\n\t2").unwrap();
        assert_eq!(tsv.table_cells.as_ref().unwrap().len(), 2);
        assert_eq!(tsv.table_cells.as_ref().unwrap()[0][0], "甲<br>乙");
        assert!(!tsv.header_markdown.unwrap().starts_with("|  |"));
    }

    #[test]
    fn header_choice_preserves_all_tables_and_surrounding_text() {
        let paste = convert_html("<p>前文</p><table><tr><td>第一行</td><td>1</td></tr><tr><td>第二行</td><td>2</td></tr></table><p>后文</p>");
        let alternate = paste.header_markdown.unwrap();
        assert!(alternate.contains("前文") && alternate.contains("后文"));
        assert!(alternate.contains("| 第一行 | 1 |\n| --- | --- |\n| 第二行 | 2 |"));
        assert!(!alternate.contains("|  |  |"));
    }

    /// 没有替代文字的远程图片同样不能漏进 Markdown（粘贴与剪藏都不许悄悄触发远程下载）。
    #[test]
    fn remote_images_without_alt_are_dropped_too() {
        let paste = convert_html(r#"<p>前<img src="https://e.com/a.jpg" alt="" title="">后</p>"#);
        assert!(!paste.markdown.contains("e.com"), "{}", paste.markdown);
        assert!(paste.markdown.contains("前") && paste.markdown.contains("后"));
        assert_eq!(paste.warnings, vec![PasteWarning::ImagesOmitted]);
    }

    /// 包在链接里的图片不能被吞掉（「点图看大图」很常见），多行 title 不进图片语法。
    #[test]
    fn images_wrapped_in_links_survive() {
        let paste = convert_html_with(
            "<p><a href=\"http://album.example/x\"><img src=\"assets/t-1.jpg\" alt=\"甲\" title=\"多\n行\"></a></p>",
            true,
        );
        assert_eq!(paste.markdown, "![甲](assets/t-1.jpg)");
    }
}
