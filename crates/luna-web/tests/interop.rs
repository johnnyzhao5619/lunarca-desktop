//! WASM 端读同一份互通夹具（`luna-sync-core/tests/fixtures/sync-interop-v1.json`），逐项核对与原生端
//! 结果逐字节一致（ADR-177 ⑥、TDD-17 §8）：加解密、分块、合并、路径口径、提交请求体与命名规则。在 Node 里跑：
//! `cargo test -p luna-web --target wasm32-unknown-unknown`（runner 见 `.cargo/config.toml`）。
#![cfg(all(target_arch = "wasm32", target_os = "unknown"))]

use base64::Engine as _;
use luna_web::{
    child_path, conflict_copy_path, content_hash, merge3, moved_path, name_key, new_file_id, nfc,
    renamed_path, vault_path_error, Civil, LoginStart, PreparedCommit, VaultCrypto, VaultSession,
};
use serde_json::Value;
use wasm_bindgen::JsCast as _;
use wasm_bindgen_test::wasm_bindgen_test;

const FIXTURE: &str = include_str!("../../luna-sync-core/tests/fixtures/sync-interop-v1.json");

fn fixture() -> Value {
    serde_json::from_str(FIXTURE).expect("fixture")
}

fn unb64(text: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(text)
        .expect("base64")
}

fn vault(f: &Value) -> VaultCrypto {
    VaultCrypto::new(
        f["vault_id"].as_str().unwrap(),
        u32::try_from(f["cmk_version"].as_u64().unwrap()).unwrap(),
        &unb64(f["cmk_b64"].as_str().unwrap()),
    )
    .unwrap_or_else(|_| panic!("vault crypto"))
}

#[wasm_bindgen_test]
fn chunks_and_entry_match_the_native_implementation() {
    let f = fixture();
    let crypto = vault(&f);
    let prepared = crypto
        .prepare(
            f["path"].as_str().unwrap(),
            f["plaintext"].as_str().unwrap().as_bytes(),
            Some(0.0),
        )
        .unwrap_or_else(|_| panic!("prepare"));
    let summary: Value = serde_json::from_str(&prepared.summary()).unwrap();
    assert_eq!(summary["entry"], f["entry"]);
    assert_eq!(summary["size_padded"], f["size_padded"]);
    let expected = f["chunks"].as_array().unwrap();
    assert_eq!(prepared.len(), expected.len());
    for (index, chunk) in expected.iter().enumerate() {
        assert_eq!(summary["chunks"][index]["id"], chunk["id"]);
        assert_eq!(summary["chunks"][index]["ct_sha256"], chunk["ct_sha256"]);
        let sealed = prepared.sealed(index).unwrap_or_else(|_| panic!("sealed"));
        assert_eq!(
            sealed,
            unb64(chunk["sealed_b64"].as_str().unwrap()),
            "块密文必须与原生端逐字节一致"
        );
    }
}

#[wasm_bindgen_test]
fn an_entry_sealed_natively_opens_and_assembles_here() {
    let f = fixture();
    let crypto = vault(&f);
    let entry = crypto
        .open_entry(
            f["file_id"].as_str().unwrap(),
            f["rev"].as_f64().unwrap(),
            &unb64(f["entry_sealed_b64"].as_str().unwrap()),
        )
        .unwrap_or_else(|_| panic!("open entry"));
    assert_eq!(serde_json::from_str::<Value>(&entry).unwrap(), f["entry"]);
    let (ids, chunks): (Vec<String>, Vec<js_sys::Uint8Array>) = f["chunks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            let bytes = unb64(c["sealed_b64"].as_str().unwrap());
            (
                c["id"].as_str().unwrap().to_owned(),
                js_sys::Uint8Array::from(bytes.as_slice()),
            )
        })
        .unzip();
    let bytes = crypto
        .assemble(&entry, ids, chunks)
        .unwrap_or_else(|_| panic!("assemble"));
    assert_eq!(bytes, f["plaintext"].as_str().unwrap().as_bytes());
    assert_eq!(
        content_hash(&bytes),
        f["entry"]["content_hash"].as_str().unwrap()
    );
}

