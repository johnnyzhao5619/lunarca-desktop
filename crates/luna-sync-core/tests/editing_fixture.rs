//! 编辑面的互通夹具（ADR-177 ⑥、TDD-17 §2、PRD-21-020 / 030）：属性面板的读与就地改写、「查看对比」的
//! 行级差异。桌面经 IPC、网页经 `luna-web` 的 WASM 调的是同一份 `luna-sync-core`，这里把输入、线上 JSON
//! 与改出来的全文逐字节记下；`luna-web` 的 WASM 测试读同一份夹具核对，结果必须逐字节一致。
//!
//! 改了属性面板的改写规则或差异算法：`LUNA_REGEN_EDITING_FIXTURE=1 cargo test -p luna-sync-core --test
//! editing_fixture` 重新生成，并在同一次提交里说明为什么变。

use luna_sync_core::diff::diff_lines;
use luna_sync_core::frontmatter_edit::{
    apply_property_edit, read_properties, text_change, PropertyEdit,
};
use serde_json::{json, Value};

const FIXTURE_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/editing-interop-v1.json"
);

/// 一篇写法齐全的笔记：带引号的标量、日期、勾选、保留写法的数字、块列表与流式列表、注释、认不出的写法、
/// Lunarca 自有块；值里有中文与 emoji（UTF-16 下标与 UTF-8 字节下标不同，替换区间必须按 UTF-16 算）。
const NOTE: &str = "---\n# 我的注释\ntitle: '周会 🗓️'\ndue: 2026-10-04\ndone: false\nscore: 1.50\ntags:\n  - 工作\n  - \"重要\"\naliases: [例会]\nmeta:\n  owner: 我\nanchor: &a 值\nluna:\n  type: note\n---\n正文 🙂\n";

const CRLF: &str = "---\r\ntitle: 周报\r\ntags: [a, b]\r\n---\r\n正文\r\n";

fn property_inputs() -> Vec<&'static str> {
    vec![
        NOTE,
        CRLF,
        "没有 frontmatter 的笔记\n",
        "---\ntitle: 没闭合\n正文\n",
        "---\ntitle: a\ntitle: b\n---\n",
    ]
}

fn edit_inputs() -> Vec<(&'static str, Value)> {
    vec![
        (
            NOTE,
            json!({ "op": "set", "key": "title", "value": { "kind": "text", "value": "周会 · 第 2 次" } }),
        ),
        (
            NOTE,
            json!({ "op": "set", "key": "title", "value": { "kind": "text", "value": "a: b" } }),
        ),
        (
            NOTE,
            json!({ "op": "set", "key": "score", "value": { "kind": "number", "value": "2.00" } }),
        ),
        (
            NOTE,
            json!({ "op": "set", "key": "score", "value": { "kind": "number", "value": "两个" } }),
        ),
        (
            NOTE,
            json!({ "op": "set", "key": "done", "value": { "kind": "checkbox", "value": true } }),
        ),
        (
            NOTE,
            json!({ "op": "set", "key": "due", "value": { "kind": "date", "value": "2026-11-06" } }),
        ),
        (
            NOTE,
            json!({ "op": "set", "key": "tags", "value": { "kind": "list", "value": ["工作", "重要", "新"] } }),
        ),
        (
            NOTE,
            json!({ "op": "set", "key": "aliases", "value": { "kind": "list", "value": ["例会", "周例会"] } }),
        ),
        (
            NOTE,
            json!({ "op": "set", "key": "meta", "value": { "kind": "text", "value": "x" } }),
        ),
        (
            NOTE,
            json!({ "op": "set", "key": "missing", "value": { "kind": "text", "value": "x" } }),
        ),
        (
            NOTE,
            json!({ "op": "add", "key": "status", "value": { "kind": "text", "value": "" } }),
        ),
        (
            NOTE,
            json!({ "op": "add", "key": "title", "value": { "kind": "text", "value": "" } }),
        ),
        (
            NOTE,
            json!({ "op": "add", "key": "luna", "value": { "kind": "text", "value": "" } }),
        ),
        (NOTE, json!({ "op": "remove", "key": "done" })),
        (NOTE, json!({ "op": "remove", "key": "tags" })),
        (
            NOTE,
            json!({ "op": "rename", "from": "due", "to": "deadline" }),
        ),
        (NOTE, json!({ "op": "rename", "from": "due", "to": "done" })),
        (
            NOTE,
            json!({ "op": "retype", "key": "title", "kind": "list", "today": "2026-11-06", "now": "2026-11-06T09:30" }),
        ),
        (
            NOTE,
            json!({ "op": "retype", "key": "title", "kind": "date", "today": "2026-11-06", "now": "2026-11-06T09:30" }),
        ),
        (
            NOTE,
            json!({ "op": "retype", "key": "due", "kind": "date_time", "today": "2026-11-06", "now": "2026-11-06T09:30" }),
        ),
        (
            CRLF,
            json!({ "op": "set", "key": "title", "value": { "kind": "text", "value": "月报" } }),
        ),
        (
            CRLF,
            json!({ "op": "add", "key": "done", "value": { "kind": "checkbox", "value": false } }),
        ),
        (
            "正文\n",
            json!({ "op": "add", "key": "title", "value": { "kind": "text", "value": "新" } }),
        ),
        (
            "---\ntitle: 没闭合\n正文\n",
            json!({ "op": "set", "key": "title", "value": { "kind": "text", "value": "x" } }),
        ),
    ]
}

