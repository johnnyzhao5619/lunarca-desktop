// 笔记库主区「标签页 + 分屏」布局状态机（PRD-07-035/036，TDD-07 §4.1）。
// 纯逻辑零 React/零 IPC：WorkspaceShell 持有状态并渲染，本模块只做不可变变换，
// 便于无头测试锁定交互语义（Obsidian/VS Code 对标）。
//
// 语义要点：
// - 默认「点击文件 = 复用当前标签」（Obsidian 心智），Cmd/中键 = 新标签；
// - 每个 pane 独立标签栈；分屏 = 克隆当前标签到新 pane；
// - 最后一个 pane 永远存在，关到只剩空标签（「新标签页」空态）；
// - 视图模式（Live Preview / 源码 / 阅读）是**标签级**状态，随标签持久化。

import type { EditorViewModeDto, WorkspaceLayoutDto } from "../../ipc/bindings";

export type EditorViewMode = EditorViewModeDto;

export const DEFAULT_VIEW_MODE: EditorViewMode = "live";

/** 分屏上限：超过后编辑区过窄失去可用性（VS Code 同为软约束）。 */
export const MAX_PANES = 3;

export interface NoteTab {
  id: number;
  /** Vault 相对路径；null = 空标签（尚未选择笔记）。 */
  path: string | null;
  mode: EditorViewMode;
  /**
   * 预览标签（PRD-07-035 2026-10-05 修订）：浏览时打开、下一次浏览会被复用；编辑、双击、拖动
   * 或 ⌘S 后转为常驻。同一 pane 至多一个；重启恢复一律按常驻（不入布局快照）。
   */
  preview?: boolean;
  /**
   * 固定标签（PRD-07-035 W2b）：「关闭其他 / 关闭右侧」与中键都不关它，打开别的笔记也不会
   * 把它换掉（落到右侧新标签）。随布局快照持久化。固定即常驻，空标签不能固定。
   */
  pinned?: boolean;
}

/**
 * 一条「刚关闭的标签」记录（⌘⇧T 重开，PRD-07-035 W2b）：关在哪个 pane 的第几位。
 * 只在本次运行内存里（不入布局快照）——重启后「刚关闭」已无意义。
 */
export interface ClosedTab {
  path: string;
  mode: EditorViewMode;
  paneId: number;
  index: number;
}

/** 重开栈上限（VS Code / 浏览器同为有界栈；再早的关闭不值得占内存）。 */
export const MAX_CLOSED_TABS = 20;

/**
 * 一条导航记录（TDD-07 §4.1 的 `NavEntry`）：**目标 + 视图状态的引用，不镜像正文**。
 *
 * 正文由 Rust 现读现给；历史镜像正文的代价是快照随笔记增长，且与磁盘那一份必然分叉。
 */
export interface NavEntry {
  path: string;
  mode: EditorViewMode;
}

/**
 * 一个 pane 的导航历史（PRD-07-037，浏览器/IDE 心智）。
 *
 * `cursor` = `entries` 中「此刻正显示的那一条」的下标；空历史时为 0。
 * **不用 -1 当哨兵**：它与标签下标同为无符号量，负值会逼每个消费者各写一遍
 * 「-1 是什么意思」，而空列表本身已经说清了「没有可后退的」。
 */
export interface NavHistory {
  entries: NavEntry[];
  cursor: number;
}

export interface NotePane {
  id: number;
  tabs: NoteTab[];
  activeTabId: number;
  history: NavHistory;
}

export interface NotePanesState {
  panes: NotePane[];
  activePaneId: number;
  /** 单调递增 id 源（pane 与 tab 共用，保证全局唯一便于 DnD 寻址）。 */
  nextId: number;
  /** 刚关闭的标签，最近的在末尾（⌘⇧T 依次弹出）。 */
  closed: ClosedTab[];
}

