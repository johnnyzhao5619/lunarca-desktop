#!/usr/bin/env bash
set -euo pipefail

# 官网发布用的 macOS 安装包（客户端分发 D3，TDD-13 §5）。不加入 Apple Developer Program：没有
# Developer ID 与公证，签名用本机那张固定的自签证书（与 build-and-run.sh 同一份签法），自动更新
# 用发布者自己的 minisign 密钥（tauri-plugin-updater）。构建与签名都在所有者的 Mac 上做，不进 CI。
#
# 产物（本脚本不联网：源码快照由 scripts/publish-source-snapshot.sh 推到公开仓，安装包由 vps-edge 的
# scripts/publish-lunarca-release.sh 传到 https://lunarca.app/releases/，先源码后安装包）：
#   <输出>/latest.json                              更新清单（应用检查更新时读它）
#   <输出>/<版本>/Lunarca_<版本>_aarch64.dmg          首次安装用（官网下载页链到它）
#   <输出>/<版本>/Lunarca_<版本>_aarch64.app.tar.gz   自动更新包（.sig 是它的 minisign 签名）
#   <输出>/<版本>/SHA256SUMS
#
# 两种用法：
#   scripts/release-macos.sh --notes <发布说明.txt>
#       正式发布。要求：已提交的干净工作树、tauri.conf.json 里是真公钥、签名私钥在
#       ${LUNARCA_UPDATER_KEY:-~/.tauri/lunarca-updater.key}（口令取 TAURI_SIGNING_PRIVATE_KEY_PASSWORD，
#       没设就在这里问一次）。输出到 target/release-dist/。
#   scripts/release-macos.sh --rehearsal --notes <发布说明.txt> [--version 1.1.1] [--port 8765]
#       演练：一次性密钥（target/release-rehearsal/updater.key，跨次复用，两次演练之间才升得上去）、
#       清单地址指向 http://127.0.0.1:<端口>，可覆盖版本号。不碰真密钥、不要求工作树干净，
#       用来在临时 HOME 里走「装旧版 → 应用内升级到新版」。产物**不能**拿去分发。
#
# 更新包签好之后当场用「编进这个包的公钥」验一次签（scripts/release-manifest.mjs）：私钥与公钥
# 不是一对时签名照样签得出来，但每个用户都会装不上——在这里拦下，而不是发出去之后才知道。

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/macos-bundle.sh
source "$ROOT_DIR/scripts/lib/macos-bundle.sh"

usage() {
  echo "usage: $0 [--rehearsal [--version X.Y.Z] [--port N]] --notes <发布说明.txt>" >&2
  exit 2
}

REHEARSAL=0
NOTES_FILE=""
VERSION_OVERRIDE=""
PORT=8765
while [[ $# -gt 0 ]]; do
  case "$1" in
    --rehearsal) REHEARSAL=1; shift ;;
    --notes) NOTES_FILE="${2:-}"; shift 2 ;;
    --version) VERSION_OVERRIDE="${2:-}"; shift 2 ;;
    --port) PORT="${2:-}"; shift 2 ;;
    *) usage ;;
  esac
done
[[ -n "$NOTES_FILE" && -s "$NOTES_FILE" ]] || { echo "缺发布说明：--notes 指向一个非空的纯文本文件" >&2; usage; }
NOTES_FILE="$(cd "$(dirname "$NOTES_FILE")" && pwd)/$(basename "$NOTES_FILE")"
if [[ "$REHEARSAL" == 0 && ( -n "$VERSION_OVERRIDE" || "$PORT" != 8765 ) ]]; then
  echo "--version / --port 只用于演练：正式发布的版本号以 tauri.conf.json 为准" >&2
  exit 2
fi

cd "$ROOT_DIR"
CONF="$ROOT_DIR/src-tauri/tauri.conf.json"
APP_NAME="Lunarca"
APP_BUNDLE="$ROOT_DIR/target/release/bundle/macos/$APP_NAME.app"

if [[ "$REHEARSAL" == 1 ]]; then
  OUT_DIR="$ROOT_DIR/target/release-rehearsal/dist"
  BASE_URL="http://127.0.0.1:$PORT"
  KEY_FILE="$ROOT_DIR/target/release-rehearsal/updater.key"
  mkdir -p "$(dirname "$KEY_FILE")"
  resolve_tauri_cli "$ROOT_DIR"
  if [[ ! -f "$KEY_FILE" ]]; then
    "${TAURI[@]}" signer generate --ci -p "" -w "$KEY_FILE" >/dev/null
  fi
  export TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""
  PUBKEY="$(cat "$KEY_FILE.pub")"
  VERSION="${VERSION_OVERRIDE:-$(node -p "require('$CONF').version")}"
  BUILD_CONFIG="$(node -e '
    const [pubkey, endpoint, version] = process.argv.slice(1);
    console.log(JSON.stringify({
      version,
      bundle: { active: true },
      plugins: { updater: { pubkey, endpoints: [endpoint], dangerousInsecureTransportProtocol: true } },
    }));' "$PUBKEY" "$BASE_URL/latest.json" "$VERSION")"
