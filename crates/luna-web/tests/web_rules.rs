//! WASM 端读「附件、标签、全文命中」的互通夹具（`luna-sync-core/tests/fixtures/web-rules-v1.json`），逐项核对
//! 与原生端结果一致（ADR-177 ⑥、TDD-17 §8）：附件放在哪、叫什么、链接怎么写；标签怎么读；全文搜索命中哪几个字。
//! 同时走一遍 Worker 用的封装（上传落点、全文索引、标签列表、逐块拼装、以某一版提交）。
//! `cargo test -p luna-web --target wasm32-unknown-unknown`。
#![cfg(all(target_arch = "wasm32", target_os = "unknown"))]

use base64::Engine as _;
use luna_sync_core::attachment::{
    asset_ext_from_mime, asset_stamp, attachment_target, named_asset_name, pasted_asset_name,
    pasted_asset_prefix,
};
use luna_sync_core::naming::CivilTime;
use luna_sync_core::tags::tag_covers;
use luna_web::search::{match_note_json, note_tags, TextIndex};
use luna_web::{
    default_vault_id, new_file_id, new_vault_request, open_vault_name, Civil, RegistrationStart,
    VaultCrypto, VaultSession,
};
use serde_json::{json, Value};
use wasm_bindgen::JsValue;
use wasm_bindgen_test::wasm_bindgen_test;

const RULES: &str = include_str!("../../luna-sync-core/tests/fixtures/web-rules-v1.json");
const INTEROP: &str = include_str!("../../luna-sync-core/tests/fixtures/sync-interop-v1.json");

