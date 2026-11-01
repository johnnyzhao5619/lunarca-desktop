#!/usr/bin/env bash
set -euo pipefail

# 构建档：默认 debug（连联调服务端）；`--release` 出 release 包——默认地址是生产
# `https://api.lunarca.app`、官方后端只认系统信任库（`luna-cloud` 的 `DEFAULT_API_BASE` /
# `strict_official`），用来在本机真机验「装上正式包的用户连官方云」这条路。它只用本机自签身份
# 重签、没有公证，不能拿去分发。
PROFILE="debug"
if [[ "${1:-}" == "--release" ]]; then
  PROFILE="release"
  shift
fi
MODE="${1:-run}"
# productName（tauri.conf.json）决定 bundle 名；ADR-030 更名后为 Lunarca。
# identifier 已在首个公开安装包前迁到正式值（ADR-030）；旧目录由应用首启改名迁移。
APP_NAME="Lunarca"
PROCESS_NAME="luna-app"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_BUNDLE="$ROOT_DIR/target/$PROFILE/bundle/macos/$APP_NAME.app"
APP_BINARY="$APP_BUNDLE/Contents/MacOS/$PROCESS_NAME"

# `build` 只打包不启动，也就不去结束正在跑的实例——那多半是用户自己开着的那一个。
if [[ "$MODE" != "build" ]]; then
  pkill -x "$PROCESS_NAME" >/dev/null 2>&1 || true
fi

cd "$ROOT_DIR"

# shellcheck source=lib/macos-bundle.sh
source "$ROOT_DIR/scripts/lib/macos-bundle.sh"
resolve_tauri_cli "$ROOT_DIR"

# 随包转写档（PRD-14 §6）：权重不入库，构建前按 model_store.rs 声明的 sha256 取回一次；
# 已就位时什么都不做。缺失且取不回时**在这里失败**——比产出一个「引导第一屏在撒谎」的包好。
python3 "$ROOT_DIR/scripts/fetch-bundled-asr.py"
# 随包推理引擎（ADR-143）：同理，不入库，按 runtime.rs 声明的 sha256 取回。
python3 "$ROOT_DIR/scripts/fetch-llama-runtime.py"

BUILD_FLAGS=(--bundles app --config '{"bundle":{"active":true}}')
if [[ "$PROFILE" == "debug" ]]; then
  BUILD_FLAGS=(--debug "${BUILD_FLAGS[@]}")
fi
"${TAURI[@]}" build "${BUILD_FLAGS[@]}"

# 调试包默认是 linker 生成的 ad-hoc 签名，cdhash 随二进制内容变——而钥匙串条目的 ACL 与「本地
# 网络」授权钉的正是签名身份。于是每重新构建一次，读保险箱主密钥（钥匙串里唯一的那一条，
# TDD-12 §2）都要重新弹一次系统密码框，「始终允许」只管到下次构建为止。用一张**稳定**的自签名
# 代码签名证书、以固定标识符重签，ACL 与授权就一直对得上（TDD-13 §3.1）。
#
# 身份的查找与签法见 scripts/lib/macos-bundle.sh（与发布脚本同一份）；找不到时打印一次性准备步骤，
# 调试包保持 ad-hoc 签名继续——每次重编之后首次启动都会再问一次钥匙串授权。
BUNDLE_IDENTIFIER="$LUNARCA_BUNDLE_IDENTIFIER"
resolve_codesign_identity

SIGNED_STABLE=0
if [[ -n "$SIGN_IDENTITY" ]]; then
  if sign_app_bundle "$SIGN_IDENTITY" "$APP_BUNDLE"; then
    SIGNED_STABLE=1
  else
    echo "codesign 失败：钥匙串授权会照旧每次构建重来（身份 $SIGN_IDENTITY）" >&2
  fi
fi

# 没有稳定签名身份时**不经 `open` 启动**：macOS 的「本地网络」授权与钥匙串 ACL 一样按签名
# 记录，ad-hoc 包每重编一次身份就变，经 LaunchServices 起的进程连局域网服务端（联调机
# 10.0.0.62）一律被拒——界面报「连不上 / 到不了」，curl 却是通的（2026-10-07、10-13 两次
# 真机坐实）。直接执行包内二进制则继承终端的授权。用稳定身份签上了就恢复 `open`：身份稳定，
# 授权一次即长期有效。
APP_LOG="$ROOT_DIR/target/luna-app.log"
open_app() {
  if [[ "$SIGNED_STABLE" == 1 ]]; then
    /usr/bin/open -n "$APP_BUNDLE"
  else
    echo "调试包为 ad-hoc 签名：直接执行包内二进制以继承终端的本地网络授权（日志 $APP_LOG）。" >&2
    nohup "$APP_BINARY" >"$APP_LOG" 2>&1 &
  fi
}

case "$MODE" in
  run)
    open_app
    ;;
  build)
    echo "已打包（$PROFILE）：$APP_BUNDLE" >&2
    ;;
  --debug|debug)
    lldb -- "$APP_BINARY"
    ;;
  --logs|logs)
    open_app
    /usr/bin/log stream --info --style compact --predicate "process == \"$PROCESS_NAME\""
    ;;
  --telemetry|telemetry)
    open_app
    /usr/bin/log stream --info --style compact --predicate "subsystem == \"$BUNDLE_IDENTIFIER\""
    ;;
  --verify|verify)
    open_app
    sleep 1
    pgrep -x "$PROCESS_NAME" >/dev/null
    ;;
  *)
    echo "usage: $0 [--release] [run|build|--debug|--logs|--telemetry|--verify]" >&2
    echo "  --release 出 release 包（连生产）；--debug 是用 lldb 起应用，不是构建档" >&2
    exit 2
    ;;
esac
