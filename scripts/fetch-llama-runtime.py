#!/usr/bin/env python3
"""取回随包推理引擎（llama.cpp 的 `llama-server` 与 `llama-fit-params`，ADR-143）。

与 `fetch-bundled-asr.py` 同理：二进制**不入库**，构建前按 sha256 取回并校验，产物落在
`.gitignore` 覆盖的 `src-tauri/resources/llama/`。

**事实源只有一处**：构建号、包地址、sha256、字节数与文件清单全部从
`crates/luna-engine/src/llama/runtime.rs` 的 `MACOS_ARM64` 读出来。换引擎版本只改那里。

只取清单里的文件（发布包 37 个 dylib 里的 11 个，外加两个可执行文件与 LICENSE），
符号链接解引用成 soname 实名；取完用 `otool -L` 走一遍 `@rpath` 依赖闭包，缺一个就失败
——否则要等用户机器上 dyld 报错才知道。

本平台没有引擎包时（非 Apple Silicon macOS）只建空目录：`bundle.resources` 指着它，
缺目录时 `tauri_build` 在 build script 阶段就会让每条 cargo 命令起不来。应用运行时
`runtime::locate` 找不全文件即如实报「本平台暂无本地引擎」。

用法：
    python3 scripts/fetch-llama-runtime.py           # 取回（已就位则什么都不做）
    python3 scripts/fetch-llama-runtime.py --check   # 只检查，不联网
"""

from __future__ import annotations

import argparse
import hashlib
import platform
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
RUNTIME_RS = ROOT / "crates" / "luna-engine" / "src" / "llama" / "runtime.rs"
TARGET_DIR = ROOT / "src-tauri" / "resources" / "llama"
STAMP = ".build"


def read_spec() -> dict:
    source = RUNTIME_RS.read_text(encoding="utf-8")
    start = source.find("pub const MACOS_ARM64: RuntimeSpec")
    if start < 0:
        sys.exit("runtime.rs 里找不到 MACOS_ARM64")
    body = source[start : source.index("};", start)]

    def field(name: str) -> str:
        match = re.search(rf'{name}:\s*"([^"]+)"', body)
        if not match:
            sys.exit(f"MACOS_ARM64 里找不到字段 {name}")
        return match.group(1)

    size = re.search(r"archive_size:\s*([0-9_]+)", body)
    files = re.search(r"files:\s*&\[(.*?)\]", body, re.S)
    if not size or not files:
        sys.exit("MACOS_ARM64 里找不到 archive_size 或 files")
    return {
        "build": field("build"),
        "url": field("archive_url"),
        "sha256": field("archive_sha256"),
        "size": int(size.group(1).replace("_", "")),
        "root": field("archive_root"),
        "files": re.findall(r'"([^"]+)"', files.group(1)),
    }


def supported_host() -> bool:
    return sys.platform == "darwin" and platform.machine() == "arm64"


def digest(path: Path) -> str:
    hasher = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            hasher.update(chunk)
    return hasher.hexdigest()


def in_place(spec: dict) -> bool:
    stamp = TARGET_DIR / STAMP
    return (
        stamp.is_file()
        and stamp.read_text(encoding="utf-8").strip() == spec["sha256"]
        and all((TARGET_DIR / f).is_file() for f in spec["files"])
    )


def rpath_closure_missing(directory: Path, roots: list[str]) -> list[str]:
    """从可执行文件出发走 `@rpath` 依赖；返回目录里缺的那些。"""
    seen: set[str] = set()
    missing: list[str] = []
    todo = list(roots)
    while todo:
        name = todo.pop()
        out = subprocess.run(["otool", "-L", str(directory / name)], capture_output=True, text=True, check=True)
        for line in out.stdout.splitlines()[1:]:
            match = re.search(r"@rpath/(\S+)", line)
            if not match or match.group(1) in seen:
                continue
            dep = match.group(1)
            seen.add(dep)
            if (directory / dep).is_file():
                todo.append(dep)
            else:
                missing.append(dep)
    return missing


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="只检查是否已就位，不联网；缺失即以 1 退出。")
    args = parser.parse_args()
    spec = read_spec()

    if not supported_host():
        TARGET_DIR.mkdir(parents=True, exist_ok=True)
        print(f"本平台没有随包引擎包（{sys.platform}/{platform.machine()}），只建空目录：{TARGET_DIR.relative_to(ROOT)}")
        return 0

    if in_place(spec):
        print(f"随包推理引擎已就位：{TARGET_DIR.relative_to(ROOT)}（{spec['build']}）")
        return 0
    if args.check:
        print(
            f"随包推理引擎缺失或版本不符：{TARGET_DIR.relative_to(ROOT)}\n"
            "  先跑一次 `python3 scripts/fetch-llama-runtime.py`（约 12 MB，需联网）。",
            file=sys.stderr,
        )
        return 1

    # 走系统 curl：python.org 的 Python 自带 CA 集在不少 macOS 上不可用（同 fetch-bundled-asr.py）。
    curl = shutil.which("curl")
    if curl is None:
        print("找不到 curl。", file=sys.stderr)
        return 1
    with tempfile.TemporaryDirectory() as tmp:
        archive = Path(tmp) / "llama.tar.gz"
        print(f"取回随包推理引擎 {spec['build']}（{spec['size'] / 1_048_576:.1f} MiB）…\n  {spec['url']}", flush=True)
        done = subprocess.run([curl, "-fL", "--retry", "3", "--retry-delay", "2", "-o", str(archive), spec["url"]], check=False)
        if done.returncode != 0:
            print(f"curl 取回失败（退出码 {done.returncode}）。", file=sys.stderr)
            return 1
        size, sha = archive.stat().st_size, digest(archive)
        if size != spec["size"] or sha != spec["sha256"]:
            print(
                f"取回的包与 runtime.rs 声明的不符，已丢弃：\n  期望 {spec['size']} 字节 / {spec['sha256']}\n  实得 {size} 字节 / {sha}",
                file=sys.stderr,
            )
            return 1

        extract = Path(tmp) / "x"
        with tarfile.open(archive) as tar:
            tar.extractall(extract, filter="data")
        source = extract / spec["root"]
        staging = TARGET_DIR.with_name("llama.part")
        shutil.rmtree(staging, ignore_errors=True)
        staging.mkdir(parents=True)
        for name in spec["files"]:
            src = source / name
            if not src.exists():
                print(f"发布包里没有 {name}——清单与包不符。", file=sys.stderr)
                return 1
            # 解引用符号链接：资源打包与签名都只认实体文件。
            shutil.copy2(src.resolve(), staging / name)

        missing = rpath_closure_missing(staging, ["llama-server", "llama-fit-params"])
        if missing:
            print(f"依赖闭包不完整，清单缺：{', '.join(sorted(missing))}", file=sys.stderr)
            return 1
        (staging / STAMP).write_text(spec["sha256"] + "\n", encoding="utf-8")
        shutil.rmtree(TARGET_DIR, ignore_errors=True)
        staging.replace(TARGET_DIR)
    print(f"已就位：{TARGET_DIR.relative_to(ROOT)}（{len(spec['files'])} 个文件）")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
