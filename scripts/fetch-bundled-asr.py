#!/usr/bin/env python3
"""取回随包转写档的权重（`bundle.resources` 的源文件）。

为什么需要这一步：随包档是「零下载即可在引导内完成首个任务」的全部依据
（PRD-14 §6），但 30.7 MB 的二进制**不入库**——AGPL 仓库的历史一旦背上它就再也卸不掉。
于是权重在构建前由本脚本按 sha256 取回并校验，产物落在 `.gitignore` 覆盖的目录里。

**事实源只有一处**：地址、大小与 sha256 从模型目录 `crates/luna-model-catalog/bundled.json`
里标了 `bundled` 的那一条读出来（ADR-145），资源落点从 `src-tauri/src/model_store.rs` 的
`BUNDLED_WHISPER_RESOURCE_REL` 读。换档时只改目录那一条，本脚本自动跟着走；
在这里抄一份，迟早会出现"脚本下的是 A、应用校验的是 B"。

用法：
    python3 scripts/fetch-bundled-asr.py           # 取回（已就位则什么都不做）
    python3 scripts/fetch-bundled-asr.py --check   # 只检查，不联网（CI / 构建前置）
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MODEL_STORE = ROOT / "src-tauri" / "src" / "model_store.rs"
CATALOG = ROOT / "crates" / "luna-model-catalog" / "bundled.json"
RESOURCE_BASE = ROOT / "src-tauri" / "resources"


def _const_str(source: str, name: str) -> str:
    match = re.search(rf'{name}:\s*&str\s*=\s*"([^"]+)"', source)
    if not match:
        sys.exit(f"在 model_store.rs 里找不到常量 {name}")
    return match.group(1)


def read_spec() -> tuple[str, str, str, int]:
    """→ (资源相对路径, 下载地址, sha256, 字节数)。"""
    rel = _const_str(MODEL_STORE.read_text(encoding="utf-8"), "BUNDLED_WHISPER_RESOURCE_REL")
    bundled = [e for e in json.loads(CATALOG.read_text(encoding="utf-8"))["entries"] if e.get("bundled")]
    if len(bundled) != 1:
        sys.exit(f"{CATALOG.relative_to(ROOT)} 里标了 bundled 的条目应恰好一条，实际 {len(bundled)} 条")
    weights = bundled[0]["weights"]
    url = next((s["url"] for s in weights["sources"] if s.get("kind") == "url"), None)
    if url is None:
        sys.exit("随包档条目没有直连来源")
    return rel, url, weights["sha256"], int(weights["size_bytes"])


def digest(path: Path) -> str:
    hasher = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            hasher.update(chunk)
    return hasher.hexdigest()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="只检查是否已就位且 sha256 相符，不联网；缺失即以 1 退出。",
    )
    args = parser.parse_args()

    rel, url, expected_sha, expected_size = read_spec()
    target = RESOURCE_BASE / rel

    if target.is_file() and target.stat().st_size == expected_size:
        if digest(target) == expected_sha:
            print(f"随包转写档已就位：{target.relative_to(ROOT)}")
            return 0
        print(f"随包转写档 sha256 不符，将重新取回：{target.relative_to(ROOT)}")

    if args.check:
        print(
            f"随包转写档缺失或损坏：{target.relative_to(ROOT)}\n"
            "  先跑一次 `python3 scripts/fetch-bundled-asr.py`（约 31 MB，需联网）。",
            file=sys.stderr,
        )
        return 1

    target.parent.mkdir(parents=True, exist_ok=True)
    # 先落临时名再改名：下到一半被打断时，留下的不会是一个"大小对不上却看着像成品"的文件。
    staging = target.with_suffix(target.suffix + ".part")
    # flush：下面 curl 的进度条直接写终端，不冲一下的话这句会排在它后面。
    print(f"取回随包转写档（{expected_size / 1_048_576:.1f} MiB）…\n  {url}", flush=True)
    # 走系统 curl 而不是 urllib：python.org 那份 Python 自带的 CA 集要另跑一次
    # `Install Certificates.command` 才可用，于是 urllib 在相当一部分 macOS 机器上直接
    # 以 CERTIFICATE_VERIFY_FAILED 停下——那是一条与本脚本毫无关系、却要每个人各查一遍的
    # 岔路。curl 用系统信任库，macOS / Linux / Windows 10+ 都自带。校验仍由下面自己做。
    curl = shutil.which("curl")
    if curl is None:
        print("找不到 curl；装一个，或手工把权重放到上面那个路径。", file=sys.stderr)
        return 1
    # `-f` 让 HTTP 错误码算失败（而不是把一页 HTML 存成权重），`-L` 跟随 HF 的重定向，
    # `--retry` 覆盖掉偶发的网络抖动。
    completed = subprocess.run(
        [curl, "-fL", "--retry", "3", "--retry-delay", "2", "-o", str(staging), url],
        check=False,
    )
    if completed.returncode != 0:
        staging.unlink(missing_ok=True)
        print(f"curl 取回失败（退出码 {completed.returncode}）。", file=sys.stderr)
        return 1

    actual_size = staging.stat().st_size
    actual_sha = digest(staging)
    if actual_size != expected_size or actual_sha != expected_sha:
        staging.unlink(missing_ok=True)
        print(
            "取回的文件与模型目录声明的不符，已丢弃：\n"
            f"  期望 {expected_size} 字节 / {expected_sha}\n"
            f"  实得 {actual_size} 字节 / {actual_sha}",
            file=sys.stderr,
        )
        return 1

    staging.replace(target)
    print(f"已就位：{target.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