export interface OpenNoteOptions {
  /** 目标 pane；缺省 = 当前活跃 pane。 */
  paneId?: number;
  /** true = 强制新标签打开（Cmd/中键点击）。 */
  newTab?: boolean;
  /**
   * 打开方式（PRD-07-035 2026-10-05 修订）：`preview` = 浏览（文件树单击、搜索结果、反链…），
   * `permanent` = 明确意图（快速切换器、正文链接、新建…）。缺省 = 「复用当前标签」
   * （「启用预览标签」关闭时两种方式都退回这一种）。
   */
  as?: "preview" | "permanent";
}

export function emptyHistory(): NavHistory {
  return { entries: [], cursor: 0 };
}

/**
 * 记下这一格此刻显示的是什么。
 *
 * 语义与浏览器逐条对齐：**在当前位置之后打开新目标 → 截断 cursor 之后并追加**
 * （前进栈就此作废，正如浏览器里点了新链接）。连续两次落到同一目标不重复记
 * ——那不是一次导航，用户按后退时期待的是"上一篇"而不是"同一篇"。
 */
function pushHistory(history: NavHistory, entry: NavEntry): NavHistory {
  const current = history.entries[history.cursor];
  if (current && current.path === entry.path) {
    // 同一目标只更新视图状态：模式变了不该新增一条历史。
    const entries = history.entries.map((item, index) =>
      index === history.cursor ? entry : item,
    );
    return { entries, cursor: history.cursor };
  }
  const kept =
    history.entries.length === 0
      ? []
      : history.entries.slice(0, history.cursor + 1);
  const entries = [...kept, entry];
  return { entries, cursor: entries.length - 1 };
}

/** 这一格此刻显示的笔记（空标签 → `null`，不入历史）。 */
function shownEntry(pane: NotePane): NavEntry | null {
  const tab = activeTabOf(pane);
  return tab && tab.path !== null ? { path: tab.path, mode: tab.mode } : null;
}

/** 把 pane 的历史推进到它此刻显示的东西上（所有会改变显示内容的动作末尾调用）。 */
function recordShown(pane: NotePane): NotePane {
  const entry = shownEntry(pane);
  return entry ? { ...pane, history: pushHistory(pane.history, entry) } : pane;
}

export function initialNotePanesState(): NotePanesState {
  return {
    panes: [
      {
        id: 1,
        tabs: [{ id: 2, path: null, mode: DEFAULT_VIEW_MODE }],
        activeTabId: 2,
        history: emptyHistory(),
      },
    ],
    activePaneId: 1,
    nextId: 3,
    closed: [],
  };
}

export function findPane(
  state: NotePanesState,
  paneId: number,
): NotePane | null {
  return state.panes.find((pane) => pane.id === paneId) ?? null;
}

export function activePane(state: NotePanesState): NotePane {
  return findPane(state, state.activePaneId) ?? state.panes[0];
}

export function activeTabOf(pane: NotePane): NoteTab | null {
  return pane.tabs.find((tab) => tab.id === pane.activeTabId) ?? null;
}

export function activeTab(state: NotePanesState): NoteTab | null {
  return activeTabOf(activePane(state));
}

function replacePane(state: NotePanesState, next: NotePane): NotePanesState {
  return {
    ...state,
    panes: state.panes.map((pane) => (pane.id === next.id ? next : pane)),
  };
}

/** 在 `pane` 里把这一个标签设为活跃（顺带把整个状态的活跃 pane 切过去）。 */
function focusTab(state: NotePanesState, pane: NotePane, tab: NoteTab, permanent: boolean): NotePanesState {
  const tabs = permanent && tab.preview
    ? pane.tabs.map((item) => (item.id === tab.id ? { ...item, preview: false } : item))
    : pane.tabs;
  return {
    ...replacePane(state, recordShown({ ...pane, tabs, activeTabId: tab.id })),
    activePaneId: pane.id,
  };
}

/** 在活跃标签右侧插入一个新标签并聚焦。 */
function insertTab(
  state: NotePanesState,
  pane: NotePane,
  path: string,
  preview: boolean,
): NotePanesState {
  const active = activeTabOf(pane);
  const tab: NoteTab = { id: state.nextId, path, mode: DEFAULT_VIEW_MODE, ...(preview ? { preview } : {}) };
  const at = active ? pane.tabs.indexOf(active) + 1 : pane.tabs.length;
  const tabs = [...pane.tabs.slice(0, at), tab, ...pane.tabs.slice(at)];
  return {
    ...replacePane(state, recordShown({ ...pane, tabs, activeTabId: tab.id })),
    activePaneId: pane.id,
    nextId: state.nextId + 1,
  };
}

