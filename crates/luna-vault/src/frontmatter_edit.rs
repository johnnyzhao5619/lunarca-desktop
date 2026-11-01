//! frontmatter 顶层用户键的就地改写（ADR-139；ADR-135 的标签写入建立在它之上；TDD-06 §4.4）。
//!
//! 纪律：一次编辑只改被编辑的那一个顶层键所占的字节，其余字节（注释、空行、键序、其它键的
//! 引号与缩进）逐字不动；绝不整段重新序列化 YAML。只认得几种写法——单行标量（裸写 / 单引号 /
//! 双引号）、单行流式列表 `[a, b]`、块列表（逐行 `- a`，项为单行标量）；认不出的写法（嵌套
//! 映射、锚点 / 别名、块标量、行内注释、跨行流式列表、重复的键……）只读、如实说原因，交回源码。
//! `luna:` 块归 Lunarca 自有（TDD-06 §4.5），这里不列、不改。
//!
//! 全部是纯函数：输入是当前全文（编辑器缓冲区或文件），输出是新全文或拒绝原因。面板的一次
//! 编辑由调用方作为编辑器里的一次变更应用（[`text_change`] 给出最小替换区间），不另起写盘路径。

use crate::frontmatter::{frontmatter_inner_range, has_unclosed_frontmatter};

/// 这一项为什么不能用表单改（只读显示，交回源码）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditSkip {
    /// 锚点 / 别名 / 块标量 / 标签（`&` `*` `|` `>` `!`）或带引号的键等认不出的写法。
    UnsupportedForm,
    /// 值里带行内注释：改写后无法保证注释仍在原处。
    InlineComment,
    /// 跨行的标量或流式列表。
    MultiLine,
    /// 嵌套映射、列表里套映射。
    Nested,
    /// 同一个键写了不止一次。
    DuplicateKey,
    /// 开头是 `---` 却没有闭合行：一律拒写（TDD-06 §4.4）。
    Unclosed,
    /// 要加 / 改成的键名已经存在。
    KeyExists,
    /// 键名不合法（空、带 `:` / `#`、以 YAML 指示符开头，或是保留的 `luna`）。
    InvalidKey,
    /// 要改的键不存在。
    NotFound,
    /// 值的写法不对（数字不像数字、日期不像日期），写进去会悄悄变成别的类型。
    InvalidValue,
}

/// 一个属性的值（按写法推断类型，ADR-139 ②）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PropertyValue {
    Text(String),
    /// 原样保留数字的写法（`1.50` 不变成 `1.5`）。
    Number(String),
    Checkbox(bool),
    /// `YYYY-MM-DD`。
    Date(String),
    /// ISO 8601 日期时间（`YYYY-MM-DDTHH:MM[:SS]…`）。
    DateTime(String),
    List(Vec<String>),
}

/// 属性类型（改类型时的目标）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropertyKind {
    Text,
    Number,
    Checkbox,
    Date,
    DateTime,
    List,
}

/// 面板上的一行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Property {
    pub key: String,
    /// 能用表单改时的值；不能改时是原因（行只读，给「在源码中编辑」入口）。
    pub value: Result<PropertyValue, EditSkip>,
    /// 值在源码里的原文（只读行照原文显示）。
    pub raw: String,
    /// 键所在的行（1 起，按整个文件计）。
    pub line: u32,
}

/// 面板的一次编辑。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PropertyEdit {
    Set {
        key: String,
        value: PropertyValue,
    },
    Add {
        key: String,
        value: PropertyValue,
    },
    Remove {
        key: String,
    },
    Rename {
        from: String,
        to: String,
    },
    /// 改类型：按 [`convert`] 把现值换成目标类型再写。日期类在现值认不出日期时取
    /// `today`（`YYYY-MM-DD`）/ `now`（`YYYY-MM-DDTHH:MM`）——「现在几点」是调用方按本地时区给的。
    Retype {
        key: String,
        kind: PropertyKind,
        today: String,
        now: String,
    },
}

/// 最小替换区间（UTF-16 下标，与编辑器的位置口径一致）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextChange {
    pub from: usize,
    pub to: usize,
    pub insert: String,
}

/// Lunarca 自有块的键：不列、不改、不许用户拿来当新键名。
const LUNA_KEY: &str = "luna";

// ─── 行与列表项（ADR-135 起就在用的原语）────────────────────────────────────

pub(crate) struct Line<'a> {
    /// 行在全文中的起止（不含行尾换行）。
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) text: &'a str,
}

pub(crate) fn split_lines(content: &str, range: std::ops::Range<usize>) -> Vec<Line<'_>> {
    let mut out = Vec::new();
    let mut offset = range.start;
    for raw in content[range].split_inclusive('\n') {
        let text = raw.trim_end_matches(['\n', '\r']);
        out.push(Line {
            start: offset,
            end: offset + text.len(),
            text,
        });
        offset += raw.len();
    }
    out
}

/// 全文用的换行风格（第一处换行说了算；没有换行时用 `\n`）。
fn newline_of(content: &str) -> &'static str {
    match content.find('\n') {
        Some(i) if i > 0 && content.as_bytes()[i - 1] == b'\r' => "\r\n",
        _ => "\n",
    }
}

/// 一个列表项 / 标量：去引号、解转义后的值与原引号字符。
pub(crate) struct Item {
    pub(crate) value: String,
    pub(crate) quote: Option<char>,
}

