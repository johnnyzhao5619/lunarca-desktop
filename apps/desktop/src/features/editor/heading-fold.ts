// 标题与列表折叠（对标方案 B3；Obsidian 的折叠三角）。
//
// 折叠范围不在这里算：Markdown 语言包自带——标题折到下一个同级或更高级标题之前（章节），
// 列表项折掉它的子项与续行。这里只做界面：
// - 悬停标题行或带子项的列表项时，正文左侧页边出现 ▸ / ▾；折起来的常显。点一下切换。
// - 折起后行尾显示「…」，点一下展开。键位 ⌘⌥[ / ⌘⌥]（CodeMirror 默认，未被占用）。
// - 段落、代码块、表格不给三角：那几种可折，但折起来几乎没有好处，满页三角只是噪音。
//
// 三角是一个零宽的行首部件，按钮用 transform 推进左侧页边——不占正文宽度、不挪动文字。

import { codeFolding, foldable, foldEffect, foldedRanges, foldKeymap, syntaxTree, unfoldEffect } from "@codemirror/language";
import { RangeSetBuilder, type EditorState, type Extension } from "@codemirror/state";
import { Decoration, EditorView, keymap, ViewPlugin, WidgetType, type DecorationSet, type ViewUpdate } from "@codemirror/view";
import type { SyntaxNode } from "@lezer/common";

export interface FoldLabels {
  fold: string;
  unfold: string;
}

/** 这一行值不值得给三角：标题行，或带子列表 / 续行的列表项首行。 */
function foldableLine(state: EditorState, lineFrom: number): boolean {
  const line = state.doc.lineAt(lineFrom);
  for (let node: SyntaxNode | null = syntaxTree(state).resolveInner(line.from + Math.min(line.length, 1), 1); node; node = node.parent) {
    if (/^(ATX|Setext)Heading\d$/u.test(node.name)) return node.from >= line.from;
    if (node.name === "ListItem") return node.from >= line.from && state.doc.lineAt(node.to).number > line.number;
    if (node.name === "Document") return false;
  }
  return false;
}

function isFolded(state: EditorState, from: number, to: number): boolean {
  let folded = false;
  foldedRanges(state).between(from, to, (rangeFrom, rangeTo) => {
    if (rangeFrom === from && rangeTo === to) folded = true;
  });
  return folded;
}

class FoldMarker extends WidgetType {
  constructor(
    readonly from: number,
    readonly to: number,
    readonly folded: boolean,
    readonly labels: FoldLabels,
  ) {
    super();
  }

  override eq(other: FoldMarker) {
    return other.from === this.from && other.to === this.to && other.folded === this.folded;
  }

  toDOM(view: EditorView) {
    const wrap = document.createElement("span");
    wrap.className = "luna-fold-marker";
    const button = document.createElement("button");
    button.type = "button";
    button.className = "luna-fold-marker__button";
    if (this.folded) wrap.dataset.folded = "true";
    button.textContent = this.folded ? "▸" : "▾";
    const label = this.folded ? this.labels.unfold : this.labels.fold;
    button.setAttribute("aria-label", label);
    button.title = label;
    button.tabIndex = -1;
    button.addEventListener("mousedown", (event) => {
      event.preventDefault();
    });
    button.addEventListener("click", () => {
      const range = { from: this.from, to: this.to };
      view.dispatch({ effects: this.folded ? unfoldEffect.of(range) : foldEffect.of(range) });
    });
    wrap.appendChild(button);
    return wrap;
  }

  override ignoreEvent() {
    return true;
  }
}

function buildMarkers(view: EditorView, labels: FoldLabels): DecorationSet {
  const builder = new RangeSetBuilder<Decoration>();
  const { state } = view;
  for (const { from, to } of view.visibleRanges) {
    for (let pos = from; pos <= to; ) {
      const line = state.doc.lineAt(pos);
      if (foldableLine(state, line.from)) {
        const range = foldable(state, line.from, line.to);
        if (range) {
          builder.add(
            line.from,
            line.from,
            Decoration.widget({ widget: new FoldMarker(range.from, range.to, isFolded(state, range.from, range.to), labels), side: -1 }),
          );
        }
      }
      pos = line.to + 1;
    }
  }
  return builder.finish();
}

export function headingFoldExtension(labels: FoldLabels): Extension {
  const markers = ViewPlugin.fromClass(
    class {
      decorations: DecorationSet;
      constructor(view: EditorView) {
        this.decorations = buildMarkers(view, labels);
      }
      update(update: ViewUpdate) {
        const foldChanged = update.transactions.some((tr) => tr.effects.some((e) => e.is(foldEffect) || e.is(unfoldEffect)));
        if (update.docChanged || update.viewportChanged || foldChanged || syntaxTree(update.state) !== syntaxTree(update.startState)) {
          this.decorations = buildMarkers(update.view, labels);
        }
      }
    },
    { decorations: (instance) => instance.decorations },
  );
  return [
    codeFolding({
      placeholderDOM(_view, onclick) {
        const element = document.createElement("span");
        element.className = "luna-fold-placeholder";
        element.textContent = "…";
        element.title = labels.unfold;
        element.setAttribute("aria-label", labels.unfold);
        element.onclick = onclick;
        return element;
      },
    }),
    markers,
    keymap.of(foldKeymap),
  ];
}
