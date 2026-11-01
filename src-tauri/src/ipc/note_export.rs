//! 笔记富格式导出与附件打包命令域（PRD-11-010/011/012，TDD-10 §1.1/§1.2）。
//!
//! 分工（TDD-10 §4「纯函数 + IO 在调用方」）：
//! - **HTML 正文由前端渲染**（`ReadingView` 静态渲染 + 运行时从样式表收集 `.luna-markdown`
//!   规则与已解析的设计 Token）——这就是「导出的样子 = 编辑器里看到的样子」的唯一办法：
//!   仓库里只有这一份 Markdown→渲染实现（`components/markdown`），Rust 侧不另写解析器。
//! - **本层只做 IO 与不可信内容的收口**：把相对图片内联为 data URI（读 Vault 内文件，
//!   路径经 `resolve_note_relative_asset` 防穿越）、拼整页、弹保存对话框、写盘；附件打包
//!   则复制正文引用到的本地附件，保持笔记相对路径。
//! - **PDF = 打印窗口**：不引排版引擎——开一扇独立窗口用同一份阅读视图渲染，前端就绪后
//!   调 `print_current_window` 唤起系统打印面板，用户在那里「存储为 PDF」。

use super::*;

/// 打印窗口标签前缀（capability `note-editor.json` 以 `note-print-*` 授权关窗）。
const NOTE_PRINT_WINDOW_PREFIX: &str = "note-print-";

/// `<img src>` 里能内联的图片类型 → MIME。其余扩展名原样保留（不猜）。
fn image_mime(path: &Path) -> Option<&'static str> {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => Some("image/png"),
        Some("jpg" | "jpeg") => Some("image/jpeg"),
        Some("gif") => Some("image/gif"),
        Some("webp") => Some("image/webp"),
        Some("svg") => Some("image/svg+xml"),
        Some("bmp") => Some("image/bmp"),
        Some("avif") => Some("image/avif"),
        _ => None,
    }
}

/// 是否是「不该动」的 src：外链 / data / 协议 URL。
fn is_external_src(src: &str) -> bool {
    let lower = src.to_ascii_lowercase();
    lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("data:")
        || lower.starts_with("asset:")
        || lower.contains("://")
}

/// 读一张笔记相对图片：返回 `(MIME, 字节)`；读不到 / 不是图片返回 `None`。
pub(crate) trait AssetReader {
    fn read_asset(&self, src: &str) -> Option<(String, Vec<u8>)>;
}

impl<F: Fn(&str) -> Option<(String, Vec<u8>)>> AssetReader for F {
    fn read_asset(&self, src: &str) -> Option<(String, Vec<u8>)> {
        self(src)
    }
}

/// 把 HTML 里 `<img … src="相对路径" …>` 的相对图片内联为 data URI（纯函数；读文件经
/// `read`）。不解析整棵 DOM——只认 `<img` 标签内第一个 `src="…"`，其它属性原样保留。
/// 读不到 / 不是图片类型的保留原 src（导出物里的失链比一次失败的导出更诚实）。
pub(crate) fn inline_relative_images(html: &str, read: &dyn AssetReader) -> String {
    use base64::Engine as _;
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(tag_at) = rest.find("<img") {
        let (before, tag_start) = rest.split_at(tag_at);
        out.push_str(before);
        let Some(tag_len) = tag_start.find('>') else {
            out.push_str(tag_start);
            return out;
        };
        let tag = &tag_start[..=tag_len];
        let rewritten = match tag.find("src=\"") {
            Some(src_at) => {
                let value_start = src_at + "src=\"".len();
                match tag[value_start..].find('"') {
                    Some(value_len) => {
                        let src = &tag[value_start..value_start + value_len];
                        if is_external_src(src) {
                            tag.to_owned()
                        } else if let Some((mime, bytes)) = read.read_asset(src) {
                            let data = base64::engine::general_purpose::STANDARD.encode(bytes);
                            format!(
                                "{}src=\"data:{mime};base64,{data}\"{}",
                                &tag[..src_at],
                                &tag[value_start + value_len + 1..]
                            )
                        } else {
                            tag.to_owned()
                        }
                    }
                    None => tag.to_owned(),
                }
            }
            None => tag.to_owned(),
        };
        out.push_str(&rewritten);
        rest = &tag_start[tag_len + 1..];
    }
    out.push_str(rest);
    out
}

