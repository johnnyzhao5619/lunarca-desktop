#!/usr/bin/env bash
# 转写准确率冒烟（TDD-13 §2 转写侧半边；macOS 真机作业项，刻意不叫 check-*、不进 CI）。
#
# 两半：① 用系统 `say` 合成两段**确定性**语音（文本就在下面，音频不进仓）守「链路没断、
# 口径没漂」；② `crates/luna-cli/tests/golden-asr/<lang>/*.wav` 里的真实语音样本（来源与许可证
# 见同目录 ATTRIBUTION.md）守真实口音下的 CER / WER。两半都对给定 ggml 模型跑
# `luna bench-asr --reference`。
#
# 用法：scripts/bench-asr-golden.sh <ggml 模型路径> [luna 二进制，缺省 target/release/luna]
set -euo pipefail
MODEL="${1:?用法：$0 <ggml 模型路径> [luna 二进制]}"
LUNA="${2:-target/release/luna}"
command -v say >/dev/null || { echo "需要 macOS 的 say" >&2; exit 2; }
[[ -x "$LUNA" ]] || { echo "找不到可执行的 $LUNA（先 cargo build --release -p luna-cli）" >&2; exit 2; }
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
ZH_TEXT="今天下午三点，我们在会议室讨论下个季度的产品路线图，请大家提前准备好各自的数据。"
EN_TEXT="The quick brown fox jumps over the lazy dog while the meeting starts at three in the afternoon."
printf '%s' "$ZH_TEXT" > "$WORK/zh.txt"
printf '%s' "$EN_TEXT" > "$WORK/en.txt"
say -v "Flo (Chinese (China mainland))" -o "$WORK/zh.wav" --data-format=LEI16@16000 "$ZH_TEXT"
say -v Samantha -o "$WORK/en.wav" --data-format=LEI16@16000 "$EN_TEXT" 2>/dev/null \
  || say -v Albert -o "$WORK/en.wav" --data-format=LEI16@16000 "$EN_TEXT"
for lang in zh en; do
  echo "== synthetic $lang"
  "$LUNA" bench-asr "$WORK/$lang.wav" --model "$MODEL" --runs 1 --lang "$lang" --reference "$WORK/$lang.txt" \
    | grep -E '^(run 1|cer|wer|hypothesis)'
done
GOLDEN="$(cd "$(dirname "$0")/.." && pwd)/crates/luna-cli/tests/golden-asr"
for wav in "$GOLDEN"/*/*.wav; do
  [[ -f "$wav" ]] || continue
  lang="$(basename "$(dirname "$wav")")"
  ref="${wav%.wav}.txt"
  echo "== real $lang $(basename "$wav")"
  "$LUNA" bench-asr "$wav" --model "$MODEL" --runs 1 --lang "$lang" --reference "$ref" \
    | grep -E '^(run 1|cer|wer|hypothesis)'
done
