// frontmatter（文首 `---` … `---`）在编辑面上的呈现（ADR-139）：实时预览里整段折成一枚胶囊、展开是
// 属性面板（`PropertiesPanel`，规则与控件原语经 `EditorHost` 注入）；源码模式里按 YAML 原文淡一档显示。「哪段算元数据」
// 的判据只此一处（`frontmatterCloseLine`），区间、属性数与字数共用它。

import { StateEffect, StateField, type EditorState, type Extension } from "@codemirror/state";
import { Decoration, type DecorationSet, EditorView, WidgetType } from "@codemirror/view";
import { createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import type { EditorHost } from "./editor-host";
import { PropertiesPanel } from "./PropertiesPanel";

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
  el.className = `luna-frontmatter-toggle ${collapsed ? "collapsed" : "expanded"}`;
  el.setAttribute("role", "button");
  el.setAttribute("tabindex", "0");
  el.setAttribute("title", collapsed ? labels.expandTitle : labels.collapseTitle);

  // 刻意用字符而不是 SVG 图标：图标资产是 React 组件、装饰层是手写 DOM，两面各画一份
  // 必然漂移（标注块的同一裁决，TDD-11 §2.1）。折叠/展开态以 ▸ / ▾ 表达，与树形控件同一语言。
  const icon = document.createElement("span");
  icon.className = "luna-frontmatter-toggle__icon";
  icon.textContent = collapsed ? "▸" : "▾";

  const label = document.createElement("span");
  label.className = "luna-frontmatter-toggle__label";
  label.textContent = labels.label(count);

  const action = document.createElement("span");
  action.className = "luna-frontmatter-toggle__action";
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
    private readonly host: EditorHost,
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
        rules: this.host.properties,
        ui: this.host.ui,
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
    wrap.className = "luna-frontmatter-panel";
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
export function frontmatterCloseLine(lineText: (line: number) => string, lineCount: number): number | null {
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
  host: EditorHost,
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
    : new FrontmatterPanelWidget(host, labels, panel, count, state.sliceDoc(0, range.to));
  return Decoration.set([Decoration.replace({ widget, block: true }).range(0, range.to)]);
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

export function frontmatterFoldingExtension(
  host: EditorHost,
  labels: FrontmatterLabels,
  panel: FrontmatterPanelOptions,
): Extension {
  const decosField = StateField.define<DecorationSet>({
    create(state) {
      return buildFrontmatterDecos(host, state, true, labels, panel);
    },
    update(decos, tr) {
      const collapsed = tr.state.field(frontmatterCollapsedField);
      // 光标移动、选区变化与 frontmatter 无关：不重建（找闭合 `---` 在未闭合时要扫全文）。
      // 起始状态里可能还没有折叠字段：从源码模式切回实时预览时，这组扩展与它同一笔事务装进来
      // （真机 2026-10-04：不带 `false` 就抛「Field is not present」，整个界面白屏）。
      if (!tr.docChanged && collapsed === tr.startState.field(frontmatterCollapsedField, false)) return decos;
      return buildFrontmatterDecos(host, tr.state, collapsed, labels, panel);
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
