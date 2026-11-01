#!/usr/bin/env python3
"""成熟度轴（`luna.stage`）判定唯一事实源守护（ADR-086，TDD-06 §3/§5）。

ADR-086 把知识分层改成来源 × 成熟度双轴，而成熟度的缺省规则有**两条互不对称的分支**：
`quick-note` 无键 = 记录态，其余类型无键 = 知识态；再加一条「未知取值按记录态」。
六个判定面（树 / 图谱 / 打标 / 搜索 / 问答 / 主题页建议）都要用到这个结论。

**纯纪律守不住它**，与提示词那件事同构：

- 下一个人要在某个判定面加一句「顺便别把草稿算进来」，最省事的写法永远是就地
  `meta.note_type == QuickNote || fm.get("stage") == Some("draft")`——它编译得过、
  lint 不管、单测照绿；
- 而代价要到很久以后才现形：某天 ADR 改了缺省（比如给 `note` 加第二个合法值），
  收敛的那一处改了，散写的那几处没改，于是**同一篇笔记在图谱上是记录、在问答里是知识**。
  没有任何地方会报错，用户只会觉得"过滤开关有时候不管用"。

故本脚本回答三个机器事实：

- **A. 缺省规则只有一处**：判定文本（`NoteStage::resolve` 的函数体）只允许出现在
  `crates/luna-vault/src/frontmatter.rs`。任何别的 `.rs` / `.ts` / `.tsx` 里出现
  「按 `quick-note` 决定 stage 缺省」这一形状即判红。
- **B. `"draft"` 字面量零散写**：这个词是 frontmatter 取值、也是派生索引列的值，
  全仓只允许 `STAGE_DRAFT` 常量那一处（以及测试与本脚本）。前端更是一个字都不该有——
  前端拿到的是**已判定的布尔**（`draft: bool`），不是取值（ADR-003 前端零业务）。
- **C. 判据函数确实唯一**：`NoteStage::resolve` / `NoteStage::allows_auto_ai_tags`
  （按完整签名认，见 `UNIQUE_FN_DEFS`）在全仓各只有一处定义。
  打标白名单是本批最容易被复制的一条：它同时需要两根轴，而 `NoteType` 只知道一根，
  于是"在调用点补一句 && 非记录态"是最自然的错法。

**B 只认字符串字面量而不是词根**：拿 `draft` 去扫正文注释会把中文说明扫红，
于是下一个人就会去放宽规则；而放宽过的门禁是最贵的一种——它还在跑，只是不再拦得住。

用法：
    python3 scripts/check-note-stage-single-source.py              # 校验工作树
    python3 scripts/check-note-stage-single-source.py --self-test  # 自测（证明断言非空跑）
"""

from __future__ import annotations

import argparse
import re
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

# 唯一事实源：成熟度判定与 `STAGE_DRAFT` 常量都住在这里。
SOURCE = "crates/luna-vault/src/frontmatter.rs"

# 被扫描的源码面。
SCAN_GLOBS = (
    "crates/**/*.rs",
    "src-tauri/src/**/*.rs",
    "apps/desktop/src/**/*.ts",
    "apps/desktop/src/**/*.tsx",
)

# A：「按 quick-note 决定 stage 缺省」的形状——一行里同时出现速记类型与 stage/draft 概念。
# 收得很窄（同一行 + 两个概念同时在场）正是为了不误报：谈论速记的地方很多，
# 但把速记与成熟度写在同一个判断里的，只该有 `NoteStage::resolve`。
DEFAULT_RULE_RE = re.compile(
    r'^(?![^\n]*//).*(QuickNote|"quick-note").*(stage|Stage|STAGE_DRAFT|"draft").*$',
    re.M,
)

# B：`"draft"` 字符串字面量（Rust 与 TS 同一形状；TS 单引号一并认）。
DRAFT_LITERAL_RE = re.compile(r"""(?<![\w"'])(?:"draft"|'draft')(?![\w"'])""")

