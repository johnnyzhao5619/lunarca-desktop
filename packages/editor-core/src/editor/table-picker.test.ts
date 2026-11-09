// @vitest-environment happy-dom
import { afterEach, describe, expect, it } from "vitest";
import { EditorState } from "@codemirror/state";
import { EditorView, runScopeHandlers } from "@codemirror/view";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { insertTableCommand } from "./table-edit";
import { tablePickerExtension } from "./table-picker";

const views: EditorView[] = [];
afterEach(() => {
  for (const view of views.splice(0)) view.destroy();
  document.body.replaceChildren();
});

function mount(doc = "段落\n\n") {
  const parent = document.createElement("div");
  document.body.appendChild(parent);
  const view = new EditorView({
    parent,
    state: EditorState.create({
      doc,
      selection: { anchor: doc.length },
      extensions: [markdown({ base: markdownLanguage }), tablePickerExtension({ title: "pick", size: (c, r) => `${c}x${r}` })],
    }),
  });
  views.push(view);
  return view;
}

const press = (view: EditorView, key: string) => runScopeHandlers(view, new KeyboardEvent("keydown", { key }), "editor");
const tableLines = (view: EditorView) => view.state.doc.toString().split("\n").filter((line) => line.startsWith("|"));

describe("插入表格的网格尺寸选择器（对标方案 T5）", () => {
  it("插入表格先问尺寸：默认 3 列 × 3 行，方向键调整，Enter 插入", () => {
    const view = mount();
    insertTableCommand(view);
    expect(view.dom.querySelector(".luna-table-picker__caption")?.textContent).toBe("3x3");
    expect(view.state.doc.toString()).toBe("段落\n\n");
    press(view, "ArrowRight");
    press(view, "ArrowDown");
    expect(view.dom.querySelector(".luna-table-picker__caption")?.textContent).toBe("4x4");
    expect(view.dom.querySelectorAll(".luna-table-picker__cell[data-on]")).toHaveLength(16);
    press(view, "Enter");
    expect(view.dom.querySelector(".luna-table-picker")).toBeNull();
    const lines = tableLines(view);
    expect(lines).toHaveLength(5); // 表头 + 分隔 + 3 行正文
    expect(lines[0].split("|").length - 2).toBe(4);
  });

  it("点某一格就按它的尺寸插入；Esc 取消不改正文", () => {
    const view = mount();
    insertTableCommand(view);
    expect(press(view, "Escape")).toBe(true);
    expect(view.dom.querySelector(".luna-table-picker")).toBeNull();
    expect(view.state.doc.toString()).toBe("段落\n\n");
    insertTableCommand(view);
    view.dom.querySelector<HTMLButtonElement>('.luna-table-picker__cell[data-row="2"][data-column="5"]')!.click();
    const lines = tableLines(view);
    expect(lines).toHaveLength(3);
    expect(lines[0].split("|").length - 2).toBe(5);
  });

  it("尺寸夹在 1–8 列、2–8 行；正文一改浮层即关", () => {
    const view = mount();
    insertTableCommand(view);
    for (let i = 0; i < 10; i++) press(view, "ArrowUp");
    expect(view.dom.querySelector(".luna-table-picker__caption")?.textContent).toBe("3x2");
    view.dispatch({ changes: { from: 0, insert: "x" } });
    expect(view.dom.querySelector(".luna-table-picker")).toBeNull();
  });
});
