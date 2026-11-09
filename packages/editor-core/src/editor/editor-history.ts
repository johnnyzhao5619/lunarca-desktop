// 撤销历史跟着笔记走（PRD-06-014）：一个编辑器跨标签复用，换篇时重建历史，切回先前编辑过的那篇时带回
// 它离开时的历史、光标与折叠。

import { history, historyField } from "@codemirror/commands";
import { foldedRanges } from "@codemirror/language";
import type { Compartment, EditorState, Text } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";

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
