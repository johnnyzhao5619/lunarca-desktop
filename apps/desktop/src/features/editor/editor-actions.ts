// 编辑器格式动作路由（PRD-06-014）：右键菜单与格式工具栏（FormatToolbar）共用的
// **同一套**动作判别联合、菜单条目构建与执行分发（DRY，具体命令单一来源 =
// format-commands / table-edit / markdown-format）。
//
// 右键菜单自 2026-07-26 起不再自带面板组件，而是把条目交给全应用唯一的菜单宿主
// （`components/ContextMenuHost.tsx`，PRD-07-059）——定位/键盘/关闭不再各写一遍。

import type { EditorView } from "@codemirror/view";
import {
  buildMenu,
  separator,
  type ContextMenuEntry,
  type ContextMenuItem,
} from "../../lib/context-menu";
import { formatShortcut } from "../../lib/shortcut-display";
import { editorShortcut, type EditorCommandId } from "../command/command-registry";
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
import { assetMarkdown, insertAtCursor, isImageAssetPath } from "./paste-assets";
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

/** 带勾选态的菜单条目（labelKey 由视图层过 i18n）。 */
export interface CheckableMenuItem {
  key: string;
  labelKey: string;
  checked: boolean;
  action: EditorFormatAction;
}

/** 文本格式条目（加粗/斜体/删除线/行内代码）。 */
export function inlineFormatItems(fs: FormatState): CheckableMenuItem[] {
  return [
    { key: "bold", labelKey: "menu.bold", checked: fs.strong, action: { kind: "inline", mark: "strong" } },
    { key: "italic", labelKey: "menu.italic", checked: fs.em, action: { kind: "inline", mark: "em" } },
    { key: "strike", labelKey: "menu.strike", checked: fs.del, action: { kind: "inline", mark: "del" } },
    { key: "code", labelKey: "menu.inlineCode", checked: fs.code, action: { kind: "inline", mark: "code" } },
  ];
}

/** 标题条目（正文 + H1–H3；工具栏下拉与右键「段落设置」共用）。 */
export function headingItems(fs: FormatState): CheckableMenuItem[] {
  const plainBody =
    fs.heading === 0 && !fs.bullet && !fs.ordered && !fs.task && !fs.quote;
  return [
    { key: "text", labelKey: "menu.paragraphText", checked: plainBody, action: { kind: "heading", level: 0 } },
    { key: "h1", labelKey: "menu.h1", checked: fs.heading === 1, action: { kind: "heading", level: 1 } },
    { key: "h2", labelKey: "menu.h2", checked: fs.heading === 2, action: { kind: "heading", level: 2 } },
    { key: "h3", labelKey: "menu.h3", checked: fs.heading === 3, action: { kind: "heading", level: 3 } },
  ];
}

/** 块级条目（引用/无序/有序/任务）。 */
export function lineBlockItems(fs: FormatState): CheckableMenuItem[] {
  return [
    { key: "quote", labelKey: "menu.quote", checked: fs.quote, action: { kind: "block", block: "quote" } },
    { key: "bullet", labelKey: "menu.bulletList", checked: fs.bullet, action: { kind: "block", block: "bullet" } },
    { key: "ordered", labelKey: "menu.orderedList", checked: fs.ordered, action: { kind: "block", block: "ordered" } },
    { key: "task", labelKey: "menu.taskList", checked: fs.task, action: { kind: "block", block: "task" } },
  ];
}

/** i18n 取词器（带命名空间前缀调用；无头测试注入桩）。 */
export type EditorMenuTranslate = (key: string, vars?: Record<string, string>) => string;

/**
 * 条目 key → 命令注册表里的编辑器命令 id。键位本身只在注册表定义一次
 * （`features/command/command-registry.ts`），CM6 keymap 与这里的提示从同一处派生。
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

/**
 * 条目的快捷键展示串（右键菜单与格式工具栏共用；没绑键返回 `undefined`）。
 * 平台符号由 `formatShortcut` 决定——工具栏此前写死 `⌘K` / `⌘⌥T`，在 Windows 上就是错的。
 */
export function formatItemShortcut(key: string): string | undefined {
  const id = ITEM_COMMANDS[key];
  return id ? formatShortcut(editorShortcut(id)) : undefined;
}

