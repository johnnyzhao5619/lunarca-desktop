#!/usr/bin/env python3
"""领域错误的 `Display` 不得含中文（TDD-15 §5 L10 的第二条判定面，[ADR-101]）。

# 它补的是哪个洞

`src-tauri/src/ipc_error_copy.rs` 那道门禁扫的是 **IPC 层源码里的字符串字面量**：
`format!("CODE: 中文")` 会被它抓住。但中文进 IPC 返回串还有第二条路，而那条路它一眼都
看不见——**把领域错误的 `Display` 拼进去**：

    pub(crate) fn vault_err(e: luna_vault::VaultError) -> String { e.to_string() }
    JobEvent::Failed(err) => return Err(format!("{}: {err}", err.code())),

前者是全仓最高频的错误转换之一（数百个调用点），后者散在几条 AI 管线上。两处的字面量里
一个汉字都没有，而流到英文界面的用户手上的，是 `#[error("笔记不存在：{0}")]` 里的那句中文。
**判定面在字面量上，泄漏在类型上**——这正是「凡是没打算翻译的，都不会被发现没翻译」在
另一个方向上的复现。

# 判据

全仓（`crates/luna-*/src` + `src-tauri/src`）的 `#[error("…")]` 属性文案不得含 CJK。

**不是"把错误信息翻译成英文"，而是改变它的受众**：`thiserror` 的 `Display` 在 Rust
生态里就是**诊断串**——给日志、给 `{err:?}`、给运维看的。给用户的那半句一律由稳定错误码
经 i18n 产生（`ipc::coded_error` + `resources/i18n/*.ftl`，见 ADR-100）。两者混为一谈时，
一个字符串要同时满足"可被机器聚类""对开发者精确""对用户可读""随界面语言变"，而它做不到。

# 无豁免（欠账已清零，2026-08-31 批次 K）

这道门禁曾带着一份只减不增的欠账台账（`scripts/error-display-debt.txt` + `MAX_DEBT`）：
86 → 73（ADR-105 迁走 `VaultError` 13 条）→ 0。归零的那一刻台账与常量**一并删除**，
判据转为「一句都不许有」——而**不是**留一个空清单当过渡（ADR-046 不留兼容层）。理由与
`ipc_error_copy` 那道门禁逐字同源：一个恒空的豁免口是在邀请下一个人往里加第一行，
而那一行不会有人反对，因为「反正机制本来就在」。

最后那 21 条（`JobError`）能清掉，靠的是先修好三处**把 `Display` 拼进 IPC 返回串**的
消费面（ADR-101 ② 点名的障碍，见 ADR-108），而不是靠纪律。

用法：
    python3 scripts/check-error-display-locale.py              # 校验工作树
    python3 scripts/check-error-display-locale.py --self-test  # 自测（证明断言非空跑）
"""

from __future__ import annotations

import argparse
import re
import sys
import tempfile
from collections import Counter
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent


# 扫描面。前端不在内（它没有 `#[error]`）；`target/` 与 `node_modules/` 天然不在这两棵树里。
SOURCE_ROOTS = ("crates", "src-tauri/src")

# `#[error("…")]`：thiserror 的 Display 属性。只认单行形态——多行属性在本仓不存在，
# 出现了会**扫不到**（漏扫方向），故下面另有"扫描面非空"的空跑防护把它兜住。
ERROR_ATTR = re.compile(r'#\[error\("((?:[^"\\]|\\.)*)"')

# 判定面比「基本区汉字」宽一档，与 `ipc_error_copy::is_cjk` 逐字同源：`「」。、` 与全角
# `！？（）` 同样是只有中文界面才该出现的字形，只锁汉字会让一行残留了中文排版的英文串溜过去。
CJK = re.compile(
    "["
    "　-〿"  # CJK 标点：。、「」《》
    "㐀-䶿"  # 汉字扩展 A
    "一-鿿"  # 汉字基本区
    "豈-﫿"  # 兼容汉字
    "＀-￯"  # 全角形式：！？（）
    "]"
)


def sources(root: Path) -> list[Path]:
    """扫描面里的全部 `.rs`（排序，保证报错顺序稳定）。"""
    found: list[Path] = []
    for rel in SOURCE_ROOTS:
        base = root / rel
        if base.is_dir():
            found.extend(p for p in base.rglob("*.rs") if p.is_file())
    return sorted(found)


