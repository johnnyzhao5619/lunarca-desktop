//! 文档导入（ADR-164）：用户手头的文档 → 库里一篇 Markdown 资料笔记的正文。
//!
//! 只做**字节 → Markdown** 的纯转换：不读写库、不联网、不调 AI。建笔记、落附件、写
//! frontmatter 与去重都在调用方（`luna-app`）。转不全的地方以 [`DocWarning`] 交回，由调用方
//! 按界面语言写进笔记顶部——这里不出任何用户可见文字。

mod docx;
mod escape;
mod pdf;
mod sheet;
mod table;
mod text;
mod web;

use std::path::Path;

pub use escape::tidy_escapes;
pub use table::{TABLE_MAX_COLS, TABLE_MAX_ROWS, TABLE_PREVIEW_ROWS};
pub use text::{decode_text, decode_with_charset};
pub use web::{extract_article, rewrite_images, strip_tracking, WebArticle};

/// 单个文件的体积上限（PRD-06-065）。
pub const MAX_DOCUMENT_BYTES: u64 = 100 * 1024 * 1024;

/// 认得的文档格式；**唯一判定**是 [`DocFormat::from_path`]（入口分流与转换共用）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocFormat {
    Text,
    Markdown,
    Csv,
    Tsv,
    Html,
    Docx,
    /// Excel 2007 起的工作簿（含宏、二进制工作簿）。
    Xlsx,
    /// Excel 97–2003 工作簿。
    Xls,
    /// OpenDocument 表格。
    Ods,
    /// Word 97–2003 文档：要先经系统转换成 HTML（[`DocFormat::needs_system_html`]）。
    Doc,
    Rtf,
    /// OpenDocument 文本。
    Odt,
    /// PDF（有文字层）。
    Pdf,
}

impl DocFormat {
    /// 全部取值与各自认的扩展名（小写，不带点）；文件选择框的过滤器也取自这里。
    pub const EXTENSIONS: [(Self, &'static [&'static str]); 13] = [
        (Self::Text, &["txt", "text"]),
        (Self::Markdown, &["md", "markdown"]),
        (Self::Csv, &["csv"]),
        (Self::Tsv, &["tsv", "tab"]),
        (Self::Html, &["html", "htm"]),
        (Self::Docx, &["docx"]),
        (Self::Xlsx, &["xlsx", "xlsm", "xlsb"]),
        (Self::Xls, &["xls"]),
        (Self::Ods, &["ods"]),
        (Self::Doc, &["doc"]),
        (Self::Rtf, &["rtf"]),
        (Self::Odt, &["odt"]),
        (Self::Pdf, &["pdf"]),
    ];

    /// 按扩展名认格式（大小写不敏感）；认不出 → `None`。
    #[must_use]
    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Self::EXTENSIONS
            .iter()
            .find(|(_, exts)| exts.contains(&ext.as_str()))
            .map(|(format, _)| *format)
    }

    /// 本平台能导入的扩展名（文件选择框过滤器用）：要系统转换的几种只在 macOS 上列出。
    #[must_use]
    pub fn all_extensions() -> Vec<&'static str> {
        Self::EXTENSIONS
            .iter()
            .filter(|(format, _)| format.available_here())
            .flat_map(|(_, exts)| exts.iter().copied())
            .collect()
    }

    /// 要先由系统（macOS `textutil`）转成 HTML 再走网页那一套的格式（ADR-164 ⑨）。
    #[must_use]
    pub fn needs_system_html(self) -> bool {
        matches!(self, Self::Doc | Self::Rtf | Self::Odt)
    }

    /// 本平台能不能导入它。
    #[must_use]
    pub fn available_here(self) -> bool {
        !self.needs_system_html() || cfg!(target_os = "macos")
    }

    /// 写进 `luna.original.format` 的键。
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Text => "txt",
            Self::Markdown => "md",
            Self::Csv => "csv",
            Self::Tsv => "tsv",
            Self::Html => "html",
            Self::Docx => "docx",
            Self::Xlsx => "xlsx",
            Self::Xls => "xls",
            Self::Ods => "ods",
            Self::Doc => "doc",
            Self::Rtf => "rtf",
            Self::Odt => "odt",
            Self::Pdf => "pdf",
        }
    }
}

