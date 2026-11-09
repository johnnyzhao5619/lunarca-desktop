// @vitest-environment happy-dom
import { afterEach, describe, expect, it } from "vitest";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { splitImageWidth } from "../markdown/markdown";
import { openImageLightbox } from "../reading/image-lightbox";
import { ImageEmbedWidget, setImageWidth } from "./image-embed";
import { parseFully } from "./syntax-tree-test-fixture";
import { fakeEditorHost } from "../test-fixture";

const host = fakeEditorHost();

afterEach(() => {
  document.body.replaceChildren();
});

describe("图片宽度（Obsidian 写法 |300，对标方案 D4）", () => {
  it.each([
    ["说明|300", { alt: "说明", width: 300 }],
    ["|240", { alt: "", width: 240 }],
    ["图|300x200", { alt: "图", width: 300 }],
    ["a|b", { alt: "a|b", width: null }],
    ["普通说明", { alt: "普通说明", width: null }],
  ])("%s", (alt, expected) => {
    expect(splitImageWidth(alt)).toEqual(expected);
  });

  it("拖动写回只改 alt：加上、改掉、还原宽度", () => {
    const doc = "前文 ![封面|120](assets/a.png \"标题\") 后文";
    const parent = document.createElement("div");
    document.body.appendChild(parent);
    const view = new EditorView({ parent, state: EditorState.create({ doc, extensions: [markdown({ base: markdownLanguage })] }) });
    parseFully(view);
    const at = doc.indexOf("!");
    expect(setImageWidth(view, at, 300.4)).toBe(true);
    expect(view.state.doc.toString()).toBe(doc.replace("封面|120", "封面|300"));
    expect(setImageWidth(view, at, null)).toBe(true);
    expect(view.state.doc.toString()).toBe(doc.replace("封面|120", "封面"));
    expect(setImageWidth(view, at, null)).toBe(false);
    view.destroy();
  });
});

describe("编辑面图片部件", () => {
  it("按写下的宽度显示；悬停控件只在给了文案时挂上", () => {
    const view = new EditorView({ parent: document.body, state: EditorState.create({ doc: "" }) });
    const labels = { view: "大图", resize: "缩放", lightbox: { dialog: "d", close: "c", previous: "p", next: "n" } };
    const withWidth = new ImageEmbedWidget(host, "n.md", "a.png", "说明", 320, labels).toDOM(view);
    expect(withWidth.querySelector("img")!.width).toBe(320);
    expect(withWidth.querySelector(".luna-md-image-embed__resize")?.getAttribute("aria-label")).toBe("缩放");
    const plain = new ImageEmbedWidget(host, "n.md", "a.png", "说明").toDOM(view);
    expect(plain.querySelector("img")!.hasAttribute("width")).toBe(false);
    expect(plain.querySelector(".luna-md-image-embed__view")).toBeNull();
    view.destroy();
  });
});

describe("看大图", () => {
  const labels = { dialog: "viewer", close: "close", previous: "prev", next: "next" };

  it("←/→ 在图片间切换，Esc 关闭并把焦点还回去", () => {
    const opener = document.createElement("button");
    document.body.appendChild(opener);
    opener.focus();
    openImageLightbox([{ src: "a.png", alt: "甲" }, { src: "b.png", alt: "乙" }], 1, labels);
    const dialog = document.querySelector<HTMLElement>(".luna-lightbox")!;
    expect(dialog.getAttribute("aria-label")).toBe("viewer");
    expect(dialog.querySelector<HTMLImageElement>("img")!.alt).toBe("乙");
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
    expect(dialog.querySelector<HTMLImageElement>("img")!.alt).toBe("甲");
    expect(dialog.querySelector(".luna-lightbox__caption")!.textContent).toBe("甲 · 1 / 2");
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(document.querySelector(".luna-lightbox")).toBeNull();
    expect(document.activeElement).toBe(opener);
  });

  it("只有一张图时没有翻页按钮；点遮罩关闭", () => {
    openImageLightbox([{ src: "a.png", alt: "" }], 0, labels);
    const dialog = document.querySelector<HTMLElement>(".luna-lightbox")!;
    expect(dialog.querySelector(".luna-lightbox__next")).toBeNull();
    dialog.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(document.querySelector(".luna-lightbox")).toBeNull();
  });
});
