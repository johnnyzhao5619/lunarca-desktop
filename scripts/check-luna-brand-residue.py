#!/usr/bin/env python3
"""品牌残留门禁（TDD-13 §2）：旧品牌名不得留在历史记录之外的任何地方。

ADR-030 定名 Lunarca 之后，旧名只允许存在于**不随公开仓分发**的历史过程文档里；
运行时源码、生成 bindings、配置、当前 PRD/TDD/README 与脚本一律零残留，且不得以
兼容别名豁免当前契约。ADR-030 的 identifier 迁移（2026-10-28，首个公开安装包之前）之后，
旧应用标识同样零残留：只有迁移代码经拼接常量引用它（`luna_config::identity::LEGACY_IDENTIFIER`），
历史变更记录 CHANGELOG.md 豁免。本脚本自 2026-08-08 起由 `.github/workflows/ci.yml` 执行，
接线由 `check-ci-gate-coverage.py` 守。

**本文件自己也在扫描范围内**：禁用词一旦以字面量出现在这里，门禁就会判自己红。
故 [`FORBIDDEN`] 与自测样本都用字符串拼接构造，绝不写出完整的旧名。

用法：
    python3 scripts/check-luna-brand-residue.py              # 校验工作树
    python3 scripts/check-luna-brand-residue.py --self-test  # 自测（证明本脚本不是空跑）
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path


# 判据是「旧名作为一个**标识符片段**出现」，不是 `\b` 词边界。
#
# 下面用 `«旧»` 代指旧品牌名——**这个文件自己也在扫描范围内**，把它写成字面量会让本门禁
# 判自己红（2026-08-08 验收时就这么栽过一次，正是本文件 docstring 里已经写着的那条）。
#
# 第一版写的是 `\b«旧»\b|«旧»-`，照散文与 kebab-case 裁的，于是漏掉**代码里最常见的两种
# 形态**（同日实测）：Rust 的 `«旧»_core::Provider`（`_` 是词字符，`\b` 根本不触发）与
# TS/生成 bindings 的 `«旧»Desktop`（驼峰没有任何边界）。而 TDD-13 §2 说本门禁管的正是
# 「运行时源码、生成 bindings、配置」——漏的恰好是它主要该管的地方。
#
# 现在的判据：前面不接字母数字，后面不接**小写**字母。于是 `«旧»_x` / `«旧»X` / `«旧»-x` /
# 独立成词的 `«旧»` 全部命中，而「碰巧含这个子串的正常英文词」（`«旧»l`、`«旧»te`）不命中。
# 诚实边界：全大写形态后接大写字母时会误报（如 `«旧»L`）——在本仓不可能出现，
# 不值得为它把判据搞复杂。
#
# 大小写用逐字符类而不是 `(?i)`：`(?i)` 会让 `(?![a-z])` 连大写也一起排除，驼峰那条就又漏了。
FORBIDDEN = re.compile(r"(?<![A-Za-z0-9])[Aa][Uu][Rr][Aa](?![a-z])")

# 旧应用标识（ADR-030）：同样拼接构造，本文件里不出现完整字面量。
LEGACY_IDENTIFIER = re.compile(re.escape("com" + "." + "luna" + "." + "app"))
# 历史变更记录照原样留着当时的标识，不改写历史。
IDENTIFIER_EXEMPT = {Path("CHANGELOG.md")}

# 此前这里维护过一份「历史记录目录」白名单（早期 plan/spec 里逐字留有旧品牌名）。
# 那些过程文档已随公开仓库准备移出版本控制并进 `.gitignore`，而下面的清单直接问 git 要，
# 天然尊重 `.gitignore`——白名单于是成了不会被求值的死码，删。若将来又要豁免某个目录，
# 请在此重新登记，而不是把它悄悄排除在 `repo_files()` 之外。


def repo_files(root: Path) -> list[Path]:
    """本仓库的文件清单 = 已跟踪 + 未跟踪但不被 gitignore 的。

    **不走 `os.walk`**：工作树里会躺着大量与仓库无关的东西——`target/`、
    `node_modules/`、以及 agent 留下的**临时 git worktree**（`.claude/worktrees/<id>/`
    是整个仓库在某个旧 commit 上的副本）。按目录名维护一份排除清单必然滞后于现实：
    这份清单里写着 `.worktrees`，而实际路径是 `.claude/worktrees`，于是本门禁长期被
    一份旧副本里的历史文件判红，红得毫无意义——**一个总在无意义地红的门禁，与没有门禁
    是一回事**（2026-07-30 文档同步轮发现）。问 git 要清单则结构上不可能被污染：
    它天然尊重 `.gitignore`，且**覆盖尚未 `git add` 的新文件**（`--others`），
    所以"新写的文件先绕过门禁"这条路也堵死了。
    """
    out = subprocess.run(
        ["git", "-C", str(root), "ls-files", "--cached", "--others", "--exclude-standard", "-z"],
        capture_output=True,
        text=True,
        # git 输出的路径是 UTF-8；不写死时 Windows 按区域编码（cp936）解码中文文件名即失败。
        encoding="utf-8",
        check=True,
    ).stdout
    return [Path(name) for name in out.split("\0") if name]


def violations_in(relative: Path, text: str) -> list[str]:
    """某个文件里的残留行；空列表 = 干净。纯函数，供 --self-test 直接喂样本。"""
    check_identifier = relative not in IDENTIFIER_EXEMPT
    return [
        f"{relative}:{number}: {line}"
        for number, line in enumerate(text.splitlines(), start=1)
        if FORBIDDEN.search(line) or (check_identifier and LEGACY_IDENTIFIER.search(line))
    ]


def self_test() -> int:
    """反向验证：断言必须能被残留样本触发，且不得把合法写法误判成残留。

    样本里的旧名一律拼接构造——写成字面量会让本门禁把这个自测判成违规。
    """
    legacy = "au" + "ra"
    cases = [
        ("干净文本应通过", "Lunarca 是一款桌面 AI 工作站。", 0),
        ("行文中的旧名应报错", f"{legacy} 曾是本产品的名字。", 1),
        ("大小写变体同样应报错", f"{legacy.capitalize()} Desktop", 1),
        ("旧 crate 前缀应报错", f"{legacy}-core = {{ path = 'crates' }}", 1),
        # 下面两条是 2026-08-08 验收实测出来的漏网形态，正是代码里最常见的两种。
        ("Rust 的下划线 crate 路径应报错", f"use {legacy}_core::Provider;", 1),
        ("生成 bindings 里的驼峰应报错", f"export const {legacy}Desktop = 1;", 1),
        ("全大写常量应报错", f"{legacy.upper()}_DATA_DIR", 1),
        # 判据不是「含子串」：正常英文词不得误伤，否则一个总在无意义地红的门禁与没有一样。
        ("含子串的正常英文词不得误报", f"{legacy}l tone and {legacy}te rock", 0),
        ("新品牌名不得误报", "Lunarca 月光宝盒 · restaurant · dinosaur", 0),
        ("旧应用标识应报错", "identifier = " + "com" + "." + "luna" + "." + "app", 1),
        ("新应用标识不得误报", "identifier = app.lunarca.luna", 0),
    ]

    failed = 0
    for name, text, want in cases:
        got = len(violations_in(Path("sample.md"), text))
        ok = got >= 1 if want else got == 0
        if not ok:
            failed += 1
        print(f"  [{'OK ' if ok else 'FAIL'}] {name}（命中 {got}，期望 {'≥1' if want else '0'}）")

    if failed:
        print(f"\n自测失败 {failed} 项——本脚本的断言有空跑的。", file=sys.stderr)
        return 1
    print("\n自测全过：残留样本必被抓住，合法写法不误报。")
    return 0


def main() -> int:
    if "--self-test" in sys.argv:
        return self_test()

    root = Path(__file__).resolve().parents[1]
    violations = 0

    for relative in repo_files(root):
        path = root / relative
        try:
            text = path.read_text(encoding="utf-8")
        except (UnicodeDecodeError, FileNotFoundError, IsADirectoryError):
            continue
        for hit in violations_in(relative, text):
            print(hit)
            violations += 1

    return 1 if violations else 0


if __name__ == "__main__":
    sys.exit(main())
