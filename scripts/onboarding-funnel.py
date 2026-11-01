#!/usr/bin/env python3
"""读回这台机器上的首启引导记录（PRD-14 §1）。

回答三个问题：**停在哪一步、每步花了多久、走完没有**。在此之前这三个只能靠回忆——真机
作业时尤其明显：同一轮验收里判断「是我自己停在那一屏，还是它卡住了」，没有记录只能猜。

**它不是聚合漏斗。** 文件在设备级目录、默认离线、不上报（同 `startup-diagnostics.json`，
ADR-114 ①）。要拿到"一群用户在哪一步流失"需要 opt-in 遥测，那是另一件事。

记录里只有步骤 id 与时刻，**不含任何用户内容**（TDD-12 脱敏纪律）。

用法：
    python3 scripts/onboarding-funnel.py            # 读默认路径
    python3 scripts/onboarding-funnel.py <文件>     # 读指定文件（真机作业带回来的那一份）
    python3 scripts/onboarding-funnel.py --self-test
"""

from __future__ import annotations

import argparse
import json
import sys
from datetime import datetime, timezone
from pathlib import Path

DEFAULT = Path.home() / "Library/Application Support/app.lunarca.luna/onboarding-funnel.json"
COMPLETED = "completed"

TITLES = {
    "welcome": "欢迎",
    "vault": "选择 Vault",
    "compute": "AI 在哪跑",
    "models": "模型准备",
    "permissions": "系统权限",
    "shortcuts": "快捷键",
    "first_task": "第一个任务",
    COMPLETED: "收场进入应用",
}


def render(marks: list[dict]) -> list[str]:
    """把记录排成「哪一步 · 停了多久」，最后给一句结论。"""
    if not marks:
        return ["这台设备上还没有引导记录（引导没跑过，或跑在别的应用数据目录下）。"]

    lines = []
    for index, mark in enumerate(marks):
        step = str(mark.get("step", "?"))
        at_ms = float(mark.get("at_ms", 0))
        when = datetime.fromtimestamp(at_ms / 1000, tz=timezone.utc).astimezone()
        # 停留时长 = 到下一条为止；最后一条没有下一条，故不报时长（它可能还停在那儿）。
        held = ""
        if index + 1 < len(marks):
            seconds = (float(marks[index + 1].get("at_ms", at_ms)) - at_ms) / 1000
            held = f"  停留 {seconds:>7.1f}s"
        label = TITLES.get(step, step)
        lines.append(f"  {when:%Y-%m-%d %H:%M:%S}  {label:<12}{held}")

    completed = any(m.get("step") == COMPLETED for m in marks)
    if completed:
        first, last = float(marks[0]["at_ms"]), float(marks[-1]["at_ms"])
        lines.append("")
        lines.append(f"结论：走完了，从第一屏到进入应用共 {(last - first) / 1000:.1f}s。")
    else:
        lines.append("")
        lines.append(
            f"结论：**没走完**，最后停在「{TITLES.get(str(marks[-1]['step']), marks[-1]['step'])}」。"
        )
    return lines


def self_test() -> int:
    """反向验证：走完与没走完必须给出不同的结论，且时长真的被算出来。"""
    done = render(
        [
            {"step": "welcome", "at_ms": 1_000_000.0},
            {"step": "vault", "at_ms": 1_012_000.0},
            {"step": COMPLETED, "at_ms": 1_015_000.0},
        ]
    )
    if not any("走完了" in line for line in done):
        print("自测：走完的样本没给出「走完了」", file=sys.stderr)
        return 1
    if not any("12.0s" in line for line in done):
        print("自测：停留时长没算对（welcome 应为 12.0s）", file=sys.stderr)
        return 1

    stuck = render([{"step": "welcome", "at_ms": 1.0}, {"step": "models", "at_ms": 2.0}])
    if not any("没走完" in line and "模型准备" in line for line in stuck):
        print("自测：没走完的样本没指出停在哪一步", file=sys.stderr)
        return 1
    if render([]) == []:
        print("自测：空记录应给出一句人话而不是空输出", file=sys.stderr)
        return 1
    print("引导记录读取自测 OK — 走完 / 没走完 / 空记录三种输入各给出不同结论。")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("path", nargs="?", type=Path, default=DEFAULT)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        return self_test()

    if not args.path.is_file():
        print(f"读不到引导记录：{args.path}", file=sys.stderr)
        return 1
    marks = json.loads(args.path.read_text(encoding="utf-8"))
    print(f"首启引导记录（{args.path}）：")
    for line in render(marks):
        print(line)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