/// 转换时没能带进来、或被改动了的东西（调用方按界面语言逐条写成一句话）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocWarning {
    /// 有字符解不出来，被替换成了 �（编码猜错或文件本身损坏）。
    UndecodableText,
    /// 远程图片或没有附带的图片只留了替代文字。
    ImagesOmitted,
    /// 合并单元格或嵌套表格被拆平。
    TableFlattened,
    /// 表格太大，只放了前几行 / 几列；`sheet` = 是哪个工作表（只有一张表时为 `None`）。
    TableTruncated {
        sheet: Option<String>,
        rows: usize,
        cols: usize,
        shown_rows: usize,
        shown_cols: usize,
    },
    /// Word 的批注。
    CommentsDropped,
    /// Word 的修订（删除的文字不带入，插入的文字照常保留）。
    TrackedChangesDropped,
    /// Word 的文本框。
    TextBoxesDropped,
    /// Word 的脚注与尾注。
    NotesDropped,
    /// PDF 平均每页文字很少：内容多半是图片（如从浏览器打印的网页只剩页眉页脚的字）。
    MostlyImages,
    /// PDF 里这几页（从 1 起）没有可提取的文字。
    PagesWithoutText { pages: Vec<usize> },
    /// PDF 从第 `from` 页起读不出来（共 `total` 页），后面的页没有带进来。
    PagesUnreadable { from: usize, total: usize },
}

/// 文档里的一张图片：调用方把它写进附件目录，`link` 是转换时它在正文里用的链接目标。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbeddedImage {
    pub link: String,
    pub bytes: Vec<u8>,
}

/// 一次转换的结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Converted {
    /// 文档自己带的标题（Word 的文档属性、网页的 `<title>`）；`None` = 没有。
    pub title: Option<String>,
    /// 正文 Markdown。Markdown 原文件原样交回，**含它自己的 frontmatter**。
    pub markdown: String,
    pub warnings: Vec<DocWarning>,
    pub images: Vec<EmbeddedImage>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DocImportError {
    /// 转换后什么文字都没有。
    #[error("DOC_EMPTY")]
    Empty,
    /// 文件坏了，或不是它扩展名说的那种格式。
    #[error("DOC_CORRUPT: {0}")]
    Corrupt(String),
    /// 这种格式要先经系统转换成 HTML（调用方转好后按 [`DocFormat::Html`] 交进来）。
    #[error("DOC_NEEDS_SYSTEM_HTML")]
    NeedsSystemHtml,
    /// PDF 有打开密码。
    #[error("DOC_ENCRYPTED")]
    Encrypted,
    /// PDF 几乎每页都没有文字（扫描件），要先识别文字。
    #[error("DOC_NO_TEXT_LAYER")]
    NoTextLayer,
}

/// 转换时要向调用方要的东西：只有调用方知道附件落在哪、界面是什么语言。
pub trait ConvertHost {
    /// 第 `index` 张内嵌图片（扩展名 `ext`）在正文里的链接目标（Word 用）。
    fn image_link(&mut self, index: usize, ext: &str) -> String;
    /// 第 `page` 页（从 1 起）前的那一行页标（PDF 用）。
    fn page_marker(&mut self, page: usize) -> String;
}

/// 只要图片链接的调用方（测试、非 PDF）可以直接传一个闭包；页标退回成不带语言的 `[N](#page=N)`。
impl<F: FnMut(usize, &str) -> String> ConvertHost for F {
    fn image_link(&mut self, index: usize, ext: &str) -> String {
        self(index, ext)
    }

    fn page_marker(&mut self, page: usize) -> String {
        format!("[{page}](#page={page})")
    }
}

