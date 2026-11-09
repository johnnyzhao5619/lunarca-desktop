import type { DoubleTapDetector } from "../../lib/double-tap";
import { parseShortcutValue } from "@lunarca/editor-core/shortcut-display";
import { mainKeyOf } from "../../lib/shortcut-keys";
import { EDITOR_SHORTCUTS, SHORTCUT_MODIFIER_TOKENS, type EditorCommandId } from "@lunarca/editor-core/editor-shortcuts";

/**
 * 一级空间 id（与 `features/workspace/workspace-ia.ts` 的空间联合逐字一致，由
 * `command-registry.test.ts` 钉死）。不从 workspace 引入：那会把 `command → workspace →
 * editor → command` 连成环（结构门禁），而注册表本该是最底层。
 */
export type CommandSpace =
  | "search"
  | "notes"
  | "knowledge"
  | "quick_notes"
  | "recordings"
  | "transcripts"
  | "chat"
  | "creator"
  | "teleprompter"
  | "apps"
  | "trash"
  | "settings";

export type CommandScope = "workspace" | "editor";

/**
 * 工作区命令的执行上下文——由 shell（`WorkspaceApp`）一处提供。
 *
 * 命令面板、快捷键、菜单都经 `commandById(id).run(context)` 执行，于是"新建笔记走哪条
 * 路径 / 分屏怎么开"只在 shell 定义一次；注册表本身不持有任何状态。
 */
export interface CommandContext {
  openQuickSwitcher: () => void;
  openCommandPalette: () => void;
  /** 切到搜索目的地并聚焦查询框（PRD-06-031）。 */
  openVaultSearch: () => void;
  /** 主侧栏收起/展开（PRD-07-057）。 */
  toggleSidebar: () => void;
  createNewNote: () => void;
  /**
   * 文字速记（PRD-07-067 ②）：跳到「速记」并把光标放进那里的捕获框。
   *
   * 与「添加内容」浮层里那一项共用宿主的同一条实现，不在这里另写一遍：两个
   * 呈现面对同一个动作给出两种行为，是这张注册表存在的理由所要杜绝的第一件事。
   */
  captureTextQuickNote: () => void;
  /** 向右分屏（PRD-07-036）；已到上限时 shell 自行忽略。 */
  splitRight: () => void;
  openEmptyTab: () => void;
  /** 重开最近关闭的标签（PRD-07-035 W2b）；栈空时 shell 自行忽略。 */
  reopenClosedTab: () => void;
  /** 前往某个一级空间（TDD-07 §4.2c：命令面板可执行「前往」，但它不是隐藏导航的唯一路径）。 */
  goToSpace: (space: CommandSpace) => void;
  /**
   * 打开启动概览（PRD-07-050）。
   *
   * 概览刻意不是导航目的地、也不回活动栏，但它必须**有一个入口**：它此前只在启动时
   * 「有事」才弹一次，关掉之后本次运行再也打不开——而一个用来回答「现在什么在跑 /
   * 我这周做了什么」的表面，如果只在被打扰时可见，那份数据等于不存在。
   * 命令面板是既有的非导航入口，用它（对标 VS Code 的 Help → Welcome）。
   */
  openOverview: () => void;
  /** 创作：新建视频项目 / 新建灵感 / 打开灵感库（与侧栏、看板同一条实现）。 */
  createCreatorProject: () => void;
  createSwipe: () => void;
  openSwipeLibrary: () => void;
  /** 创作：此刻打开着的项目的提词器 / 发布清单（没有打开着的项目时命令面板不列）。 */
  openCreatorProjectTeleprompter: () => void;
  openCreatorProjectPublish: () => void;
  /** 提词器：用提词器打开此刻编辑着的那篇笔记（没有打开着的笔记时命令面板不列）。 */
  openActiveNoteInTeleprompter: () => void;
  /** 提词器：新建一篇提词稿（`fromClipboard`：正文取剪贴板）并用提词器打开。 */
  createTeleprompterDraft: (fromClipboard: boolean) => void;
}

export type WorkspaceCommandId =
  | "quick_switcher"
  | "command_palette"
  | "vault_search"
  | "toggle_sidebar"
  | "new_note"
  | "text_quick_note"
  | "split_right"
  | "new_tab"
  | "reopen_closed_tab"
  | "open_overview"
  | "creator_new_project"
  | "creator_new_swipe"
  | "creator_swipe_library"
  | "creator_teleprompter"
  | "creator_publish"
  | "teleprompter_open_current"
  | "teleprompter_new_draft"
  | "teleprompter_from_clipboard"
  | `go_${CommandSpace}`;