# C：判据函数的定义（Rust）。按**完整签名**匹配而不是函数名——`fn resolve(` 这种名字
# 全仓有七处（配置、模板、路由…），拿名字去数必然误报，而误报会逼下一个人放宽规则。
# 签名变了（哪怕只是改个参数名）本脚本会如实报「找不到定义，先更新本脚本」——
# 那正是想要的：判据搬了家，守护必须跟着搬，而不是安静地空跑。
UNIQUE_FN_DEFS = (
    "fn resolve(note_type: NoteType, raw: Option<&str>)",
    "fn allows_auto_ai_tags(self, note_type: NoteType)",
    # PRD-06-054：「生成知识笔记」白名单（资料 ∪ 记录）。蒸馏管线上有四道门都问这个
    # 问题（状态查询 / 入队前置 / worker 门槛 / 输入装配），而在调用点写
    # `is_source_material() || 记录态` 同样编译得过——四道门各写一遍，就是四处会漂。
    "fn allows_distill(self)",
    # PRD-06-058：「标记为记录」反向动作的白名单（只有知识态能降回记录态）。两个入口
    # （树行右键 / 编辑器右键）问同一个问题，而「在调用点补一句 && 非资料」会让同一篇
    # 笔记在两个菜单里能做的事不一样——与打标白名单同构的错法。
    "fn allows_mark_as_draft(self)",
)

# 测试块：`#[cfg(test)]` 之后的内容不参与 A/B（用例里写死取值是必要的——
# 判定矩阵这类测试本来就要逐个取值喂进去）。
CFG_TEST = "#[cfg(test)]"
TS_TEST_SUFFIXES = (".test.ts", ".test.tsx")


def production_part(rel: str, text: str) -> str:
    """一份源码里**不含测试**的那部分。"""
    if rel.endswith(TS_TEST_SUFFIXES):
        return ""
    head, _, _ = text.partition(CFG_TEST)
    return head


def sources(root: Path) -> list[tuple[str, str]]:
    out: list[tuple[str, str]] = []
    for pattern in SCAN_GLOBS:
        for path in sorted(root.glob(pattern)):
            try:
                out.append((path.relative_to(root).as_posix(), path.read_text(encoding="utf-8")))
            except (OSError, UnicodeDecodeError):
                continue
    return out


def check(files: list[tuple[str, str]]) -> list[str]:
    """返回违规说明；空列表 = 通过。纯函数，供 --self-test 直接喂样本。"""
    problems: list[str] = []
    fn_defs: dict[str, list[str]] = {name: [] for name in UNIQUE_FN_DEFS}

    for rel, text in files:
        body = production_part(rel, text)
        if rel != SOURCE:
            for match in DEFAULT_RULE_RE.finditer(body):
                problems.append(
                    f"❌ {rel} 里出现了成熟度缺省规则的形状：{match.group(0).strip()!r}\n"
                    f"   `luna.stage` 的缺省判定只有一处——{SOURCE} 的 `NoteStage::resolve`"
                    f"（ADR-086 / TDD-06 §5）。消费面只读它的结论（`is_draft` / `is_knowledge`"
                    f" / `notes.stage` 派生列），绝不各写一遍缺省。"
                )
            for match in DRAFT_LITERAL_RE.finditer(body):
                problems.append(
                    f"❌ {rel} 里出现 `\"draft\"` 字面量——它是 frontmatter 取值与派生索引列"
                    f"的值，唯一事实源是 {SOURCE} 的 `STAGE_DRAFT`。前端更是一个字都不该有："
                    f"下发到前端的是已判定的布尔 `draft`，不是取值（ADR-003）。"
                    f"（第 {body[: match.start()].count(chr(10)) + 1} 行）"
                )
        for name in UNIQUE_FN_DEFS:
            if name in body:
                fn_defs[name].append(rel)

    for name, where in fn_defs.items():
        if len(where) > 1:
            problems.append(
                f"❌ `{name}` 在多处定义：{where}——双轴判据必须只有一个实现。"
                f"打标白名单尤其危险：它同时需要 type 与 stage，而 `NoteType` 只知道前者，"
                f"于是「在调用点补一句 && 非记录态」是最自然、也最会漂移的错法。"
            )
        elif not where:
            problems.append(
                f"❌ 全仓找不到 `{name}` 的定义——形状变了（改名或搬家），先更新本脚本，"
                f"否则这条断言从此空跑。"
            )
        elif where[0] != SOURCE:
            problems.append(
                f"❌ `{name}` 定义在 {where[0]}，而它该住在 {SOURCE}（判定与 `type` 缺省同点）。"
            )
    return problems


