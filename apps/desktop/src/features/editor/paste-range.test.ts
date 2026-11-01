// @vitest-environment happy-dom
import { afterEach, describe, expect, it } from "vitest";
import { EditorState } from "@codemirror/state";
import { history, undo } from "@codemirror/commands";
import { EditorView } from "@codemirror/view";
import { freezePaste, insertFrozenPaste, pasteRangeField } from "./paste-range";
const views: EditorView[] = [];
function mount() {
  const parent = document.createElement("div"); document.body.appendChild(parent);
  const view = new EditorView({ parent, state: EditorState.create({ doc: "abcd", selection: { anchor: 2 }, extensions: [history(), pasteRangeField] }) });
  views.push(view); return view;
}
afterEach(() => { for (const view of views.splice(0)) view.destroy(); document.body.replaceChildren(); });
describe("async paste range", () => {
  it("块结构粘贴在正文中间时留出边界，撤销不改原段落", () => {
    const view = mount(); const id = freezePaste(view);
    expect(insertFrozenPaste(view, id, "## 标题", true)).toBe(true);
    expect(view.state.doc.toString()).toBe("ab\n\n## 标题\n\ncd");
    undo(view); expect(view.state.doc.toString()).toBe("abcd");
  });
  it("迟到结果跟随源范围，移动光标不会改变落点；一次撤销恢复", () => {
    const view = mount(); const id = freezePaste(view);
    view.dispatch({ changes: { from: 0, insert: "新增" }, selection: { anchor: 6 }, userEvent: "input.type" });
    expect(insertFrozenPaste(view, id, "表格")).toBe(true);
    expect(view.state.doc.toString()).toBe("新增ab表格cd");
    expect(undo(view)).toBe(true); expect(view.state.doc.toString()).toBe("新增abcd");
  });
  it("换文档或撤销后，迟到粘贴取消", () => {
    const view = mount(); const id = freezePaste(view);
    view.dispatch({ changes: { from: 0, to: 4, insert: "另一篇" } });
    expect(insertFrozenPaste(view, id, "迟到")).toBe(false);
    const next = freezePaste(view); view.dispatch({ userEvent: "undo" });
    expect(insertFrozenPaste(view, next, "迟到")).toBe(false);
  });
});