fn rules() -> Value {
    serde_json::from_str(RULES).expect("fixture")
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn unb64(text: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(text)
        .expect("base64")
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap()
}

#[wasm_bindgen_test]
fn attachment_targets_and_names_match_the_native_implementation() {
    let f = rules();
    for case in f["attachments"].as_array().unwrap() {
        let target = attachment_target(
            s(&case["mode"]),
            s(&case["folder"]),
            s(&case["note"]),
            s(&case["name"]),
        );
        assert_eq!(target.dir_rel, s(&case["dir_rel"]));
        assert_eq!(target.link, s(&case["link"]));
    }
    let names = &f["names"];
    let n = |key: &str| u32::try_from(names["at"][key].as_u64().unwrap()).unwrap();
    let at = CivilTime {
        year: i32::try_from(names["at"]["year"].as_i64().unwrap()).unwrap(),
        month: n("month"),
        day: n("day"),
        hour: n("hour"),
        minute: n("minute"),
        second: n("second"),
    };
    let stamp = asset_stamp(&at);
    assert_eq!(stamp, s(&names["at"]["stamp"]));
    for case in names["mimes"].as_array().unwrap() {
        let mime = s(&case["mime"]);
        let prefix = pasted_asset_prefix(mime);
        let ext = asset_ext_from_mime(mime);
        assert_eq!(prefix, s(&case["prefix"]));
        assert_eq!(ext, s(&case["ext"]));
        assert_eq!(
            pasted_asset_name(prefix, &stamp, &ext, 0),
            s(&case["first"])
        );
        assert_eq!(
            pasted_asset_name(prefix, &stamp, &ext, 2),
            s(&case["third"])
        );
    }
    for case in names["named"].as_array().unwrap() {
        let attempt = u32::try_from(case["attempt"].as_u64().unwrap()).unwrap();
        assert_eq!(
            named_asset_name(s(&case["file"]), attempt),
            s(&case["name"])
        );
    }
}

#[wasm_bindgen_test]
fn tags_and_full_text_hits_match_the_native_implementation() {
    let f = rules();
    for case in f["tags"]["notes"].as_array().unwrap() {
        let tags: Value = serde_json::from_str(&note_tags(s(&case["content"]))).unwrap();
        assert_eq!(tags, case["tags"]);
    }
    for case in f["tags"]["covers"].as_array().unwrap() {
        assert_eq!(
            tag_covers(s(&case["filter"]), s(&case["tag"])),
            case["covers"].as_bool().unwrap()
        );
    }
    for case in f["matches"].as_array().unwrap() {
        let hit: Value = serde_json::from_str(&match_note_json(
            s(&case["content"]),
            s(&case["query"]),
            case["match_case"].as_bool().unwrap(),
        ))
        .unwrap();
        assert_eq!(hit, case["result"], "{}", case["query"]);
    }
}

// ─── Worker 用的封装 ────────────────────────────────────────────────────────

fn interop() -> Value {
    serde_json::from_str(INTEROP).expect("fixture")
}

fn crypto(f: &Value) -> VaultCrypto {
    VaultCrypto::new(
        s(&f["vault_id"]),
        u32::try_from(f["cmk_version"].as_u64().unwrap()).unwrap(),
        &unb64(s(&f["cmk_b64"])),
    )
    .unwrap_or_else(|_| panic!("vault crypto"))
}

fn envelopes(f: &Value) -> Vec<u8> {
    json!({ "envelopes": [
        { "slot": "managed", "cmk_version": f["cmk_version"], "wrapped_cmk": f["cmk_b64"],
          "kdf_params": {}, "created_by_device": f["file_id"], "created_at": "2026-11-06T00:00:00Z" },
    ]})
    .to_string()
    .into_bytes()
}

/// 一个已打开的库：每个文件一条 rev 1 的条目；返回会话、各文件的 id 与块（id → 密文）。
fn session_with(
    f: &Value,
    files: &[(&str, &[u8])],
) -> (
    VaultSession,
    Vec<String>,
    std::collections::HashMap<String, Vec<u8>>,
) {
    let crypto = crypto(f);
    let mut ids = Vec::new();
    let mut blobs = std::collections::HashMap::new();
    let entries: Vec<Value> = files
        .iter()
        .map(|(path, bytes)| {
            let prepared = crypto
                .prepare(path, bytes, Some(1_000.0))
                .unwrap_or_else(|_| panic!("prepare"));
            let summary: Value = serde_json::from_str(&prepared.summary()).unwrap();
            for (index, chunk) in summary["chunks"].as_array().unwrap().iter().enumerate() {
                blobs.insert(
                    s(&chunk["id"]).to_owned(),
                    prepared.sealed(index).unwrap_or_else(|_| panic!("sealed")),
                );
            }
            let file_id = new_file_id();
            let sealed = crypto
                .seal_entry(&file_id, 1.0, &summary["entry"].to_string())
                .unwrap_or_else(|_| panic!("seal"));
            ids.push(file_id.clone());
            json!({ "file_id": file_id, "rev": 1, "seq": 1, "deleted": false, "entry_ct": b64(&sealed),
                    "size_padded": summary["size_padded"], "device_id": file_id,
                    "committed_at": "2026-11-06T00:00:00Z" })
        })
        .collect();
    let mut session = VaultSession::open(s(&f["vault_id"]), &envelopes(f))
        .unwrap_or_else(|_| panic!("open session"));
    session
        .load_manifest(
            json!({ "epoch": "0190f0a0-0000-7000-8000-0000000000ee", "at_seq": 1, "entries": entries })
                .to_string()
                .as_bytes(),
        )
        .unwrap_or_else(|_| panic!("manifest"));
    (session, ids, blobs)
}

#[wasm_bindgen_test]
fn uploads_land_where_the_vault_settings_say_and_dodge_taken_names() {
    let f = interop();
    let settings = b"schema_version = 1\n\n[values]\n\"vault.attachment_location\" = \"specified_folder\"\n\"vault.attachment_folder\" = \"\xe9\x99\x84\xe4\xbb\xb6\"\n";
    let (session, _, blobs) = session_with(
        &f,
        &[
            ("写作/上传.md", b"# x\n"),
            (".luna/settings.toml", settings),
            ("附件/Pasted-image-20261106-210405.png", b"png"),
        ],
    );
    let file: Value = serde_json::from_str(&session.vault_settings_file().unwrap()).unwrap();
    assert_eq!(file["chunks"].as_array().unwrap().len(), 1);
    assert!(blobs.contains_key(s(&file["chunks"][0])));
    let at = Civil::new(2026, 11, 6, 21, 4, 5);
    let target: Value = serde_json::from_str(
        &session
            .attachment_upload_target(
                "写作/上传.md",
                Some(String::from_utf8(settings.to_vec()).unwrap()),
                "pasted",
                "image.png",
                "image/png",
                &at,
            )
            .unwrap_or_else(|_| panic!("target")),
    )
    .unwrap();
    assert_eq!(target["path"], "附件/Pasted-image-20261106-210405-1.png");
    assert_eq!(target["link"], "../附件/Pasted-image-20261106-210405-1.png");
    let target: Value = serde_json::from_str(
        &session
            .attachment_upload_target(
                "写作/上传.md",
                None,
                "named",
                "说明 书.PDF",
                "application/pdf",
                &at,
            )
            .unwrap_or_else(|_| panic!("target")),
    )
    .unwrap();
    assert_eq!(target["path"], "写作/assets/说明-书.pdf");
    assert_eq!(target["link"], "assets/说明-书.pdf");
}

#[wasm_bindgen_test]
fn the_text_index_downloads_only_notes_and_answers_search_and_tags() {
    let f = interop();
    let (session, ids, _) = session_with(
        &f,
        &[("周报.md", b"x"), ("韩文/인사.md", b"y"), ("图.png", b"z")],
    );
    let mut index = TextIndex::new();
    let wanted: Value = serde_json::from_str(&index.sync(&session)).unwrap();
    assert_eq!(wanted.as_array().unwrap().len(), 2);
    index
        .add(&session, &ids[0], "本周完成了网页版骨架 #工作/周报".into())
        .unwrap_or_else(|_| panic!("add"));
    index
        .add(&session, &ids[1], "안녕하세요 #한국어".into())
        .unwrap_or_else(|_| panic!("add"));
    let hits: Value = serde_json::from_str(&index.search("骨架", false)).unwrap();
    assert_eq!(hits["notes"][0]["path"], "周报.md");
    let hits: Value = serde_json::from_str(&index.search("안녕", false)).unwrap();
    assert_eq!(hits["notes"][0]["path"], "韩文/인사.md");
    let tags: Value = serde_json::from_str(&index.tags()).unwrap();
    assert_eq!(tags[0]["tag"], "工作/周报");
    let notes: Value =
        serde_json::from_str(&index.tag_notes("工作").unwrap_or_else(|_| panic!("tag"))).unwrap();
    assert_eq!(notes[0]["path"], "周报.md");
    assert!(index.tag_notes(" ").is_err());
}

#[wasm_bindgen_test]
fn big_files_assemble_chunk_by_chunk_and_versions_commit_with_their_old_chunks() {
    let f = interop();
    let pdf: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
    let (session, ids, blobs) = session_with(&f, &[("附件/说明.pdf", &pdf), ("a.md", b"x")]);
    let plan: Value = serde_json::from_str(
        &session
            .download_plan(&ids[0])
            .unwrap_or_else(|_| panic!("plan")),
    )
    .unwrap();
    assert_eq!(plan["name"], "说明.pdf");
    assert_eq!(plan["too_large"], false);
    let mut assembly = session
        .begin_assembly(&ids[0])
        .unwrap_or_else(|_| panic!("begin"));
    let mut out = Vec::new();
    while let Some(id) = assembly.next_chunk() {
        out.extend(
            assembly
                .push(&session, &id, &blobs[&id])
                .unwrap_or_else(|_| panic!("push")),
        );
    }
    assembly.finish().unwrap_or_else(|_| panic!("finish"));
    assert_eq!(out, pdf);

    let head: Value =
        serde_json::from_str(&session.file(&ids[1]).unwrap_or_else(|_| panic!("head"))).unwrap();
    let entry = json!({ "v": 1, "path": "a.md", "size": 1, "mtime_ms": 1000,
        "content_hash": luna_web::content_hash(b"x"), "chunks": head["chunks"] });
    let sealed = crypto(&f)
        .seal_entry(&ids[1], 1.0, &entry.to_string())
        .unwrap_or_else(|_| panic!("seal"));
    let version =
        json!({ "file_id": ids[1], "rev": 1, "seq": 1, "deleted": false, "entry_ct": b64(&sealed),
        "size_padded": 4096, "device_id": ids[1], "committed_at": "2026-11-06T00:00:00Z" })
        .to_string();
    let prepared = session
        .prepare_revert(&ids[1], 4.0, None, version.as_bytes(), Some(2_000.0))
        .unwrap_or_else(|_| panic!("revert"));
    assert!(prepared.chunk_ids().is_empty(), "块原样引用、不重传");
    assert_eq!(prepared.path().as_deref(), Some("a.md"));
    let body: Value = serde_json::from_str(&prepared.body()).unwrap();
    assert_eq!(body["items"][0]["base_rev"], 4);
    assert_eq!(body["items"][0]["change"]["chunk_ids"], head["chunks"]);
    assert_eq!(body["items"][0]["change"]["size_padded"], 4096);
    assert!(
        session
            .prepare_revert(
                &ids[1],
                4.0,
                Some("附件/说明.pdf".into()),
                version.as_bytes(),
                None
            )
            .is_err(),
        "路径被别的文件占着"
    );
    // 墓碑那一版没有条目：恢复不了。
    let tombstone =
        json!({ "file_id": ids[1], "rev": 2, "seq": 2, "deleted": true, "entry_ct": null,
        "size_padded": 0, "device_id": ids[1], "committed_at": "2026-11-06T00:00:00Z" })
        .to_string();
    assert!(session
        .prepare_revert(&ids[1], 4.0, None, tombstone.as_bytes(), None)
        .is_err());
}

#[wasm_bindgen_test]
fn the_recently_deleted_page_opens_names_and_hides_machine_files() {
    let f = interop();
    let (session, _, _) = session_with(&f, &[("a.md", b"x")]);
    let crypto = crypto(&f);
    let sealed = |file_id: &str, path: &str| {
        let entry = json!({ "v": 1, "path": path, "size": 1, "mtime_ms": null,
            "content_hash": luna_web::content_hash(b"x"), "chunks": ["0".repeat(64)] });
        b64(&crypto
            .seal_entry(file_id, 4.0, &entry.to_string())
            .unwrap_or_else(|_| panic!("seal")))
    };
    let (kept, hidden, expired) = (new_file_id(), new_file_id(), new_file_id());
    let response = json!({
        "entries": [
            { "file_id": kept, "rev": 5, "seq": 9, "last_rev": 4, "entry_ct": sealed(&kept, "写作/回收.md"),
              "deleted_at": "2026-11-06T01:00:00Z", "device_id": kept },
            { "file_id": hidden, "rev": 5, "seq": 8, "last_rev": 4, "entry_ct": sealed(&hidden, ".luna/sidecars/x.json"),
              "deleted_at": "2026-11-06T00:30:00Z", "device_id": hidden },
            { "file_id": expired, "rev": 2, "seq": 3, "deleted_at": "2026-10-01T00:00:00Z", "device_id": expired },
        ],
        "next_cursor": 3,
    })
    .to_string();
    let page: Value = serde_json::from_str(
        &session
            .deleted_page(response.as_bytes())
            .unwrap_or_else(|_| panic!("page")),
    )
    .unwrap();
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "隐藏路径不列：{items:?}");
    assert_eq!(items[0]["path"], "写作/回收.md");
    assert_eq!(items[0]["name"], "回收.md");
    assert_eq!(items[0]["restorable"], true);
    assert_eq!(items[0]["last_rev"], 4);
    assert_eq!(items[1]["path"], Value::Null);
    assert_eq!(items[1]["restorable"], false, "那一版过了保留期：只列出");
    assert_eq!(page["next_cursor"], 3);
}

