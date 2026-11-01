//! 导出文件命名模板（PRD-11-007）：`{name}`、`{date}`、`{time}`、`{format}`、`{content}` 五个占位，
//! 其余字面照抄。校验（设置写入时）与渲染（导出时）住在同一处，两边永远认同一份占位表。
//!
//! 渲染结果是**一个文件名的 stem**（不含扩展名、不含路径）：路径分隔符与各平台非法字符一律
//! 换成 `_`，首尾的空格与点去掉，空了退回笔记名——模板再怎么写也造不出一条越界的路径。

use std::fmt;

/// 合法占位符（不含花括号）。
pub const PLACEHOLDERS: &[&str] = &["name", "date", "time", "format", "content"];

/// 默认模板：只有笔记名（= 既有行为）。
pub const DEFAULT_TEMPLATE: &str = "{name}";

/// 渲染上下文：全部由调用方给已算好的字符串，本模块不读时钟、不猜内容。
#[derive(Debug, Clone, Copy)]
pub struct FilenameContext<'a> {
    /// 源 / 笔记名（文件 stem）。
    pub name: &'a str,
    /// 导出日期 `YYYY-MM-DD`。
    pub date: &'a str,
    /// 导出时刻 `HHMM`。
    pub time: &'a str,
    /// 格式扩展名（srt / vtt / txt / md / html…）。
    pub format: &'a str,
    /// 内容口径（source / translation / bilingual / text / html / bundle）。
    pub content: &'a str,
}

/// 模板不合法的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateError {
    /// 空模板。
    Empty,
    /// 花括号不成对。
    Unbalanced,
    /// 认不出的占位符。
    UnknownPlaceholder(String),
    /// 字面里含路径分隔符或平台非法字符。
    IllegalChar(char),
}

impl fmt::Display for TemplateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("template is empty"),
            Self::Unbalanced => f.write_str("unbalanced braces"),
            Self::UnknownPlaceholder(name) => write!(f, "unknown placeholder {{{name}}}"),
            Self::IllegalChar(c) => write!(f, "illegal character {c:?}"),
        }
    }
}

const ILLEGAL: &[char] = &['/', '\\', ':', '*', '?', '"', '<', '>', '|'];

/// 校验模板：非空、花括号成对、占位符全在表内、字面无非法字符。
///
/// # Errors
/// 见 [`TemplateError`]。
pub fn validate(template: &str) -> Result<(), TemplateError> {
    if template.trim().is_empty() {
        return Err(TemplateError::Empty);
    }
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let literal = &rest[..open];
        if let Some(c) = literal
            .chars()
            .find(|c| ILLEGAL.contains(c) || c.is_control())
        {
            return Err(TemplateError::IllegalChar(c));
        }
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            return Err(TemplateError::Unbalanced);
        };
        let name = &after[..close];
        if name.contains('{') {
            return Err(TemplateError::Unbalanced);
        }
        if !PLACEHOLDERS.contains(&name) {
            return Err(TemplateError::UnknownPlaceholder(name.to_owned()));
        }
        rest = &after[close + 1..];
    }
    if rest.contains('}') {
        return Err(TemplateError::Unbalanced);
    }
    if let Some(c) = rest.chars().find(|c| ILLEGAL.contains(c) || c.is_control()) {
        return Err(TemplateError::IllegalChar(c));
    }
    Ok(())
}

/// 单个片段做成安全的文件名成分：非法字符换 `_`，连续空白折成一个空格（首尾空格保留，
/// 那是模板里占位符之间的分隔，整串的首尾另在 `render` 收口）。
fn sanitize_component(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            pending_space = true;
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        out.push(if ILLEGAL.contains(&c) || c.is_control() {
            '_'
        } else {
            c
        });
    }
    if pending_space {
        out.push(' ');
    }
    out
}

/// 渲染模板为文件名 stem。模板不合法（不该发生：写入时已校验）时按 [`DEFAULT_TEMPLATE`]。
#[must_use]
pub fn render(template: &str, ctx: FilenameContext<'_>) -> String {
    let template = if validate(template).is_ok() {
        template
    } else {
        DEFAULT_TEMPLATE
    };
    let mut out = String::with_capacity(template.len() + ctx.name.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&sanitize_component(&rest[..open]));
        let after = &rest[open + 1..];
        let close = after.find('}').unwrap_or(after.len());
        let value = match &after[..close] {
            "name" => ctx.name,
            "date" => ctx.date,
            "time" => ctx.time,
            "format" => ctx.format,
            "content" => ctx.content,
            _ => "",
        };
        out.push_str(&sanitize_component(value));
        rest = after.get(close + 1..).unwrap_or("");
    }
    out.push_str(&sanitize_component(rest));
    let trimmed = out.trim_matches(|c: char| c == ' ' || c == '.').to_owned();
    if trimmed.is_empty() {
        sanitize_component(ctx.name)
            .trim_matches(|c: char| c == ' ' || c == '.')
            .to_owned()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> FilenameContext<'static> {
        FilenameContext {
            name: "周会: 复盘/总结",
            date: "2026-09-06",
            time: "1430",
            format: "srt",
            content: "bilingual",
        }
    }

    #[test]
    fn validates_placeholders_braces_and_illegal_literals() {
        assert_eq!(validate("{name}"), Ok(()));
        assert_eq!(validate("{date} {name} [{content}]"), Ok(()));
        assert_eq!(validate("  "), Err(TemplateError::Empty));
        assert_eq!(validate("{name"), Err(TemplateError::Unbalanced));
        assert_eq!(validate("name}"), Err(TemplateError::Unbalanced));
        assert_eq!(validate("{{name}}"), Err(TemplateError::Unbalanced));
        assert_eq!(
            validate("{title}"),
            Err(TemplateError::UnknownPlaceholder("title".into()))
        );
        assert_eq!(validate("a/{name}"), Err(TemplateError::IllegalChar('/')));
        assert_eq!(validate("{name}?"), Err(TemplateError::IllegalChar('?')));
    }

    #[test]
    fn renders_and_sanitizes_values_not_the_template_grammar() {
        // 值里的冒号与斜杠换成下划线，字面照抄，日期时间原样。
        assert_eq!(
            render("{date} {name} ({content}).{format}", ctx()),
            "2026-09-06 周会_ 复盘_总结 (bilingual).srt"
        );
        // 默认模板 = 笔记名。
        assert_eq!(render(DEFAULT_TEMPLATE, ctx()), "周会_ 复盘_总结");
        // 非法模板回默认；全空退回名字；首尾点与空格去掉。
        assert_eq!(render("{title}", ctx()), "周会_ 复盘_总结");
        assert_eq!(render(" . ", ctx()), "周会_ 复盘_总结");
        assert_eq!(render("{date}.", ctx()), "2026-09-06");
    }
}