export interface CommandDefinition {
  id: WorkspaceCommandId;
  titleKey: string;
  /**
   * 键位：指向设置注册表里的那一项（`shortcuts.*`），默认值与真值都由 Rust 下发
   * （[`ShortcutBindings`]）——应用内快捷键全部可改键（PRD-09-005，W3），这里不写字面值。
   * 没绑键的命令不写（命令面板右侧就不显示提示——绝不显示一个按不出效果的组合）。
   */
  settingKey?: string;
  scope: CommandScope;
  /**
   * 「前往」类命令的目的地：所属一方应用未启用时，命令面板不列它（ADR-165 ④——停用收回全部
   * 入口；列出来点了也到不了）。
   */
  space?: CommandSpace;
  /** 只在打开着一个创作项目时有意义（项目的提词器、发布清单）：没打开时命令面板不列。 */
  needsCreatorProject?: true;
  /** 只在有一篇打开着的笔记时有意义（「在提词器中打开这篇」）：没有时命令面板不列。 */
  needsActiveNote?: true;
  run: (context: CommandContext) => void;
}

/** 设置注册表下发的快捷键真值：`shortcuts.<id>` → 键值串（可带 `double_tap:` 前缀）。 */
export type ShortcutBindings = Readonly<Record<string, string>>;

/** 一条命令此刻的键值串（设置真值；没绑键 = `undefined`）。 */
export function commandShortcut(
  command: CommandDefinition,
  bindings: ShortcutBindings,
): string | undefined {
  return command.settingKey ? bindings[command.settingKey] || undefined : undefined;
}

/** 一级空间（顺序即活动栏默认顺序，`workspace-ia.ts`）：命令面板的「前往 …」逐个生成。 */
export const COMMAND_SPACES: readonly CommandSpace[] = [
  "search",
  "notes",
  "knowledge",
  "quick_notes",
  "recordings",
  "transcripts",
  "chat",
  "creator",
  "teleprompter",
  "apps",
  "trash",
  "settings",
];

