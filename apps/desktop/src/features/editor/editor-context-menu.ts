// 编辑器右键菜单（PRD-06-014 / PRD-07-059，桌面）：条目交给全应用唯一的菜单宿主
// （`components/ContextMenuHost.tsx`），定位 / 键盘 / 关闭不再各写一遍。格式条目与动作分发是共享编辑器的
// （`@lunarca/editor-core/editor-actions`）；这里只拼桌面独有的那几组：AI、朗读、就地加词、整理列表层级、
// 标记为记录、拼写建议。

import {
  buildMenu,
  separator,
  type ContextMenuEntry,
  type ContextMenuItem,
} from "../../lib/context-menu";
import { formatShortcut } from "@lunarca/editor-core/shortcut-display";
import {
  formatItemShortcut,
  headingItems,
  inlineFormatItems,
  lineBlockItems,
  type CheckableMenuItem,
  type EditorFormatAction,
} from "@lunarca/editor-core/editor-actions";
import type { FormatState } from "@lunarca/editor-core/format-commands";

/** i18n 取词器（带命名空间前缀调用；无头测试注入桩）。 */
export type EditorMenuTranslate = (key: string, vars?: Record<string, string>) => string;

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
    label: t(`editor-core:${item.labelKey}`),
    checked: item.checked,
    shortcut: formatItemShortcut(item.key),
    onSelect: () => onAction(item.action),
  });

  return buildMenu([
    ...(spelling ? spellingItems(t, spelling) : []),
    {
      kind: "action",
      id: "aiActions",
      label: t("editor-core:editor.menu.aiActions"),
      // 键位是可改的设置（W3b），由宿主传入真值；没绑键就不显示提示。
      shortcut: aiShortcut ? formatShortcut(aiShortcut) : undefined,
      onSelect: onAiActions,
    },
    separator("ai"),
    {
      kind: "action",
      id: "link",
      label: t("editor-core:editor.menu.insertLink"),
      shortcut: formatItemShortcut("link"),
      onSelect: () => onAction({ kind: "link" }),
    },
    {
      kind: "action",
      id: "wikiLink",
      label: t("editor-core:editor.menu.insertWikiLink"),
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
          label: t("editor-core:editor.table.insert"),
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
          label: t("editor-core:editor.menu.insertAttachment"),
          onSelect: () => onAction({ kind: "attachment" }),
        },
        templatesAvailable && {
          kind: "action" as const,
          id: "template",
          label: t("editor-core:editor.menu.insertTemplate"),
          onSelect: () => onAction({ kind: "template" }),
        },
        {
          kind: "action",
          id: "codeBlock",
          label: t("editor-core:editor.menu.codeBlock"),
          onSelect: () => onAction({ kind: "codeBlock" }),
        },
        {
          kind: "action",
          id: "callout",
          label: t("editor-core:editor.menu.callout"),
          onSelect: () => onAction({ kind: "callout" }),
        },
        {
          kind: "action",
          id: "hr",
          label: t("editor-core:editor.menu.hr"),
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
