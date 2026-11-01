import { describe, expect, it } from "vitest";

import { scriptSectionRangeAt } from "./script-section-range";

// 一篇脚本：开场在第 1 行，「## 开头」在第 3 行，「## 结尾」在第 6 行。
const TEXT = "开场的话\n\n## 开头\n第一句\n\n## 结尾\n收尾\n\n";
const STARTS = [1, 3, 6];
const at = (needle: string) => TEXT.indexOf(needle) + 1;

describe("AI 改稿没有选区时改哪一段（ADR-165 ⑦）", () => {
  it("光标在哪一段就改哪一段：从标题行到下一段之前，段尾空行不算", () => {
    const range = scriptSectionRangeAt(TEXT, STARTS, at("第一句"));
    expect(range && TEXT.slice(range.from, range.to)).toBe("## 开头\n第一句");
  });

  it("最后一段到文末为止，同样不带尾部空白", () => {
    const range = scriptSectionRangeAt(TEXT, STARTS, at("收尾"));
    expect(range && TEXT.slice(range.from, range.to)).toBe("## 结尾\n收尾");
  });

  it("光标落在标题行本身也算这一段；开场是第一段", () => {
    expect(scriptSectionRangeAt(TEXT, STARTS, TEXT.indexOf("## 结尾"))).toEqual(
      scriptSectionRangeAt(TEXT, STARTS, at("收尾")),
    );
    const opening = scriptSectionRangeAt(TEXT, STARTS, 0);
    expect(opening && TEXT.slice(opening.from, opening.to)).toBe("开场的话");
  });

  it("光标在第一段之前（没有开场段）、或根本没有分段：不改", () => {
    expect(scriptSectionRangeAt(TEXT, [3, 6], 0)).toBeNull();
    expect(scriptSectionRangeAt(TEXT, [], at("第一句"))).toBeNull();
    expect(scriptSectionRangeAt("", [1], 0)).toBeNull();
  });

  it("越界或重复的行号不影响结果", () => {
    expect(scriptSectionRangeAt(TEXT, [6, 3, 1, 3, 99, 0], at("第一句"))).toEqual(
      scriptSectionRangeAt(TEXT, STARTS, at("第一句")),
    );
  });
});
