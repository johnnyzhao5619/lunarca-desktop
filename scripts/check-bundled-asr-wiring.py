#!/usr/bin/env python3
"""随包转写档的接线守护（静态，不联网）。

这一档模型是 PRD-14 §6「零下载即可在引导内完成首个任务」的全部依据，而它要成立得靠
**四处对齐**：Rust 常量、打包配置、恢复下载地址、`.gitignore`。任一处掉队时的表现都
一样——构建照常绿、应用照常起，只有全新安装的用户在断网时发现引导第一屏承诺的事做
不到，而那正是最没人跑的一条路径。本仓此前就是这么漏掉整个随包档的：`bundle.resources`
从头到尾没配过，而 `ModelRecord::status()` 会安静地把它判成 `downloadable`。

随包**示例语音**（PRD-14-051 那条成功路径）同理，且多一条：它必须与
`crates/luna-cli/tests/golden-asr/` 下的某个黄金样本**逐字节相同**。理由是选样判据——
那段音频要充当新用户的「成功时刻」，所以随包模型必须把它转对；而"转得对不对"已经有
`scripts/bench-asr-golden.sh` 在真机上逐档量（它需要 `say` 与 release 构建，刻意不进 CI）。
让随包示例就是它已经在量的那个文件，换模型档时准确率自然被覆盖，不必另起第二套门禁。
初版曾选了一段**基准**样本（故意含生僻专名），随包 tiny-q5_1 在它上面 WER 23.5%——
用来测缺陷是对的，用来演示就是反效果。

判据（任一不成立即以 1 退出）：
  1. `BUNDLED_WHISPER_RESOURCE_REL` 的文件名 == `BUNDLED_WHISPER_FILE`；
  2. 模型目录（`crates/luna-model-catalog/bundled.json`）里恰好一条 `bundled` 条目，且其恢复下载
     地址的文件名 == `BUNDLED_WHISPER_FILE`（下的与装的必须是同一档）；
  3. `tauri.conf.json` 的 `bundle.resources` 把两份资源的 `resources/<REL>` 各映射到 `<REL>`；
  4. `.gitignore` 覆盖随包模型目录（31 MB 不得入库，理由见该文件里那段注释）；
  5. 随包示例语音与某个黄金样本逐字节相同，且那个样本带参考文本。

用法：
    python3 scripts/check-bundled-asr-wiring.py              # 校验工作树
    python3 scripts/check-bundled-asr-wiring.py --self-test  # 自测（证明断言非空跑）
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
IGNORE_LINE = "/src-tauri/resources/models/"


def _const_str(source: str, name: str) -> str | None:
    match = re.search(rf'{name}:\s*&str\s*=\s*"([^"]+)"', source)
    return match.group(1) if match else None


def problems(root: Path) -> list[str]:
    """把四条判据跑一遍，返回人话的失败原因（空列表 = 全部成立）。"""
    model_store = root / "src-tauri" / "src" / "model_store.rs"
    catalog = root / "crates" / "luna-model-catalog" / "bundled.json"
    onboarding = root / "src-tauri" / "src" / "ipc" / "onboarding.rs"
    tauri_conf = root / "src-tauri" / "tauri.conf.json"
    gitignore = root / ".gitignore"
    for path in (model_store, catalog, onboarding, tauri_conf, gitignore):
        if not path.is_file():
            return [f"读不到 {path.relative_to(root)}"]

    source = model_store.read_text(encoding="utf-8")
    file_name = _const_str(source, "BUNDLED_WHISPER_FILE")
    rel = _const_str(source, "BUNDLED_WHISPER_RESOURCE_REL")
    if file_name is None or rel is None:
        return ["model_store.rs 里缺 BUNDLED_WHISPER_FILE / BUNDLED_WHISPER_RESOURCE_REL"]

    found: list[str] = []

    if rel.rsplit("/", 1)[-1] != file_name:
        found.append(f"资源相对路径 {rel!r} 的文件名与 BUNDLED_WHISPER_FILE {file_name!r} 不一致")

    bundled = [e for e in json.loads(catalog.read_text(encoding="utf-8")).get("entries", []) if e.get("bundled")]
    urls = [s.get("url", "") for e in bundled for s in e.get("weights", {}).get("sources", [])]
    if len(bundled) != 1 or not urls:
        found.append(f"模型目录里标了 bundled 的条目应恰好一条且带来源，实际 {len(bundled)} 条")
    elif urls[0].rsplit("/", 1)[-1] != file_name:
        found.append(
            f"恢复下载地址落在 {urls[0].rsplit('/', 1)[-1]!r}，"
            f"而随包的是 {file_name!r}——下的与装的不是同一档"
        )

    resources = json.loads(tauri_conf.read_text(encoding="utf-8")).get("bundle", {}).get("resources")
    expected_src = f"resources/{rel}"
    if not isinstance(resources, dict) or resources.get(expected_src) != rel:
        found.append(
            f"tauri.conf.json 的 bundle.resources 没有把 {expected_src!r} 映射到 {rel!r}"
            "——随包档不会被复制进 .app，应用启动时物化不到任何东西"
        )

    if IGNORE_LINE not in gitignore.read_text(encoding="utf-8"):
        found.append(f".gitignore 没有覆盖 {IGNORE_LINE}——31 MB 权重会被误提交进历史")

    found.extend(_sample_problems(root, onboarding.read_text(encoding="utf-8"), resources))
    return found


def _sample_problems(root: Path, source: str, resources: object) -> list[str]:
    """随包示例语音的三条：常量在、打包映射对、与黄金样本逐字节相同。"""
    rel = _const_str(source, "ONBOARDING_SAMPLE_REL")
    if rel is None:
        return ["ipc/onboarding.rs 里缺 ONBOARDING_SAMPLE_REL"]

    found: list[str] = []
    expected_src = f"resources/{rel}"
    if not isinstance(resources, dict) or resources.get(expected_src) != rel:
        found.append(
            f"tauri.conf.json 的 bundle.resources 没有把 {expected_src!r} 映射到 {rel!r}"
            "——示例语音不会进 .app，那张卡在真机上永远不出现"
        )

    shipped = root / "src-tauri" / "resources" / rel
    if not shipped.is_file():
        found.append(f"随包示例语音不在：{shipped.relative_to(root)}")
        return found

    goldens = sorted((root / "crates" / "luna-cli" / "tests" / "golden-asr").glob("*/*.wav"))
    payload = shipped.read_bytes()
    match = next((g for g in goldens if g.read_bytes() == payload), None)
    if match is None:
        found.append(
            "随包示例语音不是任何一个黄金样本的逐字节副本——于是换模型档时，"
            "bench-asr-golden.sh 不会报出它的 CER / WER，"
            "「新用户第一眼看到的转写是不是胡话」就没有任何东西在盯"
        )
    elif not match.with_suffix(".txt").is_file():
        found.append(f"黄金样本 {match.name} 没有参考文本，量不出 CER / WER")

    return found


SAMPLE_REL = "samples/clip.wav"


def _fixture(
    root: Path,
    *,
    rel: str,
    url_file: str,
    mapped: bool,
    ignored: bool,
    sample_mapped: bool = True,
    sample_is_golden: bool = True,
    golden_has_reference: bool = True,
) -> None:
    (root / "src-tauri" / "src" / "ipc").mkdir(parents=True, exist_ok=True)
    (root / "src-tauri" / "src" / "model_store.rs").write_text(
        f'pub const BUNDLED_WHISPER_FILE: &str = "{rel.rsplit("/", 1)[-1]}";\n'
        f'pub const BUNDLED_WHISPER_RESOURCE_REL: &str = "{rel}";\n',
        encoding="utf-8",
    )
    catalog = root / "crates" / "luna-model-catalog" / "bundled.json"
    catalog.parent.mkdir(parents=True, exist_ok=True)
    entry = {"id": "tiny", "bundled": True, "weights": {"sources": [{"kind": "url", "url": f"https://example.invalid/{url_file}"}]}}
    catalog.write_text(json.dumps({"schema": 1, "entries": [entry]}), encoding="utf-8")
    (root / "src-tauri" / "src" / "ipc" / "onboarding.rs").write_text(
        f'pub(crate) const ONBOARDING_SAMPLE_REL: &str = "{SAMPLE_REL}";\n', encoding="utf-8"
    )
    resources: dict[str, str] = {}
    if mapped:
        resources[f"resources/{rel}"] = rel
    if sample_mapped:
        resources[f"resources/{SAMPLE_REL}"] = SAMPLE_REL
    (root / "src-tauri" / "tauri.conf.json").write_text(
        json.dumps({"bundle": {"resources": resources}}), encoding="utf-8"
    )
    (root / ".gitignore").write_text(IGNORE_LINE + "\n" if ignored else "target\n", encoding="utf-8")

    shipped = root / "src-tauri" / "resources" / SAMPLE_REL
    shipped.parent.mkdir(parents=True, exist_ok=True)
    shipped.write_bytes(b"RIFF-sample-bytes")
    golden = root / "crates" / "luna-cli" / "tests" / "golden-asr" / "en" / "clip.wav"
    golden.parent.mkdir(parents=True, exist_ok=True)
    golden.write_bytes(b"RIFF-sample-bytes" if sample_is_golden else b"RIFF-other-bytes")
    if golden_has_reference:
        golden.with_suffix(".txt").write_text("SHE RAN UPSTAIRS\n", encoding="utf-8")


def self_test() -> int:
    """反向验证：四条判据各自都要能把对应的缺口抓出来，而对齐的样本不得误报。"""
    ok = {"rel": "models/ggml-tiny-q5_1.bin", "url_file": "ggml-tiny-q5_1.bin", "mapped": True, "ignored": True}
    cases: list[tuple[str, dict[str, object], bool]] = [
        ("全部对齐", ok, False),
        ("打包没配资源", {**ok, "mapped": False}, True),
        ("恢复下载指向另一档", {**ok, "url_file": "ggml-tiny.bin"}, True),
        (".gitignore 没覆盖", {**ok, "ignored": False}, True),
        ("常量与资源路径不一致", {**ok, "rel": "models/other.bin", "url_file": "other.bin"}, False),
        ("示例语音没进打包", {**ok, "sample_mapped": False}, True),
        ("示例语音不是黄金样本的副本", {**ok, "sample_is_golden": False}, True),
        ("黄金样本没有参考文本", {**ok, "golden_has_reference": False}, True),
    ]
    for label, kwargs, should_fail in cases:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _fixture(root, **kwargs)  # type: ignore[arg-type]
            failed = bool(problems(root))
            if failed != should_fail:
                verb = "本该判红却放过了" if should_fail else "本该放过却判红了"
                print(f"自测失败：{label} —— {verb}", file=sys.stderr)
                return 1
    print("随包转写档接线守护自测 OK — 七条判据各自都能变红，对齐样本不误报。")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true", help="跑自测而不是校验工作树。")
    args = parser.parse_args()
    if args.self_test:
        return self_test()

    found = problems(REPO)
    if found:
        print("随包转写档接线守护失败：", file=sys.stderr)
        for item in found:
            print(f"  - {item}", file=sys.stderr)
        return 1
    source = (REPO / "src-tauri" / "src" / "model_store.rs").read_text(encoding="utf-8")
    print(
        "随包资源接线 OK — "
        f"{_const_str(source, 'BUNDLED_WHISPER_FILE')} → 资源 "
        f"{_const_str(source, 'BUNDLED_WHISPER_RESOURCE_REL')}；"
        "示例语音与黄金样本逐字节相同（准确率由 bench-asr-golden.sh 覆盖）；"
        "打包与 .gitignore 均已对齐。"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
