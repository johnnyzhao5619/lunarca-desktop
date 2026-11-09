// 括号 / 引号智能配对回归（PRD-06-014，Typora 对标）：closeBrackets × markdown 语言 data。

import { describe, expect, it } from "vitest";
import { EditorState } from "@codemirror/state";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { insertBracket } from "@codemirror/autocomplete";
import { markdownCloseBracketsData } from "./editor-extensions";

describe("括号/引号智能配对（closeBrackets × markdown 语言 data）", () => {
  function pairState(doc: string, anchor: number): EditorState {
    return EditorState.create({
      doc,
      selection: { anchor },
      extensions: [markdown({ base: markdownLanguage }), markdownCloseBracketsData()],
    });
  }

  it("括号即时配对；反引号经语言 data 补入配对集", () => {
    const st = pairState("abc ", 4);
    expect(insertBracket(st, "(")?.newDoc.toString()).toBe("abc ()");
    expect(insertBracket(st, "`")?.newDoc.toString()).toBe("abc ``");
    expect(insertBracket(st, '"')?.newDoc.toString()).toBe('abc ""');
  });

  it("引号在字词字符后不自动配对（CM 既定防误配策略，锁定为设计行为）", () => {
    const afterWord = pairState("don", 3);
    expect(insertBracket(afterWord, "'")).toBeNull();
    const afterCjk = pairState("你好", 2);
    expect(insertBracket(afterCjk, '"')).toBeNull();
  });

  it("选区被括号包裹", () => {
    const st = EditorState.create({
      doc: "abc",
      selection: { anchor: 0, head: 3 },
      extensions: [markdown({ base: markdownLanguage }), markdownCloseBracketsData()],
    });
    expect(insertBracket(st, "(")?.newDoc.toString()).toBe("(abc)");
  });
});
