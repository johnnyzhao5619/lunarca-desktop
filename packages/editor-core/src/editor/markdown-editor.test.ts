// @vitest-environment happy-dom
// 编辑器组装（PRD-21-020）：桌面与网页拿到的是同一份扩展列表；宿主插槽的位置就是按键的先后。
import { afterEach, describe, expect, it } from "vitest";
import { history } from "@codemirror/commands";
import { EditorState, type Extension } from "@codemirror/state";
import { EditorView, keymap, runScopeHandlers } from "@codemirror/view";
import { fakeEditorHost, t } from "../test-fixture";
import { editorPhrases, livePreviewExtensions, markdownEditorExtensions, type MarkdownEditorSlots } from "./markdown-editor";
import { editorDocumentText } from "./save-session";

const views: EditorView[] = [];
afterEach(() => {
  views.splice(0).forEach((view) => view.destroy());
  document.body.replaceChildren();
});

function mount(doc: string, slots: MarkdownEditorSlots = {}, live = true) {
  const host = fakeEditorHost();
  const parent = document.createElement("div");
  document.body.appendChild(parent);
  const presentation: Extension = live
    ? livePreviewExtensions(host, {
        t,
        notePath: () => "笔记.md",
        onOpenPath: () => {},
        onActivateWikiLink: () => {},
        onOpenTag: () => {},
        onEditSource: () => {},
      })
    : [];
  const view = new EditorView({
    parent,
    state: EditorState.create({
      doc,
      selection: { anchor: doc.replace(/\r\n?/g, "\n").length },
      extensions: markdownEditorExtensions(host, {
        t,
        notePath: () => "笔记.md",
        history: history(),
        readOnly: EditorState.readOnly.of(false),
        locale: editorPhrases(t),
        presentation,
        slots,
      }),
    }),
  });
  views.push(view);
  return view;
}

const press = (view: EditorView, key: string) => runScopeHandlers(view, new KeyboardEvent("keydown", { key }), "editor");

function recordingKeymap(log: string[], name: string): Extension {
  return keymap.of([{ key: "Enter", run: () => (log.push(name), true) }]);
}

describe("编辑器组装", () => {
  it("实时预览与源码两种装法都能建起来，正文原样进出", () => {
    const doc = "---\ntitle: 甲\n---\n# 标题\r\n\n- 一\n- 二\n\n| a | b |\n| - | - |\n| 1 | 2 |\n";
    expect(editorDocumentText(mount(doc).state)).toBe(doc.replace("\r\n", "\n"));
    expect(editorDocumentText(mount(doc, {}, false).state)).toBe(doc.replace("\r\n", "\n"));
  });

  it("编辑键位链最前的插槽先于列表续写消费回车；预览之后的插槽排在它们后面", () => {
    const log: string[] = [];
    const first = mount("- 一项", { beforeEditingKeymaps: recordingKeymap(log, "前") });
    expect(press(first, "Enter")).toBe(true);
    expect(log).toEqual(["前"]);
    expect(first.state.doc.toString()).toBe("- 一项");

    const later = mount("- 一项", { afterPresentation: recordingKeymap(log, "后") });
    expect(press(later, "Enter")).toBe(true);
    // 列表续写先接住了回车，插在后面的那个没轮到。
    expect(log).toEqual(["前"]);
    expect(later.state.doc.toString()).toBe("- 一项\n- ");
  });

  it("查找面板的文案取自包自带的文案（不是 CM 的英文原文）", () => {
    const view = mount("正文");
    expect(view.state.phrase("Find")).toBe(t("editor.search.find"));
    expect(view.state.phrase("Find")).not.toBe("Find");
  });
});
