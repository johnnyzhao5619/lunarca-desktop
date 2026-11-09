// @vitest-environment happy-dom
// 「插入模板」动作（PRD-06-015，V6）：替换选区插入 Rust 渲染好的正文，光标落在 `{{cursor}}`
// 那里（没写就落在末尾）；取消或选模板期间正文变了，一个字都不动。
import { describe, expect, it } from "vitest";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { runEditorFormatAction, type EditorActionDeps } from "./editor-actions";


function deps(pickTemplate: EditorActionDeps["pickTemplate"]): EditorActionDeps {
  return {
    linkPlaceholder: "",
    copyText: async () => {},
    paste: async () => {},
    pickAttachments: async () => [],
    pickTemplate,
    convertTextToTable: async () => null,
  };
}

function view(doc: string, anchor: number, head = anchor): EditorView {
  return new EditorView({ parent: document.body, state: EditorState.create({ doc, selection: { anchor, head } }) });
}

describe("插入模板", () => {
  it("替换选区，光标落在 {{cursor}} 处", async () => {
    const v = view("前文选中后文", 2, 4);
    await runEditorFormatAction(v, { kind: "template" }, deps(async () => ({ text: "## 议程\n- ", cursor: 8 })));
    expect(v.state.doc.toString()).toBe("前文## 议程\n- 后文");
    expect(v.state.selection.main.head).toBe(2 + 8);
  });

  it("没写 {{cursor}} 时光标落在插入内容末尾", async () => {
    const v = view("ab", 1);
    await runEditorFormatAction(v, { kind: "template" }, deps(async () => ({ text: "XYZ", cursor: null })));
    expect(v.state.doc.toString()).toBe("aXYZb");
    expect(v.state.selection.main.head).toBe(4);
  });

  it("取消，或选的时候正文变了：不动", async () => {
    const cancelled = view("ab", 1);
    await runEditorFormatAction(cancelled, { kind: "template" }, deps(async () => null));
    expect(cancelled.state.doc.toString()).toBe("ab");

    const edited = view("ab", 1);
    await runEditorFormatAction(
      edited,
      { kind: "template" },
      deps(async () => {
        edited.dispatch({ changes: { from: 0, insert: "改" } });
        return { text: "XYZ", cursor: null };
      }),
    );
    expect(edited.state.doc.toString()).toBe("改ab");
  });
});
