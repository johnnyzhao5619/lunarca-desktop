#!/usr/bin/env python3
"""错误码族对齐门禁（TDD-15 §5 L10 的最后一支，AGENTS.md §3）。

Job 错误码 ↔ 渲染通道那一支由 `job-error-code-channels.test.ts` 从生成物判定；后端 Fluent
注册表那一支由 `ipc_error_copy` 扫源码判定。本脚本校验以下错误码族：

- **抓取**：`luna_fetch::FetchError::code()` → 前端 `lib/task-failure.ts` 的 `CODE_TO_KEY`
  → `transcribe:fetchError.<sub>`；
- **设置写入**：`luna_config::Rejection::code()` → `features/settings/settings-errors.ts` 的
  `CODE_TO_KEY` → `settings:<key>`；
- **Vault 存储**：`luna_vault::VaultError::code()` → `common:vaultError.<CODE>`（或
  `common:failure.<CODE>`，`vault-error.ts` 的 `lookup` 按此顺序问）。
- **云 IPC**：`CloudError` 的生成联合类型 → `lib/cloud-error.ts` → `cloud:error.<code>`。

Rust 给稳定码，前端按码取句子。任何一端加了一条、另一端没跟上，
现象都是"界面上出现一串 SCREAMING_SNAKE"——而这一类错只在真机上撞见。本脚本把它变成
会变红的东西，四条断言：

1) Rust `code()` 里的每一个码，前端映射表（或直接 i18n 表）都认识；
2) 前端映射到的每一个 i18n key 在 **zh-CN 与 en-US 两份** locale 里都存在；
3) 前端映射表里的每一个码在 Rust 源码里**真的出现过**（`crates/` + `src-tauri/src/`）
   ——否则是一条永远走不到的死映射；
4) 各族涉及的 i18n 节在两种语言里键集相同（少翻一条即红）。

用法：
    python3 scripts/check-error-code-families.py            # 校验工作树
    python3 scripts/check-error-code-families.py --self-test
"""

from __future__ import annotations

import json
import re
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
LOCALES = ("zh-CN", "en-US")
RUST_ROOTS = ("crates", "src-tauri/src")
CLOUD_BINDINGS = Path("apps/desktop/src/ipc/cloud-bindings.ts")

CODE_ARM = re.compile(r'=>\s*"([A-Z][A-Z0-9_]+)"')
TS_ENTRY = re.compile(r'^\s*([A-Z][A-Z0-9_]+)\s*:\s*"([^"]+)"', re.MULTILINE)

# (族名, Rust 文件, `fn code` 的锚点, 前端映射表文件或 None, i18n 命名空间, 前端子键前缀, 直接查表的前缀列表)
FAMILIES = (
    {
        "name": "fetch",
        "rust": "crates/luna-fetch/src/error.rs",
        "anchor": "pub const fn code(&self)",
        "ts": "apps/desktop/src/lib/task-failure.ts",
        "namespace": "transcribe",
        "prefix": "fetchError.",
        "sections": ("fetchError",),
    },
    {
        "name": "settings",
        "rust": "crates/luna-config/src/registry.rs",
        "anchor": "impl Rejection {",
        "ts": "apps/desktop/src/features/settings/settings-errors.ts",
        "namespace": "settings",
        "prefix": "",
        "sections": ("error",),
    },
    {
        "name": "vault",
        "rust": "crates/luna-vault/src/error.rs",
        "anchor": "pub fn code(&self)",
        "ts": None,
        "namespace": "common",
        "direct": ("vaultError.", "failure."),
        "sections": ("vaultError",),
    },
)


def rust_codes(text: str, anchor: str) -> set[str]:
    """锚点之后第一个 `fn code` 的 `match` 臂里的全部码。"""
    start = text.find(anchor)
    if start == -1:
        return set()
    body = text[start:]
    fn_at = body.find("fn code(")
    if fn_at == -1:
        return set()
    body = body[fn_at:]
    # 函数体在 4 空格缩进的 `}` 处结束（impl 内的方法）。
    end = body.find("\n    }\n")
    if end != -1:
        body = body[:end]
    return set(CODE_ARM.findall(body))


def ts_map(text: str) -> dict[str, str]:
    start = text.find("CODE_TO_KEY")
    if start == -1:
        return {}
    body = text[start:]
    end = body.find("};")
    if end != -1:
        body = body[:end]
    return dict(TS_ENTRY.findall(body))


def load_locale(root: Path, locale: str, namespace: str) -> dict | None:
    path = root / "apps/desktop/src/i18n/locales" / locale / f"{namespace}.json"
    if not path.is_file():
        return None
    return json.loads(path.read_text(encoding="utf-8"))


def key_exists(tree: dict | None, dotted: str) -> bool:
    node: object = tree
    for part in dotted.split("."):
        if not isinstance(node, dict) or part not in node:
            return False
        node = node[part]
    return isinstance(node, str)


