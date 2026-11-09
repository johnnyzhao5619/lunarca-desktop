//! frontmatter 区段的定位口径（纯逻辑）：本机库的读写器、三路合并与网页版共用这一份
//! （ADR-139 ④ 的字节级写出规矩、ADR-148 ② 的按顶层键合并）。

/// frontmatter 区段的字节定位（内部）。
pub struct FmSpan {
    /// 内部内容起点（开头 `---\n` 之后）。
    pub inner_start: usize,
    /// 内部内容止点（闭合 `---` 行起点）。
    pub inner_end: usize,
    /// 正文起点（闭合 `---\n` 之后）。
    pub body_start: usize,
}

/// 取一行（不含换行符）及其结束偏移（含换行符的下一字节，或 EOF）。
pub fn line_at(s: &str, start: usize) -> (&str, usize) {
    match s[start..].find('\n') {
        Some(rel) => {
            let nl = start + rel;
            (&s[start..nl], nl + 1)
        }
        None => (&s[start..], s.len()),
    }
}

/// 去掉一行末尾可能的 `\r`（CRLF 兼容），用于内容比较——不改写原字节。
pub fn trim_cr(line: &str) -> &str {
    line.strip_suffix('\r').unwrap_or(line)
}

/// frontmatter 内部（两条 `---` 之间）的字节区间；无 frontmatter 为 `None`。标签写入器
/// （ADR-135）与 `luna:` 块写入共用同一个定位口径。
pub fn frontmatter_inner_range(content: &str) -> Option<std::ops::Range<usize>> {
    find_frontmatter(content).map(|span| span.inner_start..span.inner_end)
}

/// frontmatter 的四段原文：开头分隔行（含换行）、内部、闭合分隔行（含换行）、正文。三路合并
/// 按这四段拆开再拼回，每一段都是原字节。无 frontmatter（含未闭合）为 `None`。
pub fn frontmatter_parts(content: &str) -> Option<[&str; 4]> {
    find_frontmatter(content).map(|span| {
        [
            &content[..span.inner_start],
            &content[span.inner_start..span.inner_end],
            &content[span.inner_end..span.body_start],
            &content[span.body_start..],
        ]
    })
}

/// 定位 frontmatter 区段。必须在文件最开头：第一行恰为 `---`，其后有闭合 `---` 行。
pub fn find_frontmatter(content: &str) -> Option<FmSpan> {
    let (first, after_first) = line_at(content, 0);
    if trim_cr(first) != "---" {
        return None;
    }
    let mut cursor = after_first;
    while cursor < content.len() {
        let (line, after) = line_at(content, cursor);
        if trim_cr(line) == "---" {
            return Some(FmSpan {
                inner_start: after_first,
                inner_end: cursor,
                body_start: after,
            });
        }
        cursor = after;
    }
    None // 无闭合分隔符：视为无 frontmatter（保守，整体当正文）。
}

/// 第一行是 `---` 却找不到闭合行：frontmatter 写了一半或被改坏（真机 2026-09-27：用户在
/// 编辑器里删掉了闭合 `---` 后的换行，成了 `---# 标题`）。解析上它「没有 frontmatter」，
/// 但写入器若照「无 frontmatter」在前面再新建一份，就会叠出两段、把用户那半段挤进正文。
/// 所有写入器遇到它都拒绝改写，等用户把它修好（同 Obsidian `processFrontMatter` 的做法）。
#[must_use]
pub fn has_unclosed_frontmatter(content: &str) -> bool {
    trim_cr(line_at(content, 0).0) == "---" && find_frontmatter(content).is_none()
}

/// 顶层键行：行首不缩进、不是注释或列表项，第一个「后面跟空白或行尾的冒号」之前是键。
pub fn key_of(text: &str) -> Option<(&str, usize)> {
    if text.starts_with([' ', '\t', '#', '-']) || text.trim().is_empty() {
        return None;
    }
    let bytes = text.as_bytes();
    let colon = (0..bytes.len()).find(|&i| {
        bytes[i] == b':' && bytes.get(i + 1).is_none_or(|b| *b == b' ' || *b == b'\t')
    })?;
    Some((text[..colon].trim_end(), colon + 1))
}