#[wasm_bindgen_test]
fn an_entry_sealed_here_opens_again() {
    let f = fixture();
    let crypto = vault(&f);
    let entry = f["entry"].to_string();
    let sealed = crypto
        .seal_entry(f["file_id"].as_str().unwrap(), 4.0, &entry)
        .unwrap_or_else(|_| panic!("seal"));
    let opened = crypto
        .open_entry(f["file_id"].as_str().unwrap(), 4.0, &sealed)
        .unwrap_or_else(|_| panic!("open"));
    assert_eq!(serde_json::from_str::<Value>(&opened).unwrap(), f["entry"]);
    assert!(
        crypto
            .open_entry(f["file_id"].as_str().unwrap(), 3.0, &sealed)
            .is_err(),
        "条目绑 rev：拿旧版本冒充新版本必须打不开"
    );
}

#[wasm_bindgen_test]
fn merge_and_path_rules_match_the_native_implementation() {
    let f = fixture();
    for case in f["merge"].as_array().unwrap() {
        let got: Value = serde_json::from_str(&merge3(
            case["base"].as_str().unwrap(),
            case["ours"].as_str().unwrap(),
            case["theirs"].as_str().unwrap(),
        ))
        .unwrap();
        assert_eq!(got, case["outcome"]);
    }
    for case in f["paths"].as_array().unwrap() {
        let raw = case["raw"].as_str().unwrap();
        assert_eq!(nfc(raw), case["nfc"].as_str().unwrap());
        assert_eq!(name_key(raw), case["name_key"].as_str().unwrap());
    }
}

/// 服务端信封响应的形状（TDD-16 §9 `GET …/envelopes`）：托管槽之外还有一枚恢复槽。
fn envelopes_response(f: &Value) -> Vec<u8> {
    serde_json::json!({ "envelopes": [
        { "slot": "recovery", "cmk_version": 1, "wrapped_cmk": "AAAA", "kdf_params": {},
          "created_by_device": f["file_id"], "created_at": "2026-11-06T00:00:00Z" },
        { "slot": "managed", "cmk_version": f["cmk_version"], "wrapped_cmk": f["cmk_b64"],
          "kdf_params": {}, "created_by_device": f["file_id"], "created_at": "2026-11-06T00:00:00Z" },
    ]})
    .to_string()
    .into_bytes()
}

#[wasm_bindgen_test]
fn the_managed_envelope_opens_entries_from_a_manifest_response() {
    let f = fixture();
    let crypto =
        VaultCrypto::from_envelopes(f["vault_id"].as_str().unwrap(), &envelopes_response(&f))
            .unwrap_or_else(|_| panic!("from envelopes"));
    let manifest = serde_json::json!({
        "epoch": "0190f0a0-0000-7000-8000-0000000000ee",
        "at_seq": 9,
        "entries": [
            { "file_id": f["file_id"], "rev": f["rev"], "seq": 9, "deleted": false,
              "entry_ct": f["entry_sealed_b64"], "size_padded": f["size_padded"],
              "device_id": f["file_id"], "committed_at": "2026-11-06T00:00:00Z" },
            // 同一份密文挂到别的 rev 上：绑定不符，必须标成解不开而不是整批失败。
            { "file_id": f["file_id"], "rev": 99, "seq": 8, "deleted": false,
              "entry_ct": f["entry_sealed_b64"], "size_padded": 1,
              "device_id": f["file_id"], "committed_at": "2026-11-06T00:00:00Z" },
            // 墓碑：没有条目密文。
            { "file_id": f["file_id"], "rev": 4, "seq": 7, "deleted": true,
              "size_padded": 0, "device_id": f["file_id"], "committed_at": "2026-11-06T00:00:00Z" },
        ],
    });
    let opened: Value = serde_json::from_str(
        &crypto
            .open_entries(manifest.to_string().as_bytes())
            .unwrap_or_else(|_| panic!("open entries")),
    )
    .unwrap();
    assert_eq!(opened["at_seq"], 9, "顶层字段原样保留");
    let entries = opened["entries"].as_array().unwrap();
    assert_eq!(entries[0]["entry"], f["entry"]);
    assert!(entries[0].get("entry_ct").is_none(), "密文不回给调用方");
    assert!(entries[0].get("unreadable").is_none());
    assert_eq!(entries[1]["entry"], Value::Null);
    assert_eq!(entries[1]["unreadable"], true);
    assert_eq!(entries[2]["entry"], Value::Null);
    assert!(entries[2].get("unreadable").is_none(), "墓碑不算解不开");
}

