import { describe, expect, it } from "vitest";
import { FrontmatterSyntax, lunaMarkdownParser } from "./markdown-dialect";

function strongTexts(text: string): string[] {
  const out: string[] = [];
  lunaMarkdownParser.parse(text).iterate({
    enter: (node) => {
      if (node.name === "StrongEmphasis") out.push(text.slice(node.from, node.to));
    },
  });
  return out;
}

function emphasisTexts(text: string): string[] {
  const out: string[] = [];
  lunaMarkdownParser.parse(text).iterate({
    enter: (node) => {
      if (node.name === "Emphasis") out.push(text.slice(node.from, node.to));
    },
  });
  return out;
}

describe("CJK 友好强调（PRD-06-016）", () => {
  it("全角标点紧贴分隔符、外侧是中文时照样加粗", () => {
    expect(strongTexts("**测试。**文字")).toEqual(["**测试。**"]);
    expect(strongTexts("他说**“你好”**然后")).toEqual(["**“你好”**"]);
    expect(strongTexts("**（注）**见下")).toEqual(["**（注）**"]);
    expect(emphasisTexts("这是*「引号」*内容")).toEqual(["*「引号」*"]);
  });

  it("CommonMark 本来就认的写法不变", () => {
    expect(strongTexts("这是**加粗**文字")).toEqual(["**加粗**"]);
    expect(strongTexts("**粗体**。")).toEqual(["**粗体**"]);
    expect(strongTexts("a **b** c")).toEqual(["**b**"]);
  });

  it("拉丁文本逐字沿用 CommonMark：标点内侧 + 字母外侧仍不加粗", () => {
    expect(strongTexts("a**\"b\"**c")).toEqual([]);
    expect(emphasisTexts("snake_case_name")).toEqual([]);
    expect(strongTexts("** 不是 **")).toEqual([]);
  });
});

function nodes(text: string, name: string): string[] {
  const out: string[] = [];
  lunaMarkdownParser.parse(text).iterate({
    enter: (node) => {
      if (node.name === name) out.push(text.slice(node.from, node.to));
    },
  });
  return out;
}

describe("公式语法（PRD-06-011 2026-10-03）", () => {
  it("行内 $…$；金额与转义不算", () => {
    expect(nodes("质能 $E=mc^2$ 关系", "InlineMath")).toEqual(["$E=mc^2$"]);
    expect(nodes("花了 $5 和 $10", "InlineMath")).toEqual([]);
    expect(nodes("价格 \\$5 与 $x$", "InlineMath")).toEqual(["$x$"]);
    expect(nodes("$ a $", "InlineMath")).toEqual([]);
    expect(nodes("`$x$` 是代码", "InlineMath")).toEqual([]);
  });

  it("成块 $$…$$：多行、单行，可打断段落", () => {
    expect(nodes("$$\n\\int_0^1 x\\,dx\n$$\n\n后文", "BlockMath")).toEqual(["$$\n\\int_0^1 x\\,dx\n$$"]);
    expect(nodes("$$ a+b $$", "BlockMath")).toEqual(["$$ a+b $$"]);
    expect(nodes("段落\n$$\nx\n$$", "BlockMath")).toEqual(["$$\nx\n$$"]);
  });

  it("mathSource 去掉分隔符", async () => {
    const { mathSource } = await import("./markdown-dialect");
    expect(mathSource("$E=mc^2$", false)).toBe("E=mc^2");
    expect(mathSource("$$\n\\int x\n$$", true)).toBe("\\int x");
    expect(mathSource("$$ a+b $$", true)).toBe("a+b");
  });
});

describe("脚注语法（对标方案 D3）", () => {
  it("[^id] 是引用，[^id]: 起一段定义（含行内格式，续行到空行）", () => {
    const doc = "正文[^1]与[^note]。\n\n[^1]: 第一条 **说明**\n  续写\n\n[^note]: 另一条";
    expect(nodes(doc, "FootnoteRef")).toEqual(["[^1]", "[^note]"]);
    expect(nodes(doc, "FootnoteDefinition")).toEqual(["[^1]: 第一条 **说明**\n  续写", "[^note]: 另一条"]);
    expect(nodes(doc, "StrongEmphasis")).toEqual(["**说明**"]);
  });

  it("[^ 空格] 与普通方括号不是脚注", () => {
    expect(nodes("[^ x] 与 [普通](a.md)", "FootnoteRef")).toEqual([]);
  });
});

describe("文首 frontmatter（编辑器方言，ADR-139 真机修订）", () => {
  const parser = lunaMarkdownParser.configure([FrontmatterSyntax]);
  const topNames = (text: string) => {
    const names: string[] = [];
    for (let node = parser.parse(text).topNode.firstChild; node; node = node.nextSibling) names.push(node.name);
    return names;
  };

  it("闭合的 frontmatter 整段一块：YAML 注释不是标题、列表不是列表、闭合行不是分隔线", () => {
    const doc = "---\n# 注释\ntags:\n  - 工作\n---\n# 正文标题\n\n段落";
    expect(topNames(doc)).toEqual(["Frontmatter", "ATXHeading1", "Paragraph"]);
    const tree = parser.parse(doc);
    expect(tree.topNode.firstChild?.to).toBe(doc.indexOf("\n# 正文标题"));
  });

  it("没闭合、不在文首、或首行不是 ---：照常按 Markdown 解析", () => {
    expect(topNames("---\n# 半截\n正文")).not.toContain("Frontmatter");
    expect(topNames("前言\n\n---\na: 1\n---\n")).not.toContain("Frontmatter");
    expect(topNames("--- \na: 1\n---\n")).not.toContain("Frontmatter");
  });
});
