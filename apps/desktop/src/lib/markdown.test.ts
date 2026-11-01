import { describe, expect, it } from "vitest";
import {
  embedTargetsOf,
  inlineMarkdownToText,
  isSafeExternalUrl,
  parseChatMarkdown,
  parseTableAlign,
  type MdBlock,
  type MdInline,
} from "./markdown";

function inlineText(inlines: MdInline[]): string {
  return inlines
    .map((node) => {
      switch (node.kind) {
        case "break":
          return "\n";
        case "math":
          return node.tex;
        case "footnote":
          return "";
        case "text":
        case "code":
          return node.text;
        case "image":
          return node.alt;
        case "embed":
          return node.display ?? node.target;
        default:
          return inlineText(node.children);
      }
    })
    .join("");
}

describe("parseChatMarkdown", () => {
  it("keeps generated claim sources clickable while hiding only their machine markers", () => {
    const blocks = parseChatMarkdown(
      "<!-- luna:claim:019d1a08-7cf3-72d7-90fa-382ef03524ad -->\n" +
        "- 已完成原型（[会议](../Notes/会议%202026.md)）\n" +
        "<!-- /luna:claim -->\n",
    );
    expect(blocks).toHaveLength(1);
    const item = (blocks[0] as Extract<MdBlock, { kind: "list" }>).items[0];
    const paragraph = item.blocks[0] as Extract<MdBlock, { kind: "paragraph" }>;
    expect(paragraph.children.some((node) => node.kind === "link" && node.href === "../Notes/会议%202026.md")).toBe(true);
  });
  it("parses headings, paragraphs and inline styles", () => {
    const blocks = parseChatMarkdown("## Title\n\nSome **bold** and *em* and `code` here.");
    expect(blocks[0]).toMatchObject({ kind: "heading", level: 2 });
    expect(inlineText((blocks[0] as Extract<MdBlock, { kind: "heading" }>).children)).toBe(
      "Title",
    );
    const para = blocks[1] as Extract<MdBlock, { kind: "paragraph" }>;
    expect(para.kind).toBe("paragraph");
    expect(para.children.some((n) => n.kind === "strong")).toBe(true);
    expect(para.children.some((n) => n.kind === "em")).toBe(true);
    expect(para.children.some((n) => n.kind === "code" && n.text === "code")).toBe(true);
    expect(inlineText(para.children)).toBe("Some bold and em and code here.");
  });

  it("parses fenced code blocks with language info", () => {
    const blocks = parseChatMarkdown("```rust\nfn main() {}\n```");
    expect(blocks[0]).toEqual({ kind: "code_block", lang: "rust", code: "fn main() {}" });
  });

  it("keeps streaming half-open code fences renderable", () => {
    // 流式中途：fence 未闭合也不能崩、内容不能丢。
    const blocks = parseChatMarkdown("```python\nprint(1)\n");
    expect(blocks[0]).toMatchObject({ kind: "code_block", lang: "python" });
    expect((blocks[0] as Extract<MdBlock, { kind: "code_block" }>).code).toContain("print(1)");
  });

  it("parses nested lists, ordered lists and task items", () => {
    const blocks = parseChatMarkdown(
      "- outer\n  - inner\n- [x] done\n- [ ] todo\n\n1. first\n2. second",
    );
    const bullets = blocks[0] as Extract<MdBlock, { kind: "list" }>;
    expect(bullets.ordered).toBe(false);
    expect(bullets.items).toHaveLength(3);
    expect(bullets.items[0].blocks.some((b) => b.kind === "list")).toBe(true);
    expect(bullets.items[1].task).toBe("done");
    expect(bullets.items[2].task).toBe("todo");
    const ordered = blocks[1] as Extract<MdBlock, { kind: "list" }>;
    expect(ordered.ordered).toBe(true);
    expect(ordered.items).toHaveLength(2);
  });

  it("parses blockquotes, tables and horizontal rules", () => {
    const blocks = parseChatMarkdown("> quoted\n\n| A | B |\n| - | - |\n| 1 | 2 |\n\n---");
    expect(blocks[0].kind).toBe("blockquote");
    const table = blocks[1] as Extract<MdBlock, { kind: "table" }>;
    expect(table.header.map(inlineText)).toEqual(["A", "B"]);
    expect(table.rows[0].map(inlineText)).toEqual(["1", "2"]);
    expect(table.align).toEqual([null, null]);
    expect(blocks[2].kind).toBe("hr");
  });

  /**
   * 对齐此前**根本没被解析**：同一张表在编辑面右对齐、切到阅读面全左对齐
   * （TDD-15 §3.4a 的同一类缺陷）。判定与编辑面同一个 `parseTableAlign`。
   */
  it("reads per-column alignment from the GFM delimiter row", () => {
    const blocks = parseChatMarkdown("| A | B | C | D |\n| :-- | :-: | --: | --- |\n| 1 | 2 | 3 | 4 |");
    const table = blocks[0] as Extract<MdBlock, { kind: "table" }>;
    expect(table.align).toEqual(["left", "center", "right", null]);
  });

  /** 编辑面把单元格 `<br>` 显示成换行，阅读面与导出此前却原样露出 `<br>`（2026-10-03）。 */
  it("单元格里的 <br> 是换行，别的 HTML 仍按原文", () => {
    const blocks = parseChatMarkdown("| A |\n| - |\n| 一<br>二<BR />三 <b>粗</b> |");
    const table = blocks[0] as Extract<MdBlock, { kind: "table" }>;
    const cell = table.rows[0][0];
    expect(cell.filter((n) => n.kind === "break")).toHaveLength(2);
    expect(inlineText(cell)).toBe("一\n二\n三 <b>粗</b>");
  });

  it("parseTableAlign 认得无外围管道的分隔行（GFM 允许）", () => {
    expect(parseTableAlign(":-- | --:")).toEqual(["left", "right"]);
    expect(parseTableAlign("|---|")).toEqual([null]);
  });

  it("extracts link targets and emits images as dedicated nodes", () => {
    const blocks = parseChatMarkdown(
      "See [docs](https://example.com) and ![alt](assets/img.png)",
    );
    const para = blocks[0] as Extract<MdBlock, { kind: "paragraph" }>;
    const link = para.children.find(
      (n): n is Extract<MdInline, { kind: "link" }> => n.kind === "link",
    );
    expect(link?.href).toBe("https://example.com");
    expect(inlineText(link?.children ?? [])).toBe("docs");
    const image = para.children.find(
      (n): n is Extract<MdInline, { kind: "image" }> => n.kind === "image",
    );
    expect(image).toEqual({ kind: "image", src: "assets/img.png", alt: "alt", width: null });
  });

  it("renders raw html as plain text without losing content", () => {
    const blocks = parseChatMarkdown("<div>hi</div>");
    expect(blocks.length).toBeGreaterThan(0);
    const flattened = blocks
      .map((b) => (b.kind === "paragraph" ? inlineText(b.children) : ""))
      .join("");
    expect(flattened).toContain("<div>hi</div>");
  });

  it("unescapes backslash escapes", () => {
    const para = parseChatMarkdown("a \\* b")[0] as Extract<MdBlock, { kind: "paragraph" }>;
    expect(inlineText(para.children)).toBe("a * b");
  });
});

