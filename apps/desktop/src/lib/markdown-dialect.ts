/**
 * Lunarca 的 Markdown 方言：GFM 基线（`markdownLanguage`）+ CJK 友好的强调规则。
 *
 * 编辑器、阅读视图、大纲、表格单元格与导出都从这里取解析器——同一行字在哪个面上加不加粗，
 * 只能有一个答案（PRD-06-016）。
 *
 * **CJK 友好强调**（markdown-cjk-friendly 口径，GitLab 已采用）：CommonMark 的左右侧判定
 * 把全角标点当作"标点"，于是 `**测试。**文字`、`他说**“你好”**然后` 这类中文里最常见的
 * 写法加粗失败，而且是静默失败。规则只放宽一处：分隔符串**外侧**是中日韩文字时，
 * 内侧的标点不再否决它。GitHub 仍按 CommonMark 显示这类写法（用户 2026-10-03 已接受）。
 */

import { markdownLanguage } from "@codemirror/lang-markdown";
import { parser as baseParser, type DelimiterType, type MarkdownConfig, type MarkdownParser } from "@lezer/markdown";

/** 中日韩文字与全角符号（不含 `“”` 这类东亚宽度"不定"的通用标点）。 */
const CJK = /[\p{Script=Han}\p{Script=Hiragana}\p{Script=Katakana}\p{Script=Hangul}\u2E80-\u2FFF\u3000-\u303F\u3200-\u33FF\uFF00-\uFFEF]/u;
const PUNCTUATION = /[\p{S}\p{P}]/u;
const SPACE = /\s|^$/u;

const ASTERISK = 42;
const UNDERSCORE = 95;

/**
 * 内置强调用的分隔符类型（`@lezer/markdown` 没有导出）：在一次探测解析里从解析现场取出。
 * 拿同一个对象才能让配对与「一个还是两个星号」的判定沿用内置实现；取不到时返回 `null`，
 * 扩展退化为不生效（`markdown-dialect.test.ts` 会因此变红，而不是静默错渲染）。
 */
function captureEmphasisTypes(): { asterisk: DelimiterType; underscore: DelimiterType } | null {
  const found = new Map<number, DelimiterType>();
  const probe = baseParser.configure({
    parseInline: [
      {
        name: "LunaEmphasisProbe",
        after: "Emphasis",
        parse(cx) {
          const parts = (cx as unknown as { parts?: unknown[] }).parts ?? [];
          for (const part of parts) {
            if (part && typeof part === "object" && "type" in part && "from" in part) {
              const { type, from } = part as { type: DelimiterType; from: number };
              found.set(cx.char(from), type);
            }
          }
          return -1;
        },
      },
    ],
  });
  probe.parse("*a* _b_");
  const asterisk = found.get(ASTERISK);
  const underscore = found.get(UNDERSCORE);
  return asterisk && underscore && asterisk.resolve === "Emphasis" ? { asterisk, underscore } : null;
}

const EMPHASIS_TYPES = captureEmphasisTypes();

/**
 * 只在 CJK 规则与 CommonMark 结论不同的那一刻接手；其余一律交还内置解析（返回 -1），
 * 拉丁文本的行为逐字不变。
 */
export const CjkFriendlyEmphasis: MarkdownConfig = {
  parseInline: [
    {
      name: "CjkFriendlyEmphasis",
      before: "Emphasis",
      parse(cx, next, start) {
        if (!EMPHASIS_TYPES || (next !== ASTERISK && next !== UNDERSCORE)) return -1;
        let pos = start + 1;
        while (cx.char(pos) === next) pos++;
        const before = cx.slice(start - 1, start);
        const after = cx.slice(pos, pos + 1);
        const pBefore = PUNCTUATION.test(before);
        const pAfter = PUNCTUATION.test(after);
        const sBefore = SPACE.test(before);
        const sAfter = SPACE.test(after);
        const left = !sAfter && (!pAfter || sBefore || pBefore);
        const right = !sBefore && (!pBefore || sAfter || pAfter);
        const cjkLeft = !sAfter && (!pAfter || sBefore || pBefore || CJK.test(before));
        const cjkRight = !sBefore && (!pBefore || sAfter || pAfter || CJK.test(after));
        if (cjkLeft === left && cjkRight === right) return -1;
        const canOpen = cjkLeft && (next === ASTERISK || !cjkRight || pBefore);
        const canClose = cjkRight && (next === ASTERISK || !cjkLeft || pAfter);
        const type = next === ASTERISK ? EMPHASIS_TYPES.asterisk : EMPHASIS_TYPES.underscore;
        return cx.addDelimiter(type, start, pos, canOpen, canClose);
      },
    },
  ],
};