/** 把某个标签原地换成 `path`（并设好预览 / 常驻），聚焦它。 */
function replaceTab(
  state: NotePanesState,
  pane: NotePane,
  target: NoteTab,
  path: string,
  preview: boolean,
): NotePanesState {
  const tabs = pane.tabs.map((tab) => {
    if (tab.id !== target.id) return tab;
    const next: NoteTab = { id: tab.id, path, mode: tab.mode };
    return preview ? { ...next, preview } : next;
  });
  return {
    ...replacePane(state, recordShown({ ...pane, tabs, activeTabId: target.id })),
    activePaneId: pane.id,
  };
}

/**
 * 打开笔记（PRD-07-035）。
 *
 * - 本 pane 已开同路径 → 聚焦（以常驻方式打开时，预览顺带转常驻）；
 * - 指定了打开方式且没指定 pane、目标只在别的 pane 开着 → 聚焦那个 pane，不在这里复制一份；
 * - `newTab` → 右侧新常驻标签；
 * - `preview` → 复用本 pane 的预览标签；没有就把空标签用作预览，再没有就右侧新开一个预览；
 * - `permanent` → 当前标签是预览或空标签就原地换成常驻，否则右侧新开常驻；
 * - 缺省 → 复用活跃标签（「启用预览标签」关闭时的行为，也是此前的行为）；
 * - 任何方式都不替换**固定标签**：活跃标签是固定的就在它右侧新开（固定标签不会是预览或空标签，
 *   所以只有「缺省」这一支要判）。
 */
export function openNote(
  state: NotePanesState,
  path: string,
  options: OpenNoteOptions = {},
): NotePanesState {
  const pane =
    findPane(state, options.paneId ?? state.activePaneId) ?? activePane(state);
  const permanent = options.as !== "preview";
  const existing = pane.tabs.find((tab) => tab.path === path);
  if (existing) {
    return focusTab(state, pane, existing, permanent && options.as !== undefined);
  }
  if (options.as && options.paneId === undefined && !options.newTab) {
    for (const other of state.panes) {
      const tab = other.tabs.find((item) => item.path === path);
      if (tab) return focusTab(state, other, tab, permanent);
    }
  }
  const active = activeTabOf(pane);
  if (options.newTab || !active) {
    return insertTab(state, pane, path, false);
  }
  if (options.as === "preview") {
    const previewTab = pane.tabs.find((tab) => tab.preview);
    if (previewTab) return replaceTab(state, pane, previewTab, path, true);
    if (active.path === null) return replaceTab(state, pane, active, path, true);
    return insertTab(state, pane, path, true);
  }
  if (options.as === "permanent") {
    if (active.preview || active.path === null) return replaceTab(state, pane, active, path, false);
    return insertTab(state, pane, path, false);
  }
  if (active.pinned) return insertTab(state, pane, path, false);
  return replaceTab(state, pane, active, path, Boolean(active.preview));
}

/** 预览标签转为常驻（开始编辑、双击标签或文件树行、⌘S，PRD-07-035）。已是常驻则不变。 */
export function keepTab(state: NotePanesState, paneId: number, tabId: number): NotePanesState {
  const pane = findPane(state, paneId);
  const tab = pane?.tabs.find((item) => item.id === tabId);
  if (!pane || !tab?.preview) return state;
  return replacePane(state, {
    ...pane,
    tabs: pane.tabs.map((item) => (item.id === tabId ? { ...item, preview: false } : item)),
  });
}