/// 把一份文档的字节转成 Markdown；图片链接与页标经 `host` 向调用方要（[`ConvertHost`]）。
///
/// # Errors
/// 文件损坏、转换后为空、PDF 有密码或没有文字层时返回 [`DocImportError`]。
pub fn convert(
    format: DocFormat,
    bytes: &[u8],
    host: &mut dyn ConvertHost,
) -> Result<Converted, DocImportError> {
    let converted = match format {
        DocFormat::Text | DocFormat::Markdown => {
            let (text, lossy) = decode_text(bytes);
            Converted {
                markdown: text,
                warnings: lossy
                    .then_some(DocWarning::UndecodableText)
                    .into_iter()
                    .collect(),
                ..Converted::default()
            }
        }
        DocFormat::Csv | DocFormat::Tsv => {
            let (text, lossy) = decode_text(bytes);
            let delimiter = if format == DocFormat::Csv {
                b','
            } else {
                b'\t'
            };
            let (markdown, mut warnings) = table::delimited_to_markdown(&text, delimiter)?;
            if lossy {
                warnings.insert(0, DocWarning::UndecodableText);
            }
            Converted {
                markdown,
                warnings,
                ..Converted::default()
            }
        }
        DocFormat::Html => {
            let (text, lossy) = decode_text(bytes);
            let mut converted = html_to_markdown(&text, false);
            if lossy {
                converted.warnings.insert(0, DocWarning::UndecodableText);
            }
            converted
        }
        DocFormat::Docx => docx::convert(bytes, &mut |index, ext| host.image_link(index, ext))?,
        DocFormat::Pdf => pdf::convert(bytes, host)?,
        DocFormat::Xlsx | DocFormat::Xls | DocFormat::Ods => sheet::convert(bytes)?,
        DocFormat::Doc | DocFormat::Rtf | DocFormat::Odt => {
            return Err(DocImportError::NeedsSystemHtml);
        }
    };
    if converted.markdown.trim().is_empty() && converted.images.is_empty() {
        return Err(DocImportError::Empty);
    }
    Ok(converted)
}

/// HTML → Markdown，经粘贴网页同一套转换（ADR-164 ①）；顺带取 `<title>`。
/// 一段 HTML → Markdown（与导入 .html 同一套转换）；`keep_relative_images` 为真时保留已改写成
/// 相对附件路径的图片（网页剪藏下载图片之后用，ADR-164 ⑦）。
#[must_use]
pub fn convert_html_document(html: &str, keep_relative_images: bool) -> Converted {
    html_to_markdown(html, keep_relative_images)
}

fn html_to_markdown(html: &str, keep_relative_images: bool) -> Converted {
    let doc = dom_query::Document::from(html);
    let title = doc.select("head > title").text().trim().to_owned();
    // 空的粗斜体标签（系统转换常见）会序列化成一串 `****`。
    doc.select("b:empty, strong:empty, i:empty, em:empty, u:empty")
        .remove();
    let paste = luna_vault::html_paste::convert_html_with(&doc.html(), keep_relative_images);
    let mut warnings = Vec::new();
    for warning in &paste.warnings {
        let mapped = match warning {
            luna_vault::html_paste::PasteWarning::ImagesOmitted => DocWarning::ImagesOmitted,
            luna_vault::html_paste::PasteWarning::ComplexTable => DocWarning::TableFlattened,
            // 格式简化、无表头、不安全链接被去掉：对一篇资料而言不是值得逐条告知的损失。
            _ => continue,
        };
        push_warning(&mut warnings, mapped);
    }
    // 文档里的表几乎都把首行当表头，只是没用 <th>（粘贴时才需要保留「首行也是数据」那一版）。
    let markdown = paste.header_markdown.unwrap_or(paste.markdown);
    Converted {
        title: (!title.is_empty()).then_some(title),
        markdown: tidy_escapes(&markdown),
        warnings,
        images: Vec::new(),
    }
}

fn push_warning(warnings: &mut Vec<DocWarning>, warning: DocWarning) {
    if !warnings.contains(&warning) {
        warnings.push(warning);
    }
}