pub(crate) fn parse_item(raw: &str) -> Result<Item, EditSkip> {
    let raw = raw.trim();
    if let Some(inner) = raw.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')) {
        if raw.len() >= 2 {
            // 单引号里唯一的转义是 `''`；落单的 `'` 说明这不是一个完整的单引号标量。
            if inner.replace("''", "").contains('\'') {
                return Err(EditSkip::UnsupportedForm);
            }
            return Ok(Item {
                value: inner.replace("''", "'"),
                quote: Some('\''),
            });
        }
    }
    if let Some(inner) = raw.strip_prefix('"').and_then(|r| r.strip_suffix('"')) {
        if raw.len() >= 2 {
            return unescape_double(inner).map(|value| Item {
                value,
                quote: Some('"'),
            });
        }
    }
    if raw.starts_with(['"', '\'', '&', '*', '!', '[', '{', '|', '>']) {
        return Err(EditSkip::UnsupportedForm);
    }
    if raw.contains(" #") {
        return Err(EditSkip::InlineComment);
    }
    // `- a: b` 是列表里套映射，不是一个写着「a: b」的字符串。
    if raw.contains(": ") || raw.ends_with(':') {
        return Err(EditSkip::Nested);
    }
    Ok(Item {
        value: raw.to_owned(),
        quote: None,
    })
}

/// 双引号标量只认最常见的几种转义；其它（`\x`、`\u` 等）当作认不出的写法。
fn unescape_double(inner: &str) -> Result<String, EditSkip> {
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => return Err(EditSkip::UnsupportedForm),
            '\\' => match chars.next() {
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                _ => return Err(EditSkip::UnsupportedForm),
            },
            _ => out.push(ch),
        }
    }
    Ok(out)
}

fn escape_double(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\t', "\\t")
}

/// 裸写会改变 YAML 含义的字符串（要加引号）。
fn needs_quote(value: &str) -> bool {
    value.is_empty()
        || value != value.trim()
        || value.starts_with([
            '-', '?', ':', ',', '[', ']', '{', '}', '#', '&', '*', '!', '|', '>', '\'', '"', '%',
            '@', '`',
        ])
        || value.contains(": ")
        || value.contains(" #")
        || value.ends_with(':')
        || value.contains(['\n', '\t'])
}

/// 裸写会被读成别的类型的文本（布尔 / 空值 / 数字 / 日期）：作为文本写出时要加引号。
fn looks_typed(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "true" | "false" | "yes" | "no" | "on" | "off" | "null" | "~"
    ) || is_number(value)
        || is_date(value)
        || is_datetime(value)
}

/// 写出一段文本：沿用原来的引号；原来没引号而裸写会出问题时用双引号。
fn render_text(value: &str, quote: Option<char>) -> String {
    match quote {
        Some('\'') if !value.contains(['\n', '\t']) => format!("'{}'", value.replace('\'', "''")),
        Some(_) => format!("\"{}\"", escape_double(value)),
        None if needs_quote(value) || looks_typed(value) => {
            format!("\"{}\"", escape_double(value))
        }
        None => value.to_owned(),
    }
}

/// 列表项沿用同一位置的引号；流式列表里逗号与括号也要引号。
pub(crate) fn render_item(value: &str, quote: Option<char>) -> String {
    if quote.is_none() && value.contains([',', '[', ']', '{', '}']) {
        return format!("\"{}\"", escape_double(value));
    }
    render_text(value, quote)
}

pub(crate) fn dedupe(values: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for value in values {
        if !out.iter().any(|kept| kept.eq_ignore_ascii_case(&value)) {
            out.push(value);
        }
    }
    out
}

/// 按新值重写每一项：没变的原样保留原文，变了的沿用同一位置的引号风格。
fn render_items(items: &[Item], raws: &[&str], next: &[String]) -> Vec<String> {
    next.iter()
        .enumerate()
        .map(|(index, value)| {
            if let Some(pos) = items.iter().position(|item| &item.value == value) {
                raws[pos].trim().to_owned()
            } else {
                render_item(value, items.get(index).and_then(|item| item.quote))
            }
        })
        .collect()
}

fn splice(content: &str, start: usize, end: usize, replacement: &str) -> String {
    format!("{}{}{}", &content[..start], replacement, &content[end..])
}

// ─── 顶层键 ──────────────────────────────────────────────────────────────────

/// 一个顶层键占的行：键所在行与其后属于它的续行（缩进行、`- ` 项、续写的标量）。
struct Entry<'a> {
    key: &'a str,
    /// 键名是否是认得出的写法（带引号的键等认不出）。
    plain_key: bool,
    key_line: usize,
    last_line: usize,
    /// 冒号之后（含前导空白）在全文中的起点。
    value_start: usize,
}

/// 顶层键行：行首不缩进、不是注释或列表项，第一个「后面跟空白或行尾的冒号」之前是键。
pub(crate) fn key_of(text: &str) -> Option<(&str, usize)> {
    if text.starts_with([' ', '\t', '#', '-']) || text.trim().is_empty() {
        return None;
    }
    let bytes = text.as_bytes();
    let colon = (0..bytes.len()).find(|&i| {
        bytes[i] == b':' && bytes.get(i + 1).is_none_or(|b| *b == b' ' || *b == b'\t')
    })?;
    Some((text[..colon].trim_end(), colon + 1))
}