/// 拼整页：`<style>` 用前端收集来的规则 + 已解析 Token；正文放进与阅读视图同名的容器。
fn html_document(title: &str, lang: &str, styles: &str, body: &str) -> String {
    let escaped_title = title
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        "<!doctype html>\n<html lang=\"{lang}\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>{escaped_title}</title>\n<style>\n{styles}\n</style>\n</head>\n<body class=\"luna-export\">\n<main class=\"luna-export__page\">\n{body}\n</main>\n</body>\n</html>\n"
    )
}

/// 导出一篇笔记为**自包含 HTML**（PRD-11-011）：相对图片内联 base64；保存对话框在此。
/// 返回写入路径；用户取消返回 `None`。
#[tauri::command]
#[specta::specta]
pub(crate) async fn export_note_html(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    note_rel: String,
    body_html: String,
    styles: String,
    lang: String,
) -> Result<Option<String>, String> {
    let (document, stem) = {
        let vault_guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = vault_guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        // 笔记必须存在（挡路径穿越；正文来自前端渲染，不再读一遍）。
        vault.read(&note_rel).map_err(vault_err)?;
        let root = vault.root().to_path_buf();
        let note = note_rel.clone();
        let read = move |src: &str| -> Option<(String, Vec<u8>)> {
            let path = crate::media_server::resolve_note_relative_asset(&root, &note, src).ok()?;
            let mime = image_mime(&path)?;
            std::fs::read(&path)
                .ok()
                .map(|bytes| (mime.to_owned(), bytes))
        };
        let body = inline_relative_images(&body_html, &read);
        let title = super::playback::note_stem_of(&note_rel);
        let stem = super::playback::export_stem(&state.settings_snapshot(), &title, "html", "html");
        (html_document(&title, &lang, &styles, &body), stem)
    };
    let Some(dest) = super::playback::pick_export_destination(&app, &stem, "html").await? else {
        return Ok(None);
    };
    std::fs::write(&dest, document).map_err(|e| format!("EXPORT_WRITE_FAILED: {e}"))?;
    Ok(Some(dest.to_string_lossy().into_owned()))
}

/// 一篇笔记正文引用到的**本地**附件（笔记相对链接 → Vault 相对路径），去重保序。
/// 外链、wiki 链接目标（`.md`）不算附件。
pub(crate) fn local_attachment_refs(note_rel: &str, content: &str) -> Vec<(String, String)> {
    let dir = luna_vault::pathutil::parent_dir(note_rel);
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for link in luna_vault::extract_links(content) {
        if is_external_src(&link) || link.ends_with(".md") || link.starts_with('#') {
            continue;
        }
        let Some(vault_rel) = luna_vault::pathutil::normalize_vault_rel(dir, &link) else {
            continue;
        };
        if seen.insert(vault_rel.clone()) {
            out.push((link, vault_rel));
        }
    }
    out
}

/// 导出**笔记 + 附件**为一个文件夹（PRD-11-010 `BundleRelative`）：`<目录>/<笔记名>/<笔记名>.md`
/// 原样（含 frontmatter，Obsidian 可直接打开）+ 引用到的本地附件按**笔记相对路径**复制。
/// 返回复制的附件数；用户取消目录选择返回 `None`。
#[tauri::command]
#[specta::specta]
pub(crate) async fn export_note_bundle(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    note_rel: String,
) -> Result<Option<u32>, String> {
    let (content, refs, root, stem) = {
        let vault_guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = vault_guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        let doc = vault.read(&note_rel).map_err(vault_err)?;
        let refs = local_attachment_refs(&note_rel, &doc.content);
        (
            doc.content,
            refs,
            vault.root().to_path_buf(),
            super::playback::export_stem(
                &state.settings_snapshot(),
                &super::playback::note_stem_of(&note_rel),
                "md",
                "bundle",
            ),
        )
    };
    let Some(dir) = super::playback::pick_export_dir(&app).await? else {
        return Ok(None);
    };
    let bundle = dir.join(&stem);
    std::fs::create_dir_all(&bundle).map_err(|e| format!("EXPORT_WRITE_FAILED: {e}"))?;
    std::fs::write(bundle.join(format!("{stem}.md")), &content)
        .map_err(|e| format!("EXPORT_WRITE_FAILED: {e}"))?;
    let mut copied = 0u32;
    for (link, vault_rel) in refs {
        // 目标位置按**笔记相对链接**摆放（`assets/x.png` → `<bundle>/assets/x.png`），
        // 正文一字不改就能在任何 Markdown 工具里打开。越出 bundle 的相对链接（`../`）
        // 会被 `normalize_vault_rel` 归到库相对路径，此处按库相对路径落在 bundle 下。
        let target_rel = if link.starts_with("../") || link.starts_with('/') {
            vault_rel.clone()
        } else {
            link.clone()
        };
        let target = bundle.join(&target_rel);
        let source = root.join(&vault_rel);
        if !source.is_file() {
            continue;
        }
        if let Some(parent) = target.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::copy(&source, &target) {
            Ok(_) => copied += 1,
            Err(e) => eprintln!("[export] 复制附件失败（{}）：{e}", vault_rel),
        }
    }
    Ok(Some(copied))
}

