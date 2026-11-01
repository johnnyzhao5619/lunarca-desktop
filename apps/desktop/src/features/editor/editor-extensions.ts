import { history, historyField, indentLess, indentMore } from "@codemirror/commands";
import { foldedRanges } from "@codemirror/language";
import { markdownLanguage } from "@codemirror/lang-markdown";
import { openSearchPanel } from "@codemirror/search";
import {
  type Compartment,
  EditorSelection,
  StateEffect,
  StateField,
  type EditorState,
  type Extension,
  type Text,
} from "@codemirror/state";
import {
  Decoration,
  type DecorationSet,
  EditorView,
  gutter,
  GutterMarker,
  keymap,
  WidgetType,
} from "@codemirror/view";
import { codeMirrorKey, editorShortcut } from "../command/command-registry";
import { toggleTabFocusMode } from "../../lib/tab-focus-mode";
import { isInsideFencedCode } from "./code-fence-editing";
import { parentIndentOf, parseListLine, splitQuote, withRenumber } from "./list-editing";
import { singleCursorOnly } from "./multi-cursor";
import { PropertiesPanel } from "./PropertiesPanel";
import { createElement } from "react";
import { createRoot, type Root } from "react-dom/client";

export interface ListContinuation {
  from: number;
  to: number;
  insert: string;
  cursor: number;
}

/** 各选区合计的字数（多光标时相加，B6）；没有选中任何文字返回 null。 */
export function selectionWordCount(state: EditorState): number | null {
  if (state.selection.ranges.every((range) => range.empty)) return null;
  return state.selection.ranges.reduce((sum, range) => sum + wordCount(state.sliceDoc(range.from, range.to)), 0);
}