describe("isSafeExternalUrl", () => {
  it("allows only http and https", () => {
    expect(isSafeExternalUrl("https://example.com")).toBe(true);
    expect(isSafeExternalUrl("http://example.com")).toBe(true);
    expect(isSafeExternalUrl("file:///etc/passwd")).toBe(false);
    expect(isSafeExternalUrl("javascript:alert(1)")).toBe(false);
    expect(isSafeExternalUrl("notes://x")).toBe(false);
  });
});

describe("标注块（callout，PRD-06-011，Obsidian 对标）", () => {
  function firstBlock(text: string) {
    return parseChatMarkdown(text)[0];
  }

  it("引用块以 [!type] 开头即成标注块，标题与正文分离", () => {
    const block = firstBlock("> [!warning] 注意事项\n> 正文一\n> 正文二");
    expect(block).toMatchObject({ kind: "callout", tone: "warning", label: "WARNING" });
    const callout = block as Extract<MdBlock, { kind: "callout" }>;
    expect(inlineText(callout.title)).toBe("注意事项");
    expect(callout.blocks).toHaveLength(1);
  });

  it("无标题时标题为空（呈现层用类型标签兜底）", () => {
    const callout = firstBlock("> [!tip]\n> 只有正文") as Extract<MdBlock, { kind: "callout" }>;
    expect(callout.kind).toBe("callout");
    expect(callout.title).toHaveLength(0);
    expect(callout.tone).toBe("success");
  });

  it("折叠记号 `-` 落到 defaultFolded", () => {
    const callout = firstBlock("> [!note]- 收起\n> 正文") as Extract<MdBlock, { kind: "callout" }>;
    expect(callout.defaultFolded).toBe(true);
  });

  it("普通引用块仍是普通引用（不误判）", () => {
    expect(firstBlock("> 普通引用\n> 第二行").kind).toBe("blockquote");
  });

  it("标注正文保留块结构（列表/代码块不被压平）", () => {
    const callout = firstBlock("> [!info] 清单\n> - 甲\n> - 乙") as Extract<
      MdBlock,
      { kind: "callout" }
    >;
    expect(callout.blocks[0]?.kind).toBe("list");
  });
});

