// @vitest-environment happy-dom
import { afterEach, describe, expect, it } from "vitest";
import { EditorSelection, EditorState, type Extension } from "@codemirror/state";
import { EditorView, keymap, runScopeHandlers } from "@codemirror/view";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { defaultKeymap, indentMore } from "@codemirror/commands";
import { searchKeymap } from "@codemirror/search";
import { blockMoveKeymap } from "./block-move";
import { setHeadingChanges, toggleLineBlockChanges } from "./format-commands";
import { listEditingKeymap } from "./list-editing";
import { multiCursorExtension, singleCursorOnly } from "./multi-cursor";

const views: EditorView[] = [];
function mount(doc: string, ranges: [number, number][], extensions: Extension[] = []) {
  const parent = document.createElement("div");
  document.body.appendChild(parent);
  const view = new EditorView({
    parent,
    state: EditorState.create({
      doc,
      selection: EditorSelection.create(ranges.map(([anchor, head]) => EditorSelection.range(anchor, head))),
      extensions: [multiCursorExtension(), markdown({ base: markdownLanguage }), ...extensions],
    }),
  });
  views.push(view);
  return view;
}
const press = (view: EditorView, init: KeyboardEventInit) => runScopeHandlers(view, new KeyboardEvent("keydown", init), "editor");
afterEach(() => { views.splice(0).forEach((view) => view.destroy()); document.body.replaceChildren(); });

describe("多光标（B5）", () => {
  it("多个选区不再被收成一个，输入落到每个光标", () => {
    const view = mount("甲\n乙\n丙", [[1, 1], [3, 3], [5, 5]]);
    expect(view.state.selection.ranges).toHaveLength(3);
    view.dispatch(view.state.replaceSelection("！"));
    expect(view.state.doc.toString()).toBe("甲！\n乙！\n丙！");
  });

  it("⌘D 选中下一处相同文字，Esc 收回到一个光标", () => {
    const view = mount("会议 纪要 会议", [[0, 2]], [keymap.of([...searchKeymap, ...defaultKeymap])]);
    expect(press(view, { key: "d", ctrlKey: true })  /* happy-dom 不是 mac 平台：Mod = Ctrl */).toBe(true);
    expect(view.state.selection.ranges.map((r) => [r.from, r.to])).toEqual([[0, 2], [6, 8]]);
    expect(press(view, { key: "Escape" })).toBe(true);
    expect(view.state.selection.ranges).toHaveLength(1);
  });

  it("只看主光标的命令在多光标下放行，单光标照常", () => {
    let ran = 0;
    const command = singleCursorOnly(() => { ran += 1; return true; });
    expect(command(mount("ab", [[0, 0], [1, 1]]))).toBe(false);
    expect(command(mount("ab", [[0, 0]]))).toBe(true);
    expect(ran).toBe(1);
  });

  it("列表 Tab 在多光标下交给通用缩进：每个光标所在行都缩进", () => {
    const doc = "- 一\n- 二\n- 三";
    const view = mount(doc, [[7, 7], [11, 11]], [listEditingKeymap(), keymap.of([{ key: "Tab", run: indentMore }])]);
    expect(press(view, { key: "Tab" })).toBe(true);
    expect(view.state.doc.toString()).toBe("- 一\n  - 二\n  - 三");
    expect(view.state.selection.ranges).toHaveLength(2);
  });

  it("⌥↓ 多光标时退回逐行移动，不把其余光标收掉", () => {
    const view = mount("甲\n乙\n丙\n丁", [[0, 0], [4, 4]], [blockMoveKeymap(), keymap.of(defaultKeymap)]);
    expect(press(view, { key: "ArrowDown", altKey: true })).toBe(true);
    expect(view.state.doc.toString()).toBe("乙\n甲\n丁\n丙");
    expect(view.state.selection.ranges).toHaveLength(2);
  });

  it("标题与列表前缀作用到每个光标所在行；有序编号按连续段各自从 1 起", () => {
    const state = EditorState.create({
      doc: "甲\n乙\n\n丙",
      selection: EditorSelection.create([EditorSelection.range(0, 3), EditorSelection.cursor(6)]),
      extensions: [EditorState.allowMultipleSelections.of(true)],
    });
    expect(state.update({ changes: setHeadingChanges(state, 2) }).state.doc.toString()).toBe("## 甲\n## 乙\n\n## 丙");
    expect(state.update({ changes: toggleLineBlockChanges(state, "ordered") }).state.doc.toString()).toBe("1. 甲\n2. 乙\n\n1. 丙");
  });

  it("单个选区跨空行的有序列表仍连续编号", () => {
    const state = EditorState.create({ doc: "甲\n\n乙", selection: EditorSelection.range(0, 4) });
    expect(state.update({ changes: toggleLineBlockChanges(state, "ordered") }).state.doc.toString()).toBe("1. 甲\n\n2. 乙");
  });
});

describe("多光标粘贴（B5）", () => {
  it("行数等于光标数时各分一行，否则每处插整段", async () => {
    const { insertAtCursor } = await import("./paste-assets");
    const view = mount("a\nb\nc", [[1, 1], [3, 3], [5, 5]]);
    insertAtCursor(view, "1\n2\n3");
    expect(view.state.doc.toString()).toBe("a1\nb2\nc3");
    insertAtCursor(view, "x");
    expect(view.state.doc.toString()).toBe("a1x\nb2x\nc3x");
    expect(view.state.selection.ranges.map((r) => r.head)).toEqual([3, 7, 11]);
  });
});

describe("选中字数（B6）", () => {
  it("多段选区相加，没选中为 null", async () => {
    const { selectionWordCount } = await import("./editor-extensions");
    const state = EditorState.create({
      doc: "你好 world\n第二行 two words",
      selection: EditorSelection.create([EditorSelection.range(0, 2), EditorSelection.range(9, 16)]),
      extensions: [EditorState.allowMultipleSelections.of(true)],
    });
    expect(selectionWordCount(state)).toBe(2 + 4);
    expect(selectionWordCount(EditorState.create({ doc: "x" }))).toBeNull();
  });
});