export const COMMANDS: readonly CommandDefinition[] = [
  {
    id: "quick_switcher",
    titleKey: "command:quick_switcher.title",
    settingKey: "shortcuts.quick_switcher",
    scope: "workspace",
    run: (context) => {
      context.openQuickSwitcher();
    },
  },
  // 命令面板（PRD-07-041，`⌘P` / `Ctrl+P`，Obsidian / VS Code 同键）：按名执行任意命令。
  {
    id: "command_palette",
    titleKey: "command:palette.title",
    settingKey: "shortcuts.command_palette",
    scope: "workspace",
    run: (context) => {
      context.openCommandPalette();
    },
  },
  {
    id: "vault_search",
    titleKey: "command:vault_search.title",
    settingKey: "shortcuts.vault_search",
    scope: "workspace",
    run: (context) => {
      context.openVaultSearch();
    },
  },
  {
    id: "toggle_sidebar",
    titleKey: "command:toggle_sidebar.title",
    settingKey: "shortcuts.toggle_sidebar",
    scope: "workspace",
    run: (context) => {
      context.toggleSidebar();
    },
  },
  {
    id: "new_note",
    titleKey: "command:new_note.title",
    scope: "workspace",
    run: (context) => {
      context.createNewNote();
    },
  },
  // 文字速记（PRD-06-058 B / PRD-07-067 ②）。刻意不写 `shortcut`：这个动作的键是全局热键
  // （`shortcuts.text_quick_note`，用户可改），由 Rust 响应；命令面板不展示应用内接不到的键，
  // 发现性由「速记」侧栏与空态上的真实绑定展示承担。
  {
    id: "text_quick_note",
    titleKey: "command:text_quick_note.title",
    scope: "workspace",
    run: (context) => {
      context.captureTextQuickNote();
    },
  },
  {
    id: "split_right",
    titleKey: "workspace:tabsbar.splitRight",
    scope: "workspace",
    run: (context) => {
      context.splitRight();
    },
  },
  {
    id: "new_tab",
    titleKey: "workspace:tabsbar.newTab",
    scope: "workspace",
    run: (context) => {
      context.openEmptyTab();
    },
  },
  // 浏览器 / VS Code / Obsidian 同键。
  {
    id: "reopen_closed_tab",
    titleKey: "workspace:tabsbar.reopenClosed",
    settingKey: "shortcuts.reopen_closed_tab",
    scope: "workspace",
    run: (context) => {
      context.reopenClosedTab();
    },
  },
  // 启动概览的唯一手动入口（PRD-07-050）。刻意不给固定键：它不是高频动作，而应用里
  // 没被占用的组合已经不多，为一个一天开一次的表面占掉一个键不划算。
  {
    id: "open_overview",
    titleKey: "command:open_overview.title",
    scope: "workspace",
    run: (context) => {
      context.openOverview();
    },
  },
  // 「创作：」命令组（ADR-165 ⑤ 贡献点：命令）。归创作这个空间——应用未启用时与「前往：创作」
  // 一起不列（ADR-165 ④）。
  {
    id: "creator_new_project",
    titleKey: "command:creator.new_project",
    scope: "workspace",
    space: "creator",
    run: (context) => {
      context.createCreatorProject();
    },
  },
  {
    id: "creator_new_swipe",
    titleKey: "command:creator.new_swipe",
    scope: "workspace",
    space: "creator",
    run: (context) => {
      context.createSwipe();
    },
  },
  {
    id: "creator_swipe_library",
    titleKey: "command:creator.swipe_library",
    scope: "workspace",
    space: "creator",
    run: (context) => {
      context.openSwipeLibrary();
    },
  },
  {
    id: "creator_teleprompter",
    titleKey: "command:creator.teleprompter",
    scope: "workspace",
    space: "creator",
    needsCreatorProject: true,
    run: (context) => {
      context.openCreatorProjectTeleprompter();
    },
  },
  {
    id: "creator_publish",
    titleKey: "command:creator.publish",
    scope: "workspace",
    space: "creator",
    needsCreatorProject: true,
    run: (context) => {
      context.openCreatorProjectPublish();
    },
  },
  // 「提词器：」命令组（ADR-176 ①）：归提词器空间——应用未生效时与「前往：提词器」一起不列。
  {
    id: "teleprompter_open_current",
    titleKey: "command:teleprompter.open_current",
    scope: "workspace",
    space: "teleprompter",
    needsActiveNote: true,
    run: (context) => {
      context.openActiveNoteInTeleprompter();
    },
  },
  {
    id: "teleprompter_new_draft",
    titleKey: "command:teleprompter.new_draft",
    scope: "workspace",
    space: "teleprompter",
    run: (context) => {
      context.createTeleprompterDraft(false);
    },
  },
  {
    id: "teleprompter_from_clipboard",
    titleKey: "command:teleprompter.from_clipboard",
    scope: "workspace",
    space: "teleprompter",
    run: (context) => {
      context.createTeleprompterDraft(true);
    },
  },
  ...COMMAND_SPACES.map(
    (space): CommandDefinition => ({
      id: `go_${space}`,
      titleKey: `command:go.${space}`,
      // 「前往设置」独有一个键：`⌘,` / `Ctrl+,` 是 macOS 系统惯例（VS Code / Obsidian 同键），
      // 其余空间没有约定俗成的键，不硬造。
      ...(space === "settings" ? { settingKey: "shortcuts.open_settings" } : {}),
      scope: "workspace",
      space,
      run: (context) => {
        context.goToSpace(space);
      },
    }),
  ),
];

export function commandById(id: WorkspaceCommandId): CommandDefinition | undefined {
  return COMMANDS.find((command) => command.id === id);
}

/**
 * 命令面板里可按名执行的**编辑器作用域**动作（执行归活跃编辑器，经
 * `lib/app-events.ts::requestEditorCommand` 广播，编辑器按 `active` 认领）。
 *
 * 与 `EditorCommandId` 分开：那张表是**键位**（格式命令在编辑器内由 keymap 直接执行，
 * 不必绕命令面板——面板里列「加粗」只是噪声，Obsidian 也不列）；这张表是**面板条目**，
 * 只收"离开键盘也想得到名字"的整篇动作。有键的条目从 `EDITOR_SHORTCUTS` 取提示。
 */
export type EditorPaletteCommandId =
  | "toggle_reading_view"
  | "ai_composer"
  | "view_live"
  | "view_source"
  | "view_reading"
  | "read_aloud"
  | "version_history"
  | "ai_doc"
  | "typewriter_mode"
  | "focus_mode"
  | "spellcheck"
  | "word_goal"
  | "format_toolbar"
  | "view_in_graph"
  | "insert_template"
  | "generate_graph_tags"
  | "tab_focus_mode";

