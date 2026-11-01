//! Word（.docx）→ HTML → Markdown（ADR-164 ⑥）。
//!
//! docx 是 zip 包里的一组 XML。这里只读正文（`word/document.xml`）以及解释它需要的样式、编号、
//! 关系与图片，走一遍生成简单的 HTML（标题、段落、列表、粗斜体、链接、表格、图片），再交给
//! 粘贴网页同一套 HTML → Markdown（与 MarkItDown 先转 HTML 的路子相同）。批注、修订、文本框、
//! 脚注不带入，记成 [`DocWarning`]。

use std::collections::HashMap;
use std::io::{Cursor, Read};

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

use crate::{push_warning, Converted, DocImportError, DocWarning, EmbeddedImage};

/// 图片在 HTML 里的占位（私用区字符，正文里不会出现）；走完正文再换成真正的链接。
const IMAGE_MARK: char = '\u{E000}';

/// 浏览器画不出来的图片格式（Windows 图元文件）：只留替代文字。
const UNRENDERABLE_IMAGES: [&str; 4] = ["emf", "wmf", "emz", "wmz"];

pub(crate) fn convert(
    bytes: &[u8],
    image_link: &mut dyn FnMut(usize, &str) -> String,
) -> Result<Converted, DocImportError> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| DocImportError::Corrupt(e.to_string()))?;
    let document = read_entry(&mut zip, "word/document.xml")?
        .ok_or_else(|| DocImportError::Corrupt("word/document.xml".into()))?;
    let rels = read_entry(&mut zip, "word/_rels/document.xml.rels")?
        .map(|xml| parse_rels(&xml))
        .unwrap_or_default();
    let styles = read_entry(&mut zip, "word/styles.xml")?
        .map(|xml| parse_styles(&xml))
        .unwrap_or_default();
    let numbering = read_entry(&mut zip, "word/numbering.xml")?
        .map(|xml| parse_numbering(&xml))
        .unwrap_or_default();
    let title = read_entry(&mut zip, "docProps/core.xml")?.and_then(|xml| parse_title(&xml));

    let mut walker = Walker::new(&rels);
    walker.run(&document)?;
    let mut html = render(&walker.blocks, &styles, &numbering);
    let mut warnings = walker.warnings;

    // 图片：同一张图被引用多次只落一份。
    let mut images = Vec::new();
    for (index, rel_id) in walker.image_rels.iter().enumerate() {
        let mark = format!("{IMAGE_MARK}{index}{IMAGE_MARK}");
        let resolved = rels
            .get(rel_id)
            .filter(|rel| !rel.external)
            .and_then(|rel| {
                let path = resolve_part(&rel.target);
                let ext = path.rsplit_once('.')?.1.to_ascii_lowercase();
                if UNRENDERABLE_IMAGES.contains(&ext.as_str()) {
                    return None;
                }
                let bytes = read_entry_bytes(&mut zip, &path).ok().flatten()?;
                Some((ext, bytes))
            });
        match resolved {
            Some((ext, bytes)) => {
                let link = image_link(images.len(), &ext);
                html = html.replace(&mark, &escape_attr(&link));
                images.push(EmbeddedImage { link, bytes });
            }
            None => {
                html = html.replace(&mark, "");
                push_warning(&mut warnings, DocWarning::ImagesOmitted);
            }
        }
    }

    let paste = luna_vault::html_paste::convert_html_with(&html, true);
    for warning in paste.warnings {
        match warning {
            luna_vault::html_paste::PasteWarning::ImagesOmitted => {
                push_warning(&mut warnings, DocWarning::ImagesOmitted);
            }
            luna_vault::html_paste::PasteWarning::ComplexTable => {
                push_warning(&mut warnings, DocWarning::TableFlattened);
            }
            _ => {}
        }
    }
    Ok(Converted {
        title,
        markdown: crate::tidy_escapes(&paste.markdown),
        warnings,
        images,
    })
}

fn read_entry(
    zip: &mut zip::ZipArchive<Cursor<&[u8]>>,
    name: &str,
) -> Result<Option<String>, DocImportError> {
    Ok(read_entry_bytes(zip, name)?.map(|bytes| String::from_utf8_lossy(&bytes).into_owned()))
}

fn read_entry_bytes(
    zip: &mut zip::ZipArchive<Cursor<&[u8]>>,
    name: &str,
) -> Result<Option<Vec<u8>>, DocImportError> {
    let mut file = match zip.by_name(name) {
        Ok(file) => file,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(DocImportError::Corrupt(e.to_string())),
    };
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|e| DocImportError::Corrupt(e.to_string()))?;
    Ok(Some(bytes))
}

