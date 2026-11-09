// 围栏代码块语言高亮 + 括号配对端到端回归（PRD-06-011/014，Typora 对标）。
// 真机排查结论（2026-07-17）：管线本身通——「不生效」的直接诱因是语言名拼写
// 错误（如 `pyhton`）匹配不到解析器（诚实降级为纯文本）；本测试把整条
// 「language-data 加载 → 嵌套解析 → luna 高亮样式产出 token 类」链路锁定防回归。

import { describe, expect, it } from "vitest";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { highlightTree, tags } from "@lezer/highlight";
import { HighlightStyle } from "@codemirror/language";
import { lunaHighlightStyle } from "./markdown-highlight";
import { codeLanguageForFence, ensureFenceLanguage, highlightCodeSpans } from "./code-highlight";

/**
 * 阅读面（React 只读渲染）走的是同一条链路的**手动**版本：没有 CM6 帮忙嵌套解析，
 * 故 `highlightCodeSpans()` 自己跑一次 `highlightTree`。本组锁的是"两面拿到的是同一批
 * class"——此前阅读面压根没有这一步，同一段代码编辑态彩色、阅读态一整片单色。
 */
describe("阅读面代码着色（highlightCodeSpans，两面同源）", () => {
  it("语法包就绪后产出与编辑面同一批 luna-tok-* 类", async () => {
    expect(await ensureFenceLanguage("python")).toBe(true);
    const spans = highlightCodeSpans("def f(x):\n    return 'hi'\n", "python");
    expect(spans).not.toBeNull();
    const classes = new Set(spans!.flatMap((s) => s.className.split(" ")));
    expect(classes.has("luna-tok-keyword")).toBe(true);
    expect(classes.has("luna-tok-string")).toBe(true);
  });

  it("区间落在真实字符上（拿区间切原文能切回 token 文本）", async () => {
    await ensureFenceLanguage("python");
    const code = "def f(x):\n    return 'hi'\n";
    const spans = highlightCodeSpans(code, "python")!;
    const keyword = spans.find((s) => s.className.includes("luna-tok-keyword"));
    expect(code.slice(keyword!.from, keyword!.to)).toBe("def");
  });

  it("未知语言名诚实降级为 null（调用方按纯文本渲染，不报错不闪烁）", async () => {
    expect(highlightCodeSpans("def f(): pass", "pyhton")).toBeNull();
    expect(highlightCodeSpans("plain text", "")).toBeNull();
    expect(await ensureFenceLanguage("pyhton")).toBe(false);
    expect(await ensureFenceLanguage("")).toBe(false);
  });

  it("语法包未加载时同样返回 null（不阻塞首帧，加载完再着色）", () => {
    // `scheme` 未被本文件其它用例加载过；同步调用必须落在降级分支上。
    expect(highlightCodeSpans("(define (f x) x)", "scheme")).toBeNull();
  });
});

