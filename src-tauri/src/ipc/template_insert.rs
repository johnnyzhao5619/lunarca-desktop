//! 把模板（库里的或装上的模板包里的，ADR-169 ⑤）插入到正在编辑的笔记里（PRD-06-015，笔记库对标轮 V6）。
//!
//! 渲染与新建时同一个渲染器（`luna_vault::render_user_template`）；这里只做插入专属的两步：
//! **只取模板正文**（模板自带的 frontmatter 不插入——塞进正文中间只会成为一段多余的 `---`
//! 文字；与目标笔记属性合并不在本轮），以及把 `{{cursor}}` 的字节偏移换成编辑器用的 UTF-16
//! 偏移。插入本身由编辑器在光标处做（它是一次普通编辑，可撤销）。
//!
//! 模板来自主库的模板目录或内容层里的模板包（`read_template_file`），与是哪一个窗口无关；笔记路径只用来
//! 取 `{{title}}`，不经它读写任何库，所以这条命令不按窗口换库。

use super::*;

/// 渲染好、等着插入的模板。
#[derive(Debug, Clone, Serialize, Type)]
pub struct TemplateInsertDto {
    pub text: String,
    /// `{{cursor}}` 在 `text` 里的位置（UTF-16 偏移，编辑器直接可用）；没写为 `null`。
    pub cursor: Option<u32>,
}

/// 渲染一份模板供插入。`note_path` 是正在编辑的那一篇（`{{title}}` 取它的文件名）。
#[tauri::command]
#[specta::specta]
pub(crate) fn render_template_for_insert(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    template: TemplateFileDto,
    note_path: Option<String>,
) -> Result<TemplateInsertDto, String> {
    let source = read_template_file(&app, &state, &template)?;
    let title = note_path
        .as_deref()
        .and_then(|path| path.rsplit('/').next())
        .map(|name| name.strip_suffix(".md").unwrap_or(name).to_owned())
        .unwrap_or_default();
    Ok(insertion_of(
        &source,
        &UserTemplateVars {
            title,
            now: chrono::Local::now().fixed_offset(),
        },
    ))
}

/// 只取模板正文渲染，并把光标的字节偏移换成 UTF-16 偏移（纯逻辑，可测）。
fn insertion_of(source: &str, vars: &UserTemplateVars) -> TemplateInsertDto {
    let rendered = luna_vault::render_user_template(luna_vault::body_of(source), vars);
    let cursor = rendered
        .cursor
        .map(|at| rendered.text[..at].encode_utf16().count() as u32);
    TemplateInsertDto {
        text: rendered.text,
        cursor,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_is_dropped_and_the_cursor_is_counted_in_utf16() {
        let vars = UserTemplateVars {
            title: "周会".into(),
            now: chrono::DateTime::parse_from_rfc3339("2026-10-05T09:00:00+08:00").unwrap(),
        };
        let out = insertion_of("---\ntags: [会]\n---\n## {{title}} 😀{{cursor}}\n", &vars);
        assert_eq!(out.text, "## 周会 😀\n");
        // 「## 周会 😀」：😀 在 UTF-16 里占两个单位。
        assert_eq!(out.cursor, Some(8));
    }
}
