import { describe, expect, it } from "vitest";
import { GLOSSARY_TERM_MAX_CHARS, glossaryTermFromSelection } from "./glossary-term";

describe("就地加词的选区判据（PRD-07-061）", () => {
  it("普通词条原样取用（去掉首尾空白）", () => {
    expect(glossaryTermFromSelection("  Kubernetes ")).toBe("Kubernetes");
    expect(glossaryTermFromSelection("王强")).toBe("王强");
  });

  it("空选区不给菜单项", () => {
    expect(glossaryTermFromSelection(null)).toBeNull();
    expect(glossaryTermFromSelection(undefined)).toBeNull();
    expect(glossaryTermFromSelection("   ")).toBeNull();
  });

  it("跨行的选区一定不是词条", () => {
    // 在字幕里往下拖一把就会选中好几条——那是段落，不是术语。
    expect(glossaryTermFromSelection("第一行\n第二行")).toBeNull();
    expect(glossaryTermFromSelection("第一行\r\n第二行")).toBeNull();
  });

  it("超长选区不给菜单项——这是功能边界而非防呆", () => {
    // 词条会拼进 whisper 的 initial prompt（预算有限）：整段话加进去既不提升识别率，
    // 还会把真正的术语挤出提示。
    expect(glossaryTermFromSelection("字".repeat(GLOSSARY_TERM_MAX_CHARS))).toHaveLength(
      GLOSSARY_TERM_MAX_CHARS,
    );
    expect(glossaryTermFromSelection("字".repeat(GLOSSARY_TERM_MAX_CHARS + 1))).toBeNull();
  });

  it("上限按去空白后的长度判（不因首尾空格误拒）", () => {
    const term = "字".repeat(GLOSSARY_TERM_MAX_CHARS);
    expect(glossaryTermFromSelection(`   ${term}   `)).toBe(term);
  });
});
