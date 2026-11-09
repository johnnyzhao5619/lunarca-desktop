//! 文档导入（ADR-164）：按格式分流入队，在处理队列里转换，建 `document` 资料笔记，原文件进附件目录。
//!
//! 转换本身是 `luna_docimport` 的纯函数；这里管 IO、命名、去重、回滚与界面语言的提示。

use std::path::{Path, PathBuf};

use luna_core::cancel::TaskCancel;
use luna_docimport::{DocFormat, DocWarning};
use luna_vault::{NoteType, OriginalFile};
use sha2::{Digest as _, Sha256};
use tauri::Manager as _;

use super::backend_copy::{backend_copy_arc, backend_text_with, coded_error};
use super::notes::create_unique_note;
use super::notes::vault_err;
use super::processing::safe_media_stem;
use super::transcribe::{emit_processing_queue, start_processing_worker_if_idle};
use super::workspace::media_asset_dir;
use super::{emit_vault_changed, note_type_dir, AppState};
use crate::processing_queue::{ProcPhase, ProcTaskKind};

/// 把路径分成认得的文档与其余（其余交给媒体那一条）。判定只看 [`DocFormat::from_path`]。
pub(crate) fn split_documents(paths: Vec<String>) -> (Vec<PathBuf>, Vec<String>) {
    let mut documents = Vec::new();
    let mut rest = Vec::new();
    for path in paths {
        let candidate = PathBuf::from(&path);
        if DocFormat::from_path(&candidate).is_some() {
            documents.push(candidate);
        } else {
            rest.push(path);
        }
    }
    (documents, rest)
}

/// 打开「音视频或文档」多选对话框（项目概览「导入素材…」），交回选中的文件；**不入队**。
/// `[]` = 用户取消。不做转写引擎的早验证：只选文档时用不到它（入队时选了音视频才验）。
///
/// # Errors
/// 对话框异常关闭时返回错误。
#[tauri::command]
#[specta::specta]
pub(crate) async fn pick_material_files(app: tauri::AppHandle) -> Result<Vec<String>, String> {
    use tauri_plugin_dialog::DialogExt as _;
    let exts: Vec<&str> = super::playback::AUDIO_MEDIA_EXTS
        .iter()
        .chain(super::playback::VIDEO_MEDIA_EXTS.iter())
        .copied()
        .chain(DocFormat::all_extensions())
        .collect();
    let filter = backend_text_with(
        backend_copy_arc(&app).as_deref(),
        "import-filter-materials",
        "音视频与文档",
    );
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter(filter, &exts)
        .pick_files(move |paths| {
            let picked: Vec<String> = paths
                .unwrap_or_default()
                .into_iter()
                .filter_map(|p| p.into_path().ok())
                .filter(|p| p.is_file())
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
            let _ = tx.send(picked);
        });
    rx.await
        .map_err(|_| coded_error(&app, "DIALOG_CANCELLED", "dialog-cancelled"))
}

/// 「添加内容 ▸ 导入资料」（ADR-164 ⑩）：选好的文件按格式分流——文档建在文档文件夹，音视频照旧
/// 进转写文件夹。返回入队件数（文档 + 音视频）。
///
/// # Errors
/// 选了音视频而批量转写不可用时返回错误（文档已照常入队）。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn import_material_paths(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    paths: Vec<String>,
) -> Result<u32, String> {
    let (documents, media) = split_documents(paths);
    let documents = enqueue_document_imports(&app, &state, documents, None, None);
    let media = super::transcribe::enqueue_media_imports(&app, &state, media, None, None)?;
    Ok(documents + media)
}