def self_test() -> int:
    """反向验证：三条断言各配违规样本，且正确形状不得误报。"""
    good_source = (
        'pub const STAGE_DRAFT: &str = "draft";\n'
        "impl NoteStage {\n"
        "    pub fn resolve(note_type: NoteType, raw: Option<&str>) -> Self {\n"
        "        match note_type { NoteType::QuickNote => Self::Draft, _ => Self::Knowledge }\n"
        "    }\n"
        "    pub fn allows_auto_ai_tags(self, note_type: NoteType) -> bool { true }\n"
        "    pub fn allows_distill(self) -> bool { true }\n"
        "    pub fn allows_mark_as_draft(self) -> bool { true }\n"
        "}\n"
    )
    consumer_ok = "if meta.stage().is_draft() { return; }\n"

    cases: list[tuple[str, list[tuple[str, str]], int]] = [
        (
            "正确收敛不得误报",
            [(SOURCE, good_source), ("crates/luna-vault/src/index.rs", consumer_ok)],
            0,
        ),
        (
            "A：消费面自己写一遍缺省规则，必须红",
            [
                (SOURCE, good_source),
                (
                    "src-tauri/src/ipc/search.rs",
                    'let draft = t == NoteType::QuickNote || raw == Some("draft");\n',
                ),
            ],
            1,
        ),
        (
            "B：前端出现取值字面量，必须红",
            [
                (SOURCE, good_source),
                ("apps/desktop/src/features/search/SearchView.tsx", "n.stage === 'draft'\n"),
            ],
            1,
        ),
        (
            "B：注释里谈论 draft 不算（只认字符串字面量）",
            [
                (SOURCE, good_source),
                ("crates/luna-vault/src/graph.rs", "// 记录态（draft）只是事实，不过滤。\n"),
            ],
            0,
        ),
        (
            "B：测试里写死取值不算（判定矩阵本来就要逐个喂）",
            [
                (SOURCE, good_source),
                (
                    "crates/luna-vault/src/index.rs",
                    consumer_ok + '#[cfg(test)]\nmod t { const S: &str = "draft"; }\n',
                ),
            ],
            0,
        ),
        (
            "C：判据被复制到第二处，必须红",
            [
                (SOURCE, good_source),
                (
                    "src-tauri/src/ipc/graph.rs",
                    "fn allows_auto_ai_tags(self, note_type: NoteType) -> bool { true }\n",
                ),
            ],
            1,
        ),
        (
            "C：判据改名搬家导致断言空跑，必须红",
            [(SOURCE, 'pub const STAGE_DRAFT: &str = "draft";\n')],
            1,
        ),
    ]

    failed = 0
    for name, files, want in cases:
        got = len(check(files))
        ok = got >= 1 if want else got == 0
        failed += 0 if ok else 1
        print(f"  [{'OK ' if ok else 'FAIL'}] {name}（违规数 {got}，期望 {'≥1' if want else '0'}）")

    if failed:
        print(f"\n自测失败 {failed} 项——本脚本的断言有空跑的。", file=sys.stderr)
        return 1

    # 顺带证明扫描面真的能读到工作树（否则 A/B 会在一个空文件集上恒绿）。
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        (root / "crates/luna-vault/src").mkdir(parents=True)
        (root / SOURCE).write_text(good_source, encoding="utf-8")
        if not sources(root):
            print("❌ 自测：扫描面读不到任何文件", file=sys.stderr)
            return 1

    print("\n自测全过：三条断言各被对应样本触发，注释/测试豁免不误报，扫描面非空。")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true", help="反向验证断言非空跑")
    args = parser.parse_args()
    if args.self_test:
        return self_test()

    problems = check(sources(REPO))
    if problems:
        print("成熟度轴唯一事实源守护失败（ADR-086）：\n", file=sys.stderr)
        for problem in problems:
            print(f"  {problem}\n", file=sys.stderr)
        return 1
    print(
        f"✅ `luna.stage` 唯一事实源：缺省判定与 `STAGE_DRAFT` 只在 {SOURCE}，"
        f"消费面只读结论，判据函数各只有一处定义。"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