/// 打开**打印窗口**（PDF 导出，PRD-11-011）：独立窗口以同一份阅读视图渲染这篇笔记，
/// 前端就绪后调 [`print_current_window`] 唤起系统打印面板（macOS 在那里「存储为 PDF」）。
#[tauri::command]
#[specta::specta]
pub(crate) fn open_note_print_window(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<(), String> {
    if !path.ends_with(".md") {
        return Err(coded_error(&app, "NOT_A_NOTE", "not-a-note"));
    }
    {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        crate::media_server::resolve_vault_path(vault.root(), &path).map_err(|_| {
            coded_error_args(
                &app,
                "NOTE_NOT_FOUND",
                "note-not-found",
                &[("path", path.to_string())],
            )
        })?;
    }
    let seq = state
        .note_window_seq
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let label = format!("{NOTE_PRINT_WINDOW_PREFIX}{seq}");
    // 与弹出编辑器共用「label → 笔记路径」表：打印窗口挂载时同样经 `note_window_path` 取路径。
    state
        .note_windows
        .lock()
        .map_err(|_| "窗口表锁中毒".to_string())?
        .insert(label.clone(), path.clone());
    crate::open_note_editor_window(&app, &label, &super::playback::note_stem_of(&path)).map_err(
        |e| {
            if let Ok(mut map) = state.note_windows.lock() {
                map.remove(&label);
            }
            e.to_string()
        },
    )
}

/// 唤起当前窗口的系统打印面板（打印窗口渲染就绪后调用）。
#[tauri::command]
#[specta::specta]
pub(crate) fn print_current_window(window: tauri::WebviewWindow) -> Result<(), String> {
    window.print().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_images_are_inlined_and_external_ones_untouched() {
        let html =
            r#"<p><img class="a" src="assets/x.png" alt="x"><img src="https://e.com/y.png"></p>"#;
        let read = |src: &str| -> Option<(String, Vec<u8>)> {
            (src == "assets/x.png").then(|| ("image/png".to_owned(), vec![0x89, 0x50]))
        };
        let out = inline_relative_images(html, &read);
        assert!(
            out.contains(r#"<img class="a" src="data:image/png;base64,iVA=" alt="x">"#),
            "{out}"
        );
        assert!(out.contains(r#"<img src="https://e.com/y.png">"#));
    }

    #[test]
    fn unreadable_images_keep_their_src() {
        let html = r#"<img src="missing.png">"#;
        let out = inline_relative_images(html, &|_: &str| None);
        assert_eq!(out, html);
    }

    #[test]
    fn attachment_refs_skip_links_notes_and_dedupe() {
        let content = "![a](assets/a.png) ![b](assets/a.png) [n](other.md) ![e](https://x/y.png) ![c](../shared/c.jpg)";
        let refs = local_attachment_refs("notes/n.md", content);
        assert_eq!(
            refs,
            vec![
                ("assets/a.png".to_owned(), "notes/assets/a.png".to_owned()),
                ("../shared/c.jpg".to_owned(), "shared/c.jpg".to_owned()),
            ]
        );
    }

    #[test]
    fn html_document_escapes_the_title() {
        let doc = html_document("a<b", "zh-CN", "p{}", "<p>x</p>");
        assert!(doc.contains("<title>a&lt;b</title>"));
        assert!(doc.contains("<html lang=\"zh-CN\">"));
    }
}
