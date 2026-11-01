// 标签页 + 分屏状态机语义锁定（PRD-07-035/036，Obsidian/VS Code 对标）。

import { describe, expect, it } from "vitest";
import {
  activePane,
  activeTab,
  canGoBack,
  canGoForward,
  closePane,
  closeTab,
  closeTabsUnder,
  goBack,
  goForward,
  initialNotePanesState,
  MAX_PANES,
  moveTab,
  openEmptyTab,
  openNote,
  activateTab,
  remapPath,
  restoreNotePanes,
  serializeNotePanes,
  setTabMode,
  splitPane,
  type NotePanesState,
} from "./note-panes";

function paths(state: NotePanesState): (string | null)[][] {
  return state.panes.map((pane) => pane.tabs.map((tab) => tab.path));
}

describe("openNote", () => {
  it("reuses the active tab by default (Obsidian click semantics)", () => {
    let state = initialNotePanesState();
    state = openNote(state, "a.md");
    state = openNote(state, "b.md");
    expect(paths(state)).toEqual([["b.md"]]);
    expect(activeTab(state)?.path).toBe("b.md");
  });

  it("opens a new tab when requested and inserts after the active tab", () => {
    let state = initialNotePanesState();
    state = openNote(state, "a.md");
    state = openNote(state, "b.md", { newTab: true });
    state = openNote(state, "c.md", { newTab: true });
    expect(paths(state)).toEqual([["a.md", "b.md", "c.md"]]);
    expect(activeTab(state)?.path).toBe("c.md");
  });

  it("focuses the existing tab instead of duplicating the same note", () => {
    let state = initialNotePanesState();
    state = openNote(state, "a.md");
    state = openNote(state, "b.md", { newTab: true });
    state = openNote(state, "a.md");
    expect(paths(state)).toEqual([["a.md", "b.md"]]);
    expect(activeTab(state)?.path).toBe("a.md");
  });

  it("keeps the tab view mode when reusing a tab for another note", () => {
    let state = initialNotePanesState();
    state = openNote(state, "a.md");
    const pane = activePane(state);
    state = setTabMode(state, pane.id, pane.activeTabId, "reading");
    state = openNote(state, "b.md");
    expect(activeTab(state)?.mode).toBe("reading");
  });
});

describe("closeTab", () => {
  it("activates the right neighbor after closing the active tab", () => {
    let state = initialNotePanesState();
    state = openNote(state, "a.md");
    state = openNote(state, "b.md", { newTab: true });
    state = openNote(state, "c.md", { newTab: true });
    const pane = activePane(state);
    const bTab = pane.tabs.find((tab) => tab.path === "b.md");
    state = closeTab(state, pane.id, bTab?.id ?? 0);
    expect(paths(state)).toEqual([["a.md", "c.md"]]);

    const active = activePane(state);
    state = closeTab(state, active.id, active.activeTabId);
    // c.md 曾是活跃标签之外的关闭；现在关闭活跃者 → 右邻优先。
    expect(activeTab(state)?.path).toBe("a.md");
  });

  it("keeps a single empty tab when the last tab of the last pane closes", () => {
    let state = initialNotePanesState();
    state = openNote(state, "a.md");
    const pane = activePane(state);
    state = closeTab(state, pane.id, pane.activeTabId);
    expect(state.panes).toHaveLength(1);
    expect(activeTab(state)?.path).toBeNull();
  });

  it("removes the pane when its last tab closes and other panes remain", () => {
    let state = initialNotePanesState();
    state = openNote(state, "a.md");
    state = splitPane(state, activePane(state).id);
    const split = activePane(state);
    state = closeTab(state, split.id, split.activeTabId);
    expect(state.panes).toHaveLength(1);
    expect(activeTab(state)?.path).toBe("a.md");
  });
});

describe("splitPane / closePane", () => {
  it("clones the active tab (path and mode) into the new pane", () => {
    let state = initialNotePanesState();
    state = openNote(state, "a.md");
    const pane = activePane(state);
    state = setTabMode(state, pane.id, pane.activeTabId, "reading");
    state = splitPane(state, pane.id);
    expect(state.panes).toHaveLength(2);
    expect(activeTab(state)?.path).toBe("a.md");
    expect(activeTab(state)?.mode).toBe("reading");
  });

  it("caps the number of panes at MAX_PANES", () => {
    let state = initialNotePanesState();
    state = openNote(state, "a.md");
    for (let i = 0; i < MAX_PANES + 2; i += 1) {
      state = splitPane(state, activePane(state).id);
    }
    expect(state.panes).toHaveLength(MAX_PANES);
  });

  it("closePane removes the pane and never drops the last one", () => {
    let state = initialNotePanesState();
    state = openNote(state, "a.md");
    state = splitPane(state, activePane(state).id);
    const splitId = activePane(state).id;
    state = closePane(state, splitId);
    expect(state.panes).toHaveLength(1);
    state = closePane(state, state.panes[0].id);
    expect(state.panes).toHaveLength(1);
  });
});

