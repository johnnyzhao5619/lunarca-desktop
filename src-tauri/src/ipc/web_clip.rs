//! 网页剪藏（ADR-164 ⑦，PRD-19-008）：用户点了才发生的一次联网——抓一页、抽正文、存成项目「素材」
//! 里的文档资料笔记，整页 HTML 作快照，链进研究笔记「来源」一节。
//!
//! 护栏两道：严格本地模式（入队与开跑各查一次）与设备级开关 `privacy.web_clip`。只请求用户给的那个
//! 地址与正文里的图片，不带 cookie、不执行脚本；日志只出脱敏主机名。

use std::path::Path;
use std::time::Duration;

use luna_core::cancel::TaskCancel;
use luna_vault::{NoteType, OriginalFile, WebClip};
use sha2::{Digest as _, Sha256};
use tauri::Manager as _;

use super::backend_copy::{backend_copy_arc, coded_error, coded_error_args};
use super::doc_import::{rollback, unique_asset, warning_text, Written};
use super::notes::{create_unique_note, vault_err};
use super::processing::safe_media_stem;
use super::transcribe::{emit_processing_queue, start_processing_worker_if_idle};
use super::workspace::media_asset_dir;
use super::{emit_vault_changed, AppState};
use crate::notifications::{NotificationCenter, NotificationEvent};
use crate::processing_queue::{ProcPhase, ProcTaskKind};

/// 诚实标识（与抓取工具同一口径）：不伪装浏览器。
const USER_AGENT: &str = concat!(
    "Lunarca/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/johnnyzhao5619/lunarca-desktop)"
);
const PAGE_TIMEOUT: Duration = Duration::from_secs(30);
const IMAGE_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_REDIRECTS: usize = 5;
const MAX_PAGE_BYTES: usize = 10 * 1024 * 1024;
const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
const MAX_IMAGES: usize = 20;
/// 笔记文件名最多取标题的这么多个字。
const MAX_STEM_CHARS: usize = 60;

/// 一次剪藏的结果：新笔记（或同一文件夹里已剪过的那一篇）、规范链接与标题（写「来源」一节用）。
pub(crate) struct ClipDone {
    pub(crate) note_rel: String,
    pub(crate) url: String,
    pub(crate) title: String,
}

/// 两道联网护栏：严格本地优先，其次是「网页剪藏」开关。
fn network_guard(app: &tauri::AppHandle, state: &AppState) -> Result<(), String> {
    if state.strict_local() {
        if let Some(center) = app.try_state::<NotificationCenter>() {
            center.publish(NotificationEvent::strict_local_blocked(), app);
        }
        return Err(coded_error(
            app,
            "STRICT_LOCAL_BLOCKED",
            "strict-local-blocked-web-clip",
        ));
    }
    if !state
        .settings_snapshot()
        .get_bool("privacy.web_clip")
        .unwrap_or(true)
    {
        return Err(coded_error(app, "WEB_CLIP_DISABLED", "web-clip-disabled"));
    }
    Ok(())
}

/// 项目概览「剪藏网页…」：校验地址与护栏后排进处理队列。
///
/// # Errors
/// `home` 不是视频项目主页、地址不是 http(s)、严格本地模式开着或剪藏已关时返回错误。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn creator_clip_web(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    home: String,
    url: String,
) -> Result<(), String> {
    let url = luna_fetch::url::normalize(&url)
        .map_err(|_| coded_error(&app, "URL_REJECTED", "url-rejected"))?;
    network_guard(&app, &state)?;
    let (folder, research) = super::creator_quote::material_target(&app, &window, &state, &home)?;
    let host = luna_fetch::url::host_of(&url);
    let label = if host.is_empty() {
        "web".to_owned()
    } else {
        host
    };
    state.processing.enqueue(
        ProcTaskKind::ClipWeb {
            url,
            folder,
            link_from: Some(research),
        },
        label,
    );
    emit_processing_queue(&app);
    start_processing_worker_if_idle(&app);
    Ok(())
}

