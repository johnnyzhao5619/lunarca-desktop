import { describe, expect, it } from "vitest";
import {
  bubbleAnchorBox,
  clampPanelPosition,
  placeFloatingMenu,
  placeSelectionBubble,
} from "./floating-menu-position";

const EDITOR = { width: 800, height: 600 };
const PANEL = { width: 420, height: 320 };

describe("placeFloatingMenu", () => {
  it("places below the selection end when there is room", () => {
    const placed = placeFloatingMenu(
      { left: 100, top: 80, bottom: 100 },
      EDITOR,
      PANEL,
    );
    expect(placed).toEqual({ x: 100, y: 108, placement: "below" });
  });

  it("flips above when below would overflow the editor", () => {
    const placed = placeFloatingMenu(
      { left: 100, top: 500, bottom: 520 },
      EDITOR,
      PANEL,
    );
    expect(placed.placement).toBe("above");
    expect(placed.y).toBe(500 - 8 - PANEL.height);
  });

  it("clamps horizontally so the panel never overflows the right edge", () => {
    const placed = placeFloatingMenu(
      { left: 780, top: 80, bottom: 100 },
      EDITOR,
      PANEL,
    );
    expect(placed.x).toBe(EDITOR.width - PANEL.width - 8);
  });

  it("clamps to the left gap for selections at the left edge", () => {
    const placed = placeFloatingMenu(
      { left: 0, top: 80, bottom: 100 },
      EDITOR,
      PANEL,
    );
    expect(placed.x).toBe(8);
  });

  it("pins to the bottom when neither side fits", () => {
    const placed = placeFloatingMenu(
      { left: 10, top: 150, bottom: 170 },
      { width: 500, height: 300 },
      PANEL,
    );
    expect(placed.y).toBe(Math.max(8, 300 - PANEL.height - 8));
  });
});

describe("bubbleAnchorBox", () => {
  const start = { left: 200, right: 210, top: 100, bottom: 124 };

  it("单行选区：首尾并集", () => {
    const end = { left: 400, right: 410, top: 100, bottom: 124 };
    expect(bubbleAnchorBox(start, end, 760)).toEqual({ left: 200, right: 410, top: 100, bottom: 124 });
  });

  it("跨行选区：只取首行，从起点到正文右缘", () => {
    const end = { left: 80, right: 90, top: 220, bottom: 244 };
    expect(bubbleAnchorBox(start, end, 760)).toEqual({ left: 200, right: 760, top: 100, bottom: 124 });
  });

  it("反向选区（尾在前）同样按坐标而不是按文档顺序", () => {
    const end = { left: 400, right: 410, top: 100, bottom: 124 };
    expect(bubbleAnchorBox(end, start, 760)).toEqual({ left: 200, right: 410, top: 100, bottom: 124 });
  });
});

describe("placeSelectionBubble", () => {
  const BUBBLE = { width: 300, height: 36 };

  it("centres above the selection when there is room", () => {
    const placed = placeSelectionBubble(
      { left: 200, right: 400, top: 100, bottom: 120 },
      EDITOR,
      BUBBLE,
    );
    expect(placed).toEqual({ x: 150, y: 100 - 8 - 36, placement: "above" });
  });

  it("falls below the selection when it starts at the top", () => {
    const placed = placeSelectionBubble(
      { left: 200, right: 400, top: 10, bottom: 30 },
      EDITOR,
      BUBBLE,
    );
    expect(placed.placement).toBe("below");
    expect(placed.y).toBe(38);
  });

  it("clamps horizontally inside the editor", () => {
    const left = placeSelectionBubble({ left: 0, right: 20, top: 100, bottom: 120 }, EDITOR, BUBBLE);
    expect(left.x).toBe(8);
    const right = placeSelectionBubble(
      { left: 780, right: 800, top: 100, bottom: 120 },
      EDITOR,
      BUBBLE,
    );
    expect(right.x).toBe(EDITOR.width - BUBBLE.width - 8);
  });
});

describe("clampPanelPosition", () => {
  const PANEL = { width: 420, height: 320 };

  it("keeps a position that is already inside", () => {
    expect(clampPanelPosition(100, 120, EDITOR, PANEL)).toEqual({ x: 100, y: 120 });
  });

  it("clamps to the gap on the top and left edges", () => {
    expect(clampPanelPosition(-500, -500, EDITOR, PANEL)).toEqual({ x: 8, y: 8 });
  });

  it("keeps the whole panel inside horizontally", () => {
    expect(clampPanelPosition(9999, 100, EDITOR, PANEL).x).toBe(EDITOR.width - PANEL.width - 8);
  });

  it("keeps the whole panel inside vertically", () => {
    // 容器 overflow:hidden——越界的部分是被裁掉，而不是露出一角。
    expect(clampPanelPosition(100, 9999, EDITOR, PANEL).y).toBe(
      EDITOR.height - PANEL.height - 8,
    );
  });

  it("pins a panel taller than the editor to the top gap", () => {
    const tall = { width: 420, height: EDITOR.height + 200 };
    expect(clampPanelPosition(100, 9999, EDITOR, tall).y).toBe(8);
  });
});
