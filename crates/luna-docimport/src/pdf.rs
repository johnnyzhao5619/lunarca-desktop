//! PDF（有文字层）→ 逐页文字，每页前一行页标（ADR-164 ⑧）。
//!
//! 取字用 `pdf-extract`；文字按 PDF 里的换行原样保留，不猜段落。页标的文字由调用方按界面语言
//! 给出（[`crate::ConvertHost::page_marker`]）。

use crate::{ConvertHost, Converted, DocImportError, DocWarning};

/// 全篇平均每页可见字符少于这么多 → 判为扫描件（没有文字层）。
const SCANNED_CHARS_PER_PAGE: usize = 20;
/// 少于这么多 → 照常导入，但提示内容多半是图片（真机 2026-10-19：浏览器打印的网页每页只有
/// 页眉页脚那 40 个字，正文是一整张图）。
const SPARSE_CHARS_PER_PAGE: usize = 100;

pub(crate) fn convert(
    bytes: &[u8],
    host: &mut dyn ConvertHost,
) -> Result<Converted, DocImportError> {
    let mut doc = pdf_extract::Document::load_mem(bytes)
        .map_err(|e| DocImportError::Corrupt(e.to_string()))?;
    if doc.is_encrypted() && doc.decrypt("").is_err() {
        // 只有「打开密码」解不开；只设了权限密码的 PDF 用空密码即可读。
        return Err(DocImportError::Encrypted);
    }
    let page_count = doc.get_pages().len();
    // pdf-extract 碰到读不了的页就停，后面的页静默丢掉——数一遍页数才知道少没少。
    let pages = std::panic::catch_unwind(|| pdf_extract::extract_text_from_mem_by_pages(bytes))
        .map_err(|_| DocImportError::Corrupt("pdf-extract panicked".into()))?
        .map_err(|e| DocImportError::Corrupt(e.to_string()))?;
    let pages: Vec<String> = pages.iter().map(|page| normalize(page)).collect();
    let visible: usize = pages
        .iter()
        .map(|page| page.chars().filter(|c| !c.is_whitespace()).count())
        .sum();
    if page_count == 0 || visible < SCANNED_CHARS_PER_PAGE * page_count.max(1) {
        return Err(DocImportError::NoTextLayer);
    }

    let mut warnings = Vec::new();
    if visible < SPARSE_CHARS_PER_PAGE * page_count {
        warnings.push(DocWarning::MostlyImages);
    }
    let empty: Vec<usize> = pages
        .iter()
        .enumerate()
        .filter(|(_, page)| page.trim().is_empty())
        .map(|(index, _)| index + 1)
        .collect();
    if !empty.is_empty() {
        warnings.push(DocWarning::PagesWithoutText { pages: empty });
    }
    if pages.len() < page_count {
        warnings.push(DocWarning::PagesUnreadable {
            from: pages.len() + 1,
            total: page_count,
        });
    }
    let mut sections = Vec::with_capacity(pages.len());
    for (index, page) in pages.iter().enumerate() {
        let marker = host.page_marker(index + 1);
        let text = guard_line_starts(page.trim());
        sections.push(if text.is_empty() {
            marker
        } else {
            format!("{marker}\n\n{text}")
        });
    }
    Ok(Converted {
        markdown: sections.join("\n\n"),
        warnings,
        ..Converted::default()
    })
}

/// 部分字体把汉字映射成康熙部首（「一」→「⼀」）、把 fi 写成连字：只把这几段逐字做 NFKC。
/// 整篇 NFKC 会把全角标点（「，」「：」）改成半角，不能用。
fn normalize(text: &str) -> String {
    use unicode_normalization::UnicodeNormalization as _;
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if matches!(ch, '\u{2E80}'..='\u{2EFF}' | '\u{2F00}'..='\u{2FDF}' | '\u{FB00}'..='\u{FB06}')
        {
            out.extend(std::iter::once(ch).nfkc());
        } else {
            out.push(ch);
        }
    }
    out
}

/// 行首的 `#` `>` `|` 在 Markdown 里会变成标题、引用、表格：PDF 里的文字原样，转义它们。
fn guard_line_starts(text: &str) -> String {
    text.lines()
        .map(|line| {
            let indent = line.len() - line.trim_start().len();
            let (lead, rest) = line.split_at(indent);
            match rest.chars().next() {
                Some('#' | '>' | '|') => format!("{lead}\\{rest}"),
                _ => line.to_owned(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kangxi_radicals_and_ligatures_are_normalized_but_fullwidth_punctuation_stays() {
        assert_eq!(normalize("这是⼀段，说明：ﬁrst"), "这是一段，说明：first");
    }

    #[test]
    fn markdown_line_starts_are_guarded() {
        assert_eq!(
            guard_line_starts("# 不是标题\n  > 不是引用\n| 不是表格\n正文 # 不动"),
            "\\# 不是标题\n  \\> 不是引用\n\\| 不是表格\n正文 # 不动"
        );
    }

    #[test]
    fn garbage_is_corrupt() {
        let mut host = |_: usize, _: &str| String::new();
        assert!(matches!(
            convert(b"%PDF-1.4 nope", &mut host),
            Err(DocImportError::Corrupt(_))
        ));
    }
}