/// 处理一件剪藏。
pub(crate) async fn process_web_clip(
    app: &tauri::AppHandle,
    url: String,
    folder: String,
    progress: &(dyn Fn(ProcPhase, u32, u32) + Sync),
    cancel: &TaskCancel,
) -> Result<Option<ClipDone>, String> {
    // 入队之后才打开严格本地 / 关掉剪藏的，这里也要拦。
    network_guard(app, &app.state::<AppState>())?;
    progress(ProcPhase::Fetching, 0, 0);
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS))
        .build()
        .map_err(|e| format!("WEB_CLIP_FETCH_FAILED: {e}"))?;
    let (page_bytes, final_url) = fetch_page(app, &client, &url, cancel).await?;
    cancel.bail_if_cancelled()?;
    let html = decode_page(&page_bytes.1, &page_bytes.0);
    let extract_url = final_url.clone();
    let article =
        tokio::task::spawn_blocking(move || luna_docimport::extract_article(&html, &extract_url))
            .await
            .map_err(|e| format!("WEB_CLIP_TASK_PANIC: {e}"))?
            .map_err(|_| coded_error(app, "WEB_CLIP_NO_ARTICLE", "web-clip-no-article"))?;

    let state = app.state::<AppState>();
    let settings = state.settings_snapshot();
    let title = if article.title.trim().is_empty() {
        article
            .site
            .clone()
            .unwrap_or_else(|| article.canonical_url.clone())
    } else {
        article.title.trim().to_owned()
    };
    let stem: String = safe_media_stem(&title)
        .chars()
        .take(MAX_STEM_CHARS)
        .collect();
    let (note_rel, root) = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_owned())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        if let Some(existing) = vault
            .find_clip(&folder, &article.canonical_url)
            .map_err(vault_err)?
        {
            return Ok(Some(ClipDone {
                note_rel: existing,
                url: article.canonical_url,
                title,
            }));
        }
        let doc = create_unique_note(
            state.copy(),
            vault,
            &format!("{folder}/{stem}.md"),
            NoteType::Document,
            format!("# {stem}\n\n"),
        )?;
        (doc.path, vault.root().to_path_buf())
    };

    let mut written = Written {
        note_rel: Some(note_rel.clone()),
        files: Vec::new(),
    };
    let outcome = fill_clip_note(
        app,
        &client,
        &note_rel,
        &root,
        &settings,
        &title,
        &article,
        &page_bytes.0,
        progress,
        cancel,
        &mut written,
    )
    .await;
    match outcome {
        Ok(()) => {
            emit_vault_changed(app, &note_rel, false);
            super::embed::spawn_embed_if_configured(app, &note_rel);
            Ok(Some(ClipDone {
                note_rel,
                url: article.canonical_url,
                title,
            }))
        }
        Err(err) => {
            rollback(app, written);
            Err(err)
        }
    }
}