/// 关系里的 `Target` 相对 `word/`；以 `/` 开头的是包根路径。
fn resolve_part(target: &str) -> String {
    match target.strip_prefix('/') {
        Some(absolute) => absolute.to_owned(),
        None => {
            let mut parts: Vec<&str> = vec!["word"];
            for segment in target.split('/') {
                match segment {
                    ".." => {
                        parts.pop();
                    }
                    "." | "" => {}
                    other => parts.push(other),
                }
            }
            parts.join("/")
        }
    }
}

// ---- 属性与 XML 小工具 ----

/// 按本地名取属性值（忽略前缀：`w:val` / `r:embed` 等）。
fn attr(e: &BytesStart<'_>, local: &[u8]) -> Option<String> {
    e.attributes().flatten().find_map(|a| {
        (a.key.local_name().as_ref() == local)
            .then(|| {
                a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                    .ok()
                    .map(|v| v.into_owned())
            })
            .flatten()
    })
}

/// `w:b` / `w:i` 这类开关：没有 `w:val` 或值不是 0 / false / off 即为开。
fn toggle_on(e: &BytesStart<'_>) -> bool {
    !matches!(
        attr(e, b"val").as_deref(),
        Some("0" | "false" | "off" | "none")
    )
}

/// 依次交出 XML 事件（开始 / 空元素 / 结束 / 文字）；文字已解开实体。
/// `Open` / `Close` 的第二个值为真表示空元素（`<a/>`）：它不加深 XML 层级。
enum Node<'a> {
    Open(BytesStart<'a>, bool),
    Close(Vec<u8>, bool),
    Text(String),
}

