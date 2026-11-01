import { describe, expect, it } from "vitest";
import { moveRailItemToIndex } from "./rail-layout";
import type { WorkspaceSpace } from "./workspace-ia";

const order: WorkspaceSpace[] = ["search", "notes", "knowledge", "recordings", "transcripts", "chat"];

describe("活动栏拖拽落点（PRD-07-003 就地入口）", () => {
  it("插到第 n 个之前；n = 长度即末尾", () => {
    expect(moveRailItemToIndex(order, "chat", 1)).toEqual([
      "search",
      "chat",
      "notes",
      "knowledge",
      "recordings",
      "transcripts",
    ]);
    expect(moveRailItemToIndex(order, "search", order.length)).toEqual([
      "notes",
      "knowledge",
      "recordings",
      "transcripts",
      "chat",
      "search",
    ]);
    // 往后挪：落点下标要扣掉自己让出的那一格。
    expect(moveRailItemToIndex(order, "notes", 4)).toEqual([
      "search",
      "knowledge",
      "recordings",
      "notes",
      "transcripts",
      "chat",
    ]);
  });

  it("位置没变（落在自己或自己的下一格之前）时返回同一引用，调用方据此不发请求", () => {
    expect(moveRailItemToIndex(order, "notes", 1)).toBe(order);
    expect(moveRailItemToIndex(order, "notes", 2)).toBe(order);
    expect(moveRailItemToIndex(order, "notes", -1)).toBe(order);
    expect(moveRailItemToIndex(order, "notes", 99)).toBe(order);
  });
});