export interface EditorContextMenuOptions {
  t: EditorMenuTranslate;
  formatState: FormatState;
  hasSelection: boolean;
  onAction: (action: EditorFormatAction) => void;
  /**
   * 打开 AI 操作面板（PRD-06-020，Apple 写作工具「右键 → 写作工具」对标）：有选区 = 选区
   * 作用域，无选区 = 光标处；键位提示从命令注册表派生。
   */
  onAiActions: () => void;
  /** 朗读当前选区（TTS，非格式动作，单开通道；TDD-05 §9）。 */
  onReadSelection: () => void;
  /**
   * 选区收敛出的词条（`null` = 这段选区不适合当词条，该项不渲染）。
   * 判据在 `features/settings/glossary-term.ts`，与回放字幕行同一份。
   */
  glossaryTerm: string | null;
  /** 就地加词（PRD-07-061）：派发到唯一宿主 `GlossaryQuickAdd`。 */
  onAddToGlossary: () => void;
  /**
   * 整理列表层级（PRD-06-056）：折叠 AI 导入内容常见的单行空嵌套列表链
   * （`* * 文字`）。显式动作：预览确认卡在 VaultEditor，归一逻辑全在 Rust。
   */
  onTidyListLevels: () => void;
  /**
   * 「标记为记录」（PRD-06-058 D）：把这一篇打回记录态（frontmatter 加 `luna.stage`）。
   *
   * `null` = 这一篇不能被标记（记录态自己 / 资料 / 主题页 / 知识地图），此时**该项不
   * 渲染**而不是渲染成禁用——一个永远点不动的菜单项只是噪声（同「就地加词」的先例）。
   * 判据由 Rust 下发（`note_draft_state`），前端不自己拼。
   */
  onMarkAsDraft: (() => void) | null;
  /** 「插入模板…」可不可用（副库窗口里不给：模板属主库，V6）。 */
  templatesAvailable: boolean;
  /** 「AI 操作」的键位（设置真值，W3b；空 = 没绑键）。 */
  aiShortcut: string;
  /**
   * 光标处这个词拼错了（W6a，判定在 Rust）：候选排在菜单最上面，点了即替换；`null` = 没拼错、
   * 不是西文单词或拼写检查没开，整组不渲染。
   */
  spelling?: SpellingMenu | null;
}

/** 拼错的那个词的菜单内容。 */
export interface SpellingMenu {
  /** 系统给的候选（菜单里最多列 {@link MAX_SPELLING_GUESSES} 个）。 */
  guesses: readonly string[];
  onReplace: (guess: string) => void;
  /** 「学习拼写」：记进系统用户词典。 */
  onLearn: () => void;
}

/** 菜单里最多列几个候选（与系统原生菜单口径相近，再多就是在让人读字典）。 */
export const MAX_SPELLING_GUESSES = 5;

function spellingItems(t: EditorMenuTranslate, spelling: SpellingMenu): ContextMenuEntry[] {
  const guesses = spelling.guesses.slice(0, MAX_SPELLING_GUESSES);
  return [
    ...(guesses.length > 0
      ? guesses.map(
          (guess, index): ContextMenuEntry => ({
            kind: "action",
            id: `spelling-${index}`,
            label: guess,
            onSelect: () => spelling.onReplace(guess),
          }),
        )
      : [
          {
            kind: "action" as const,
            id: "spelling-none",
            label: t("editor:menu.noSpellingGuesses"),
            disabled: true,
            onSelect: () => {},
          },
        ]),
    { kind: "action", id: "spelling-learn", label: t("editor:menu.learnSpelling"), onSelect: spelling.onLearn },
    separator("spelling"),
  ];
}

/**
 * 编辑器右键菜单条目（PRD-06-014）。
 *
 * 与格式工具栏共用同一套条目构建器（`inlineFormatItems` / `headingItems` /
 * `lineBlockItems`），当前格式态以 `checked` 呈现；剪贴板四项的文案取 `common:menu.*`
 * ——那是全应用同一份「剪切/复制/粘贴/全选」，编辑器不该另存一份。
 */