describe("围栏代码块语言语法高亮（端到端管线）", () => {
  it("语言加载后嵌套解析产出 luna-tok-* 语义类", async () => {
    const desc = codeLanguageForFence("python");
    expect(desc?.name).toBe("Python");
    await desc!.load();

    const md = markdown({ base: markdownLanguage, codeLanguages: codeLanguageForFence });
    const doc = "```python\ndef f(x):\n    return 'hi'\n```\n";
    const tree = md.language.parser.parse(doc);
    const classes = new Set<string>();
    highlightTree(tree, lunaHighlightStyle, (_from, _to, cls) => {
      for (const c of cls.split(" ")) {
        classes.add(c);
      }
    });
    expect(classes.has("luna-tok-keyword")).toBe(true); // def / return
    expect(classes.has("luna-tok-string")).toBe(true); // 'hi'
  });

  it("未知语言名（拼写错误）诚实降级为纯文本，不产代码 token 类", () => {
    const md = markdown({ base: markdownLanguage, codeLanguages: codeLanguageForFence });
    const doc = "```pyhton\ndef f(x):\n    return 1\n```\n";
    const tree = md.language.parser.parse(doc);
    const classes = new Set<string>();
    highlightTree(tree, lunaHighlightStyle, (_from, _to, cls) => {
      for (const c of cls.split(" ")) {
        classes.add(c);
      }
    });
    expect(classes.has("luna-tok-keyword")).toBe(false);
  });

  it("主流 Markdown 围栏短别名归一后仍按需加载对应语言", async () => {
    const aliases = [
      ["py", "Python"],
      ["rs", "Rust"],
      ["kt", "Kotlin"],
      ["docker", "Dockerfile"],
      ["md", "Markdown"],
    ] as const;
    for (const [alias, expectedName] of aliases) {
      const desc = codeLanguageForFence(`${alias} linenos`);
      expect(desc?.name).toBe(expectedName);
      await desc!.load();
    }

    const md = markdown({ base: markdownLanguage, codeLanguages: codeLanguageForFence });
    const tree = md.language.parser.parse("```py\ndef f():\n  return 1\n```");
    const classes = new Set<string>();
    highlightTree(tree, lunaHighlightStyle, (_from, _to, cls) => {
      for (const item of cls.split(" ")) {
        classes.add(item);
      }
    });
    expect(classes.has("luna-tok-keyword")).toBe(true);
  });
});

/**
 * 列表项**正文**不得被按语法标记上色（2026-08-02 真机，用户截图一眼可见）。
 *
 * lezer-markdown 的 `tags.list` 挂在**整个 ListItem** 上——它说的是"这段文字在列表里"，
 * 不是"这是项目符号"。它曾被映射成 `.luna-md-list-mark`（accent 色 + 600 字重），于是
 * **每篇以要点为主的笔记，正文每一个字都被按项目符号上色加粗**；而 AI 蒸馏产物几乎全是
 * 要点列表——产品的核心产出面整片染色，却没有任何一条既有断言会红（没有测试看过"列表里的
 * 普通文字被涂成了什么"）。守护就立在这个空白上。
 */
describe("列表项正文的高亮归属（PRD-06-011）", () => {
  const md = markdown({ base: markdownLanguage });
  /** 取一段文本在高亮结果中被赋予的全部 class。 */
  function classesOf(doc: string, needle: string): string[] {
    const tree = md.language.parser.parse(doc);
    const at = doc.indexOf(needle);
    expect(at, `夹具里找不到 ${needle}`).toBeGreaterThanOrEqual(0);
    const out: string[] = [];
    highlightTree(tree, lunaHighlightStyle, (from, to, cls) => {
      if (from <= at && to >= at + needle.length) {
        out.push(...cls.split(" "));
      }
    });
    return out;
  }

  const doc = "- 普通正文 CTA 145\n- **要点**：正文\n\n段落 CTA 145\n";

  it("列表里的普通文字与段落里的普通文字得到同一种（无）处理", () => {
    expect(classesOf(doc, "普通正文 CTA 145")).toEqual([]);
    expect(classesOf(doc, "段落 CTA 145")).toEqual([]);
  });

  it("项目符号本身仍是语法标记（弱化色），加粗仍只属于 `**` 包住的那几个字", () => {
    expect(classesOf(doc, "-")).toContain("luna-md-mark");
    expect(classesOf(doc, "要点")).toContain("luna-md-strong");
  });

  it("反向验证：`tags.list` 一旦被映射，列表正文立刻被染上它（这就是那条缺陷）", () => {
    // 只映射这一条即可复现——正文被涂色与其余映射无关，是 `tags.list` 的覆盖面所致。
    const withListTag = HighlightStyle.define([{ tag: tags.list, class: "luna-md-list-mark" }]);
    const tree = md.language.parser.parse(doc);
    const at = doc.indexOf("普通正文 CTA 145");
    const hit: string[] = [];
    highlightTree(tree, withListTag, (from, to, cls) => {
      if (from <= at && to >= at + "普通正文 CTA 145".length) {
        hit.push(...cls.split(" "));
      }
    });
    expect(hit).toContain("luna-md-list-mark");
  });
});
