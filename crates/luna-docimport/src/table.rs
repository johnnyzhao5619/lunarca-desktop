//! CSV / TSV / 工作表 → GFM 表格（ADR-164 ⑨）。

use crate::{DocImportError, DocWarning};

/// 超过这么多行（不含表头）或列就只放预览（PRD-06-065）。
pub const TABLE_MAX_ROWS: usize = 500;
pub const TABLE_MAX_COLS: usize = 30;
/// 预览放多少行数据（不含表头）。
pub const TABLE_PREVIEW_ROWS: usize = 100;

/// 分隔符文本 → 一张 GFM 表；首行作表头。
pub(crate) fn delimited_to_markdown(
    text: &str,
    delimiter: u8,
) -> Result<(String, Vec<DocWarning>), DocImportError> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(delimiter)
        .from_reader(text.as_bytes());
    let mut rows: Vec<Vec<String>> = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|e| DocImportError::Corrupt(e.to_string()))?;
        rows.push(record.iter().map(cell).collect());
    }
    let (markdown, warning) = rows_to_markdown(rows, None).ok_or(DocImportError::Empty)?;
    Ok((markdown, warning.into_iter().collect()))
}

/// 已经过 [`cell`] 的行 → 一张 GFM 表；首行作表头，全空的行跳过，超限只放预览。
/// 没有任何非空行 → `None`。CSV 与工作表共用（`sheet` 只用来标明是哪张表被截断）。
pub(crate) fn rows_to_markdown(
    rows: Vec<Vec<String>>,
    sheet: Option<&str>,
) -> Option<(String, Option<DocWarning>)> {
    let rows: Vec<Vec<String>> = rows
        .into_iter()
        .filter(|row| row.iter().any(|c| !c.is_empty()))
        .collect();
    let (header, data) = rows.split_first()?;
    // 末尾整列为空的列不算（工作表的已用区域常常带着空列）。
    let cols = rows
        .iter()
        .map(|row| row.iter().rposition(|c| !c.is_empty()).map_or(0, |i| i + 1))
        .max()
        .unwrap_or(0);
    let mut warning = None;
    let (shown_rows, shown_cols) = if data.len() > TABLE_MAX_ROWS || cols > TABLE_MAX_COLS {
        let shown = (data.len().min(TABLE_PREVIEW_ROWS), cols.min(TABLE_MAX_COLS));
        warning = Some(DocWarning::TableTruncated {
            sheet: sheet.map(str::to_owned),
            rows: data.len(),
            cols,
            shown_rows: shown.0,
            shown_cols: shown.1,
        });
        shown
    } else {
        (data.len(), cols)
    };
    let line = |row: &[String]| {
        let cells: Vec<&str> = (0..shown_cols)
            .map(|c| row.get(c).map_or("", String::as_str))
            .collect();
        format!("| {} |", cells.join(" | "))
    };
    let mut out = vec![
        line(header),
        format!("| {} |", vec!["---"; shown_cols].join(" | ")),
    ];
    out.extend(data.iter().take(shown_rows).map(|row| line(row)));
    Some((out.join("\n"), warning))
}

/// 单元格：去首尾空白，`|` 转义，换行写成 `<br>`（GFM 表格一格只能一行）。
pub(crate) fn cell(raw: &str) -> String {
    raw.trim()
        .replace('|', "\\|")
        .replace("\r\n", "<br>")
        .replace(['\n', '\r'], "<br>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoted_newlines_pipes_and_ragged_rows() {
        let csv = "名称,说明,数量\n\"A, B\",\"第一行\n第二行\",3\nC|D,,\n\n短行\n";
        let (md, warnings) = delimited_to_markdown(csv, b',').unwrap();
        assert_eq!(
            md,
            "| 名称 | 说明 | 数量 |\n| --- | --- | --- |\n| A, B | 第一行<br>第二行 | 3 |\n\
             | C\\|D |  |  |\n| 短行 |  |  |"
        );
        assert!(warnings.is_empty());
    }

    #[test]
    fn tsv_uses_tabs() {
        let (md, _) = delimited_to_markdown("a\tb\n1\t2\n", b'\t').unwrap();
        assert_eq!(md, "| a | b |\n| --- | --- |\n| 1 | 2 |");
    }

    #[test]
    fn big_tables_only_show_a_preview() {
        let mut csv = String::from("h1,h2\n");
        for i in 0..=TABLE_MAX_ROWS {
            csv.push_str(&format!("{i},x\n"));
        }
        let (md, warnings) = delimited_to_markdown(&csv, b',').unwrap();
        assert_eq!(md.lines().count(), 2 + TABLE_PREVIEW_ROWS);
        assert_eq!(
            warnings,
            vec![DocWarning::TableTruncated {
                sheet: None,
                rows: TABLE_MAX_ROWS + 1,
                cols: 2,
                shown_rows: TABLE_PREVIEW_ROWS,
                shown_cols: 2,
            }]
        );
        let wide = (0..=TABLE_MAX_COLS)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let (md, warnings) = delimited_to_markdown(&format!("{wide}\n{wide}\n"), b',').unwrap();
        assert_eq!(
            md.lines().next().unwrap().matches(" | ").count(),
            TABLE_MAX_COLS - 1
        );
        assert!(matches!(
            warnings[0],
            DocWarning::TableTruncated {
                shown_cols: TABLE_MAX_COLS,
                ..
            }
        ));
    }

    #[test]
    fn blank_input_is_empty() {
        assert_eq!(
            delimited_to_markdown("\n,,\n", b','),
            Err(DocImportError::Empty)
        );
    }
}
