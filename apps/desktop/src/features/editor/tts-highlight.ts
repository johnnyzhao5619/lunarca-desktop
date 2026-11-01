// 全文朗读的「当前句」高亮（CodeMirror 装饰，TDD-05 §9）。
//
// 后端逐句朗读时经 `tts://state` 广播当前句在源串中的 UTF-16 区间（与 CM 文档位置同口径）。
// VaultEditor 据此 dispatch `setTtsReadingRange` 效果，本 StateField 把该区间渲染为高亮 mark，
// 编辑改动经 `deco.map` 自动跟随位移（不因用户输入错位）。区间越界一律钳制（防陈旧回调崩溃）。
//
// 遵「前端零业务」：断句/区间计算全在 Rust，此处只把区间画成装饰。

import { StateEffect, StateField } from "@codemirror/state";
import { Decoration, type DecorationSet, EditorView } from "@codemirror/view";

/** 设置当前朗读高亮区间（`null` = 清除）。区间为文档 UTF-16 位置 `[from, to)`。 */
export const setTtsReadingRange = StateEffect.define<{ from: number; to: number } | null>();

const readingMark = Decoration.mark({ class: "luna-tts-reading" });

const ttsReadingField = StateField.define<DecorationSet>({
  create() {
    return Decoration.none;
  },
  update(deco, tr) {
    let next = deco.map(tr.changes);
    for (const effect of tr.effects) {
      if (effect.is(setTtsReadingRange)) {
        const value = effect.value;
        if (value === null) {
          next = Decoration.none;
          continue;
        }
        const docLength = tr.state.doc.length;
        const from = Math.max(0, Math.min(value.from, docLength));
        const to = Math.max(from, Math.min(value.to, docLength));
        next = to > from ? Decoration.set([readingMark.range(from, to)]) : Decoration.none;
      }
    }
    return next;
  },
  provide: (field) => EditorView.decorations.from(field),
});

/** 朗读高亮扩展（挂进 VaultEditor 的 EditorView 扩展集）。 */
export function ttsReadingHighlight() {
  return ttsReadingField;
}