#[wasm_bindgen_test]
fn a_deleted_entry_opens_under_its_last_rev() {
    let f = fixture();
    let crypto = vault(&f);
    let deleted = serde_json::json!({
        "entries": [{ "file_id": f["file_id"], "rev": 4, "seq": 10, "last_rev": f["rev"],
            "entry_ct": f["entry_sealed_b64"], "deleted_at": "2026-11-06T00:00:00Z",
            "device_id": f["file_id"] }],
    });
    let opened: Value = serde_json::from_str(
        &crypto
            .open_entries(deleted.to_string().as_bytes())
            .unwrap_or_else(|_| panic!("open deleted")),
    )
    .unwrap();
    assert_eq!(opened["entries"][0]["entry"], f["entry"]);
    assert!(opened.get("next_cursor").is_none());
}

#[wasm_bindgen_test]
fn a_vault_without_a_managed_envelope_is_not_open_on_the_web() {
    let f = fixture();
    let only_recovery = serde_json::json!({ "envelopes": [
        { "slot": "recovery", "cmk_version": 1, "wrapped_cmk": "AAAA", "kdf_params": {},
          "created_by_device": f["file_id"], "created_at": "2026-11-06T00:00:00Z" },
    ]});
    let err = VaultCrypto::from_envelopes(
        f["vault_id"].as_str().unwrap(),
        only_recovery.to_string().as_bytes(),
    )
    .err()
    .expect("must fail");
    assert_eq!(
        wasm_bindgen::JsValue::from(err)
            .dyn_into::<js_sys::Error>()
            .unwrap()
            .message(),
        "no_managed_key"
    );
    assert!(VaultCrypto::from_envelopes(f["vault_id"].as_str().unwrap(), b"not json").is_err());
}

#[wasm_bindgen_test]
fn new_file_ids_are_v7_and_distinct() {
    let (a, b) = (new_file_id(), new_file_id());
    assert_ne!(a, b);
    assert_eq!(uuid::Uuid::parse_str(&a).unwrap().get_version_num(), 7);
}

#[wasm_bindgen_test]
fn an_opaque_login_starts_in_the_browser() {
    let start = LoginStart::new("correct horse").unwrap_or_else(|_| panic!("start"));
    assert!(!start.request().is_empty());
}

fn civil(f: &Value) -> Civil {
    let at = &f["at"];
    let n = |key: &str| u32::try_from(at[key].as_u64().unwrap()).unwrap();
    Civil::new(
        i32::try_from(at["year"].as_i64().unwrap()).unwrap(),
        n("month"),
        n("day"),
        n("hour"),
        n("minute"),
        n("second"),
    )
}

fn message(err: wasm_bindgen::JsError) -> String {
    wasm_bindgen::JsValue::from(err)
        .dyn_into::<js_sys::Error>()
        .unwrap()
        .message()
        .into()
}

