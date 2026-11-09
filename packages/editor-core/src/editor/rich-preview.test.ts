// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from "vitest";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { LUNA_MARKDOWN_EXTENSIONS } from "../markdown/markdown-dialect";
import { setI18n } from "react-i18next";
import { fullyParsed } from "./syntax-tree-test-fixture";
import { fakeEditorHost, recordingHost, t, testI18n } from "../test-fixture";

vi.mock("../render/math-render", () => ({
  renderMathInto: (element: HTMLElement, tex: string) => { element.textContent = `[${tex}]`; },
}));
vi.mock("../render/mermaid-render", () => ({
  renderMermaid: async () => ({ svg: null, syntaxProblem: "bad" }),
  mermaidIfRendered: () => null,
}));

import { buildRichBlockDecorations, richPreviewExtension } from "./rich-preview";

// 图表部件挂的是阅读面同一个 `MermaidView`，它在部件自己的 React root 里取全局 i18n 实例（桌面同此）。
setI18n(testI18n);

const views: EditorView[] = [];
afterEach(() => {
  for (const view of views.splice(0)) view.destroy();
  document.body.replaceChildren();
});

function state(doc: string, cursor = 0) {
  return fullyParsed(EditorState.create({ doc, selection: { anchor: cursor }, extensions: [markdown({ base: markdownLanguage, extensions: LUNA_MARKDOWN_EXTENSIONS })] }));
}

const host = fakeEditorHost();

describe("实时预览里的公式与图表（对标方案 D1 / D2）", () => {
  it("成块公式与 mermaid 在光标外整块替换，光标进入即还原源码", () => {
    const doc = "首行\n\n$$\nx^2\n$$\n\n```mermaid\ngraph TD\nA-->B\n```\n\n```js\nlet a\n```";
    expect(buildRichBlockDecorations(host, state(doc), {}).size).toBe(2);
    expect(buildRichBlockDecorations(host, state(doc, doc.indexOf("x^2")), {}).size).toBe(1);
    expect(buildRichBlockDecorations(host, state(doc, doc.indexOf("A-->")), {}).size).toBe(1);
  });

  it("行内公式在光标外显示渲染结果，光标碰到就露出 TeX", () => {
    const doc = "质能 $E=mc^2$ 关系";
    const parent = document.createElement("div");
    document.body.appendChild(parent);
    const view = new EditorView({ parent, state: EditorState.create({ doc, selection: { anchor: 0 }, extensions: [
      markdown({ base: markdownLanguage, extensions: LUNA_MARKDOWN_EXTENSIONS }), richPreviewExtension(host, {}),
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
      markdown({ base: markdownLanguage, extensions: LUNA_MARKDOWN_EXTENSIONS }), richPreviewExtension(host, {}),
    ] }) });
    views.push(view);
    await vi.waitFor(() =>
      expect(view.dom.querySelector(".luna-mermaid .luna-field-error")?.textContent).toBe(
        t("markdown.diagramError", { message: "bad" }),
      ),
    );
    expect(view.dom.querySelector(".luna-mermaid pre")?.textContent).toBe("graph TD");
  });

  it("宿主要求画成图片时（网页）显示宿主给的图片，部件拆掉时交还地址", async () => {
    const release = vi.fn();
    const render = vi.fn(async () => ({ kind: "image" as const, url: "blob:diagram", release }));
    const imageHost = fakeEditorHost({ markdown: recordingHost({ diagrams: { kind: "image", render } }) });
    const doc = "首行\n\n```mermaid\ngraph TD\nA-->B\n```";
    const parent = document.createElement("div");
    document.body.appendChild(parent);
    const view = new EditorView({ parent, state: EditorState.create({ doc, selection: { anchor: 0 }, extensions: [
      markdown({ base: markdownLanguage, extensions: LUNA_MARKDOWN_EXTENSIONS }), richPreviewExtension(imageHost, {}),
    ] }) });
    await vi.waitFor(() =>
      expect(view.dom.querySelector<HTMLImageElement>(".luna-mermaid img.luna-mermaid__image")?.getAttribute("src")).toBe(
        "blob:diagram",
      ),
    );
    expect(render).toHaveBeenCalledWith("graph TD\nA-->B", "default");
    view.destroy();
    await vi.waitFor(() => expect(release).toHaveBeenCalledTimes(1));
  });
});

describe("脚注（对标方案 D3）", () => {
  it("光标外的引用显示成上标，悬停是脚注内容，点一下跳到定义；配不上的引用原样显示", () => {
    const doc = "正文[^1]与[^x]。\n\n[^1]: 第一条 **说明**";
    const parent = document.createElement("div");
    document.body.appendChild(parent);
    const view = new EditorView({ parent, state: EditorState.create({ doc, selection: { anchor: 0 }, extensions: [
      markdown({ base: markdownLanguage, extensions: LUNA_MARKDOWN_EXTENSIONS }), richPreviewExtension(host, {}),
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
