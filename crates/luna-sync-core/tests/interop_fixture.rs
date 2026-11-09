//! 原生端与网页版（WASM）共用的互通夹具（ADR-177 ⑥、TDD-17 §2）。
//!
//! 夹具 `fixtures/sync-interop-v1.json` 由本测试按固定输入生成：块 id 与块密文是确定的（块用确定性
//! nonce），条目密文用随机 nonce，所以夹具里记下一份原生端封好的条目密文，两端都必须能打开它；提交
//! 请求体同理，条目密文以外的部分逐字节比对，条目密文两端都要能在「基线 + 1」上打开。命名规则
//! （冲突副本、新笔记默认文件名、路径合法性）的输入与结果也在这里。
//! `luna-web` 的 WASM 测试读同一份夹具逐项核对，结果必须逐字节一致。
//!
//! 改了加密格式、分块或合并规则：`LUNA_REGEN_INTEROP_FIXTURE=1 cargo test -p luna-sync-core --test
//! interop_fixture` 重新生成，并在同一次提交里说明为什么变。

use base64::Engine as _;
use luna_sync_core::commit;
use luna_sync_core::crypto::{Cmk, KeyRing};
use luna_sync_core::manifest::{self, EntryPlain};
use luna_sync_core::merge3::{merge3, MergeOutcome};
use luna_sync_core::naming::{self, CivilTime};
use luna_sync_core::path::{name_key, nfc};
use serde_json::{json, Value};
use uuid::Uuid;

const FIXTURE_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/sync-interop-v1.json"
);

const VAULT_ID: &str = "0190f0a0-0000-7000-8000-000000000001";
const FILE_ID: &str = "0190f0a0-0000-7000-8000-0000000000aa";
const REV: i64 = 3;
const CMK_VERSION: u32 = 1;
const CMK_BYTES: [u8; 32] = [7; 32];

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unb64(text: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(text)
        .expect("base64")
}

fn ring() -> KeyRing {
    let vault = Uuid::parse_str(VAULT_ID).unwrap();
    KeyRing::new(vault, &[Cmk::from_bytes(CMK_VERSION, CMK_BYTES)]).unwrap()
}

/// 夹具里的明文：一篇带 frontmatter 的 `.md`，含中文与组合字符。
fn note() -> &'static str {
    "---\ntags: [周报]\n---\n# 周报\n\nCafe\u{301} 进展顺利。\n"
}

fn merge_cases() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("a\nb\nc\n", "A\nb\nc\n", "a\nb\nC\n"),
        ("a\nb\nc\n", "a\nX\nc\n", "a\nY\nc\n"),
        (
            "---\ntitle: 甲\ntags: [x]\n---\n正文\n",
            "---\ntitle: 乙\ntags: [x]\n---\n正文\n",
            "---\ntitle: 甲\ntags: [y]\n---\n正文\n",
        ),
    ]
}

fn outcome_json(outcome: &MergeOutcome) -> Value {
    match outcome {
        MergeOutcome::Merged(text) => json!({ "kind": "merged", "text": text }),
        MergeOutcome::Conflict { regions } => json!({ "kind": "conflict", "regions": regions }),
        MergeOutcome::TooLarge => json!({ "kind": "too_large" }),
    }
}

fn path_cases() -> Vec<&'static str> {
    vec![
        "Cafe\u{301}.md",
        "周报/\u{1100}\u{1167}\u{11a8}.md",
        "MiXeD.md",
    ]
}

/// 冲突副本命名：（原路径，说明，日期，序号）。
fn conflict_copy_cases() -> Vec<(&'static str, &'static str, &'static str, u32)> {
    vec![
        ("周报/2026-11.md", "冲突 网页", "2026-11-06", 1),
        ("周报/2026-11.md", "冲突 网页", "2026-11-06", 2),
        ("附件/图.tar.gz", "冲突副本", "2026-11-06", 1),
        (".env", "conflict copy", "2026-11-06", 3),
    ]
}

/// 新笔记默认路径：（目录，正文，兜底前缀，库里已有的路径）。时刻固定为 [`AT`]。
fn new_note_cases() -> Vec<(&'static str, &'static str, &'static str, Vec<&'static str>)> {
    vec![
        ("Notes", "# 周会纪要\n\n正文", "笔记", vec![]),
        (
            "Notes",
            "## 周会纪要\n",
            "笔记",
            vec!["Notes/周会纪要.md", "notes/周会纪要-1.MD"],
        ),
        ("", "   \n", "Note", vec![]),
        ("Notes", "hello / from: luna", "Note", vec![]),
    ]
}

const AT: CivilTime = CivilTime {
    year: 2026,
    month: 11,
    day: 6,
    hour: 9,
    minute: 5,
    second: 7,
};

fn vault_path_cases() -> Vec<&'static str> {
    vec![
        "工作/周报.md",
        "",
        "a//b.md",
        "a/../b.md",
        ".luna/x.json",
        "a/ b.md",
        "a\\b.md",
    ]
}

/// 提交请求体（JSON）去掉条目密文：两端逐字节一致的那部分。
fn split_entry_ct(body: &str) -> (Value, Vec<u8>) {
    let mut body: Value = serde_json::from_str(body).unwrap();
    let ct = body["items"][0]["change"]
        .as_object_mut()
        .unwrap()
        .remove("entry_ct")
        .unwrap();
    (body, unb64(ct.as_str().unwrap()))
}