describe("inlineMarkdownToText（摘录压成可读一行）", () => {
  it("链接只留文字，转义过的目标不外泄", () => {
    // 真机现场：体检卡把这一行原样摆出来，用户读到的是给解析器看的语法。
    expect(
      inlineMarkdownToText("[对话 2026-07-29 2227 目前v · 笔记](对话%202026-07-29%20·%20笔记.md)"),
    ).toBe("对话 2026-07-29 2227 目前v · 笔记");
  });

  it("强调与行内代码只去记号、不吃内容", () => {
    expect(inlineMarkdownToText("**NextOps**：`num_ctx` 要与预算同一个数")).toBe(
      "NextOps：num_ctx 要与预算同一个数",
    );
  });

  it("纯文本原样返回（去两端空白）", () => {
    expect(inlineMarkdownToText("  就是一句话  ")).toBe("就是一句话");
  });
});

/**
 * 图片判定（`imageTargetOf`）——**两个渲染面共用的那一处**（TDD-15 §3.4a）。
 *
 * 此前编辑面走一条手写正则 `^!\[([^\]]*)\]\(([^)]+)\)$`：下面前四组在编辑面要么完全不
 * 渲染、要么把 `x.png "标题"` 整段当成 src 去解析附件（于是显示为破图）。判定改由语法树
 * 给出后，两面拿到同一个 `{alt, src}`——不可能再分叉，也不用再往正则上接分支。
 */
function imageOf(text: string): Extract<MdInline, { kind: "image" }> | null {
  const blocks = parseChatMarkdown(text);
  const para = blocks[0];
  if (para?.kind !== "paragraph") {
    return null;
  }
  return (
    para.children.find(
      (n): n is Extract<MdInline, { kind: "image" }> => n.kind === "image",
    ) ?? null
  );
}

/** 整段拍平成纯文本（反向条款用：不渲染成图时必须原样看得见）。 */
function paragraphText(text: string): string {
  const para = parseChatMarkdown(text)[0];
  return para?.kind === "paragraph" ? inlineText(para.children) : "";
}

