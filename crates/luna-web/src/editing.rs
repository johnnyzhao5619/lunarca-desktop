//! 编辑面用到的纯规则（PRD-21-020 / 030、TDD-17 §2）：属性面板的读与就地改写、「查看对比」的行级差异。
//! 与桌面调的是同一份 `luna-sync-core`（桌面经 IPC），线上 JSON 与桌面的 DTO 同形；两端逐字节一致由
//! 互通夹具钉住（`luna-sync-core/tests/fixtures/editing-interop-v1.json`）。这里不另写任何规则。

use luna_sync_core::diff;
use luna_sync_core::frontmatter_edit::{self, PropertyEdit};
use wasm_bindgen::prelude::*;

fn json<T: serde::Serialize>(value: &T) -> Result<String, JsError> {
    serde_json::to_string(value).map_err(|_| JsError::new("internal"))
}

/// 属性面板的各行（ADR-139）：JSON 数组，每行 `{key, value, locked, raw, line}`。
///
/// # Errors
/// 认不出整段 frontmatter 时抛那条原因的稳定码（如开头 `---` 没闭合 → `unclosed`）。
#[wasm_bindgen]
pub fn note_properties(content: &str) -> Result<String, JsError> {
    let rows =
        frontmatter_edit::read_properties(content).map_err(|skip| JsError::new(skip.code()))?;
    json(&rows)
}

/// 应用属性面板的一次编辑（`edit` 是 JSON，`{op: "set" | "add" | "remove" | "rename" | "retype", …}`），
/// 给出编辑器里要做的那一次最小替换 `{from, to, insert}`（UTF-16 下标）；没改时 `from == to` 且 `insert` 为空。
///
/// # Errors
/// 编辑做不了时抛原因的稳定码（`key_exists` / `not_found` / `invalid_value` …）；`edit` 形状不对抛 `invalid_edit`。
#[wasm_bindgen]
pub fn note_property_edit(content: &str, edit: &str) -> Result<String, JsError> {
    let edit: PropertyEdit =
        serde_json::from_str(edit).map_err(|_| JsError::new("invalid_edit"))?;
    let next = frontmatter_edit::apply_property_edit(content, &edit)
        .map_err(|skip| JsError::new(skip.code()))?;
    json(&frontmatter_edit::text_change(content, &next))
}

/// 旧 → 新的统一行差异（PRD-21-030「查看对比」）：`{lines: [{kind, old_line, new_line, text}], added,
/// removed, degraded}`。
///
/// # Errors
/// 不会失败（序列化出错时抛 `internal`）。
#[wasm_bindgen]
pub fn diff_lines(old: &str, new: &str) -> Result<String, JsError> {
    json(&diff::diff_lines(old, new))
}
