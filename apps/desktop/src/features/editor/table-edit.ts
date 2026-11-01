// 表格编辑生产力（PRD-06-011/014 扩展，Obsidian 对标）：
// - Tab / Shift-Tab：表格内跳到下一个 / 上一个单元格；末行末格 Tab 自动追加新行；
//   导航不改写正文；只有追加行等结构操作才重排表格。
// - 插入表格命令：光标处插入 2×2 骨架并把焦点放进首个表头单元格。
// 全部落点/改写以纯函数产出变更规格（可无头测试），keymap 只做 dispatch。

import type { EditorState, Extension } from "@codemirror/state";
import { keymap, type EditorView } from "@codemirror/view";
import { type TableAlign } from "../../lib/markdown";
import { codeMirrorKey, editorShortcut } from "../command/command-registry";
import { focusTableCellEffect, splitRowCells, tableAt, tableInsertHandler, type TableModel } from "./table-model";
import { singleCursorOnly } from "./multi-cursor";

export interface TableChangeSpec {
  from: number;
  to: number;
  insert: string;
  /** 变更后的光标位置（绝对偏移）。 */
  anchor: number;
}

/** GFM 单元格保持换行与管道边界，输入桥接和矩形粘贴共用。 */
export function tableCellText(value: string): string {
  return value.replace(/\r?\n/gu, "<br>").replace(/(\\*)\|/gu, (match, slashes: string) => slashes.length % 2 ? match : `${slashes}\\|`);
}

export interface TablePastePreview {
  row: number;
  column: number;
  rows: number;
  columns: number;
  addedRows: number;
  addedColumns: number;
  replacedCells: number;
}

/** 编辑事务投影：只替换目标矩形；原有对齐与矩形之外的内容保留。 */
export function tableFillSpec(model: TableModel, row: number, column: number, incoming: string[][]): { spec: TableChangeSpec; preview: TablePastePreview } | null {
  const columns = Math.max(0, ...incoming.map((cells) => cells.length));
  if (!incoming.length || !columns || row < 0 || column < 0 || row >= model.rows.length || column >= model.columnCount) return null;
  const cells = modelCells(model);
  const totalColumns = Math.max(model.columnCount, column + columns);
  while (cells.length < row + incoming.length) cells.push([]);
  let replacedCells = 0;
  for (let r = 0; r < incoming.length; r++) {
    for (let c = 0; c < incoming[r].length; c++) {
      const previous = cells[row + r][column + c] ?? "";
      const next = tableCellText(incoming[r][c]);
      if (previous && previous !== next) replacedCells++;
      cells[row + r][column + c] = next;
    }
  }
  const formatted = formatTableText(cells, model.align, totalColumns);
  return { spec: { from: model.from, to: model.to, insert: formatted.text,
    anchor: model.from + formatted.cellOffsets[row][column].to },
    preview: { row: row + 1, column: column + 1, rows: incoming.length, columns,
      addedRows: Math.max(0, row + incoming.length - model.rows.length),
      addedColumns: Math.max(0, column + columns - model.columnCount), replacedCells } };
}

/** 宽字符（CJK/全角/谚文）：对齐时按 2 列计（等宽字体下的近似列对齐）。 */
const WIDE_CHAR_RE =
  /[\u1100-\u115F\u2E80-\uA4CF\uAC00-\uD7A3\uF900-\uFAFF\uFE30-\uFE4F\uFF00-\uFF60\uFFE0-\uFFE6]/u;

function displayWidth(text: string): number {
  let w = 0;
  for (const ch of text) {
    w += WIDE_CHAR_RE.test(ch) ? 2 : 1;
  }
  return w;
}

export function delimiterCell(align: TableAlign, width: number): string {
  const colons = align === "center" ? 2 : align ? 1 : 0;
  const bar = "-".repeat(Math.max(1, width - colons));
  if (align === "center") return `:${bar}:`;
  if (align === "left") return `:${bar}`;
  if (align === "right") return `${bar}:`;
  return bar;
}

interface FormattedTable {
  text: string;
  /** 每行每列的单元格内容在 `text` 内的 [from, to) 偏移。 */
  cellOffsets: { from: number; to: number }[][];
}