describe("图片语法：两面共用一处判定（CommonMark 口径）", () => {
  it("带 title 的三种引号形式都认，且 title 不混进 src / alt", () => {
    for (const raw of [
      '![说明](assets/x.png "标题")',
      "![说明](assets/x.png '标题')",
      "![说明](assets/x.png (标题))",
    ]) {
      expect(imageOf(raw), raw).toEqual({ kind: "image", src: "assets/x.png", alt: "说明", width: null });
    }
  });

  it("尖括号包裹的路径脱去尖括号（含空格路径）", () => {
    expect(imageOf("![a](<有 空格.png>)")).toEqual({
      kind: "image",
      src: "有 空格.png",
      alt: "a",
      width: null,
    });
    expect(imageOf('![a](<有 空格.png> "标题")')).toEqual({
      kind: "image",
      src: "有 空格.png",
      alt: "a",
      width: null,
    });
  });

  it("路径里的转义括号还原成真字符", () => {
    expect(imageOf("![a](assets/图\\(1\\).png)")).toEqual({
      kind: "image",
      src: "assets/图(1).png",
      alt: "a",
      width: null,
    });
  });

  it("alt 里的 `]`、强调与行内代码只去语法记号、不吃内容", () => {
    expect(imageOf("![数组 a\\[0\\]](x.png)")?.alt).toBe("数组 a[0]");
    expect(imageOf("![很 *重要* 的图](x.png)")?.alt).toBe("很 重要 的图");
    expect(imageOf("![值 `null` 时](x.png)")?.alt).toBe("值 null 时");
  });

  it("目标区两侧的空白既不进 src 也不进 alt", () => {
    expect(imageOf("![a](  x.png  )")).toEqual({ kind: "image", src: "x.png", alt: "a", width: null });
  });

  it("图片不必独占整行（前后有文字照样认）", () => {
    expect(imageOf("前文 ![a](x.png) 后文")).toEqual({ kind: "image", src: "x.png", alt: "a", width: null });
  });

  // ── 反向条款 ───────────────────────────────────────────────────────────────
  it("`![[内嵌]]`（对标方案 D5）是嵌入节点不是图片：整段只有它时成块，夹在文字里是行内", () => {
    expect(imageOf("![[note.png]]")).toBeNull();
    expect(parseChatMarkdown("![[图.png|300]]")).toEqual([{ kind: "embed", target: "图.png", display: "300" }]);
    expect(parseChatMarkdown("见 ![[周会#结论]] 一节")).toEqual([
      {
        kind: "paragraph",
        children: [
          { kind: "text", text: "见 " },
          { kind: "embed", target: "周会#结论", display: null },
          { kind: "text", text: " 一节" },
        ],
      },
    ]);
    expect(embedTargetsOf("![[a]]\n\n```\n![[代码里]]\n```\n\n![[b|x]] ![[a]]")).toEqual(["a", "b"]);
    // 空的、跨行的不算嵌入，照原文。
    expect(paragraphText("![[]]")).toBe("![[]]");
  });

  it("没有行内目标的写法（引用式 / 空目标）同样退纯文本，不产出空 src 的图片", () => {
    for (const raw of ["![a][ref]", "![a]()"]) {
      expect(imageOf(raw), raw).toBeNull();
      expect(paragraphText(raw), raw).toBe(raw);
    }
  });

  it("普通链接不是图片（少一个 `!` 就不该就地渲染）", () => {
    expect(imageOf("[link](x.md)")).toBeNull();
    expect(imageOf("plain text")).toBeNull();
  });
});

describe("脚注（对标方案 D3）", () => {
  it("引用挂上定义的纯文本；定义是独立的块，配不上的引用 note 为 null", () => {
    const blocks = parseChatMarkdown("正文[^1]与[^x]。\n\n[^1]: 第一条 **说明**");
    const paragraph = blocks[0] as Extract<MdBlock, { kind: "paragraph" }>;
    expect(paragraph.children.filter((n) => n.kind === "footnote")).toEqual([
      { kind: "footnote", id: "1", note: "第一条 说明" },
      { kind: "footnote", id: "x", note: null },
    ]);
    const note = blocks[1] as Extract<MdBlock, { kind: "footnote" }>;
    expect(note.kind).toBe("footnote");
    expect(note.id).toBe("1");
    expect(inlineText(note.children)).toBe("第一条 说明");
  });
});


describe("图片宽度（Obsidian 写法，对标方案 D4）", () => {
  it("![说明|300] 拆出宽度，alt 不带后缀", () => {
    const blocks = parseChatMarkdown("![封面|300](assets/a.png)");
    const paragraph = blocks[0] as Extract<MdBlock, { kind: "paragraph" }>;
    expect(paragraph.children[0]).toEqual({ kind: "image", src: "assets/a.png", alt: "封面", width: 300 });
  });
});

describe("块标记 ^id（Obsidian）", () => {
  it("段落与列表项末尾的 ^id 不显示；正文中间的 ^ 照常", () => {
    expect(parseChatMarkdown("上线新版本 ^k1")).toEqual([{ kind: "paragraph", children: [{ kind: "text", text: "上线新版本" }] }]);
    const list = parseChatMarkdown("- 第一项 ^a-1\n- 第二项");
    expect(JSON.stringify(list)).not.toContain("^a-1");
    expect(parseChatMarkdown("2^10 是 1024")).toEqual([{ kind: "paragraph", children: [{ kind: "text", text: "2^10 是 1024" }] }]);
  });
});

describe("wiki 链接整体是原文（真机回归：阅读面显示成「[周会]」）", () => {
  it("[[目标]] 与 [[目标|显示]] 不被拆成方括号加空目标链接", () => {
    const blocks = parseChatMarkdown("参见 [[周会]] 和 [[Weekly#下周|下周计划]]，还有 [源笔记](../项目/周会.md)。");
    const json = JSON.stringify(blocks);
    expect(json).toContain('"text":"[[周会]]"');
    expect(json).toContain('"text":"[[Weekly#下周|下周计划]]"');
    expect(json).not.toContain('"href":""');
    expect(json).toContain('"href":"../项目/周会.md"');
  });
});
