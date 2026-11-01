# shellcheck shell=bash
# macOS 打包的共用步骤：`scripts/build-and-run.sh`（本机真机验）与 `scripts/release-macos.sh`
# （官网发布，客户端分发 D3）都 source 这一份，签名方式只有一处定义（TDD-13 §3.1）。

# 开发构建与官网发布共用这一张自签证书：TCC 授权与钥匙串 ACL 认的是签名身份（证书哈希 + 标识符），
# 换证书 = 所有用户的系统权限全部重新授权。所以它要一次建好、有效期拉满、加密备份，之后不再换。
LUNARCA_IDENTITY_NAME="Lunarca Code Signing"
LUNARCA_BUNDLE_IDENTIFIER="app.lunarca.luna"

# Tauri CLI 有两种装法，两种都行：`cargo install tauri-cli`（`cargo tauri`）与随前端依赖装的
# JS 包（`apps/desktop/node_modules/.bin/tauri`，同一个原生二进制）。cargo 那一份不在
# rustup 管辖内，清 ~/.cargo/bin 或换机器就会没有——那时不该让作业停在
# 「no such command: tauri」上，直接用工作树里已有的那一份。结果放进数组 TAURI。
resolve_tauri_cli() {
  local root_dir="$1"
  local node_tauri="$root_dir/apps/desktop/node_modules/.bin/tauri"
  if cargo tauri --version >/dev/null 2>&1; then
    TAURI=(cargo tauri)
  elif [[ -x "$node_tauri" ]]; then
    TAURI=("$node_tauri")
  else
    echo "找不到 Tauri CLI：装 cargo 那份（cargo install tauri-cli --version '^2' --locked）" >&2
    echo "或在 apps/desktop 里 npm install 拿回 node_modules/.bin/tauri。" >&2
    return 2
  fi
}

print_codesign_setup() {
  cat >&2 <<EOF

[签名] 没找到可用的稳定代码签名身份「$LUNARCA_IDENTITY_NAME」。一次性准备（需要你本人操作，约 1 分钟）：
  1. 打开「钥匙串访问」，菜单「钥匙串访问 → 证书助理 → 创建证书…」。
  2. 名称填「$LUNARCA_IDENTITY_NAME」，身份类型选「自签名根证书」，证书类型选「代码签名」，
     勾「让我覆盖默认设置」，有效期填 7300 天（官网安装包也用它签，中途换证书用户要重新授权全部系统权限），
     其余保持默认，存进「登录」钥匙串。
  3. 在「登录 → 我的证书」里双击这张证书 → 展开「信任」→「代码签名」设为「始终信任」，
     关窗口时按提示输入登录密码确认。
  4. 终端核对：security find-identity -v -p codesigning 能列出 "$LUNARCA_IDENTITY_NAME"。
  5. 备份：在「我的证书」里右键它 →「导出」为 .p12（设强口令），加密离线保存两份；丢了就只能换证书。
  之后重跑脚本即自动使用它（也可 export LUNARCA_CODESIGN_IDENTITY 指定别的身份）。
  第一次用它签名时 codesign 会问能否使用这把私钥，选「始终允许」；应用首次读取
  「Lunarca Safe Storage」时同样选「始终允许」，此后重编与重启都不再弹窗。

EOF
}

# 找稳定的代码签名身份，结果放进 SIGN_IDENTITY（找不到为空，并打印一次性准备步骤）。
# 来源依次：LUNARCA_CODESIGN_IDENTITY（显式指定，原样使用）→ 钥匙串里名为
# LUNARCA_IDENTITY_NAME 的有效身份。**只查询、不改动钥匙串**：不建证书、不导入密钥、
# 不改信任设置——那一步由用户本人完成。
resolve_codesign_identity() {
  SIGN_IDENTITY=""
  if [[ -n "${LUNARCA_CODESIGN_IDENTITY:-}" ]]; then
    SIGN_IDENTITY="$LUNARCA_CODESIGN_IDENTITY"
    return 0
  fi
  # 先取全量输出再匹配：`set -o pipefail` 下 `grep -q` 提前退出会让上游收到 SIGPIPE、整条管道判假。
  local valid all
  valid="$(/usr/bin/security find-identity -v -p codesigning 2>/dev/null || true)"
  if grep -Fq "\"$LUNARCA_IDENTITY_NAME\"" <<<"$valid"; then
    SIGN_IDENTITY="$LUNARCA_IDENTITY_NAME"
    return 0
  fi
  print_codesign_setup
  all="$(/usr/bin/security find-identity -p codesigning 2>/dev/null || true)"
  if grep -Fq "\"$LUNARCA_IDENTITY_NAME\"" <<<"$all"; then
    echo "[签名] 证书「$LUNARCA_IDENTITY_NAME」已在钥匙串里但尚未被信任：做上面第 3 步即可。" >&2
  fi
}

# 用稳定身份重签整个 .app。先整包深签（随包推理引擎、dylib 各用各的标识符），再单独重签外层
# bundle 钉住固定标识符——`--deep` 与 `--identifier` 同用会把同一个标识符盖到每个嵌套二进制上。
# 不开 hardened runtime：它的硬性用途是公证，这条路不公证（客户端分发方案 §0）；开了还要另配
# 麦克风等 entitlements 并逐个复验随包 dylib 的库校验，而不开的这套签法已在真机上长期验过。
sign_app_bundle() {
  local identity="$1" bundle="$2"
  /usr/bin/codesign --force --deep --sign "$identity" "$bundle" \
    && /usr/bin/codesign --force --sign "$identity" --identifier "$LUNARCA_BUNDLE_IDENTIFIER" "$bundle"
}
