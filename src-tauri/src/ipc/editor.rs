//! 编辑器结构转换与比较的 IPC 面；领域实现归 luna-vault。
use super::*;

#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[tauri_specta(event_name = "editor://flush-requested")]
pub(crate) struct EditorFlushRequested {
    pub request_id: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[tauri_specta(event_name = "editor://flush-cancelled")]
pub(crate) struct EditorFlushCancelled;

#[tauri::command]
#[specta::specta]
pub(crate) fn editor_flush_reply(
    app: tauri::AppHandle,
    window: tauri::Window,
    request_id: u32,
    saved: bool,
) {
    crate::editor_exit::reply(&app, window.label(), request_id, saved);
}

/// 「转为表格」：选中的多行文字按制表符 / 逗号 / 对齐空格分列成 GFM 表；看不出列返回 `None`。
#[tauri::command]
#[specta::specta]
pub(crate) fn editor_text_to_table(text: String) -> Result<Option<String>, String> {
    if text.len() > 8 * 1024 * 1024 {
        return Err("PASTE_TOO_LARGE".to_owned());
    }
    Ok(luna_vault::html_paste::text_to_table(&text))
}

#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct HtmlPasteDto {
    pub markdown: String,
    pub warnings: Vec<String>,
    pub block: bool,
    pub header_markdown: Option<String>,
    pub table_cells: Option<Vec<Vec<String>>>,
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn convert_clipboard_html(
    html: String,
    text: String,
) -> Result<HtmlPasteDto, String> {
    if html.len().max(text.len()) > 8 * 1024 * 1024 {
        return Err("PASTE_TOO_LARGE".to_owned());
    }
    tokio::task::spawn_blocking(move || convert_paste(html, text))
        .await
        .map_err(|_| "PASTE_CONVERSION_FAILED".to_owned())
}

fn convert_paste(html: String, text: String) -> HtmlPasteDto {
    let paste = if html.is_empty() {
        luna_vault::html_paste::convert_tsv(&text).unwrap_or(luna_vault::html_paste::HtmlPaste {
            markdown: text,
            warnings: Vec::new(),
            block: false,
            header_markdown: None,
            table_cells: None,
        })
    } else {
        luna_vault::html_paste::convert_html(&html)
    };
    HtmlPasteDto {
        markdown: paste.markdown,
        block: paste.block,
        header_markdown: paste.header_markdown,
        table_cells: paste.table_cells,
        warnings: paste
            .warnings
            .into_iter()
            .map(|warning| {
                match warning {
                    luna_vault::html_paste::PasteWarning::FormattingSimplified => {
                        "formatting_simplified"
                    }
                    luna_vault::html_paste::PasteWarning::ComplexTable => "complex_table",
                    luna_vault::html_paste::PasteWarning::MissingHeader => "missing_header",
                    luna_vault::html_paste::PasteWarning::ImagesOmitted => "images_omitted",
                    luna_vault::html_paste::PasteWarning::UnsafeLinks => "unsafe_links",
                }
                .to_owned()
            })
            .collect(),
    }
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn note_edit_diff(before: String, after: String) -> Result<LineDiffDto, String> {
    tokio::task::spawn_blocking(move || luna_sync_core::diff::diff_lines(&before, &after).into())
        .await
        .map_err(|_| "NOTE_DIFF_FAILED".to_owned())
}

/// 外部改动横幅「合并」的结果（ADR-148 ②、TDD-06 §4.2）。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum NoteMergeDto {
    /// 两边改动不重叠：把缓冲区变成合并结果的那一次最小替换（作为一次可撤销的编辑落下）。
    Merged { change: TextChangeDto },
    /// 有 `regions` 处两边改得不一样，没有结果。
    Conflict { regions: u32 },
    /// 改动范围超出比对预算，没有比。
    TooLarge,
}

/// 三路合并：基线 = 编辑器的已保存内容，我方 = 缓冲区，对方 = 磁盘上的外部版本。
/// 不碰文件——结果回到编辑器，保存仍走同一条带基线的保存护栏（ADR-138 ①）。
#[tauri::command]
#[specta::specta]
pub(crate) async fn note_merge(
    base: String,
    ours: String,
    theirs: String,
) -> Result<NoteMergeDto, String> {
    tokio::task::spawn_blocking(move || {
        match luna_sync_core::merge3::merge3(&base, &ours, &theirs) {
            luna_sync_core::merge3::MergeOutcome::Merged(text) => NoteMergeDto::Merged {
                change: luna_sync_core::frontmatter_edit::text_change(&ours, &text).into(),
            },
            luna_sync_core::merge3::MergeOutcome::Conflict { regions } => {
                NoteMergeDto::Conflict { regions }
            }
            luna_sync_core::merge3::MergeOutcome::TooLarge => NoteMergeDto::TooLarge,
        }
    })
    .await
    .map_err(|_| "NOTE_MERGE_FAILED".to_owned())
}

// ─── 笔记属性面板（ADR-139）──────────────────────────────────────────────────
//
// 读与写都是「当前正文 → 结果」的纯函数：前端传编辑器缓冲区的全文，拿回面板行或一次最小
// 替换，再作为编辑器里的一次变更应用——不碰文件，不与编辑器的保存路径抢写（ADR-138 ①）。

/// 属性为什么只读 / 编辑为什么没做。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PropertySkipDto {
    UnsupportedForm,
    InlineComment,
    MultiLine,
    Nested,
    DuplicateKey,
    Unclosed,
    KeyExists,
    InvalidKey,
    NotFound,
    InvalidValue,
}