fn entries<'a>(lines: &[Line<'a>]) -> Vec<Entry<'a>> {
    let mut out: Vec<Entry<'a>> = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if let Some((key, after_colon)) = key_of(line.text) {
            out.push(Entry {
                key,
                plain_key: !key.is_empty() && !key.starts_with(['"', '\'', '?', '&', '*', '!']),
                key_line: index,
                last_line: index,
                value_start: line.start + after_colon,
            });
        } else if line.text.trim().is_empty() || line.text.starts_with('#') {
            // 空行与行首注释不属于任何键（键之间的留白原样留着）。
        } else if let Some(last) = out.last_mut() {
            last.last_line = index;
        }
    }
    out
}

/// 一个键的值是哪种写法。
enum Form<'a> {
    /// 单行标量（`key: v`）或什么都没写（`key:`，此时 `raw` 为空）。
    Scalar { raw: &'a str },
    /// 单行流式列表：`[` 之后、`]` 之前的内容。
    Flow { inside: &'a str, open: usize },
    /// 块列表：各项的行号与项原文。
    Block { items: Vec<(usize, &'a str)> },
}

fn form<'a>(content: &'a str, lines: &[Line<'a>], entry: &Entry<'a>) -> Result<Form<'a>, EditSkip> {
    if !entry.plain_key {
        return Err(EditSkip::UnsupportedForm);
    }
    let key_line = &lines[entry.key_line];
    let value = content[entry.value_start..key_line.end].trim();
    let continuation = &lines[entry.key_line + 1..=entry.last_line];
    if value.starts_with(['&', '*', '|', '>', '!']) {
        return Err(EditSkip::UnsupportedForm);
    }
    if value.starts_with('#') {
        return Err(EditSkip::InlineComment);
    }
    if value.starts_with('{') {
        return Err(EditSkip::Nested);
    }
    if let Some(list) = value.strip_prefix('[') {
        let Some(close) = list.find(']') else {
            return Err(EditSkip::MultiLine);
        };
        if !continuation.is_empty() {
            return Err(EditSkip::MultiLine);
        }
        let (inside, suffix) = (&list[..close], list[close + 1..].trim());
        if inside.contains(['[', '{']) {
            return Err(EditSkip::Nested);
        }
        // `]` 之后只允许空白或注释（注释原样留在原位）。
        if !suffix.is_empty() && !suffix.starts_with('#') {
            return Err(EditSkip::UnsupportedForm);
        }
        let open = key_line.start + key_line.text.find('[').expect("flow list has '['") + 1;
        return Ok(Form::Flow { inside, open });
    }
    if !value.is_empty() {
        if !continuation.is_empty() {
            return Err(EditSkip::MultiLine);
        }
        return Ok(Form::Scalar { raw: value });
    }
    if continuation.is_empty() {
        return Ok(Form::Scalar { raw: "" });
    }
    // 冒号后没写值：其后要么是同一缩进的 `- ` 项（块列表），要么是嵌套映射。
    let mut prefix: Option<&str> = None;
    let mut items = Vec::new();
    for (offset, line) in continuation.iter().enumerate() {
        let indent_len = line.text.len() - line.text.trim_start().len();
        let (indent, rest) = line.text.split_at(indent_len);
        let Some(item) = rest.strip_prefix("- ").or((rest == "-").then_some("")) else {
            return Err(if rest.trim().is_empty() || rest.starts_with('#') {
                EditSkip::UnsupportedForm
            } else {
                EditSkip::Nested
            });
        };
        match prefix {
            None => prefix = Some(indent),
            Some(p) if p != indent => return Err(EditSkip::Nested),
            Some(_) => {}
        }
        items.push((entry.key_line + 1 + offset, item));
    }
    Ok(Form::Block { items })
}

/// `tags` / `aliases` 恒为列表（与索引同一读法）：写成一个字符串时按逗号分项。
fn is_list_key(key: &str) -> bool {
    matches!(key, "tags" | "aliases")
}

fn is_number(value: &str) -> bool {
    let digits = value.strip_prefix('-').unwrap_or(value);
    let mut parts = digits.splitn(2, '.');
    let int = parts.next().unwrap_or("");
    let frac = parts.next();
    !int.is_empty()
        && int.bytes().all(|b| b.is_ascii_digit())
        && frac.is_none_or(|f| !f.is_empty() && f.bytes().all(|b| b.is_ascii_digit()))
}

fn is_date(value: &str) -> bool {
    is_date_bytes(value.as_bytes())
}

/// 按字节判断（不切字符串）：值可能是任意文字，按字节下标切 `&str` 会切进多字节字符里 panic——
/// 而 Tauri 命令里的 panic 会让整个应用退出（真机 2026-10-04：「十月周会修订」一回车就闪退）。
fn is_date_bytes(b: &[u8]) -> bool {
    b.len() == 10
        && b.iter().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                *c == b'-'
            } else {
                c.is_ascii_digit()
            }
        })
}

fn is_datetime(value: &str) -> bool {
    let b = value.as_bytes();
    b.len() >= 16
        && is_date_bytes(&b[..10])
        && matches!(b[10], b'T' | b' ')
        && b[11..13].iter().all(u8::is_ascii_digit)
        && b[13] == b':'
        && b[14..16].iter().all(u8::is_ascii_digit)
}

