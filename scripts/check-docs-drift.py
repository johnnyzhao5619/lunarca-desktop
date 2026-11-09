#!/usr/bin/env python3
"""文档漂移守护（AGENTS.md §6「状态记录分层」的第二道门）。

姊妹脚本 `check-docs-context-budget.py` 管的是**体积**（每会话必加载的文件别涨）。
这一条管的是**形状**——三类只靠自觉就一定会复发的漂移，各配一条断言：

1) **表格被块引用撕开**。`docs/ROADMAP.md` 里曾有一条 `>` 开头的补充说明夹在两个表格
   行之间。按 GFM 规则表格到此终止，**它后面的六行纵切全部掉出表格**，在 GitHub 上
   渲染成带一堆竖线的散文；同一处还把两行挤进了一行，纵切 3 因此没有自己的行。
   本地 grep 看不出来，写的人也看不出来——只有渲染出来才看得见，所以必须机器管。
   判据刻意只抓「**表格中间**混入非表格行」：表格末尾紧跟一段正文是无害的常见写法
   （TDD-03/10/15 都这么写），抓它只会制造假警报。

2) 路线图「不得回填实现流水」那一组指纹（commit 链接 / 全文体量 / 单元格体量 / 带日期的
   落地条目数）已随路线图迁到私有商业仓的 `scripts/check-spec-drift.py`——规格与路线图不再
   随本仓分发。

3) **可从代码库重建的计数写进了每会话必加载的文件**。`CLAUDE.md` 与 `AGENTS.md` 曾都写
   「11 个 `crates/luna-*`」，而 `luna-cloud` 迁入后实际是 12——两处同时陈旧，且没有任何
   东西会因此变红。本条不禁止写数字，只要求**写了就必须与工作树一致**；真嫌麻烦，
   按 AGENTS.md §6 的判据把它删掉即可（能删掉的数字就别写，它本身就是下一个漂移源）。

4) **CHANGELOG 同一版本里出现重复的分类标题**。Keep a Changelog 的一个版本下每类
   （新增/变更/修复/…）只该有一节。`## [未发布]` 曾同时有两节 `### 变更` 和两节
   `### 修复`——按批次往下追加时，谁都不会先翻到上面去看已经有没有那一节，而 Markdown
   渲染出来是两个一模一样的小标题，读者无从判断哪一节是全的。这与 2) 是同一类病
   （按时间追加而不收口），只是发生在另一个文件上；同样看不见，同样只能靠会变红的东西
   发现。判据只管**同一版本节内**：跨版本重复是格式本身要求的。

用法：
    python3 scripts/check-docs-drift.py              # 校验工作树
    python3 scripts/check-docs-drift.py --self-test  # 自测（证明本脚本不是空跑）
"""

import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

CHANGELOG_MD = "CHANGELOG.md"
ALWAYS_LOADED = ("CLAUDE.md", "AGENTS.md", "docs/agents/coding/hard-rules.md")

# 表格结构在**所有** Markdown 上校验：断裂是渲染事故，与文件是谁无关。
TABLE_GLOBS = ("*.md", "docs/**/*.md")

# 「N 个 `crates/luna-*`」形状的计数（带或不带反引号都算）。
CRATE_COUNT_RE = re.compile(r"(\d+)\s*个\s*`?crates/luna-\*`?")

SEPARATOR_CHARS = set("-: ")

# 指向**不随公开仓分发**的内部过程文档（实现计划 / 逐批验收证据 / 真机作业单）的引用。
# CLAUDE.md 已定下处理办法：规格里引用它们的地方降级为具名纯文本「…（内部记录）」。
# 代码注释此前不受这条约束，于是 ADR-087 重组一次性写进 33 处形如「«计»§6 批次 5 ③」的
# 章节号——那份文档不在工作树里、不随 AGPL 公开仓分发，且自身有 v1.0→v1.4 的版本变迁：
# 读者查不到，而它一改版，同一个编号指向的就是别的东西。事实源只能是 PRD / TDD / ADR。
#
# **本文件自己也在扫描范围内**（口径同 `check-luna-brand-residue.py`）：判据一旦以字面量
# 出现在这里，门禁就判自己红。故禁用形态与自测样本一律**拼接**构造，本文件里绝不写出完整
# 形态；上面用 `«计»` 代指那个词。自测里的 `_plan_ref()` 是同一个拼接器，样本因此与判据同源
# ——判据改了而样本没跟上，自测会当场变红，不会留下一条恒绿的假守护。
_PLAN_WORD = "计" + "划"
_PRIVATE_DIR = ".co" + "dex/visualizations"

PRIVATE_PLAN_RES = (
    re.compile(_PLAN_WORD + r"\s*§"),
    re.compile(re.escape(_PRIVATE_DIR)),
)