/// 一个装着这些路径（各一个文件，rev 1）的已打开库。
fn session_with(f: &Value, paths: &[&str]) -> VaultSession {
    let crypto = vault(f);
    let entries: Vec<Value> = paths
        .iter()
        .map(|path| {
            let prepared = crypto
                .prepare(path, b"x", None)
                .unwrap_or_else(|_| panic!("prepare"));
            let summary: Value = serde_json::from_str(&prepared.summary()).unwrap();
            let file_id = new_file_id();
            let sealed = crypto
                .seal_entry(&file_id, 1.0, &summary["entry"].to_string())
                .unwrap_or_else(|_| panic!("seal"));
            serde_json::json!({ "file_id": file_id, "rev": 1, "seq": 1, "deleted": false,
                "entry_ct": base64::engine::general_purpose::STANDARD.encode(sealed),
                "size_padded": summary["size_padded"], "device_id": file_id,
                "committed_at": "2026-11-06T00:00:00Z" })
        })
        .collect();
    let mut session = VaultSession::open(f["vault_id"].as_str().unwrap(), &envelopes_response(f))
        .unwrap_or_else(|_| panic!("open session"));
    session
        .load_manifest(
            serde_json::json!({ "epoch": "0190f0a0-0000-7000-8000-0000000000ee", "at_seq": 1,
                "entries": entries })
            .to_string()
            .as_bytes(),
        )
        .unwrap_or_else(|_| panic!("manifest"));
    session
}

#[wasm_bindgen_test]
fn naming_rules_match_the_native_implementation() {
    let f = fixture();
    for case in f["conflict_copies"].as_array().unwrap() {
        assert_eq!(
            conflict_copy_path(
                case["path"].as_str().unwrap(),
                case["label"].as_str().unwrap(),
                case["date"].as_str().unwrap(),
                u32::try_from(case["attempt"].as_u64().unwrap()).unwrap(),
            ),
            case["copy"].as_str().unwrap()
        );
    }
    for case in f["vault_paths"].as_array().unwrap() {
        assert_eq!(
            vault_path_error(case["path"].as_str().unwrap()).as_deref(),
            case["error"].as_str()
        );
    }
    let at = civil(&f);
    assert_eq!(at.date(), f["at"]["date"].as_str().unwrap());
    assert_eq!(at.stamp(), f["at"]["stamp"].as_str().unwrap());
    for case in f["new_notes"].as_array().unwrap() {
        let taken: Vec<&str> = case["taken"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p.as_str().unwrap())
            .collect();
        let session = session_with(&f, &taken);
        let path = session
            .new_note_path(
                case["dir"].as_str().unwrap(),
                case["text"].as_str().unwrap(),
                case["prefix"].as_str().unwrap(),
                &at,
            )
            .unwrap_or_else(|_| panic!("new note path"));
        assert_eq!(path, case["path"].as_str().unwrap());
    }
}

#[wasm_bindgen_test]
fn the_conflict_copy_path_skips_names_the_vault_already_has() {
    let f = fixture();
    let first = f["conflict_copies"][0]["copy"]
        .as_str()
        .unwrap()
        .to_uppercase();
    let session = session_with(&f, &[&first]);
    assert_eq!(
        session
            .conflict_copy_path("周报/2026-11.md", "冲突 网页", "2026-11-06")
            .unwrap_or_else(|_| panic!("copy path")),
        f["conflict_copies"][1]["copy"].as_str().unwrap()
    );
}

