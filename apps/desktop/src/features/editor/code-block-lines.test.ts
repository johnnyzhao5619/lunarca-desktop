import { describe, expect, it } from "vitest";
import { EditorState } from "@codemirror/state";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { buildCodeBlockLineDecorations } from "./code-block-lines";

function mdState(doc: string): EditorState {
  return EditorState.create({ doc, extensions: [markdown({ base: markdownLanguage })] });
}

/** 行装饰的 class（同一个装饰集里还有复制按钮 widget，按下面的 `copyWidgets` 取）。 */
function lineClasses(doc: string): string[] {
  const set = buildCodeBlockLineDecorations(
    mdState(doc),
    [{ from: 0, to: doc.length }],
  );
  const out: string[] = [];
  const iter = set.iter();
  while (iter.value) {
    const cls = iter.value.spec.class as string | undefined;
    if (cls !== undefined) {
      out.push(cls);
    }
    iter.next();
  }
  return out;
}

/** 复制按钮 widget 及其挂载位置（每块一个，挂在首行末尾）。 */
function copyWidgets(doc: string): { pos: number; code: string }[] {
  const state = mdState(doc);
  const set = buildCodeBlockLineDecorations(state, [{ from: 0, to: state.doc.length }]);
  const out: { pos: number; code: string }[] = [];
  const iter = set.iter();
  while (iter.value) {
    const widget = iter.value.spec.widget as { code?: string } | undefined;
    if (widget) {
      out.push({ pos: iter.from, code: widget.code ?? "" });
    }
    iter.next();
  }
  return out;
}

describe("代码块整块容器化（PRD-06-011，Typora 对标）", () => {
  it("围栏代码块每行（含 fence 行与空行）获得行装饰，首末行带圆角变体", () => {
    const classes = lineClasses("```python\nx = 1\n\ny = 2\n```");
    expect(classes).toHaveLength(5);
    expect(classes[0]).toContain("luna-md-codeblock-line--first");
    expect(classes[4]).toContain("luna-md-codeblock-line--last");
    expect(classes[1]).toBe("luna-md-codeblock-line");
    expect(classes[2]).toBe("luna-md-codeblock-line"); // 空行同样有底色
  });

  it("代码块外的行不加装饰", () => {
    const classes = lineClasses("正文\n\n```\ncode\n```\n\n尾部");
    expect(classes).toHaveLength(3); // 仅 fence/code/fence 三行
  });

  it("未闭合围栏（编辑中）覆盖到文末", () => {
    const classes = lineClasses("```\nhalf");
    expect(classes).toHaveLength(2);
    expect(classes[1]).toContain("--last");
  });

  it("单行代码块同时携带首末变体", () => {
    const classes = lineClasses("    indented-code");
    expect(classes).toHaveLength(1);
    expect(classes[0]).toContain("--first");
    expect(classes[0]).toContain("--last");
  });
});

/**
 * 「复制代码」按钮此前**只有阅读面有**：同一块代码在编辑面没有任何复制入口。
 * 本组钉住三件事：按钮挂在首行末尾（绝对定位不占行盒）、复制的是**正文**而不是
 * 连围栏一起、空块不挂。
 */
describe("代码块复制按钮（编辑面，与阅读面同一颗按钮）", () => {
  it("每块一个，挂在首行末尾", () => {
    const doc = "```python\nx = 1\ny = 2\n```";
    const widgets = copyWidgets(doc);
    expect(widgets).toHaveLength(1);
    expect(widgets[0].pos).toBe(doc.indexOf("\n"));
  });

  it("复制的是代码正文——不含围栏行，与阅读面 `codeBlockContentOf` 同一个答案", () => {
    expect(copyWidgets("```python\nx = 1\ny = 2\n```")[0].code).toBe("x = 1\ny = 2");
  });

  it("缩进式代码块剥掉缩进标记后再复制", () => {
    expect(copyWidgets("    indented-code")[0].code).toBe("indented-code");
  });

  it("空代码块不挂按钮（复制空字符串没有意义）", () => {
    expect(copyWidgets("```python\n```")).toEqual([]);
  });

  it("多个代码块各挂各的", () => {
    const widgets = copyWidgets("```js\na\n```\n\n```py\nb\n```");
    expect(widgets.map((w) => w.code)).toEqual(["a", "b"]);
  });
});