fn for_each_node(xml: &str, mut visit: impl FnMut(Node<'_>)) -> Result<(), DocImportError> {
    let mut reader = Reader::from_str(xml);
    loop {
        let event = reader
            .read_event()
            .map_err(|e| DocImportError::Corrupt(e.to_string()))?;
        match event {
            Event::Start(e) => visit(Node::Open(e, false)),
            Event::Empty(e) => {
                let name = e.name().as_ref().to_vec();
                visit(Node::Open(e, true));
                visit(Node::Close(name, true));
            }
            Event::End(e) => visit(Node::Close(e.name().as_ref().to_vec(), false)),
            Event::Text(t) => {
                let text = t
                    .xml10_content()
                    .map_err(|e| DocImportError::Corrupt(e.to_string()))?;
                visit(Node::Text(text.into_owned()));
            }
            Event::CData(t) => {
                visit(Node::Text(String::from_utf8_lossy(&t).into_owned()));
            }
            Event::GeneralRef(r) => {
                let resolved = match r.resolve_char_ref() {
                    Ok(Some(ch)) => Some(ch.to_string()),
                    _ => match r.decode().ok().as_deref() {
                        Some("lt") => Some("<".into()),
                        Some("gt") => Some(">".into()),
                        Some("amp") => Some("&".into()),
                        Some("quot") => Some("\"".into()),
                        Some("apos") => Some("'".into()),
                        _ => None,
                    },
                };
                if let Some(text) = resolved {
                    visit(Node::Text(text));
                }
            }
            Event::Eof => return Ok(()),
            _ => {}
        }
    }
}

fn local(name: &[u8]) -> &[u8] {
    name.rsplit(|b| *b == b':').next().unwrap_or(name)
}

fn escape_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_attr(text: &str) -> String {
    escape_text(text).replace('"', "&quot;")
}

// ---- 关系、样式、编号、标题 ----

#[derive(Default)]
struct Rel {
    target: String,
    external: bool,
}

fn parse_rels(xml: &str) -> HashMap<String, Rel> {
    let mut rels = HashMap::new();
    let _ = for_each_node(xml, |node| {
        if let Node::Open(e, _) = node {
            if local(e.name().as_ref()) == b"Relationship" {
                if let (Some(id), Some(target)) = (attr(&e, b"Id"), attr(&e, b"Target")) {
                    let external = attr(&e, b"TargetMode").as_deref() == Some("External");
                    rels.insert(id, Rel { target, external });
                }
            }
        }
    });
    rels
}

#[derive(Default)]
struct Style {
    name: Option<String>,
    based_on: Option<String>,
    outline: Option<u8>,
}

fn parse_styles(xml: &str) -> HashMap<String, Style> {
    let mut styles = HashMap::new();
    let mut current: Option<(String, Style)> = None;
    let _ = for_each_node(xml, |node| match node {
        Node::Open(e, _) => match e.name().as_ref() {
            b"w:style" => {
                current = (attr(&e, b"type").as_deref() == Some("paragraph"))
                    .then(|| attr(&e, b"styleId"))
                    .flatten()
                    .map(|id| (id, Style::default()));
            }
            b"w:name" => {
                if let Some((_, style)) = &mut current {
                    style.name = attr(&e, b"val");
                }
            }
            b"w:basedOn" => {
                if let Some((_, style)) = &mut current {
                    style.based_on = attr(&e, b"val");
                }
            }
            b"w:outlineLvl" => {
                if let Some((_, style)) = &mut current {
                    style.outline = attr(&e, b"val").and_then(|v| v.parse().ok());
                }
            }
            _ => {}
        },
        Node::Close(name, _) if name == b"w:style" => {
            if let Some((id, style)) = current.take() {
                styles.insert(id, style);
            }
        }
        _ => {}
    });
    styles
}

/// 样式对应的标题级别（1–6）；`None` = 正文。中英文样式名与大纲级别都认，沿 `basedOn` 往上找。
fn heading_level(styles: &HashMap<String, Style>, style_id: &str) -> Option<u8> {
    let mut id = style_id.to_owned();
    for _ in 0..8 {
        let Some(style) = styles.get(&id) else {
            return level_from_name(&id);
        };
        if let Some(level) = style.name.as_deref().and_then(level_from_name) {
            return Some(level);
        }
        if let Some(outline) = style.outline.filter(|o| *o < 9) {
            return Some((outline + 1).min(6));
        }
        id = style.based_on.clone()?;
    }
    None
}

fn level_from_name(name: &str) -> Option<u8> {
    let lower = name.trim().to_lowercase();
    if lower == "title" || lower == "标题" {
        return Some(1);
    }
    let digits = lower
        .strip_prefix("heading")
        .or_else(|| lower.strip_prefix("标题"))?
        .trim();
    digits
        .parse::<u8>()
        .ok()
        .filter(|n| *n >= 1)
        .map(|n| n.min(6))
}

/// 编号：`numId` → 各级是否为有序（不是项目符号）。
#[derive(Default)]
struct Numbering {
    abstract_formats: HashMap<String, HashMap<u8, String>>,
    num_to_abstract: HashMap<String, String>,
}

impl Numbering {
    fn ordered(&self, num_id: &str, level: u8) -> bool {
        self.num_to_abstract
            .get(num_id)
            .and_then(|abs| self.abstract_formats.get(abs))
            .and_then(|levels| levels.get(&level))
            .is_some_and(|fmt| fmt != "bullet" && fmt != "none")
    }
}

fn parse_numbering(xml: &str) -> Numbering {
    let mut numbering = Numbering::default();
    let mut abstract_id: Option<String> = None;
    let mut level: Option<u8> = None;
    let mut num_id: Option<String> = None;
    let _ = for_each_node(xml, |node| match node {
        Node::Open(e, _) => match e.name().as_ref() {
            b"w:abstractNum" => abstract_id = attr(&e, b"abstractNumId"),
            b"w:lvl" => level = attr(&e, b"ilvl").and_then(|v| v.parse().ok()),
            b"w:numFmt" => {
                if let (Some(abs), Some(lvl), Some(fmt)) = (&abstract_id, level, attr(&e, b"val")) {
                    numbering
                        .abstract_formats
                        .entry(abs.clone())
                        .or_default()
                        .insert(lvl, fmt);
                }
            }
            b"w:num" => num_id = attr(&e, b"numId"),
            b"w:abstractNumId" => {
                if let (Some(num), Some(abs)) = (&num_id, attr(&e, b"val")) {
                    numbering.num_to_abstract.insert(num.clone(), abs);
                }
            }
            _ => {}
        },
        Node::Close(name, _) => match name.as_slice() {
            b"w:abstractNum" => abstract_id = None,
            b"w:lvl" => level = None,
            b"w:num" => num_id = None,
            _ => {}
        },
        Node::Text(_) => {}
    });
    numbering
}

fn parse_title(xml: &str) -> Option<String> {
    let mut in_title = false;
    let mut title = String::new();
    let _ = for_each_node(xml, |node| match node {
        Node::Open(e, empty) => in_title = !empty && e.name().as_ref() == b"dc:title",
        Node::Close(..) => in_title = false,
        Node::Text(text) if in_title => title.push_str(&text),
        Node::Text(_) => {}
    });
    let title = title.trim().to_owned();
    (!title.is_empty()).then_some(title)
}

// ---- 正文 ----

/// 一段里的一截：同样粗斜体的相邻文字并成一截（Word 常把一句话拆成十几个 run）。
struct Segment {
    bold: bool,
    italic: bool,
    /// 已转义的 HTML。
    html: String,
    /// 链接的起止标签等原样 HTML：不与前后合并、不加粗斜体。
    raw: bool,
}

#[derive(Default)]
struct Para {
    style: Option<String>,
    num_id: Option<String>,
    level: u8,
    segments: Vec<Segment>,
}

impl Para {
    fn push_run(&mut self, bold: bool, italic: bool, html: String) {
        if html.is_empty() {
            return;
        }
        if let Some(last) = self.segments.last_mut() {
            if !last.raw && last.bold == bold && last.italic == italic {
                last.html.push_str(&html);
                return;
            }
        }
        self.segments.push(Segment {
            bold,
            italic,
            html,
            raw: false,
        });
    }

    fn push_raw(&mut self, html: String) {
        self.segments.push(Segment {
            bold: false,
            italic: false,
            html,
            raw: true,
        });
    }

    /// 段内 HTML；粗斜体标签收在文字两侧的空白之内（`** a **` 在 Markdown 里不成立）。
    fn inline_html(&self) -> String {
        let mut out = String::new();
        for segment in &self.segments {
            if segment.raw || !(segment.bold || segment.italic) {
                out.push_str(&segment.html);
                continue;
            }
            let trimmed = segment.html.trim();
            if trimmed.is_empty() {
                out.push_str(&segment.html);
                continue;
            }
            let lead = &segment.html[..segment.html.len() - segment.html.trim_start().len()];
            let tail = &segment.html[segment.html.trim_end().len()..];
            out.push_str(lead);
            let (open, close) = match (segment.bold, segment.italic) {
                (true, true) => ("<strong><em>", "</em></strong>"),
                (true, false) => ("<strong>", "</strong>"),
                _ => ("<em>", "</em>"),
            };
            out.push_str(open);
            out.push_str(trimmed);
            out.push_str(close);
            out.push_str(tail);
        }
        out.trim().to_owned()
    }
}

enum Block {
    Para {
        style: Option<String>,
        list: Option<(String, u8)>,
        html: String,
    },
    Table(Vec<Vec<String>>),
}

#[derive(Default)]
struct Run {
    bold: bool,
    italic: bool,
    html: String,
}

struct Walker<'r> {
    rels: &'r HashMap<String, Rel>,
    blocks: Vec<Block>,
    warnings: Vec<DocWarning>,
    /// 出现过的图片关系 id（序号即占位号）。
    image_rels: Vec<String>,
    depth: usize,
    /// 正在跳过的子树起点深度。
    skip_from: Option<usize>,
    para: Option<Para>,
    run: Option<Run>,
    in_ppr: bool,
    in_rpr: bool,
    in_text: bool,
    /// 每层超链接是否输出了 `<a>`（关闭时对应输出 `</a>`）。
    links: Vec<bool>,
    /// 图片：起点深度与找到的关系 id、替代文字。
    drawing: Option<(usize, Option<String>, Option<String>)>,
    table_depth: usize,
    rows: Vec<Vec<String>>,
    cell: Option<String>,
    cell_span: usize,
}