fn diff_inputs() -> Vec<(&'static str, &'static str)> {
    vec![
        ("a\nb\nc\n", "a\nB\nc\n"),
        ("一\n二\n", "一\n二\n三\n"),
        ("# 标题\n\n第一段\n第二段\n", "# 标题\n\n第二段\n"),
        ("", "新的一篇\n"),
        ("同样\n", "同样\n"),
        ("行一\r\n行二\r\n", "行一\n行二改\n"),
    ]
}

fn properties_case(content: &str) -> Value {
    match read_properties(content) {
        Ok(rows) => json!({ "content": content, "rows": rows }),
        Err(skip) => json!({ "content": content, "skip": skip.code() }),
    }
}

fn edit_case(content: &str, edit: &Value) -> Value {
    let parsed: PropertyEdit = serde_json::from_value(edit.clone()).expect("edit 形状");
    match apply_property_edit(content, &parsed) {
        Ok(text) => json!({
            "content": content,
            "edit": edit,
            "change": text_change(content, &text),
            "text": text,
        }),
        Err(skip) => json!({ "content": content, "edit": edit, "skip": skip.code() }),
    }
}

fn build() -> Value {
    json!({
        "properties": property_inputs().into_iter().map(properties_case).collect::<Vec<_>>(),
        "edits": edit_inputs().iter().map(|(content, edit)| edit_case(content, edit)).collect::<Vec<_>>(),
        "diffs": diff_inputs().into_iter().map(|(old, new)| json!({
            "old": old, "new": new, "diff": diff_lines(old, new),
        })).collect::<Vec<_>>(),
    })
}

/// UTF-16 下标的替换落在旧全文上，得到的就是改写后的全文（与编辑器应用一次变更同一口径）。
fn apply_utf16(old: &str, change: &Value) -> String {
    let units: Vec<u16> = old.encode_utf16().collect();
    let from = usize::try_from(change["from"].as_u64().unwrap()).unwrap();
    let to = usize::try_from(change["to"].as_u64().unwrap()).unwrap();
    let mut out: Vec<u16> = units[..from].to_vec();
    out.extend(change["insert"].as_str().unwrap().encode_utf16());
    out.extend_from_slice(&units[to..]);
    String::from_utf16(&out).unwrap()
}

#[test]
fn the_editing_fixture_matches_the_native_implementation() {
    let built = build();
    if std::env::var_os("LUNA_REGEN_EDITING_FIXTURE").is_some() {
        let text = serde_json::to_string_pretty(&built).unwrap() + "\n";
        std::fs::write(FIXTURE_PATH, text).unwrap();
        return;
    }
    let stored: Value =
        serde_json::from_str(&std::fs::read_to_string(FIXTURE_PATH).expect("夹具")).unwrap();
    assert_eq!(
        stored, built,
        "改写规则或差异算法变了：重新生成夹具并说明原因"
    );
}

#[test]
fn each_recorded_change_rebuilds_the_recorded_text() {
    let built = build();
    let edits = built["edits"].as_array().unwrap();
    assert!(edits.iter().any(|case| case.get("skip").is_some()));
    for case in edits.iter().filter(|case| case.get("change").is_some()) {
        let content = case["content"].as_str().unwrap();
        assert_eq!(
            apply_utf16(content, &case["change"]),
            case["text"].as_str().unwrap()
        );
        assert_ne!(
            case["text"].as_str().unwrap(),
            content,
            "夹具里的每一次改写都要真的改了字节"
        );
    }
}
