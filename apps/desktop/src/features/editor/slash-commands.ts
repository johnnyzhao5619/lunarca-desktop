// 斜杠菜单（对标方案 B1；Notion / Craft 的 `/`，Obsidian 的 Slash commands 核心插件）。
//
// 只放「插入类」动作与 AI 入口；条目执行走与格式工具栏、右键菜单**同一个**动作源
// （`editor-actions.ts` 的 `EditorFormatAction`），文案复用同一批 i18n 键——同一件事在三个入口
// 叫同一个名字、做同一件事。
//
// 触发：行首或空白之后的 `/`；中文输入法下 `/` 键打出的是 `、`，故行首 / 空白之后的 `、`
// 同样触发（Notion 的中文做法）。紧跟在文字后的 `/`、`、`（`and/or`、`苹果、香蕉`）不触发。

import type { EditorFormatAction } from "./editor-actions";

export interface SlashCommand {
  id: string;
  /** `editor` 命名空间下的文案键（与右键菜单 / 工具栏同一个）。 */
  labelKey: string;
  /** 匹配用的别名：英文名与常用中文说法，前缀命中即算。 */
  keywords: readonly string[];
  action: EditorFormatAction | { kind: "ai" };
}

export const SLASH_COMMANDS: readonly SlashCommand[] = [
  { id: "h1", labelKey: "menu.h1", keywords: ["h1", "heading", "title", "标题", "一级"], action: { kind: "heading", level: 1 } },
  { id: "h2", labelKey: "menu.h2", keywords: ["h2", "heading", "subtitle", "标题", "二级"], action: { kind: "heading", level: 2 } },
  { id: "h3", labelKey: "menu.h3", keywords: ["h3", "heading", "标题", "三级"], action: { kind: "heading", level: 3 } },
  { id: "bullet", labelKey: "menu.bulletList", keywords: ["bullet", "list", "ul", "列表", "无序"], action: { kind: "block", block: "bullet" } },
  { id: "ordered", labelKey: "menu.orderedList", keywords: ["numbered", "ordered", "ol", "list", "列表", "有序", "编号"], action: { kind: "block", block: "ordered" } },
  { id: "task", labelKey: "menu.taskList", keywords: ["todo", "task", "checkbox", "待办", "任务"], action: { kind: "block", block: "task" } },
  { id: "quote", labelKey: "menu.quote", keywords: ["quote", "blockquote", "引用"], action: { kind: "block", block: "quote" } },
  { id: "table", labelKey: "table.insert", keywords: ["table", "grid", "表格"], action: { kind: "table" } },
  { id: "code", labelKey: "menu.codeBlock", keywords: ["code", "codeblock", "代码"], action: { kind: "codeBlock" } },
  { id: "callout", labelKey: "menu.callout", keywords: ["callout", "admonition", "note", "warning", "标注", "提示"], action: { kind: "callout" } },
  { id: "hr", labelKey: "menu.hr", keywords: ["divider", "hr", "line", "separator", "分隔", "分割"], action: { kind: "hr" } },
  { id: "wiki", labelKey: "menu.insertWikiLink", keywords: ["link", "wiki", "note", "链接", "笔记"], action: { kind: "wikiLink" } },
  { id: "template", labelKey: "menu.insertTemplate", keywords: ["template", "snippet", "模板", "模版"], action: { kind: "template" } },
  { id: "attachment", labelKey: "menu.insertAttachment", keywords: ["image", "file", "attachment", "picture", "图片", "附件", "文件"], action: { kind: "attachment" } },
  { id: "ai", labelKey: "menu.aiActions", keywords: ["ai", "ask", "write", "continue", "summarize", "续写", "总结", "智能"], action: { kind: "ai" } },
];

/** 斜杠触发：行首或空白之后的 `/` 或 `、`，后面跟着到光标为止的查询词（不含空白）。 */
export const SLASH_TRIGGER = /(?:^|\s)([/、])([^\s/、]*)$/u;

/**
 * 按查询词筛选并排序：文案以查询开头的最前，其次别名前缀命中，再次文案包含查询；
 * 查询为空时按清单顺序全给。比较不分大小写。
 */
export function matchSlashCommands(query: string, label: (cmd: SlashCommand) => string): SlashCommand[] {
  const q = query.trim().toLowerCase();
  if (!q) return [...SLASH_COMMANDS];
  const scored: { cmd: SlashCommand; score: number; index: number }[] = [];
  SLASH_COMMANDS.forEach((cmd, index) => {
    const text = label(cmd).toLowerCase();
    const score = text.startsWith(q) ? 0 : cmd.keywords.some((k) => k.startsWith(q)) ? 1 : text.includes(q) ? 2 : -1;
    if (score >= 0) scored.push({ cmd, score, index });
  });
  return scored.sort((a, b) => a.score - b.score || a.index - b.index).map((entry) => entry.cmd);
}