/// 把文档逐个排进处理队列；`folder` = 笔记建在哪（`None` = 文档文件夹），`link_from` = 成功后
/// 链进哪篇研究笔记。返回入队件数。
pub(crate) fn enqueue_document_imports(
    app: &tauri::AppHandle,
    state: &AppState,
    documents: Vec<PathBuf>,
    folder: Option<String>,
    link_from: Option<String>,
) -> u32 {
    let mut count = 0;
    for path in documents.into_iter().filter(|p| p.is_file()) {
        let label = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("document")
            .to_owned();
        state.processing.enqueue(
            ProcTaskKind::ImportDocument {
                path,
                folder: folder.clone(),
                link_from: link_from.clone(),
            },
            label,
        );
        count += 1;
    }
    if count > 0 {
        emit_processing_queue(app);
        start_processing_worker_if_idle(app);
    }
    count
}

/// 读原文件失败时给人看的那句话。云盘（iCloud / OneDrive 等）里还没下载到本机的文件，读的时候
/// 由系统现场下载，慢或断网就报超时（真机 2026-10-19：`Operation timed out (os error 60)`）。
fn read_error(app: &tauri::AppHandle, error: &std::io::Error) -> String {
    eprintln!("[doc-import] 读原文件失败：{:?}", error.kind());
    if error.kind() == std::io::ErrorKind::TimedOut {
        coded_error(app, "DOC_READ_TIMEOUT", "doc-import-read-timeout")
    } else {
        coded_error(app, "DOC_READ_FAILED", "doc-import-read-failed")
    }
}

/// 已写进库的文件（失败时据此回滚）。
#[derive(Default)]
pub(crate) struct Written {
    pub(crate) note_rel: Option<String>,
    pub(crate) files: Vec<PathBuf>,
}

/// 处理一件文档导入：返回新笔记（或同一文件夹里已导入过的那一篇）的库相对路径。
pub(crate) async fn process_document_import(
    app: &tauri::AppHandle,
    path: PathBuf,
    folder: Option<String>,
    progress: &(dyn Fn(ProcPhase, u32, u32) + Sync),
    cancel: &TaskCancel,
) -> Result<Option<String>, String> {
    let format = DocFormat::from_path(&path)
        .ok_or_else(|| coded_error(app, "DOC_UNSUPPORTED", "doc-import-unsupported"))?;
    progress(ProcPhase::Converting, 0, 0);
    let size = std::fs::metadata(&path)
        .map_err(|e| read_error(app, &e))?
        .len();
    if size > luna_docimport::MAX_DOCUMENT_BYTES {
        return Err(coded_error(app, "DOC_TOO_LARGE", "doc-import-too-large"));
    }
    let read_path = path.clone();
    let bytes = tokio::task::spawn_blocking(move || std::fs::read(read_path))
        .await
        .map_err(|e| format!("DOC_TASK_PANIC: {e}"))?
        .map_err(|e| read_error(app, &e))?;
    cancel.bail_if_cancelled()?;
    let sha256 = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();

    let state = app.state::<AppState>();
    let settings = state.settings_snapshot();
    let folder = folder.unwrap_or_else(|| note_type_dir(&settings, NoteType::Document));
    let stem = safe_media_stem(
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("document"),
    );
    // 去重与建占位笔记在同一把锁里：两件同一文件的导入不会都建出笔记。
    let note_rel = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_owned())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        if let Some(existing) = vault.find_document(&folder, &sha256).map_err(vault_err)? {
            return Ok(Some(existing));
        }
        create_unique_note(
            state.copy(),
            vault,
            &format!("{folder}/{stem}.md"),
            NoteType::Document,
            format!("# {stem}\n\n"),
        )?
        .path
    };

    let mut written = Written {
        note_rel: Some(note_rel.clone()),
        files: Vec::new(),
    };
    let outcome = fill_document_note(
        app,
        &note_rel,
        &path,
        format,
        bytes,
        sha256,
        &settings,
        cancel.flag(),
        &mut written,
    )
    .await;
    match outcome {
        Ok(()) => {
            emit_vault_changed(app, &note_rel, false);
            // 资料在检索里降权不排除（ADR-024），同样要有向量。
            super::embed::spawn_embed_if_configured(app, &note_rel);
            Ok(Some(note_rel))
        }
        Err(err) => {
            rollback(app, written);
            Err(err)
        }
    }
}

