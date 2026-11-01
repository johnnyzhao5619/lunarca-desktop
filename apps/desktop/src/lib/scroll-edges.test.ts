import { describe, expect, it } from "vitest";
import { scrollEdges } from "./scroll-edges";

describe("滚动区两端提示", () => {
  it("放得下：两端都没有", () => {
    expect(scrollEdges(0, 400, 400)).toEqual({ start: false, end: false });
  });

  it("在顶上且放不下：只提示下面还有", () => {
    expect(scrollEdges(0, 300, 420)).toEqual({ start: false, end: true });
  });

  it("滚到中间：两端都提示", () => {
    expect(scrollEdges(50, 300, 420)).toEqual({ start: true, end: true });
  });

  it("滚到底：只提示上面还有；亚像素误差不算「还有」", () => {
    expect(scrollEdges(120, 300, 420)).toEqual({ start: true, end: false });
    expect(scrollEdges(119.5, 300, 420)).toEqual({ start: true, end: false });
    expect(scrollEdges(0.5, 300, 300.6)).toEqual({ start: false, end: false });
  });
});
