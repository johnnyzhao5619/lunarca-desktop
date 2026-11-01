// @vitest-environment happy-dom
import { describe, expect, it, vi } from "vitest";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { runEditorFormatAction, type EditorActionDeps } from "./editor-actions";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

function deps(convert: EditorActionDeps["convertTextToTable"]): EditorActionDeps {
  return { linkPlaceholder: "", copyText: async () => {}, paste: async () => {}, pickAttachments: async () => [], pickTemplate: async () => null, convertTextToTable: convert };
}

describe("将选中文字转为表格（对标方案 T5）", () => {
  it("按整行取选区交给 Rust，一笔替换；看不出列就不动正文", async () => {
    const doc = "前文\n名称\t数量\n苹果\t3\n后文";
    const view = new EditorView({ parent: document.body, state: EditorState.create({ doc, selection: { anchor: 5, head: 9 } }) });
    const convert = vi.fn(async (text: string) => (text === "名称\t数量\n苹果\t3" ? "| 名称 | 数量 |\n| --- | --- |\n| 苹果 | 3 |" : null));
    await runEditorFormatAction(view, { kind: "textToTable" }, deps(convert));
    expect(convert).toHaveBeenCalledWith("名称\t数量\n苹果\t3");
    expect(view.state.doc.toString()).toBe("前文\n| 名称 | 数量 |\n| --- | --- |\n| 苹果 | 3 |\n后文");
    // happy-dom 在聚焦后的 DOM 更新中同步派发 selectionchange（浏览器不会）。
    view.contentDOM.blur();
    view.dispatch({ selection: { anchor: 0, head: 2 } });
    await runEditorFormatAction(view, { kind: "textToTable" }, deps(async () => null));
    expect(view.state.doc.toString()).toBe("前文\n| 名称 | 数量 |\n| --- | --- |\n| 苹果 | 3 |\n后文");
    view.destroy();
  });
});
