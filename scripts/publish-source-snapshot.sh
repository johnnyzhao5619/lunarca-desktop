#!/usr/bin/env bash
set -euo pipefail

# 把某个发布版本的源码推到公开仓 https://github.com/johnnyzhao5619/lunarca-desktop（AGPL-3.0 对应源码）。
# 本仓（开发仓）保持私有、历史不改写；公开仓每个版本只有一个快照提交，历史里没有开发过程与规格。
# 只给开发用的文件（agent 指引、绑自托管 runner 的 CI、只管这些文件的门禁）不进公开仓，清单是
# scripts/public-snapshot-exclude.txt——它本身随快照公开，读者看得到少了哪些、为什么。
#
#   scripts/publish-source-snapshot.sh <版本> [<开发仓提交>] [--dry-run] [--remote <地址>]
#
# 开发仓提交缺省 HEAD；正式发布时传安装包产物里的 COMMIT（target/release-dist/<版本>/COMMIT），
# 两边才是同一份源码。顺序：先推本快照，再上传安装包（vps-edge 的 publish-lunarca-release.sh 会核对
# 公开仓上有没有这个版本的标签、标签里记的开发仓提交与安装包是否一致）。
#
# 做法：把那个提交的树减去排除清单（临时索引 read-tree → rm --cached → write-tree，确定性），
# 用 git commit-tree 造快照提交，父提交是公开仓上一个快照。其余文件的 blob 原样复用，所以任何人拿
# 开发仓提交按同一份清单重算都得到同一棵树；推送只带走快照链上可达的对象，开发仓历史一个提交也不会出去。标签 v<版本> 只推到远端，不在本仓建（本仓有同名的内部标签）。

DEFAULT_REMOTE="https://github.com/johnnyzhao5619/lunarca-desktop.git"
SNAPSHOT_REF="refs/source-snapshot/main"
TRAILER="Source-Commit"
EXCLUDE_FILE="scripts/public-snapshot-exclude.txt"

usage() {
  echo "usage: $0 <版本> [<开发仓提交>] [--dry-run] [--remote <地址>]" >&2
  exit 2
}

VERSION=""
COMMIT_ARG=""
DRY_RUN=0
REMOTE="$DEFAULT_REMOTE"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run) DRY_RUN=1; shift ;;
    --remote) REMOTE="${2:-}"; [[ -n "$REMOTE" ]] || usage; shift 2 ;;
    -*) usage ;;
    *)
      if [[ -z "$VERSION" ]]; then VERSION="$1"
      elif [[ -z "$COMMIT_ARG" ]]; then COMMIT_ARG="$1"
      else usage
      fi
      shift ;;
  esac
done
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$ ]] || { echo "版本号要像 1.2.0：$VERSION" >&2; usage; }

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
G=(git -C "$ROOT_DIR")
fail() { printf '✗ %s\n' "$*" >&2; exit 1; }
ok() { printf '✓ %s\n' "$*"; }

[[ "${COMMIT_ARG:-HEAD}" != *+dirty ]] || fail "提交带 +dirty：未提交工作树打出的包没有对应源码，不能发布"
COMMIT="$("${G[@]}" rev-parse --verify "${COMMIT_ARG:-HEAD}^{commit}")" || fail "找不到提交 ${COMMIT_ARG:-HEAD}"
TAG="v$VERSION"

# 1. 这个提交确实是这一版
CONF_VERSION="$("${G[@]}" show "$COMMIT:src-tauri/tauri.conf.json" | node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>console.log(JSON.parse(s).version))')"
[[ "$CONF_VERSION" == "$VERSION" ]] || fail "提交 ${COMMIT:0:12} 的 tauri.conf.json 版本是 $CONF_VERSION，不是 $VERSION"