/** 固定 / 取消固定（PRD-07-035 W2b）。固定即常驻；空标签不能固定。 */
export function togglePinTab(state: NotePanesState, paneId: number, tabId: number): NotePanesState {
  const pane = findPane(state, paneId);
  const tab = pane?.tabs.find((item) => item.id === tabId);
  if (!pane || !tab || tab.path === null) return state;
  return replacePane(state, {
    ...pane,
    tabs: pane.tabs.map((item) => {
      if (item.id !== tabId) return item;
      const base: NoteTab = { id: item.id, path: item.path, mode: item.mode };
      return item.pinned ? base : { ...base, pinned: true };
    }),
  });
}

/** 把这些即将关闭的标签记进重开栈（空标签没有可重开的目标，不记）。 */
function rememberClosed(state: NotePanesState, pane: NotePane, closing: NoteTab[]): ClosedTab[] {
  const entries = closing
    .filter((tab) => tab.path !== null)
    .map((tab) => ({
      path: tab.path as string,
      mode: tab.mode,
      paneId: pane.id,
      index: pane.tabs.indexOf(tab),
    }));
  return [...state.closed, ...entries].slice(-MAX_CLOSED_TABS);
}

/**
 * 重开最近关闭的标签（⌘⇧T，PRD-07-035 W2b）：回到原 pane 的原位置（pane 已不在就落到活跃
 * pane 末尾），以常驻打开；它已在某处开着就聚焦那里。活跃标签是空标签时就地用掉它。
 * 栈空原样返回。
 */
export function reopenClosedTab(state: NotePanesState): NotePanesState {
  const entry = state.closed[state.closed.length - 1];
  if (!entry) return state;
  const rest: NotePanesState = { ...state, closed: state.closed.slice(0, -1) };
  for (const pane of rest.panes) {
    const open = pane.tabs.find((tab) => tab.path === entry.path);
    if (open) return focusTab(rest, pane, open, true);
  }
  const pane = findPane(rest, entry.paneId) ?? activePane(rest);
  const active = activeTabOf(pane);
  const tab: NoteTab = { id: rest.nextId, path: entry.path, mode: entry.mode };
  if (active && active.path === null) {
    return replaceTab(rest, pane, active, entry.path, false);
  }
  const at = pane.id === entry.paneId ? Math.min(entry.index, pane.tabs.length) : pane.tabs.length;
  const tabs = [...pane.tabs.slice(0, at), tab, ...pane.tabs.slice(at)];
  return {
    ...replacePane(rest, recordShown({ ...pane, tabs, activeTabId: tab.id })),
    activePaneId: pane.id,
    nextId: rest.nextId + 1,
  };
}

/** 追加空标签（「新标签页」）并聚焦。 */
export function openEmptyTab(
  state: NotePanesState,
  paneId?: number,
): NotePanesState {
  const pane =
    findPane(state, paneId ?? state.activePaneId) ?? activePane(state);
  const tab: NoteTab = {
    id: state.nextId,
    path: null,
    mode: DEFAULT_VIEW_MODE,
  };
  // 空标签不入历史（它没有目标）；历史保持在上一条真实目标上，后退因此仍然可用。
  return {
    ...replacePane(state, {
      ...pane,
      tabs: [...pane.tabs, tab],
      activeTabId: tab.id,
    }),
    activePaneId: pane.id,
    nextId: state.nextId + 1,
  };
}

/** 空 pane 规整：多 pane 时移除空者；仅剩单 pane 时保底一个空标签。 */
function normalize(state: NotePanesState): NotePanesState {
  let panes = state.panes.filter((pane) => pane.tabs.length > 0);
  let nextId = state.nextId;
  if (panes.length === 0) {
    panes = [
      {
        id: nextId,
        tabs: [{ id: nextId + 1, path: null, mode: DEFAULT_VIEW_MODE }],
        activeTabId: nextId + 1,
        history: emptyHistory(),
      },
    ];
    nextId += 2;
  }
  const activePaneId = panes.some((pane) => pane.id === state.activePaneId)
    ? state.activePaneId
    : panes[panes.length - 1].id;
  return { ...state, panes, activePaneId, nextId };
}