/// 抓页面：返回（字节, Content-Type）与跟完重定向后的地址。
async fn fetch_page(
    app: &tauri::AppHandle,
    client: &reqwest::Client,
    url: &str,
    cancel: &TaskCancel,
) -> Result<((Vec<u8>, String), String), String> {
    let response = client
        .get(url)
        .timeout(PAGE_TIMEOUT)
        .send()
        .await
        .map_err(|e| fetch_error(app, url, &e))?;
    let status = response.status();
    if !status.is_success() {
        return Err(coded_error_args(
            app,
            "WEB_CLIP_HTTP",
            "web-clip-http",
            &[("status", status.as_u16().to_string())],
        ));
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let mime = content_type.split(';').next().unwrap_or("").trim();
    if mime != "text/html" && mime != "application/xhtml+xml" {
        return Err(coded_error(app, "WEB_CLIP_NOT_HTML", "web-clip-not-html"));
    }
    let final_url = response.url().to_string();
    let bytes = read_limited(response, MAX_PAGE_BYTES, cancel)
        .await
        .map_err(|code| match code {
            ReadError::TooLarge => coded_error(app, "WEB_CLIP_TOO_LARGE", "web-clip-too-large"),
            ReadError::Cancelled => luna_core::CANCELLED_CODE.to_owned(),
            ReadError::Network(e) => fetch_error(app, url, &e),
        })?;
    Ok(((bytes, content_type), final_url))
}

fn fetch_error(app: &tauri::AppHandle, url: &str, error: &reqwest::Error) -> String {
    eprintln!(
        "[web-clip] 抓取失败：host={} timeout={} connect={}",
        luna_fetch::url::redact(url),
        error.is_timeout(),
        error.is_connect()
    );
    if error.is_timeout() {
        coded_error(app, "WEB_CLIP_TIMEOUT", "web-clip-timeout")
    } else {
        coded_error(app, "WEB_CLIP_FETCH_FAILED", "web-clip-fetch-failed")
    }
}

enum ReadError {
    TooLarge,
    Cancelled,
    Network(reqwest::Error),
}

/// 流式读正文，超过上限立即停（不信 Content-Length，边读边数）。
async fn read_limited(
    mut response: reqwest::Response,
    limit: usize,
    cancel: &TaskCancel,
) -> Result<Vec<u8>, ReadError> {
    if response
        .content_length()
        .is_some_and(|len| len > limit as u64)
    {
        return Err(ReadError::TooLarge);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(ReadError::Network)? {
        if cancel.is_cancelled() {
            return Err(ReadError::Cancelled);
        }
        if bytes.len() + chunk.len() > limit {
            return Err(ReadError::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// 按响应头的字符集解码；没有或认不出时走文本导入同一套猜测（BOM → UTF-8 → chardetng）。
fn decode_page(content_type: &str, bytes: &[u8]) -> String {
    let charset = content_type
        .split(';')
        .find_map(|part| part.trim().strip_prefix("charset="));
    luna_docimport::decode_with_charset(charset, bytes)
}

/// 下载图片、写快照、转换正文、写 frontmatter。
#[allow(clippy::too_many_arguments)]
async fn fill_clip_note(
    app: &tauri::AppHandle,
    client: &reqwest::Client,
    note_rel: &str,
    root: &Path,
    settings: &luna_config::Settings,
    title: &str,
    article: &luna_docimport::WebArticle,
    page: &[u8],
    progress: &(dyn Fn(ProcPhase, u32, u32) + Sync),
    cancel: &TaskCancel,
    written: &mut Written,
) -> Result<(), String> {
    let note_stem = Path::new(note_rel)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("clip")
        .to_owned();
    let asset_dir = media_asset_dir(settings, note_rel);

    // 整页快照：网页之后改了、删了，引文仍有原文可查。
    let snapshot_rel = unique_asset(root, &asset_dir, &note_stem, "html");
    write_file(root, &snapshot_rel, page, written)?;

    // 正文图片：按「本地化远程图片」那一项下载（ADR-164 ⑦）。
    let mut images: Vec<(String, String)> = Vec::new();
    if settings
        .get_bool("editor.localize_remote_images")
        .unwrap_or(true)
    {
        let total = article.images.len().min(MAX_IMAGES);
        for (index, url) in article.images.iter().take(MAX_IMAGES).enumerate() {
            cancel.bail_if_cancelled()?;
            progress(
                ProcPhase::Fetching,
                u32::try_from(index).unwrap_or(u32::MAX),
                u32::try_from(total).unwrap_or(u32::MAX),
            );
            let Some((bytes, ext)) = fetch_image(client, url, &article.canonical_url, cancel).await
            else {
                continue;
            };
            let rel = unique_asset(root, &asset_dir, &format!("{note_stem}-{}", index + 1), ext);
            write_file(root, &rel, &bytes, written)?;
            let link = luna_vault::encode_link_target(&luna_sync_core::links::relative_link_path(
                note_rel, &rel,
            ));
            images.push((url.clone(), link));
        }
    }
    progress(ProcPhase::Converting, 0, 0);
    let content = luna_docimport::rewrite_images(&article.content_html, &images);
    let converted = luna_docimport::convert_html_document(&content, true);

    let i18n = backend_copy_arc(app);
    let copy = i18n.as_deref();
    let site = article
        .site
        .clone()
        .unwrap_or_else(|| article.canonical_url.clone());
    let source = format!(
        "[{}]({})",
        site.replace(['[', ']'], ""),
        article.canonical_url
    );
    let translate = |key: &str, args: &[(&str, String)], fallback: String| {
        copy.and_then(|i18n| i18n.translate_args(key, args))
            .unwrap_or(fallback)
    };
    let mut notice = vec![translate(
        "web-clip-notice",
        &[("source", source.clone())],
        source,
    )];
    if let Some(author) = &article.author {
        notice.push(translate(
            "web-clip-author",
            &[("author", author.clone())],
            author.clone(),
        ));
    }
    if let Some(published) = &article.published {
        notice.push(translate(
            "web-clip-published",
            &[("published", published.clone())],
            published.clone(),
        ));
    }
    let snapshot_link =
        luna_vault::markdown_link(note_rel, &snapshot_rel, &format!("{note_stem}.html"));
    notice.push(translate(
        "web-clip-snapshot",
        &[("file", snapshot_link.clone())],
        snapshot_link,
    ));
    notice.extend(converted.warnings.iter().map(|w| warning_text(copy, w)));
    let body = luna_docimport::compose_note(None, Some(title), &notice, &converted.markdown);

    let state = app.state::<AppState>();
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_owned())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    let mut meta = vault.read(note_rel).map_err(vault_err)?.meta;
    meta.note_type = NoteType::Document;
    meta.source_url = Some(article.canonical_url.clone());
    meta.clip = Some(WebClip {
        author: article.author.clone(),
        published: article.published.clone(),
        site: article.site.clone(),
    });
    meta.original = Some(OriginalFile {
        path: snapshot_rel,
        sha256: Sha256::digest(page)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
        format: "html".to_owned(),
    });
    let content = luna_vault::update_luna(&body, &meta).map_err(vault_err)?;
    vault.write(note_rel, &content).map_err(vault_err)
}

/// 下载一张图（只收 `image/*`、不超过上限）；失败一律跳过（正文里只留替代文字）。
///
/// 带上被剪藏那一页作 Referer——浏览器打开那一页时本来就这么请求图片；不带的话，有防盗链的
/// 站点会回一张「图片无法显示」的占位图（真机 2026-10-19：新浪博客）。
async fn fetch_image(
    client: &reqwest::Client,
    url: &str,
    page_url: &str,
    cancel: &TaskCancel,
) -> Option<(Vec<u8>, &'static str)> {
    let response = client
        .get(url)
        .header(reqwest::header::REFERER, page_url)
        .timeout(IMAGE_TIMEOUT)
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let mime = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())?
        .split(';')
        .next()?
        .trim()
        .to_ascii_lowercase();
    let ext = match mime.as_str() {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/avif" => "avif",
        "image/svg+xml" => "svg",
        _ => return None,
    };
    let bytes = read_limited(response, MAX_IMAGE_BYTES, cancel).await.ok()?;
    (!bytes.is_empty()).then_some((bytes, ext))
}

fn write_file(root: &Path, rel: &str, bytes: &[u8], written: &mut Written) -> Result<(), String> {
    let abs = root.join(rel);
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("MKDIR_FAILED: {e}"))?;
    }
    std::fs::write(&abs, bytes).map_err(|e| format!("WEB_CLIP_WRITE_FAILED: {e}"))?;
    written.files.push(abs);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_charset_comes_from_the_header_then_falls_back_to_guessing() {
        // 「网页」的 GBK 编码。
        let gbk = [0xCD, 0xF8, 0xD2, 0xB3];
        assert_eq!(decode_page("text/html; charset=gbk", &gbk), "网页");
        assert_eq!(decode_page("text/html", "网页正文".as_bytes()), "网页正文");
    }

    #[test]
    fn user_agent_is_honest() {
        assert!(USER_AGENT.starts_with("Lunarca/"));
        assert!(!USER_AGENT.contains("Mozilla"));
    }
}