/// 转换、落原文件与图片、写正文与 `luna.original`。
#[allow(clippy::too_many_arguments)]
async fn fill_document_note(
    app: &tauri::AppHandle,
    note_rel: &str,
    source: &Path,
    format: DocFormat,
    bytes: Vec<u8>,
    sha256: String,
    settings: &luna_config::Settings,
    cancel: luna_core::CancelToken,
    written: &mut Written,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    let root = state
        .vault
        .lock()
        .map_err(|_| "Vault 锁中毒".to_owned())?
        .as_ref()
        .map(|vault| vault.root().to_path_buf())
        .ok_or_else(|| state.vault_not_open())?;
    let note_stem = Path::new(note_rel)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("document")
        .to_owned();
    let asset_dir = media_asset_dir(settings, note_rel);
    let ext = source
        .extension()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_else(|| format.key().to_owned());
    let original_rel = unique_asset(&root, &asset_dir, &note_stem, &ext);

    // 转换在阻塞线程池（大 docx 的解压与解析是同步 CPU 活）。
    let (converted, image_rels, original_bytes) = {
        let note_rel = note_rel.to_owned();
        let asset_dir = asset_dir.clone();
        let root = root.clone();
        let note_stem = note_stem.clone();
        let source = source.to_path_buf();
        let original_link = luna_vault::encode_link_target(
            &luna_sync_core::links::relative_link_path(&note_rel, &original_rel),
        );
        let i18n = backend_copy_arc(app);
        tokio::task::spawn_blocking(move || {
            let mut host = NoteHost {
                root: &root,
                asset_dir: &asset_dir,
                note_rel: &note_rel,
                note_stem: &note_stem,
                original_link: &original_link,
                copy: i18n.as_deref(),
                image_rels: Vec::new(),
            };
            // .doc / .rtf / .odt：先由系统转成 HTML，再走网页那一套（ADR-164 ⑨）。
            let (format, input) = if format.needs_system_html() {
                match system_html(&source, &cancel) {
                    Ok(html) => (DocFormat::Html, std::borrow::Cow::Owned(html)),
                    Err(code) => return (Err(code), Vec::new(), bytes),
                }
            } else {
                (format, std::borrow::Cow::Borrowed(bytes.as_slice()))
            };
            let converted =
                luna_docimport::convert(format, &input, &mut host).map_err(|e| match e {
                    luna_docimport::DocImportError::Empty => "DOC_EMPTY",
                    luna_docimport::DocImportError::Encrypted => "DOC_ENCRYPTED",
                    luna_docimport::DocImportError::NoTextLayer => "DOC_NO_TEXT_LAYER",
                    luna_docimport::DocImportError::Corrupt(_)
                    | luna_docimport::DocImportError::NeedsSystemHtml => "DOC_CORRUPT",
                });
            drop(input);
            let image_rels = host.image_rels;
            (converted, image_rels, bytes)
        })
        .await
        .map_err(|e| format!("DOC_TASK_PANIC: {e}"))?
    };
    let converted = converted.map_err(|code| {
        let key = match code {
            "DOC_EMPTY" => "doc-import-empty",
            "DOC_NEEDS_MACOS" => "doc-import-needs-macos",
            "DOC_CONVERT_TIMEOUT" => "doc-import-timeout",
            "DOC_ENCRYPTED" => "doc-import-encrypted",
            "DOC_NO_TEXT_LAYER" => "doc-import-no-text-layer",
            luna_core::CANCELLED_CODE => return code.to_owned(),
            _ => "doc-import-corrupt",
        };
        coded_error(app, code, key)
    })?;

    // 原文件与图片落附件目录。
    let mut files: Vec<(String, Vec<u8>)> = vec![(original_rel.clone(), original_bytes)];
    for (rel, image) in image_rels.into_iter().zip(converted.images.iter()) {
        files.push((rel, image.bytes.clone()));
    }
    for (rel, bytes) in files {
        let abs = root.join(&rel);
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("MKDIR_FAILED: {e}"))?;
        }
        std::fs::write(&abs, bytes).map_err(|e| format!("DOC_WRITE_FAILED: {e}"))?;
        written.files.push(abs);
    }

    let i18n = backend_copy_arc(app);
    let copy = i18n.as_deref();
    let file_name = source
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&note_stem)
        .to_owned();
    let original_link = luna_vault::markdown_link(note_rel, &original_rel, &file_name);
    let mut notice = vec![copy
        .and_then(|i18n| {
            i18n.translate_args("doc-import-notice", &[("file", original_link.clone())])
        })
        .unwrap_or(original_link)];
    notice.extend(converted.warnings.iter().map(|w| warning_text(copy, w)));

    let (frontmatter, body, heading) = if format == DocFormat::Markdown {
        let body = luna_vault::body_of(&converted.markdown);
        let frontmatter = &converted.markdown[..converted.markdown.len() - body.len()];
        ((!frontmatter.is_empty()).then_some(frontmatter), body, None)
    } else {
        (None, converted.markdown.as_str(), Some(note_stem.as_str()))
    };
    let content = luna_docimport::compose_note(frontmatter, heading, &notice, body);

    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_owned())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    let mut meta = vault.read(note_rel).map_err(vault_err)?.meta;
    meta.note_type = NoteType::Document;
    meta.original = Some(OriginalFile {
        path: original_rel,
        sha256,
        format: format.key().to_owned(),
    });
    let content = luna_vault::update_luna(&content, &meta).map_err(vault_err)?;
    vault.write(note_rel, &content).map_err(vault_err)
}

