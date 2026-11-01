// 预览标签（PRD-07-035 2026-10-05 修订，笔记库对标轮 W2）的纯函数规则。
import { describe, expect, it } from "vitest";
import {
  activePane,
  activeTab,
  initialNotePanesState,
  keepTab,
  moveTab,
  openNote,
  splitPane,
  type NotePanesState,
} from "./note-panes";

function tabs(state: NotePanesState): Array<[string | null, boolean]> {
  return activePane(state).tabs.map((tab) => [tab.path, Boolean(tab.preview)]);
}

describe("预览标签", () => {
  it("浏览打开复用同一个预览标签；空标签先被用作预览", () => {
    let state = initialNotePanesState();
    state = openNote(state, "a.md", { as: "preview" });
    expect(tabs(state)).toEqual([["a.md", true]]);
    state = openNote(state, "b.md", { as: "preview" });
    expect(tabs(state)).toEqual([["b.md", true]]);
  });

  it("预览标签转常驻后，下一次浏览在右侧新开一个预览，常驻的不被替换", () => {
    let state = openNote(initialNotePanesState(), "a.md", { as: "preview" });
    const pane = activePane(state);
    state = keepTab(state, pane.id, pane.activeTabId);
    state = openNote(state, "b.md", { as: "preview" });
    expect(tabs(state)).toEqual([
      ["a.md", false],
      ["b.md", true],
    ]);
    state = openNote(state, "c.md", { as: "preview" });
    expect(tabs(state)).toEqual([
      ["a.md", false],
      ["c.md", true],
    ]);
  });

  it("常驻打开：当前是预览就原地转常驻，否则右侧新开；已开着的预览被常驻打开时转常驻", () => {
    let state = openNote(initialNotePanesState(), "a.md", { as: "preview" });
    state = openNote(state, "b.md", { as: "permanent" });
    expect(tabs(state)).toEqual([["b.md", false]]);
    state = openNote(state, "c.md", { as: "permanent" });
    expect(tabs(state)).toEqual([
      ["b.md", false],
      ["c.md", false],
    ]);
    state = openNote(state, "d.md", { as: "preview" });
    state = openNote(state, "b.md", { as: "preview" });
    expect(activeTab(state)?.path).toBe("b.md");
    state = openNote(state, "d.md", { as: "permanent" });
    expect(tabs(state)).toEqual([
      ["b.md", false],
      ["c.md", false],
      ["d.md", false],
    ]);
  });

  it("目标只在别的分屏开着：聚焦那个分屏，不在当前分屏复制一份", () => {
    let state = openNote(initialNotePanesState(), "a.md", { as: "permanent" });
    const left = state.activePaneId;
    state = splitPane(state, left);
    // 拆分时新分屏带着同一篇；先把它原地换成别的，让 a.md 只在左边。
    state = openNote(state, "b.md", { paneId: state.activePaneId });
    expect(state.activePaneId).not.toBe(left);
    state = openNote(state, "a.md", { as: "preview" });
    expect(state.activePaneId).toBe(left);
    expect(state.panes.flatMap((pane) => pane.tabs.map((tab) => tab.path)).filter((p) => p === "a.md")).toHaveLength(1);
  });

  it("拖动标签即转常驻", () => {
    let state = openNote(initialNotePanesState(), "a.md", { as: "preview" });
    const pane = activePane(state);
    state = moveTab(state, { paneId: pane.id, tabId: pane.activeTabId }, { paneId: pane.id, index: 0 });
    expect(tabs(state)).toEqual([["a.md", false]]);
  });

  it("不指定打开方式（关闭预览标签时）仍是复用当前标签", () => {
    let state = openNote(initialNotePanesState(), "a.md");
    state = openNote(state, "b.md");
    expect(tabs(state)).toEqual([["b.md", false]]);
  });
});