impl<'r> Walker<'r> {
    fn new(rels: &'r HashMap<String, Rel>) -> Self {
        Self {
            rels,
            blocks: Vec::new(),
            warnings: Vec::new(),
            image_rels: Vec::new(),
            depth: 0,
            skip_from: None,
            para: None,
            run: None,
            in_ppr: false,
            in_rpr: false,
            in_text: false,
            links: Vec::new(),
            drawing: None,
            table_depth: 0,
            rows: Vec::new(),
            cell: None,
            cell_span: 1,
        }
    }

    fn run(&mut self, xml: &str) -> Result<(), DocImportError> {
        for_each_node(xml, |node| match node {
            Node::Open(e, empty) => {
                if !empty {
                    self.depth += 1;
                }
                if self.skip_from.is_none() {
                    self.open(&e, empty);
                }
            }
            Node::Close(name, empty) => {
                if self.skip_from.is_none() {
                    self.close(&name, empty);
                }
                if !empty {
                    if self.skip_from == Some(self.depth) {
                        self.skip_from = None;
                    }
                    self.depth = self.depth.saturating_sub(1);
                }
            }
            Node::Text(text) => {
                if self.skip_from.is_none() && self.in_text && self.drawing.is_none() {
                    if let Some(run) = &mut self.run {
                        run.html.push_str(&escape_text(&text));
                    }
                }
            }
        })
    }

    fn skip(&mut self, empty: bool) {
        if !empty {
            self.skip_from = Some(self.depth);
        }
    }

