// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from "vitest";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { LUNA_MARKDOWN_EXTENSIONS } from "../../lib/markdown-dialect";
import { fullyParsed } from "./syntax-tree-test-fixture";

vi.mock("../../lib/math-render", () => ({
  renderMathInto: (element: HTMLElement, tex: string) => { element.textContent = `[${tex}]`; },
}));
vi.mock("../../lib/mermaid-render", () => ({
  renderMermaid: async () => ({ svg: null, syntaxProblem: "bad" }),
  svgNode: () => null,
}));

import { buildRichBlockDecorations, richPreviewExtension } from "./rich-preview";

const views: EditorView[] = [];
afterEach(() => {
  for (const view of views.splice(0)) view.destroy();
  document.body.replaceChildren();
});

function state(doc: string, cursor = 0) {
  return fullyParsed(EditorState.create({ doc, selection: { anchor: cursor }, extensions: [markdown({ base: markdownLanguage, extensions: LUNA_MARKDOWN_EXTENSIONS })] }));
}

const labels = { diagramError: (m: string) => `err:${m}` };

describe("实时预览里的公式与图表（对标方案 D1 / D2）", () => {
  it("成块公式与 mermaid 在光标外整块替换，光标进入即还原源码", () => {
    const doc = "首行\n\n$$\nx^2\n$$\n\n```mermaid\ngraph TD\nA-->B\n```\n\n```js\nlet a\n```";
    expect(buildRichBlockDecorations(state(doc), labels).size).toBe(2);
    expect(buildRichBlockDecorations(state(doc, doc.indexOf("x^2")), labels).size).toBe(1);
    expect(buildRichBlockDecorations(state(doc, doc.indexOf("A-->")), labels).size).toBe(1);
  });

  it("行内公式在光标外显示渲染结果，光标碰到就露出 TeX", () => {
    const doc = "质能 $E=mc^2$ 关系";
    const parent = document.createElement("div");
    document.body.appendChild(parent);
    const view = new EditorView({ parent, state: EditorState.create({ doc, selection: { anchor: 0 }, extensions: [
      markdown({ base: markdownLanguage, extensions: LUNA_MARKDOWN_EXTENSIONS }), richPreviewExtension(labels),
    ] }) });
    views.push(view);
    expect(view.contentDOM.textContent).toContain("[E=mc^2]");
    expect(view.contentDOM.textContent).not.toContain("$");
    view.dispatch({ selection: { anchor: doc.indexOf("mc") } });
    expect(view.contentDOM.textContent).toContain("$E=mc^2$");
  });

  it("图表画不出来时显示说明与源码", async () => {
    const doc = "首行\n\n```mermaid\ngraph TD\n```";
    const parent = document.createElement("div");
    document.body.appendChild(parent);
    const view = new EditorView({ parent, state: EditorState.create({ doc, selection: { anchor: 0 }, extensions: [
      markdown({ base: markdownLanguage, extensions: LUNA_MARKDOWN_EXTENSIONS }), richPreviewExtension(labels),
    ] }) });
    views.push(view);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(view.dom.querySelector(".luna-mermaid .luna-field-error")?.textContent).toBe("err:bad");
    expect(view.dom.querySelector(".luna-mermaid pre")?.textContent).toBe("graph TD");
  });
});

describe("脚注（对标方案 D3）", () => {
  it("光标外的引用显示成上标，悬停是脚注内容，点一下跳到定义；配不上的引用原样显示", () => {
    const doc = "正文[^1]与[^x]。\n\n[^1]: 第一条 **说明**";
    const parent = document.createElement("div");
    document.body.appendChild(parent);
    const view = new EditorView({ parent, state: EditorState.create({ doc, selection: { anchor: 0 }, extensions: [
      markdown({ base: markdownLanguage, extensions: LUNA_MARKDOWN_EXTENSIONS }), richPreviewExtension(labels),
    ] }) });
    views.push(view);
    const refs = view.dom.querySelectorAll<HTMLElement>("sup.luna-footnote-ref");
    expect(refs).toHaveLength(1);
    expect(refs[0].textContent).toBe("1");
    expect(refs[0].title).toBe("第一条 说明");
    expect(view.contentDOM.textContent).toContain("[^x]");
    refs[0].dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));
    expect(view.state.doc.lineAt(view.state.selection.main.head).text.startsWith("[^1]:")).toBe(true);
  });
});