/** 关闭标签并记进重开栈（关闭按钮、⌘W 类入口；对固定标签同样有效——显式关闭就是要关）。 */
export function closeTab(
  state: NotePanesState,
  paneId: number,
  tabId: number,
): NotePanesState {
  const pane = findPane(state, paneId);
  const tab = pane?.tabs.find((item) => item.id === tabId);
  if (!pane || !tab) return state;
  return removeTab({ ...state, closed: rememberClosed(state, pane, [tab]) }, paneId, tabId);
}

/** 把标签移出主区但**不**记进重开栈（移到独立窗口：它没被关掉，只是换了地方开）。 */
export function removeTab(
  state: NotePanesState,
  paneId: number,
  tabId: number,
): NotePanesState {
  const pane = findPane(state, paneId);
  if (!pane) {
    return state;
  }
  const index = pane.tabs.findIndex((tab) => tab.id === tabId);
  if (index === -1) {
    return state;
  }
  const tabs = pane.tabs.filter((tab) => tab.id !== tabId);
  // 关闭活跃标签 → 邻位接替（优先右邻，无则左邻，IDE 惯例）。
  const activeTabId =
    pane.activeTabId === tabId
      ? ((tabs[index] ?? tabs[index - 1])?.id ?? pane.activeTabId)
      : pane.activeTabId;
  return normalize(replacePane(state, { ...pane, tabs, activeTabId }));
}

export function activateTab(
  state: NotePanesState,
  paneId: number,
  tabId: number,
): NotePanesState {
  const pane = findPane(state, paneId);
  if (!pane || !pane.tabs.some((tab) => tab.id === tabId)) {
    return state;
  }
  // 切标签同样改变了这一格显示的东西——IDE 的后退会退回上一个标签，正是这条。
  return {
    ...replacePane(state, recordShown({ ...pane, activeTabId: tabId })),
    activePaneId: paneId,
  };
}

export function setActivePane(
  state: NotePanesState,
  paneId: number,
): NotePanesState {
  return findPane(state, paneId) ? { ...state, activePaneId: paneId } : state;
}

/** 视图模式为标签级状态（PRD-06-017）：同一笔记可在不同标签用不同模式并排看。 */
export function setTabMode(
  state: NotePanesState,
  paneId: number,
  tabId: number,
  mode: EditorViewMode,
): NotePanesState {
  const pane = findPane(state, paneId);
  if (!pane) {
    return state;
  }
  // 只在改的是**正显示的那个**标签时同步历史条目：后台标签换模式不是一次导航，
  // 而不同步的话，后退回来会把用户刚设的模式撤掉。
  const next: NotePane = {
    ...pane,
    tabs: pane.tabs.map((tab) => (tab.id === tabId ? { ...tab, mode } : tab)),
  };
  return replacePane(
    state,
    tabId === pane.activeTabId ? recordShown(next) : next,
  );
}

/** 分屏（PRD-07-036）：克隆活跃标签到右侧新 pane 并聚焦；到上限则原样返回。 */
export function splitPane(
  state: NotePanesState,
  paneId: number,
): NotePanesState {
  const pane = findPane(state, paneId);
  if (!pane || state.panes.length >= MAX_PANES) {
    return state;
  }
  const source = activeTabOf(pane);
  const newPane: NotePane = {
    id: state.nextId,
    tabs: [
      {
        id: state.nextId + 1,
        path: source?.path ?? null,
        mode: source?.mode ?? DEFAULT_VIEW_MODE,
      },
    ],
    activeTabId: state.nextId + 1,
    // 新 pane 的历史从它被克隆出来的那一条起步——它此刻显示的就是那一篇。
    history: emptyHistory(),
  };
  const at = state.panes.findIndex((p) => p.id === paneId) + 1;
  return {
    panes: [
      ...state.panes.slice(0, at),
      recordShown(newPane),
      ...state.panes.slice(at),
    ],
    activePaneId: newPane.id,
    nextId: state.nextId + 2,
    closed: state.closed,
  };
}

export function closePane(
  state: NotePanesState,
  paneId: number,
): NotePanesState {
  const pane = findPane(state, paneId);
  if (state.panes.length <= 1 || !pane) {
    return state;
  }
  return normalize({
    ...state,
    panes: state.panes.filter((item) => item.id !== paneId),
    closed: rememberClosed(state, pane, pane.tabs),
  });
}