/** 把规范化的单元格文本矩阵格式化为管道对齐的表格源码。 */
export function formatTableText(cells: string[][], align: TableAlign[], columnCount: number): FormattedTable {
  const widths: number[] = [];
  for (let c = 0; c < columnCount; c++) {
    widths.push(Math.max(3, ...cells.map((row) => displayWidth(row[c] ?? ""))));
  }
  const lines: string[] = [];
  const cellOffsets: { from: number; to: number }[][] = [];
  let offset = 0;
  const pushLine = (line: string) => {
    lines.push(line);
    offset += line.length + 1; // 换行符
  };
  cells.forEach((row, r) => {
    const rowOffsets: { from: number; to: number }[] = [];
    let line = "|";
    for (let c = 0; c < columnCount; c++) {
      const text = row[c] ?? "";
      const pad = " ".repeat(Math.max(0, widths[c] - displayWidth(text)));
      const from = offset + line.length + 1;
      line += ` ${text}${pad} |`;
      rowOffsets.push({ from, to: from + text.length });
    }
    cellOffsets.push(rowOffsets);
    pushLine(line);
    if (r === 0) {
      let delim = "|";
      for (let c = 0; c < columnCount; c++) {
        delim += ` ${delimiterCell(align[c] ?? null, widths[c])} |`;
      }
      pushLine(delim);
    }
  });
  return { text: lines.join("\n"), cellOffsets };
}

export function modelCells(model: TableModel): string[][] {
  return model.rows.map((row) => {
    const out: string[] = [];
    for (let c = 0; c < model.columnCount; c++) {
      out.push(row.cells[c]?.text ?? "");
    }
    return out;
  });
}

/** 光标所在单元格坐标（行=模型行序号，列=单元格序号；分隔行归属表头行）。 */
export function cellCoordsAt(model: TableModel, pos: number): { row: number; col: number } {
  let row = 0;
  for (let r = 0; r < model.rows.length; r++) {
    if (pos <= model.rows[r].to) {
      row = r;
      break;
    }
    row = r;
  }
  const cells = model.rows[row].cells;
  let col = 0;
  for (let c = 0; c < cells.length; c++) {
    if (pos >= cells[c].segFrom && pos <= cells[c].segTo) {
      col = c;
      break;
    }
    if (pos > cells[c].segTo) {
      col = Math.min(c + 1, Math.max(0, model.columnCount - 1));
    }
  }
  return { row, col };
}

export interface TableStep {
  spec: TableChangeSpec;
  /** 落点单元格（行 = 模型行序号，含表头；列 = 列序号）。 */
  row: number;
  col: number;
}

/**
 * 从 (row, col) 走到下一格 / 上一格（纯函数，按单元格坐标而非编辑器选区）：跨行环绕；
 * 末行末格向前追加一个空行；首格向后返回 null（停在原处）。导航只移动选区，不改正文。
 * 短行缺的格照样是落点（选区落在行尾，部件按坐标聚焦）。
 */
export function tableStepSpec(model: TableModel, row: number, col: number, backward: boolean): TableStep | null {
  if (backward) {
    if (col === 0 && row === 0) return null;
    const target = col > 0 ? { row, col: col - 1 } : { row: row - 1, col: model.columnCount - 1 };
    return { ...target, spec: selectionOnly(model, target.row, target.col) };
  }
  if (col + 1 < model.columnCount) return { row, col: col + 1, spec: selectionOnly(model, row, col + 1) };
  if (row + 1 < model.rows.length) return { row: row + 1, col: 0, spec: selectionOnly(model, row + 1, 0) };
  // 末行末格：追加新空行（Obsidian / Google Docs 心智）。
  return insertRowStep(model, model.rows.length - 1, 0);
}

/**
 * 上 / 下一行的同一列（纯函数）：越过表头或末行返回 null——调用方据此离开表格，
 * 方向键**从不**新增行（Zettlr / Apple Notes 口径：新增行只属于 Tab / Enter）。
 */
export function tableVerticalStep(model: TableModel, row: number, col: number, down: boolean): TableStep | null {
  const target = down ? row + 1 : row - 1;
  if (target < 0 || target >= model.rows.length) return null;
  return { row: target, col, spec: selectionOnly(model, target, col) };
}

/** Enter：下一行同一列；在末行时新增一行（Obsidian / Zettlr / Advanced Tables 口径）。 */
export function tableEnterStep(model: TableModel, row: number, col: number): TableStep {
  return tableVerticalStep(model, row, col, true) ?? insertRowStep(model, row, col);
}

