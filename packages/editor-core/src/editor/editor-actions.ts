// 编辑器格式动作（PRD-06-014）：右键菜单、格式工具栏与斜杠菜单共用的**同一套**动作判别联合、
// 条目构建与执行分发（DRY，具体命令单一来源 = format-commands / table-edit / markdown-format）。
// 菜单本身（面板、定位、键盘）归宿主：桌面交给全应用唯一的菜单宿主（PRD-07-059）。

import type { EditorView } from "@codemirror/view";
import { formatShortcut } from "../input/shortcut-display";
import { editorShortcut, type EditorCommandId } from "./editor-shortcuts";
import {
  insertCallout,
  insertCodeBlock,
  insertHorizontalRule,
  insertWikiLink,
  setHeading,
  toggleInlineMark,
  toggleLineBlock,
  type FormatState,
  type HeadingLevel,
  type InlineMark,
  type LineBlockKind,
} from "./format-commands";
import { insertLink } from "./markdown-format";
import { assetMarkdown, insertAtCursor, isImageAssetPath } from "./paste-text";
import { insertTableCommand } from "./table-edit";

export type EditorFormatAction =
  | { kind: "link" }
  | { kind: "wikiLink" }
  | { kind: "inline"; mark: InlineMark }
  | { kind: "heading"; level: HeadingLevel }
  | { kind: "block"; block: LineBlockKind }
  | { kind: "table" }
  | { kind: "textToTable" }
  | { kind: "attachment" }
  | { kind: "template" }
  | { kind: "codeBlock" }
  | { kind: "callout" }
  | { kind: "hr" }
  | { kind: "cut" }
  | { kind: "copy" }
  | { kind: "paste" }
  | { kind: "pasteMarkdown" }
  | { kind: "selectAll" };

/** 带勾选态的菜单条目（`labelKey` 是 `editor-core` 命名空间下的文案键，由视图层过 i18n）。 */
export interface CheckableMenuItem {
  key: string;
  labelKey: string;
  checked: boolean;
  action: EditorFormatAction;
}

/** 文本格式条目（加粗/斜体/删除线/行内代码）。 */
export function inlineFormatItems(fs: FormatState): CheckableMenuItem[] {
  return [
    { key: "bold", labelKey: "editor.menu.bold", checked: fs.strong, action: { kind: "inline", mark: "strong" } },
    { key: "italic", labelKey: "editor.menu.italic", checked: fs.em, action: { kind: "inline", mark: "em" } },
    { key: "strike", labelKey: "editor.menu.strike", checked: fs.del, action: { kind: "inline", mark: "del" } },
    { key: "code", labelKey: "editor.menu.inlineCode", checked: fs.code, action: { kind: "inline", mark: "code" } },
  ];
}

/** 标题条目（正文 + H1–H3；工具栏下拉与右键「段落设置」共用）。 */
export function headingItems(fs: FormatState): CheckableMenuItem[] {
  const plainBody =
    fs.heading === 0 && !fs.bullet && !fs.ordered && !fs.task && !fs.quote;
  return [
    { key: "text", labelKey: "editor.menu.paragraphText", checked: plainBody, action: { kind: "heading", level: 0 } },
    { key: "h1", labelKey: "editor.menu.h1", checked: fs.heading === 1, action: { kind: "heading", level: 1 } },
    { key: "h2", labelKey: "editor.menu.h2", checked: fs.heading === 2, action: { kind: "heading", level: 2 } },
    { key: "h3", labelKey: "editor.menu.h3", checked: fs.heading === 3, action: { kind: "heading", level: 3 } },
  ];
}

/** 块级条目（引用/无序/有序/任务）。 */
export function lineBlockItems(fs: FormatState): CheckableMenuItem[] {
  return [
    { key: "quote", labelKey: "editor.menu.quote", checked: fs.quote, action: { kind: "block", block: "quote" } },
    { key: "bullet", labelKey: "editor.menu.bulletList", checked: fs.bullet, action: { kind: "block", block: "bullet" } },
    { key: "ordered", labelKey: "editor.menu.orderedList", checked: fs.ordered, action: { kind: "block", block: "ordered" } },
    { key: "task", labelKey: "editor.menu.taskList", checked: fs.task, action: { kind: "block", block: "task" } },
  ];
}

/**
 * 条目 key → 编辑器命令 id。键位本身只在 `editor-shortcuts.ts` 定义一次，CM6 keymap 与菜单 / 工具栏的
 * 提示从同一处派生。
 * 未列出的条目没有绑定键，菜单与工具栏就不显示提示——**绝不显示一个按不出效果的组合**。
 */
const ITEM_COMMANDS: Readonly<Record<string, EditorCommandId>> = {
  bold: "bold",
  italic: "italic",
  text: "heading_0",
  h1: "heading_1",
  h2: "heading_2",
  h3: "heading_3",
  link: "insert_link",
  table: "insert_table",
};

/** 条目绑定的编辑器命令（没绑键返回 `undefined`）。 */
export function formatItemCommand(key: string): EditorCommandId | undefined {
  return ITEM_COMMANDS[key];
}

