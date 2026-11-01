/**
 * 笔记 → 自包含 HTML（PRD-11-011 / TDD-10 §1.2）。
 *
 * 「导出的样子 = 编辑器里看到的样子」只有一条路：用**同一份**阅读视图渲染（仓库里只有
 * `components/markdown` 这一份 Markdown→渲染实现，Rust 侧不另写解析器）。这里做两件事：
 * 1. 把 `ReadingView` 静态渲染成 HTML 片段（`staticExport` 形态：图片保留笔记相对 src，
 *    wiki-link / 标签退成文本）；
 * 2. 从**当前页面的样式表**收集阅读视图用到的规则与它们引用的设计 Token 的**已解析值**
 *    ——导出物因此跟随用户此刻的主题（明暗 / 配色族），而不是另一套写死的样式。
 * 图片内联、拼页、写盘在 Rust（`export_note_html`）。
 */

import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { commands } from "../../ipc/bindings";
import { i18n } from "../../i18n/i18n";
import { ReadingView, resolveEmbedsFor } from "./ReadingView";
import type { EmbedHitDto } from "../../ipc/bindings";
import { prepareRichContent } from "../../lib/rich-content";

/** 阅读视图会用到的选择器片段：命中任一即收进导出样式。 */
const EXPORT_SELECTOR_HINTS = [
  ".luna-reading",
  ".luna-markdown",
  ".luna-wiki-link",
  ".luna-inline-tag",
  ".luna-code-block",
  ".luna-callout",
  ".luna-export",
  // 公式（KaTeX 自己的类）与图表：导出物里也要排好版。KaTeX 的数学字体是应用内资源，
  // 导出的独立 HTML 里取不到，浏览器按 MathML / 系统字体显示，字形会朴素一些。
  ".luna-math",
  ".katex",
  ".luna-mermaid",
  // 嵌入 `![[笔记]]` 展开进导出物时的卡片样式（对标方案 D5）。
  ".luna-embed",
];

/** 导出页面的底座：正文容器、页边距、打印分页。全部经 Token 引用，值随 `:root` 一起导出。 */
const EXPORT_BASE_CSS = `
.luna-export { margin: 0; padding: 0; color: var(--color-text-primary); background: var(--color-bg-canvas); font-family: var(--font-family-base, system-ui, sans-serif); line-height: 1.6; }
.luna-export__page { max-width: 72ch; margin: 0 auto; padding: 2.5rem 1.5rem; }
.luna-export img { max-width: 100%; height: auto; }
@media print { .luna-export { background: transparent; } .luna-export__page { max-width: none; padding: 0; } pre, blockquote, table, img { break-inside: avoid; } }
`;

function ruleMatches(selector: string): boolean {
  return EXPORT_SELECTOR_HINTS.some((hint) => selector.includes(hint));
}

/** 递归收集命中的规则文本（含 `@media` 内嵌）。 */
function collectRules(rules: CSSRuleList, into: string[]): void {
  for (const rule of Array.from(rules)) {
    if (rule instanceof CSSStyleRule) {
      if (ruleMatches(rule.selectorText)) {
        into.push(rule.cssText);
      }
    } else if (rule instanceof CSSMediaRule || rule instanceof CSSSupportsRule) {
      const inner: string[] = [];
      collectRules(rule.cssRules, inner);
      if (inner.length > 0) {
        into.push(`${rule.cssText.slice(0, rule.cssText.indexOf("{"))}{${inner.join("\n")}}`);
      }
    }
  }
}

/**
 * 从当前文档收集导出样式：命中的规则 + 它们引用的 Token 在**此刻**的已解析值。
 * 跨域样式表读不到 `cssRules` 时跳过（应用内全部同源，这只是防御）。
 */
export function collectExportStyles(): string {
  const rules: string[] = [EXPORT_BASE_CSS];
  for (const sheet of Array.from(document.styleSheets)) {
    try {
      collectRules(sheet.cssRules, rules);
    } catch {
      // 跨域样式表：跳过。
    }
  }
  const joined = rules.join("\n");
  const tokens = new Set<string>();
  for (const match of joined.matchAll(/var\((--[\w-]+)/g)) {
    tokens.add(match[1]);
  }
  const computed = getComputedStyle(document.documentElement);
  const declarations: string[] = [];
  for (const token of tokens) {
    const value = computed.getPropertyValue(token).trim();
    if (value) {
      declarations.push(`${token}: ${value};`);
    }
  }
  return `:root { ${declarations.join(" ")} }\n${joined}`;
}

/** 静态渲染阅读视图为 HTML 片段（图片保留笔记相对 src，由 Rust 内联）。 */
export function renderNoteHtml(content: string, notePath: string, embeds?: ReadonlyMap<string, EmbedHitDto>): string {
  return renderToStaticMarkup(
    createElement(ReadingView, {
      content,
      notePath,
      staticExport: true,
      // 嵌入 `![[…]]` 展开进导出物（用户 2026-10-04 裁决）；不给就是没有嵌入。
      embeds: embeds ?? new Map(),
    }),
  );
}

/**
 * 导出一篇笔记为 HTML：读正文 → 静态渲染 → 收集样式 → Rust 内联图片并写盘。
 * 返回写入路径；用户取消保存返回 `null`；失败抛错误串（调用方翻成人话）。
 */
export async function exportNoteAsHtml(notePath: string): Promise<string | null> {
  const doc = await commands.readNote(notePath);
  if (doc.status === "error") {
    throw new Error(doc.error);
  }
  await prepareRichContent(doc.data.content);
  const embeds = await resolveEmbedsFor(notePath, doc.data.content);
  const body = renderNoteHtml(doc.data.content, notePath, embeds);
  const styles = collectExportStyles();
  const result = await commands.exportNoteHtml(notePath, body, styles, i18n.language);
  if (result.status === "error") {
    throw new Error(result.error);
  }
  return result.data;
}
