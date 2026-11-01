import { describe, expect, it } from "vitest";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { EditorState } from "@codemirror/state";
import { placeMisspellings, spellingTargetAt } from "./spellcheck";
import { fullyParsed } from "./syntax-tree-test-fixture";

const DOC = "---\ntitel: x\n---\nThe wrold is `teh` here.\n\n```\nfuntion foo\n```\nSee https://exmaple.com now\n";

function stateOf(doc: string, cursor = 0) {
  return fullyParsed(EditorState.create({ doc, selection: { anchor: cursor }, extensions: [markdown({ base: markdownLanguage })] }));
}

/** 某个词在文档里的区间（模拟 Rust 返回的、相对整篇的位置）。 */
function rangeOf(doc: string, word: string) {
  const start = doc.indexOf(word);
  return { start, end: start + word.length };
}

describe("spellingTargetAt", () => {
  const state = stateOf(DOC);
  const at = (needle: string) => spellingTargetAt(state, DOC.indexOf(needle) + 1);

  it("正文里的词交出去，带上原位置", () => {
    const target = at("wrold");
    expect(target?.word).toBe("wrold");
    expect(state.sliceDoc(target!.from, target!.to)).toBe("wrold");
  });

  it("行内代码、代码块、网址、frontmatter 里的词不查", () => {
    expect(at("teh")).toBeNull();
    expect(at("funtion")).toBeNull();
    expect(at("exmaple")).toBeNull();
    expect(at("titel")).toBeNull();
  });

  it("光标不在词上时没有目标", () => {
    expect(spellingTargetAt(state, DOC.indexOf("\n\n") + 1)).toBeNull();
  });
});

describe("placeMisspellings", () => {
  const all = ["titel", "wrold", "teh", "funtion", "exmaple"].map((word) => rangeOf(DOC, word));

  it("只画正文里的错词：代码、网址、frontmatter 里的丢掉", () => {
    const state = stateOf(DOC, 0);
    const placed = placeMisspellings(state, 0, all).map((span) => state.sliceDoc(span.from, span.to));
    expect(placed).toEqual(["wrold"]);
  });

  it("光标正在打的那个词先不标", () => {
    const state = stateOf(DOC, DOC.indexOf("wrold") + 5);
    expect(placeMisspellings(state, 0, all)).toEqual([]);
  });

  it("送去的是从某行开始的一段时，按那一段的起点换成文档位置", () => {
    const offset = DOC.indexOf("The wrold");
    const state = stateOf(DOC, 0);
    const local = { start: "The ".length, end: "The wrold".length };
    const [span] = placeMisspellings(state, offset, [local]);
    expect(state.sliceDoc(span.from, span.to)).toBe("wrold");
  });
});
