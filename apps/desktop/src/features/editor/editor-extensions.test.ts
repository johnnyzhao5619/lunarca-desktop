import { EditorState } from "@codemirror/state";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { describe, expect, it } from "vitest";
import {
  countFrontmatterKeys,
  countPropertyLines,
  getFrontmatterRange,
  listContinuationForState,
  wordCount,
  noteWordCount,
} from "./editor-extensions";

function stateAtEnd(doc: string) {
  return EditorState.create({
    doc,
    selection: { anchor: doc.length },
  });
}

function markdownStateAtEnd(doc: string) {
  return EditorState.create({
    doc,
    selection: { anchor: doc.length },
    extensions: [markdown({ base: markdownLanguage })],
  });
}

describe("editor extensions", () => {
  it("continues unordered lists", () => {
    expect(listContinuationForState(stateAtEnd("- item"))?.insert).toBe("\n- ");
  });

  it("increments ordered lists", () => {
    expect(listContinuationForState(stateAtEnd("1. item"))?.insert).toBe("\n2. ");
  });

  it("continues task lists", () => {
    expect(listContinuationForState(stateAtEnd("- [ ] item"))?.insert).toBe("\n- [ ] ");
  });

  it("exits an empty list item", () => {
    expect(listContinuationForState(stateAtEnd("- "))?.insert).toBe("\n");
  });

  it("引用块：续一行 `> `，空引用行回车退出，引用里的空项回车留在引用里", () => {
    expect(listContinuationForState(stateAtEnd("> 引文"))?.insert).toBe("\n> ");
    expect(listContinuationForState(stateAtEnd("> - 甲"))?.insert).toBe("\n> - ");
    expect(listContinuationForState(stateAtEnd("> "))).toEqual({ from: 0, to: 2, insert: "", cursor: 0 });
    expect(listContinuationForState(stateAtEnd("> - "))).toEqual({ from: 2, to: 4, insert: "", cursor: 2 });
  });

  it("嵌套的空项回车先降一级，不直接退出列表", () => {
    expect(listContinuationForState(stateAtEnd("- a\n  - "))).toEqual({
      from: 4,
      to: 6,
      insert: "",
      cursor: 6,
    });
  });

  it("不在 fenced code 内续写 Markdown 列表", () => {
    expect(listContinuationForState(markdownStateAtEnd("```text\n- literal"))).toBeNull();
  });

  it("counts CJK characters and latin words as a mixed sum", () => {
    expect(wordCount("hello world 中文 mix中文")).toBe(7);
    // 只数正文：frontmatter 不算（设字数目标会往 `luna:` 块里加一行）。
    expect(noteWordCount("---\nluna:\n  word_goal: 20\n---\n正文 two words")).toBe(4);
    expect(noteWordCount("没有 frontmatter")).toBe(13 - 10);
    // 没闭合的 `---` 不是 frontmatter，照常数。
    expect(noteWordCount("---\nabc")).toBe(1);
  });
});

describe("frontmatter 胶囊（折叠态自带信息量）", () => {
  const doc = "---\ntitle: 会议\ntags:\n  - a\n  - b\nluna.type: recording\n---\n正文";

  it("区间从文首到闭合 `---` 行尾；没有 frontmatter 则 null", () => {
    const state = stateAtEnd(doc);
    const range = getFrontmatterRange(state);
    expect(range?.from).toBe(0);
    expect(state.doc.sliceString(range!.to - 3, range!.to)).toBe("---");
    expect(getFrontmatterRange(stateAtEnd("正文\n---\n"))).toBeNull();
  });

  it("数属性、不数容器：列表项是值，`luna:` 这类只装子键的容器不算，子键各算一项", () => {
    const state = stateAtEnd(doc);
    expect(countFrontmatterKeys(state, getFrontmatterRange(state)!)).toBe(3);
    // 本仓库的真实形状：一切嵌在 `luna:` 下——只数顶层键永远是 1，等于没数。
    expect(
      countPropertyLines([
        "luna:",
        "  schema: 1",
        "  type: note",
        "  ai_tags:",
        "",
        "    - 趋势分析",
        "    - 交易策略",
        "  distill_source: Transcripts/x.md",
        "  distill_full_text: true",
      ]),
    ).toBe(5);
  });
});