/// 除条目密文以外、由输入唯一决定的那部分夹具。
fn deterministic_part() -> Value {
    let ring = ring();
    let prepared = manifest::prepare(
        ring.current(),
        "周报/2026-11.md",
        note().as_bytes(),
        Some(0),
    );
    json!({
        "vault_id": VAULT_ID,
        "cmk_version": CMK_VERSION,
        "cmk_b64": b64(&CMK_BYTES),
        "file_id": FILE_ID,
        "rev": REV,
        "path": "周报/2026-11.md",
        "plaintext": note(),
        "entry": serde_json::to_value(&prepared.entry).unwrap(),
        "size_padded": prepared.size_padded,
        "chunks": prepared.chunks.iter().map(|c| json!({
            "id": c.id,
            "sealed_b64": b64(&c.sealed),
            "ct_sha256": hex(&c.ct_sha256),
        })).collect::<Vec<_>>(),
        "merge": merge_cases().iter().map(|(b, o, t)| json!({
            "base": b, "ours": o, "theirs": t, "outcome": outcome_json(&merge3(b, o, t)),
        })).collect::<Vec<_>>(),
        "paths": path_cases().iter().map(|p| json!({
            "raw": p, "nfc": nfc(p), "name_key": name_key(p),
        })).collect::<Vec<_>>(),
        "conflict_copies": conflict_copy_cases().iter().map(|(path, label, date, attempt)| json!({
            "path": path, "label": label, "date": date, "attempt": attempt,
            "copy": naming::conflict_copy_path(path, label, date, *attempt),
        })).collect::<Vec<_>>(),
        "at": { "date": AT.date(), "stamp": AT.stamp(), "year": AT.year, "month": AT.month,
            "day": AT.day, "hour": AT.hour, "minute": AT.minute, "second": AT.second },
        "new_notes": new_note_cases().iter().map(|(dir, text, prefix, taken)| json!({
            "dir": dir, "text": text, "prefix": prefix, "taken": taken,
            "path": naming::free_note_path(dir, &naming::draft_stem(text, prefix, &AT), |c| {
                taken.iter().any(|t| name_key(t) == name_key(c))
            }),
        })).collect::<Vec<_>>(),
        "vault_paths": vault_path_cases().iter().map(|p| json!({
            "path": p, "error": naming::check_vault_path(p).err().map(naming::NameError::code),
        })).collect::<Vec<_>>(),
        "commit": {
            "base_rev": REV,
            "put_without_entry_ct": split_entry_ct(&commit::body_json(&[commit::put(
                ring.current(), Uuid::parse_str(FILE_ID).unwrap(), Some(REV), &prepared,
            )])).0,
            "delete": serde_json::from_str::<Value>(&commit::body_json(&[commit::delete(
                Uuid::parse_str(FILE_ID).unwrap(), REV,
            )])).unwrap(),
        },
    })
}

#[test]
fn the_shared_fixture_matches_this_implementation() {
    let expected = deterministic_part();
    if std::env::var_os("LUNA_REGEN_INTEROP_FIXTURE").is_some() {
        let ring = ring();
        let entry: EntryPlain = serde_json::from_value(expected["entry"].clone()).unwrap();
        let file_id = Uuid::parse_str(FILE_ID).unwrap();
        // 已有的条目密文仍打得开就留着（随机 nonce：重封一次夹具就无谓地变一次）。
        let kept = std::fs::read_to_string(FIXTURE_PATH)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|old| old["entry_sealed_b64"].as_str().map(str::to_owned))
            .filter(|b64_ct| manifest::open_entry(&ring, file_id, REV, &unb64(b64_ct)).is_ok());
        let sealed = kept
            .unwrap_or_else(|| b64(&manifest::seal_entry(ring.current(), file_id, REV, &entry)));
        let mut written = expected.clone();
        written["entry_sealed_b64"] = json!(sealed);
        let text = serde_json::to_string_pretty(&written).unwrap() + "\n";
        std::fs::write(FIXTURE_PATH, text).unwrap();
    }
    let fixture: Value =
        serde_json::from_str(&std::fs::read_to_string(FIXTURE_PATH).expect("fixture")).unwrap();
    let mut without_entry_ct = fixture.clone();
    without_entry_ct
        .as_object_mut()
        .unwrap()
        .remove("entry_sealed_b64");
    assert_eq!(
        without_entry_ct, expected,
        "加密格式、分块或合并规则变了：重新生成夹具，并在提交里说明原因"
    );

    // 原生端封好的条目密文，原生端自己能打开、拼出原文。
    let ring = ring();
    let file_id = Uuid::parse_str(FILE_ID).unwrap();
    let sealed = unb64(fixture["entry_sealed_b64"].as_str().unwrap());
    let entry = manifest::open_entry(&ring, file_id, REV, &sealed).unwrap();
    let chunks: std::collections::HashMap<String, Vec<u8>> = fixture["chunks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["id"].as_str().unwrap().to_owned(),
                unb64(c["sealed_b64"].as_str().unwrap()),
            )
        })
        .collect();
    let bytes = manifest::assemble(&ring, &entry, |id| {
        chunks.get(id).cloned().ok_or_else(|| id.to_owned())
    })
    .unwrap();
    assert_eq!(bytes, note().as_bytes());

    // 提交请求体里的条目密文封在基线 + 1 上。
    let prepared = manifest::prepare(
        ring.current(),
        "周报/2026-11.md",
        note().as_bytes(),
        Some(0),
    );
    let (_, ct) = split_entry_ct(&commit::body_json(&[commit::put(
        ring.current(),
        file_id,
        Some(REV),
        &prepared,
    )]));
    assert_eq!(
        manifest::open_entry(&ring, file_id, REV + 1, &ct).unwrap(),
        prepared.entry
    );
}
