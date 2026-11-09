// 活动栏布局的前端判定（PRD-07-003「临时显示」/ ADR-087 ⑥）：只测前端才知道的那两件事。

import { describe, expect, it } from "vitest";
import type { RailLayoutItemDto } from "../../ipc/bindings";
import {
  fallbackRailLayout,
  hiddenRailItems,
  isTemporarilyShown,
  visibleRail,
  railScrollTarget,
} from "./rail-layout";
import { isAnchor, PRIMARY_SPACES, railDividerIndices } from "./workspace-ia";

const NONE: ReadonlySet<never> = new Set();

function layout(overrides: Partial<Record<string, boolean>> = {}): RailLayoutItemDto[] {
  return fallbackRailLayout().map((item) => ({
    ...item,
    // 没点名的格沿用兜底布局的默认显隐（创作空间默认隐藏，ADR-162 ①）。
    visible: overrides[item.space] ?? item.visible,
  }));
}

describe("兜底布局", () => {
  it("与 IA 表同序、除两个一方应用空间外全部显示、锚点即锁定", () => {
    const items = fallbackRailLayout();
    expect(items.map((item) => item.space)).toEqual(PRIMARY_SPACES);
    // 创作空间默认隐藏（ADR-162 ①），与 Rust 词表 `RAIL_LAYOUT_DEFAULT` 同形。
    expect(items.filter((item) => !item.visible).map((item) => item.space)).toEqual(["creator", "teleprompter"]);
    expect(items.filter((item) => item.locked).map((item) => item.space)).toEqual([
      "notes",
      "knowledge",
    ]);
    for (const item of items) {
      expect(item.locked).toBe(isAnchor(item.space));
    }
  });
});

describe("临时显示（PRD-07-003）", () => {
  it("收起的空间不画；站进去时在**原位**出现，离开即消失", () => {
    const items = layout({ quick_notes: false, transcripts: false });
    expect(visibleRail(items, "notes", NONE).map((item) => item.space)).toEqual([
      "search",
      "notes",
      "knowledge",
      "recordings",
      "chat",
    ]);
    // 站进「速记」：它回到搜索 / 笔记 / 知识库之后、录音之前的原位，而不是追加到末尾。
    expect(visibleRail(items, "quick_notes", NONE).map((item) => item.space)).toEqual([
      "search",
      "notes",
      "knowledge",
      "quick_notes",
      "recordings",
      "chat",
    ]);
  });

  it("录音进行中「录音」始终显示，且被标为临时", () => {
    const items = layout({ recordings: false });
    const force = new Set(["recordings"] as const);
    const shown = visibleRail(items, "notes", force);
    expect(shown.map((item) => item.space)).toContain("recordings");
    const recordings = shown.find((item) => item.space === "recordings")!;
    expect(isTemporarilyShown(recordings, "notes", force)).toBe(true);
    // 用户本来就显示的那格不算临时。
    expect(isTemporarilyShown(shown[0], "notes", force)).toBe(false);
  });

  it("隐藏候选 = 用户收起的那几格；锚点永远不会在里面", () => {
    const items = layout({ search: false, chat: false });
    expect(hiddenRailItems(items).map((item) => item.space)).toEqual(["search", "chat", "creator", "teleprompter"]);
    // 兜底布局里的隐藏候选就是默认隐藏的两个一方应用空间。
    expect(hiddenRailItems(fallbackRailLayout()).map((item) => item.space)).toEqual(["creator", "teleprompter"]);
  });

  it("分隔线按**画出来的**那几格的锁定态重算", () => {
    // 收起搜索后第一格就是锚点，前面那根线随之消失。
    const shown = visibleRail(layout({ search: false }), "notes", NONE);
    expect([...railDividerIndices(shown, (item) => item.locked)]).toEqual([2]);
  });
});

describe("活动栏滚动箭头的落点", () => {
  it("翻大半屏", () => {
    expect(railScrollTarget(0, 300, 1000, 1)).toBe(240);
    expect(railScrollTarget(600, 300, 1000, -1)).toBe(360);
  });

  it("翻完只剩不到四分之一屏就直接到头，不留一段空白再要一下", () => {
    expect(railScrollTarget(0, 300, 600, 1)).toBe(300);
    expect(railScrollTarget(100, 300, 600, -1)).toBe(0);
  });
});