/**
 * 条目的快捷键展示串（右键菜单、格式工具条与选区格式条共用；没绑键返回 `undefined`）。平台符号由
 * `formatShortcut` 决定——不写死 `⌘K` 一类字面量，否则在 Windows 上显示的就是错的。
 */
export function formatItemShortcut(key: string): string | undefined {
  const id = formatItemCommand(key);
  return id ? formatShortcut(editorShortcut(id)) : undefined;
}

/** 执行环境（剪贴板、附件与模板的选取、分列都归宿主；链接占位符走 i18n，由组装编辑器的一层注入）。 */
export interface EditorActionDeps {
  linkPlaceholder: string;
  /** 写剪贴板（复制/剪切）。 */
  copyText: (text: string) => Promise<void>;
  /** 读剪贴板（粘贴；不可用返回 null）。 */
  paste: (convertToMarkdown: boolean) => Promise<void>;
  /**
   * 「插入图片 / 附件」（PRD-06-012）：弹原生多选对话框、把文件复制进附件目录，返回**笔记相对**
   * 链接（用户取消 = 空数组）。对话框与落盘归宿主（桌面在 Rust，`import_asset_files`），这里只回填。
   */
  pickAttachments: () => Promise<string[]>;
  /**
   * 「插入模板…」（PRD-06-015，V6）：让用户选一份模板，交宿主渲染好（变量、`{{cursor}}`），
   * 返回要插的正文与光标落点（UTF-16 偏移）；取消 = `null`。
   */
  pickTemplate: () => Promise<{ text: string; cursor: number | null } | null>;
  /**
   * 「转为表格」：选中的整行文字交宿主分列（制表符 / 逗号 / 对齐空格），返回 GFM 表；
   * 看不出列返回 null（由调用方告诉用户为什么没转）。
   */
  convertTextToTable: (text: string) => Promise<string | null>;
}

/** 动作分发（菜单/工具栏共用唯一入口）。 */
export async function runEditorFormatAction(
  view: EditorView,
  action: EditorFormatAction,
  deps: EditorActionDeps,
): Promise<void> {
  switch (action.kind) {
    case "link":
      insertLink(view, deps.linkPlaceholder);
      break;
    case "wikiLink":
      insertWikiLink(view);
      break;
    case "inline":
      toggleInlineMark(view, action.mark);
      break;
    case "heading":
      setHeading(view, action.level);
      break;
    case "block":
      toggleLineBlock(view, action.block);
      break;
    case "table":
      insertTableCommand(view);
      view.focus();
      break;
    case "textToTable": {
      // 按整行取：半行选区转出来的表会把一行字劈成两半。
      const { from, to } = view.state.selection.main;
      const start = view.state.doc.lineAt(from).from;
      const end = view.state.doc.lineAt(to).to;
      const before = view.state.doc;
      const table = await deps.convertTextToTable(view.state.sliceDoc(start, end));
      // 等转换期间正文变了：范围已不可信，不落地（宁可不转，不改错地方）。
      if (table !== null && view.state.doc === before) {
        view.dispatch({ changes: { from: start, to: end, insert: table }, selection: { anchor: start }, userEvent: "input" });
      }
      view.focus();
      break;
    }
    case "attachment": {
      const links = await deps.pickAttachments();
      // 多个附件各占一行——图片挤在同一行会被渲染成并排，用户多半不是这个意思。
      const markdown = links
        .map((link) => assetMarkdown(link, isImageAssetPath(link)))
        .join("\n");
      if (markdown.length > 0) {
        insertAtCursor(view, markdown);
      }
      view.focus();
      break;
    }
    case "template": {
      const before = view.state.doc;
      const picked = await deps.pickTemplate();
      // 选模板期间正文变了：光标与选区已不可信，不落地（宁可不插，不插错地方）。
      if (picked !== null && view.state.doc === before) {
        const { from, to } = view.state.selection.main;
        view.dispatch({
          changes: { from, to, insert: picked.text },
          selection: { anchor: from + (picked.cursor ?? picked.text.length) },
          userEvent: "input",
          scrollIntoView: true,
        });
      }
      view.focus();
      break;
    }
    case "codeBlock":
      insertCodeBlock(view);
      break;
    case "callout":
      insertCallout(view);
      break;
    case "hr":
      insertHorizontalRule(view);
      break;
    case "cut": {
      const { from, to } = view.state.selection.main;
      if (from !== to) {
        await deps.copyText(view.state.sliceDoc(from, to));
        view.dispatch({ changes: { from, to, insert: "" }, userEvent: "delete.cut" });
      }
      view.focus();
      break;
    }
    case "copy": {
      const { from, to } = view.state.selection.main;
      if (from !== to) {
        await deps.copyText(view.state.sliceDoc(from, to));
      }
      view.focus();
      break;
    }
    case "paste": {
      await deps.paste(false);
      view.focus();
      break;
    }
    case "pasteMarkdown": {
      await deps.paste(true);
      view.focus();
      break;
    }
    case "selectAll":
      view.dispatch({ selection: { anchor: 0, head: view.state.doc.length } });
      view.focus();
      break;
    default:
      break;
  }
}