def violations(root: Path, paths: list[Path]) -> Counter[tuple[str, str]]:
    """`(相对路径, Display 文案) -> 条数`。纯函数，供 `--self-test` 直接喂样本。"""
    tally: Counter[tuple[str, str]] = Counter()
    for path in paths:
        text = path.read_text(encoding="utf-8")
        for message in ERROR_ATTR.findall(text):
            if CJK.search(message):
                tally[(path.relative_to(root).as_posix(), message)] += 1
    return tally


def scanned_attrs(paths: list[Path]) -> int:
    """扫到的 `#[error]` 总数（只用来证明扫描器不是在空跑）。"""
    return sum(len(ERROR_ATTR.findall(p.read_text(encoding="utf-8"))) for p in paths)


def ledger_lines(tally: Counter[tuple[str, str]]) -> list[str]:
    lines = [f"{path}\t{message}" for (path, message), n in tally.items() for _ in range(n)]
    return sorted(lines)


def check(scan: Counter[tuple[str, str]]) -> list[str]:
    """判据只有一条：一句中文 `#[error]` 都不许有。空列表 = 通过。"""
    problems: list[str] = []

    if scan:
        problems.append(
            "新的中文 `#[error]` 文案进了领域错误——它会经 `e.to_string()` 流到英文界面，"
            "而 IPC 那道门禁只看字面量，一眼都看不见。\n"
            "     改法：`#[error]` 写英文诊断串（它的受众是日志与开发者），"
            "给用户的那半句由稳定错误码经 i18n 产生（ADR-100/101）。\n"
            "     **这道门禁没有豁免口**，欠账已清零。发现的：\n       "
            + "\n       ".join(ledger_lines(scan))
        )

    return problems


def self_test() -> int:
    """反向验证：三条断言各配违规样本，且正确形状不得误报。"""
    good = '#[error("upstream error [{provider}]: {message}")]\n'
    bad = '#[error("笔记不存在：{0}")]\n'
    # 只残留中文排版的"英文"串：只锁汉字的判据会放它过去。
    punct_only = '#[error("not found（{0}）")]\n'

    cases: list[tuple[str, str, int]] = [
        ("英文 Display 不得误报", good, 0),
        ("新写一句中文 Display 必须红（无豁免口）", bad, 1),
        ("判定面含中文标点：只锁汉字会漏掉它", punct_only, 1),
    ]

    failures = 0
    for name, source, expected in cases:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            target = root / "crates" / "x" / "src" / "e.rs"
            target.parent.mkdir(parents=True)
            target.write_text(source, encoding="utf-8")
            problems = check(violations(root, sources(root)))
        got = len(problems)
        ok = (got > 0) == (expected > 0)
        print(f"  {'✅' if ok else '❌'} {name}（问题数 {got}，期望 {'>0' if expected else '0'}）")
        failures += 0 if ok else 1

    # 空跑防护：真实工作树上必须扫得到 `#[error]`，否则上面全部恒绿。
    scanned = scanned_attrs(sources(REPO))
    enough = scanned > 50
    print(f"  {'✅' if enough else '❌'} 扫描面非空（工作树里扫到 {scanned} 条 `#[error]`）")
    failures += 0 if enough else 1

    if failures:
        print(f"\n❌ 自测未过：{failures} 项", file=sys.stderr)
        return 1
    print("\n自测全过：违规样本必红、英文 Display 不误报、扫描面非空。")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true", help="反向验证断言非空跑")
    args = parser.parse_args()
    if args.self_test:
        return self_test()

    paths = sources(REPO)
    scan = violations(REPO, paths)
    problems = check(scan)
    if problems:
        print("领域错误 Display 本地化守护失败（ADR-101）：\n", file=sys.stderr)
        for problem in problems:
            print(f"  {problem}\n", file=sys.stderr)
        return 1
    print(
        f"✅ 领域错误 Display：{len(paths)} 个 .rs 里的 {scanned_attrs(paths)} 条 `#[error]` "
        "已全部纳入判定面，无一句中文（无豁免口，ADR-108）。"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