/// `textutil` 最多跑这么久：畸形文件让它卡住时，任务不能一直挂着、连取消都取消不了。
#[cfg(target_os = "macos")]
const SYSTEM_HTML_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// 旧版 Word / RTF / ODT → HTML：macOS 系统自带的 `textutil`（不经 shell，路径只作参数）。
/// 超时或任务被取消就杀掉它。错误为稳定码：`DOC_NEEDS_MACOS`（别的平台）/ `DOC_CORRUPT`
/// （转换失败）/ `DOC_CONVERT_TIMEOUT` / [`luna_core::CANCELLED_CODE`]。
#[cfg(target_os = "macos")]
pub(crate) fn system_html(
    path: &Path,
    cancel: &luna_core::CancelToken,
) -> Result<Vec<u8>, &'static str> {
    use std::io::Read as _;
    use std::process::{Command, Stdio};
    let mut child = Command::new("/usr/bin/textutil")
        .args(["-convert", "html", "-encoding", "UTF-8", "-stdout"])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "DOC_CORRUPT")?;
    // 输出在另一条线程里读：管道写满时子进程会停下来等，而这边在等它退出——两边互等就是卡死。
    let mut stdout = child.stdout.take().ok_or("DOC_CORRUPT")?;
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        stdout.read_to_end(&mut out).map(|_| out)
    });
    let started = std::time::Instant::now();
    let outcome = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if cancel.is_cancelled() => break Err(luna_core::CANCELLED_CODE),
            Ok(None) if started.elapsed() > SYSTEM_HTML_TIMEOUT => {
                break Err("DOC_CONVERT_TIMEOUT")
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
            Err(_) => break Err("DOC_CORRUPT"),
        }
    };
    if outcome.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    let output = reader.join().ok().and_then(Result::ok).unwrap_or_default();
    match outcome {
        Ok(status) if status.success() && !output.is_empty() => Ok(output),
        Ok(_) => Err("DOC_CORRUPT"),
        Err(code) => Err(code),
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn system_html(
    _path: &Path,
    _cancel: &luna_core::CancelToken,
) -> Result<Vec<u8>, &'static str> {
    Err("DOC_NEEDS_MACOS")
}