    fn open(&mut self, e: &BytesStart<'_>, empty: bool) {
        let name = e.name();
        let name = name.as_ref();
        if self.drawing.is_some() {
            match local(name) {
                b"blip" | b"imagedata" => {
                    let id = attr(e, b"embed").or_else(|| attr(e, b"id"));
                    if let (Some((_, rel, _)), Some(id)) = (&mut self.drawing, id) {
                        rel.get_or_insert(id);
                    }
                }
                b"docPr" => {
                    let alt = attr(e, b"descr").filter(|s| !s.trim().is_empty());
                    if let (Some((_, _, slot)), Some(alt)) = (&mut self.drawing, alt) {
                        *slot = Some(alt);
                    }
                }
                b"txbxContent" => {
                    push_warning(&mut self.warnings, DocWarning::TextBoxesDropped);
                    self.skip(empty);
                }
                _ => {}
            }
            return;
        }
        match name {
            b"w:p" => self.para = Some(Para::default()),
            b"w:pPr" => self.in_ppr = !empty,
            b"w:rPr" => self.in_rpr = !empty,
            b"w:pStyle" if self.in_ppr => {
                if let Some(para) = &mut self.para {
                    para.style = attr(e, b"val");
                }
            }
            b"w:ilvl" if self.in_ppr => {
                if let Some(para) = &mut self.para {
                    para.level = attr(e, b"val").and_then(|v| v.parse().ok()).unwrap_or(0);
                }
            }
            b"w:numId" if self.in_ppr => {
                if let Some(para) = &mut self.para {
                    para.num_id = attr(e, b"val").filter(|v| v != "0");
                }
            }
            b"w:sectPr" => self.skip(empty),
            b"w:tabs" if self.in_ppr => self.skip(empty),
            b"w:r" => self.run = Some(Run::default()),
            b"w:b" if self.in_rpr && !self.in_ppr => {
                if let Some(run) = &mut self.run {
                    run.bold = toggle_on(e);
                }
            }
            b"w:i" if self.in_rpr && !self.in_ppr => {
                if let Some(run) = &mut self.run {
                    run.italic = toggle_on(e);
                }
            }
            b"w:t" => self.in_text = !empty,
            b"w:tab" if !self.in_ppr => self.push_run_html(" "),
            b"w:br" => {
                if attr(e, b"type").as_deref() != Some("page") {
                    self.push_run_html("<br>");
                }
            }
            b"w:cr" => self.push_run_html("<br>"),
            b"w:noBreakHyphen" => self.push_run_html("-"),
            b"w:del" | b"w:moveFrom" => {
                push_warning(&mut self.warnings, DocWarning::TrackedChangesDropped);
                self.skip(empty);
            }
            b"w:ins" | b"w:moveTo" => {
                push_warning(&mut self.warnings, DocWarning::TrackedChangesDropped);
            }
            b"w:instrText" | b"w:delInstrText" => self.skip(empty),
            b"w:footnoteReference" | b"w:endnoteReference" => {
                push_warning(&mut self.warnings, DocWarning::NotesDropped);
            }
            b"w:commentReference" | b"w:commentRangeStart" => {
                push_warning(&mut self.warnings, DocWarning::CommentsDropped);
            }
            b"w:txbxContent" => {
                push_warning(&mut self.warnings, DocWarning::TextBoxesDropped);
                self.skip(empty);
            }
            b"mc:Fallback" => self.skip(empty),
            b"w:drawing" | b"w:pict" | b"w:object" if !empty => {
                self.drawing = Some((self.depth, None, None));
            }
            b"w:hyperlink" => {
                let href = attr(e, b"id")
                    .and_then(|id| self.rels.get(&id))
                    .filter(|rel| rel.external)
                    .map(|rel| rel.target.clone());
                let opened = match (&mut self.para, href) {
                    (Some(para), Some(href)) if !empty => {
                        para.push_raw(format!("<a href=\"{}\">", escape_attr(&href)));
                        true
                    }
                    _ => false,
                };
                if !empty {
                    self.links.push(opened);
                }
            }
            b"w:tbl" => {
                self.table_depth += 1;
                if self.table_depth == 1 {
                    self.rows.clear();
                } else {
                    push_warning(&mut self.warnings, DocWarning::TableFlattened);
                }
            }
            b"w:tr" if self.table_depth == 1 => self.rows.push(Vec::new()),
            b"w:tc" if self.table_depth == 1 => {
                self.cell = Some(String::new());
                self.cell_span = 1;
            }
            b"w:gridSpan" if self.table_depth == 1 => {
                self.cell_span = attr(e, b"val")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1)
                    .max(1);
                if self.cell_span > 1 {
                    push_warning(&mut self.warnings, DocWarning::TableFlattened);
                }
            }
            b"w:vMerge" if self.table_depth == 1 => {
                push_warning(&mut self.warnings, DocWarning::TableFlattened);
            }
            _ => {}
        }
    }

    fn close(&mut self, name: &[u8], empty: bool) {
        if let Some((start, rel, alt)) = &self.drawing {
            if empty || *start != self.depth {
                return;
            }
            let (rel, alt) = (rel.clone(), alt.clone());
            self.drawing = None;
            let alt = escape_attr(alt.as_deref().unwrap_or_default());
            let html = match rel {
                Some(rel) => {
                    let index = match self.image_rels.iter().position(|r| *r == rel) {
                        Some(index) => index,
                        None => {
                            self.image_rels.push(rel);
                            self.image_rels.len() - 1
                        }
                    };
                    format!("<img src=\"{IMAGE_MARK}{index}{IMAGE_MARK}\" alt=\"{alt}\">")
                }
                None => alt,
            };
            self.push_run_html(&html);
            return;
        }
        match name {
            b"w:pPr" => self.in_ppr = false,
            b"w:rPr" => self.in_rpr = false,
            b"w:t" => self.in_text = false,
            b"w:r" => {
                if let (Some(run), Some(para)) = (self.run.take(), &mut self.para) {
                    para.push_run(run.bold, run.italic, run.html);
                }
            }
            b"w:hyperlink" => {
                if self.links.pop() == Some(true) {
                    if let Some(para) = &mut self.para {
                        para.push_raw("</a>".into());
                    }
                }
            }
            b"w:p" => self.finish_para(),
            b"w:tc" if self.table_depth == 1 => {
                if let (Some(cell), Some(row)) = (self.cell.take(), self.rows.last_mut()) {
                    row.push(cell);
                    row.extend(std::iter::repeat_n(String::new(), self.cell_span - 1));
                }
            }
            b"w:tbl" => {
                if self.table_depth == 1 {
                    let rows = std::mem::take(&mut self.rows);
                    if rows.iter().any(|row| !row.is_empty()) {
                        self.blocks.push(Block::Table(rows));
                    }
                }
                self.table_depth = self.table_depth.saturating_sub(1);
            }
            _ => {}
        }
    }

    fn push_run_html(&mut self, html: &str) {
        if let Some(run) = &mut self.run {
            run.html.push_str(html);
        } else if let Some(para) = &mut self.para {
            para.push_run(false, false, html.to_owned());
        }
    }

    fn finish_para(&mut self) {
        let Some(para) = self.para.take() else {
            return;
        };
        let html = para.inline_html();
        if self.table_depth > 0 {
            if let Some(cell) = &mut self.cell {
                if !html.is_empty() {
                    if !cell.is_empty() {
                        cell.push_str("<br>");
                    }
                    cell.push_str(&html);
                }
            }
            return;
        }
        if html.is_empty() {
            return;
        }
        self.blocks.push(Block::Para {
            style: para.style,
            list: para.num_id.map(|id| (id, para.level)),
            html,
        });
    }
}