export function editorContextMenuItems(options: EditorContextMenuOptions): ContextMenuItem[] {
  const {
    t,
    formatState,
    hasSelection,
    onAction,
    onAiActions,
    onReadSelection,
    glossaryTerm,
    onAddToGlossary,
    onTidyListLevels,
    onMarkAsDraft,
    templatesAvailable,
    aiShortcut,
    spelling,
  } = options;
  const checkable = (item: CheckableMenuItem) => ({
    kind: "action" as const,
    id: item.key,
    label: t(`editor:${item.labelKey}`),
    checked: item.checked,
    shortcut: formatItemShortcut(item.key),
    onSelect: () => onAction(item.action),
  });

  return buildMenu([
    ...(spelling ? spellingItems(t, spelling) : []),
    {
      kind: "action",
      id: "aiActions",
      label: t("editor:menu.aiActions"),
      // 键位是可改的设置（W3b），由宿主传入真值；没绑键就不显示提示。
      shortcut: aiShortcut ? formatShortcut(aiShortcut) : undefined,
      onSelect: onAiActions,
    },
    separator("ai"),
    {
      kind: "action",
      id: "link",
      label: t("editor:menu.insertLink"),
      shortcut: formatItemShortcut("link"),
      onSelect: () => onAction({ kind: "link" }),
    },
    {
      kind: "action",
      id: "wikiLink",
      label: t("editor:menu.insertWikiLink"),
      onSelect: () => onAction({ kind: "wikiLink" }),
    },
    separator("insert"),
    {
      kind: "submenu",
      id: "textFormat",
      label: t("editor:menu.textFormat"),
      items: inlineFormatItems(formatState).map(checkable),
    },
    {
      kind: "submenu",
      id: "paragraph",
      label: t("editor:menu.paragraph"),
      items: [
        ...headingItems(formatState).map(checkable),
        ...lineBlockItems(formatState).map(checkable),
      ],
    },
    {
      kind: "submenu",
      id: "insert",
      label: t("editor:menu.insert"),
      items: buildMenu([
        {
          kind: "action",
          id: "table",
          label: t("editor:table.insert"),
          shortcut: formatItemShortcut("table"),
          onSelect: () => onAction({ kind: "table" }),
        },
        {
          kind: "action",
          id: "textToTable",
          label: t("editor:menu.textToTable"),
          disabled: !hasSelection,
          onSelect: () => onAction({ kind: "textToTable" }),
        },
        {
          kind: "action",
          id: "attachment",
          label: t("editor:menu.insertAttachment"),
          onSelect: () => onAction({ kind: "attachment" }),
        },
        templatesAvailable && {
          kind: "action" as const,
          id: "template",
          label: t("editor:menu.insertTemplate"),
          onSelect: () => onAction({ kind: "template" }),
        },
        {
          kind: "action",
          id: "codeBlock",
          label: t("editor:menu.codeBlock"),
          onSelect: () => onAction({ kind: "codeBlock" }),
        },
        {
          kind: "action",
          id: "callout",
          label: t("editor:menu.callout"),
          onSelect: () => onAction({ kind: "callout" }),
        },
        {
          kind: "action",
          id: "hr",
          label: t("editor:menu.hr"),
          onSelect: () => onAction({ kind: "hr" }),
        },
      ]),
    },
    separator("read"),
    {
      kind: "action",
      id: "readSelection",
      label: t("editor:read.selection"),
      disabled: !hasSelection,
      onSelect: onReadSelection,
    },
    // 就地加词：只在选区**确实像一条词条**时出现（跨行/超长的选区不渲染，
    // 而不是渲染一个永远点不动的禁用项——那只是噪声）。
    glossaryTerm !== null && {
      kind: "action",
      id: "addToGlossary",
      label: t("settings:glossary.quickAdd.menu", { term: glossaryTerm }),
      onSelect: onAddToGlossary,
    },
    separator("tidy"),
    // 整理列表层级（PRD-06-056）：对象是整篇正文而非选区/光标处格式，故自成一组。
    {
      kind: "action",
      id: "tidyListLevels",
      label: t("editor:menu.tidyListLevels"),
      onSelect: onTidyListLevels,
    },
    // 「标记为记录」（PRD-06-058）：同样是对整篇的动作，故与上一条同组。
    // 页头**不给**常驻按钮——把写好的笔记打回记录是低频的自我修正，高频面只留高频动作。
    onMarkAsDraft && {
      kind: "action",
      id: "markAsDraft",
      label: t("workspace:draft.markAsDraft"),
      onSelect: onMarkAsDraft,
    },
    separator("clipboard"),
    {
      kind: "action",
      id: "cut",
      label: t("common:menu.cut"),
      shortcut: formatShortcut("cmd_or_ctrl+x"),
      disabled: !hasSelection,
      onSelect: () => onAction({ kind: "cut" }),
    },
    {
      kind: "action",
      id: "copy",
      label: t("common:menu.copy"),
      shortcut: formatShortcut("cmd_or_ctrl+c"),
      disabled: !hasSelection,
      onSelect: () => onAction({ kind: "copy" }),
    },
    {
      kind: "action",
      id: "paste",
      label: t("common:menu.paste"),
      shortcut: formatShortcut("cmd_or_ctrl+v"),
      onSelect: () => onAction({ kind: "paste" }),
    },
    {
      kind: "action",
      id: "pasteMarkdown",
      label: t("editor:paste.asMarkdown"),
      onSelect: () => onAction({ kind: "pasteMarkdown" }),
    },
    {
      kind: "action",
      id: "selectAll",
      label: t("common:menu.selectAll"),
      shortcut: formatShortcut("cmd_or_ctrl+a"),
      onSelect: () => onAction({ kind: "selectAll" }),
    },
  ]);
}

/** 执行环境（剪贴板走 Rust 命令、链接占位符走 i18n，由 VaultEditor 注入）。 */
export interface EditorActionDeps {
  linkPlaceholder: string;
  /** 写剪贴板（复制/剪切）。 */
  copyText: (text: string) => Promise<void>;
  /** 读剪贴板（粘贴；不可用返回 null）。 */
  paste: (convertToMarkdown: boolean) => Promise<void>;
  /**
   * 「插入图片 / 附件」（PRD-06-012）：弹原生多选对话框、把文件复制进附件目录，返回**笔记相对**
   * 链接（用户取消 = 空数组）。对话框与落盘全在 Rust（`import_asset_files`），这里只回填。
   */
  pickAttachments: () => Promise<string[]>;
  /**
   * 「插入模板…」（PRD-06-015，V6）：让用户选一份模板，交 Rust 渲染好（变量、`{{cursor}}`），
   * 返回要插的正文与光标落点（UTF-16 偏移）；取消 = `null`。
   */
  pickTemplate: () => Promise<{ text: string; cursor: number | null } | null>;
  /**
   * 「转为表格」：选中的整行文字交 Rust 分列（制表符 / 逗号 / 对齐空格），返回 GFM 表；
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
