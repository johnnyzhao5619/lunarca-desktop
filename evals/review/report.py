#!/usr/bin/env python3
"""人工验收标注的按版本通过率（PRD-16 §4；标注办法见 `evals/README.md`）。零依赖。

跑法：

    python3 evals/review/report.py 标注后.csv  # 按 model_revision × prompt_revision 报通过率
    python3 evals/review/report.py --self-test

输入是 `annotation-template.csv` 或 `topic_review_set_export` 导出的 CSV（两者都含
`sample_id` / `model_revision` / `prompt_revision` / `claim_text` 与四个标注列）。
一行 = 一条声明，按这四列对齐；标注值只认 `yes` / `no` / `na`，留空 = 这条没标（无新增、
协议失败的行本来就没有声明可标）。

报告按版本分组、不合并，不以总体均值掩盖差模型；放不放行由所有者看过之后决定，脚本不下结论。
"""

from __future__ import annotations

import csv
import io
import sys
from collections import Counter, defaultdict

KEY = ("sample_id", "model_revision", "prompt_revision", "claim_text")
FIELDS = ("relevant", "supported", "certainty_kept", "conflict_kept")
LABELS = ("yes", "no", "na")


class AnnotationError(ValueError):
    pass


def read_rows(text: str, name: str) -> dict[tuple[str, ...], dict[str, str]]:
    """按对齐键读出每行的标注；重复键或非法取值直接报错，不猜。"""
    reader = csv.DictReader(io.StringIO(text))
    missing = [column for column in KEY + FIELDS if column not in (reader.fieldnames or [])]
    if missing:
        raise AnnotationError(f"{name}: 缺列 {', '.join(missing)}")
    rows: dict[tuple[str, ...], dict[str, str]] = {}
    for number, row in enumerate(reader, start=2):
        key = tuple(row[column] for column in KEY)
        labels = {}
        for field in FIELDS:
            value = (row[field] or "").strip().lower()
            if value and value not in LABELS:
                raise AnnotationError(f"{name}:{number}: {field} 只能是 yes/no/na，实为 {row[field]!r}")
            labels[field] = value
        if key in rows:
            raise AnnotationError(f"{name}:{number}: 同一条声明出现两次（{key[0]}）")
        rows[key] = labels
    return rows


def rate(counts: Counter, positive: str) -> float | None:
    decided = counts["yes"] + counts["no"]
    return counts[positive] / decided if decided else None


def report(text: str, name: str = "report") -> list[str]:
    rows = read_rows(text, name)
    groups: dict[tuple[str, str], dict[str, Counter]] = defaultdict(lambda: defaultdict(Counter))
    for key, labels in rows.items():
        if not any(labels.values()):
            continue
        group = groups[(key[1], key[2])]
        for field in FIELDS:
            if labels[field]:
                group[field][labels[field]] += 1
    lines: list[str] = []
    for (model, prompt), counts in sorted(groups.items()):
        lines.append(f"model={model} prompt={prompt}")
        for field in FIELDS:
            c = counts[field]
            value = rate(c, "yes")
            shown = "—" if value is None else f"{value:.1%}"
            lines.append(f"  {field}: {shown}（yes {c['yes']} / no {c['no']} / na {c['na']}）")
    return lines or ["没有已标注的行"]


def self_test() -> None:
    header = ",".join(KEY + FIELDS + ("reason", "reviewer"))
    # 版本分组不合并：好模型不能把差模型的均值拉上去。
    final = "\n".join(
        [header]
        + [f"G{i},good,p,句{i},yes,yes,yes,na,," for i in range(20)]
        + ["B1,bad,p,串,no,yes,yes,na,,", "B2,bad,p,缺,yes,no,yes,na,,", "B3,bad,p,,,,,,,"]
    )
    out = report(final)
    good = out.index("model=good prompt=p")
    bad = out.index("model=bad prompt=p")
    assert out[good + 2] == "  supported: 100.0%（yes 20 / no 0 / na 0）", out
    assert out[bad + 2] == "  supported: 50.0%（yes 1 / no 1 / na 0）", out
    assert out[bad + 1] == "  relevant: 50.0%（yes 1 / no 1 / na 0）", out
    assert not any("达标" in line for line in out), out
    assert report(header) == ["没有已标注的行"]
    for broken, message in [
        (header + "\nN01,m,p,甲,maybe,yes,yes,na,,A", "只能是 yes/no/na"),
        (header + "\nN01,m,p,甲,yes,,,,,A\nN01,m,p,甲,no,,,,,A", "出现两次"),
        ("sample_id,claim_text\nN01,甲", "缺列"),
    ]:
        try:
            read_rows(broken, "x")
        except AnnotationError as error:
            assert message in str(error), error
        else:
            raise AssertionError(f"应拒绝：{broken!r}")
    print("self-test ok")


def main(argv: list[str]) -> int:
    if argv == ["--self-test"]:
        self_test()
        return 0
    if len(argv) != 1:
        print(__doc__, file=sys.stderr)
        return 2
    try:
        with open(argv[0], encoding="utf-8", newline="") as handle:
            print("\n".join(report(handle.read(), argv[0])))
    except AnnotationError as error:
        print(error, file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
