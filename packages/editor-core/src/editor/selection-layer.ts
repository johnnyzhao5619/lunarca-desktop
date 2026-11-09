// 自绘选区层（PRD-06-011，2026-09-04 裁决；整行高亮取 VS Code / Obsidian 口径，换行残块不画）。
//
// **为什么不用 CM 自带 `drawSelection` 的选区层**：它算整行高亮的左界时取**文档里第一个
// `.cm-line`** 的 `padding-left + min(0, text-indent)`，默认所有行同一内距。本仓库列表行走
// 悬挂缩进（`list-indent.ts`：内距加 hang、等量负 text-indent），首行若是列表行，左界就被
// 负缩进拉进正文左侧的留白里——整行选中时高亮比正文左缘多出一截；它还给"只含换行符"的
// 选区尾在下一行画一块残块（真机 2026-09-04）。该层不可单独关闭、左界不可配置，故：
// `drawSelection()` 继续负责光标与隐藏原生选区，它的选区层由 `@lunarca/styles/codemirror.css`
// 藏起来，选区改由本层画。矩形仍取自 CM 公开的 `RectangleMarker.forRange`（bidi / 折行 /
// 块级部件的处理全部沿用），本层只做两条后处理（`trimSelectionRects`，纯函数可无头测试）：
// ① 左界夹到正文列（`.cm-content` 的内距边）；② 选区尾恰在某行行首时，不在那一行画残块——
// 上一行仍画到右缘（换行符确实在选区里，复制会带上它），只是下一行不再多出一小块。

import type { Extension } from "@codemirror/state";
import { Direction, EditorView, layer, type LayerMarker, RectangleMarker } from "@codemirror/view";

/** 选区矩形的 class（样式在 editor-core.css，颜色与界面 `::selection` 同一 Token）。 */
export const SELECTION_MARKER_CLASS = "luna-selection";

/** 层坐标里的一块选区矩形（与 `RectangleMarker` 同字段，便于纯函数处理）。 */
export interface SelectionRect {
  left: number;
  top: number;
  width: number | null;
  height: number;
}

export interface SelectionRectFrame {
  /** 正文列左界（层坐标）：整行高亮不得越过它。 */
  textLeft: number;
  /** 选区尾所在行的顶（层坐标）；`null` = 选区尾不在行首，不裁残块。 */
  endLineTop: number | null;
}

/** 与 CM 自身的矩形合并容差同量级：行顶比较只需分辨"是不是同一行"。 */
const ROW_EPSILON = 0.5;

/** 夹左界 + 去残块（纯函数）。 */
export function trimSelectionRects(
  rects: readonly SelectionRect[],
  frame: SelectionRectFrame,
): SelectionRect[] {
  const out: SelectionRect[] = [];
  for (const rect of rects) {
    if (frame.endLineTop !== null && rect.top >= frame.endLineTop - ROW_EPSILON) {
      continue;
    }
    if (rect.left < frame.textLeft) {
      const shift = frame.textLeft - rect.left;
      out.push({
        left: frame.textLeft,
        top: rect.top,
        width: rect.width === null ? null : Math.max(0, rect.width - shift),
        height: rect.height,
      });
    } else {
      out.push(rect);
    }
  }
  return out;
}

/** 层坐标原点（与 CM 的 `getBase` 同一算法：滚动容器的文档原点，含已滚出的部分）。 */
function layerBase(view: EditorView): { left: number; top: number } {
  const rect = view.scrollDOM.getBoundingClientRect();
  const left =
    view.textDirection === Direction.LTR
      ? rect.left
      : rect.right - view.scrollDOM.clientWidth * view.scaleX;
  return {
    left: left - view.scrollDOM.scrollLeft * view.scaleX,
    top: rect.top - view.scrollDOM.scrollTop * view.scaleY,
  };
}

export function selectionLayerExtension(): Extension {
  return layer({
    above: false,
    class: "luna-selection-layer",
    update: (update) => update.docChanged || update.selectionSet || update.viewportChanged,
    markers(view) {
      const base = layerBase(view);
      const contentRect = view.contentDOM.getBoundingClientRect();
      const padding = Number.parseFloat(getComputedStyle(view.contentDOM).paddingLeft) || 0;
      // 从右往左的文档正文列在右侧，左界不夹（CM 自己的左界算法在 RTL 下本就不同）。
      const textLeft =
        view.textDirection === Direction.LTR
          ? contentRect.left + padding - base.left
          : Number.NEGATIVE_INFINITY;
      const markers: LayerMarker[] = [];
      for (const range of view.state.selection.ranges) {
        if (range.empty) {
          continue;
        }
        const rects = RectangleMarker.forRange(view, SELECTION_MARKER_CLASS, range);
        const endLine = view.state.doc.lineAt(range.to);
        let endLineTop: number | null = null;
        if (endLine.from === range.to) {
          const coords = view.coordsAtPos(range.to, 1);
          endLineTop = coords ? coords.top - base.top : null;
        }
        for (const rect of trimSelectionRects(rects, { textLeft, endLineTop })) {
          markers.push(
            new RectangleMarker(SELECTION_MARKER_CLASS, rect.left, rect.top, rect.width, rect.height),
          );
        }
      }
      return markers;
    },
  });
}