/// 块 → HTML：列表按 `ilvl` 嵌套，表格首行作表头。
fn render(blocks: &[Block], styles: &HashMap<String, Style>, numbering: &Numbering) -> String {
    let mut out = String::new();
    // 打开着的列表：每层的结束标签。
    let mut lists: Vec<&'static str> = Vec::new();
    let close_lists = |out: &mut String, lists: &mut Vec<&'static str>, keep: usize| {
        while lists.len() > keep {
            let tag = lists.pop().unwrap_or("</ul>");
            out.push_str("</li>");
            out.push_str(tag);
        }
    };
    for block in blocks {
        match block {
            Block::Para {
                style,
                list: Some((num_id, level)),
                html,
            } if style
                .as_deref()
                .and_then(|s| heading_level(styles, s))
                .is_none() =>
            {
                let depth = usize::from(*level) + 1;
                close_lists(&mut out, &mut lists, depth);
                if lists.len() == depth {
                    out.push_str("</li>");
                }
                while lists.len() < depth {
                    let lvl = u8::try_from(lists.len()).unwrap_or(u8::MAX);
                    if numbering.ordered(num_id, lvl) {
                        out.push_str("<ol>");
                        lists.push("</ol>");
                    } else {
                        out.push_str("<ul>");
                        lists.push("</ul>");
                    }
                }
                out.push_str("<li>");
                out.push_str(html);
            }
            Block::Para { style, html, .. } => {
                close_lists(&mut out, &mut lists, 0);
                match style.as_deref().and_then(|s| heading_level(styles, s)) {
                    Some(level) => out.push_str(&format!("<h{level}>{html}</h{level}>")),
                    None => out.push_str(&format!("<p>{html}</p>")),
                }
            }
            Block::Table(rows) => {
                close_lists(&mut out, &mut lists, 0);
                out.push_str("<table>");
                for (index, row) in rows.iter().enumerate() {
                    let tag = if index == 0 { "th" } else { "td" };
                    out.push_str("<tr>");
                    for cell in row {
                        out.push_str(&format!("<{tag}>{cell}</{tag}>"));
                    }
                    out.push_str("</tr>");
                }
                out.push_str("</table>");
            }
        }
    }
    close_lists(&mut out, &mut lists, 0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    const NS: &str = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" \
                      xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" \
                      xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" \
                      xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" \
                      xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\"";

    fn docx(body: &str, extra: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            let document = format!(
                "<?xml version=\"1.0\"?><w:document {NS}><w:body>{body}</w:body></w:document>"
            );
            zip.start_file("word/document.xml", opts).unwrap();
            zip.write_all(document.as_bytes()).unwrap();
            for (name, bytes) in extra {
                zip.start_file(*name, opts).unwrap();
                zip.write_all(bytes).unwrap();
            }
            zip.finish().unwrap();
        }
        buf.into_inner()
    }

    const STYLES: &str = r#"<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
        <w:style w:type="paragraph" w:styleId="1"><w:name w:val="heading 1"/></w:style>
        <w:style w:type="paragraph" w:styleId="Subtitle2"><w:name w:val="My Sub"/><w:pPr><w:outlineLvl w:val="1"/></w:pPr></w:style>
        <w:style w:type="paragraph" w:styleId="Derived"><w:name w:val="Derived"/><w:basedOn w:val="1"/></w:style>
        </w:styles>"#;

    const NUMBERING: &str = r#"<w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
        <w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:numFmt w:val="decimal"/></w:lvl><w:lvl w:ilvl="1"><w:numFmt w:val="bullet"/></w:lvl></w:abstractNum>
        <w:abstractNum w:abstractNumId="1"><w:lvl w:ilvl="0"><w:numFmt w:val="bullet"/></w:lvl></w:abstractNum>
        <w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>
        <w:num w:numId="2"><w:abstractNumId w:val="1"/></w:num>
        </w:numbering>"#;

    fn para(style: Option<&str>, num: Option<(u8, u8)>, runs: &str) -> String {
        let mut ppr = String::new();
        if let Some(style) = style {
            ppr.push_str(&format!("<w:pStyle w:val=\"{style}\"/>"));
        }
        if let Some((id, lvl)) = num {
            ppr.push_str(&format!(
                "<w:numPr><w:ilvl w:val=\"{lvl}\"/><w:numId w:val=\"{id}\"/></w:numPr>"
            ));
        }
        format!("<w:p><w:pPr>{ppr}<w:rPr><w:b/></w:rPr></w:pPr>{runs}</w:p>")
    }

    fn run(text: &str, bold: bool, italic: bool) -> String {
        let mut rpr = String::new();
        if bold {
            rpr.push_str("<w:b/>");
        }
        if italic {
            rpr.push_str("<w:i w:val=\"1\"/>");
        }
        format!("<w:r><w:rPr>{rpr}</w:rPr><w:t xml:space=\"preserve\">{text}</w:t></w:r>")
    }

    fn convert_body(body: &str) -> Converted {
        let bytes = docx(
            body,
            &[
                ("word/styles.xml", STYLES.as_bytes()),
                ("word/numbering.xml", NUMBERING.as_bytes()),
            ],
        );
        convert(&bytes, &mut |i, ext| format!("assets/img-{i}.{ext}")).unwrap()
    }

    #[test]
    fn headings_by_style_name_outline_and_based_on() {
        let body = [
            para(Some("1"), None, &run("第一章", false, false)),
            para(Some("Subtitle2"), None, &run("小节", false, false)),
            para(Some("Derived"), None, &run("派生", false, false)),
            para(Some("Heading3"), None, &run("没有样式表也认", false, false)),
            para(
                None,
                None,
                &run("正文 &amp; 符号 &lt;tag&gt;", false, false),
            ),
        ]
        .concat();
        let md = convert_body(&body).markdown;
        assert!(
            md.starts_with("# 第一章\n\n## 小节\n\n# 派生\n\n### 没有样式表也认\n\n正文 & 符号 "),
            "实际：{md}"
        );
        assert!(md.contains("tag"), "实际：{md}");
    }

    #[test]
    fn runs_merge_and_emphasis_hugs_text() {
        let body = para(
            None,
            None,
            &[
                run("开头 ", false, false),
                run("加", true, false),
                run("粗 ", true, false),
                run("斜体", false, true),
                run("。", false, false),
            ]
            .concat(),
        );
        assert_eq!(convert_body(&body).markdown, "开头 **加粗** *斜体*。");
    }

    #[test]
    fn lists_nest_by_level_and_pick_ordered_or_bullet() {
        let body = [
            para(None, Some((1, 0)), &run("一", false, false)),
            para(None, Some((1, 1)), &run("子项", false, false)),
            para(None, Some((1, 0)), &run("二", false, false)),
            para(None, None, &run("隔开", false, false)),
            para(None, Some((2, 0)), &run("要点", false, false)),
        ]
        .concat();
        let md = convert_body(&body).markdown;
        // 有序列表每项都写成 `1.`（Markdown 渲染时自动编号），子项缩进在上一项之下。
        assert_eq!(md, "1. 一\n\n    - 子项\n1. 二\n\n隔开\n\n- 要点");
    }

    #[test]
    fn tables_links_tracked_changes_and_notes() {
        let rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
            <Relationship Id="rId9" Type="hyperlink" Target="https://example.com/a?b=1&amp;c=2" TargetMode="External"/>
            </Relationships>"#;
        let body = format!(
            "<w:tbl><w:tr><w:tc>{h1}</w:tc><w:tc>{h2}</w:tc></w:tr>\
             <w:tr><w:tc><w:tcPr><w:gridSpan w:val=\"2\"/></w:tcPr>{c}</w:tc></w:tr></w:tbl>\
             <w:p><w:hyperlink r:id=\"rId9\">{link}</w:hyperlink>\
             <w:del><w:r><w:delText>删掉的</w:delText></w:r></w:del>\
             <w:ins><w:r><w:t>新加的</w:t></w:r></w:ins>\
             <w:r><w:footnoteReference w:id=\"1\"/></w:r>\
             <w:r><w:fldChar w:fldCharType=\"begin\"/></w:r><w:r><w:instrText>PAGE</w:instrText></w:r>\
             <w:r><w:fldChar w:fldCharType=\"end\"/></w:r></w:p>",
            h1 = para(None, None, &run("名称", false, false)),
            h2 = para(None, None, &run("说明", false, false)),
            c = para(None, None, &run("合并 | 的", false, false)),
            link = run("链接", false, false),
        );
        let bytes = docx(&body, &[("word/_rels/document.xml.rels", rels.as_bytes())]);
        let converted = convert(&bytes, &mut |_, _| unreachable!()).unwrap();
        let md = &converted.markdown;
        assert!(md.contains("| 名称 | 说明 |"), "实际：{md}");
        assert!(md.contains("合并 \\| 的"), "实际：{md}");
        assert!(
            md.contains("[链接](https://example.com/a?b=1&c=2)"),
            "实际：{md}"
        );
        assert!(
            md.contains("新加的") && !md.contains("删掉的") && !md.contains("PAGE"),
            "实际：{md}"
        );
        assert_eq!(
            converted.warnings,
            vec![
                DocWarning::TableFlattened,
                DocWarning::TrackedChangesDropped,
                DocWarning::NotesDropped
            ]
        );
    }

    #[test]
    fn images_are_extracted_once_and_text_boxes_dropped() {
        let rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
            <Relationship Id="rId5" Type="image" Target="media/image1.png"/>
            <Relationship Id="rId6" Type="image" Target="media/image2.emf"/>
            </Relationships>"#;
        let drawing = |id: &str| {
            format!(
                "<w:r><w:drawing><wp:inline><wp:docPr id=\"1\" name=\"图\" descr=\"示意图\"/>\
                 <a:graphic><a:graphicData><a:blip r:embed=\"{id}\"/></a:graphicData></a:graphic>\
                 </wp:inline></w:drawing></w:r>"
            )
        };
        let body = format!(
            "<w:p>{}{}{}<w:r><mc:AlternateContent><mc:Choice><w:drawing><w:txbxContent><w:p><w:r><w:t>框里的字</w:t></w:r></w:p></w:txbxContent></w:drawing></mc:Choice>\
             <mc:Fallback><w:pict><w:txbxContent><w:p><w:r><w:t>框里的字</w:t></w:r></w:p></w:txbxContent></w:pict></mc:Fallback></mc:AlternateContent></w:r></w:p>",
            drawing("rId5"),
            drawing("rId5"),
            drawing("rId6"),
        );
        let bytes = docx(
            &body,
            &[
                ("word/_rels/document.xml.rels", rels.as_bytes()),
                ("word/media/image1.png", b"PNG"),
                ("word/media/image2.emf", b"EMF"),
            ],
        );
        let mut calls = Vec::new();
        let converted = convert(&bytes, &mut |i, ext| {
            calls.push((i, ext.to_owned()));
            format!("assets/图 {i}.{ext}")
        })
        .unwrap();
        assert_eq!(calls, vec![(0, "png".to_owned())]);
        assert_eq!(converted.images.len(), 1);
        assert_eq!(converted.images[0].bytes, b"PNG");
        assert_eq!(
            converted.markdown.matches("![示意图](").count(),
            2,
            "实际：{}",
            converted.markdown
        );
        assert!(!converted.markdown.contains("框里的字"));
        assert!(converted.warnings.contains(&DocWarning::TextBoxesDropped));
        assert!(converted.warnings.contains(&DocWarning::ImagesOmitted));
    }

    #[test]
    fn title_from_core_properties_and_corrupt_input() {
        let core = r#"<cp:coreProperties xmlns:cp="x" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>访谈提纲</dc:title></cp:coreProperties>"#;
        let bytes = docx(
            &para(None, None, &run("正文", false, false)),
            &[("docProps/core.xml", core.as_bytes())],
        );
        assert_eq!(
            convert(&bytes, &mut |_, _| unreachable!())
                .unwrap()
                .title
                .as_deref(),
            Some("访谈提纲")
        );
        assert!(matches!(
            convert(b"not a zip", &mut |_, _| unreachable!()),
            Err(DocImportError::Corrupt(_))
        ));
    }
}