def table_problems(path: str, text: str) -> list[str]:
    """表格块内是否被非表格行撕开、或数据行列数与表头不一致。"""
    problems: list[str] = []
    lines = text.split("\n")
    i = 0
    while i < len(lines):
        is_header = (
            lines[i].startswith("|")
            and i + 1 < len(lines)
            and lines[i + 1].startswith("|")
            and set(lines[i + 1]) <= SEPARATOR_CHARS | {"|"}
        )
        if not is_header:
            i += 1
            continue

        head_pipes = lines[i].count("|")
        header_line = i + 1  # 1-based，报错用
        j = i + 2
        pending: list[str] = []  # 表格「中间」的非表格行：后面还有表格行才算撕开
        while j < len(lines) and lines[j].strip():
            line = lines[j]
            if not line.startswith("|"):
                pending.append(
                    f"{path}: 第 {j + 1} 行是非表格行，却夹在表格中间"
                    f"（表头在第 {header_line} 行）——它后面的表格行会全部掉出表格。"
                    f"把补充说明移到表格**之后**，或并进单元格里。"
                )
                j += 1
                continue
            if pending:
                # 非表格行之后又出现表格行 → 确实把表格撕成了两段。
                problems.extend(pending)
                pending.clear()
            if line.count("|") != head_pipes:
                problems.append(
                    f"{path}: 第 {j + 1} 行有 {line.count('|')} 个 `|`，表头（第 {header_line} 行）"
                    f"有 {head_pipes} 个——该行会少一列或多一列。"
                )
            j += 1
        i = j
    return problems


def changelog_problems(text: str) -> list[str]:
    """同一版本节里出现重复的 `### 分类` 标题即违规（Keep a Changelog 每类一节）。

    只在版本节（`## [...]`）**内部**判重：跨版本当然会重复，那是格式本身要求的。
    """
    problems: list[str] = []
    version = "（文件开头，尚未进入任何版本节）"
    seen: dict[str, int] = {}
    for line in text.splitlines():
        if line.startswith("## "):
            version = line[3:].strip()
            seen = {}
            continue
        if not line.startswith("### "):
            continue
        section = line[4:].strip()
        seen[section] = seen.get(section, 0) + 1
        if seen[section] == 2:
            problems.append(
                f"{CHANGELOG_MD} 的「{version}」里有两节「{section}」。"
                f"Keep a Changelog 每个版本每类只该有一节——渲染出来是两个一模一样的小标题，"
                f"读者无从判断哪一节是全的。把后来那节的条目并进前一节。"
            )
    return problems


def private_plan_problems(path: str, text: str) -> list[str]:
    """引用了不随仓分发的内部过程文档（章节号或其绝对路径）。"""
    problems: list[str] = []
    for number, line in enumerate(text.split("\n"), start=1):
        for pattern in PRIVATE_PLAN_RES:
            hit = pattern.search(line)
            if hit is None:
                continue
            problems.append(
                f"{path}:{number} 引用了不随公开仓分发的内部过程文档（{hit.group(0)!r}）——"
                f"读者在工作树里找不到它，而那份文档自己还会改版，同一个编号迟早指向别的"
                f"东西。事实源只能是 PRD / TDD / ADR 编号；确实只有内部记录才有的论断，"
                f"按 CLAUDE.md 的既有写法降级为具名纯文本「…（内部记录）」。"
            )
    return problems


def check(
    docs: dict[str, str],
    crate_count: int,
    sources: dict[str, str] | None = None,
) -> list[str]:
    """返回违规说明列表；空列表 = 通过。纯函数，供 --self-test 直接喂样本。"""
    problems: list[str] = []

    for path, text in sorted(docs.items()):
        problems.extend(table_problems(path, text))
        problems.extend(private_plan_problems(path, text))

    # 源码注释与文档受同一条约束：读者查不到的引用，写在 .ts 里不会比写在 .md 里更好。
    for path, text in sorted((sources or {}).items()):
        problems.extend(private_plan_problems(path, text))

    changelog = docs.get(CHANGELOG_MD)
    if changelog is not None:
        problems.extend(changelog_problems(changelog))

    for path in ALWAYS_LOADED:
        text = docs.get(path)
        if text is None:
            continue
        for m in CRATE_COUNT_RE.finditer(text):
            written = int(m.group(1))
            if written != crate_count:
                problems.append(
                    f"{path} 写着「{m.group(0)}」，工作树实际是 {crate_count} 个。"
                    f"这类能从代码库重建的计数一旦写进每会话必加载的文件就必然陈旧——"
                    f"改对它，或按 AGENTS.md §6 直接把数字删掉。"
                )

    return problems


def collect_docs() -> dict[str, str]:
    docs: dict[str, str] = {}
    for pattern in TABLE_GLOBS:
        for p in REPO.glob(pattern):
            if p.is_file():
                docs[p.relative_to(REPO).as_posix()] = p.read_text(encoding="utf-8")
    return docs


# 注释里同样会写引用，故扫描面是源码本身而不只是文档。`bindings.ts` 是生成物，
# 它脏就说明 Rust 侧脏或绑定没重生成——两种都该报。
SOURCE_GLOBS = (
    "apps/desktop/src/**/*.ts",
    "apps/desktop/src/**/*.tsx",
    "apps/desktop/src/**/*.css",
    "crates/**/*.rs",
    "src-tauri/**/*.rs",
    "scripts/*.py",
)