/** 这一格能不能后退 / 前进（按钮禁用态的**唯一**判据，界面不自己数下标）。 */
export function canGoBack(pane: NotePane): boolean {
  return pane.history.cursor > 0;
}

export function canGoForward(pane: NotePane): boolean {
  return (
    pane.history.entries.length > 0 &&
    pane.history.cursor < pane.history.entries.length - 1
  );
}

/**
 * 把这一格导航到历史里的某一条。**移动游标，不追加**——否则后退一次就把前进栈毁了。
 *
 * 落位与普通打开同一条规则：pane 内已开同路径 → 聚焦那个标签；否则复用活跃标签。
 * 视图模式一并恢复（`NavEntry` 的 `viewState` 就是它）——历史记的是「当时看到的样子」。
 */
function travel(
  state: NotePanesState,
  paneId: number,
  nextCursor: number,
): NotePanesState {
  const pane = findPane(state, paneId);
  if (!pane) {
    return state;
  }
  const entry = pane.history.entries[nextCursor];
  if (!entry) {
    return state;
  }
  const existing = pane.tabs.find((tab) => tab.path === entry.path);
  const target = existing ?? activeTabOf(pane);
  if (!target) {
    return state;
  }
  if (!existing && target.pinned) {
    // 固定标签不被后退 / 前进换掉：这一条在它右侧新开，游标照常移动。
    const tab: NoteTab = { id: state.nextId, path: entry.path, mode: entry.mode };
    const at = pane.tabs.indexOf(target) + 1;
    return {
      ...replacePane(state, {
        ...pane,
        tabs: [...pane.tabs.slice(0, at), tab, ...pane.tabs.slice(at)],
        activeTabId: tab.id,
        history: { ...pane.history, cursor: nextCursor },
      }),
      activePaneId: paneId,
      nextId: state.nextId + 1,
    };
  }
  const tabs = pane.tabs.map((tab) =>
    tab.id === target.id ? { ...tab, path: entry.path, mode: entry.mode } : tab,
  );
  return {
    ...replacePane(state, {
      ...pane,
      tabs,
      activeTabId: target.id,
      history: { ...pane.history, cursor: nextCursor },
    }),
    activePaneId: paneId,
  };
}

/** 后退（PRD-07-037）。已在最早一条即原样返回。 */
export function goBack(state: NotePanesState, paneId: number): NotePanesState {
  const pane = findPane(state, paneId);
  return pane && canGoBack(pane)
    ? travel(state, paneId, pane.history.cursor - 1)
    : state;
}

/** 前进（PRD-07-037）。已在最新一条即原样返回。 */
export function goForward(
  state: NotePanesState,
  paneId: number,
): NotePanesState {
  const pane = findPane(state, paneId);
  return pane && canGoForward(pane)
    ? travel(state, paneId, pane.history.cursor + 1)
    : state;
}

export interface TabAddress {
  paneId: number;
  tabId: number;
}

/** 拖拽重排/跨 pane 移动：把 `from` 标签插到目标 pane 的 `index` 处并聚焦。 */
export function moveTab(
  state: NotePanesState,
  from: TabAddress,
  to: { paneId: number; index: number },
): NotePanesState {
  const sourcePane = findPane(state, from.paneId);
  const tab = sourcePane?.tabs.find((item) => item.id === from.tabId);
  if (!sourcePane || !tab || !findPane(state, to.paneId)) {
    return state;
  }
  const sourceIndex = sourcePane.tabs.indexOf(tab);
  let panes = state.panes.map((pane) => {
    if (pane.id !== from.paneId) {
      return pane;
    }
    const tabs = pane.tabs.filter((item) => item.id !== from.tabId);
    const activeTabId =
      pane.activeTabId === from.tabId
        ? ((tabs[sourceIndex] ?? tabs[sourceIndex - 1])?.id ?? pane.activeTabId)
        : pane.activeTabId;
    return { ...pane, tabs, activeTabId };
  });
  panes = panes.map((pane) => {
    if (pane.id !== to.paneId) {
      return pane;
    }
    // 同 pane 内移动：源已被移除，插入下标夹回现有范围。
    const index = Math.max(0, Math.min(to.index, pane.tabs.length));
    return {
      ...pane,
      // 拖动即表示「要留着它」：预览转常驻（PRD-07-035），也免得一个 pane 里出现两个预览。
      tabs: [...pane.tabs.slice(0, index), { ...tab, preview: false }, ...pane.tabs.slice(index)],
      activeTabId: tab.id,
    };
  });
  return normalize({ ...state, panes, activePaneId: to.paneId });
}

