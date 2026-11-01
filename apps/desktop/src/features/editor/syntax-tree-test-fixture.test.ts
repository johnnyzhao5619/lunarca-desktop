// @vitest-environment happy-dom
import { afterEach, describe, expect, it } from "vitest";
import { ensureSyntaxTree, syntaxTree } from "@codemirror/language";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { EditorView } from "@codemirror/view";
import { fullyParsed, parseFully, starvedState as starve } from "./syntax-tree-test-fixture";

const DOC = "# 标题\n\n段落一\n\n| a | b |\n| - | - |\n| 1 | 2 |\n\n- 项\n  - 子项\n\n结尾段落";

const starvedState = () => starve({ doc: DOC, extensions: [markdown({ base: markdownLanguage })] });

afterEach(() => {
  document.body.replaceChildren();
});

describe("语法树完整的用例状态", () => {
  it("前提：首轮解析没做完，且只调 ensureSyntaxTree 时 syntaxTree() 仍是半棵树", () => {
    const state = starvedState();
    expect(syntaxTree(state).length).toBeLessThan(DOC.length);
    expect(ensureSyntaxTree(state, state.doc.length, 5000)?.length).toBe(DOC.length);
    expect(syntaxTree(state).length).toBeLessThan(DOC.length);
  });

  it("fullyParsed 交出的状态 syntaxTree() 已到文末，文档与选区不变", () => {
    const starved = starvedState();
    const state = fullyParsed(starved);
    expect(syntaxTree(state).length).toBe(DOC.length);
    expect(state.doc.toString()).toBe(DOC);
    expect(state.selection.eq(starved.selection)).toBe(true);
  });

  it("parseFully 让视图的 syntaxTree() 到文末", () => {
    const view = new EditorView({ parent: document.body, state: starvedState() });
    expect(syntaxTree(view.state).length).toBeLessThan(DOC.length);
    parseFully(view);
    expect(syntaxTree(view.state).length).toBe(DOC.length);
    view.destroy();
  });
});