/** 一个空的表格行（标准写法 `|  |  |`）。 */
export function blankTableRow(columnCount: number): string {
  return `| ${Array.from({ length: columnCount }, () => "").join(" | ")} |`;
}

/**
 * 在 `afterRow` 之下插入一个空行（表头之下即第一行正文），落点为新行的 `col` 列。
 * 只插这一行，不重排整表（ADR-138 ② 2026-10-03 修订）。
 */
export function insertRowStep(model: TableModel, afterRow: number, col: number): TableStep {
  // 表头之后紧跟分隔行：插在分隔行之后。分隔行不在模型里，它的行尾就是下一行行首的前一位。
  const lineEnd = afterRow === 0 ? (model.rows[1] ? model.rows[1].from - 1 : model.to) : model.rows[afterRow].to;
  const line = blankTableRow(model.columnCount);
  const cells = splitRowCells(line, lineEnd + 1);
  return {
    row: afterRow + 1,
    col,
    spec: { from: lineEnd, to: lineEnd, insert: `\n${line}`, anchor: cells[Math.min(col, cells.length - 1)].to },
  };
}

/** 只把编辑器选区移到某一格（不改正文）。 */
export function cellSelection(model: TableModel, row: number, col: number): TableChangeSpec {
  return selectionOnly(model, row, col);
}

function selectionOnly(model: TableModel, row: number, col: number): TableChangeSpec {
  const target = model.rows[row].cells[col];
  return { from: model.from, to: model.from, insert: "", anchor: target?.to ?? model.rows[row].to };
}

/**
 * Tab / Shift-Tab 单元格跳转规格（编辑器光标在表格源码里时用）：光标不在表格内返回 null
 * （调用方落回缩进语义）。
 */
export function tableAdvanceSpec(state: EditorState, backward: boolean): TableChangeSpec | null {
  const selection = state.selection.main;
  if (!selection.empty) {
    return null;
  }
  const model = tableAt(state, selection.head);
  if (!model) {
    return null;
  }
  const { row, col } = cellCoordsAt(model, selection.head);
  return tableStepSpec(model, row, col, backward)?.spec ?? null;
}

/**
 * 往短行里缺的那一格写字：只改这一行，按需补齐管道（GFM 允许行比表头短，但没有管道就
 * 没有可写的位置）。不重排整张表。
 */
export function rowCellInsertSpec(state: EditorState, model: TableModel, row: number, col: number, text: string): TableChangeSpec {
  const target = model.rows[row];
  const line = state.doc.sliceString(target.from, target.to).replace(/\s+$/u, "");
  const last = target.cells[target.cells.length - 1];
  const closed = last !== undefined && last.segTo < target.from + line.length && line[last.segTo - target.from] === "|";
  let insert = closed ? line : `${line} |`;
  for (let c = target.cells.length; c <= col; c++) {
    insert += c === col ? ` ${text} |` : "  |";
  }
  return { from: target.from, to: target.to, insert, anchor: target.from + insert.length - 2 };
}

/** 离开表格落到上方正文：上一行的行尾；表格就在文首时补一个空段落（可撤销）。 */
export function tableEnterAboveSpec(state: EditorState, model: TableModel): TableChangeSpec {
  const first = state.doc.lineAt(model.from);
  if (first.number === 1) {
    return { from: model.from, to: model.from, insert: "\n\n", anchor: model.from };
  }
  return { from: model.from, to: model.from, insert: "", anchor: state.doc.line(first.number - 1).to };
}

/**
 * 离开表格落到下方正文的落点：绝不落在**紧贴表格的那一行**——GFM 把紧跟表格的非空行
 * 当作表格的一行，在那儿打字会并进表格（2026-10-03）。表格下面有空行就落到空行之后；
 * 紧接着另一个块（标题 / 列表）就落到它的行首；文末才补空行（可撤销）。
 */
export function tableExitSpec(state: EditorState, model: TableModel): TableChangeSpec {
  const doc = state.doc;
  const tableLine = doc.lineAt(model.to);
  if (tableLine.number === doc.lines) {
    return { from: model.to, to: model.to, insert: "\n\n", anchor: model.to + 2 };
  }
  const next = doc.line(tableLine.number + 1);
  if (next.text.trim() !== "") {
    return { from: model.to, to: model.to, insert: "", anchor: next.from };
  }
  if (next.number === doc.lines) {
    return { from: doc.length, to: doc.length, insert: "\n", anchor: doc.length + 1 };
  }
  return { from: model.to, to: model.to, insert: "", anchor: doc.line(next.number + 1).from };
}