/** 重命名/移动后的路径重映射（含目录前缀），保持所有 pane 中打开的标签指向新路径。 */
export function remapPath(
  state: NotePanesState,
  from: string,
  to: string,
): NotePanesState {
  const map = (path: string | null): string | null => {
    if (path === null) {
      return null;
    }
    if (path === from) {
      return to;
    }
    if (path.startsWith(`${from}/`)) {
      return to + path.slice(from.length);
    }
    return path;
  };
  return {
    ...state,
    panes: state.panes.map((pane) => ({
      ...pane,
      tabs: pane.tabs.map((tab) => ({ ...tab, path: map(tab.path) })),
      // 历史一并改名：不改的话，重命名之后按后退会打开一个不存在的路径，
      // 而用户读到的是「后退坏了」。
      history: {
        ...pane.history,
        entries: pane.history.entries.map((entry) => ({
          ...entry,
          path: map(entry.path) as string,
        })),
      },
    })),
    closed: state.closed.map((entry) => ({ ...entry, path: map(entry.path) as string })),
  };
}

/** 删除后收口：关闭指向该路径（或该目录下）的所有标签。 */
export function closeTabsUnder(
  state: NotePanesState,
  path: string,
): NotePanesState {
  const gone = (tabPath: string | null): boolean =>
    tabPath !== null && (tabPath === path || tabPath.startsWith(`${path}/`));
  const panes = state.panes.map((pane) => {
    const tabs = pane.tabs.filter((tab) => !gone(tab.path));
    // 历史同剪（与 Rust 加载端同一口径）：留着一条指向已删除笔记的记录，
    // 按下去只会打开一个空白标签。游标按**对象身份**跟随，不按下标夹回。
    const at = pane.history.entries[pane.history.cursor]?.path ?? null;
    const entries = pane.history.entries.filter((entry) => !gone(entry.path));
    const cursor =
      entries.length === 0
        ? 0
        : Math.max(
            0,
            at === null
              ? entries.length - 1
              : entries.findIndex((entry) => entry.path === at),
          );
    const history: NavHistory = { entries, cursor };
    if (
      tabs.length === pane.tabs.length &&
      entries.length === pane.history.entries.length
    ) {
      return pane;
    }
    const activeTabId = tabs.some((tab) => tab.id === pane.activeTabId)
      ? pane.activeTabId
      : (tabs[tabs.length - 1]?.id ?? pane.activeTabId);
    return { ...pane, tabs, activeTabId, history };
  });
  // 删掉的笔记不该还能被 ⌘⇧T 重开出一个空白标签。
  const closed = state.closed.filter((entry) => !gone(entry.path));
  return normalize({ ...state, panes, closed });
}

/** 关闭除目标标签外的其余所有标签（IDE / Obsidian 对标）。 */
export function closeOtherTabs(
  state: NotePanesState,
  paneId: number,
  tabId: number,
): NotePanesState {
  const pane = findPane(state, paneId);
  if (!pane || !pane.tabs.some((tab) => tab.id === tabId)) {
    return state;
  }
  // 固定标签不随「关闭其他」关掉（PRD-07-035 W2b）。
  const tabs = pane.tabs.filter((tab) => tab.id === tabId || tab.pinned);
  const closing = pane.tabs.filter((tab) => !tabs.includes(tab));
  return normalize(
    replacePane(
      { ...state, closed: rememberClosed(state, pane, closing) },
      { ...pane, tabs, activeTabId: tabId },
    ),
  );
}

