import { describe, expect, it } from "vitest";
import { parseOutline } from "./outline";

describe("parseOutline（PRD-06-014 大纲 / TOC）", () => {
  it("解析多级标题：级别、文本与 1-based 行号", () => {
    const doc = "# 一级\n\n正文\n\n## 二级\n\n### 三级";
    const outline = parseOutline(doc);
    expect(outline).toEqual([
      { level: 1, text: "一级", line: 1, headingIndex: 0 },
      { level: 2, text: "二级", line: 5, headingIndex: 1 },
      { level: 3, text: "三级", line: 7, headingIndex: 2 },
    ]);
  });

  it("frontmatter 剥离且行号补偿回原文坐标", () => {
    const doc = "---\nluna:\n  type: note\n---\n# 标题\n正文";
    const outline = parseOutline(doc);
    expect(outline).toHaveLength(1);
    expect(outline[0].line).toBe(5);
  });

  it("代码块内伪标题不入大纲", () => {
    const doc = "# 真标题\n\n```\n# 注释不是标题\n```";
    const outline = parseOutline(doc);
    expect(outline).toHaveLength(1);
    expect(outline[0].text).toBe("真标题");
  });

  it("闭合式 ATX 标题剥尾部 #；空标题跳过", () => {
    const doc = "## 闭合式 ##\n\n#\n";
    const outline = parseOutline(doc);
    expect(outline).toHaveLength(1);
    expect(outline[0].text).toBe("闭合式");
  });

  it("Setext 标题识别（=== 一级 / --- 二级）", () => {
    const doc = "一级标题\n===\n\n二级标题\n---";
    const outline = parseOutline(doc);
    expect(outline.map((h) => [h.level, h.text])).toEqual([
      [1, "一级标题"],
      [2, "二级标题"],
    ]);
  });
});
