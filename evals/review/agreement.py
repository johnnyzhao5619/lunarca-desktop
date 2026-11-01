#!/usr/bin/env python3
"""两名评审标注的一致性与按版本通过率（PRD-16 §4；标注办法见 `evals/README.md`）。零依赖。

跑法：

    python3 evals/review/agreement.py A.csv B.csv   # 校准 / 正式标注：逐字段 Cohen κ + 分歧清单
    python3 evals/review/agreement.py --report 仲裁后.csv  # 按 model_revision × prompt_revision 报通过率
    python3 evals/review/agreement.py --self-test

输入是 `annotation-template.csv` 或 `topic_review_set_export` 导出的 CSV（两者都含
`sample_id` / `model_revision` / `prompt_revision` / `claim_text` 与四个标注列）。
一行 = 一条声明，按这四列对齐；标注值只认 `yes` / `no` / `na`，留空 = 这条没标（无新增、
协议失败的行本来就没有声明可标）。

κ 只算两人都标了的行；任一字段 κ < 0.6 退出码为 1——那表示标注说明还不够清楚，
先对照分歧改说明再标一轮，而不是开始正式标。报告按版本分组、不合并：PRD-16 §4 的
「语义支持率 ≥95%、跨主题混入率 ≤2%」逐组判定，不以总体均值掩盖差模型。
"""

from __future__ import annotations

import csv
import io
import sys
from collections import Counter, defaultdict

KEY = ("sample_id", "model_revision", "prompt_revision", "claim_text")
FIELDS = ("relevant", "supported", "certainty_kept", "conflict_kept")
LABELS = ("yes", "no", "na")
KAPPA_MIN = 0.6
SUPPORTED_MIN = 0.95
CROSS_TOPIC_MAX = 0.02


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


def cohen_kappa(pairs: list[tuple[str, str]]) -> float | None:
    """两名评审在同一批条目上的 Cohen κ；没有条目时为 None。"""
    if not pairs:
        return None
    total = len(pairs)
    observed = sum(1 for a, b in pairs if a == b) / total
    first = Counter(a for a, _ in pairs)
    second = Counter(b for _, b in pairs)
    expected = sum(first[label] * second[label] for label in LABELS) / (total * total)
    if expected == 1:
        # 两人全程只用同一个取值：一致但无从区分偶然，按完全一致记。
        return 1.0
    return (observed - expected) / (1 - expected)


def agreement(a_text: str, b_text: str, a_name: str = "A", b_name: str = "B") -> tuple[list[str], bool]:
    a_rows, b_rows = read_rows(a_text, a_name), read_rows(b_text, b_name)
    lines: list[str] = []
    only_a = len(a_rows.keys() - b_rows.keys())
    only_b = len(b_rows.keys() - a_rows.keys())
    if only_a or only_b:
        lines.append(f"对不齐：仅 {a_name} 有 {only_a} 行，仅 {b_name} 有 {only_b} 行（这些行不参与计算）")
    shared = sorted(a_rows.keys() & b_rows.keys())
    passed = True
    disagreements: list[str] = []
    for field in FIELDS:
        pairs = []
        for key in shared:
            a, b = a_rows[key][field], b_rows[key][field]
            if a and b:
                pairs.append((a, b))
                if a != b:
                    disagreements.append(f"  {key[0]} {field}: {a_name}={a} {b_name}={b} | {key[3][:60]}")
        kappa = cohen_kappa(pairs)
        if kappa is None:
            passed = False
            lines.append(f"{field}: 两人都标了的行数为 0")
            continue
        ok = kappa >= KAPPA_MIN
        passed &= ok
        lines.append(f"{field}: κ={kappa:.3f} n={len(pairs)} {'达标' if ok else f'未达 {KAPPA_MIN}'}")
    if disagreements:
        lines.append(f"分歧 {len(disagreements)} 处（交第三人或两人讨论仲裁，理由回写标注说明）：")
        lines.extend(disagreements)
    return lines, passed


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
        supported = rate(counts["supported"], "yes")
        cross = rate(counts["relevant"], "no")
        verdict = (
            supported is not None
            and cross is not None
            and supported >= SUPPORTED_MIN
            and cross <= CROSS_TOPIC_MAX
        )
        lines.append(
            f"  PRD-16 §4：支持率 ≥{SUPPORTED_MIN:.0%}、跨主题混入 ≤{CROSS_TOPIC_MAX:.0%} → "
            + ("达标" if verdict else "未达标（仅开放建议或停止该能力）")
        )
    return lines or ["没有已标注的行"]


def self_test() -> None:
    header = ",".join(KEY + FIELDS + ("reason", "reviewer"))
    a = "\n".join(
        [
            header,
            "N01,m,p,甲,yes,yes,yes,na,,A",
            "N02,m,p,乙,yes,no,yes,na,,A",
            "N03,m,p,丙,no,yes,no,na,,A",
            "N04,m,p,丁,yes,yes,yes,yes,,A",
            "N05,m,p,,,,,,,A",
        ]
    )
    b = "\n".join(
        [
            header,
            "N01,m,p,甲,yes,yes,yes,na,,B",
            "N02,m,p,乙,yes,no,no,na,,B",
            "N03,m,p,丙,no,yes,no,na,,B",
            "N04,m,p,丁,yes,yes,yes,yes,,B",
            "N06,m,p,戊,yes,yes,yes,na,,B",
        ]
    )
    lines, passed = agreement(a, b)
    assert "relevant: κ=1.000 n=4 达标" in lines, lines
    # certainty_kept：一致 3/4，A 为 yes×3/no×1、B 为 yes×2/no×2 → pe=0.5，κ=0.5。
    assert "certainty_kept: κ=0.500 n=4 未达 0.6" in lines, lines
    assert not passed
    assert any(line.startswith("对不齐：仅 A 有 1 行，仅 B 有 1 行") for line in lines), lines
    assert any("N02 certainty_kept: A=yes B=no" in line for line in lines), lines
    assert cohen_kappa([("yes", "yes"), ("yes", "yes")]) == 1.0
    assert cohen_kappa([]) is None
    # 版本分组不合并：好模型不能把差模型的均值拉上去。
    final = "\n".join(
        [header]
        + [f"G{i},good,p,句{i},yes,yes,yes,na,," for i in range(20)]
        + ["B1,bad,p,串,no,yes,yes,na,,", "B2,bad,p,缺,yes,no,yes,na,,"]
    )
    out = report(final)
    good = out.index("model=good prompt=p")
    bad = out.index("model=bad prompt=p")
    assert out[bad + 5].endswith("未达标（仅开放建议或停止该能力）"), out
    assert out[good + 5].endswith("达标") and "未达标" not in out[good + 5], out
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
    try:
        if len(argv) == 2 and argv[0] == "--report":
            with open(argv[1], encoding="utf-8") as handle:
                print("\n".join(report(handle.read(), argv[1])))
            return 0
        if len(argv) == 2:
            with open(argv[0], encoding="utf-8") as a, open(argv[1], encoding="utf-8") as b:
                lines, passed = agreement(a.read(), b.read(), argv[0], argv[1])
            print("\n".join(lines))
            return 0 if passed else 1
    except AnnotationError as error:
        print(error, file=sys.stderr)
        return 2
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