/// 单个标量按写法推断类型：日期不论引号；布尔与数字只认裸写（加了引号就是用户要的文本）。
fn infer(item: &Item) -> PropertyValue {
    let value = item.value.as_str();
    if is_date(value) {
        return PropertyValue::Date(value.to_owned());
    }
    if is_datetime(value) {
        return PropertyValue::DateTime(value.to_owned());
    }
    if item.quote.is_none() {
        match value {
            "true" | "True" | "TRUE" => return PropertyValue::Checkbox(true),
            "false" | "False" | "FALSE" => return PropertyValue::Checkbox(false),
            "null" | "Null" | "NULL" | "~" => return PropertyValue::Text(String::new()),
            _ if is_number(value) => return PropertyValue::Number(value.to_owned()),
            _ => {}
        }
    }
    PropertyValue::Text(value.to_owned())
}

fn split_scalar_list(item: &Item) -> Vec<String> {
    item.value
        .split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
        .collect()
}

fn list_items(raws: &[&str]) -> Result<Vec<Item>, EditSkip> {
    raws.iter().map(|raw| parse_item(raw)).collect()
}

fn flow_raws(inside: &str) -> Vec<&str> {
    if inside.trim().is_empty() {
        Vec::new()
    } else {
        inside.split(',').collect()
    }
}

fn value_of(entry: &Entry<'_>, form: &Form<'_>) -> Result<PropertyValue, EditSkip> {
    match form {
        Form::Scalar { raw: "" } => Ok(if is_list_key(entry.key) {
            PropertyValue::List(Vec::new())
        } else {
            PropertyValue::Text(String::new())
        }),
        Form::Scalar { raw } => {
            let item = parse_item(raw)?;
            Ok(if is_list_key(entry.key) {
                PropertyValue::List(split_scalar_list(&item))
            } else {
                infer(&item)
            })
        }
        Form::Flow { inside, .. } => {
            let items = list_items(&flow_raws(inside))?;
            Ok(PropertyValue::List(
                items.into_iter().map(|i| i.value).collect(),
            ))
        }
        Form::Block { items } => {
            let raws: Vec<&str> = items.iter().map(|(_, raw)| *raw).collect();
            let items = list_items(&raws)?;
            Ok(PropertyValue::List(
                items.into_iter().map(|i| i.value).collect(),
            ))
        }
    }
}

fn line_number(content: &str, offset: usize) -> u32 {
    u32::try_from(content[..offset].matches('\n').count() + 1).unwrap_or(u32::MAX)
}

/// 面板要显示的属性（按书写顺序；`luna:` 不列）。没有 frontmatter 时为空；开头 `---` 没闭合
/// 时是 `Err(Unclosed)`——那时面板不显示表单，提示去源码里修。
pub fn read_properties(content: &str) -> Result<Vec<Property>, EditSkip> {
    let Some(inner) = frontmatter_inner_range(content) else {
        return if has_unclosed_frontmatter(content) {
            Err(EditSkip::Unclosed)
        } else {
            Ok(Vec::new())
        };
    };
    let lines = split_lines(content, inner);
    let entries = entries(&lines);
    Ok(entries
        .iter()
        .filter(|entry| entry.key != LUNA_KEY)
        .map(|entry| {
            let duplicated = entries.iter().filter(|e| e.key == entry.key).count() > 1;
            let value = if duplicated {
                Err(EditSkip::DuplicateKey)
            } else {
                form(content, &lines, entry).and_then(|form| value_of(entry, &form))
            };
            let raw_end = lines[entry.last_line].end;
            Property {
                key: entry.key.to_owned(),
                value,
                raw: content[entry.value_start..raw_end].trim().to_owned(),
                line: line_number(content, lines[entry.key_line].start),
            }
        })
        .collect())
}

// ─── 写 ──────────────────────────────────────────────────────────────────────

