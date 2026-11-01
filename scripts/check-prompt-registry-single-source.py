#!/usr/bin/env python3
"""能力提示词唯一事实源守护（ADR-050，TDD-13 §2）。

本批把四能力的系统提示词从三个 crate 里的字面量搬进 `crates/luna-core/prompts/bundled.json`，
组装点收敛为 `luna_provider::llm_http::build_chat_messages` 一处。**纯纪律守不住这件事**：

- 加一句提示词最省事的写法永远是就地写个字符串——它编译得过、lint 不管、单测照绿；
- 而它的代价要到很久以后才现形：运营在管理台改了模板，产品只有一半改口，
  另一半仍是发版那天焊死的那句话，**没有任何地方会报错**。

故本脚本回答两个机器事实：

- **A. 零残留**：`FINGERPRINTS` 里的每一句提示词，只允许出现在版本化资源与评测用例里。
  出现在任何 `.rs` / `.ts` / `.tsx` 里即判红（包括测试——用例里写死一句模板，等于
  允许实现里也写死它，而那正是本批消灭的形态）。
- **B. 清单齐备且无冗余**：`prompt.rs::REQUIRED_TEMPLATES` 声明的每个 key 在
  `bundled.json` 里必须存在且非空；反过来，`bundled.json` 里也不许有未登记的 key
  ——未登记 = 服务端发布校验不会要求它 = 目录覆盖时它会静默回落随包。

**A 只认「整句」而不是关键词**：拿 `"You are"` 之类的词根去扫会把无关代码扫红，
于是下一个人就会去放宽规则；而放宽过的门禁是最贵的一种——它还在跑，只是不再拦得住。
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

# 版本化资源（唯一事实源）与评测用例——只有这两处允许出现提示词原文。
RESOURCE = Path("crates/luna-core/prompts/bundled.json")
ALLOWED_PREFIXES = ("crates/luna-core/prompts/", "evals/", "scripts/check-prompt-registry-single-source.py")

# 被扫描的源码面。
SCAN_GLOBS = ("crates/**/*.rs", "src-tauri/src/**/*.rs", "apps/desktop/src/**/*.ts", "apps/desktop/src/**/*.tsx")

# 每一句都曾经是代码里的字面量（本批之前）。删一条之前先回答：
# 「它现在从哪里来？」——答案只能是 bundled.json。
FINGERPRINTS = (
    "You are a professional translation engine.",
    "Summarize the user's text concisely.",
    "You are a writing assistant.",
    "Polish the text to be clearer and more natural",
    "Preserve the original line breaks, lists, paragraphs",
    "Polish the transcript into clear, natural writing",
    "Polish the transcript in a professional email tone",
    "Polish the transcript in a friendly team chat style",
    "Polish the transcript into precise code comments",
    "Translate the transcript into",
    "Use a casual, concise Slack messaging tone.",
    "Use a polite, professional email tone.",
    "Use natural, idiomatic English.",
    "You are Lunarca, a helpful, concise desktop assistant",
    "You are a professional translator.",
    "You are a senior software engineer.",
    "You distill text into faithful, compact summaries",
    # ADR-083 ⑦：实时/批量共用的那句上下文指令，本批拆成 translate.live_context /
    # translate.document_context 两条模板。它此前硬编码在
    # crates/luna-jobqueue/src/realtime_translate.rs::context_instruction 里，
    # 于是运营改不了、发布下发不了、评测也覆盖不到——而批量补译正是拿它译的。
    "The text is one sentence from a live speech transcript",
    "Preceding sentences (source => translation)",
    # ADR-050 修订 2026-09-05：七个任务域 + 术语约束句 + 弹窗同语润色，此前全是 Rust 里的
    # format! 字面量（src-tauri/src/ipc/{translate,graph,ingest,checkup,quick_note_refine,
    # knowledge}.rs、crates/luna-core/src/{translate,distill_template,glossary}.rs）。
    "Use exactly these terms (do not paraphrase them)",
    "Rewrite the text in {lang} to be more natural and idiomatic",
    "Rewrite the text to be clearer and more natural without changing its meaning",
    "in the exact same script and character variant",
    "Fix only spelling, grammar, and punctuation errors",
    "while keeping all facts, meaning, and structure",
    "Continue writing from exactly where it ends",
    "here is the text surrounding the passage",
    "Write the summary as concise bullet points",
    "Write the summary as one flowing paragraph",
    # 对标方案 T6（2026-10-03）：表格 AI 动作的三条模板。
    "Rearrange the text into one Markdown (GFM) table",
    "The text is a Markdown table. Explain what it shows",
    "You fill in one cell of a table.",
    "Write the summary as a single sentence",
    "The text is a meeting transcript; each line starts with the speaker's name",
    "The text is what comes right before the cursor and is context only",
    "Follow the user's instruction and apply it to the text",
    "Summarize this part of a longer document",
    "The text is a sentence from a document being proofread",
    "a note that someone can read months later without the original material",
    "The text is a conversation between a user and an AI assistant",
    "The text is a transcript of recorded audio",
    "The text is the user's own rough notes",
    "The following facts were already extracted from this source and confirmed",
    "This is one part of a longer transcript. Condense it",
    "Rewrite this part of a transcript in",
    "one coherent note that groups them by topic",
    "The following excerpts were retrieved from the user's knowledge base",
    "以下是用户知识库中检索到的相关摘录",
    "You are a knowledge-base tagging engine",
    "Prefer reusing these existing vault tags",
    "Write all tags in Simplified Chinese",
    "Write all tags in English",
    "You maintain a topic note",
    "你在维护一篇主题笔记",
    "Below are the section headings of a topic note",
    "下面是一篇主题笔记的小标题清单",
    "Below is a single claim from a topic note",
    "下面是一篇主题笔记里的**一条声明**",
    "The text is a short voice quick note",
)

REQUIRED_BLOCK = re.compile(
    r"pub const REQUIRED_TEMPLATES:.*?=\s*&\[(?P<body>.*?)\n\];", re.S
)
SCOPE_ENTRY = re.compile(r"PromptScope::(?P<scope>\w+)\s*,\s*&\[(?P<keys>[^\]]*)\]", re.S)
# 一条必备模板：`("name", since)`（ADR-050 修订 2026-09-04：清单带版本）。
KEY_ENTRY = re.compile(r'\(\s*"(?P<name>[^"]+)"\s*,\s*(?P<since>\d+)\s*\)')
MANIFEST_VERSION = re.compile(r"pub const PROMPT_MANIFEST_VERSION:\s*u32\s*=\s*(?P<v>\d+)\s*;")
# 一方应用自带的模板（ADR-166 ⑤）：`("app", PromptScope::X, &["name", ...])`。
APP_BLOCK = re.compile(r"pub const APP_TEMPLATES:.*?=\s*&\[(?P<body>.*?)\n\];", re.S)
APP_ENTRY = re.compile(
    r'\(\s*"(?P<app>[^"]+)"\s*,\s*PromptScope::(?P<scope>\w+)\s*,\s*&\[(?P<keys>[^\]]*)\]', re.S
)
APP_KEY = re.compile(r'"(?P<name>[^"]+)"')
# 随包 manifest（`export-prompt-manifest` 的产物；服务端读的是它的副本）。
MANIFEST = Path("crates/luna-core/prompts/manifest.json")


def declared_requirements(prompt_rs: str) -> dict[str, list[str]]:
    """从 `prompt.rs` 里读出 REQUIRED_TEMPLATES 的模板名（两处清单靠解析同步，不靠人对齐）。"""
    return {
        scope: [name for name, _ in entries]
        for scope, entries in declared_manifest(prompt_rs)[1].items()
    }


def declared_manifest(prompt_rs: str) -> tuple[int, dict[str, list[tuple[str, int]]]]:
    """从 `prompt.rs` 里读出（manifest 版本, scope → [(模板名, since)]）。"""
    match = REQUIRED_BLOCK.search(prompt_rs)
    if match is None:
        raise SystemExit("❌ 在 crates/luna-core/src/prompt.rs 里找不到 REQUIRED_TEMPLATES")
    version = MANIFEST_VERSION.search(prompt_rs)
    if version is None:
        raise SystemExit("❌ 在 crates/luna-core/src/prompt.rs 里找不到 PROMPT_MANIFEST_VERSION")
    out: dict[str, list[tuple[str, int]]] = {}
    for entry in SCOPE_ENTRY.finditer(match.group("body")):
        scope = entry.group("scope")
        scope_wire = re.sub(r"(?<!^)(?=[A-Z])", "_", scope).lower()
        keys = [(m.group("name"), int(m.group("since"))) for m in KEY_ENTRY.finditer(entry.group("keys"))]
        out[scope_wire] = keys
    # 单 key 的域写成 `&[("structure", 1)]`，上面的正则一并吃到；没吃到就是形状变了。
    if not out or not any(out.values()):
        raise SystemExit("❌ REQUIRED_TEMPLATES 解析为空——形状变了，先更新本脚本")
    return int(version.group("v")), out


def declared_app_templates(prompt_rs: str) -> list[tuple[str, str, list[str]]]:
    """从 `prompt.rs` 里读出 APP_TEMPLATES：[(应用, scope, [模板名])]；没有这张表就是空。"""
    match = APP_BLOCK.search(prompt_rs)
    if match is None:
        return []
    out = []
    for entry in APP_ENTRY.finditer(match.group("body")):
        scope = re.sub(r"(?<!^)(?=[A-Z])", "_", entry.group("scope")).lower()
        out.append((entry.group("app"), scope, [m.group("name") for m in APP_KEY.finditer(entry.group("keys"))]))
    if not out:
        raise SystemExit("❌ APP_TEMPLATES 解析为空——形状变了，先更新本脚本")
    return out


def check_manifest(root: Path) -> list[str]:
    """随包 `manifest.json` 必须与 `prompt.rs` 逐字一致（服务端读的是它的副本）。"""
    prompt_rs = root / "crates/luna-core/src/prompt.rs"
    manifest = root / MANIFEST
    if not manifest.exists():
        return [f"❌ 缺随包 manifest {MANIFEST}——跑 `cargo run -p luna-app --bin export-prompt-manifest`"]
    try:
        data = json.loads(manifest.read_text(encoding="utf-8"))
    except json.JSONDecodeError as err:
        return [f"❌ {MANIFEST} 不是合法 JSON：{err}"]
    source = prompt_rs.read_text(encoding="utf-8")
    version, required = declared_manifest(source)
    expected = {
        "manifest_version": version,
        "scopes": [
            {"scope": scope, "templates": [{"name": n, "since": s} for n, s in keys]}
            for scope, keys in required.items()
        ],
        "apps": [
            {"app": app, "scope": scope, "templates": names}
            for app, scope, names in declared_app_templates(source)
        ],
    }
    problems: list[str] = []
    if data != expected:
        problems.append(
            f"❌ {MANIFEST} 与 REQUIRED_TEMPLATES / PROMPT_MANIFEST_VERSION 不一致——重跑 "
            "`cargo run -p luna-app --bin export-prompt-manifest > crates/luna-core/prompts/manifest.json`"
        )
    for scope, keys in required.items():
        for name, since in keys:
            if not 1 <= since <= version:
                problems.append(
                    f"❌ `{scope}.{name}` 的 since={since} 越出 1..={version}——新增必备键要同时把 "
                    "PROMPT_MANIFEST_VERSION +1"
                )
    return problems


RAW_ESCAPE = "Instruction::raw"


def check_raw_instruction(root: Path) -> list[str]:
    """`Instruction::raw` 只准出现在测试里（ADR-050 修订 2026-09-05 ②：类型即约束）。

    `JobKind` 的指令字段只收 `Instruction`，而它在注册表之外唯一的构造口就是 `raw`——它是
    给测试夹具与 wire 反序列化留的。生产代码调用它，等于把编译期约束重新变回散文纪律。
    判据是**位置**：文件路径含 `/tests/`、或调用点在该文件第一个 `#[cfg(test)]` 之后。
    """
    problems: list[str] = []
    for pattern in ("crates/**/*.rs", "src-tauri/src/**/*.rs"):
        for path in sorted(root.glob(pattern)):
            rel = path.relative_to(root).as_posix()
            if "/tests/" in rel or rel == "crates/luna-core/src/prompt.rs":
                continue
            try:
                text = path.read_text(encoding="utf-8")
            except (OSError, UnicodeDecodeError):
                continue
            test_start = text.find("#[cfg(test)]")
            cursor = 0
            while (at := text.find(RAW_ESCAPE, cursor)) != -1:
                if test_start == -1 or at < test_start:
                    line = text.count("\n", 0, at) + 1
                    problems.append(
                        f"❌ {rel}:{line} 在生产代码里调用 `{RAW_ESCAPE}`——它只准测试用；"
                        "要一句新指令就往 bundled.json 加模板并登记进 REQUIRED_TEMPLATES"
                    )
                cursor = at + len(RAW_ESCAPE)
    return problems


def check_no_residue(root: Path) -> list[str]:
    problems: list[str] = []
    for pattern in SCAN_GLOBS:
        for path in sorted(root.glob(pattern)):
            rel = path.relative_to(root).as_posix()
            if rel.startswith(ALLOWED_PREFIXES):
                continue
            try:
                text = path.read_text(encoding="utf-8")
            except (OSError, UnicodeDecodeError):
                continue
            for fingerprint in FINGERPRINTS:
                if fingerprint in text:
                    problems.append(
                        f"❌ {rel} 里出现提示词字面量 {fingerprint!r}——"
                        "能力提示词的唯一事实源是 crates/luna-core/prompts/bundled.json（ADR-050）"
                    )
    return problems


def check_resource(root: Path) -> list[str]:
    problems: list[str] = []
    resource = root / RESOURCE
    prompt_rs = root / "crates/luna-core/src/prompt.rs"
    if not resource.exists():
        return [f"❌ 缺随包提示词资源 {RESOURCE}"]
    if not prompt_rs.exists():
        return ["❌ 缺 crates/luna-core/src/prompt.rs"]

    try:
        data = json.loads(resource.read_text(encoding="utf-8"))
    except json.JSONDecodeError as err:
        return [f"❌ {RESOURCE} 不是合法 JSON：{err}"]

    present: dict[str, dict[str, str]] = {}
    for entry in data.get("sets", []):
        if entry.get("locale") is not None:
            continue  # locale 变体允许只覆盖一部分，不参与完整性判定。
        present[entry["scope"]] = entry.get("templates", {})

    source = prompt_rs.read_text(encoding="utf-8")
    required = declared_requirements(source)
    app_keys: dict[str, set[str]] = {}
    for app, scope, names in declared_app_templates(source):
        for name in names:
            if not name.startswith(f"{app}."):
                problems.append(f"❌ 应用 {app} 自带的 `{scope}.{name}` 没以 `{app}.` 开头——一眼看不出归谁")
            app_keys.setdefault(scope, set()).add(name)
            body = present.get(scope, {}).get(name)
            if body is None or not body.strip():
                problems.append(f"❌ {RESOURCE} 的 `{scope}.{name}` 缺失或为空——应用自带的模板必须随包")
    for scope, keys in required.items():
        templates = present.get(scope)
        if templates is None:
            problems.append(f"❌ {RESOURCE} 缺 scope `{scope}` 的默认变体")
            continue
        for key in keys:
            body = templates.get(key)
            if body is None or not body.strip():
                problems.append(
                    f"❌ {RESOURCE} 的 `{scope}.{key}` 缺失或为空——离线用户会拿到一句空提示词"
                )
        for key in templates:
            if key not in keys and key not in app_keys.get(scope, set()):
                problems.append(
                    f"❌ {RESOURCE} 的 `{scope}.{key}` 没登记进 REQUIRED_TEMPLATES 或 APP_TEMPLATES——"
                    "未登记的 key 服务端发布校验不会要求它，目录覆盖时它会静默回落随包"
                )
    for scope in present:
        if scope not in required:
            problems.append(f"❌ {RESOURCE} 出现未登记的 scope `{scope}`")
    return problems


def self_test() -> int:
    """反向验证：断言不是空跑的。"""
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        (root / "crates/luna-core/src").mkdir(parents=True)
        (root / "crates/luna-core/prompts").mkdir(parents=True)
        (root / "crates/luna-provider/src").mkdir(parents=True)
        (root / "crates/luna-core/src/prompt.rs").write_text(
            "pub const PROMPT_MANIFEST_VERSION: u32 = 2;\n"
            "pub const REQUIRED_TEMPLATES: &[(PromptScope, &[(&str, u32)])] = &[\n"
            '    (PromptScope::Common, &[("structure", 1)]),\n'
            '    (PromptScope::Translate, &[("system", 1), ("style", 2)]),\n'
            "];\n"
            "pub const APP_TEMPLATES: &[(&str, PromptScope, &[&str])] = &[\n"
            '    ("creator", PromptScope::Translate, &["creator.pitch"]),\n'
            "];\n",
            encoding="utf-8",
        )
        (root / MANIFEST).write_text(
            json.dumps(
                {
                    "manifest_version": 2,
                    "scopes": [
                        {"scope": "common", "templates": [{"name": "structure", "since": 1}]},
                        {
                            "scope": "translate",
                            "templates": [
                                {"name": "system", "since": 1},
                                {"name": "style", "since": 2},
                            ],
                        },
                    ],
                    "apps": [{"app": "creator", "scope": "translate", "templates": ["creator.pitch"]}],
                }
            ),
            encoding="utf-8",
        )
        good = {
            "sets": [
                {"scope": "common", "locale": None, "templates": {"structure": "keep lines"}},
                {
                    "scope": "translate",
                    "locale": None,
                    "templates": {"system": "translate {x}", "style": "s", "creator.pitch": "p"},
                },
            ]
        }
        resource = root / RESOURCE
        resource.write_text(json.dumps(good), encoding="utf-8")
        (root / "crates/luna-provider/src/llm_http.rs").write_text("fn ok() {}\n", encoding="utf-8")

        if check_no_residue(root) or check_resource(root) or check_manifest(root):
            print("❌ 自测：干净工作树被判红")
            return 1

        # ⓪ manifest.json 与代码不一致（漏跑导出）→ 红；since 越出版本 → 红。
        stale = json.loads((root / MANIFEST).read_text(encoding="utf-8"))
        stale["manifest_version"] = 1
        (root / MANIFEST).write_text(json.dumps(stale), encoding="utf-8")
        if not check_manifest(root):
            print("❌ 自测：过期的 manifest.json 没有被抓住")
            return 1
        (root / MANIFEST).write_text(
            json.dumps({**stale, "manifest_version": 2}), encoding="utf-8"
        )
        drifted = (root / "crates/luna-core/src/prompt.rs").read_text(encoding="utf-8")
        (root / "crates/luna-core/src/prompt.rs").write_text(
            drifted.replace('("style", 2)', '("style", 3)'), encoding="utf-8"
        )
        if not check_manifest(root):
            print("❌ 自测：since 越出 PROMPT_MANIFEST_VERSION 没有被抓住")
            return 1
        (root / "crates/luna-core/src/prompt.rs").write_text(drifted, encoding="utf-8")

        # ① 源码里出现提示词字面量 → 红。
        (root / "crates/luna-provider/src/llm_http.rs").write_text(
            'const S: &str = "You are a writing assistant.";\n', encoding="utf-8"
        )
        if not check_no_residue(root):
            print("❌ 自测：源码里的提示词字面量没有被抓住")
            return 1
        (root / "crates/luna-provider/src/llm_http.rs").write_text("fn ok() {}\n", encoding="utf-8")

        # ② 资源缺一个已登记的 key → 红。
        missing = json.loads(json.dumps(good))
        missing["sets"][1]["templates"] = {}
        resource.write_text(json.dumps(missing), encoding="utf-8")
        if not check_resource(root):
            print("❌ 自测：资源缺 key 没有被抓住")
            return 1

        # ③ 资源多出未登记的 key → 红。
        extra = json.loads(json.dumps(good))
        extra["sets"][1]["templates"]["surprise"] = "x"
        resource.write_text(json.dumps(extra), encoding="utf-8")
        if not check_resource(root):
            print("❌ 自测：资源里未登记的 key 没有被抓住")
            return 1

        # ③b 应用自带的模板没随包 → 红；应用模板不以应用 id 开头 → 红。
        no_app = json.loads(json.dumps(good))
        del no_app["sets"][1]["templates"]["creator.pitch"]
        resource.write_text(json.dumps(no_app), encoding="utf-8")
        if not check_resource(root):
            print("❌ 自测：应用自带模板没随包没有被抓住")
            return 1
        resource.write_text(json.dumps(good), encoding="utf-8")
        source = (root / "crates/luna-core/src/prompt.rs").read_text(encoding="utf-8")
        (root / "crates/luna-core/src/prompt.rs").write_text(
            source.replace('&["creator.pitch"]', '&["pitch"]'), encoding="utf-8"
        )
        unprefixed = json.loads(json.dumps(good))
        unprefixed["sets"][1]["templates"]["pitch"] = unprefixed["sets"][1]["templates"].pop("creator.pitch")
        resource.write_text(json.dumps(unprefixed), encoding="utf-8")
        if not check_resource(root):
            print("❌ 自测：不以应用 id 开头的应用模板没有被抓住")
            return 1
        (root / "crates/luna-core/src/prompt.rs").write_text(source, encoding="utf-8")
        resource.write_text(json.dumps(good), encoding="utf-8")

        # ④ 生产代码里出现 `Instruction::raw` → 红；测试模块里 → 放行。
        raw_prod = root / "crates/luna-core/src/translate.rs"
        raw_prod.write_text(
            'fn f() { let _ = Instruction::raw("x"); }\n#[cfg(test)]\nmod t { fn g() { let _ = Instruction::raw("y"); } }\n',
            encoding="utf-8",
        )
        if not check_raw_instruction(root):
            print("❌ 自测：生产代码里的 Instruction::raw 没有被抓住")
            return 1
        raw_prod.write_text(
            '#[cfg(test)]\nmod t { fn g() { let _ = Instruction::raw("y"); } }\n',
            encoding="utf-8",
        )
        if check_raw_instruction(root):
            print("❌ 自测：测试模块里的 Instruction::raw 被误判")
            return 1

    print("✅ 自测通过：八条断言都会在被违反时变红")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true", help="反向验证断言非空跑")
    args = parser.parse_args()
    if args.self_test:
        return self_test()

    problems = (
        check_no_residue(REPO)
        + check_resource(REPO)
        + check_manifest(REPO)
        + check_raw_instruction(REPO)
    )
    if problems:
        for problem in problems:
            print(problem)
        return 1
    print(
        "✅ 能力提示词唯一事实源：源码零残留，随包资源、manifest.json 与 REQUIRED_TEMPLATES 一致"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