describe("moveTab", () => {
  it("reorders within a pane", () => {
    let state = initialNotePanesState();
    state = openNote(state, "a.md");
    state = openNote(state, "b.md", { newTab: true });
    const pane = activePane(state);
    const aTab = pane.tabs.find((tab) => tab.path === "a.md");
    state = moveTab(
      state,
      { paneId: pane.id, tabId: aTab?.id ?? 0 },
      { paneId: pane.id, index: 2 },
    );
    expect(paths(state)).toEqual([["b.md", "a.md"]]);
  });

  it("moves across panes and collapses the emptied source pane", () => {
    let state = initialNotePanesState();
    state = openNote(state, "a.md");
    state = splitPane(state, activePane(state).id);
    state = openNote(state, "b.md");
    const source = activePane(state);
    const target = state.panes.find((pane) => pane.id !== source.id);
    const bTab = source.tabs.find((tab) => tab.path === "b.md");
    state = moveTab(
      state,
      { paneId: source.id, tabId: bTab?.id ?? 0 },
      { paneId: target?.id ?? 0, index: 0 },
    );
    expect(state.panes).toHaveLength(1);
    expect(paths(state)).toEqual([["b.md", "a.md"]]);
    expect(activeTab(state)?.path).toBe("b.md");
  });
});

describe("remapPath / closeTabsUnder", () => {
  it("remaps exact path and directory prefix across panes", () => {
    let state = initialNotePanesState();
    state = openNote(state, "dir/a.md");
    state = splitPane(state, activePane(state).id);
    state = openNote(state, "dir/sub/b.md");
    state = remapPath(state, "dir", "renamed");
    expect(paths(state)).toEqual([["renamed/a.md"], ["renamed/sub/b.md"]]);
  });

  it("closes all tabs under a deleted directory", () => {
    let state = initialNotePanesState();
    state = openNote(state, "dir/a.md");
    state = openNote(state, "keep.md", { newTab: true });
    state = openNote(state, "dir/sub/b.md", { newTab: true });
    state = closeTabsUnder(state, "dir");
    expect(paths(state)).toEqual([["keep.md"]]);
  });
});

describe("serialize / restore", () => {
  it("round-trips panes, tab order, active indices and view modes", () => {
    let state = initialNotePanesState();
    state = openNote(state, "a.md");
    state = openNote(state, "b.md", { newTab: true });
    const pane = activePane(state);
    state = setTabMode(state, pane.id, pane.activeTabId, "source");
    state = splitPane(state, pane.id);
    state = openNote(state, "c.md");

    const layout = serializeNotePanes(state);
    expect(layout).not.toBeNull();
    const restored = restoreNotePanes(layout!);
    expect(paths(restored)).toEqual([["a.md", "b.md"], ["c.md"]]);
    expect(activeTab(restored)?.path).toBe("c.md");
    expect(restored.panes[0].tabs[1].mode).toBe("source");
  });

  it("skips empty tabs and returns null when nothing is open", () => {
    let state = initialNotePanesState();
    expect(serializeNotePanes(state)).toBeNull();
    state = openNote(state, "a.md");
    state = openEmptyTab(state);
    const layout = serializeNotePanes(state);
    expect(layout?.panes).toHaveLength(1);
    expect(layout?.panes[0].tabs).toHaveLength(1);
  });

  it("clamps out-of-range active indices on restore", () => {
    const restored = restoreNotePanes({
      panes: [{ tabs: [{ path: "a.md", mode: "live" }], active_tab: 9 }],
      active_pane: 9,
    });
    expect(activeTab(restored)?.path).toBe("a.md");
  });
});