export function wordCount(text: string) {
  const cjkMatches = text.match(/[\u3400-\u9fff\uf900-\ufaff]/gu);
  const latinMatches = text.match(/[A-Za-z0-9]+(?:['-][A-Za-z0-9]+)*/gu);
  return (cjkMatches?.length ?? 0) + (latinMatches?.length ?? 0);
}

export function listContinuationForState(state: EditorState): ListContinuation | null {
  const selection = state.selection.main;
  if (!selection.empty) {
    return null;
  }
  // 代码内容必须原样输入；Markdown 列表续写不能劫持以 `-`/`1.` 开头的代码行。
  if (isInsideFencedCode(state, selection.head)) {
    return null;
  }

  const line = state.doc.lineAt(selection.head);
  const linePrefix = line.text.slice(0, selection.head - line.from);
  const item = parseListLine(linePrefix);
  if (!item) {
    // 引用块（此前由 lang-markdown 自带键位承担，见 list-editing.ts 抬头）：
    // `> 文字|` 回车续一行 `> `；空的 `> |` 回车退出引用。
    const { quote, rest } = splitQuote(linePrefix);
    if (quote.length === 0) {
      return null;
    }
    if (rest.trim() === "") {
      return { from: line.from, to: selection.head, insert: "", cursor: line.from };
    }
    const insert = `\n${quote}`;
    return {
      from: selection.head,
      to: selection.head,
      insert,
      cursor: selection.head + insert.length,
    };
  }
  const lineBody = line.from + item.quote.length;
  if (item.body.length === 0) {
    // 嵌套的空项：回车降一级而不是退出列表（Notion / Obsidian 同）——用户多半是想
    // 回到父级继续写；降到顶层再按一次才真正退出。
    if (item.indent.length > 0) {
      const step = item.indent.length - parentIndentOf(state, line.number, item.indent.length);
      return {
        from: lineBody,
        to: lineBody + step,
        insert: "",
        cursor: selection.head - step,
      };
    }
    // 引用里的顶层空项：去掉标记、留在引用里（`> - |` → `> |`）。
    if (item.quote.length > 0) {
      return { from: lineBody, to: selection.head, insert: "", cursor: lineBody };
    }
    // 顶层空项：去掉标记并换行——留下的那条空行是刻意的，`- a\n正文` 里的正文是
    // 上一项的懒续行（CommonMark），中间没有空行就退不出列表。
    return {
      from: line.from,
      to: selection.head,
      insert: "\n",
      cursor: line.from + 1,
    };
  }

  const marker =
    item.number === null ? item.marker : `${item.number + 1}${item.marker.slice(-1)}`;
  const insert = `\n${item.quote}${item.indent}${marker} ${item.task}`;
  return {
    from: selection.head,
    to: selection.head,
    insert,
    cursor: selection.head + insert.length,
  };
}

function continueList(view: EditorView) {
  const continuation = listContinuationForState(view.state);
  if (!continuation) {
    return false;
  }

  // 在有序列表中间插项 / 嵌套空项降级之后，后续编号跟着走（与插项合成一个撤销步）。
  view.dispatch(
    withRenumber(
      view.state,
      { from: continuation.from, to: continuation.to, insert: continuation.insert },
      EditorSelection.single(continuation.cursor),
    ),
  );
  return true;
}

// ─── Cue line highlight (playback sidecar) ───────────────────────────────────

/** Dispatch this effect with a 0-based line number (or null to clear). */
export const highlightCueLineEffect = StateEffect.define<number | null>();

const cueLineMark = Decoration.line({ class: "cm-cue-active" });

/** StateField that tracks which 0-based line is highlighted for cue sync. */
const cueLineDecorationField = StateField.define<DecorationSet>({
  create() {
    return Decoration.none;
  },
  update(deco, tr) {
    deco = deco.map(tr.changes);
    for (const effect of tr.effects) {
      if (effect.is(highlightCueLineEffect)) {
        const lineNo = effect.value;
        if (lineNo === null) {
          deco = Decoration.none;
        } else {
          const line = tr.state.doc.line(Math.max(1, Math.min(lineNo + 1, tr.state.doc.lines)));
          deco = Decoration.set([cueLineMark.range(line.from)]);
        }
      }
    }
    return deco;
  },
  provide(f) {
    return EditorView.decorations.from(f);
  },
});

/** Provides CM6 decoration for cue line highlighting. Include in editor extensions. */
export function cueLineHighlightExtension(): Extension {
  return [
    cueLineDecorationField,
    EditorView.baseTheme({
      ".cm-cue-active": {
        // 走语义 Token（TDD-15 §3.2 禁硬编码色）：此前这里是两个**没有定义点**的变量名
        // 加两抹写死的黄兜底——于是"当前行是什么颜色"既不随主题也不随配色族。
        backgroundColor: "var(--editor-cue-active-surface)",
        boxShadow: "inset 2px 0 0 0 var(--editor-cue-active-border)",
      },
    }),
  ];
}

// ─── 时间戳行：就地起播（PRD-04-066 / ADR-118）─────────────────────────────────

/**
 * 正文里带时间戳的行 → 会话绝对毫秒。**表由 Rust 给**（`playback_timecode_lines`），
 * 前端既不解析 `[HH:MM:SS.mmm]` 的格式，也不自己数行（ADR-003）。
 */
export type TimecodeLineMap = ReadonlyMap<number, number>;

/** 派发它更新本编辑器认得的时间戳行表；空表 = 这一篇没有可听的行。 */
export const setTimecodeLinesEffect = StateEffect.define<TimecodeLineMap>();

const timecodeLinesField = StateField.define<TimecodeLineMap>({
  create() {
    return new Map();
  },
  update(lines, tr) {
    for (const effect of tr.effects) {
      if (effect.is(setTimecodeLinesEffect)) return effect.value;
    }
    return lines;
  },
});

/**
 * 行表本身（**常驻**，与装订线分开）。
 *
 * 它不能跟着装订线一起进隔间：装订线是按"这一篇有没有时间戳行"动态装卸的，而
 * 判断依据正是这张表。字段与写它的 effect 若在同一次事务里才被创建，新字段拿到的是
 * `create()` 的空表，那条 effect 就此丢掉——于是永远装不上装订线。
 * 本扩展不渲染任何东西，常驻的代价是一个空 Map。
 */
export function timecodeLinesExtension(): Extension {
  return [timecodeLinesField];
}

/**
 * 行首那枚播放键。**常态不可见**：光标所在行常显、指针进入装订线那一列时全列淡显、
 * 悬停到键上回满（真机 2026-09-08：一整列淡三角被读成"意义不明的白条"）。
 */
class TimecodePlayMarker extends GutterMarker {
  constructor(
    private readonly title: string,
    active: boolean,
  ) {
    super();
    // 挂在装订线元素上而不是键上：CSS 据此让光标行的键常显。
    this.elementClass = active ? "cm-timecode-gutter__line--active" : "";
  }

  override toDOM(): HTMLElement {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "cm-timecode-play";
    button.title = this.title;
    button.setAttribute("aria-label", this.title);
    // 图标走内联 SVG 而不是字符 ▶：字形/基线/粗细随系统字体变，与整套图标对不齐。
    button.innerHTML =
      '<svg viewBox="0 0 24 24" width="11" height="11" aria-hidden="true">' +
      '<polygon points="6 3 20 12 6 21" fill="currentColor" /></svg>';
    return button;
  }
}

export interface TimecodeGutterOptions {
  /** 跳到这一毫秒并播放。 */
  onSeek: (ms: number) => void;
  /** 播放键的可访问名与提示（i18n 由调用方给，扩展里不出现文案）。 */
  playLabel: () => string;
}

/**
 * 时间戳行的就地起播（PRD-04-066）。
 *
 * 两条通路，**都不夺走编辑语义**：
 * - 行首装订线上的播放键：唯一的可见入口，点它 = 跳到这一句；
 * - `⌘/Ctrl + 点击`该行任意处：熟手通路。普通单击仍然只是放光标——转写正文是可编辑的，
 *   把裸单击改成 seek 会让"点进去改个错别字"变成"音频跳走"。
 *
 * 判据一律查**行表**（Rust 给的）：没有时间戳的行既没有键、`⌘` 点击也不响应。
 */
export function timecodeGutterExtension(options: TimecodeGutterOptions): Extension {
  // 行表由 `timecodeLinesExtension()` 常驻提供；本扩展只在"这一篇确有时间戳行"时装上。
  const msAt = (state: EditorState, pos: number): number | undefined =>
    state.field(timecodeLinesField).get(state.doc.lineAt(pos).number);

  return [
    gutter({
      class: "cm-timecode-gutter",
      lineMarker(view, line) {
        const ms = msAt(view.state, line.from);
        if (ms === undefined) return null;
        const head = view.state.selection.main.head;
        return new TimecodePlayMarker(options.playLabel(), head >= line.from && head <= line.to);
      },
      // 行表一变（换篇 / 二次转写落盘）或光标换行，装订线要跟着重画。
      lineMarkerChange(update) {
        return (
          update.selectionSet ||
          update.transactions.some((tr) =>
            tr.effects.some((effect) => effect.is(setTimecodeLinesEffect)),
          )
        );
      },
      domEventHandlers: {
        mousedown(view, line, event) {
          const ms = msAt(view.state, line.from);
          if (ms === undefined) return false;
          options.onSeek(ms);
          event.preventDefault();
          return true;
        },
      },
    }),
    EditorView.domEventHandlers({
      mousedown(event, view) {
        if (!(event.metaKey || event.ctrlKey)) return false;
        const pos = view.posAtCoords({ x: event.clientX, y: event.clientY });
        if (pos === null) return false;
        const ms = msAt(view.state, pos);
        if (ms === undefined) return false;
        options.onSeek(ms);
        event.preventDefault();
        return true;
      },
    }),
  ];
}

/**
 * Markdown 语言级括号配对声明（PRD-06-014，Typora 对标）：CM `closeBrackets`
 * 默认集缺**反引号**（markdown 行内代码高频），经语言 data 补齐。
 * 引号/反引号在**字词字符之后不自动配对**是 CM 既定策略（防 `don't`、所有格
 * 与中文语句内误配），非缺陷、不覆盖——在空白/行首输入即配对。
 */
export function markdownCloseBracketsData(): Extension {
  return markdownLanguage.data.of({
    closeBrackets: { brackets: ["(", "[", "{", "'", '"', "`"] },
  });
}

/**
 * 换篇时重建撤销历史。CM 的 history 没有"清空"效果——重建它的唯一办法是把装着 `history()`
 * 的 Compartment 先配成空、再配回来；两次 dispatch 都不改文档。装载本身不进历史
 * （`applyIncrementalText`），但上一篇里的编辑仍在，不清就能被"撤"进新的一篇。
 *
 * `restore`：切回一篇先前编辑过的笔记时，带回它离开时的历史（`saveEditorHistory`），切回来仍能
 * ⌘Z（Obsidian / Typora / VS Code 同；PRD-06-014 2026-10-03 修订）。
 */
export function resetEditorHistory(view: EditorView, compartment: Compartment, restore?: SavedEditorHistory): void {
  view.dispatch({ effects: compartment.reconfigure([]) });
  view.dispatch({
    effects: compartment.reconfigure(restore ? [history(), historyField.init(() => restore.history)] : history()),
  });
}

/** 一篇笔记离开时的撤销历史与光标；只在回来时正文**逐字相同**才能接上（历史记的是相对这份正文的改动）。 */
export interface SavedEditorHistory {
  doc: Text;
  history: unknown;
  anchor: number;
  head: number;
  /** 离开时折起的章节 / 列表项（B3）：正文没变，回来照样折着。 */
  folds: { from: number; to: number }[];
}

export function saveEditorHistory(state: EditorState): SavedEditorHistory | null {
  const saved = state.field(historyField, false);
  if (saved === undefined) return null;
  const { anchor, head } = state.selection.main;
  const folds: { from: number; to: number }[] = [];
  foldedRanges(state).between(0, state.doc.length, (from, to) => {
    folds.push({ from, to });
  });
  return { doc: state.doc, history: saved, anchor, head, folds };
}

/** 按笔记路径留最近几篇的历史：一个编辑器跨标签复用，标签切换就是换篇。 */
export class EditorHistoryCache {
  private readonly entries = new Map<string, SavedEditorHistory>();

  constructor(private readonly limit = 12) {}

  put(path: string, saved: SavedEditorHistory): void {
    this.entries.delete(path);
    this.entries.set(path, saved);
    while (this.entries.size > this.limit) {
      const oldest = this.entries.keys().next().value;
      if (oldest === undefined) break;
      this.entries.delete(oldest);
    }
  }

  /** 取出并移除：只有正文与离开时逐字相同才返回（被外部改过就从空历史开始）。 */
  take(path: string, doc: Text): SavedEditorHistory | null {
    const saved = this.entries.get(path);
    this.entries.delete(path);
    return saved && saved.doc.eq(doc) ? saved : null;
  }
}

export function editorProductivityExtensions(): Extension {
  return keymap.of([
    {
      key: "Enter",
      run: singleCursorOnly(continueList),
    },
    {
      key: "Tab",
      run: indentMore,
    },
    {
      key: "Shift-Tab",
      run: indentLess,
    },
    {
      key: "Mod-Alt-f",
      run: openSearchPanel,
    },
    // 「Tab 键移动焦点」：切会话级那一份。CM 默认键表里的 ⇧⌥M 只切本编辑器、与状态栏对不上，
    // 故同一处一并接管（键盘布局让它能按出来时也走同一份）。
    ...[codeMirrorKey(editorShortcut("tab_focus_mode")), "Shift-Alt-m"].map((key) => ({
      key,
      run: () => {
        toggleTabFocusMode();
        return true;
      },
    })),
  ]);
}

// ─── Frontmatter (YAML) Metadata Folding ─────────────────────────────────────

export const toggleFrontmatterEffect = StateEffect.define<void>();

export const frontmatterCollapsedField = StateField.define<boolean>({
  create() {
    return true; // Collapsed by default
  },
  update(value, tr) {
    for (const effect of tr.effects) {
      if (effect.is(toggleFrontmatterEffect)) {
        return !value;
      }
    }
    return value;
  },
});

/** 属性面板（ADR-139）里要回到编辑器宿主的动作。 */
export interface FrontmatterPanelOptions {
  /** 「在源码中编辑」：切到源码模式并把光标放到这一行（1 起）。 */
  onEditSource: (view: EditorView, line: number) => void;
}

/** frontmatter 折叠部件的 UI 文案（由调用方经 i18n 提供，扩展内不落任何字面文案）。 */
export interface FrontmatterLabels {
  /** 胶囊标签；`count` = 顶层属性数（折叠状态本身就该有信息量，不必展开才知道里面有几项）。 */
  label: (count: number) => string;
  expandTitle: string;
  collapseTitle: string;
  expandAction: string;
  collapseAction: string;
}

/** 胶囊那一行（折叠态是整个部件，展开态是面板的页头）。 */
function frontmatterToggleRow(view: EditorView, labels: FrontmatterLabels, collapsed: boolean, count: number): HTMLElement {
  const el = document.createElement("div");
  el.className = `luna-editor__frontmatter-toggle ${collapsed ? "collapsed" : "expanded"}`;
  el.setAttribute("role", "button");
  el.setAttribute("tabindex", "0");
  el.setAttribute("title", collapsed ? labels.expandTitle : labels.collapseTitle);

  // 刻意用字符而不是 SVG 图标：图标资产是 React 组件、装饰层是手写 DOM，两面各画一份
  // 必然漂移（标注块的同一裁决，TDD-11 §2.1）。折叠/展开态以 ▸ / ▾ 表达，与树形控件同一语言。
  const icon = document.createElement("span");
  icon.className = "luna-editor__frontmatter-toggle-icon";
  icon.textContent = collapsed ? "▸" : "▾";

  const label = document.createElement("span");
  label.className = "luna-editor__frontmatter-toggle-label";
  label.textContent = labels.label(count);

  const action = document.createElement("span");
  action.className = "luna-editor__frontmatter-toggle-action";
  action.textContent = collapsed ? labels.expandAction : labels.collapseAction;

  el.appendChild(icon);
  el.appendChild(label);
  el.appendChild(action);

  el.addEventListener("click", (e) => {
    e.preventDefault();
    view.dispatch({
      effects: toggleFrontmatterEffect.of(),
    });
  });
  return el;
}

class FrontmatterToggleWidget extends WidgetType {
  constructor(
    private readonly labels: FrontmatterLabels,
    private readonly count: number,
  ) {
    super();
  }

  override eq(other: FrontmatterToggleWidget) {
    return other.labels === this.labels && other.count === this.count;
  }

  toDOM(view: EditorView) {
    return frontmatterToggleRow(view, this.labels, true, this.count);
  }
}

/** 属性面板的宿主（ADR-139）：每个部件 DOM 一个 React root，`updateDOM` 原地重渲染而不重挂。 */
const panelRoots = new WeakMap<HTMLElement, Root>();

/**
 * 展开态：页头 + 属性面板（取代原始 YAML）。`source` 是 frontmatter 原文——变了就原地把新原文交给
 * 面板（`updateDOM`），而不是拆掉重建：重建会把正在输入的那个框的焦点和草稿一起丢掉。
 */
class FrontmatterPanelWidget extends WidgetType {
  constructor(
    private readonly labels: FrontmatterLabels,
    private readonly panel: FrontmatterPanelOptions,
    private readonly count: number,
    private readonly source: string,
  ) {
    super();
  }

  override eq(other: FrontmatterPanelWidget) {
    return other.labels === this.labels && other.count === this.count && other.source === this.source;
  }

  private render(view: EditorView, root: Root) {
    root.render(
      createElement(PropertiesPanel, {
        source: this.source,
        currentSource: () => {
          const range = getFrontmatterRange(view.state);
          return range ? view.state.sliceDoc(0, range.to) : null;
        },
        applyChange: (change) => {
          view.dispatch({ changes: change, userEvent: "input.property" });
        },
        onEditSource: (line) => this.panel.onEditSource(view, line),
        focusEditor: () => view.focus(),
      }),
    );
  }

  toDOM(view: EditorView) {
    const wrap = document.createElement("div");
    wrap.className = "luna-editor__frontmatter-panel";
    // 面板是表单不是正文：不让光标落进来，键盘与输入法事件归面板自己（`ignoreEvent`）。
    wrap.contentEditable = "false";
    wrap.appendChild(frontmatterToggleRow(view, this.labels, false, this.count));
    const host = document.createElement("div");
    wrap.appendChild(host);
    const root = createRoot(host);
    panelRoots.set(wrap, root);
    this.render(view, root);
    return wrap;
  }

  override updateDOM(dom: HTMLElement, view: EditorView) {
    const root = panelRoots.get(dom);
    if (!root || !dom.firstChild) return false;
    dom.replaceChild(frontmatterToggleRow(view, this.labels, false, this.count), dom.firstChild);
    this.render(view, root);
    return true;
  }

  override destroy(dom: HTMLElement) {
    const root = panelRoots.get(dom);
    panelRoots.delete(dom);
    // 与代码块复制按钮同一做法：CM 更新 DOM 时正处在事件处理里，推到微任务后再卸载 root。
    queueMicrotask(() => root?.unmount());
  }

  override ignoreEvent() {
    return true;
  }
}

/**
 * frontmatter 闭合 `---` 所在的行号（1 起）；没有 frontmatter 为 `null`。判据只此一处：首行是
 * `---`，其后第一行 `---` 闭合。编辑器区间与字数共用它，两边不会对「哪段算元数据」各说各话。
 */
function frontmatterCloseLine(lineText: (line: number) => string, lineCount: number): number | null {
  if (lineCount === 0 || lineText(1).trim() !== "---") {
    return null;
  }
  for (let line = 2; line <= lineCount; line++) {
    if (lineText(line).trim() === "---") {
      return line;
    }
  }
  return null;
}

/** frontmatter（文首 `---` … `---`）的文档区间；没有则 `null`。加载时光标据此落到正文首行。 */
export function getFrontmatterRange(state: EditorState): { from: number; to: number } | null {
  if (state.doc.length === 0) {
    return null;
  }
  const close = frontmatterCloseLine((line) => state.doc.line(line).text, state.doc.lines);
  return close === null ? null : { from: 0, to: state.doc.line(close).to };
}

/**
 * 一篇笔记的字数：只数正文，frontmatter 不算。状态栏与字数目标（W6b）都用它——此前整篇
 * 一起数，设个目标（`luna:` 块多一行）字数就凭空涨了几个（真机 2026-10-05）。
 */
export function noteWordCount(content: string): number {
  const lines = content.split("\n");
  const close = frontmatterCloseLine((line) => lines[line - 1] ?? "", lines.length);
  return wordCount(close === null ? content : lines.slice(close).join("\n"));
}

/**
 * frontmatter 的**属性**数：任意缩进层级的 `key:` 行都算，两类除外——① 列表项（`- x`，
 * 那是某个属性的值）；② **容器键**（冒号后没有值、且下一行是更深一级的键，如本仓库把
 * 一切都嵌在 `luna:` 下）——只数顶层键时每篇笔记永远是「1 项」，等于没数（真机 2026-09-02）。
 * 只用于胶囊上的计数，不解析 YAML；数错一两项的代价远小于引入一个 YAML 解析器。
 */
export function countFrontmatterKeys(state: EditorState, range: { from: number; to: number }): number {
  const startLine = state.doc.lineAt(range.from).number;
  const endLine = state.doc.lineAt(range.to).number;
  const lines: string[] = [];
  for (let i = startLine + 1; i < endLine; i++) {
    lines.push(state.doc.line(i).text);
  }
  return countPropertyLines(lines);
}

const KEY_LINE = /^(\s*)([A-Za-z0-9_.-]+)\s*:(.*)$/u;

/** 纯逻辑（无头测试直接喂行）：见 `countFrontmatterKeys`。 */
export function countPropertyLines(lines: readonly string[]): number {
  let count = 0;
  for (let i = 0; i < lines.length; i++) {
    const match = KEY_LINE.exec(lines[i]);
    if (!match) {
      continue;
    }
    const indent = match[1].length;
    const hasValue = match[3].trim().length > 0;
    if (!hasValue) {
      // 找下一行非空行：更深缩进的**键**行 → 本行是容器，不算；列表项 → 本行是列表属性，算。
      let next = i + 1;
      while (next < lines.length && lines[next].trim().length === 0) {
        next += 1;
      }
      const nextLine = next < lines.length ? lines[next] : "";
      const nextIndent = nextLine.length - nextLine.trimStart().length;
      const nextIsKey = KEY_LINE.test(nextLine) && !nextLine.trimStart().startsWith("-");
      if (nextIsKey && nextIndent > indent) {
        continue;
      }
    }
    count += 1;
  }
  return count;
}

function buildFrontmatterDecos(
  state: EditorState,
  collapsed: boolean,
  labels: FrontmatterLabels,
  panel: FrontmatterPanelOptions,
): DecorationSet {
  const range = getFrontmatterRange(state);
  if (!range) {
    return Decoration.none;
  }
  const count = countFrontmatterKeys(state, range);
  // 折叠：整段换成胶囊；展开：整段换成属性面板（原始 YAML 只在源码模式里看，ADR-139 ⑤）。
  const widget = collapsed
    ? new FrontmatterToggleWidget(labels, count)
    : new FrontmatterPanelWidget(labels, panel, count, state.sliceDoc(0, range.to));
  return Decoration.set([Decoration.replace({ widget, block: true }).range(0, range.to)]);
}

/**
 * 整行是否为 Lunarca 自己的**结构脚手架注释**（`<!-- luna:… -->` 独占一行）。
 *
 * **按 `luna:` 命名空间判定，刻意不复制具体哨兵串**。转写块的定界符是
 * `luna_vault::vault::TRANSCRIPT_BEGIN` / `TRANSCRIPT_END`，对话笔记的出处与补充标记在
 * `ipc::chat` 另有几个——把那几个常量抄一份到前端，就是给「同一个判据」再造一个所有者，
 * 而 `luna_vault::parse::is_scaffold_line` 的文档正是在警告这件事（两处口径分家的代价）。
 * 命名空间才是稳定契约：Rust 改某个哨兵叫什么，这条规则依然成立。
 *
 * **必须整行恰好是一个完整注释**：用户在正文里写 `见 <!-- luna:transcript --> 那一段` 是内容，
 * 不该被吞掉——与 Rust 侧「必须整行精确匹配」是同一条纪律。
 */
export function isLunaScaffoldComment(text: string): boolean {
  const trimmed = text.trim();
  if (!trimmed.startsWith("<!--") || !trimmed.endsWith("-->") || trimmed.length < 7) {
    return false;
  }
  // 注释内部不得再出现 `-->`：那意味着这一行有两个注释（或一个注释加正文），不是纯脚手架。
  const inner = trimmed.slice(4, -3);
  if (inner.includes("-->")) {
    return false;
  }
  const payload = inner.trim();
  return payload.startsWith("luna:") || payload.startsWith("/luna:");
}

export function buildScaffoldDecos(state: EditorState): DecorationSet {
  const decos = [];
  for (let i = 1; i <= state.doc.lines; i++) {
    const line = state.doc.line(i);
    if (isLunaScaffoldComment(line.text)) {
      // 与 frontmatter 折叠同一种范围写法（行首→行尾，`block`）：整行让位，不留占位文字。
      decos.push(Decoration.replace({ block: true }).range(line.from, line.to));
    }
  }
  return Decoration.set(decos, true);
}

/**
 * 实时预览里隐藏 Lunarca 的结构脚手架注释。
 *
 * Rust 侧把这些哨兵注释描述为「HTML 注释，渲染不可见」——那个假设**只在渲染视图成立**，
 * 而实时预览面同时也是编辑面，于是 `<!-- luna:transcript -->` 这行机器脚手架就直接摊在
 * 用户眼前（真机可见：任何一篇录音转写笔记的正文第一行）。
 *
 * **只在实时预览隐藏，源码模式照常显示**：本扩展与 frontmatter 折叠同处
 * `buildPresentationExtensions`。真要修一个坏掉的转写块时，总得有一个面能看见定界符；
 * 两个面都藏就没有退路了。
 *
 * **文件字节一个都不动**：这纯粹是渲染层的事。哨兵仍旧照写、照旧被 Rust 用来整块替换
 * 转写正文——它们是承重的，不是可以清理掉的历史垃圾。
 */
export function lunaScaffoldHidingExtension(): Extension {
  return StateField.define<DecorationSet>({
    create(state) {
      return buildScaffoldDecos(state);
    },
    update(decos, tr) {
      if (!tr.docChanged) return decos;
      // 只重查改动碰到的那几行（块级替换必须走 StateField，但不必每键扫全文，对标方案 A5）。
      let next = decos.map(tr.changes);
      tr.changes.iterChangedRanges((_fromA, _toA, fromB, toB) => {
        const from = tr.state.doc.lineAt(fromB).from;
        const to = tr.state.doc.lineAt(toB).to;
        const add = [];
        for (let pos = from; pos <= to; ) {
          const line = tr.state.doc.lineAt(pos);
          if (isLunaScaffoldComment(line.text)) add.push(Decoration.replace({ block: true }).range(line.from, line.to));
          pos = line.to + 1;
        }
        next = next.update({ filterFrom: from, filterTo: to, filter: () => false, add });
      });
      return next;
    },
    provide(f) {
      return EditorView.decorations.from(f);
    },
  });
}

const frontmatterLine = Decoration.line({ class: "luna-frontmatter-line" });

function buildFrontmatterLines(state: EditorState): DecorationSet {
  const range = getFrontmatterRange(state);
  if (!range) return Decoration.none;
  const last = state.doc.lineAt(range.to).number;
  const decos = [];
  for (let n = 1; n <= last; n++) decos.push(frontmatterLine.range(state.doc.line(n).from));
  return Decoration.set(decos);
}

/**
 * 源码模式里 frontmatter 的几行按 YAML 原文显示（等宽、淡一档），与正文分开（ADR-139 真机修订）。
 * 实时预览里这段被胶囊 / 属性面板整段替换，这些行装饰在那里不可见，故两种模式都挂、不必分。
 */
export function frontmatterLinesExtension(): Extension {
  return StateField.define<DecorationSet>({
    create: buildFrontmatterLines,
    update: (decos, tr) => (tr.docChanged ? buildFrontmatterLines(tr.state) : decos),
    provide: (f) => EditorView.decorations.from(f),
  });
}

export function frontmatterFoldingExtension(labels: FrontmatterLabels, panel: FrontmatterPanelOptions): Extension {
  const decosField = StateField.define<DecorationSet>({
    create(state) {
      return buildFrontmatterDecos(state, true, labels, panel);
    },
    update(decos, tr) {
      const collapsed = tr.state.field(frontmatterCollapsedField);
      // 光标移动、选区变化与 frontmatter 无关：不重建（找闭合 `---` 在未闭合时要扫全文）。
      // 起始状态里可能还没有折叠字段：从源码模式切回实时预览时，这组扩展与它同一笔事务装进来
      // （真机 2026-10-04：不带 `false` 就抛「Field is not present」，整个界面白屏）。
      if (!tr.docChanged && collapsed === tr.startState.field(frontmatterCollapsedField, false)) return decos;
      return buildFrontmatterDecos(tr.state, collapsed, labels, panel);
    },
    provide(f) {
      return EditorView.decorations.from(f);
    },
  });

  return [
    frontmatterCollapsedField,
    decosField,
  ];
}
