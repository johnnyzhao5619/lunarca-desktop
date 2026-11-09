// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { LUNA_MARKDOWN_EXTENSIONS } from "@lunarca/editor-core/markdown-dialect";
import { richPreviewExtension } from "@lunarca/editor-core/rich-preview";
import { wikiLinkExtension } from "@lunarca/editor-core/wiki-link";
import { livePreviewExtension } from "@lunarca/editor-core/live-preview";
import { desktopEditorHost } from "./editor-host";

const views: EditorView[] = [];
beforeEach(() => {
  resetBindingsDouble();
  setCommandReply("vaultResolveEmbeds", { status: "ok", data: [{ kind: "missing" }] });
});
afterEach(() => {
  views.splice(0).forEach((view) => view.destroy());
  document.body.replaceChildren();
});

function mount(doc: string, cursor: number) {
  const parent = document.createElement("div");
  document.body.appendChild(parent);
  const view = new EditorView({
    parent,
    state: EditorState.create({
      doc,
      selection: { anchor: cursor },
      extensions: [
        markdown({ base: markdownLanguage, extensions: LUNA_MARKDOWN_EXTENSIONS }),
        richPreviewExtension(desktopEditorHost, {
          embed: { notePath: () => "日记.md", onOpenPath: vi.fn(), onActivateWikiLink: vi.fn() },
        }),
        wikiLinkExtension(desktopEditorHost, { notePath: () => "日记.md", onActivate: vi.fn() }),
        livePreviewExtension(desktopEditorHost, () => "日记.md"),
      ],
    }),
  });
  views.push(view);
  return view;
}

describe("实时预览里的嵌入（D5）", () => {
  it("独占一行、光标不在这一行：整行换成渲染结果；光标进来还原源码", () => {
    const doc = "开头\n\n![[周会#结论]]\n\n结尾";
    const view = mount(doc, 0);
    expect(view.dom.querySelector(".luna-editor-embed")).not.toBeNull();
    expect(view.contentDOM.textContent).not.toContain("![[周会#结论]]");
    view.dispatch({ selection: { anchor: doc.indexOf("周会") } });
    expect(view.dom.querySelector(".luna-editor-embed")).toBeNull();
    expect(view.contentDOM.textContent).toContain("![[周会#结论]]");
  });

  it("夹在文字中间的嵌入照原文", () => {
    const view = mount("见 ![[周会]] 一节\n\n结尾", 14);
    expect(view.dom.querySelector(".luna-editor-embed")).toBeNull();
    expect(view.contentDOM.textContent).toContain("![[周会]]");
    // 不当成 wiki 链接装饰（否则 `[[ ]]` 被藏掉、只剩一个「!」）。
    expect(view.dom.querySelector(".luna-wiki-link")).toBeNull();
  });
});
