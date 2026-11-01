import { describe, expect, it } from "vitest";
import { trimSelectionRects, type SelectionRect } from "./selection-layer";

/** CM `RectangleMarker.forRange` 对"三行选区、尾在第四行行首"给出的典型三块：首行 / 中段 / 残块。 */
const threeLines: SelectionRect[] = [
  { left: 40, top: 0, width: 560, height: 27 }, // 首行：从选区起点到右缘
  { left: -26, top: 27, width: 626, height: 54 }, // 中段：CM 的左界被首个列表行的负 text-indent 拉到正文左侧
  { left: -26, top: 81, width: 30, height: 27 }, // 残块：下一行行首那一小块
];

describe("自绘选区层的矩形后处理（PRD-06-011，2026-09-04）", () => {
  it("整行高亮的左界夹到正文列，宽度同步收窄", () => {
    const out = trimSelectionRects(threeLines, { textLeft: 24, endLineTop: null });
    expect(out[1]).toEqual({ left: 24, top: 27, width: 576, height: 54 });
    // 起点本就在正文列右侧的矩形原样保留。
    expect(out[0]).toEqual(threeLines[0]);
  });

  it("选区尾恰在行首：那一行的残块不画，上一行仍画到右缘", () => {
    const out = trimSelectionRects(threeLines, { textLeft: 24, endLineTop: 81 });
    expect(out).toHaveLength(2);
    expect(out.map((r) => r.top)).toEqual([0, 27]);
  });

  it("残块判定按行顶比较、带亚像素容差（不会误裁上一行）", () => {
    const out = trimSelectionRects(threeLines, { textLeft: 24, endLineTop: 81.3 });
    expect(out).toHaveLength(2);
  });

  it("选区尾不在行首（endLineTop 为 null）时一块都不裁", () => {
    expect(trimSelectionRects(threeLines, { textLeft: -100, endLineTop: null })).toEqual(threeLines);
  });

  it("夹左界不会把宽度夹成负数", () => {
    const out = trimSelectionRects([{ left: 0, top: 0, width: 10, height: 20 }], {
      textLeft: 30,
      endLineTop: null,
    });
    expect(out).toEqual([{ left: 30, top: 0, width: 0, height: 20 }]);
  });
});