def collect_sources() -> dict[str, str]:
    sources: dict[str, str] = {}
    for pattern in SOURCE_GLOBS:
        for p in REPO.glob(pattern):
            if p.is_file() and "node_modules" not in p.parts and "target" not in p.parts:
                sources[p.relative_to(REPO).as_posix()] = p.read_text(encoding="utf-8")
    return sources


def count_crates() -> int:
    return len([p for p in (REPO / "crates").glob("luna-*") if p.is_dir()])


def _plan_ref() -> str:
    """自测用：拼出一条会被 `PRIVATE_PLAN_RES` 命中的引用（本文件不写字面量）。"""
    return _PLAN_WORD + " §6 批次 5 ③"


def self_test() -> int:
    """反向验证：每条断言都必须能被对应的违规样本触发，否则这个脚本是在空跑。"""
    ok_table = "| a | b |\n| --- | --- |\n| 1 | 2 |\n| 3 | 4 |\n"
    # 表格**末尾**接一段正文：常见且无害，不得报错。
    ok_trailing = ok_table + "> 补充说明放在表格之后是可以的。\n"
    torn = "| a | b |\n| --- | --- |\n| 1 | 2 |\n> 夹在中间的补充说明\n| 3 | 4 |\n"
    short_row = "| a | b |\n| --- | --- |\n| 1 | 2 |\n| 3 |\n"

    cases = [
        ("基线应通过", {"README.md": ok_table, "CLAUDE.md": "无计数"}, 12, 0),
        ("表格末尾接正文不得误报", {"README.md": ok_trailing}, 12, 0),
        ("表格被块引用撕开应报错", {"README.md": torn}, 12, 1),
        ("数据行少一列应报错", {"README.md": short_row}, 12, 1),
        (
            "文档引用不随仓分发的实现计划应报错",
            {"README.md": ok_table + f"判据见{_plan_ref()}。\n"},
            12,
            1,
        ),
        (
            "文档写死内部计划的绝对路径应报错",
            {"README.md": ok_table + f"见 ~/{_PRIVATE_DIR}/2026/08/25/plan.md。\n"},
            12,
            1,
        ),
        (
            "「（内部记录）」这一既有写法不得误报",
            {"README.md": ok_table + "逐批验收证据见验收清单 §〇（内部记录）。\n"},
            12,
            0,
        ),
        # CHANGELOG 同版本重复分类：真实发生过（`## [未发布]` 同时有两节「变更」两节「修复」）。
        (
            "CHANGELOG 同一版本里两节「变更」应报错",
            {CHANGELOG_MD: "## [未发布]\n\n### 修复\n\n- a\n\n### 变更\n\n- b\n\n### 变更\n\n- c\n"},
            12,
            1,
        ),
        (
            "跨版本重复分类是格式本身要求的，不得误报",
            {CHANGELOG_MD: "## [未发布]\n\n### 变更\n\n- a\n\n## [1.0.0]\n\n### 变更\n\n- b\n"},
            12,
            0,
        ),
        ("陈旧的 crates 计数应报错", {"CLAUDE.md": "含 11 个 `crates/luna-*`。"}, 12, 1),
        ("正确的 crates 计数应通过", {"CLAUDE.md": "含 12 个 `crates/luna-*`。"}, 12, 0),
        # 计数只管每会话必加载的文件；别处写数字不是本门禁的事。
        ("非 always-loaded 文件的计数不管", {"CONTRIBUTING.md": "11 个 `crates/luna-*`"}, 12, 0),
    ]

    failed = 0
    for name, docs, crates, want in cases:
        got = len(check(docs, crates))
        ok = got >= 1 if want else got == 0
        if not ok:
            failed += 1
        print(f"  [{'OK ' if ok else 'FAIL'}] {name}（违规数 {got}，期望 {'≥1' if want else '0'}）")

    if failed:
        print(f"\n自测失败 {failed} 项——本脚本的断言有空跑的。", file=sys.stderr)
        return 1
    print("\n自测全过：每条断言都已被对应违规样本触发，且无害写法不误报。")
    return 0


def main() -> int:
    if "--self-test" in sys.argv:
        return self_test()

    docs = collect_docs()
    sources = collect_sources()
    problems = check(docs, count_crates(), sources)

    if problems:
        print("文档漂移守护失败（AGENTS.md §6「状态记录分层」）：\n", file=sys.stderr)
        for p in problems:
            print(f"  ✗ {p}\n", file=sys.stderr)
        return 1

    print(
        f"文档漂移守护 OK — 已检查 {len(docs)} 个 Markdown 的表格结构、"
        f"{CHANGELOG_MD} 的同版本分类唯一性、always-loaded 文件里的 crates 计数，"
        f"以及 {len(sources)} 个源码文件 + 全部文档里对内部过程文档的引用。"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