/// 转换时向这篇笔记要图片落点与页标（[`luna_docimport::ConvertHost`]）。
struct NoteHost<'a> {
    root: &'a Path,
    asset_dir: &'a str,
    note_rel: &'a str,
    note_stem: &'a str,
    /// 原文件相对这篇笔记的链接目标（已编码）：页标指向它的 `#page=N`。
    original_link: &'a str,
    copy: super::backend_copy::BackendCopy<'a>,
    /// 按序号排好的图片落点（库相对路径）。
    image_rels: Vec<String>,
}

impl luna_docimport::ConvertHost for NoteHost<'_> {
    fn image_link(&mut self, index: usize, ext: &str) -> String {
        let rel = unique_asset(
            self.root,
            self.asset_dir,
            &format!("{}-{}", self.note_stem, index + 1),
            ext,
        );
        let link = luna_vault::encode_link_target(&luna_sync_core::links::relative_link_path(
            self.note_rel,
            &rel,
        ));
        self.image_rels.push(rel);
        link
    }

    fn page_marker(&mut self, page: usize) -> String {
        let label = self
            .copy
            .and_then(|i18n| i18n.translate_args("doc-import-page", &[("page", page.to_string())]))
            .unwrap_or_else(|| format!("第 {page} 页"));
        format!("[{label}]({}#page={page})", self.original_link)
    }
}

/// 附件目录里一个还没被占用的文件名 `<stem>.<ext>`（占用了就 `<stem>-2.<ext>`…）。
pub(crate) fn unique_asset(root: &Path, asset_dir: &str, stem: &str, ext: &str) -> String {
    let mut index = 1;
    loop {
        let name = if index == 1 {
            format!("{stem}.{ext}")
        } else {
            format!("{stem}-{index}.{ext}")
        };
        let rel = luna_sync_core::attachment::join_vault_rel(asset_dir, &name);
        if !root.join(&rel).exists() || index > 999 {
            return rel;
        }
        index += 1;
    }
}

pub(crate) fn warning_text(
    copy: super::backend_copy::BackendCopy<'_>,
    warning: &DocWarning,
) -> String {
    let (key, fallback) = match warning {
        DocWarning::UndecodableText => ("doc-import-undecodable", "有些字符解不出来，显示为 �。"),
        DocWarning::ImagesOmitted => (
            "doc-import-images-omitted",
            "有些图片没有带进来，只留了替代文字。",
        ),
        DocWarning::TableFlattened => (
            "doc-import-table-flattened",
            "合并单元格或嵌套表格被拆平了。",
        ),
        DocWarning::CommentsDropped => ("doc-import-comments", "批注没有带进来。"),
        DocWarning::TrackedChangesDropped => (
            "doc-import-tracked-changes",
            "修订没有带进来：删掉的文字不保留，新加的照常保留。",
        ),
        DocWarning::TextBoxesDropped => ("doc-import-text-boxes", "文本框里的文字没有带进来。"),
        DocWarning::NotesDropped => ("doc-import-notes", "脚注与尾注没有带进来。"),
        DocWarning::MostlyImages => (
            "doc-import-mostly-images",
            "这份 PDF 文字很少，内容可能主要是图片（比如从浏览器打印的网页）；图片里的文字没有带进来。",
        ),
        DocWarning::PagesWithoutText { pages } => {
            let separator = backend_text_with(copy, "doc-import-page-separator", "、");
            let pages = pages
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(&separator);
            return copy
                .and_then(|i18n| {
                    i18n.translate_args("doc-import-pages-without-text", &[("pages", pages)])
                })
                .unwrap_or_else(|| "有几页没有可提取的文字（可能是图片）。".to_owned());
        }
        DocWarning::PagesUnreadable { from, total } => {
            let args = [("from", from.to_string()), ("total", total.to_string())];
            return copy
                .and_then(|i18n| i18n.translate_args("doc-import-pages-unreadable", &args))
                .unwrap_or_else(|| "后面有几页读不出来，没有带进来。".to_owned());
        }
        DocWarning::TableTruncated {
            sheet,
            rows,
            cols,
            shown_rows,
            shown_cols,
        } => {
            let mut args = vec![
                ("rows", rows.to_string()),
                ("cols", cols.to_string()),
                ("shown_rows", shown_rows.to_string()),
                ("shown_cols", shown_cols.to_string()),
            ];
            let key = match sheet {
                Some(sheet) => {
                    args.push(("sheet", sheet.clone()));
                    "doc-import-sheet-truncated"
                }
                None => "doc-import-table-truncated",
            };
            return copy
                .and_then(|i18n| i18n.translate_args(key, &args))
                .unwrap_or_else(|| "表格太大，这里只放了一部分；完整数据在原文件里。".to_owned());
        }
    };
    backend_text_with(copy, key, fallback)
}

