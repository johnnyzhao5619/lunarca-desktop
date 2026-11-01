/**
 * 静态渲染前的预热（HTML 导出 / 打印窗口 / 复制为带格式）：公式与 Mermaid 图都是按需异步
 * 加载的，而静态渲染等不了异步——不先预热，导出物里就是 TeX 原文与图表源码。
 */

import { parseChatMarkdown, type MdBlock } from "./markdown";
import { loadKatex, mayContainMath } from "./math-render";
import { renderMermaid } from "./mermaid-render";

function mermaidSources(blocks: MdBlock[], out: string[]): string[] {
  for (const block of blocks) {
    if (block.kind === "code_block" && block.lang.trim().toLowerCase() === "mermaid") out.push(block.code);
    if (block.kind === "blockquote" || block.kind === "callout") mermaidSources(block.blocks, out);
    if (block.kind === "list") for (const item of block.items) mermaidSources(item.blocks, out);
  }
  return out;
}

/** 让这段 Markdown 里的公式与图表都能同步渲染（`MathView` / `MermaidView` 读缓存）。 */
export async function prepareRichContent(markdown: string): Promise<void> {
  const tasks: Promise<unknown>[] = [];
  if (mayContainMath(markdown)) tasks.push(loadKatex());
  for (const code of mermaidSources(parseChatMarkdown(markdown), [])) tasks.push(renderMermaid(code));
  await Promise.all(tasks);
}