#[wasm_bindgen_test]
fn default_vault_id_is_stable_per_account() {
    let first = default_vault_id("a@example.com");
    assert_eq!(first, default_vault_id("a@example.com"));
    assert_ne!(first, default_vault_id("b@example.com"));
    assert_eq!(uuid::Uuid::parse_str(&first).unwrap().get_version_num(), 8);
}

#[wasm_bindgen_test]
fn a_new_vault_request_opens_with_its_own_envelope() {
    let id = new_file_id();
    let body: Value = serde_json::from_slice(
        &new_vault_request(&id, "我的笔记")
            .map_err(JsValue::from)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["id"], id);
    let envelopes = json!({ "envelopes": [body["envelope"].clone()] }).to_string();
    let name = open_vault_name(&id, envelopes.as_bytes(), body["name_ct"].as_str().unwrap())
        .map_err(JsValue::from)
        .unwrap();
    assert_eq!(name, "我的笔记");
}

#[wasm_bindgen_test]
fn registration_produces_an_upload_the_server_side_accepts() {
    use opaque_ke::{RegistrationRequest, RegistrationUpload, ServerRegistration, ServerSetup};

    let setup = ServerSetup::<luna_opaque::Suite>::new(&mut aes_gcm::aead::OsRng);
    let mut registration = RegistrationStart::new("a long passphrase")
        .map_err(JsValue::from)
        .unwrap();
    let request = RegistrationRequest::<luna_opaque::Suite>::deserialize(
        &luna_opaque::from_b64u(&registration.request()).unwrap(),
    )
    .unwrap();
    let response =
        ServerRegistration::<luna_opaque::Suite>::start(&setup, request, b"a@example.com").unwrap();
    let upload = registration
        .finish(
            &luna_opaque::b64u(&response.message.serialize()),
            "a@example.com",
            "deployment",
        )
        .map_err(JsValue::from)
        .unwrap();
    RegistrationUpload::<luna_opaque::Suite>::deserialize(
        &luna_opaque::from_b64u(&upload).unwrap(),
    )
    .unwrap();
}