const DOLLAR = 36;
const BACKSLASH = 92;

/**
 * 公式（PRD-06-011 2026-10-03 修订，用户裁决提前到本轮）：行内 `$…$`、成块 `$$…$$`（GitHub /
 * Obsidian / Typora 同一写法）。行内按 Pandoc 的判定防误伤金额：开头的 `$` 后面不能是空白，
 * 收尾的 `$` 前面不能是空白、后面不能紧跟数字——「花了 $5 和 $10」不是公式。`\$` 是字面美元号。
 */
export const MathSyntax: MarkdownConfig = {
  defineNodes: [
    { name: "InlineMath" },
    { name: "InlineMathMark" },
    { name: "BlockMath", block: true },
    { name: "BlockMathMark" },
  ],
  parseInline: [
    {
      name: "InlineMath",
      after: "Escape",
      parse(cx, next, start) {
        if (next !== DOLLAR || cx.char(start + 1) === DOLLAR || /\s/u.test(cx.slice(start + 1, start + 2))) return -1;
        for (let pos = start + 1; pos < cx.end; pos++) {
          const ch = cx.char(pos);
          if (ch === BACKSLASH) {
            pos++;
            continue;
          }
          if (ch !== DOLLAR) continue;
          if (pos === start + 1 || /\s/u.test(cx.slice(pos - 1, pos)) || /\d/u.test(cx.slice(pos + 1, pos + 2))) return -1;
          return cx.addElement(
            cx.elt("InlineMath", start, pos + 1, [cx.elt("InlineMathMark", start, start + 1), cx.elt("InlineMathMark", pos, pos + 1)]),
          );
        }
        return -1;
      },
    },
  ],
  parseBlock: [
    {
      name: "BlockMath",
      before: "FencedCode",
      parse(cx, line) {
        if (!line.text.startsWith("$$", line.pos) || line.indent - line.baseIndent >= 4) return false;
        const from = cx.lineStart + line.pos;
        const marks = [cx.elt("BlockMathMark", from, from + 2)];
        const rest = line.text.slice(line.pos + 2).trimEnd();
        if (rest.length >= 2 && rest.endsWith("$$")) {
          // 单行 `$$ x $$`。
          const end = cx.lineStart + line.pos + 2 + rest.length;
          marks.push(cx.elt("BlockMathMark", end - 2, end));
          cx.nextLine();
          cx.addElement(cx.elt("BlockMath", from, end, marks));
          return true;
        }
        let end = cx.lineStart + line.text.length;
        while (cx.nextLine()) {
          end = cx.lineStart + line.text.length;
          const trimmed = line.text.trimEnd();
          if (trimmed.endsWith("$$")) {
            marks.push(cx.elt("BlockMathMark", cx.lineStart + trimmed.length - 2, cx.lineStart + trimmed.length));
            cx.nextLine();
            break;
          }
        }
        cx.addElement(cx.elt("BlockMath", from, end, marks));
        return true;
      },
      endLeaf: (_cx, line) => line.text.startsWith("$$", line.pos),
    },
  ],
};

const FOOTNOTE_DEFINITION = /^\[\^([^\]\s]+)\]:[ \t]?/u;

/**
 * 脚注（对标方案 D3；GitHub / Obsidian / Typora 同一写法）：正文里 `[^id]` 引用，另起一段
 * `[^id]: 内容` 定义（后续非空行续写）。引用与定义按 id 配对，配不上的引用照原样显示。
 */