// 导航历史（PRD-07-037 / TDD-07 §4.1）：每个 pane 一条，浏览器/IDE 心智。
describe("导航历史", () => {
  function history(state: NotePanesState): string[] {
    return activePane(state).history.entries.map((entry) => entry.path);
  }

  it("后退/前进沿浏览器心智走，且历史是**每 pane** 一条", () => {
    let state = initialNotePanesState();
    state = openNote(state, "a.md");
    state = openNote(state, "b.md");
    state = openNote(state, "c.md");
    expect(history(state)).toEqual(["a.md", "b.md", "c.md"]);
    expect(canGoForward(activePane(state))).toBe(false);

    const paneId = activePane(state).id;
    state = goBack(state, paneId);
    expect(activeTab(state)?.path).toBe("b.md");
    expect(canGoBack(activePane(state))).toBe(true);
    expect(canGoForward(activePane(state))).toBe(true);

    state = goBack(state, paneId);
    expect(activeTab(state)?.path).toBe("a.md");
    expect(canGoBack(activePane(state))).toBe(false);

    state = goForward(state, paneId);
    expect(activeTab(state)?.path).toBe("b.md");

    // 分屏出来的 pane 有它自己的一条历史——共用一条的话，在右边后退会把左边也带走。
    state = splitPane(state, paneId);
    const rightId = activePane(state).id;
    expect(rightId).not.toBe(paneId);
    expect(canGoBack(activePane(state))).toBe(false);
    state = openNote(state, "d.md", { paneId: rightId });
    expect(history(state)).toEqual(["b.md", "d.md"]);
  });

  it("后退之后打开新目标：截断前进栈并追加（浏览器点了新链接就是这样）", () => {
    let state = initialNotePanesState();
    state = openNote(state, "a.md");
    state = openNote(state, "b.md");
    state = openNote(state, "c.md");
    const paneId = activePane(state).id;
    state = goBack(state, paneId); // 停在 b.md，前进可回 c.md
    state = openNote(state, "d.md");
    expect(history(state)).toEqual(["a.md", "b.md", "d.md"]);
    expect(canGoForward(activePane(state))).toBe(false);
  });

  it("后退/前进本身不入历史——否则按一次后退就把前进栈毁了", () => {
    let state = initialNotePanesState();
    state = openNote(state, "a.md");
    state = openNote(state, "b.md");
    const paneId = activePane(state).id;
    const before = history(state);
    state = goBack(state, paneId);
    state = goForward(state, paneId);
    expect(history(state)).toEqual(before);
    expect(activePane(state).history.cursor).toBe(1);
  });

  it("切标签也是这一格的一次导航（IDE 心智），连续落到同一篇不重复记", () => {
    let state = initialNotePanesState();
    state = openNote(state, "a.md");
    state = openNote(state, "b.md", { newTab: true });
    const pane = activePane(state);
    const firstTab = pane.tabs[0].id;
    state = activateTab(state, pane.id, firstTab);
    expect(history(state)).toEqual(["a.md", "b.md", "a.md"]);
    // 已经在 a.md 上，再切一次同一个标签不该多记一条。
    state = activateTab(state, pane.id, firstTab);
    expect(history(state)).toEqual(["a.md", "b.md", "a.md"]);
  });

  it("改视图模式不新增历史条目，但会更新当前那一条——否则后退会把刚设的模式撤掉", () => {
    let state = initialNotePanesState();
    state = openNote(state, "a.md");
    state = openNote(state, "b.md");
    const pane = activePane(state);
    state = setTabMode(state, pane.id, pane.activeTabId, "reading");
    expect(history(state)).toEqual(["a.md", "b.md"]);
    expect(activePane(state).history.entries[1].mode).toBe("reading");

    state = goBack(state, pane.id);
    state = goForward(state, pane.id);
    expect(activeTab(state)?.mode).toBe("reading");
  });

  it("重命名改写历史；删除把对应条目剪掉且游标跟着身份走", () => {
    let state = initialNotePanesState();
    state = openNote(state, "a.md");
    state = openNote(state, "b.md");
    state = openNote(state, "c.md");
    state = remapPath(state, "b.md", "renamed.md");
    expect(history(state)).toEqual(["a.md", "renamed.md", "c.md"]);

    // 正停在 c.md 上；删掉中间那一篇，游标仍该指向 c.md。
    state = closeTabsUnder(state, "renamed.md");
    expect(history(state)).toEqual(["a.md", "c.md"]);
    expect(activePane(state).history.cursor).toBe(1);
  });

  it("历史随布局快照往返（重启后后退仍指向上次那几篇）", () => {
    let state = initialNotePanesState();
    state = openNote(state, "a.md");
    state = openNote(state, "b.md");
    const paneId = activePane(state).id;
    state = goBack(state, paneId);

    const layout = serializeNotePanes(state);
    expect(layout?.panes[0].history?.map((entry) => entry.path)).toEqual([
      "a.md",
      "b.md",
    ]);
    expect(layout?.panes[0].history_cursor).toBe(0);

    const restored = restoreNotePanes(layout!);
    expect(canGoForward(activePane(restored))).toBe(true);
    expect(activeTab(goForward(restored, activePane(restored).id))?.path).toBe(
      "b.md",
    );
  });

  it("没有历史的旧快照照常恢复（历史在 wire 上是可选的）", () => {
    const restored = restoreNotePanes({
      panes: [{ tabs: [{ path: "a.md", mode: "live" }], active_tab: 0 }],
      active_pane: 0,
    });
    expect(canGoBack(activePane(restored))).toBe(false);
    expect(canGoForward(activePane(restored))).toBe(false);
  });
});
