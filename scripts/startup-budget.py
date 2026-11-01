#!/usr/bin/env python3
"""冷启动预算（TDD-13 §6「冷启动到可响应热键 < 1.5s」）的**真机作业项**。

它读的是设备级 `startup-diagnostics.json`（`startup::StartupTimeline` 每次启动落盘，
ADR-114 ①），不是 CI 能产出的东西——CI 机器上没有一次真实的冷启动。故它**刻意不叫
`check-*`**（那一族由元门禁要求接进 CI），而是真机验收单里的一条命令：跑一次应用、
退出、跑这个脚本，红即超预算。

判据只有一条硬的：`shortcuts_registered`（全局热键注册完成 = 可响应热键）≤ 1500 ms。
其余里程碑（首帧 / 工作区就绪 / 引擎就绪）只打印，不判——它们受隐藏建窗、引导覆盖层与
本机模型多少影响，没有一条对所有机器都成立的数字（见 TDD-13 §6 该行）。

用法：
    python3 scripts/startup-budget.py                 # 读本机默认位置（macOS）
    python3 scripts/startup-budget.py <path.json>     # 指定文件（Windows / 另一台机器拷来的）
    python3 scripts/startup-budget.py --self-test
"""

from __future__ import annotations

import json
import os
import sys
import tempfile
from pathlib import Path

BUDGET_MS = 1500
GATED_MILESTONE = "shortcuts_registered"
DEFAULT_MAC = Path.home() / "Library/Application Support/app.lunarca.luna/startup-diagnostics.json"


def load(path: Path) -> list[dict]:
    data = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(data, list):
        raise ValueError("诊断文件应是里程碑数组")
    return data


def evaluate(marks: list[dict]) -> tuple[bool, list[str]]:
    """返回（是否达标，逐行报告）。缺 `shortcuts_registered` 视为不达标——没到就是没到。"""
    lines: list[str] = []
    gated: int | None = None
    for mark in sorted(marks, key=lambda m: m.get("elapsed_ms", 0)):
        name = str(mark.get("milestone", "?"))
        ms = int(mark.get("elapsed_ms", 0))
        flag = ""
        if name == GATED_MILESTONE:
            gated = ms
            flag = "  ← 判据" + ("（超预算）" if ms > BUDGET_MS else "")
        lines.append(f"  {name:<22} {ms:>7} ms{flag}")
    if gated is None:
        lines.append(f"  缺 {GATED_MILESTONE}：本次启动没走到热键注册")
        return False, lines
    return gated <= BUDGET_MS, lines


def self_test() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        ok_path = Path(tmp) / "ok.json"
        ok_path.write_text(
            json.dumps(
                [
                    {"milestone": "setup_begin", "elapsed_ms": 100},
                    {"milestone": GATED_MILESTONE, "elapsed_ms": 900},
                    {"milestone": "first_frame", "elapsed_ms": 9000},
                ]
            ),
            encoding="utf-8",
        )
        passed, _ = evaluate(load(ok_path))
        assert passed
        slow = [{"milestone": GATED_MILESTONE, "elapsed_ms": BUDGET_MS + 1}]
        assert not evaluate(slow)[0]
        assert not evaluate([{"milestone": "setup_begin", "elapsed_ms": 1}])[0]
    print("startup-budget 自测通过。")


def main(argv: list[str]) -> int:
    if "--self-test" in argv:
        self_test()
        return 0
    path = Path(argv[0]) if argv else DEFAULT_MAC
    if not path.is_file():
        print(f"找不到诊断文件：{path}（先跑一次应用再来）", file=sys.stderr)
        return 2
    passed, lines = evaluate(load(path))
    print(f"冷启动时间线（{path}）：")
    print("\n".join(lines))
    if passed:
        print(f"达标：{GATED_MILESTONE} ≤ {BUDGET_MS} ms。")
        return 0
    print(f"超预算：{GATED_MILESTONE} 应 ≤ {BUDGET_MS} ms（TDD-13 §6）。", file=sys.stderr)
    return 1


if __name__ == "__main__":
    os.environ.setdefault("PYTHONIOENCODING", "utf-8")
    sys.exit(main(sys.argv[1:]))
