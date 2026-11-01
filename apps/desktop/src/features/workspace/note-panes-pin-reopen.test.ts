// 固定标签与 ⌘⇧T 重开（PRD-07-035，笔记库对标轮 W2b）的纯函数规则。
import { describe, expect, it } from "vitest";
import {
  activePane,
  activeTab,
  closeOtherTabs,
  closePane,
  closeRightTabs,
  closeTab,
  closeTabsUnder,
  goBack,
  initialNotePanesState,
  MAX_CLOSED_TABS,
  openNote,
  remapPath,
  removeTab,
  reopenClosedTab,
  restoreNotePanes,
  serializeNotePanes,
  splitPane,
  togglePinTab,
  type NotePanesState,
} from "./note-panes";

function paths(state: NotePanesState): Array<string | null> {
  return activePane(state).tabs.map((tab) => tab.path);
}

/** 依次以常驻方式打开，得到一排常驻标签。 */
function withTabs(...names: string[]): NotePanesState {
  return names.reduce(
    (state, name) => openNote(state, name, { as: "permanent" }),
    initialNotePanesState(),
  );
}

function tabIdOf(state: NotePanesState, path: string): number {
  const tab = activePane(state).tabs.find((item) => item.path === path);
  if (!tab) throw new Error(`no tab ${path}`);
  return tab.id;
}

function pin(state: NotePanesState, path: string): NotePanesState {
  return togglePinTab(state, activePane(state).id, tabIdOf(state, path));
}

describe("固定标签", () => {
  it("「关闭其他」「关闭右侧」都留下固定标签", () => {
    let state = pin(withTabs("a.md", "b.md", "c.md", "d.md"), "d.md");
    const pane = activePane(state).id;
    expect(paths(closeOtherTabs(state, pane, tabIdOf(state, "b.md")))).toEqual(["b.md", "d.md"]);
    state = closeRightTabs(state, pane, tabIdOf(state, "a.md"));
    expect(paths(state)).toEqual(["a.md", "d.md"]);
  });

  it("打开别的笔记不替换固定标签，三种打开方式都落到右侧", () => {
    for (const as of [undefined, "preview", "permanent"] as const) {
      let state = pin(withTabs("a.md"), "a.md");
      state = openNote(state, "b.md", as ? { as } : {});
      expect(paths(state)).toEqual(["a.md", "b.md"]);
      expect(activePane(state).tabs[0].pinned).toBe(true);
    }
  });

  it("固定即常驻；再点一次取消固定；空标签不能固定", () => {
    let state = openNote(initialNotePanesState(), "a.md", { as: "preview" });
    state = pin(state, "a.md");
    expect(activeTab(state)).toMatchObject({ pinned: true });
    expect(activeTab(state)?.preview).toBeFalsy();
    state = pin(state, "a.md");
    expect(activeTab(state)?.pinned).toBeFalsy();
    const empty = initialNotePanesState();
    const pane = activePane(empty);
    expect(togglePinTab(empty, pane.id, pane.activeTabId)).toBe(empty);
  });

  it("后退不把固定标签换掉，而是在它右侧打开那一条", () => {
    let state = openNote(initialNotePanesState(), "a.md");
    state = openNote(state, "b.md");
    state = pin(state, "b.md");
    state = goBack(state, activePane(state).id);
    expect(paths(state)).toEqual(["b.md", "a.md"]);
    expect(activeTab(state)?.path).toBe("a.md");
  });

  it("固定状态随布局快照往返", () => {
    const state = pin(withTabs("a.md", "b.md"), "a.md");
    const layout = serializeNotePanes(state);
    if (!layout) throw new Error("no layout");
    expect(layout.panes[0].tabs.map((tab) => tab.pinned)).toEqual([true, false]);
    const restored = restoreNotePanes(layout);
    expect(activePane(restored).tabs.map((tab) => Boolean(tab.pinned))).toEqual([true, false]);
  });
});

describe("重开刚关闭的标签", () => {
  it("回到原位置、按关闭的逆序", () => {
    let state = withTabs("a.md", "b.md", "c.md");
    const pane = activePane(state).id;
    state = closeTab(state, pane, tabIdOf(state, "b.md"));
    state = closeTab(state, pane, tabIdOf(state, "a.md"));
    expect(paths(state)).toEqual(["c.md"]);
    state = reopenClosedTab(state);
    expect(paths(state)).toEqual(["a.md", "c.md"]);
    state = reopenClosedTab(state);
    expect(paths(state)).toEqual(["a.md", "b.md", "c.md"]);
    expect(activeTab(state)?.path).toBe("b.md");
    expect(reopenClosedTab(state)).toBe(state);
  });

  it("关掉最后一个标签后重开，用掉留下的空标签", () => {
    let state = withTabs("a.md");
    state = closeTab(state, activePane(state).id, tabIdOf(state, "a.md"));
    expect(paths(state)).toEqual([null]);
    state = reopenClosedTab(state);
    expect(paths(state)).toEqual(["a.md"]);
  });

  it("已在别处开着就聚焦那里，不再开一份", () => {
    let state = withTabs("a.md", "b.md");
    const pane = activePane(state).id;
    state = closeTab(state, pane, tabIdOf(state, "a.md"));
    state = openNote(state, "a.md", { as: "permanent" });
    const count = activePane(state).tabs.length;
    state = reopenClosedTab(state);
    expect(activePane(state).tabs.length).toBe(count);
    expect(activeTab(state)?.path).toBe("a.md");
  });

  it("关闭其他 / 右侧 / 整个分屏都进栈；移到新窗口不进栈", () => {
    let state = withTabs("a.md", "b.md", "c.md");
    const pane = activePane(state).id;
    state = closeOtherTabs(state, pane, tabIdOf(state, "a.md"));
    expect(state.closed.map((entry) => entry.path)).toEqual(["b.md", "c.md"]);
    state = removeTab(state, pane, tabIdOf(state, "a.md"));
    expect(state.closed.map((entry) => entry.path)).toEqual(["b.md", "c.md"]);

    let split = splitPane(withTabs("x.md"), activePane(withTabs("x.md")).id);
    split = closePane(split, split.activePaneId);
    expect(split.closed.map((entry) => entry.path)).toEqual(["x.md"]);
  });

  it("删除清掉栈里的记录，改名跟着改", () => {
    let state = withTabs("dir/a.md", "b.md", "c.md");
    const pane = activePane(state).id;
    state = closeTab(state, pane, tabIdOf(state, "dir/a.md"));
    state = closeTab(state, pane, tabIdOf(state, "b.md"));
    state = remapPath(state, "b.md", "renamed.md");
    state = closeTabsUnder(state, "dir");
    expect(state.closed.map((entry) => entry.path)).toEqual(["renamed.md"]);
  });

  it("栈有上限", () => {
    let state = initialNotePanesState();
    for (let index = 0; index < MAX_CLOSED_TABS + 5; index += 1) {
      state = openNote(state, `n${index}.md`, { newTab: true });
      state = closeTab(state, activePane(state).id, activePane(state).activeTabId);
    }
    expect(state.closed).toHaveLength(MAX_CLOSED_TABS);
    expect(state.closed[state.closed.length - 1].path).toBe(`n${MAX_CLOSED_TABS + 4}.md`);
  });
});
