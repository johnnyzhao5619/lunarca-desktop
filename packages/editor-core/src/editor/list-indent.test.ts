import { describe, expect, it } from "vitest";
import type { EditorState } from "@codemirror/state";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { buildListIndentDecorations, listHangStyle, listHangWidth } from "./list-indent";
import { fullyParsed, starvedState } from "./syntax-tree-test-fixture";

/**
 * 建 state 并把语法树解析完再交给产品代码（`syntax-tree-test-fixture.ts`）。生产代码在树推进时
 * 重新装饰，本文件断言的却是一次性的完整结果；首轮解析只有 20 ms，全量闸门并发时只做完一小截，
 * 用例偶发少几行悬挂缩进。`starvedState` 把那条路钉成每次都走。
 */
function mdState(doc: string): EditorState {
  return fullyParsed(
    starvedState({ doc, extensions: [markdown({ base: markdownLanguage })] }),
  );
}

/** 装饰行的 (行首偏移, --md-hang 值) 列表。 */
function hangs(state: EditorState): Array<{ at: number; hang: string }> {
  const set = buildListIndentDecorations(state, [{ from: 0, to: state.doc.length }]);
  const out: Array<{ at: number; hang: string }> = [];
  const iter = set.iter();
  while (iter.value) {
    const style = (iter.value.spec.attributes as { style: string }).style;
    out.push({ at: iter.from, hang: style });
    iter.next();
  }
  return out;
}

/** 期望的行内注入（与 `listHangStyle` 同口径：两个模式各一份事实）。 */
function hangCss(chars: number, bullets: number, sourceChars?: number): string {
  return listHangStyle({ chars, bullets, sourceChars: sourceChars ?? chars + bullets * 2 });
}

describe("listHangWidth", () => {
  it("无序 / 有序 / 任务 / 嵌套的前缀宽度", () => {
    expect(listHangWidth("- a")).toEqual({ chars: 0, bullets: 1, sourceChars: 2 });
    expect(listHangWidth("  - a")).toEqual({ chars: 2, bullets: 1, sourceChars: 4 });
    expect(listHangWidth("12. a")).toEqual({ chars: 4, bullets: 0, sourceChars: 4 });
    // 任务项：`- ` 只是隐藏（复选框 widget 承担锚点），`[ ]` 也原样占位，整段按字符计。
    expect(listHangWidth("- [ ] a")).toEqual({ chars: 6, bullets: 0, sourceChars: 6 });
    expect(listHangWidth("- [x] a")).toEqual({ chars: 6, bullets: 0, sourceChars: 6 });
    // 无序项按**渲染**宽度计：标记连同整段空格换成一格定宽圆点，多敲的空格不进 hang。
    // 源码模式的前缀按真实字符计：`*` + 三个空格 = 4ch，与 Live 的一格定宽解耦。
    expect(listHangWidth("*   a")).toEqual({ chars: 0, bullets: 1, sourceChars: 4 });
    expect(listHangWidth("   -  a")).toEqual({ chars: 3, bullets: 1, sourceChars: 6 });
  });

  /**
   * 一行里可以有不止一个标记：`* * a` 在 CommonMark 里是「只含一个列表的列表」，
   * 两个标记各被换成一个定宽圆点。只认第一个 = 渲染出 N 格前缀却只悬挂一格。
   */
  it("一行内嵌套：每个无序标记各计一格", () => {
    expect(listHangWidth("* * a")).toEqual({ chars: 0, bullets: 2, sourceChars: 4 });
    expect(listHangWidth("* * * * * * a")).toEqual({ chars: 0, bullets: 6, sourceChars: 12 });
    expect(listHangWidth("  - - a")).toEqual({ chars: 2, bullets: 2, sourceChars: 6 });
    // 有序嵌在无序里：编号原样可见，按字符计；圆点仍占一格。
    expect(listHangWidth("- 1. a")).toEqual({ chars: 3, bullets: 1, sourceChars: 5 });
  });

  it("非列表行不给悬挂", () => {
    expect(listHangWidth("正文一段。")).toBeNull();
    expect(listHangWidth("-连着字不是列表")).toBeNull();
    expect(listHangWidth("")).toBeNull();
    // 圆点后面跟的不是标记就到此为止，不会把正文吃进前缀。
    expect(listHangWidth("- x - y")).toEqual({ chars: 0, bullets: 1, sourceChars: 2 });
  });
});

describe("listHangStyle", () => {
  /**
   * 两份事实各按各的量纲：Live 里原样字符走 `ch`、圆点走层级步长 Token（两种量纲相加
   * 而不折算——把圆点也写成 `ch` 正是旧实现对不齐的来源）；源码模式整段前缀都是裸
   * 字符，只有 `ch` 一种量纲。选用哪份由 CSS 按 `data-view-mode` 决定。
   */
  it("Live 走 ch+Token 两量纲，源码走纯 ch", () => {
    expect(listHangStyle({ chars: 2, bullets: 1, sourceChars: 4 })).toBe(
      "--md-hang-live: calc(2ch + 1 * var(--editor-list-indent)); --md-hang-source: 4ch",
    );
    // 真机现场：六标记畸形行，Live 六格定宽（12em），源码只有 12 个裸字符（12ch）——
    // 旧实现把源码也按 12em 悬挂，续行比首行内容起点深出近一倍。
    expect(listHangStyle({ chars: 0, bullets: 6, sourceChars: 12 })).toBe(
      "--md-hang-live: calc(0ch + 6 * var(--editor-list-indent)); --md-hang-source: 12ch",
    );
  });
});

describe("buildListIndentDecorations", () => {
  it("每个列表项的标记行各得一条装饰，宽度随层级递进", () => {
    const state = mdState("- 顶层\n  - 嵌套\n- 又一条\n");
    expect(hangs(state)).toEqual([
      { at: 0, hang: hangCss(0, 1) },
      { at: state.doc.line(2).from, hang: hangCss(2, 1) },
      { at: state.doc.line(3).from, hang: hangCss(0, 1) },
    ]);
  });

  it("有序列表按编号实际宽度悬挂", () => {
    const state = mdState("9. 短\n10. 两位数\n");
    expect(hangs(state).map((h) => h.hang)).toEqual([hangCss(3, 0), hangCss(4, 0)]);
  });

  /**
   * 真机现场：AI 生成的会议纪要里满是 `* * …` / `* * * * * * …`。一行一条装饰，
   * 但那条装饰必须把该行**全部**标记算进去。
   */
  it("一行内嵌套只给一条装饰，且悬挂等于该行全部标记之和", () => {
    const state = mdState("* * 离线信号比对\n\n* * * * * * 分阶段迁移\n");
    expect(hangs(state)).toEqual([
      { at: 0, hang: hangCss(0, 2) },
      { at: state.doc.line(3).from, hang: hangCss(0, 6) },
    ]);
  });

  /**
   * 围栏代码块里的 `- foo` 是代码不是列表——这正是"必须走语法树、不能对每行
   * 跑正则"的理由（lezer 不会在 FencedCode 里解析出 ListItem）。
   */
  it("代码块里长得像列表的行不缩进", () => {
    const state = mdState("```\n- 这是代码\n```\n");
    expect(hangs(state)).toEqual([]);
  });

  it("普通正文零装饰", () => {
    const state = mdState("一段话。\n\n另一段。\n");
    expect(hangs(state)).toEqual([]);
  });
});
