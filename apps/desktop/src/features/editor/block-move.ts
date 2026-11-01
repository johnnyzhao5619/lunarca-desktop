// ⌥↑ / ⌥↓ 按块移动（对标方案 B2；Typora 1.11、Notion、Obsidian 的列表项移动）。
//
// CodeMirror 默认的 ⌥↑↓ 一次挪一行：带子项的列表项被拆散、表格被劈开、代码块的围栏被挪走。
// 这里先认「块」：
// - 光标在列表项里：这一项连同它的子项，与相邻的同级项交换；有序列表随后重编号（同一笔事务）。
// - 光标在表格源码、引用块里，或在代码块的围栏行上：整块与相邻的块交换。
// - 其余（普通段落、标题、代码块内部的代码行）：交还默认的逐行移动。
// 没有可交换的同级项时什么都不做（返回 true 吞掉按键，免得默认逐行移动拆散结构）。

import { syntaxTree } from "@codemirror/language";
import { EditorSelection, Prec, type EditorState, type Extension } from "@codemirror/state";
import { keymap, type EditorView } from "@codemirror/view";
import type { SyntaxNode } from "@lezer/common";
import { withRenumber } from "./list-editing";
import { singleCursorOnly } from "./multi-cursor";

interface LineSpan {
  from: number;
  to: number;
}

/** 节点覆盖的整行范围（不含末尾换行）。lezer 的块节点有时止于下一行行首，故按 `to - 1` 取末行。 */
function lineSpan(state: EditorState, node: SyntaxNode): LineSpan {
  const end = Math.max(node.from, node.to - (state.doc.sliceString(node.to - 1, node.to) === "\n" ? 1 : 0));
  return { from: state.doc.lineAt(node.from).from, to: state.doc.lineAt(end).to };
}

function innermost(state: EditorState, pos: number, name: string): SyntaxNode | null {
  for (let node: SyntaxNode | null = syntaxTree(state).resolveInner(pos, 1); node; node = node.parent) {
    if (node.name === name) return node;
  }
  return null;
}

function topLevel(state: EditorState, pos: number): SyntaxNode | null {
  let node: SyntaxNode | null = syntaxTree(state).resolveInner(pos, 1);
  while (node?.parent && node.parent.name !== "Document") node = node.parent;
  return node && node.name !== "Document" ? node : null;
}

/** 交换两段相邻的整行文本（`a` 在 `b` 之前），光标随被移动的那一段走。 */
function swapSpans(state: EditorState, a: LineSpan, b: LineSpan, movingIsA: boolean, head: number) {
  const textA = state.doc.sliceString(a.from, a.to);
  const textB = state.doc.sliceString(b.from, b.to);
  const gap = state.doc.sliceString(a.to, b.from);
  const insert = textB + gap + textA;
  const moving = movingIsA ? a : b;
  const offset = head - moving.from;
  const newStart = movingIsA ? a.from + textB.length + gap.length : a.from;
  return { changes: { from: a.from, to: b.to, insert }, anchor: newStart + offset };
}

/** 计算一次按块移动；不归它管返回 null（交还默认逐行移动）。 */
export function blockMoveSpec(state: EditorState, up: boolean): { changes: { from: number; to: number; insert: string }; anchor: number; list: boolean } | "blocked" | null {
  const selection = state.selection.main;
  const head = selection.head;
  if (state.doc.lineAt(selection.from).number !== state.doc.lineAt(selection.to).number) return null;

  const item = innermost(state, head, "ListItem");
  if (item) {
    const sibling = up ? item.prevSibling : item.nextSibling;
    if (!sibling || sibling.name !== "ListItem") return "blocked";
    const mine = lineSpan(state, item);
    const theirs = lineSpan(state, sibling);
    const swap = up ? swapSpans(state, theirs, mine, false, head) : swapSpans(state, mine, theirs, true, head);
    return { ...swap, list: true };
  }

  const block = topLevel(state, head);
  if (!block) return null;
  const onFence = block.name === "FencedCode" &&
    (state.doc.lineAt(head).number === state.doc.lineAt(block.from).number ||
      state.doc.lineAt(head).number === state.doc.lineAt(lineSpan(state, block).to).number);
  if (block.name !== "Table" && block.name !== "Blockquote" && !onFence) return null;
  const sibling = up ? block.prevSibling : block.nextSibling;
  if (!sibling) return "blocked";
  const mine = lineSpan(state, block);
  const theirs = lineSpan(state, sibling);
  const swap = up ? swapSpans(state, theirs, mine, false, head) : swapSpans(state, mine, theirs, true, head);
  return { ...swap, list: false };
}

export function moveBlock(view: EditorView, up: boolean): boolean {
  const spec = blockMoveSpec(view.state, up);
  if (spec === null) return false;
  if (spec === "blocked") return true;
  const selection = EditorSelection.single(spec.anchor);
  const transaction = spec.list
    ? withRenumber(view.state, spec.changes, selection)
    : { changes: spec.changes, selection, userEvent: "move.line" };
  view.dispatch({ ...transaction, scrollIntoView: true });
  return true;
}

/** 键位：压在默认 ⌥↑↓（逐行移动）之前；不归它管时放行。 */
export function blockMoveKeymap(): Extension {
  return Prec.high(
    keymap.of([
      { key: "Alt-ArrowUp", run: singleCursorOnly((view) => moveBlock(view, true)) },
      { key: "Alt-ArrowDown", run: singleCursorOnly((view) => moveBlock(view, false)) },
    ]),
  );
}
