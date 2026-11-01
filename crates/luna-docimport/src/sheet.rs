//! Excel（.xlsx / .xlsm / .xlsb / .xls）与 OpenDocument 表格（.ods）→ 每个工作表一张 GFM 表
//! （ADR-164 ⑨）。只读保存时的值，不重算公式；图表、图片、批注、格式不带入。

use std::io::Cursor;

use calamine::{Data, Reader as _};

use crate::table::{cell, rows_to_markdown};
use crate::{Converted, DocImportError};

pub(crate) fn convert(bytes: &[u8]) -> Result<Converted, DocImportError> {
    let mut workbook = calamine::open_workbook_auto_from_rs(Cursor::new(bytes))
        .map_err(|e| DocImportError::Corrupt(e.to_string()))?;
    let names = workbook.sheet_names();
    let mut sections = Vec::new();
    let mut warnings = Vec::new();
    for name in &names {
        // 图表页之类读不出单元格的工作表跳过，不让整本失败。
        let Ok(range) = workbook.worksheet_range(name) else {
            continue;
        };
        let rows: Vec<Vec<String>> = range
            .rows()
            .map(|row| row.iter().map(|c| cell(&display(c))).collect())
            .collect();
        let label = (names.len() > 1).then_some(name.as_str());
        if let Some((table, warning)) = rows_to_markdown(rows, label) {
            sections.push(match label {
                Some(name) => format!("## {}\n\n{table}", name.trim()),
                None => table,
            });
            warnings.extend(warning);
        }
    }
    if sections.is_empty() {
        return Err(DocImportError::Empty);
    }
    Ok(Converted {
        markdown: sections.join("\n\n"),
        warnings,
        ..Converted::default()
    })
}

/// 单元格的显示文字：整数不带 `.0`，布尔写成 Excel 的 TRUE / FALSE，日期按序列号还原。
fn display(data: &Data) -> String {
    match data {
        Data::Empty => String::new(),
        Data::Bool(true) => "TRUE".to_owned(),
        Data::Bool(false) => "FALSE".to_owned(),
        Data::DateTime(dt) if dt.is_datetime() => {
            let (y, m, d, hh, mm, ss, _) = dt.to_ymd_hms_milli();
            if dt.as_f64() < 1.0 {
                // 序列号小于 1：只有时间、没有日期。
                format!("{hh:02}:{mm:02}:{ss:02}")
            } else if (hh, mm, ss) == (0, 0, 0) {
                format!("{y:04}-{m:02}-{d:02}")
            } else {
                format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}")
            }
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use calamine::{ExcelDateTime, ExcelDateTimeType};

    #[test]
    fn cells_display_like_the_spreadsheet() {
        assert_eq!(display(&Data::Float(3.0)), "3");
        assert_eq!(display(&Data::Float(1234.5)), "1234.5");
        assert_eq!(display(&Data::Int(-7)), "-7");
        assert_eq!(display(&Data::Bool(true)), "TRUE");
        assert_eq!(display(&Data::String("名称".into())), "名称");
        // 2026-10-18 的 Excel 序列号（1900 纪元）。
        let date = ExcelDateTime::new(46313.0, ExcelDateTimeType::DateTime, false);
        assert_eq!(display(&Data::DateTime(date)), "2026-10-18");
        let stamp = ExcelDateTime::new(46313.75, ExcelDateTimeType::DateTime, false);
        assert_eq!(display(&Data::DateTime(stamp)), "2026-10-18 18:00");
    }

    /// 手工拼一个最小的 xlsx：两张表（共享字符串、数字、布尔、日期样式），第二张全空。
    fn workbook() -> Vec<u8> {
        use std::io::Write as _;
        let parts: [(&str, &str); 7] = [
            (
                "[Content_Types].xml",
                r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/worksheets/sheet2.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#,
            ),
            (
                "_rels/.rels",
                r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#,
            ),
            (
                "xl/workbook.xml",
                r#"<?xml version="1.0"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="选题" sheetId="1" r:id="rId1"/><sheet name="空表" sheetId="2" r:id="rId2"/></sheets></workbook>"#,
            ),
            (
                "xl/_rels/workbook.xml.rels",
                r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/><Relationship Id="rId4" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#,
            ),
            (
                "xl/sharedStrings.xml",
                r#"<?xml version="1.0"?><sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><si><t>标题</t></si><si><t>播放量</t></si><si><t>发布</t></si><si><t>开头 | 悬念</t></si></sst>"#,
            ),
            (
                "xl/styles.xml",
                r#"<?xml version="1.0"?><styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><cellXfs count="2"><xf numFmtId="0"/><xf numFmtId="14" applyNumberFormat="1"/></cellXfs></styleSheet>"#,
            ),
            (
                "xl/worksheets/sheet1.xml",
                r#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c><c r="C1" t="s"><v>2</v></c><c r="D1" t="s"><v>2</v></c></row><row r="2"><c r="A2" t="s"><v>3</v></c><c r="B2"><v>12000</v></c><c r="C2" s="1"><v>46313</v></c><c r="D2" t="b"><v>1</v></c></row></sheetData></worksheet>"#,
            ),
        ];
        let mut buf = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default();
            for (name, body) in parts {
                zip.start_file(name, opts).unwrap();
                zip.write_all(body.as_bytes()).unwrap();
            }
            zip.start_file("xl/worksheets/sheet2.xml", opts).unwrap();
            zip.write_all(br#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData/></worksheet>"#).unwrap();
            zip.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn each_non_empty_sheet_becomes_a_section() {
        let converted = convert(&workbook()).unwrap();
        assert_eq!(
            converted.markdown,
            "## 选题\n\n| 标题 | 播放量 | 发布 | 发布 |\n| --- | --- | --- | --- |\n| 开头 \\| 悬念 | 12000 | 2026-10-18 | TRUE |"
        );
        assert!(converted.warnings.is_empty());
    }

    #[test]
    fn garbage_is_corrupt() {
        assert!(matches!(
            convert(b"not a workbook"),
            Err(DocImportError::Corrupt(_))
        ));
    }
}