/// 拼出新笔记的正文（不含 `luna:` 块，由调用方 `update_luna` 写入）。
///
/// - `frontmatter`：Markdown 原文件自己的 frontmatter（含两条 `---`），原样放在最前；
/// - `heading`：一级标题；正文本来就以一级标题开头时不再加；
/// - `notice`：顶部说明（导入自哪个文件、哪些没带进来），每项一行，放进一个 `[!info]` 提示块。
#[must_use]
pub fn compose_note(
    frontmatter: Option<&str>,
    heading: Option<&str>,
    notice: &[String],
    body: &str,
) -> String {
    let mut out = String::new();
    if let Some(frontmatter) = frontmatter {
        out.push_str(frontmatter);
        if !frontmatter.ends_with('\n') {
            out.push('\n');
        }
    }
    let body = body.trim_matches('\n');
    if let Some(heading) = heading.filter(|_| !body.starts_with("# ")) {
        out.push_str("# ");
        out.push_str(heading.trim());
        out.push_str("\n\n");
    }
    if let Some((first, rest)) = notice.split_first() {
        out.push_str("> [!info] ");
        out.push_str(first);
        out.push('\n');
        for line in rest {
            out.push_str("> - ");
            out.push_str(line);
            out.push('\n');
        }
        out.push('\n');
    }
    out.push_str(body);
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_images(_: usize, _: &str) -> String {
        unreachable!("只有 Word 有内嵌图片")
    }

    #[test]
    fn formats_are_recognised_by_extension_case_insensitively() {
        assert_eq!(
            DocFormat::from_path(Path::new("a/B.DOCX")),
            Some(DocFormat::Docx)
        );
        assert_eq!(
            DocFormat::from_path(Path::new("x.htm")),
            Some(DocFormat::Html)
        );
        assert_eq!(
            DocFormat::from_path(Path::new("x.tab")),
            Some(DocFormat::Tsv)
        );
        assert_eq!(DocFormat::from_path(Path::new("x.mp4")), None);
        assert_eq!(DocFormat::from_path(Path::new("README")), None);
        assert!(DocFormat::all_extensions().contains(&"markdown"));
    }

    #[test]
    fn empty_documents_are_refused() {
        assert_eq!(
            convert(DocFormat::Text, b"  \n\n ", &mut no_images),
            Err(DocImportError::Empty)
        );
    }

    #[test]
    fn local_html_keeps_text_and_title_but_not_images() {
        let html = "<html><head><title> 访谈提纲 </title></head><body><h2>问题</h2>\
                    <p>第一问 <img src=\"a_files/x.png\" alt=\"示意图\"></p></body></html>";
        let converted = convert(DocFormat::Html, html.as_bytes(), &mut no_images).unwrap();
        assert_eq!(converted.title.as_deref(), Some("访谈提纲"));
        assert!(converted.markdown.contains("## 问题"));
        assert!(converted.markdown.contains("示意图"));
        assert!(!converted.markdown.contains("x.png"));
        assert_eq!(converted.warnings, vec![DocWarning::ImagesOmitted]);
    }

    #[test]
    fn html_tables_without_th_use_the_first_row_and_empty_bold_vanishes() {
        let html = "<p><b>标题</b><b></b></p><table><tr><td><b>环节</b></td><td>时长</td></tr>\
                    <tr><td>开场</td><td>2 分钟</td></tr></table>";
        let md = convert(DocFormat::Html, html.as_bytes(), &mut no_images)
            .unwrap()
            .markdown;
        assert_eq!(
            md,
            "**标题**\n\n| **环节** | 时长 |\n| --- | --- |\n| 开场 | 2 分钟 |"
        );
    }

    #[test]
    fn legacy_word_formats_ask_for_system_html() {
        for path in ["a.doc", "a.RTF", "a.odt"] {
            let format = DocFormat::from_path(Path::new(path)).unwrap();
            assert!(format.needs_system_html());
            assert_eq!(
                convert(format, b"x", &mut no_images),
                Err(DocImportError::NeedsSystemHtml)
            );
        }
        assert_eq!(
            DocFormat::all_extensions().contains(&"doc"),
            cfg!(target_os = "macos")
        );
        assert!(DocFormat::all_extensions().contains(&"xls"));
    }

    #[test]
    fn compose_puts_frontmatter_heading_notice_and_body_in_order() {
        let note = compose_note(
            None,
            Some("访谈"),
            &[
                "导入自 [a.docx](assets/a.docx)".into(),
                "批注没有带进来".into(),
            ],
            "\n正文\n",
        );
        assert_eq!(
            note,
            "# 访谈\n\n> [!info] 导入自 [a.docx](assets/a.docx)\n> - 批注没有带进来\n\n正文\n"
        );
        let md = compose_note(
            Some("---\ntags: [x]\n---\n"),
            Some("t"),
            &[],
            "# 自带标题\n",
        );
        assert_eq!(md, "---\ntags: [x]\n---\n# 自带标题\n");
    }
}
