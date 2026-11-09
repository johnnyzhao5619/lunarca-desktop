import { describe, expect, it } from "vitest";
import type { EditorState } from "@codemirror/state";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { buildBlockSpacingDecorations } from "./block-spacing";
import { fullyParsed, starvedState } from "./syntax-tree-test-fixture";

/**
 * 建 state 并**把语法树解析完**再交给产品代码。
 *
 * `syntaxTree(state)` 返回的是**此刻已经解析到的那一段**，不是完整树——新状态的首轮解析只有
 * 20 ms 预算。生产代码对此是正确的：它按可见范围装饰，并在树推进时重新装饰（`block-spacing.ts`
 * 那条 `syntaxTree(update.state) !== syntaxTree(update.startState)`）。但**本文件断言的是一个
 * 精确的装饰集**，只在解析完成后才有定义。
 *
 * 以前这里只调 `ensureSyntaxTree`，而它不刷新 `syntaxTree(state)` 的快照（见
 * `syntax-tree-test-fixture.ts`），CPU 吃紧时用例偶发失败。现在先饿死首轮解析、再 `fullyParsed`：
 * 半棵树那条路每次都走，漏了补全就必红。
 */
function mdState(doc: string): EditorState {
  return fullyParsed(starvedState({ doc, extensions: [markdown({ base: markdownLanguage })] }));
}

/** 每条装饰 → `行号:类型[:tight]`（类型取 data-block，缺省为普通块；上一行非空时带 tight）。 */
function marks(doc: string): string[] {
  const state = mdState(doc);
  const set = buildBlockSpacingDecorations(state, [{ from: 0, to: state.doc.length }]);
  const out: string[] = [];
  const iter = set.iter();
  while (iter.value) {
    const attrs = iter.value.spec.attributes as Record<string, string> | undefined;
    const tight = attrs?.["data-tight"] ? ":tight" : "";
    out.push(`${state.doc.lineAt(iter.from).number}:${attrs?.["data-block"] ?? "block"}${tight}`);
    iter.next();
  }
  return out;
}

describe("块级纵向节奏（PRD-06-011，Typora / Obsidian 长文排版对标）", () => {
  it("标题行与块首行各拿一份装饰，标题带 heading 标记", () => {
    expect(marks("段落一\n\n# 标题\n\n段落二")).toEqual(["3:heading", "5:block"]);
  });

  it("文首那一块不加间距（上面没有块，只会在正文顶端多出空白）", () => {
    expect(marks("# 开篇\n\n正文")).toEqual(["3:block"]);
  });

  it("一条列表只在**列表首行**拿一份，不是每项各一份", () => {
    expect(marks("正文\n\n- a\n- b\n- c")).toEqual(["3:block"]);
  });

  it("引用块内的标题不另领间距（只认文档直属块）", () => {
    expect(marks("正文\n\n> # 引用里的标题\n> 正文")).toEqual(["3:block"]);
  });

  it("围栏代码块里的 `#` 是代码不是标题", () => {
    expect(marks("正文\n\n```sh\n# not a heading\n```")).toEqual(["3:block"]);
  });

  it("Setext 标题与分隔线同样参与节奏", () => {
    expect(marks("引子\n\n标题\n===\n\n---")).toEqual(["3:heading", "6:block"]);
  });

  /**
   * 空行已经是间距（2026-09-04 裁决）：上一行是空行的块首不再另加块间距，
   * 只有紧贴上一个非空行的块才带 `data-tight`——列表紧接段落、标题紧接正文、
   * 围栏收口紧接正文。标题的章节留白不看空行，两种形态都带 heading 标记。
   */
  it("上一行非空的块首带 tight；隔着空行的不带", () => {
    expect(marks("正文\n- a\n- b")).toEqual(["2:block:tight"]);
    expect(marks("# 标题\n正文")).toEqual(["2:block:tight"]);
    expect(marks("```\ncode\n```\n正文")).toEqual(["4:block:tight"]);
    expect(marks("正文\n# 紧接的标题")).toEqual(["2:heading:tight"]);
    expect(marks("正文\n\n# 隔空行的标题\n\n段落")).toEqual(["3:heading", "5:block"]);
  });

  it("视口之外的块不产出装饰（与其它行装饰同一惰性口径）", () => {
    const state = mdState("段落一\n\n# 标题\n\n段落二");
    const set = buildBlockSpacingDecorations(state, [{ from: 0, to: 3 }]);
    expect(set.size).toBe(0);
  });
});
