// @vitest-environment happy-dom
/**
 * 输入法组字期间冻结实时预览装饰（对标方案 A3）。
 *
 * 组字时每打一个拼音字母都是一次文档变化；若每次都按新文本重建装饰，组字区附近的语法标记
 * 隐现、替换部件进出，WebKit 的组字 DOM 被改动——候选框跳位、上屏的字被吞或重复。
 * happy-dom 里起不了真组字，用 `view.composing` 打桩模拟「组字中」。
 */

import { afterEach, describe, expect, it } from "vitest";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { livePreviewExtension } from "./live-preview";

const views: EditorView[] = [];
afterEach(() => {
  for (const view of views.splice(0)) view.destroy();
  document.body.replaceChildren();
});

function mount(doc: string) {
  const parent = document.createElement("div");
  document.body.appendChild(parent);
  const view = new EditorView({
    parent,
    state: EditorState.create({ doc, extensions: [markdown({ base: markdownLanguage }), livePreviewExtension()] }),
  });
  views.push(view);
  let composing = false;
  Object.defineProperty(view, "composing", { get: () => composing, configurable: true });
  return { view, setComposing: (value: boolean) => (composing = value) };
}

describe("组字期间冻结实时预览", () => {
  it("组字中只平移旧装饰，不按新文本重建；组字结束后重建一次", async () => {
    const { view, setComposing } = mount("前文\n");
    setComposing(true);
    view.dispatch({ changes: { from: view.state.doc.length, insert: "**粗**" }, selection: { anchor: 0 } });
    // 冻结：新写出的 `**` 还没被藏起来。
    expect(view.contentDOM.textContent).toContain("**粗**");
    setComposing(false);
    view.contentDOM.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true }));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(view.contentDOM.textContent).not.toContain("**");
    expect(view.contentDOM.textContent).toContain("粗");
  });

  it("不在组字时照常随改动重建", () => {
    const { view } = mount("前文\n");
    view.dispatch({ changes: { from: view.state.doc.length, insert: "**粗**" }, selection: { anchor: 0 } });
    expect(view.contentDOM.textContent).not.toContain("**");
  });
});
