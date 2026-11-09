// @vitest-environment happy-dom
import { describe, expect, it } from "vitest";
import { Compartment, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { history, undo, undoDepth } from "@codemirror/commands";
import { applyIncrementalText } from "./markdown-format";
import { EditorHistoryCache, resetEditorHistory, saveEditorHistory } from "./editor-history";

/**
 * 撤销历史的两条边界（真机 2026-09-04：连续 ⌘Z 把整篇撤成空白）：
 * ① 装载 / 外部重载不进历史；② 切换笔记时历史整个重建。
 */
function makeView(compartment: Compartment) {
  const parent = document.createElement("div");
  document.body.appendChild(parent);
  return new EditorView({
    parent,
    state: EditorState.create({ doc: "", extensions: [compartment.of(history())] }),
  });
}

describe("撤销历史边界", () => {
  it("装载不进历史：刚打开的笔记 ⌘Z 无事发生", () => {
    const view = makeView(new Compartment());
    applyIncrementalText(view, "正文");
    expect(undoDepth(view.state)).toBe(0);
    expect(undo(view)).toBe(false);
    expect(view.state.doc.toString()).toBe("正文");
    view.destroy();
  });

  it("编辑之后外部重载：撤销回退的是编辑，不是重载", () => {
    const view = makeView(new Compartment());
    applyIncrementalText(view, "正文");
    view.dispatch({ changes: { from: 2, insert: "！" }, userEvent: "input" });
    applyIncrementalText(view, "正文！\n外部追加");
    expect(undoDepth(view.state)).toBe(1);
    undo(view);
    expect(view.state.doc.toString()).toBe("正文\n外部追加");
    view.destroy();
  });

  it("切换笔记后历史重建：上一篇的编辑撤不进新的一篇", () => {
    const compartment = new Compartment();
    const view = makeView(compartment);
    applyIncrementalText(view, "第一篇");
    view.dispatch({ changes: { from: 3, insert: "改" }, userEvent: "input" });
    expect(undoDepth(view.state)).toBe(1);
    applyIncrementalText(view, "第二篇");
    resetEditorHistory(view, compartment);
    expect(undoDepth(view.state)).toBe(0);
    expect(undo(view)).toBe(false);
    expect(view.state.doc.toString()).toBe("第二篇");
    // 重建后的历史照常记录新的编辑。
    view.dispatch({ changes: { from: 3, insert: "又改" }, userEvent: "input" });
    expect(undoDepth(view.state)).toBe(1);
    view.destroy();
  });
});

/** 撤销按标签页保留（PRD-06-014 2026-10-03 修订）：切走再切回，⌘Z 仍撤得回切走前的编辑。 */
describe("撤销历史随标签页保留", () => {
  it("切回正文没变的那篇，接上它自己的历史；切到别篇不串", () => {
    const compartment = new Compartment();
    const view = makeView(compartment);
    const cache = new EditorHistoryCache();
    applyIncrementalText(view, "甲篇");
    view.dispatch({ changes: { from: 2, insert: "！" }, userEvent: "input" });
    cache.put("甲.md", saveEditorHistory(view.state)!);

    applyIncrementalText(view, "乙篇");
    resetEditorHistory(view, compartment, cache.take("乙.md", view.state.doc) ?? undefined);
    expect(undoDepth(view.state)).toBe(0);

    applyIncrementalText(view, "甲篇！");
    resetEditorHistory(view, compartment, cache.take("甲.md", view.state.doc) ?? undefined);
    expect(undoDepth(view.state)).toBe(1);
    expect(undo(view)).toBe(true);
    expect(view.state.doc.toString()).toBe("甲篇");
    view.destroy();
  });

  it("离开期间被外部改过：不接旧历史（历史记的是相对旧正文的改动）", () => {
    const compartment = new Compartment();
    const view = makeView(compartment);
    const cache = new EditorHistoryCache();
    applyIncrementalText(view, "甲篇");
    view.dispatch({ changes: { from: 2, insert: "！" }, userEvent: "input" });
    cache.put("甲.md", saveEditorHistory(view.state)!);
    applyIncrementalText(view, "甲篇！外部追加");
    expect(cache.take("甲.md", view.state.doc)).toBeNull();
    view.destroy();
  });

  it("只留最近几篇", () => {
    const cache = new EditorHistoryCache(2);
    const state = EditorState.create({ doc: "x", extensions: [history()] });
    for (const path of ["a", "b", "c"]) cache.put(path, saveEditorHistory(state)!);
    expect(cache.take("a", state.doc)).toBeNull();
    expect(cache.take("c", state.doc)).not.toBeNull();
  });
});


describe("折叠随撤销历史一起保留（B3）", () => {
  it("离开时折起的范围记进缓存", async () => {
    const { codeFolding, foldEffect } = await import("@codemirror/language");
    const state = EditorState.create({ doc: "# 一\n正文\n# 二", extensions: [history(), codeFolding()] });
    const folded = state.update({ effects: foldEffect.of({ from: 3, to: 6 }) }).state;
    expect(saveEditorHistory(folded)?.folds).toEqual([{ from: 3, to: 6 }]);
  });
});