def flat_keys(tree: dict | None, section: str) -> set[str]:
    node = (tree or {}).get(section)
    if not isinstance(node, dict):
        return set()
    out: set[str] = set()

    def walk(prefix: str, sub: dict) -> None:
        for k, v in sub.items():
            if isinstance(v, dict):
                walk(f"{prefix}{k}.", v)
            else:
                out.add(f"{prefix}{k}")

    walk("", node)
    return out


def rust_sources(root: Path) -> str:
    chunks: list[str] = []
    for rel in RUST_ROOTS:
        base = root / rel
        if not base.is_dir():
            continue
        for path in sorted(base.rglob("*.rs")):
            if "target" in path.parts:
                continue
            chunks.append(path.read_text(encoding="utf-8", errors="replace"))
    return "\n".join(chunks)


def check_cloud(root: Path) -> list[str]:
    bindings = root / CLOUD_BINDINGS
    if not bindings.is_file():
        return [f"[cloud] 缺生成绑定 {CLOUD_BINDINGS}"]
    _, _, cloud_error = bindings.read_text(encoding="utf-8").partition("export type CloudError =\n")
    codes = re.findall(r'^\{ code: "([a-z_]+)"', cloud_error.split("\nexport type ", 1)[0], re.MULTILINE)
    if not codes:
        return ["[cloud] 未解析到 CloudError 的错误码"]
    problems = []
    special = {"network": "network_kind", "login_rejected": "login_rejected", "link_rejected": "link_rejected"}
    keys = {}
    for loc in LOCALES:
        tree = load_locale(root, loc, "cloud")
        keys[loc] = flat_keys(tree, "error")
        for code in codes:
            present = (
                any(key.startswith(f"{special[code]}.") for key in keys[loc]) if code in special
                else key_exists(tree, f"error.{code}")
            )
            if not present:
                problems.append(f"[cloud] 码 {code} 在 {loc}/cloud.json 里没有句子")
    for key in sorted(keys["zh-CN"] ^ keys["en-US"]):
        problems.append(f"[cloud] cloud:error.{key} 在中英文案中不一致")
    return problems


def check(root: Path) -> list[str]:
    problems = check_cloud(root)
    all_rust = rust_sources(root)
    for fam in FAMILIES:
        name = fam["name"]
        rust_path = root / fam["rust"]
        if not rust_path.is_file():
            problems.append(f"[{name}] 缺 Rust 源 {fam['rust']}")
            continue
        codes = rust_codes(rust_path.read_text(encoding="utf-8"), fam["anchor"])
        if not codes:
            problems.append(f"[{name}] 在 {fam['rust']} 里没解析到任何错误码（锚点 {fam['anchor']!r}）")
            continue
        locales = {loc: load_locale(root, loc, fam["namespace"]) for loc in LOCALES}
        for loc, tree in locales.items():
            if tree is None:
                problems.append(f"[{name}] 缺 locale 文件 {loc}/{fam['namespace']}.json")
        if any(tree is None for tree in locales.values()):
            continue

        if fam["ts"] is None:
            # 直接查表族：每个码在两种语言里都要有一条句子。
            for code in sorted(codes):
                for loc, tree in locales.items():
                    if not any(key_exists(tree, f"{p}{code}") for p in fam["direct"]):
                        problems.append(
                            f"[{name}] 码 {code} 在 {loc}/{fam['namespace']}.json 里没有句子"
                            f"（{' / '.join(p + code for p in fam['direct'])}）"
                        )
        else:
            ts_path = root / fam["ts"]
            if not ts_path.is_file():
                problems.append(f"[{name}] 缺前端映射表 {fam['ts']}")
                continue
            mapping = ts_map(ts_path.read_text(encoding="utf-8"))
            if not mapping:
                problems.append(f"[{name}] 在 {fam['ts']} 里没解析到 CODE_TO_KEY")
                continue
            for code in sorted(codes - set(mapping)):
                problems.append(f"[{name}] Rust 码 {code} 在 {fam['ts']} 的 CODE_TO_KEY 里没有映射")
            for code, sub in sorted(mapping.items()):
                dotted = f"{fam['prefix']}{sub}"
                for loc, tree in locales.items():
                    if not key_exists(tree, dotted):
                        problems.append(
                            f"[{name}] 前端把 {code} 映到 {fam['namespace']}:{dotted}，"
                            f"但 {loc}/{fam['namespace']}.json 里没有它"
                        )
                if re.search(rf"\b{re.escape(code)}\b", all_rust) is None:
                    problems.append(f"[{name}] 前端映射表里的 {code} 在 Rust 源码里从未出现（死映射）")

        for section in fam["sections"]:
            sets = {loc: flat_keys(tree, section) for loc, tree in locales.items()}
            zh, en = sets[LOCALES[0]], sets[LOCALES[1]]
            for key in sorted(zh - en):
                problems.append(f"[{name}] {fam['namespace']}:{section}.{key} 只有 {LOCALES[0]} 有")
            for key in sorted(en - zh):
                problems.append(f"[{name}] {fam['namespace']}:{section}.{key} 只有 {LOCALES[1]} 有")
    return problems


