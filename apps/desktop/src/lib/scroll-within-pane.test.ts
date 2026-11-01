import { describe, expect, it } from "vitest";
import { paneScrollTop } from "./scroll-within-pane";

describe("paneScrollTop", () => {
  it("放得下的目标居中", () => {
    expect(paneScrollTop(1000, 600, 100)).toBe(750);
  });

  it("比窗格还高的目标顶端对齐，不把开头推到视口上方", () => {
    expect(paneScrollTop(1000, 600, 900)).toBe(1000);
  });

  it("靠近开头的目标不滚到负值", () => {
    expect(paneScrollTop(50, 600, 100)).toBe(0);
  });
});
