// 围栏代码块的编辑语义（PRD-06-011）：代码块始终是原始 Markdown 文本，
// 不以 widget 代替。这里仅在 Enter 时补足 Markdown 围栏生命周期：
// - 在新开的围栏行末 Enter：补齐结束围栏并将光标放入正文；
// - 在最后一个空代码行 Enter：退出代码块，光标落到块后普通段落；
// - 其他位置一律交还 CodeMirror 默认换行/缩进，避免篡改真实代码内容。

import { syntaxTree } from "@codemirror/language";
import { EditorSelection, type EditorState, type Extension } from "@codemirror/state";
import { keymap, type EditorView } from "@codemirror/view";
import { singleCursorOnly } from "./multi-cursor";

interface Fence {
  indent: string;
  marker: "`" | "~";
  length: number;
}

interface FencedBlock {
  openingLine: number;
  closingLine: number | null;
  fence: Fence;
}

export interface CodeFenceEnterSpec {
  from: number;
  to: number;
  insert: string;
  anchor: number;
}

function openingFence(text: string): Fence | null {
  const match = /^(\s*)(`{3,}|~{3,})[^`~]*$/u.exec(text);
  if (!match) {
    return null;
  }
  return {
    indent: match[1],
    marker: match[2][0] as Fence["marker"],
    length: match[2].length,
  };
}

function isClosingFence(text: string, fence: Fence): boolean {
  const match = /^(\s*)(`+|~+)\s*$/u.exec(text);
  return (
    match !== null &&
    match[2][0] === fence.marker &&
    match[2].length >= fence.length
  );
}

function fencedBlockAt(state: EditorState, pos: number): FencedBlock | null {
  let from: number | null = null;
  let to: number | null = null;
  syntaxTree(state).iterate({
    from: Math.max(0, pos - 1),
    to: Math.min(state.doc.length, pos + 1),
    enter(node) {
      if (node.name === "FencedCode" && node.from <= pos && pos <= node.to) {
        from = node.from;
        to = node.to;
      }
    },
  });
  if (from === null || to === null) {
    return null;
  }

  const openingLine = state.doc.lineAt(from).number;
  const fence = openingFence(state.doc.line(openingLine).text);
  if (!fence) {
    return null;
  }

  let closingLine: number | null = null;
  const lastLine = state.doc.lineAt(to).number;
  for (let lineNumber = openingLine + 1; lineNumber <= lastLine; lineNumber += 1) {
    if (isClosingFence(state.doc.line(lineNumber).text, fence)) {
      closingLine = lineNumber;
      break;
    }
  }
  return { openingLine, closingLine, fence };
}

/** 是否处于 fenced-code 的文本内容中（供 Markdown 输入辅助避让）。 */
export function isInsideFencedCode(state: EditorState, pos: number): boolean {
  const block = fencedBlockAt(state, pos);
  if (!block) {
    return false;
  }
  const line = state.doc.lineAt(pos).number;
  return line > block.openingLine && line !== block.closingLine;
}

/**
 * 围栏代码块的 Enter 变更规格。返回 null 表示应由 CodeMirror 默认 Enter 处理。
 * 这个函数不依赖 DOM，保证自动闭合与退出语义可无头回归。
 */
export function codeFenceEnterSpec(state: EditorState): CodeFenceEnterSpec | null {
  const selection = state.selection.main;
  if (!selection.empty) {
    return null;
  }
  const line = state.doc.lineAt(selection.head);
  const block = fencedBlockAt(state, selection.head);
  if (!block) {
    return null;
  }

  // 刚输入 ` ```lang ` 且光标在行尾：一次 Enter 建立成对围栏，光标置于代码正文。
  if (
    line.number === block.openingLine &&
    selection.head === line.to &&
    block.closingLine === null
  ) {
    const closing = `${block.fence.indent}${block.fence.marker.repeat(block.fence.length)}`;
    return {
      from: selection.head,
      to: selection.head,
      insert: `\n\n${closing}`,
      anchor: selection.head + 1,
    };
  }

  // 代码块内最后一个空行：第二次 Enter 明确退出，而不是继续制造无闭合围栏。
  if (line.text.trim() !== "") {
    return null;
  }
  if (block.closingLine !== null && line.number === block.closingLine - 1) {
    const closing = state.doc.line(block.closingLine);
    return {
      from: line.from,
      to: closing.to,
      insert: `${closing.text}\n`,
      anchor: line.from + closing.text.length + 1,
    };
  }
  if (block.closingLine === null && line.number === state.doc.lines) {
    const closing = `${block.fence.indent}${block.fence.marker.repeat(block.fence.length)}`;
    return {
      from: line.from,
      to: line.to,
      insert: `${closing}\n`,
      anchor: line.from + closing.length + 1,
    };
  }
  return null;
}

function handleCodeFenceEnter(view: EditorView): boolean {
  const spec = codeFenceEnterSpec(view.state);
  if (!spec) {
    return false;
  }
  view.dispatch({
    changes: { from: spec.from, to: spec.to, insert: spec.insert },
    selection: EditorSelection.cursor(spec.anchor),
    scrollIntoView: true,
    userEvent: "input",
  });
  return true;
}

/** 置于列表续写与默认 Enter 之前，围栏语义优先且只消费自身的精确场景。 */
export function codeFenceEditingKeymap(): Extension {
  return keymap.of([{ key: "Enter", run: singleCursorOnly(handleCodeFenceEnter) }]);
}
