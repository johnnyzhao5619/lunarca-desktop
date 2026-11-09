import type { EditorView, KeyBinding } from "@codemirror/view";
import { EditorSelection, Transaction } from "@codemirror/state";
import { codeMirrorKey, editorShortcut } from "./editor-shortcuts";
import { setHeading, toggleInlineMark, type HeadingLevel } from "./format-commands";

export function toggleBold(view: EditorView) {
  // 真 toggle 语义（Obsidian/Typora 对标）：已加粗再触发即去加粗（format-commands 单一来源）。
  return toggleInlineMark(view, "strong");
}

export function toggleItalic(view: EditorView) {
  return toggleInlineMark(view, "em");
}

export function insertLink(view: EditorView, placeholder: string) {
  const transaction = view.state.changeByRange((range) => {
    const selected = view.state.doc.sliceString(range.from, range.to);
    const label = selected.length > 0 ? selected : "";
    const insert = `[${label}](${placeholder})`;
    const urlFrom = range.from + label.length + 3;
    const urlTo = urlFrom + placeholder.length;

    return {
      changes: { from: range.from, to: range.to, insert },
      range: EditorSelection.range(urlFrom, urlTo),
    };
  });

  view.dispatch(view.state.update(transaction, { userEvent: "input" }));
  return true;
}

/**
 * 格式键位（PRD-06-014）：加粗 / 斜体 / 插入链接，与 Typora 风格的段落键 ⌘0 正文、⌘1–⌘6 标题。键位一律从
 * `editor-shortcuts.ts` 派生（TDD-07 §4.2）：右键菜单与格式工具栏的提示取同一张表，于是「提示写着 ⌘K、
 * 按下去没反应」这类漂移没有发生的空间。`linkPlaceholder` 给插入链接时选中的占位网址（界面文案；按键那一刻现取，切换界面语言后跟着变）。
 */
export function formatKeyBindings(linkPlaceholder: () => string): KeyBinding[] {
  return [
    { key: codeMirrorKey(editorShortcut("bold")), run: toggleBold },
    { key: codeMirrorKey(editorShortcut("italic")), run: toggleItalic },
    { key: codeMirrorKey(editorShortcut("insert_link")), run: (view) => insertLink(view, linkPlaceholder()) },
    ...([0, 1, 2, 3, 4, 5, 6] as const).map((level) => ({
      key: codeMirrorKey(editorShortcut(`heading_${level}`)),
      run: (view: EditorView) => setHeading(view, level as HeadingLevel),
    })),
  ];
}

/**
 * 把正文整体换成 `next`（只改首尾之间真正不同的那一段，光标与装饰少受牵连）。
 *
 * **不进撤销历史**：它承担的是"装载 / 外部重载 / 跨窗同步"——这些是文档从外面来的，
 * 不是用户在这一篇里做的编辑。此前它进了历史，于是连续 ⌘Z 撤完全部编辑之后再按一次，
 * 撤掉的是**装载本身**，整篇变空（真机 2026-09-04）；切换笔记时上一篇的编辑也留在
 * 历史里，能被"撤"进新的一篇。VS Code / Obsidian 打开文件后 ⌘Z 都是无事发生。
 */
export function applyIncrementalText(view: EditorView, next: string) {
  const current = view.state.doc.toString();
  if (current === next) {
    return false;
  }

  let from = 0;
  while (from < current.length && from < next.length && current.charCodeAt(from) === next.charCodeAt(from)) {
    from += 1;
  }

  let currentEnd = current.length;
  let nextEnd = next.length;
  while (
    currentEnd > from &&
    nextEnd > from &&
    current.charCodeAt(currentEnd - 1) === next.charCodeAt(nextEnd - 1)
  ) {
    currentEnd -= 1;
    nextEnd -= 1;
  }

  view.dispatch({
    changes: {
      from,
      to: currentEnd,
      insert: next.slice(from, nextEnd),
    },
    annotations: [Transaction.addToHistory.of(false)],
  });
  return true;
}