export interface EditorPaletteCommand {
  id: EditorPaletteCommandId;
  titleKey: string;
  /**
   * 键位：设置注册表里的那一项（`shortcuts.*`，W3b 起可改键）；没键的整篇动作不写。
   * 判定归活跃编辑器（独立笔记窗与副库窗口里也要能按），见 [`editorCommandForKey`]。
   */
  settingKey?: string;
  /** 固定键位（不可改键的编辑器键，`EDITOR_SHORTCUTS`）：只用于在面板里显示提示。 */
  editorShortcut?: EditorCommandId;
}

export const EDITOR_PALETTE_COMMANDS: readonly EditorPaletteCommand[] = [
  // 编辑 ⇄ 阅读切换（PRD-06-017 三态视图，Obsidian 同键 ⌘E）。
  {
    id: "toggle_reading_view",
    titleKey: "command:editor.toggle_reading_view",
    settingKey: "shortcuts.toggle_reading_view",
  },
  // AI 操作召唤（PRD-06-020，Notion 同键 ⌘J）：有选区 = 选区作用域，无选区 = 光标处。
  {
    id: "ai_composer",
    titleKey: "command:editor.ai_composer",
    settingKey: "shortcuts.ai_composer",
  },
  { id: "view_live", titleKey: "editor-core:editor.mode.live" },
  { id: "view_source", titleKey: "editor-core:editor.mode.source" },
  { id: "view_reading", titleKey: "editor-core:editor.mode.reading" },
  { id: "read_aloud", titleKey: "editor:read.readDoc" },
  { id: "version_history", titleKey: "editor:version.open" },
  { id: "ai_doc", titleKey: "editor:floating.trigger_title" },
  { id: "typewriter_mode", titleKey: "editor:writing.typewriter" },
  { id: "focus_mode", titleKey: "editor:writing.focus" },
  // 西文拼写检查（W6a）：与「更多」菜单、设置页同一个设置项。
  { id: "spellcheck", titleKey: "editor:writing.spellcheck" },
  // 字数目标（W6b）：与页头「更多」同一张输入卡。
  { id: "word_goal", titleKey: "editor:wordGoal.menu" },
  { id: "format_toolbar", titleKey: "editor:toolbar.show" },
  // 插入模板（V6）：与斜杠菜单「/模板」、右键「插入 ▸」同一个动作。
  { id: "insert_template", titleKey: "editor-core:editor.menu.insertTemplate" },
  { id: "view_in_graph", titleKey: "editor:graphTags.viewGraph" },
  { id: "generate_graph_tags", titleKey: "editor:graphTags.generate" },
  // 键盘用户离开正文的显式出路（WCAG 2.1.2）：面板里搜得到、带键位提示，开着时状态栏有指示。
  { id: "tab_focus_mode", titleKey: "command:editor.tab_focus_mode", editorShortcut: "tab_focus_mode" },
];

/**
 * 键盘事件是否命中一枚注册表键值串（`cmd_or_ctrl+shift+f` 这种）。
 *
 * 判定是**精确**的：要求的修饰键必须都按下，没要求的 Shift / Alt 必须都没按下——
 * 否则 `⌘⇧F` 会同时命中 `⌘F` 的谓词，两条命令抢同一次按键。`cmd_or_ctrl` 取
 * `metaKey || ctrlKey`，与 `@lunarca/editor-core/shortcut-display` 的展示口径一致（macOS 显 ⌘、
 * 其余显 Ctrl，判定两边都认）。
 */
export function matchesShortcut(
  event: Pick<KeyboardEvent, "key" | "metaKey" | "ctrlKey" | "shiftKey" | "altKey"> & {
    code?: string;
  },
  value: string,
): boolean {
  // 只判"这一下按的是不是这个组合"；是否要按两次由 `workspaceCommandForKey` 另判。
  const { parts } = parseShortcutValue(value);
  const keys = parts.filter((part) => !SHORTCUT_MODIFIER_TOKENS.has(part));
  if (keys.length !== 1) {
    return false;
  }
  const wantsPrimary = parts.includes("cmd_or_ctrl");
  const wantsShift = parts.includes("shift");
  const wantsAlt = parts.includes("alt");
  return (
    (event.metaKey || event.ctrlKey) === wantsPrimary &&
    event.shiftKey === wantsShift &&
    event.altKey === wantsAlt &&
    pressedKeyName(event) === keys[0].toLowerCase()
  );
}