#[wasm_bindgen_test]
fn commit_bodies_match_the_native_implementation() {
    let f = fixture();
    let session = session_with(&f, &[]);
    let file_id = f["file_id"].as_str().unwrap();
    let base = f["commit"]["base_rev"].as_f64().unwrap();
    let put = session
        .prepare_put(
            file_id,
            Some(base),
            f["path"].as_str().unwrap(),
            f["plaintext"].as_str().unwrap(),
            Some(0.0),
        )
        .unwrap_or_else(|_| panic!("prepare put"));
    let mut body: Value = serde_json::from_str(&put.body()).unwrap();
    let ct = body["items"][0]["change"]
        .as_object_mut()
        .unwrap()
        .remove("entry_ct")
        .unwrap();
    assert_eq!(body, f["commit"]["put_without_entry_ct"]);
    let opened = vault(&f)
        .open_entry(file_id, base + 1.0, &unb64(ct.as_str().unwrap()))
        .unwrap_or_else(|_| panic!("entry opens on base + 1"));
    assert_eq!(serde_json::from_str::<Value>(&opened).unwrap(), f["entry"]);
    let expected = f["chunks"].as_array().unwrap();
    assert_eq!(put.chunk_ids().len(), expected.len());
    for (index, chunk) in expected.iter().enumerate() {
        assert_eq!(put.chunk_ids()[index], chunk["id"].as_str().unwrap());
        assert_eq!(
            put.ct_sha256(index).unwrap_or_else(|_| panic!("sha")),
            chunk["ct_sha256"].as_str().unwrap()
        );
        assert_eq!(
            put.sealed(index).unwrap_or_else(|_| panic!("sealed")),
            unb64(chunk["sealed_b64"].as_str().unwrap())
        );
    }
    let delete = session
        .prepare_delete(file_id, base)
        .unwrap_or_else(|_| panic!("prepare delete"));
    assert_eq!(
        serde_json::from_str::<Value>(&delete.body()).unwrap(),
        f["commit"]["delete"]
    );
    assert!(delete.chunk_ids().is_empty());
}

#[wasm_bindgen_test]
fn writes_refuse_bad_and_taken_paths_and_renames_keep_the_chunks() {
    let f = fixture();
    let mut session = session_with(&f, &["工作/周报.md"]);
    let new_id = new_file_id();
    let err =
        |r: Result<PreparedCommit, wasm_bindgen::JsError>| message(r.err().expect("must fail"));
    assert_eq!(
        err(session.prepare_put(&new_id, None, "工作/周报.MD", "x", None)),
        "path_taken"
    );
    assert_eq!(
        err(session.prepare_put(&new_id, None, ".luna/a.md", "x", None)),
        "invalid_name_hidden"
    );
    let created = session
        .prepare_put(&new_id, None, "工作/月报.md", "x", None)
        .unwrap_or_else(|_| panic!("create"));
    session
        .record_commit(&created, 1.0)
        .unwrap_or_else(|_| panic!("record"));
    assert!(
        session.file_by_path("工作/月报.md").is_some(),
        "提交成功即进视图"
    );
    assert_eq!(
        err(session.prepare_rename(&new_id, "工作/周报.md")),
        "path_taken"
    );
    let renamed = session
        .prepare_rename(&new_id, "归档/月报.md")
        .unwrap_or_else(|_| panic!("rename"));
    let body: Value = serde_json::from_str(&renamed.body()).unwrap();
    assert_eq!(body["items"][0]["base_rev"], 1);
    assert_eq!(
        body["items"][0]["change"]["chunk_ids"],
        serde_json::json!(created.chunk_ids())
    );
    assert!(renamed.chunk_ids().is_empty(), "改名不重传块");
    session
        .record_commit(&renamed, 2.0)
        .unwrap_or_else(|_| panic!("record rename"));
    assert!(session.file_by_path("工作/月报.md").is_none());
    assert!(session.file_by_path("归档/月报.md").is_some());
}

#[wasm_bindgen_test]
fn rename_and_move_paths_follow_the_shared_rules() {
    let ok =
        |result: Result<String, wasm_bindgen::JsError>| result.unwrap_or_else(|_| panic!("path"));
    assert_eq!(ok(renamed_path("项目/计划.md", "方案")), "项目/方案.md");
    assert_eq!(ok(moved_path("项目/计划.md", "归档")), "归档/计划.md");
    assert_eq!(ok(moved_path("项目/计划.md", "")), "计划.md");
    assert_eq!(
        message(moved_path("计划.md", ".私密").err().unwrap()),
        "invalid_name_hidden"
    );
    assert_eq!(
        message(renamed_path("计划.md", "a/b").err().unwrap()),
        "invalid_name_separator"
    );
    assert_eq!(ok(child_path("项目", "2026")), "项目/2026");
    assert_eq!(
        message(child_path("项目", ".私密").err().unwrap()),
        "invalid_name_hidden"
    );
}
