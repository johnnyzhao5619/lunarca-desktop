// 多光标（对标方案 B5；VS Code / Obsidian / Sublime 同一套手势）：
// - ⌥ 点击加一个光标，⌥ 拖动拉出矩形（列）选区；⌘ 点击已归链接与时间戳起播，不抢。
// - ⌘D 选中下一处相同文字、⌘⇧L 选中全部相同文字、⌘⌥↑ / ⌘⌥↓ 在上下行加光标、Esc 收回到
//   一个光标——这些键位来自 CodeMirror 自带的 search / default 键表（已在编辑器里），
//   这里只打开「允许多个选区」，它们才不再被收成一个。
// - 打字、删除、粘贴、括号配对、加粗 / 斜体 / 链接、标题与列表前缀对每个光标各自生效。
//
// 只看主光标的自定义按键（列表续写、列表缩进与项首退格、代码围栏 Enter、表格 Tab、按块移动）
// 在多光标下一律放行给 CodeMirror 的通用命令（逐个光标换行 / 缩进 / 退格 / 逐行移动）：
// 它们按一个光标设计，硬跑会把其余光标收掉，或只改一处——宁可退回朴素但一致的行为。
// 选区矩形由 `selection-layer.ts` 逐段画，本来就支持多段。

import { EditorState, type Extension } from "@codemirror/state";
import { EditorView, rectangularSelection, type Command } from "@codemirror/view";

/** 多光标时放行（返回 false），交给后面的通用命令；单光标照常执行。 */
export function singleCursorOnly(command: Command): Command {
  return (view) => view.state.selection.ranges.length === 1 && command(view);
}

export function multiCursorExtension(): Extension {
  return [
    EditorState.allowMultipleSelections.of(true),
    EditorView.clickAddsSelectionRange.of((event) => event.altKey),
    rectangularSelection({ eventFilter: (event) => event.altKey }),
  ];
}