/**
 * 这一下按的主键在注册表里叫什么。与录快捷键**同一份**物理键映射（`lib/shortcut-keys.ts`）：
 * 设置里存的是 `backslash`、`comma` 这样的名字，拿 `event.key`（`\`、`,`，还随布局与 Shift
 * 变）去比永远对不上。没有 `code` 的事件（合成事件）退回 `event.key`。
 */
function pressedKeyName(event: { key: string; code?: string }): string {
  return (event.code ? mainKeyOf({ code: event.code, key: event.key }) : null) ?? event.key.toLowerCase();
}

/** 应用内双击判定所需的运行态：检测器 + 时间窗（`null` = 尚未从设置读到，暂不判双击）。 */
export interface DoubleTapContext {
  detector: DoubleTapDetector;
  windowMs: number | null;
  nowMs: number;
}

/**
 * 工作区快捷键的统一判定：命中哪条有键的工作区命令就返回它（没命中 = null）。
 * shell 只挂一个 `keydown` 监听、按返回值执行；编辑器作用域的键由活跃编辑器自己判。
 *
 * 双击型绑定（`double_tap:` 前缀）第一下只记账不执行；修饰键本身的 keydown 不算"夹了别的键"。
 */
export function workspaceCommandForKey(
  event: KeyboardEvent,
  bindings: ShortcutBindings,
  doubleTap?: DoubleTapContext,
): CommandDefinition | null {
  return firedBinding(
    event,
    COMMANDS.map((command) => ({ id: command.id, value: commandShortcut(command, bindings), item: command })),
    doubleTap,
  );
}

/** 编辑器里可改键的整篇动作（W3b）的设置 key：编辑器取真值时只订这几项。 */
export const EDITOR_KEYED_SETTING_KEYS: readonly string[] = EDITOR_PALETTE_COMMANDS.flatMap((command) =>
  command.settingKey ? [command.settingKey] : [],
);

/** 一条编辑器整篇动作此刻的键值串（设置真值；没绑键 = `undefined`）。 */
export function editorPaletteShortcut(
  command: EditorPaletteCommand,
  bindings: ShortcutBindings,
): string | undefined {
  if (command.settingKey) return bindings[command.settingKey] || undefined;
  return command.editorShortcut ? EDITOR_SHORTCUTS[command.editorShortcut] : undefined;
}

/**
 * 编辑器整篇动作的按键判定（`⌘E` / `⌘J`，W3b）：与工作区命令同一套规则（含双击），
 * 返回命中的命令 id，由编辑器交给它自己的命令处理器执行——与命令面板触发同一条路。
 */
export function editorCommandForKey(
  event: KeyboardEvent,
  bindings: ShortcutBindings,
  doubleTap?: DoubleTapContext,
): EditorPaletteCommandId | null {
  return firedBinding(
    event,
    EDITOR_PALETTE_COMMANDS.map((command) => ({
      id: `editor:${command.id}`,
      value: editorPaletteShortcut(command, bindings),
      item: command.id,
    })),
    doubleTap,
  );
}

/**
 * 这一下按键命中了哪一条（工作区与编辑器共用的唯一判定）：普通绑定按一下即中；双击型绑定
 * （`double_tap:` 前缀）第一下只记账，时间窗内第二下才中；修饰键本身的 keydown 不算"夹了别的键"。
 */
function firedBinding<T>(
  event: KeyboardEvent,
  entries: readonly { id: string; value: string | undefined; item: T }[],
  doubleTap?: DoubleTapContext,
): T | null {
  for (const entry of entries) {
    if (!entry.value || !matchesShortcut(event, entry.value)) {
      continue;
    }
    if (!parseShortcutValue(entry.value).doubleTap) {
      return entry.item;
    }
    if (!doubleTap || doubleTap.windowMs === null) {
      return null;
    }
    return doubleTap.detector.record(entry.id, doubleTap.nowMs, doubleTap.windowMs) ? entry.item : null;
  }
  if (doubleTap && !isModifierKeyName(event.key)) {
    doubleTap.detector.reset();
  }
  return null;
}

function isModifierKeyName(key: string): boolean {
  return key === "Meta" || key === "Control" || key === "Shift" || key === "Alt";
}

export function isEditorSaveShortcut(event: KeyboardEvent): boolean {
  return !event.isComposing && matchesShortcut(event, EDITOR_SHORTCUTS.save);
}

