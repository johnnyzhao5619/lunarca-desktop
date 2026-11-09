//! WASM 端读编辑面的互通夹具（`luna-sync-core/tests/fixtures/editing-interop-v1.json`），逐项核对属性面板的
//! 读与就地改写、行级差异与原生端逐字节一致（ADR-177 ⑥、TDD-17 §8，PRD-21-020 / 030）。在 Node 里跑：
//! `cargo test -p luna-web --target wasm32-unknown-unknown`。
#![cfg(all(target_arch = "wasm32", target_os = "unknown"))]

use luna_web::editing::{diff_lines, note_properties, note_property_edit};
use serde_json::Value;
use wasm_bindgen::JsCast as _;
use wasm_bindgen_test::wasm_bindgen_test;

const FIXTURE: &str = include_str!("../../luna-sync-core/tests/fixtures/editing-interop-v1.json");

fn fixture() -> Value {
    serde_json::from_str(FIXTURE).expect("fixture")
}

fn message(err: wasm_bindgen::JsError) -> String {
    let value: wasm_bindgen::JsValue = err.into();
    value
        .dyn_into::<js_sys::Error>()
        .map(|error| String::from(error.message()))
        .unwrap_or_default()
}

/// UTF-16 下标的替换落在旧全文上（与编辑器应用一次变更同一口径）。
fn apply_utf16(old: &str, change: &Value) -> String {
    let units: Vec<u16> = old.encode_utf16().collect();
    let from = usize::try_from(change["from"].as_u64().unwrap()).unwrap();
    let to = usize::try_from(change["to"].as_u64().unwrap()).unwrap();
    let mut out: Vec<u16> = units[..from].to_vec();
    out.extend(change["insert"].as_str().unwrap().encode_utf16());
    out.extend_from_slice(&units[to..]);
    String::from_utf16(&out).unwrap()
}

#[wasm_bindgen_test]
fn property_rows_match_the_native_implementation() {
    let f = fixture();
    for case in f["properties"].as_array().unwrap() {
        let content = case["content"].as_str().unwrap();
        match note_properties(content) {
            Ok(rows) => {
                let rows: Value = serde_json::from_str(&rows).unwrap();
                assert_eq!(rows, case["rows"]);
            }
            Err(err) => assert_eq!(message(err), case["skip"].as_str().unwrap()),
        }
    }
}

#[wasm_bindgen_test]
fn property_edits_write_the_same_bytes_as_the_desktop() {
    let f = fixture();
    for case in f["edits"].as_array().unwrap() {
        let content = case["content"].as_str().unwrap();
        let edit = case["edit"].to_string();
        match note_property_edit(content, &edit) {
            Ok(change) => {
                let change: Value = serde_json::from_str(&change).unwrap();
                assert_eq!(change, case["change"]);
                assert_eq!(
                    apply_utf16(content, &change),
                    case["text"].as_str().unwrap(),
                    "改出来的全文必须与桌面逐字节一致"
                );
            }
            Err(err) => assert_eq!(message(err), case["skip"].as_str().unwrap()),
        }
    }
}

#[wasm_bindgen_test]
fn a_malformed_edit_is_refused_with_a_stable_code() {
    let err = note_property_edit("---\ntitle: a\n---\n", "{\"op\":\"drop\"}").unwrap_err();
    assert_eq!(message(err), "invalid_edit");
}

#[wasm_bindgen_test]
fn line_diffs_match_the_native_implementation() {
    let f = fixture();
    for case in f["diffs"].as_array().unwrap() {
        let got: Value = serde_json::from_str(
            &diff_lines(case["old"].as_str().unwrap(), case["new"].as_str().unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(got, case["diff"]);
    }
}