/** 关闭目标标签右侧的所有标签（IDE / Obsidian 对标）。 */
export function closeRightTabs(
  state: NotePanesState,
  paneId: number,
  tabId: number,
): NotePanesState {
  const pane = findPane(state, paneId);
  if (!pane) {
    return state;
  }
  const index = pane.tabs.findIndex((tab) => tab.id === tabId);
  if (index === -1) {
    return state;
  }
  // 右侧的固定标签留着（PRD-07-035 W2b）。
  const tabs = pane.tabs.filter((tab, at) => at <= index || tab.pinned);
  const closing = pane.tabs.filter((tab) => !tabs.includes(tab));
  const activeTabId = tabs.some((tab) => tab.id === pane.activeTabId)
    ? pane.activeTabId
    : tabId;
  return normalize(
    replacePane(
      { ...state, closed: rememberClosed(state, pane, closing) },
      { ...pane, tabs, activeTabId },
    ),
  );
}

/** 布局快照的 panes 部分（本模块职责边界；树列宽/折叠等工作区级字段由 Shell 组装）。 */
export type NotePanesLayout = Pick<WorkspaceLayoutDto, "panes" | "active_pane">;

/** 布局快照（PRD-07-035 重启恢复）：空标签不入快照；无实体标签返回 null（不写盘）。 */
export function serializeNotePanes(
  state: NotePanesState,
): NotePanesLayout | null {
  const panes = state.panes
    .map((pane) => {
      const tabs = pane.tabs.filter((tab) => tab.path !== null);
      const active = activeTabOf(pane);
      const activeIndex = active ? tabs.indexOf(active) : -1;
      return {
        tabs: tabs.map((tab) => ({
          path: tab.path as string,
          mode: tab.mode,
          pinned: Boolean(tab.pinned),
        })),
        active_tab: Math.max(0, activeIndex),
        // 历史属布局快照的一部分（TDD-07 §4.1）：重启后「后退」仍指向上次那几篇。
        history: pane.history.entries.map((entry) => ({
          path: entry.path,
          mode: entry.mode,
        })),
        history_cursor: pane.history.cursor,
      };
    })
    .filter((pane) => pane.tabs.length > 0);
  if (panes.length === 0) {
    return null;
  }
  const activeIndex = state.panes
    .filter((pane) => pane.tabs.some((tab) => tab.path !== null))
    .findIndex((pane) => pane.id === state.activePaneId);
  return { panes, active_pane: Math.max(0, activeIndex) };
}

/** 从快照恢复（Rust 加载端已剪枝失效路径；此处仅做下标夹回与 id 重建）。 */
export function restoreNotePanes(layout: NotePanesLayout): NotePanesState {
  let nextId = 1;
  const panes: NotePane[] = layout.panes
    .filter((pane) => pane.tabs.length > 0)
    .map((pane) => {
      const paneId = nextId;
      nextId += 1;
      const tabs: NoteTab[] = pane.tabs.map((tab) => {
        const id = nextId;
        nextId += 1;
        return { id, path: tab.path, mode: tab.mode, ...(tab.pinned ? { pinned: true } : {}) };
      });
      const activeIndex = Math.min(pane.active_tab, tabs.length - 1);
      // Rust 加载端已剪过失效路径并让游标跟随身份；此处只夹回下标。
      // `history` / `history_cursor` 在 wire 上是可选的（没有历史的 pane 写出来就是空）。
      const entries: NavEntry[] = (pane.history ?? []).map((entry) => ({
        path: entry.path,
        mode: entry.mode,
      }));
      const cursor =
        entries.length === 0
          ? 0
          : Math.min(pane.history_cursor ?? 0, entries.length - 1);
      return {
        id: paneId,
        tabs,
        activeTabId: tabs[activeIndex].id,
        history: { entries, cursor },
      };
    });
  if (panes.length === 0) {
    return initialNotePanesState();
  }
  const activePaneIndex = Math.min(layout.active_pane, panes.length - 1);
  return { panes, activePaneId: panes[activePaneIndex].id, nextId, closed: [] };
}