# 2. 公开的那棵树：开发仓提交的树减去排除清单（清单取自同一个提交，不读工作树）
EXCLUDE_TEXT="$("${G[@]}" show "$COMMIT:$EXCLUDE_FILE" 2>/dev/null)" || fail "提交 ${COMMIT:0:12} 里没有 $EXCLUDE_FILE"
EXCLUDES=()
while IFS= read -r line; do
  line="${line%%#*}"; line="${line%"${line##*[![:space:]]}"}"
  [[ -z "$line" ]] || EXCLUDES+=("$line")
done <<<"$EXCLUDE_TEXT"
INDEX="$(mktemp)"
trap 'rm -f "$INDEX"' EXIT
GIT_INDEX_FILE="$INDEX" "${G[@]}" read-tree "$COMMIT"
for path in "${EXCLUDES[@]}"; do
  # 清单里写了却已不存在的路径说明清单陈旧了：停下改清单，免得以为排除了其实早改名漏出去
  [[ -n "$(GIT_INDEX_FILE="$INDEX" "${G[@]}" ls-files -- "$path")" ]] || fail "$EXCLUDE_FILE 里的 $path 在提交里不存在：清单陈旧"
  GIT_INDEX_FILE="$INDEX" "${G[@]}" rm -r --cached --quiet -- "$path"
done
TREE="$(GIT_INDEX_FILE="$INDEX" "${G[@]}" write-tree)"

# 3. 发出去之前查一遍内容：私钥、常见云凭据、超大文件。命中就停，查清再发——公开仓推上去就收不回。
SECRET_PATTERN='BEGIN [A-Z ]*PRIVATE KEY|sk-[A-Za-z0-9]{20,}|gh[pousr]_[A-Za-z0-9]{30,}|AKIA[0-9A-Z]{16}|xox[baprs]-[A-Za-z0-9-]{10,}|AIza[0-9A-Za-z_-]{35}|GOCSPX-[A-Za-z0-9_-]{20,}|whsec_[A-Za-z0-9]{20,}|(sk|rk)_live_[A-Za-z0-9]{20,}'
if hits="$("${G[@]}" grep -nIE "$SECRET_PATTERN" "$TREE" -- . 2>/dev/null)"; then
  printf '%s\n' "$hits" | sed "s/^$TREE://" | head -20 >&2
  fail "快照里有疑似私钥或凭据（见上）"
fi
big="$("${G[@]}" ls-tree -r -l "$TREE" | awk '$4 > 20*1024*1024 {print $4, $5}')"
[[ -z "$big" ]] || { printf '%s\n' "$big" >&2; fail "快照里有超过 20 MiB 的文件（见上）：大文件应走构建时下载，不进源码"; }
ok "内容核查通过：$("${G[@]}" ls-tree -r "$TREE" | wc -l | tr -d ' ') 个文件（排除 ${#EXCLUDES[@]} 项），树 ${TREE:0:12}"

# 4. 公开仓现状：上一个快照（第一版时为空）与这个版本的标签
"${G[@]}" fetch --quiet --no-tags "$REMOTE" "+refs/heads/main:$SNAPSHOT_REF" 2>/dev/null || "${G[@]}" update-ref -d "$SNAPSHOT_REF" 2>/dev/null || true
PARENT="$("${G[@]}" rev-parse --verify --quiet "$SNAPSHOT_REF" || true)"
REMOTE_TAG="$("${G[@]}" ls-remote "$REMOTE" "refs/tags/$TAG" | cut -f1)"
if [[ -n "$REMOTE_TAG" ]]; then
  "${G[@]}" fetch --quiet --no-tags "$REMOTE" "refs/tags/$TAG"
  [[ "$("${G[@]}" rev-parse "$REMOTE_TAG^{tree}")" == "$TREE" ]] \
    || fail "公开仓已有 $TAG，但内容不是 ${COMMIT:0:12}：同一个版本号只发一次"
  ok "公开仓已有 $TAG，内容与 ${COMMIT:0:12} 相同，不用再推"
  exit 0
fi
if [[ -n "$PARENT" ]]; then
  # 快照链上的每个提交都必须是本脚本造的（带 Source-Commit），否则这条链可能连着开发仓历史。
  while read -r c; do
    "${G[@]}" log -1 --format=%B "$c" | "${G[@]}" interpret-trailers --parse | grep -q "^$TRAILER: " \
      || fail "公开仓 main 上的 ${c:0:12} 不是快照提交（没有 $TRAILER）：先查清公开仓被推过什么"
  done < <("${G[@]}" rev-list "$PARENT")
  [[ "$("${G[@]}" rev-parse "$PARENT^{tree}")" != "$TREE" ]] || fail "公开仓 main 已经是这份源码，但没有 $TAG 标签：先查清再补标签"
fi

# 5. 造快照提交（作者与时间取开发仓提交，重跑结果不变）
MESSAGE="$(printf 'Lunarca %s\n\n%s: %s\n' "$VERSION" "$TRAILER" "$COMMIT")"
SNAPSHOT="$(
  GIT_AUTHOR_NAME="$("${G[@]}" log -1 --format=%an "$COMMIT")" \
  GIT_AUTHOR_EMAIL="$("${G[@]}" log -1 --format=%ae "$COMMIT")" \
  GIT_AUTHOR_DATE="$("${G[@]}" log -1 --format=%aI "$COMMIT")" \
  GIT_COMMITTER_NAME="$("${G[@]}" log -1 --format=%an "$COMMIT")" \
  GIT_COMMITTER_EMAIL="$("${G[@]}" log -1 --format=%ae "$COMMIT")" \
  GIT_COMMITTER_DATE="$("${G[@]}" log -1 --format=%aI "$COMMIT")" \
  "${G[@]}" commit-tree "$TREE" ${PARENT:+-p "$PARENT"} -m "$MESSAGE"
)"
[[ "$("${G[@]}" rev-parse "$SNAPSHOT^{tree}")" == "$TREE" ]] || fail "快照树与按清单重算的树不一致"
[[ "$("${G[@]}" rev-list --count "$SNAPSHOT")" == "$(( ${PARENT:+$("${G[@]}" rev-list --count "$PARENT")} + 1 ))" ]] \
  || fail "快照提交的祖先数不对：可能连上了开发仓历史"
ok "快照提交 ${SNAPSHOT:0:12}（${PARENT:+父 ${PARENT:0:12}}${PARENT:-第一版，无父提交}）"

if [[ "$DRY_RUN" == 1 ]]; then
  echo "[演练] 不推送。正式推送：$0 $VERSION $COMMIT" >&2
  exit 0
fi

# 6. 推送：main 与标签一次推完（--atomic），不会只到一半
"${G[@]}" push --atomic "$REMOTE" "$SNAPSHOT:refs/heads/main" "$SNAPSHOT:refs/tags/$TAG"
"${G[@]}" update-ref "$SNAPSHOT_REF" "$SNAPSHOT"
[[ "$("${G[@]}" ls-remote "$REMOTE" "refs/tags/$TAG" | cut -f1)" == "$SNAPSHOT" ]] || fail "推送后远端 $TAG 不是 ${SNAPSHOT:0:12}"
ok "已公开：$REMOTE  $TAG → ${SNAPSHOT:0:12}（开发仓 ${COMMIT:0:12}）"
