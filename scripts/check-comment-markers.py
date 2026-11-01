#!/usr/bin/env python3
"""代码注释里不得用图标标记表达重要性（2026-09-10 用户裁决）。

仓库此前用一个红点符号给注释分级，全仓积到 3000 多处。它有三个代价：

- **重要性靠符号而不是内容**：一个标记用到三千次之后，它不再区分任何东西——
  真正反直觉的那几行和顺手写的说明挂着同一个记号；
- **渲染不一致**：同一个字符在终端、各家编辑器、`git log`、代码托管页上宽度与颜色都不同，
  中文注释里尤其容易把对齐撑歪；
- **会漏进生成物**：Rust 的 doc 注释经 specta 原样进 `bindings.ts`，标记跟着流到
  一份没人手写的文件里。

重要性该由内容和位置表达：把"为什么这段代码看起来反直觉"写在它上面，就够了。

判据刻意**只认那一个红点符号**，不做通用 emoji 扫描：
仓库里另有真实用途的字形——关闭键的 `✕`、引导步骤条上的 `✓`、
emoji 处理的测试语料——一律不该被这道门禁碰。要再禁别的符号就往 `BANNED` 里加，
每加一个都得先回答"它在这个仓库里有没有正当用途"。

扫描面是源码、脚本与工程配置（`.rs` / `.ts` / `.tsx` / `.css` / `.mjs` / `.js` / `.py`
/ `.yml` / `.yaml` / `.toml`）——CI 工作流与 Cargo 清单里的注释同样是给工程师读的。
**`docs/` 与 `*.md` 不在其中**：那是文档的排版惯例，与代码注释是两件事。

用法：
    python3 scripts/check-comment-markers.py              # 校验工作树
    python3 scripts/check-comment-markers.py --self-test  # 自测（证明断言非空跑）
"""

from __future__ import annotations

import argparse
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

#: 禁用的注释标记。加新条目前先回答它在本仓有没有正当用途（见模块文档）。
BANNED = ("\U0001f534",)

SUFFIXES = {".rs", ".ts", ".tsx", ".css", ".mjs", ".js", ".py", ".yml", ".yaml", ".toml"}
SKIP_DIRS = {"node_modules", "target", "dist", ".git", "actions-runner", "docs"}


def sources(root: Path) -> list[Path]:
    """扫描面：受管源码与脚本，跳过依赖、产物与文档。"""
    found = []
    for path in sorted(root.rglob("*")):
        if not path.is_file() or path.suffix not in SUFFIXES:
            continue
        if any(part in SKIP_DIRS for part in path.relative_to(root).parts):
            continue
        found.append(path)
    return found


def check(paths: list[Path], root: Path) -> list[str]:
    problems = []
    for path in paths:
        try:
            text = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        for number, line in enumerate(text.split("\n"), 1):
            for marker in BANNED:
                if marker in line:
                    rel = path.relative_to(root)
                    problems.append(f"{rel}:{number} 注释里有图标标记：{line.strip()[:80]}")
                    break
    return problems


def self_test() -> int:
    """反向验证：违规样本必须被抓到，正当字形与文档必须不被误报。"""
    marker = BANNED[0]
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        (root / "src").mkdir(parents=True)
        (root / "docs").mkdir(parents=True)

        (root / "src/bad.rs").write_text(f"/// {marker} 这条必须被抓到\n", encoding="utf-8")
        # 关闭键的 ✕、步骤条的 ✓、emoji 测试语料：都不该判红。
        (root / "src/ok.tsx").write_text(
            'const close = "✕";\nconst done = "✓";\nconst face = "\U0001f600";\n',
            encoding="utf-8",
        )
        # 文档的排版惯例与代码注释是两件事。
        (root / "docs/spec.md").write_text(f"{marker} 文档里照旧允许\n", encoding="utf-8")

        found = sources(root)
        if not found:
            print("自测：扫描面读不到任何文件", file=sys.stderr)
            return 1
        if any(p.suffix == ".md" for p in found):
            print("自测：扫描面不该包含 Markdown", file=sys.stderr)
            return 1

        problems = check(found, root)
        if len(problems) != 1 or "bad.rs" not in problems[0]:
            print(f"自测：应恰好抓到 src/bad.rs 一处，实得 {problems}", file=sys.stderr)
            return 1

    print("自测全过：违规样本被抓到，正当字形与 Markdown 不误报，扫描面非空。")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true", help="反向验证断言非空跑")
    args = parser.parse_args()
    if args.self_test:
        return self_test()

    paths = sources(REPO)
    problems = check(paths, REPO)
    if problems:
        print("注释标记守护失败：重要性该由内容表达，不靠符号。\n", file=sys.stderr)
        for problem in problems[:40]:
            print(f"  {problem}", file=sys.stderr)
        if len(problems) > 40:
            print(f"  …另有 {len(problems) - 40} 处", file=sys.stderr)
        return 1
    print(f"注释标记守护 OK — 已扫 {len(paths)} 个源码/脚本文件，无图标标记。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