impl From<luna_sync_core::frontmatter_edit::EditSkip> for PropertySkipDto {
    fn from(skip: luna_sync_core::frontmatter_edit::EditSkip) -> Self {
        use luna_sync_core::frontmatter_edit::EditSkip as S;
        match skip {
            S::UnsupportedForm => Self::UnsupportedForm,
            S::InlineComment => Self::InlineComment,
            S::MultiLine => Self::MultiLine,
            S::Nested => Self::Nested,
            S::DuplicateKey => Self::DuplicateKey,
            S::Unclosed => Self::Unclosed,
            S::KeyExists => Self::KeyExists,
            S::InvalidKey => Self::InvalidKey,
            S::NotFound => Self::NotFound,
            S::InvalidValue => Self::InvalidValue,
        }
    }
}

/// 属性值（按写法推断的类型）。
#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub(crate) enum PropertyValueDto {
    Text(String),
    Number(String),
    Checkbox(bool),
    Date(String),
    DateTime(String),
    List(Vec<String>),
}

impl From<luna_sync_core::frontmatter_edit::PropertyValue> for PropertyValueDto {
    fn from(value: luna_sync_core::frontmatter_edit::PropertyValue) -> Self {
        use luna_sync_core::frontmatter_edit::PropertyValue as V;
        match value {
            V::Text(v) => Self::Text(v),
            V::Number(v) => Self::Number(v),
            V::Checkbox(v) => Self::Checkbox(v),
            V::Date(v) => Self::Date(v),
            V::DateTime(v) => Self::DateTime(v),
            V::List(v) => Self::List(v),
        }
    }
}

impl From<PropertyValueDto> for luna_sync_core::frontmatter_edit::PropertyValue {
    fn from(value: PropertyValueDto) -> Self {
        match value {
            PropertyValueDto::Text(v) => Self::Text(v),
            PropertyValueDto::Number(v) => Self::Number(v),
            PropertyValueDto::Checkbox(v) => Self::Checkbox(v),
            PropertyValueDto::Date(v) => Self::Date(v),
            PropertyValueDto::DateTime(v) => Self::DateTime(v),
            PropertyValueDto::List(v) => Self::List(v),
        }
    }
}

/// 面板上的一行：能改时 `value` 有值；只读时 `locked` 说原因，界面照 `raw` 原文显示。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct NotePropertyDto {
    pub key: String,
    pub value: Option<PropertyValueDto>,
    pub locked: Option<PropertySkipDto>,
    pub raw: String,
    /// 键所在的行（1 起，按整个文件计）：「在源码中编辑」跳到这里。
    pub line: u32,
}

