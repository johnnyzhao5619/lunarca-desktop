import { describe, expect, it } from "vitest";
import { parseCalloutMarker, quotePrefixLength, stripQuoteLevel } from "./callout";

describe("callout marker parsing", () => {
  it("reads type, tone and title", () => {
    const marker = parseCalloutMarker("[!WARNING] 别忘了备份");
    expect(marker).toMatchObject({
      type: "warning",
      tone: "warning",
      label: "WARNING",
      defaultFolded: false,
      title: "别忘了备份",
    });
    // markerEnd 落在 `]` 之后（不含随后的那一个空格）。
    expect("[!WARNING] 别忘了备份".slice(marker!.markerEnd)).toBe(" 别忘了备份");
  });

  it("maps alias families onto the five tones", () => {
    expect(parseCalloutMarker("[!bug]")?.tone).toBe("danger");
    expect(parseCalloutMarker("[!tip]")?.tone).toBe("success");
    expect(parseCalloutMarker("[!caution]")?.tone).toBe("warning");
    expect(parseCalloutMarker("[!summary]")?.tone).toBe("quote");
    // 陌生类型仍是标注块（退默认色调），不退化成普通引用。
    expect(parseCalloutMarker("[!spaceship]")?.tone).toBe("note");
  });

  it("honours the fold suffix", () => {
    expect(parseCalloutMarker("[!note]- 收起的")?.defaultFolded).toBe(true);
    expect(parseCalloutMarker("[!note]+ 展开的")?.defaultFolded).toBe(false);
    expect(parseCalloutMarker("[!note] 缺省")?.defaultFolded).toBe(false);
  });

  it("keeps an empty title empty (label 由类型词兜底呈现)", () => {
    expect(parseCalloutMarker("[!note]")?.title).toBe("");
  });

  it("rejects anything that is not a callout head", () => {
    expect(parseCalloutMarker("普通引用文字")).toBeNull();
    expect(parseCalloutMarker("[note] 缺少感叹号")).toBeNull();
    expect(parseCalloutMarker("前面有字 [!note]")).toBeNull();
    expect(parseCalloutMarker("[!] 缺类型")).toBeNull();
  });

  it("strips exactly one quote level (嵌套引用逐层交还解析器)", () => {
    expect(stripQuoteLevel("> [!note] x")).toBe("[!note] x");
    expect(stripQuoteLevel("> > 嵌套")).toBe("> 嵌套");
    expect(stripQuoteLevel(">无空格")).toBe("无空格");
    expect(stripQuoteLevel("不是引用")).toBe("不是引用");
  });

  it("measures the quote prefix including nesting", () => {
    expect(quotePrefixLength("> [!note] x")).toBe(2);
    expect(quotePrefixLength(">[!note] x")).toBe(1);
    expect(quotePrefixLength("> > 嵌套")).toBe(4);
    expect(quotePrefixLength("不是引用")).toBe(0);
  });
});