export const FootnoteSyntax: MarkdownConfig = {
  defineNodes: [
    { name: "FootnoteRef" },
    { name: "FootnoteMark" },
    { name: "FootnoteLabel" },
    { name: "FootnoteDefinition", block: true },
  ],
  parseInline: [
    {
      name: "FootnoteRef",
      before: "Link",
      parse(cx, next, start) {
        if (next !== 91 /* [ */ || cx.char(start + 1) !== 94 /* ^ */) return -1;
        let end = start + 2;
        while (end < cx.end && cx.char(end) !== 93 /* ] */ && !/\s/u.test(cx.slice(end, end + 1))) end++;
        if (end === start + 2 || cx.char(end) !== 93) return -1;
        return cx.addElement(
          cx.elt("FootnoteRef", start, end + 1, [
            cx.elt("FootnoteMark", start, start + 2),
            cx.elt("FootnoteLabel", start + 2, end),
            cx.elt("FootnoteMark", end, end + 1),
          ]),
        );
      },
    },
  ],
  parseBlock: [
    {
      name: "FootnoteDefinition",
      before: "LinkReference",
      parse(cx, line) {
        const match = FOOTNOTE_DEFINITION.exec(line.text.slice(line.pos));
        if (!match || line.indent - line.baseIndent >= 4) return false;
        const from = cx.lineStart + line.pos;
        const labelFrom = from + 2;
        const contentFrom = from + match[0].length;
        const parts = [line.text.slice(line.pos + match[0].length)];
        let end = cx.lineStart + line.text.length;
        while (cx.nextLine() && line.text.trim() !== "" && !FOOTNOTE_DEFINITION.test(line.text.slice(line.pos))) {
          parts.push(line.text);
          end = cx.lineStart + line.text.length;
        }
        cx.addElement(
          cx.elt("FootnoteDefinition", from, end, [
            cx.elt("FootnoteMark", from, labelFrom),
            cx.elt("FootnoteLabel", labelFrom, labelFrom + match[1].length),
            cx.elt("FootnoteMark", labelFrom + match[1].length, contentFrom),
            ...cx.parser.parseInline(parts.join("\n"), contentFrom),
          ]),
        );
        return true;
      },
    },
  ],
};

/**
 * Obsidian 嵌入 `![[目标]]` / `![[目标|300]]`（对标方案 D5）：同一行内到 `]]` 为止，里面不能是空的。
 * 指向什么（图片 / 笔记 / 一节）由 Rust 解析（`vault_resolve_embeds`），语法层只认出这一段。
 */
export const WikiEmbedSyntax: MarkdownConfig = {
  defineNodes: [{ name: "WikiEmbed" }, { name: "WikiEmbedMark" }, { name: "WikiEmbedTarget" }],
  parseInline: [
    {
      name: "WikiEmbed",
      before: "Image",
      parse(cx, next, start) {
        if (next !== 33 /* ! */ || cx.char(start + 1) !== 91 || cx.char(start + 2) !== 91) return -1;
        const close = cx.slice(start + 3, cx.end).indexOf("]]");
        if (close <= 0) return -1;
        const inner = cx.slice(start + 3, start + 3 + close);
        if (inner.includes("\n") || inner.includes("[") || inner.trim() === "") return -1;
        const end = start + 3 + close + 2;
        return cx.addElement(
          cx.elt("WikiEmbed", start, end, [
            cx.elt("WikiEmbedMark", start, start + 3),
            cx.elt("WikiEmbedTarget", start + 3, end - 2),
            cx.elt("WikiEmbedMark", end - 2, end),
          ]),
        );
      },
    },
  ],
};

/**
 * wiki 链接 `[[目标]]` / `[[目标|显示]]` 是一个整体：不加这条，lezer 不知道引用式链接有没有定义，会把
 * 里面的 `[目标]` 当成一条空目标的 Markdown 链接，外面剩两个方括号——阅读面显示成「[周会]」、点了
 * 也不跳（真机 2026-10-04）。语法层只认出这一段；着色、跳转、补全仍按原文由 `wiki-link.ts` 处理。
 */
