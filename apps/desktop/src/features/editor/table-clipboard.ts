// 「复制表格为 ▸」的三种形态（对标方案 T2；Bear 同时提供 Markdown / CSV / 带格式）。
// 单元格文字取**去掉 Markdown 标记**的纯文本（粘进表格软件的是「乙」，不是「**乙**」）；
// 带格式那份就是阅读视图的静态渲染——与 HTML 导出同一条路（`renderNoteHtml`），不另写一套。

import { inlineToPlainText, parseChatMarkdown } from "@lunarca/editor-core/markdown";
import { commands } from "../../ipc/bindings";
import { renderNoteHtml } from "./note-export-html";
import { prepareRichContent } from "@lunarca/editor-core/rich-content";

/** 表格源码 → 纯文本单元格矩阵（首行为表头）；不是一张表时返回 null。 */
export function tablePlainCells(source: string): string[][] | null {
  const block = parseChatMarkdown(source)[0];
  if (block?.kind !== "table") return null;
  return [block.header, ...block.rows].map((row) => row.map((cell) => inlineToPlainText(cell).trim()));
}

/** RFC 4180：含逗号、引号或换行的字段加引号，引号写两遍。 */
export function toCsv(cells: string[][]): string {
  const field = (value: string) => (/[",\n\r]/u.test(value) ? `"${value.replace(/"/gu, '""')}"` : value);
  return cells.map((row) => row.map(field).join(",")).join("\r\n");
}

/** 表格软件之间互通的纯文本形态：制表符分列；格内换行与制表符压成空格。 */
export function toTsv(cells: string[][]): string {
  return cells.map((row) => row.map((value) => value.replace(/[\t\r\n]+/gu, " ")).join("\t")).join("\n");
}

/** 把表格按指定形态写进系统剪贴板（经 Rust；内容不入日志）。 */
export async function copyTableAs(source: string, format: "markdown" | "csv" | "rich", notePath: string): Promise<boolean> {
  if (format === "markdown") return (await commands.popupCopy(source)).status === "ok";
  const cells = tablePlainCells(source);
  if (!cells) return false;
  if (format === "csv") return (await commands.popupCopy(toCsv(cells))).status === "ok";
  await prepareRichContent(source);
  return (await commands.clipboardWriteRich(toTsv(cells), renderNoteHtml(source, notePath))).status === "ok";
}