else
  OUT_DIR="$ROOT_DIR/target/release-dist"
  BASE_URL="https://lunarca.app/releases"
  KEY_FILE="${LUNARCA_UPDATER_KEY:-$HOME/.tauri/lunarca-updater.key}"
  if ! git -C "$ROOT_DIR" diff --quiet HEAD --; then
    echo "工作树有未提交的改动：正式安装包只从已提交的版本构建（发出去的每个字节都要能对上一个提交）" >&2
    exit 1
  fi
  PUBKEY="$(node -p "require('$CONF').plugins.updater.pubkey")"
  node --input-type=module -e '
    import { parsePublicKey } from "./scripts/release-manifest.mjs";
    parsePublicKey(process.argv[1]);' "$PUBKEY"
  [[ -f "$KEY_FILE" ]] || { echo "找不到 updater 私钥：$KEY_FILE（或用 LUNARCA_UPDATER_KEY 指定）" >&2; exit 1; }
  if [[ -z "${TAURI_SIGNING_PRIVATE_KEY_PASSWORD+set}" ]]; then
    read -rsp "updater 私钥口令（不回显）: " TAURI_SIGNING_PRIVATE_KEY_PASSWORD
    echo >&2
    export TAURI_SIGNING_PRIVATE_KEY_PASSWORD
  fi
  resolve_tauri_cli "$ROOT_DIR"
  VERSION="$(node -p "require('$CONF').version")"
  BUILD_CONFIG='{"bundle":{"active":true}}'
fi

resolve_codesign_identity
[[ -n "$SIGN_IDENTITY" ]] || { echo "发布包必须用稳定身份签名（见上面的一次性准备步骤）" >&2; exit 1; }
# ad-hoc（`-`）只许演练用：它每次构建身份都变，用户升一次级系统权限与钥匙串授权就全部重来。
if [[ "$SIGN_IDENTITY" == "-" && "$REHEARSAL" == 0 ]]; then
  echo "正式发布不能用 ad-hoc 签名（LUNARCA_CODESIGN_IDENTITY=-）：升级后用户的系统权限会全部失效" >&2
  exit 1
fi

DIST="$OUT_DIR/$VERSION"
if [[ "$REHEARSAL" == 0 && -e "$DIST" ]]; then
  echo "$DIST 已存在：同一个版本号只发一次（要重发就升版本号）" >&2
  exit 1
fi

# 随包转写档与推理引擎不入库，构建前按声明的 sha256 取回（同 build-and-run.sh）。
python3 "$ROOT_DIR/scripts/fetch-bundled-asr.py"
python3 "$ROOT_DIR/scripts/fetch-llama-runtime.py"

"${TAURI[@]}" build --bundles app --config "$BUILD_CONFIG"

BUILT_VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP_BUNDLE/Contents/Info.plist")"
[[ "$BUILT_VERSION" == "$VERSION" ]] || { echo "打出来的包版本是 $BUILT_VERSION，不是 $VERSION" >&2; exit 1; }

sign_app_bundle "$SIGN_IDENTITY" "$APP_BUNDLE"
/usr/bin/codesign --verify --deep --strict "$APP_BUNDLE"

rm -rf "$DIST"
mkdir -p "$DIST"
STEM="${APP_NAME}_${VERSION}_aarch64"
ARCHIVE="$DIST/$STEM.app.tar.gz"
DMG="$DIST/$STEM.dmg"

# 更新包：tar.gz 顶层就是 Lunarca.app（tauri-plugin-updater 解开后整包替换）。
# COPYFILE_DISABLE 防止 macOS tar 塞进 ._ 元数据文件。
COPYFILE_DISABLE=1 tar -czf "$ARCHIVE" -C "$(dirname "$APP_BUNDLE")" "$APP_NAME.app"
"${TAURI[@]}" signer sign -f "$KEY_FILE" "$ARCHIVE" >/dev/null

# 首次安装用的 DMG：里面是签好的 .app 和一个指向「应用程序」的链接，拖进去即装。
STAGING="$(mktemp -d)"
trap 'rm -rf "$STAGING"' EXIT
/usr/bin/ditto "$APP_BUNDLE" "$STAGING/$APP_NAME.app"
ln -s /Applications "$STAGING/Applications"
/usr/bin/hdiutil create -quiet -volname "$APP_NAME" -srcfolder "$STAGING" -fs HFS+ -format UDZO -ov "$DMG"
/usr/bin/codesign --force --sign "$SIGN_IDENTITY" "$DMG"

MANIFEST_ARGS=(--archive "$ARCHIVE" --sig "$ARCHIVE.sig" --version "$VERSION" --notes "$NOTES_FILE"
  --base-url "$BASE_URL" --out "$OUT_DIR/latest.json" --pubkey "$PUBKEY")
node "$ROOT_DIR/scripts/release-manifest.mjs" "${MANIFEST_ARGS[@]}"

(cd "$DIST" && shasum -a 256 "$STEM.dmg" "$STEM.app.tar.gz" > SHA256SUMS)
COMMIT="$(git -C "$ROOT_DIR" rev-parse HEAD)"
git -C "$ROOT_DIR" diff --quiet HEAD -- || COMMIT="$COMMIT+dirty"
echo "$COMMIT" > "$DIST/COMMIT"

echo >&2
echo "[发布] $VERSION 已就绪：$OUT_DIR" >&2
if [[ "$REHEARSAL" == 1 ]]; then
  echo "[演练] 起本机清单服务：python3 -m http.server $PORT --bind 127.0.0.1 --directory $OUT_DIR" >&2
  echo "[演练] 在临时 HOME 里装上旧版本的 .app，打开「设置 › 通用 › 关于与更新」点检查更新。" >&2
else
  echo "[发布] 下一步 1：公开对应源码 scripts/publish-source-snapshot.sh $VERSION $COMMIT" >&2
  echo "[发布] 下一步 2：vps-edge 的 scripts/publish-lunarca-release.sh $OUT_DIR $VERSION（先核对公开仓上有这个版本的源码，再传安装包）。" >&2
fi
