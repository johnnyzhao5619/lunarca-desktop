/// <reference types="vite/client" />
// 笔记正文（列表以外）的**两面同源**守护（PRD-06-011/016/017；纪律见 TDD-15 §3.4a）。
//
// 列表那一份在 `list-rendering-parity.test.ts`；这一份管代码高亮 / 表格对齐 / 引用块 /
// 行内代码 / 链接 / 标题行高。每一条的现场都是同一种形状：两个渲染面各自都规规矩矩用了
// Token，用的却是**不同的** Token——单面看全合规，⌘E 切一次模式就换个样子。
//
// 本文件只断言"两面消费同一处定义"，不去比对两份 CSS 的数值：逐条对齐数值不是修复，
// 那只是把分叉推迟到下一次改动（§3.4a 的原话）。每组都配反向条款：任一面再写死一套
// 自己的值就红。

import { describe, expect, it } from "vitest";

function raw(glob: Record<string, unknown>): string {
  return Object.values(glob)[0] as string;
}

const READING_CSS = raw(
  import.meta.glob("../../../../../packages/styles/src/markdown.css", {
    eager: true,
    import: "default",
    query: "?raw",
  }),
);

/** 阅读视图自己的版式（文档级级距、图片上限……），桌面与网页共用（`@lunarca/styles/reading.css`）。 */
const READING_VIEW_CSS = raw(
  import.meta.glob("../../../../../packages/styles/src/reading.css", { eager: true, import: "default", query: "?raw" }),
);

/** 编辑面（共享编辑器的 CM6 扩展）的样式：桌面与网页共用（`@lunarca/styles/editor.css`）。 */
const EDITOR_CSS = raw(
  import.meta.glob("../../../../../packages/styles/src/editor.css", { eager: true, import: "default", query: "?raw" }),
);

/** 桌面编辑器外壳与桌面专属部件（页头、AI 面板、版本历史……）的样式。 */
const DESKTOP_EDITOR_CSS = raw(
  import.meta.glob("./editor-core.css", { eager: true, import: "default", query: "?raw" }),
);

/** 桌面的编辑器宿主端口：编辑面的复制按钮从这里的 `markdown` 取，与阅读面同一份。 */
const EDITOR_HOST = raw(
  import.meta.glob("./editor-host.ts", { eager: true, import: "default", query: "?raw" }),
);

const TOKENS_CSS = raw(
  import.meta.glob("../../../../../packages/styles/src/tokens/base.css", {
    eager: true,
    import: "default",
    query: "?raw",
  }),
);

const READING_TSX = raw(
  import.meta.glob("../../../../../packages/editor-core/src/render/MarkdownContent.tsx", {
    eager: true,
    import: "default",
    query: "?raw",
  }),
);

const COPY_BUTTON_TSX = raw(
  import.meta.glob("../../components/markdown/CodeCopyButton.tsx", {
    eager: true,
    import: "default",
    query: "?raw",
  }),
);

const MARKDOWN_HOST = raw(
  import.meta.glob("../../components/markdown/markdown-host.ts", {
    eager: true,
    import: "default",
    query: "?raw",
  }),
);

const CODE_BLOCK_LINES = raw(
  import.meta.glob("../../../../../packages/editor-core/src/editor/code-block-lines.ts", {
    eager: true,
    import: "default",
    query: "?raw",
  }),
);

const SHARED_MARKDOWN = raw(
  import.meta.glob("../../../../../packages/editor-core/src/markdown/markdown.ts", {
    eager: true,
    import: "default",
    query: "?raw",
  }),
);

const TABLE_MODEL = raw(
  import.meta.glob("../../../../../packages/editor-core/src/editor/table-model.ts", { eager: true, import: "default", query: "?raw" }),
);

const LIVE_PREVIEW = raw(
  import.meta.glob("../../../../../packages/editor-core/src/editor/live-preview.ts", { eager: true, import: "default", query: "?raw" }),
);

const IMAGE_EMBED = raw(
  import.meta.glob("../../../../../packages/editor-core/src/editor/image-embed.ts", { eager: true, import: "default", query: "?raw" }),
);

