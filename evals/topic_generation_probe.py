#!/usr/bin/env python3
"""Run synthetic topic-scope smoke cases against a local llama-server (OpenAI-compatible).

Start one yourself, e.g. `llama-server -m <model.gguf> -c 16384 --port 8080`; override the
address with `LUNA_LLAMA_BASE_URL` (default `http://127.0.0.1:8080/v1`) and, if the server
was started with a key, `LUNA_LLAMA_API_KEY`.

This is a diagnostic probe, not semantic acceptance: it bypasses AppState, JobQueue,
ResultSchema, Vault publication and the independent-review gold set.
"""

from __future__ import annotations

import argparse
import json
import os
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / "evals/fixtures/topic_generation_model.json"
PROMPTS = ROOT / "crates/luna-core/prompts/bundled.json"
BASE_URL = os.environ.get("LUNA_LLAMA_BASE_URL", "http://127.0.0.1:8080/v1").rstrip("/")
API_KEY = os.environ.get("LUNA_LLAMA_API_KEY")


def prompt() -> str:
    sets = json.loads(PROMPTS.read_text(encoding="utf-8"))["sets"]
    return next(
        item["templates"]["topic_generation"]
        for item in sets
        if item["scope"] == "ingest" and item["locale"] is None
    )


def run_case(model: str, instruction: str, case: dict) -> tuple[str, dict]:
    payload = {key: case[key] for key in ("scope", "input_complete", "evidence")}
    headers = {"Content-Type": "application/json"}
    if API_KEY:
        headers["Authorization"] = f"Bearer {API_KEY}"
    request = urllib.request.Request(
        f"{BASE_URL}/chat/completions",
        data=json.dumps(
            {
                "model": model,
                "messages": [
                    {"role": "system", "content": instruction},
                    {"role": "user", "content": json.dumps(payload, ensure_ascii=False)},
                ],
                "stream": False,
                # Qwen3 / 3.5 模板默认先想后答：不关的话正文为空、全进 reasoning_content。
                "chat_template_kwargs": {"enable_thinking": False},
                "max_tokens": 2048,
            },
            ensure_ascii=False,
        ).encode("utf-8"),
        headers=headers,
    )
    with urllib.request.urlopen(request, timeout=180) as response:
        choice = json.load(response)["choices"][0]
    # 只有自然收尾（stop）才算完整输出；length = 输出预算触顶。
    terminal = choice.get("finish_reason") == "stop"
    raw = (choice.get("message", {}).get("content") or "").strip()
    try:
        output = json.loads(raw)
    except json.JSONDecodeError:
        return raw, {"pass": False, "reason": "invalid_json", "terminal": terminal}
    if not isinstance(output, dict):
        return raw, {"pass": False, "reason": "not_an_object", "terminal": terminal}
    status = output.get("status")
    claims = output.get("claims", [])
    ids: list[str] = []
    structure_valid = False
    if status == "no_op":
        structure_valid = (
            set(output) == {"status", "reason"}
            and isinstance(output["reason"], str)
            and bool(output["reason"].strip())
        )
    elif status == "claims" and isinstance(claims, list) and claims:
        source_kinds = {item["id"]: item["kind"] for item in case["evidence"]}
        structure_valid = set(output) == {"status", "claims"}
        for claim in claims:
            if not isinstance(claim, dict):
                structure_valid = False
                continue
            references = claim.get("evidence_ids")
            if (
                set(claim) != {"text", "kind", "evidence_ids"}
                or not isinstance(claim.get("text"), str)
                or not claim["text"].strip()
                or not isinstance(references, list)
                or not references
                or not all(isinstance(item, str) for item in references)
            ):
                structure_valid = False
                continue
            ids.extend(references)
            referenced_kinds = {source_kinds.get(item) for item in references}
            if None in referenced_kinds:
                structure_valid = False
            if claim["kind"] == "synthesis" and "authoritative_transcript" in referenced_kinds:
                structure_valid = False
            if claim["kind"] == "transcript_record" and "authoritative_transcript" not in referenced_kinds:
                structure_valid = False
            if claim["kind"] not in {"synthesis", "transcript_record", "conflict"}:
                structure_valid = False
    passed = (
        terminal
        and structure_valid
        and status == case["expected_status"]
        and all(item in case["allowed_ids"] for item in ids)
    )
    return raw, {
        "pass": passed,
        "terminal": terminal,
        "structure_valid": structure_valid,
        "status": status,
        "cited_ids": ids,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", required=True, help="Model name sent in requests (single-model llama-server ignores it)")
    parser.add_argument("--repeats", type=int, default=1, help="Independent runs per case")
    args = parser.parse_args()
    if not 1 <= args.repeats <= 10:
        parser.error("--repeats must be between 1 and 10")
    cases = json.loads(FIXTURE.read_text(encoding="utf-8"))["cases"]
    instruction = prompt()
    results = []
    for run in range(1, args.repeats + 1):
        for case in cases:
            raw, verdict = run_case(args.model, instruction, case)
            results.append({"run": run, "case": case["name"], **verdict, "raw": raw})
    print(json.dumps({"model": args.model, "results": results}, ensure_ascii=False, indent=2))
    return 0 if all(item["pass"] for item in results) else 1


if __name__ == "__main__":
    raise SystemExit(main())