/// 属性类型（改类型时的目标）。
#[derive(Debug, Clone, Copy, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PropertyKindDto {
    Text,
    Number,
    Checkbox,
    Date,
    DateTime,
    List,
}

impl From<PropertyKindDto> for luna_sync_core::frontmatter_edit::PropertyKind {
    fn from(kind: PropertyKindDto) -> Self {
        match kind {
            PropertyKindDto::Text => Self::Text,
            PropertyKindDto::Number => Self::Number,
            PropertyKindDto::Checkbox => Self::Checkbox,
            PropertyKindDto::Date => Self::Date,
            PropertyKindDto::DateTime => Self::DateTime,
            PropertyKindDto::List => Self::List,
        }
    }
}

/// 面板的一次编辑。
#[derive(Debug, Clone, Deserialize, Type)]
#[serde(tag = "op", rename_all = "snake_case")]
pub(crate) enum NotePropertyEditDto {
    Set {
        key: String,
        value: PropertyValueDto,
    },
    Add {
        key: String,
        value: PropertyValueDto,
    },
    Remove {
        key: String,
    },
    Rename {
        from: String,
        to: String,
    },
    /// 改类型；`today` / `now` 是前端按本地时区给的（现值认不出日期时用）。
    Retype {
        key: String,
        kind: PropertyKindDto,
        today: String,
        now: String,
    },
}

/// 一次最小替换（UTF-16 下标，与编辑器位置口径一致）；`from == to` 且 `insert` 为空 = 没改。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct TextChangeDto {
    pub from: u32,
    pub to: u32,
    pub insert: String,
}

impl From<luna_sync_core::frontmatter_edit::TextChange> for TextChangeDto {
    fn from(change: luna_sync_core::frontmatter_edit::TextChange) -> Self {
        Self {
            from: u32::try_from(change.from).unwrap_or(u32::MAX),
            to: u32::try_from(change.to).unwrap_or(u32::MAX),
            insert: change.insert,
        }
    }
}

/// 读面板行。没有 frontmatter 为空列表；开头 `---` 没闭合时是 `Err(Unclosed)`。
#[tauri::command]
#[specta::specta]
pub(crate) fn note_properties(content: String) -> Result<Vec<NotePropertyDto>, PropertySkipDto> {
    let props = luna_sync_core::frontmatter_edit::read_properties(&content)?;
    Ok(props
        .into_iter()
        .map(|p| {
            let (value, locked) = match p.value {
                Ok(value) => (Some(value.into()), None),
                Err(skip) => (None, Some(skip.into())),
            };
            NotePropertyDto {
                key: p.key,
                value,
                locked,
                raw: p.raw,
                line: p.line,
            }
        })
        .collect())
}

/// 应用面板的一次编辑，给出要在编辑器里做的那一次替换。
#[tauri::command]
#[specta::specta]
pub(crate) fn note_property_edit(
    content: String,
    edit: NotePropertyEditDto,
) -> Result<TextChangeDto, PropertySkipDto> {
    use luna_sync_core::frontmatter_edit::{apply_property_edit, text_change, PropertyEdit};
    let edit = match edit {
        NotePropertyEditDto::Set { key, value } => PropertyEdit::Set {
            key,
            value: value.into(),
        },
        NotePropertyEditDto::Add { key, value } => PropertyEdit::Add {
            key,
            value: value.into(),
        },
        NotePropertyEditDto::Remove { key } => PropertyEdit::Remove { key },
        NotePropertyEditDto::Rename { from, to } => PropertyEdit::Rename { from, to },
        NotePropertyEditDto::Retype {
            key,
            kind,
            today,
            now,
        } => PropertyEdit::Retype {
            key,
            kind: kind.into(),
            today,
            now,
        },
    };
    let next = apply_property_edit(&content, &edit)?;
    Ok(text_change(&content, &next).into())
}