/** 剥掉注释：注释里提到某个写法不算真的在用它（CSS）。 */
function code(css: string): string {
  return css.replace(/\/\*[\s\S]*?\*\//gu, " ");
}

/** 同上（TS/TSX，块注释 + 行注释）。模块抬头写着「无 innerHTML」，那是承诺不是用法。 */
function js(src: string): string {
  return src.replace(/\/\*[\s\S]*?\*\//gu, " ").replace(/^[ \t]*\/\/.*$/gmu, " ");
}

/** 取某条规则的声明体（选择器按字面转义，避免手拼反斜杠）。 */
function ruleBody(css: string, selector: string): string | null {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/gu, "\\$&");
  return new RegExp(`${escaped}\\s*\\{([^}]*)\\}`, "u").exec(css)?.[1] ?? null;
}

/** 图片显示上限的两个消费点：编辑面就地渲染 / 阅读面 img。 */
const IMAGE_RULES = [".luna-md-image-embed .luna-md-image", ".luna-reading__image"];

/** 两面都必须消费的正文 Token（引用 / 行内代码 / 链接 / 标题行高 / 图片上限）。 */
const SHARED_TOKENS = [
  "--editor-quote-border-width",
  "--editor-quote-border-color",
  "--editor-quote-surface",
  "--editor-quote-text",
  "--editor-quote-pad-inline",
  "--editor-code-inline-surface",
  "--editor-code-inline-color",
  "--editor-code-inline-font-size",
  "--editor-link-color",
  "--editor-heading-line-height",
  "--editor-image-max-inline",
  "--editor-image-max-block",
];

describe("正文渲染：两面同源（列表以外）", () => {
  it("扫描面非空（glob 失效时本用例先红）", () => {
    for (const [name, text] of Object.entries({
      READING_CSS,
      EDITOR_CSS,
      DESKTOP_EDITOR_CSS,
      EDITOR_HOST,
      TOKENS_CSS,
      READING_TSX,
      COPY_BUTTON_TSX,
      MARKDOWN_HOST,
      CODE_BLOCK_LINES,
      SHARED_MARKDOWN,
      TABLE_MODEL,
      LIVE_PREVIEW,
      IMAGE_EMBED,
    })) {
      expect(typeof text, `${name} 读不到`).toBe("string");
      expect(text.length, `${name} 是空的`).toBeGreaterThan(200);
    }
  });

  it("每个共享 Token 都只在 Token 表里定义一次", () => {
    for (const token of SHARED_TOKENS) {
      const defs = [...TOKENS_CSS.matchAll(new RegExp(`^\\s*${token}\\s*:`, "gm"))];
      expect(defs.length, `${token} 的定义点数量`).toBe(1);
    }
  });

  // ── A. 代码高亮 ────────────────────────────────────────────────────────────
  describe("围栏代码高亮", () => {
    it("阅读面接上了编辑面同一条高亮链路（同一份 HighlightStyle + 同一个语言目录）", () => {
      // 阅读面在共享渲染包里，与编辑面消费同一个 `code-highlight` 模块（包内相对引用）。
      expect(READING_TSX).toContain('from "../highlight/code-highlight"');
      expect(READING_TSX).toContain("highlightCodeSpans");
      expect(READING_TSX).toContain("ensureFenceLanguage");
    });

    it("阅读面着色产出的是 `.luna-tok-*` 语义类，颜色单点在 --syntax-*", () => {
      // class 由 `lunaHighlightStyle` 给，CSS 只有一处定义：共享样式包的 `markdown.css`（桌面与网页、阅读面
      // 与编辑面都引它）。
      const tokenRules = [...code(READING_CSS).matchAll(/\.luna-tok-([a-z]+)\s*\{([^}]*)\}/g)];
      expect(tokenRules.length).toBeGreaterThan(4);
      for (const [, name, body] of tokenRules) {
        expect(body, `.luna-tok-${name} 应当只取 --syntax-* 色`).toContain("var(--syntax-");
      }
      // 编辑面绝不另起一套代码配色（共享编辑面与桌面编辑器外壳都不许）。
      expect(code(EDITOR_CSS)).not.toMatch(/\.luna-tok-/);
      expect(code(DESKTOP_EDITOR_CSS)).not.toMatch(/\.luna-tok-/);
    });

    it("阅读面渲染代码零 innerHTML（着色不得走字符串拼 HTML）", () => {
      expect(js(READING_TSX)).not.toContain("dangerouslySetInnerHTML");
      expect(js(READING_TSX)).not.toContain("innerHTML");
    });

    it("复制按钮是**同一个组件**，两面各自挂载（配色类名也只有一处）", () => {
      // 两面都从宿主端口取按钮：阅读面取 `MarkdownHost.CodeCopyButton`，编辑面取编辑器端口里的同一份
      // `markdown`；桌面的编辑器端口把阅读面那份端口原样交过去，于是两面挂的是同一个组件。
      expect(READING_TSX).toContain("<CodeCopyButton");
      expect(READING_TSX).toContain("const { CodeCopyButton } = host;");
      expect(MARKDOWN_HOST).toContain('import { CodeCopyButton } from "./CodeCopyButton";');
      expect(js(MARKDOWN_HOST)).toMatch(/^\s*CodeCopyButton,$/m);
      expect(js(CODE_BLOCK_LINES)).toContain("host.markdown.CodeCopyButton");
      expect(js(EDITOR_HOST)).toMatch(/^\s*markdown: desktopMarkdownHost,$/m);
      // 反向：编辑面不许绕过端口直接引一颗按钮组件。
      expect(js(CODE_BLOCK_LINES)).not.toMatch(/import[^;]*CodeCopyButton/);
      expect(COPY_BUTTON_TSX).toContain('className="luna-md-code-copy-btn"');
      expect(code(READING_CSS)).toContain(".luna-md-code-copy-btn");
      // 反向：编辑面不许自绘一颗按钮外壳（那会绕过 IconButton 的必填 label）。
      expect(CODE_BLOCK_LINES).not.toContain("luna-icon-btn");
    });

    it("「哪几行算这块代码」两面同一个函数（否则两处复制出的内容会差一行围栏）", () => {
      expect(SHARED_MARKDOWN).toContain("export function codeBlockContentOf");
      expect(CODE_BLOCK_LINES).toContain("codeBlockContentOf(");
      expect(READING_TSX).not.toMatch(/```|CodeText/); // 阅读面不自己剥围栏
    });
  });

  // ── B. 表格对齐 ────────────────────────────────────────────────────────────
  describe("表格列对齐", () => {
    it("对齐判定单点在 lib，两面都调它", () => {
      expect(SHARED_MARKDOWN).toContain("export function parseTableAlign");
      expect(TABLE_MODEL).toContain("parseTableAlign(");
      expect(TABLE_MODEL).toContain('from "../markdown/markdown"');
      // 反向：编辑面不许再留一份自己的分隔行解析。
      expect(TABLE_MODEL).not.toContain("function parseAlignRow");
    });

    it("阅读面真的把对齐渲染出来（此前 MdBlock 连 align 字段都没有）", () => {
      expect(SHARED_MARKDOWN).toMatch(/kind:\s*"table";[^|]*align:\s*TableAlign\[\]/);
      // 表头与数据格**各一处**：只给 `<th>` 加对齐、数据格全左靠，是同一条缺陷的一半。
      expect([...js(READING_TSX).matchAll(/data-align=\{alignAttr\(/g)]).toHaveLength(2);
    });

    it("两面的 `data-align` 词汇与取值一字不差", () => {
      for (const face of [code(READING_CSS), code(EDITOR_CSS)]) {
        expect(face).toMatch(/\[data-align="center"\][^{]*\{[^}]*text-align:\s*center/);
        // `end` 而不是 `right`：RTL 零成本（§3.6 逻辑属性）。
        expect(face).toMatch(/\[data-align="right"\][^{]*\{[^}]*text-align:\s*end/);
      }
    });
  });

  // ── C. 引用块 ─────────────────────────────────────────────────────────────
  describe("引用块", () => {
    it("边条 / 底色 / 文字色三项两面同 Token", () => {
      for (const token of [
        "--editor-quote-border-width",
        "--editor-quote-border-color",
        "--editor-quote-surface",
        "--editor-quote-text",
      ]) {
        expect(code(READING_CSS), `阅读面未消费 ${token}`).toContain(`var(${token})`);
        expect(code(EDITOR_CSS), `编辑面未消费 ${token}`).toContain(`var(${token})`);
      }
    });

    it("反向：阅读面不再自带一套边条/文字色", () => {
      const quote = /\.luna-markdown blockquote\s*\{([^}]*)\}/.exec(code(READING_CSS));
      expect(quote, "阅读面的 blockquote 规则找不到了").not.toBeNull();
      expect(quote?.[1]).not.toContain("--color-border-subtle");
      expect(quote?.[1]).not.toContain("--color-text-muted");
    });

    /**
     * 裁决：**两面都不斜体**。GitHub / Typora / Obsidian 均不斜体；中日文没有真斜体
     * 字形，浏览器只能合成倾斜，长引用整段更难读。视觉边界由边条 + 底色承担。
     */
    it("两面都不斜体（编辑面此前 `font-style: italic`）", () => {
      const editorQuote = /\.luna-md-quote\s*\{([^}]*)\}/.exec(code(EDITOR_CSS));
      expect(editorQuote, "编辑面的引用文字规则找不到了").not.toBeNull();
      expect(editorQuote?.[1]).not.toContain("italic");
      const readingQuote = /\.luna-markdown blockquote\s*\{([^}]*)\}/.exec(code(READING_CSS));
      expect(readingQuote?.[1]).not.toContain("italic");
    });
  });

  // ── D. 行内代码与链接 ──────────────────────────────────────────────────────
  describe("行内代码", () => {
    it("底色 / 前景 / 字号三项两面同 Token", () => {
      for (const token of [
        "--editor-code-inline-surface",
        "--editor-code-inline-color",
        "--editor-code-inline-font-size",
      ]) {
        expect(code(READING_CSS), `阅读面未消费 ${token}`).toContain(`var(${token})`);
        expect(code(EDITOR_CSS), `编辑面未消费 ${token}`).toContain(`var(${token})`);
      }
    });

    it("反向：两面都不再各写一个底色 / 一个字号", () => {
      const reading = /\.luna-markdown code\s*\{([^}]*)\}/.exec(code(READING_CSS));
      expect(reading, "阅读面的行内代码规则找不到了").not.toBeNull();
      expect(reading?.[1]).not.toContain("--color-bg-muted");
      expect(reading?.[1]).not.toMatch(/font-size:\s*0?\.\d+em/);
      const editor = /\.luna-md-code\s*\{([^}]*)\}/.exec(code(EDITOR_CSS));
      expect(editor, "编辑面的行内代码规则找不到了").not.toBeNull();
      expect(editor?.[1]).not.toContain("var(--font-size-");
    });
  });

  describe("正文链接", () => {
    /**
     * 裁决：**常态只有颜色、不加下划线**，与仓库既有内容链接一致（wiki-link、行内标签、
     * 看板链接都是「常态无下划线 / hover 加下划线」）。hover 下划线只发给真的能点的
     * 那一面——编辑面显示的是源码文字，给它悬停反馈是骗人。
     */
    it("颜色两面同 Token", () => {
      expect(code(READING_CSS)).toContain("var(--editor-link-color)");
      expect(code(EDITOR_CSS)).toContain("var(--editor-link-color)");
    });

    it("常态两面都不带下划线", () => {
      const reading = /\.luna-markdown a\s*\{([^}]*)\}/.exec(code(READING_CSS));
      expect(reading, "阅读面的链接规则找不到了").not.toBeNull();
      expect(reading?.[1]).toContain("text-decoration: none");
      for (const selector of [".luna-md-link", ".luna-md-url", ".luna-table-preview__link"]) {
        const block = new RegExp(
          `\\${selector}\\s*\\{([^}]*)\\}`.replace("\\.", "\\."),
        ).exec(code(EDITOR_CSS));
        expect(block, `编辑面的 ${selector} 规则找不到了`).not.toBeNull();
        expect(block?.[1], `${selector} 常态不该有下划线`).not.toContain("underline");
      }
    });

    it("下划线只在阅读面的 hover 上（那一面的链接才真的能点）", () => {
      expect(code(READING_CSS)).toMatch(
        /\.luna-markdown a:hover\s*\{[^}]*text-decoration:\s*underline/,
      );
    });
  });

  // ── F. 标题行高 ───────────────────────────────────────────────────────────
  it("标题行高两面同 Token（此前阅读面 1.5、编辑面继承正文的 1.7）", () => {
    expect(code(EDITOR_CSS)).toMatch(
      /\.luna-md-h1[\s\S]{0,200}line-height:\s*var\(--editor-heading-line-height\)/,
    );
    expect(code(READING_VIEW_CSS)).toMatch(
      /\.luna-reading \.luna-markdown h1[\s\S]{0,300}line-height:\s*var\(--editor-heading-line-height\)/,
    );
  });

  // ── G. 图片 ───────────────────────────────────────────────────────────────
  describe("图片", () => {
    /**
     * §3.4a「同源要一路往回收到最上游那一处」：分叉不在 CSS，而在**判定**——阅读面读
     * lezer 树、编辑面读一条手写正则，于是 `![a](x.png "标题")` / `![a](<带 空格.png>)` /
     * 转义括号 / alt 带 `]` 在编辑面全军覆没。正则再长也追不上 CommonMark，而语法树
     * 本来就在手上（Live Preview 遍历的就是它）。
     */
    it("「这是不是一张图、alt/src 是什么」两面同一个函数，且判定读语法树", () => {
      expect(SHARED_MARKDOWN).toContain("export function imageTargetOf");
      expect(LIVE_PREVIEW).toContain("imageTargetOf(");
      expect(LIVE_PREVIEW).toContain('from "../markdown/markdown"');
      // 判定的入参是语法节点（不是一段被切出来的字符串）。
      expect(SHARED_MARKDOWN).toMatch(/imageTargetOf\(\s*image: SyntaxNode/);
    });

    it("反向：编辑面不许再留一条自己的图片正则", () => {
      expect(IMAGE_EMBED).not.toContain("parseImageMarkdown");
      expect(js(IMAGE_EMBED)).not.toMatch(/!\\\[/);
      expect(js(LIVE_PREVIEW)).not.toMatch(/!\\\[/);
    });

    it("显示上限两面同 Token（此前编辑面 720/480 裸像素、阅读面纵向不封顶）", () => {
      for (const selector of IMAGE_RULES) {
        const body = ruleBody(code(`${EDITOR_CSS}\n${READING_VIEW_CSS}`), selector);
        expect(body, `${selector} 的规则找不到了`).not.toBeNull();
        for (const token of ["--editor-image-max-inline", "--editor-image-max-block"]) {
          expect(body, `${selector} 未消费 ${token}`).toContain(`var(${token})`);
        }
      }
    });

    it("反向：两面的图片规则里不再出现裸像素上限", () => {
      for (const selector of IMAGE_RULES) {
        expect(ruleBody(code(`${EDITOR_CSS}\n${READING_VIEW_CSS}`), selector), `${selector} 里还有裸像素`).not.toMatch(
          /\d+px/,
        );
      }
    });

    it("远程图片维持不加载（离线纪律，刻意行为——本条防「顺手打开」）", () => {
      expect(IMAGE_EMBED).toContain("export function isLocalAssetSrc");
      expect(LIVE_PREVIEW).toContain("isLocalAssetSrc(");
    });
  });

  // ── H. 专注模式淡化 ────────────────────────────────────────────────────────
  it("专注模式的淡化程度走 Token（此前是裸 0.35）", () => {
    const focus = /\[data-focus\][^{]*\{([^}]*)\}/.exec(code(EDITOR_CSS));
    expect(focus, "专注模式规则找不到了").not.toBeNull();
    expect(focus?.[1]).toContain("var(--editor-focus-dim-opacity)");
    // 反向：不许再写回字面量。
    expect(focus?.[1]).not.toMatch(/opacity:\s*[\d.]+\s*;/);
    expect([...TOKENS_CSS.matchAll(/^\s*--editor-focus-dim-opacity\s*:/gm)]).toHaveLength(1);
  });
});