/// 把某个顶层键的列表值交给 `transform` 改写（ADR-135 的标签写入就是它的调用方）。没有这个
/// 键或结果不变时 `Ok(None)`；`dedupe` = 结果按大小写不敏感去重。
pub(crate) fn rewrite_list_value(
    content: &str,
    key: &str,
    dedupe_values: bool,
    transform: impl Fn(Vec<String>) -> Vec<String>,
) -> Result<Option<String>, EditSkip> {
    let Some(inner) = frontmatter_inner_range(content) else {
        return Ok(None);
    };
    let lines = split_lines(content, inner);
    let all = entries(&lines);
    let mut found = all.iter().filter(|e| e.key == key);
    let Some(entry) = found.next() else {
        return Ok(None);
    };
    if found.next().is_some() {
        return Err(EditSkip::DuplicateKey);
    }
    let finish = |next: Vec<String>| if dedupe_values { dedupe(next) } else { next };
    match form(content, &lines, entry)? {
        Form::Flow { inside, open } => {
            let raws = flow_raws(inside);
            let items = list_items(&raws)?;
            let current: Vec<String> = items.iter().map(|i| i.value.clone()).collect();
            let next = finish(transform(current.clone()));
            if next == current {
                return Ok(None);
            }
            // 沿用原分隔风格；只有一项、看不出风格时用常见的 `, `。
            let separator = if inside.contains(',') && !inside.contains(", ") {
                ","
            } else {
                ", "
            };
            let rendered = render_items(&items, &raws, &next).join(separator);
            Ok(Some(splice(content, open, open + inside.len(), &rendered)))
        }
        Form::Scalar { raw } if !raw.is_empty() => {
            let item = parse_item(raw)?;
            let current = split_scalar_list(&item);
            let next = finish(transform(current.clone()));
            if next == current {
                return Ok(None);
            }
            let joined = next.join(", ");
            // 块上下文里的朴素标量可以含逗号（索引正是按逗号切的），原来没引号就不加。
            let rendered = match item.quote {
                Some(q) => render_text(&joined, Some(q)),
                None if !needs_quote(&joined) => joined,
                None => format!("\"{}\"", escape_double(&joined)),
            };
            let start = content[entry.value_start..]
                .find(raw)
                .expect("raw is on the key line")
                + entry.value_start;
            Ok(Some(splice(content, start, start + raw.len(), &rendered)))
        }
        Form::Scalar { .. } => {
            let next = finish(transform(Vec::new()));
            if next.is_empty() {
                return Ok(None);
            }
            Ok(Some(write_flow(content, &lines, entry, &next)))
        }
        Form::Block { items: entries_raw } => {
            let raws: Vec<&str> = entries_raw.iter().map(|(_, raw)| *raw).collect();
            let items = list_items(&raws)?;
            let current: Vec<String> = items.iter().map(|i| i.value.clone()).collect();
            let next = finish(transform(current.clone()));
            if next == current {
                return Ok(None);
            }
            if next.is_empty() {
                // 删光了：`key: []`，并去掉原列表行。
                return Ok(Some(write_flow(content, &lines, entry, &next)));
            }
            let first = &lines[entries_raw[0].0];
            let indent = &first.text[..first.text.len() - first.text.trim_start().len()];
            let body = render_items(&items, &raws, &next)
                .iter()
                .map(|item| format!("{indent}- {item}"))
                .collect::<Vec<_>>()
                .join(newline_of(content));
            let block_end = lines[entries_raw[entries_raw.len() - 1].0].end;
            Ok(Some(splice(content, first.start, block_end, &body)))
        }
    }
}

