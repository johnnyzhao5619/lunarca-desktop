//! 网页版与桌面共用的「附件、标签、全文命中」规则的互通夹具（ADR-177 ⑥、TDD-17 §8）。
//!
//! 夹具 `fixtures/web-rules-v1.json` 由本测试按固定输入生成：附件的落点与链接、附件名、标签的读法、`tag:`
//! 过滤器、全文搜索的命中与片段。`luna-web` 的 WASM 测试读同一份夹具逐项核对，结果必须逐字节一致。
//!
//! 改了这些规则：`LUNA_REGEN_WEB_RULES_FIXTURE=1 cargo test -p luna-sync-core --test web_rules_fixture`
//! 重新生成，并在同一次提交里说明为什么变。

use luna_sync_core::attachment::{
    asset_ext_from_mime, asset_stamp, attachment_target, named_asset_name, pasted_asset_name,
    pasted_asset_prefix,
};
use luna_sync_core::naming::CivilTime;
use luna_sync_core::search::{match_note, SearchQuery, MAX_LINES_PER_NOTE};
use luna_sync_core::tags::{extract_tags, tag_covers};
use serde_json::{json, Value};

const FIXTURE_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/web-rules-v1.json"
);

const AT: CivilTime = CivilTime {
    year: 2026,
    month: 11,
    day: 6,
    hour: 21,
    minute: 4,
    second: 5,
};

fn attachments() -> Value {
    let cases = [
        ("", "", "写作/上传.md", "Pasted-image-1.png"),
        ("note_subfolder", "assets", "根.md", "p.png"),
        ("note_folder", "", "项目/计划.md", "图 1.png"),
        ("vault_root", "", "项目/子/计划.md", "p.png"),
        ("specified_folder", "附件", "写作/上传.md", "说明.pdf"),
        ("specified_folder", "", "a.md", "p.png"),
        ("unknown", "../越界/sub", "a/b.md", "p.png"),
    ];
    json!(cases
        .iter()
        .map(|(mode, folder, note, name)| {
            let target = attachment_target(mode, folder, note, name);
            json!({ "mode": mode, "folder": folder, "note": note, "name": name,
                    "dir_rel": target.dir_rel, "link": target.link })
        })
        .collect::<Vec<_>>())
}

fn names() -> Value {
    let stamp = asset_stamp(&AT);
    let mimes = [
        "image/png",
        "image/jpeg",
        "image/heic",
        "image/x-icon",
        "application/pdf",
        "",
    ];
    let named = [
        ("diagram.png", 0),
        ("季度 报告 (终).PDF", 0),
        ("季度 报告.PDF", 2),
        ("archive.tar.gz", 0),
        ("   .png", 0),
        ("说明.p df", 1),
        ("한국어 문서.docx", 0),
    ];
    json!({
        "at": { "year": AT.year, "month": AT.month, "day": AT.day,
                "hour": AT.hour, "minute": AT.minute, "second": AT.second, "stamp": stamp },
        "mimes": mimes.iter().map(|mime| json!({
            "mime": mime,
            "ext": asset_ext_from_mime(mime),
            "prefix": pasted_asset_prefix(mime),
            "first": pasted_asset_name(pasted_asset_prefix(mime), &stamp, &asset_ext_from_mime(mime), 0),
            "third": pasted_asset_name(pasted_asset_prefix(mime), &stamp, &asset_ext_from_mime(mime), 2),
        })).collect::<Vec<_>>(),
        "named": named.iter().map(|(file, attempt)| json!({
            "file": file, "attempt": attempt, "name": named_asset_name(file, *attempt),
        })).collect::<Vec<_>>(),
    })
}

fn tags() -> Value {
    let notes = [
        "---\ntags: [入门, 网页]\n---\n# 欢迎\n正文 #标签/子 与 #入门 再见",
        "---\ntags: 工作, 重要\nluna:\n  ai_tags: [会议, 工作]\n---\n正文",
        "안녕하세요 #한국어 그리고 #中文 `#代码` 不算\n```\n#围栏\n```\n# 标题不是标签 #123",
        "没有标签的笔记",
        "---\ntags: [未闭合\n正文 #仍然读",
    ];
    let covers = [
        ("工作", "工作"),
        ("工作", "工作/周报"),
        ("#Project", "project/luna"),
        ("工作", "工作室"),
        ("工作/周报", "工作"),
    ];
    json!({
        "notes": notes.iter().map(|content| json!({
            "content": content,
            "tags": extract_tags(content).into_iter()
                .map(|t| json!({ "tag": t.tag, "source": t.source.key() }))
                .collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "covers": covers.iter().map(|(filter, tag)| json!({
            "filter": filter, "tag": tag, "covers": tag_covers(filter, tag),
        })).collect::<Vec<_>>(),
    })
}

fn matches() -> Value {
    let cases = [
        (
            "---\ntitle: 周报\n---\n# 周报\n本周完成了网页版骨架。\n",
            "骨架",
            false,
        ),
        ("Le Café ouvre\nCAFE\u{301} fermé\n", "café", false),
        ("Le Café ouvre\n", "café", true),
        ("안녕하세요 회의\n각 가\n", "가", false),
        (
            "[00:01:02] 讲到骨架 [00:01:03]\n## User\n骨架\n",
            "骨架 -草稿",
            false,
        ),
        ("甲 乙\n乙 丙\n", "甲 乙", false),
        ("甲\n", "\"甲 乙\"", false),
    ];
    json!(cases
        .iter()
        .map(|(content, query, match_case)| {
            let hit = match_note(
                content,
                &SearchQuery::parse(query, *match_case),
                MAX_LINES_PER_NOTE,
            );
            json!({ "content": content, "query": query, "match_case": match_case,
                    "result": hit.map(|body| json!({ "match_count": body.match_count, "lines": body.lines })) })
        })
        .collect::<Vec<_>>())
}

fn expected() -> Value {
    json!({
        "attachments": attachments(),
        "names": names(),
        "tags": tags(),
        "matches": matches(),
    })
}

#[test]
fn the_shared_web_rules_fixture_matches_this_implementation() {
    let expected = expected();
    if std::env::var_os("LUNA_REGEN_WEB_RULES_FIXTURE").is_some() {
        let text = serde_json::to_string_pretty(&expected).unwrap() + "\n";
        std::fs::write(FIXTURE_PATH, text).unwrap();
    }
    let fixture: Value =
        serde_json::from_str(&std::fs::read_to_string(FIXTURE_PATH).expect("fixture")).unwrap();
    assert_eq!(
        fixture, expected,
        "附件、标签或全文命中的规则变了：重新生成夹具，并在提交里说明原因"
    );
}
