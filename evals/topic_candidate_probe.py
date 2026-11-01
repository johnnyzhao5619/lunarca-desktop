#!/usr/bin/env python3
"""主题候选的检索层实验（PRD-16-063 / TDD-06 §3.9.2）：没有标签的来源靠什么到达主题。

把固定评测集 `evals/fixtures/topic_review_set.json` 的 60 组来源**去掉标签**合成一个池子
（模拟没有 AI 标签的录音、转写与速记），对 9 个主题逐一问：

- `lexical`：来源正文写到了主题名（`TopicScope::mention_names` 同一规则——标签路径拼接、
  三字以上的末段；两字词不用）。这是 Rust 实现采用的规则，本脚本离线即可跑。
- `semantic`（`--semantic`，需要一个跑着 bge-m3 GGUF 的本机 `llama-server --embeddings`，地址取
  `LUNA_LLAMA_BASE_URL`，缺省 `http://127.0.0.1:8080/v1`）：主题「名称 + 已有声明」与来源分段的
  最大余弦相似度 ≥ τ。

相关的判据：来源所在组的标签就是这个主题，且该组期望写入（`expect.status = claims`）。
「写到了别的主题名的多主题来源」（联合周会、多项目速记）在判据里算作别组，所以 lexical 的
「他主题命中」是**真实写到了那个主题**的来源，打印明细供人核对，不是误收。

这是诊断，不是语义验收：模型拿到候选之后写不写、写得对不对，由 `topic_review_set_export`
（两名评审）判定。

    python3 evals/topic_candidate_probe.py
    python3 evals/topic_candidate_probe.py --semantic
"""

from __future__ import annotations

import argparse
import json
import os
import urllib.request
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / "evals/fixtures/topic_review_set.json"
EMBED = os.environ.get("LUNA_LLAMA_BASE_URL", "http://127.0.0.1:8080/v1").rstrip("/") + "/embeddings"
MIN_CHARS = 3


def mention_names(tag: str) -> list[str]:
    """与 `TopicScope::from_tag(tag).mention_names()` 同一规则（标题带「·」时不原样用）。"""
    names: list[str] = []
    for name in ("".join(tag.split("/")), tag.rsplit("/", 1)[-1].strip()):
        if len(name) >= MIN_CHARS and name.lower() not in (known.lower() for known in names):
            names.append(name)
    return names


def embed(texts: list[str]) -> list[list[float]]:
    request = urllib.request.Request(
        EMBED,
        data=json.dumps({"model": "bge-m3", "input": texts, "encoding_format": "float"}).encode(),
        headers={"Content-Type": "application/json"},
    )
    # 引擎超窗即拒、从不截断——这里拿到的每条向量都代表整段输入。
    with urllib.request.urlopen(request, timeout=300) as response:
        rows = sorted(json.load(response)["data"], key=lambda row: row["index"])
    return [row["embedding"] for row in rows]


def paragraphs(text: str) -> list[str]:
    parts = [p.strip() for p in text.split("\n\n") if p.strip() and not p.strip().startswith("#")]
    return parts or [text]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--semantic", action="store_true", help="同时跑 bge-m3 语义相似度对照")
    args = parser.parse_args()

    groups = json.loads(FIXTURE.read_text(encoding="utf-8"))["groups"]
    sources = [
        (group["id"], group["tag"], text, group["expect"]["status"] == "claims")
        for group in groups
        for text in group["sources"]
    ]
    topics = sorted({group["tag"] for group in groups})

    def report(name: str, hit) -> None:
        tp = fp = fn = 0
        for topic in topics:
            for index, source in enumerate(sources):
                relevant = source[1] == topic and source[3]
                if hit(topic, index):
                    if relevant:
                        tp += 1
                    elif source[1] != topic:
                        fp += 1
                elif relevant:
                    fn += 1
        others = sum(1 for topic in topics for source in sources if source[1] != topic)
        print(f"{name}: recall={tp / max(tp + fn, 1):.2f} other_topic_hits={fp}/{others}")

    report("lexical", lambda topic, i: any(n in sources[i][2] for n in mention_names(topic)))
    print("  他主题命中明细（应当都真的写到了那个主题）：")
    for topic in topics:
        for source in sources:
            if source[1] != topic and any(n in source[2] for n in mention_names(topic)):
                print(f"    {topic} ← {source[0]}：{source[2][:60]!r}")

    if args.semantic:
        claims: dict[str, set[str]] = defaultdict(set)
        for group in groups:
            claims[group["tag"]].update(claim["text"] for claim in group["existing"])
        chunked = [paragraphs(source[2]) for source in sources]
        flat = embed([chunk for chunks in chunked for chunk in chunks])
        cursor = iter(flat)
        vectors = [[next(cursor) for _ in chunks] for chunks in chunked]
        queries = embed([t.replace("/", " · ") + "\n" + "\n".join(sorted(claims[t])) for t in topics])
        query = dict(zip(topics, queries))

        def score(topic: str, index: int) -> float:
            return max(sum(a * b for a, b in zip(query[topic], v)) for v in vectors[index])

        for tau in (0.55, 0.6, 0.65, 0.7):
            report(f"semantic τ={tau}", lambda topic, i, tau=tau: score(topic, i) >= tau)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