/// 把整个值（键行冒号之后到续行末尾）换成单行流式列表。
fn write_flow(content: &str, lines: &[Line<'_>], entry: &Entry<'_>, values: &[String]) -> String {
    let rendered: Vec<String> = values.iter().map(|v| render_item(v, None)).collect();
    replace_value(content, lines, entry, &format!("[{}]", rendered.join(", ")))
}

/// 把整个值换成 `rendered`（键与冒号原样保留，冒号后一个空格）。
fn replace_value(content: &str, lines: &[Line<'_>], entry: &Entry<'_>, rendered: &str) -> String {
    let end = lines[entry.last_line].end;
    let replacement = if rendered.is_empty() {
        String::new()
    } else {
        format!(" {rendered}")
    };
    splice(content, entry.value_start, end, &replacement)
}

fn render_value(value: &PropertyValue, quote: Option<char>) -> String {
    match value {
        PropertyValue::Text(text) if text.is_empty() && quote.is_none() => String::new(),
        PropertyValue::Text(text) => render_text(text, quote),
        PropertyValue::Number(n) | PropertyValue::Date(n) | PropertyValue::DateTime(n) => n.clone(),
        PropertyValue::Checkbox(b) => b.to_string(),
        PropertyValue::List(items) => format!(
            "[{}]",
            items
                .iter()
                .map(|v| render_item(v, None))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// 写出前校验值本身：数字 / 日期必须真是那个写法，否则会悄悄变成别的类型。
fn validate_value(value: &PropertyValue) -> Result<(), EditSkip> {
    let ok = match value {
        PropertyValue::Number(n) => is_number(n),
        PropertyValue::Date(d) => is_date(d),
        PropertyValue::DateTime(d) => is_datetime(d),
        _ => true,
    };
    ok.then_some(()).ok_or(EditSkip::InvalidValue)
}

fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key == key.trim()
        && !key.eq_ignore_ascii_case(LUNA_KEY)
        && !key.contains([':', '#', '\n', '\r'])
        && !key.starts_with([
            '-', '?', ',', '[', ']', '{', '}', '&', '*', '!', '|', '>', '\'', '"', '%', '@', '`',
        ])
}

fn text_of(value: &PropertyValue) -> String {
    match value {
        PropertyValue::Text(s)
        | PropertyValue::Number(s)
        | PropertyValue::Date(s)
        | PropertyValue::DateTime(s) => s.clone(),
        PropertyValue::Checkbox(b) => b.to_string(),
        PropertyValue::List(items) => items.join(", "),
    }
}

/// 改类型时现值怎么换（ADR-139 ②）：能保留的意思尽量保留——文本 ↔ 列表互转、`true` 读成
/// 勾选、日期时间截成日期；认不出的数字写 `0`，认不出的日期取调用方给的今天 / 现在。
#[must_use]
pub fn convert(value: &PropertyValue, kind: PropertyKind, today: &str, now: &str) -> PropertyValue {
    let text = text_of(value);
    let text = text.trim();
    match kind {
        PropertyKind::Text => PropertyValue::Text(text.to_owned()),
        PropertyKind::List => PropertyValue::List(match value {
            PropertyValue::List(items) => items.clone(),
            _ if text.is_empty() => Vec::new(),
            _ => vec![text.to_owned()],
        }),
        PropertyKind::Checkbox => PropertyValue::Checkbox(match value {
            PropertyValue::Checkbox(b) => *b,
            _ => text.eq_ignore_ascii_case("true"),
        }),
        PropertyKind::Number => {
            PropertyValue::Number(if is_number(text) { text } else { "0" }.to_owned())
        }
        PropertyKind::Date => PropertyValue::Date(match text.get(..10) {
            Some(head) if is_date(head) => head.to_owned(),
            _ => today.to_owned(),
        }),
        PropertyKind::DateTime => PropertyValue::DateTime(if is_datetime(text) {
            text.to_owned()
        } else if is_date(text) {
            format!("{text}T00:00")
        } else {
            now.to_owned()
        }),
    }
}

/// 应用面板的一次编辑，返回新全文（与原文相同 = 什么都没改）。
pub fn apply_property_edit(content: &str, edit: &PropertyEdit) -> Result<String, EditSkip> {
    if let PropertyEdit::Add { key, value } = edit {
        return add_property(content, key, value);
    }
    if let PropertyEdit::Retype {
        key,
        kind,
        today,
        now,
    } = edit
    {
        let current = read_properties(content)?
            .into_iter()
            .find(|p| &p.key == key)
            .ok_or(EditSkip::NotFound)?
            .value?;
        let value = convert(&current, *kind, today, now);
        return apply_property_edit(
            content,
            &PropertyEdit::Set {
                key: key.clone(),
                value,
            },
        );
    }
    let Some(inner) = frontmatter_inner_range(content) else {
        return Err(if has_unclosed_frontmatter(content) {
            EditSkip::Unclosed
        } else {
            EditSkip::NotFound
        });
    };
    let lines = split_lines(content, inner);
    let all = entries(&lines);
    let key = match edit {
        PropertyEdit::Set { key, .. } | PropertyEdit::Remove { key } => key,
        PropertyEdit::Rename { from, .. } => from,
        PropertyEdit::Add { .. } | PropertyEdit::Retype { .. } => unreachable!("handled above"),
    };
    if key == LUNA_KEY {
        return Err(EditSkip::InvalidKey);
    }
    let matching: Vec<&Entry<'_>> = all.iter().filter(|e| e.key == key).collect();
    let entry = match matching.as_slice() {
        [] => return Err(EditSkip::NotFound),
        [entry] => *entry,
        _ => return Err(EditSkip::DuplicateKey),
    };
    match edit {
        PropertyEdit::Remove { .. } => {
            let start = lines[entry.key_line].start;
            let end = lines[entry.last_line].end;
            let end = end + newline_len(&content[end..]);
            Ok(splice(content, start, end, ""))
        }
        PropertyEdit::Rename { to, .. } => {
            if !valid_key(to) {
                return Err(EditSkip::InvalidKey);
            }
            if all
                .iter()
                .any(|e| e.key_line != entry.key_line && e.key.eq_ignore_ascii_case(to))
            {
                return Err(EditSkip::KeyExists);
            }
            form(content, &lines, entry)?;
            let start = lines[entry.key_line].start;
            Ok(splice(content, start, start + entry.key.len(), to))
        }
        PropertyEdit::Set { value, .. } => {
            validate_value(value)?;
            let current = form(content, &lines, entry)?;
            value_of(entry, &current)?;
            match (value, &current) {
                (PropertyValue::List(items), Form::Flow { .. } | Form::Block { .. }) => {
                    let items = items.clone();
                    Ok(
                        rewrite_list_value(content, key, false, move |_| items.clone())?
                            .unwrap_or_else(|| content.to_owned()),
                    )
                }
                (PropertyValue::List(items), Form::Scalar { raw })
                    if is_list_key(key) && !raw.is_empty() =>
                {
                    let items = items.clone();
                    Ok(
                        rewrite_list_value(content, key, false, move |_| items.clone())?
                            .unwrap_or_else(|| content.to_owned()),
                    )
                }
                (_, Form::Scalar { raw })
                    if !raw.is_empty() && !matches!(value, PropertyValue::List(_)) =>
                {
                    let item = parse_item(raw)?;
                    if &infer(&item) == value {
                        return Ok(content.to_owned());
                    }
                    // 只换值那几个字节：冒号与值之间、值之后的空白原样保留。
                    let quote = matches!(value, PropertyValue::Text(_))
                        .then_some(item.quote)
                        .flatten();
                    let rendered = render_value(value, quote);
                    if rendered.is_empty() {
                        return Ok(replace_value(content, &lines, entry, ""));
                    }
                    let start = content[entry.value_start..]
                        .find(raw)
                        .expect("raw is on the key line")
                        + entry.value_start;
                    Ok(splice(content, start, start + raw.len(), &rendered))
                }
                _ => Ok(replace_value(
                    content,
                    &lines,
                    entry,
                    &render_value(value, None),
                )),
            }
        }
        PropertyEdit::Add { .. } | PropertyEdit::Retype { .. } => unreachable!("handled above"),
    }
}

fn newline_len(rest: &str) -> usize {
    if rest.starts_with("\r\n") {
        2
    } else {
        usize::from(rest.starts_with('\n'))
    }
}

/// 加一个属性：追加在最后一个用户键之后（`luna:` 块之前）；没有 frontmatter 就在文件最前面建一段。
fn add_property(content: &str, key: &str, value: &PropertyValue) -> Result<String, EditSkip> {
    if !valid_key(key) {
        return Err(EditSkip::InvalidKey);
    }
    validate_value(value)?;
    let rendered = render_value(value, None);
    let line = if rendered.is_empty() {
        format!("{key}:")
    } else {
        format!("{key}: {rendered}")
    };
    let newline = newline_of(content);
    let Some(inner) = frontmatter_inner_range(content) else {
        if has_unclosed_frontmatter(content) {
            return Err(EditSkip::Unclosed);
        }
        return Ok(format!("---{newline}{line}{newline}---{newline}{content}"));
    };
    let lines = split_lines(content, inner.clone());
    let all = entries(&lines);
    if all.iter().any(|e| e.key.eq_ignore_ascii_case(key)) {
        return Err(EditSkip::KeyExists);
    }
    let at = all
        .iter()
        .rfind(|e| e.key != LUNA_KEY)
        .map_or(inner.start, |entry| {
            let end = lines[entry.last_line].end;
            end + newline_len(&content[end..])
        });
    Ok(splice(content, at, at, &format!("{line}{newline}")))
}

/// 旧全文 → 新全文的最小替换区间（UTF-16 下标），供编辑器作为一次变更应用。
#[must_use]
pub fn text_change(old: &str, new: &str) -> TextChange {
    let prefix: usize = old
        .char_indices()
        .zip(new.chars())
        .take_while(|((_, a), b)| a == b)
        .last()
        .map_or(0, |((i, a), _)| i + a.len_utf8());
    let old_rest = &old[prefix..];
    let new_rest = &new[prefix..];
    let suffix: usize = old_rest
        .chars()
        .rev()
        .zip(new_rest.chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum();
    let utf16 = |s: &str| s.encode_utf16().count();
    TextChange {
        from: utf16(&old[..prefix]),
        to: utf16(&old[..old.len() - suffix]),
        insert: new_rest[..new_rest.len() - suffix].to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "---\n# 我的注释\ntitle: '周会'\ndue: 2026-10-04\ndone: false\nscore: 1.50\ntags:\n  - 工作\n  - \"重要\"\naliases: [例会]\nmeta:\n  owner: 我\nanchor: &a 值\nluna:\n  type: note\n---\n正文\n";

    fn props(content: &str) -> Vec<(String, Result<PropertyValue, EditSkip>)> {
        read_properties(content)
            .unwrap()
            .into_iter()
            .map(|p| (p.key, p.value))
            .collect()
    }

    #[test]
    fn reads_properties_with_inferred_types_and_locked_forms() {
        use PropertyValue as V;
        assert_eq!(
            props(SAMPLE),
            vec![
                ("title".into(), Ok(V::Text("周会".into()))),
                ("due".into(), Ok(V::Date("2026-10-04".into()))),
                ("done".into(), Ok(V::Checkbox(false))),
                ("score".into(), Ok(V::Number("1.50".into()))),
                (
                    "tags".into(),
                    Ok(V::List(vec!["工作".into(), "重要".into()]))
                ),
                ("aliases".into(), Ok(V::List(vec!["例会".into()]))),
                ("meta".into(), Err(EditSkip::Nested)),
                ("anchor".into(), Err(EditSkip::UnsupportedForm)),
            ]
        );
        let title = &read_properties(SAMPLE).unwrap()[0];
        assert_eq!((title.raw.as_str(), title.line), ("'周会'", 3));
    }

    #[test]
    fn duplicate_keys_and_unclosed_frontmatter_are_refused() {
        assert_eq!(
            props("---\na: 1\na: 2\n---\n"),
            vec![
                ("a".into(), Err(EditSkip::DuplicateKey)),
                ("a".into(), Err(EditSkip::DuplicateKey))
            ]
        );
        assert_eq!(read_properties("---\na: 1\n正文"), Err(EditSkip::Unclosed));
        assert_eq!(read_properties("没有 frontmatter"), Ok(Vec::new()));
        let add = PropertyEdit::Add {
            key: "b".into(),
            value: PropertyValue::Text("x".into()),
        };
        assert_eq!(
            apply_property_edit("---\na: 1\n正文", &add),
            Err(EditSkip::Unclosed)
        );
    }

    #[test]
    fn set_changes_only_that_value_and_keeps_its_quote() {
        let set = |key: &str, value: PropertyValue| {
            apply_property_edit(
                SAMPLE,
                &PropertyEdit::Set {
                    key: key.into(),
                    value,
                },
            )
            .unwrap()
        };
        let out = set("title", PropertyValue::Text("周会 · 十月".into()));
        assert_eq!(out, SAMPLE.replace("title: '周会'", "title: '周会 · 十月'"));
        let out = set("done", PropertyValue::Checkbox(true));
        assert_eq!(out, SAMPLE.replace("done: false", "done: true"));
        let out = set(
            "tags",
            PropertyValue::List(vec!["工作".into(), "重要".into(), "新".into()]),
        );
        assert_eq!(
            out,
            SAMPLE.replace("  - \"重要\"\n", "  - \"重要\"\n  - 新\n")
        );
        // 类型变了：文本 → 列表整个值换写法，其它字节不动。
        let out = set("title", PropertyValue::List(vec!["a".into(), "b".into()]));
        assert_eq!(out, SAMPLE.replace("title: '周会'", "title: [a, b]"));
        // 像数字 / 布尔的文本要加引号，读回仍是文本。
        let out = set("score", PropertyValue::Text("12".into()));
        assert_eq!(out, SAMPLE.replace("score: 1.50", "score: \"12\""));
        assert_eq!(props(&out)[3].1, Ok(PropertyValue::Text("12".into())));
        // 同值不改。
        assert_eq!(set("due", PropertyValue::Date("2026-10-04".into())), SAMPLE);
    }

    #[test]
    fn locked_forms_and_invalid_values_are_not_written() {
        let set = |key: &str, value: PropertyValue| {
            apply_property_edit(
                SAMPLE,
                &PropertyEdit::Set {
                    key: key.into(),
                    value,
                },
            )
        };
        assert_eq!(
            set("meta", PropertyValue::Text("x".into())),
            Err(EditSkip::Nested)
        );
        assert_eq!(
            set("anchor", PropertyValue::Text("x".into())),
            Err(EditSkip::UnsupportedForm)
        );
        assert_eq!(
            set("luna", PropertyValue::Text("x".into())),
            Err(EditSkip::InvalidKey)
        );
        assert_eq!(
            set("missing", PropertyValue::Text("x".into())),
            Err(EditSkip::NotFound)
        );
        assert_eq!(
            set("due", PropertyValue::Date("十月四日".into())),
            Err(EditSkip::InvalidValue)
        );
    }

    #[test]
    fn add_remove_and_rename_touch_only_their_lines() {
        let add = apply_property_edit(
            SAMPLE,
            &PropertyEdit::Add {
                key: "status".into(),
                value: PropertyValue::Text("进行中".into()),
            },
        )
        .unwrap();
        assert_eq!(add, SAMPLE.replace("luna:\n", "status: 进行中\nluna:\n"));
        assert_eq!(
            apply_property_edit(
                SAMPLE,
                &PropertyEdit::Add {
                    key: "Title".into(),
                    value: PropertyValue::Text("x".into())
                }
            ),
            Err(EditSkip::KeyExists)
        );
        let removed =
            apply_property_edit(SAMPLE, &PropertyEdit::Remove { key: "tags".into() }).unwrap();
        assert_eq!(
            removed,
            SAMPLE.replace("tags:\n  - 工作\n  - \"重要\"\n", "")
        );
        let renamed = apply_property_edit(
            SAMPLE,
            &PropertyEdit::Rename {
                from: "due".into(),
                to: "截止".into(),
            },
        )
        .unwrap();
        assert_eq!(renamed, SAMPLE.replace("due: ", "截止: "));
        // 没有 frontmatter：在最前面建一段；CRLF 文件沿用 CRLF。
        let fresh = apply_property_edit(
            "正文\r\n",
            &PropertyEdit::Add {
                key: "done".into(),
                value: PropertyValue::Checkbox(false),
            },
        )
        .unwrap();
        assert_eq!(fresh, "---\r\ndone: false\r\n---\r\n正文\r\n");
    }

    #[test]
    fn long_non_ascii_values_never_panic() {
        // 回归（真机 2026-10-04）：超过 16 字节的中文值曾在日期时间判断里按字节切进「会」字中间。
        let content = "---\ntitle: 十月周会修订版本说明\n---\n";
        assert_eq!(
            props(content)[0].1,
            Ok(PropertyValue::Text("十月周会修订版本说明".into()))
        );
        let out = apply_property_edit(
            content,
            &PropertyEdit::Set {
                key: "title".into(),
                value: PropertyValue::Text("十月周会 · 修订".into()),
            },
        )
        .unwrap();
        assert_eq!(out, "---\ntitle: 十月周会 · 修订\n---\n");
        for kind in [
            PropertyKind::Date,
            PropertyKind::DateTime,
            PropertyKind::Number,
        ] {
            apply_property_edit(
                content,
                &PropertyEdit::Retype {
                    key: "title".into(),
                    kind,
                    today: "2026-10-05".into(),
                    now: "2026-10-05T09:30".into(),
                },
            )
            .unwrap();
        }
    }

    #[test]
    fn retype_keeps_meaning_where_it_can() {
        let retype = |key: &str, kind: PropertyKind| {
            apply_property_edit(
                SAMPLE,
                &PropertyEdit::Retype {
                    key: key.into(),
                    kind,
                    today: "2026-10-05".into(),
                    now: "2026-10-05T09:30".into(),
                },
            )
            .unwrap()
        };
        assert_eq!(
            retype("title", PropertyKind::List),
            SAMPLE.replace("title: '周会'", "title: [周会]")
        );
        assert_eq!(
            retype("title", PropertyKind::Date),
            SAMPLE.replace("title: '周会'", "title: 2026-10-05")
        );
        assert_eq!(
            retype("due", PropertyKind::DateTime),
            SAMPLE.replace("due: 2026-10-04", "due: 2026-10-04T00:00")
        );
        assert_eq!(
            retype("done", PropertyKind::Text),
            SAMPLE.replace("done: false", "done: \"false\"")
        );
        assert_eq!(
            retype("aliases", PropertyKind::Text),
            SAMPLE.replace("aliases: [例会]", "aliases: 例会")
        );
    }

    #[test]
    fn text_change_is_minimal_and_counts_utf16() {
        let change = text_change("a😀b周c", "a😀X周c");
        assert_eq!(
            change,
            TextChange {
                from: 3,
                to: 4,
                insert: "X".into()
            }
        );
        assert_eq!(
            text_change("same", "same"),
            TextChange {
                from: 4,
                to: 4,
                insert: String::new()
            }
        );
    }
}