/// 失败即回滚：删掉占位笔记与已写下的附件，重试从干净状态开始（与媒体导入同一承诺）。
pub(crate) fn rollback(app: &tauri::AppHandle, written: Written) {
    for file in written.files {
        if let Err(e) = std::fs::remove_file(&file) {
            eprintln!("[doc-import] 回滚删除附件失败：{e}");
        }
    }
    let Some(note_rel) = written.note_rel else {
        return;
    };
    let state = app.state::<AppState>();
    let deleted = state.vault.lock().ok().and_then(|guard| {
        guard
            .as_ref()
            .map(|vault| vault.delete(&note_rel, luna_vault::DeleteMode::Permanent))
    });
    match deleted {
        Some(Ok(())) => emit_vault_changed(app, &note_rel, true),
        Some(Err(e)) => eprintln!("[doc-import] 回滚删除笔记失败：{e}"),
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_routes_documents_and_leaves_media_and_unknown() {
        let (docs, rest) = split_documents(vec![
            "/a/提纲.docx".into(),
            "/a/访谈.m4a".into(),
            "/a/数据.CSV".into(),
            "/a/x.pdf".into(),
            "/a/x.key".into(),
        ]);
        assert_eq!(
            docs,
            vec![
                PathBuf::from("/a/提纲.docx"),
                PathBuf::from("/a/数据.CSV"),
                PathBuf::from("/a/x.pdf")
            ]
        );
        assert_eq!(rest, vec!["/a/访谈.m4a".to_owned(), "/a/x.key".to_owned()]);
    }

    /// 真跑一次系统转换：RTF → HTML（只在 macOS 上）。
    #[cfg(target_os = "macos")]
    #[test]
    fn textutil_converts_rtf_to_html() {
        let dir = std::env::temp_dir().join(format!("doc-import-rtf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let rtf = dir.join("提纲.rtf");
        std::fs::write(&rtf, r"{\rtf1\ansi {\b Bold} plain\par}").unwrap();
        let html = system_html(&rtf, &luna_core::CancelToken::new()).unwrap();
        let html = String::from_utf8(html).unwrap();
        assert!(html.contains("Bold") && html.contains("plain"), "{html}");
        let missing = system_html(&dir.join("不存在.rtf"), &luna_core::CancelToken::new());
        assert_eq!(missing, Err("DOC_CORRUPT"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unique_asset_skips_taken_names() {
        let dir = std::env::temp_dir().join(format!("doc-import-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("P/assets")).unwrap();
        std::fs::write(dir.join("P/assets/提纲.docx"), b"x").unwrap();
        assert_eq!(
            unique_asset(&dir, "P/assets", "提纲", "docx"),
            "P/assets/提纲-2.docx"
        );
        assert_eq!(
            unique_asset(&dir, "P/assets", "提纲", "png"),
            "P/assets/提纲.png"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