/**
 * 光标停在**紧贴表格的那一行空行**上、正要打字或粘贴：先把落点挪成独立的新段落（纯函数）。
 * 不挪的话 GFM 会把这一行、连同紧随其后的那一段一起当成表格的行（真机 2026-10-04：在表下
 * 空行打一个 `/`，「后文一段」也被吞进了表格）。这一行留作表格与新段落之间的空行；下一行
 * 不空时再补一个空行，免得新段落与它连成一段。不在这种位置就返回 null。
 */
export function separateFromTableAboveSpec(state: EditorState, pos: number): TableChangeSpec | null {
  const doc = state.doc;
  const line = doc.lineAt(pos);
  if (line.text !== "" || line.number === 1) return null;
  const above = doc.line(line.number - 1);
  const model = tableAt(state, above.from);
  if (!model || model.to !== above.to) return null;
  const nextBlank = line.number === doc.lines || doc.line(line.number + 1).text.trim() === "";
  return { from: line.from, to: line.from, insert: nextBlank ? "\n" : "\n\n", anchor: line.from + 1 };
}

/** 插入表格骨架规格（纯函数）：`columns` 列 × `rows` 行（含表头），光标进首个表头单元格。 */
export function insertTableSpec(state: EditorState, columns = 2, rows = 2): TableChangeSpec {
  const selection = state.selection.main;
  const line = state.doc.lineAt(selection.head);
  const skeleton = formatTableText(
    Array.from({ length: Math.max(1, rows) }, () => Array.from({ length: Math.max(1, columns) }, () => "")),
    Array.from({ length: Math.max(1, columns) }, () => null),
    Math.max(1, columns),
  );
  const needsLeadingBreak = line.text.trim() !== "";
  const prefix = needsLeadingBreak ? "\n\n" : "";
  const insertAt = needsLeadingBreak ? line.to : selection.head;
  // 结尾换行给表格与下文留出空行；下文本来就是空行时不再补，免得留下两个空行。
  const blankFollows = line.number < state.doc.lines && state.doc.line(line.number + 1).text.trim() === "";
  return {
    from: insertAt,
    to: insertAt,
    insert: `${prefix}${skeleton.text}${blankFollows ? "" : "\n"}`,
    anchor: insertAt + prefix.length + skeleton.cellOffsets[0][0].to,
  };
}

/** `focusCell`：落地后把焦点送进光标所在单元格（实时预览下由表格部件兑现，源码模式无事发生）。 */
export function applyTableSpec(view: EditorView, spec: TableChangeSpec | null, options: { focusCell?: boolean } = {}): boolean {
  if (!spec) {
    return false;
  }
  view.dispatch({
    changes: spec.from === spec.to && spec.insert === "" ? undefined : { from: spec.from, to: spec.to, insert: spec.insert },
    selection: { anchor: spec.anchor },
    effects: options.focusCell ? focusTableCellEffect.of(null) : undefined,
    scrollIntoView: true,
    userEvent: "input",
  });
  return true;
}

/**
 * 插入表格命令（工具栏 / 快捷键 / 右键 / 斜杠菜单共用）：装了网格尺寸选择器就先问尺寸；
 * 否则插默认骨架。插完直接进首个表头格，可以马上打字。
 */
export function insertTableCommand(view: EditorView): boolean {
  const [handler] = view.state.facet(tableInsertHandler);
  if (handler) return handler(view);
  return applyTableSpec(view, insertTableSpec(view.state), { focusCell: true });
}

/** 表格编辑 keymap：置于缩进 keymap 之前，表格内优先消费 Tab。 */
export function tableEditingKeymap(): Extension {
  return keymap.of([
    { key: "Tab", run: singleCursorOnly((view) => applyTableSpec(view, tableAdvanceSpec(view.state, false), { focusCell: true })) },
    { key: "Shift-Tab", run: singleCursorOnly((view) => applyTableSpec(view, tableAdvanceSpec(view.state, true), { focusCell: true })) },
    // 键位归命令注册表（工具栏与右键菜单的提示从同一处派生）。
    { key: codeMirrorKey(editorShortcut("insert_table")), run: insertTableCommand },
  ]);
}
