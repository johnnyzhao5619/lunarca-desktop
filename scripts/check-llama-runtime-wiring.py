#!/usr/bin/env python3
"""随包推理引擎的接线守护（静态，不联网；ADR-143 ①）。

引擎要真的随包、真的能跑，得靠**四处对齐**，任一处掉队时构建照常绿、应用照常起，
只有用户点开本地 AI 时才发现「引擎不在」或「引擎一启动就退出」：

  1. `tauri.conf.json` 的 `bundle.resources` 把 `resources/<RESOURCE_DIR>/` 映射到
     `<RESOURCE_DIR>/`——否则引擎根本不进 .app；
  2. `.gitignore` 覆盖 `/src-tauri/resources/<RESOURCE_DIR>/`——十几 MB 的二进制不得入库；
  3. `tauri.conf.json` 的 `bundle.macOS.minimumSystemVersion` 与 `.cargo/config.toml` 的
     `MACOSX_DEPLOYMENT_TARGET` 都等于 `runtime.rs` 的 `MIN_MACOS`（引擎包的 `minos`）——
     低于它的系统上应用能装能开，引擎却在 dyld 阶段退出；
  4. 文件清单含 `llama-server`、`llama-fit-params` 与 `LICENSE`（MIT 要求随附许可）。

事实源是 `crates/luna-engine/src/llama/runtime.rs`；本脚本只读它，不抄。

用法：
    python3 scripts/check-llama-runtime-wiring.py              # 校验工作树
    python3 scripts/check-llama-runtime-wiring.py --self-test  # 自测（证明断言非空跑）
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
RUNTIME_RS = Path("crates/luna-engine/src/llama/runtime.rs")
REQUIRED_FILES = ("llama-server", "llama-fit-params", "LICENSE")


def _const(source: str, name: str) -> str | None:
    match = re.search(rf'pub const {name}: &str = "([^"]+)"', source)
    return match.group(1) if match else None


def problems(root: Path) -> list[str]:
    runtime = root / RUNTIME_RS
    tauri_conf = root / "src-tauri" / "tauri.conf.json"
    cargo_conf = root / ".cargo" / "config.toml"
    gitignore = root / ".gitignore"
    for path in (runtime, tauri_conf, cargo_conf, gitignore):
        if not path.is_file():
            return [f"读不到 {path.relative_to(root)}"]
    source = runtime.read_text(encoding="utf-8")
    resource_dir = _const(source, "RESOURCE_DIR")
    min_macos = _const(source, "MIN_MACOS")
    if resource_dir is None or min_macos is None:
        return ["runtime.rs 里缺 RESOURCE_DIR / MIN_MACOS"]

    found: list[str] = []
    bundle = json.loads(tauri_conf.read_text(encoding="utf-8")).get("bundle", {})
    resources = bundle.get("resources")
    src, dst = f"resources/{resource_dir}/", f"{resource_dir}/"
    if not isinstance(resources, dict) or resources.get(src) != dst:
        found.append(f"tauri.conf.json 的 bundle.resources 没有把 {src!r} 映射到 {dst!r}——引擎不会进 .app")
    ignore = f"/src-tauri/resources/{resource_dir}/"
    if ignore not in gitignore.read_text(encoding="utf-8").splitlines():
        found.append(f".gitignore 没有覆盖 {ignore}——引擎二进制会被误提交进历史")
    tauri_min = bundle.get("macOS", {}).get("minimumSystemVersion")
    if tauri_min != min_macos:
        found.append(f"tauri.conf.json minimumSystemVersion = {tauri_min!r}，引擎要求 {min_macos!r}")
    target = re.search(r'MACOSX_DEPLOYMENT_TARGET\s*=\s*"([^"]+)"', cargo_conf.read_text(encoding="utf-8"))
    if not target or target.group(1) != min_macos:
        found.append(
            f".cargo/config.toml MACOSX_DEPLOYMENT_TARGET = {target.group(1) if target else None!r}，"
            f"引擎要求 {min_macos!r}"
        )
    files = re.search(r"files:\s*&\[(.*?)\]", source, re.S)
    listed = set(re.findall(r'"([^"]+)"', files.group(1))) if files else set()
    for name in REQUIRED_FILES:
        if name not in listed:
            found.append(f"runtime.rs 的文件清单里没有 {name}")
    return found


def _fixture(root: Path, *, mapped=True, ignored=True, tauri_min="13.3", cargo_min="13.3", with_license=True) -> None:
    (root / RUNTIME_RS).parent.mkdir(parents=True)
    files = ['"llama-server"', '"llama-fit-params"'] + (['"LICENSE"'] if with_license else [])
    (root / RUNTIME_RS).write_text(
        'pub const RESOURCE_DIR: &str = "llama";\npub const MIN_MACOS: &str = "13.3";\n'
        f"pub const MACOS_ARM64: RuntimeSpec = RuntimeSpec {{\n    files: &[{', '.join(files)}],\n}};\n",
        encoding="utf-8",
    )
    (root / "src-tauri").mkdir()
    bundle = {"resources": {"resources/llama/": "llama/"} if mapped else {}, "macOS": {"minimumSystemVersion": tauri_min}}
    (root / "src-tauri" / "tauri.conf.json").write_text(json.dumps({"bundle": bundle}), encoding="utf-8")
    (root / ".cargo").mkdir()
    (root / ".cargo" / "config.toml").write_text(f'[env]\nMACOSX_DEPLOYMENT_TARGET = "{cargo_min}"\n', encoding="utf-8")
    (root / ".gitignore").write_text("/src-tauri/resources/llama/\n" if ignored else "target\n", encoding="utf-8")


def self_test() -> int:
    cases = [
        ("全部对齐", {}, False),
        ("打包没配引擎目录", {"mapped": False}, True),
        (".gitignore 没覆盖", {"ignored": False}, True),
        ("tauri 最低系统没升", {"tauri_min": "10.15"}, True),
        ("cargo 部署目标没升", {"cargo_min": "10.15"}, True),
        ("清单漏了 LICENSE", {"with_license": False}, True),
    ]
    for label, kwargs, should_fail in cases:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _fixture(root, **kwargs)
            if bool(problems(root)) != should_fail:
                verb = "本该判红却放过了" if should_fail else "本该放过却判红了"
                print(f"自测失败：{label} —— {verb}", file=sys.stderr)
                return 1
    print("随包推理引擎接线守护自测 OK — 五条判据各自都能变红，对齐样本不误报。")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true", help="跑自测而不是校验工作树。")
    if parser.parse_args().self_test:
        return self_test()
    found = problems(REPO)
    if found:
        print("随包推理引擎接线守护失败：", file=sys.stderr)
        for item in found:
            print(f"  - {item}", file=sys.stderr)
        return 1
    print("随包推理引擎接线 OK — 打包映射、.gitignore、最低系统版本、文件清单四处对齐。")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