export const WikiLinkSyntax: MarkdownConfig = {
  defineNodes: [{ name: "WikiLink" }],
  parseInline: [
    {
      name: "WikiLink",
      before: "Link",
      parse(cx, next, start) {
        if (next !== 91 /* [ */ || cx.char(start + 1) !== 91) return -1;
        const close = cx.slice(start + 2, cx.end).indexOf("]]");
        if (close <= 0) return -1;
        const inner = cx.slice(start + 2, start + 2 + close);
        if (inner.includes("\n") || inner.includes("[") || inner.trim() === "") return -1;
        return cx.addElement(cx.elt("WikiLink", start, start + 2 + close + 2));
      },
    },
  ],
};

/** `![[目标|显示]]` 里的目标与 `|` 之后（图片宽度或显示文字）。 */
export function splitEmbed(inner: string): { target: string; display: string | null } {
  const bar = inner.indexOf("|");
  return bar < 0
    ? { target: inner.trim(), display: null }
    : { target: inner.slice(0, bar).trim(), display: inner.slice(bar + 1).trim() };
}

/** 编辑器 `markdown({ extensions })` 用的方言扩展。 */
export const LUNA_MARKDOWN_EXTENSIONS = [CjkFriendlyEmphasis, MathSyntax, FootnoteSyntax, WikiEmbedSyntax, WikiLinkSyntax];

/** 块解析器拿得到的整篇文本（`BlockContext` 在运行时带着它，类型声明里没有公开）。 */
interface BlockInput {
  length: number;
  read(from: number, to: number): string;
}

/**
 * 文首 frontmatter（`---` … `---`）整段是一个块，不按 Markdown 解析（ADR-139 真机修订）：否则 YAML 注释
 * `# 说明` 成了一级标题、`- 项` 成了列表、闭合的 `---` 成了分隔线——源码模式是编辑 YAML 的唯一入口，
 * 它不能长成那样。判据与编辑器 `getFrontmatterRange`、Rust `find_frontmatter` 同一口径：第一行恰为
 * `---`，后面要有闭合的 `---` 行；没闭合就不是 frontmatter（照常按 Markdown 解析，一个字都不吞）。
 *
 * 只给编辑器用（`EDITOR_MARKDOWN_EXTENSIONS`）：阅读面先把 frontmatter 拆走再解析，对话里没有
 * 「文首 frontmatter」这回事——加进共用解析器反而可能吞掉 AI 回复开头的分隔线。
 */
export const FrontmatterSyntax: MarkdownConfig = {
  defineNodes: [{ name: "Frontmatter", block: true }],
  parseBlock: [
    {
      name: "Frontmatter",
      before: "HorizontalRule",
      parse(cx, line) {
        if (cx.lineStart !== 0 || line.text.replace(/\r$/u, "") !== "---") return false;
        const input = (cx as unknown as { input?: BlockInput }).input;
        if (!input) return false;
        const text = input.read(0, input.length);
        const close = /\n---\r?(?:\n|$)/u.exec(text);
        if (!close) return false;
        const end = close.index + 1 + 3;
        while (cx.lineStart + line.text.length < end && cx.nextLine()) {
          // 一直走到闭合的 `---` 那一行。
        }
        cx.nextLine();
        cx.addElement(cx.elt("Frontmatter", 0, end));
        return true;
      },
    },
  ],
};

/** 编辑器用的方言：共用的几条加上文首 frontmatter。 */
export const EDITOR_MARKDOWN_EXTENSIONS = [...LUNA_MARKDOWN_EXTENSIONS, FrontmatterSyntax];

/** 公式节点里的 TeX 源码（去掉两侧的 `$` / `$$`）。 */
export function mathSource(text: string, block: boolean): string {
  const fence = block ? 2 : 1;
  const trimmed = block ? text.trim() : text;
  return trimmed.slice(fence, trimmed.endsWith(block ? "$$" : "$") && trimmed.length >= fence * 2 ? -fence : undefined).trim();
}

/** 阅读视图、大纲、表格单元格等「解析一段字符串」的场合用的解析器（与编辑器同一方言）。 */
export const lunaMarkdownParser: MarkdownParser = (markdownLanguage.parser as MarkdownParser).configure(
  LUNA_MARKDOWN_EXTENSIONS,
);