# ─── 自测 ───────────────────────────────────────────────────────────────────────

FIXTURE_RUST_FETCH = '''
impl FetchError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::A => "FETCH_A",
            Self::B => "FETCH_B",
        }
    }
    pub const fn other(&self) -> bool { true }
}
'''
FIXTURE_RUST_SETTINGS = '''
impl Rejection {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Type { .. } => "SETTING_TYPE",
        }
    }
}
'''
FIXTURE_RUST_VAULT = '''
impl VaultError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound(_) => "VAULT_NOT_FOUND",
        }
    }
}
'''
FIXTURE_TS_FETCH = '''
const CODE_TO_KEY: Record<string, string> = {
  FETCH_A: "a",
  FETCH_B: "b",
};
'''
FIXTURE_TS_SETTINGS = '''
const CODE_TO_KEY: Record<string, string> = {
  SETTING_TYPE: "error.type",
};
'''


def _write_fixture(root: Path, *, drop_ts_b: bool = False, drop_en: bool = False, dead: bool = False) -> None:
    (root / "crates/luna-fetch/src").mkdir(parents=True)
    (root / "crates/luna-config/src").mkdir(parents=True)
    (root / "crates/luna-vault/src").mkdir(parents=True)
    (root / "apps/desktop/src/lib").mkdir(parents=True)
    (root / "apps/desktop/src/features/settings").mkdir(parents=True)
    (root / CLOUD_BINDINGS.parent).mkdir(parents=True)
    (root / CLOUD_BINDINGS).write_text('export type CloudError =\n{ code: "web_access_unavailable" };\n', encoding="utf-8")
    (root / "crates/luna-fetch/src/error.rs").write_text(FIXTURE_RUST_FETCH, encoding="utf-8")
    (root / "crates/luna-config/src/registry.rs").write_text(FIXTURE_RUST_SETTINGS, encoding="utf-8")
    (root / "crates/luna-vault/src/error.rs").write_text(FIXTURE_RUST_VAULT, encoding="utf-8")
    ts_fetch = FIXTURE_TS_FETCH
    if drop_ts_b:
        ts_fetch = ts_fetch.replace('  FETCH_B: "b",\n', "")
    if dead:
        ts_fetch = ts_fetch.replace("};", '  FETCH_GHOST: "a",\n};')
    (root / "apps/desktop/src/lib/task-failure.ts").write_text(ts_fetch, encoding="utf-8")
    (root / "apps/desktop/src/features/settings/settings-errors.ts").write_text(FIXTURE_TS_SETTINGS, encoding="utf-8")
    for loc in LOCALES:
        d = root / "apps/desktop/src/i18n/locales" / loc
        d.mkdir(parents=True)
        fetch = {"fetchError": {"a": "A", "b": "B"}}
        if drop_en and loc == "en-US":
            fetch = {"fetchError": {"a": "A"}}
        (d / "transcribe.json").write_text(json.dumps(fetch), encoding="utf-8")
        (d / "settings.json").write_text(json.dumps({"error": {"type": "T"}}), encoding="utf-8")
        (d / "common.json").write_text(json.dumps({"vaultError": {"VAULT_NOT_FOUND": "nf"}, "failure": {}}), encoding="utf-8")
        (d / "cloud.json").write_text(json.dumps({"error": {"web_access_unavailable": "unavailable"}}), encoding="utf-8")


def self_test() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp) / "ok"
        _write_fixture(root)
        assert check(root) == [], check(root)
        for loc in LOCALES:
            path = root / "apps/desktop/src/i18n/locales" / loc / "cloud.json"
            path.write_text('{"error": {}}', encoding="utf-8")
        assert len(check_cloud(root)) == 2, check_cloud(root)

        root = Path(tmp) / "missing-map"
        _write_fixture(root, drop_ts_b=True)
        problems = check(root)
        assert any("FETCH_B" in p and "没有映射" in p for p in problems), problems

        root = Path(tmp) / "missing-en"
        _write_fixture(root, drop_en=True)
        problems = check(root)
        assert any("en-US/transcribe.json" in p for p in problems), problems
        assert any("只有 zh-CN 有" in p for p in problems), problems

        root = Path(tmp) / "dead"
        _write_fixture(root, dead=True)
        problems = check(root)
        assert any("FETCH_GHOST" in p and "死映射" in p for p in problems), problems
    print("check-error-code-families 自测通过。")


def main(argv: list[str]) -> int:
    if "--self-test" in argv:
        self_test()
        return 0
    problems = check(REPO)
    if problems:
        print("错误码族对齐门禁失败：", file=sys.stderr)
        for line in problems:
            print(f"  - {line}", file=sys.stderr)
        return 1
    print("错误码族对齐 OK — 抓取 / 设置写入 / Vault 存储 / 云 IPC 与 zh-CN / en-US 文案对齐。")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
