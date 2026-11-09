// 表格区域选择（纯函数，对标方案 T3）：网格态下的矩形选区、清空、删除与复制。
//
// 区域有四种来历：拖选 / Shift 扩选（`cells`，Delete 只清空内容——Google Sheets / Numbers 口径）；
// 点行把手 / 列把手（`rows` / `columns`，Delete 删掉这几行 / 列——Notion / Apple Notes 口径）；
// 正文里紧贴表格下方按退格（`table`，再按一次删除整表——Obsidian 1.5 口径）。

import type { ChangeSpec, EditorState } from "@codemirror/state";
import { inlineToPlainText, parseInlineMarkdown } from "../markdown/markdown";
import { delimiterCell } from "./table-edit";
import type { TableModel } from "./table-model";
import { deleteColumnsEdit, deleteRowsEdit, deleteTableEdit, type TableEdit } from "./table-structure";

export interface CellCoord {
  row: number;
  col: number;
}

export type TableRangeKind = "cells" | "rows" | "columns" | "table";

export interface TableRange {
  anchor: CellCoord;
  head: CellCoord;
  kind: TableRangeKind;
}

export interface RangeBounds {
  top: number;
  bottom: number;
  left: number;
  right: number;
}

export function rangeBounds(range: TableRange): RangeBounds {
  return {
    top: Math.min(range.anchor.row, range.head.row),
    bottom: Math.max(range.anchor.row, range.head.row),
    left: Math.min(range.anchor.col, range.head.col),
    right: Math.max(range.anchor.col, range.head.col),
  };
}

export function rangeContains(range: TableRange, row: number, col: number): boolean {
  const b = rangeBounds(range);
  return row >= b.top && row <= b.bottom && col >= b.left && col <= b.right;
}

export function isSingleCell(range: TableRange): boolean {
  return range.anchor.row === range.head.row && range.anchor.col === range.head.col;
}

export function wholeRowsRange(model: TableModel, top: number, bottom: number): TableRange {
  return { anchor: { row: top, col: 0 }, head: { row: bottom, col: model.columnCount - 1 }, kind: "rows" };
}

export function wholeColumnsRange(model: TableModel, left: number, right: number): TableRange {
  return { anchor: { row: 0, col: left }, head: { row: model.rows.length - 1, col: right }, kind: "columns" };
}

export function wholeTableRange(model: TableModel, kind: TableRangeKind = "table"): TableRange {
  return { anchor: { row: 0, col: 0 }, head: { row: model.rows.length - 1, col: model.columnCount - 1 }, kind };
}

/** 选区是否盖住整张表（⌘A 第二次按交还正文全选的判据）。 */
export function coversTable(model: TableModel, range: TableRange): boolean {
  const b = rangeBounds(range);
  return b.top === 0 && b.left === 0 && b.bottom === model.rows.length - 1 && b.right === model.columnCount - 1;
}

/** 把选区头往某个方向挪一格（Shift + 方向键），夹在表格范围内。 */
export function extendRange(model: TableModel, range: TableRange, dRow: number, dCol: number): TableRange {
  const head = {
    row: Math.max(0, Math.min(model.rows.length - 1, range.head.row + dRow)),
    col: Math.max(0, Math.min(model.columnCount - 1, range.head.col + dCol)),
  };
  return { anchor: range.anchor, head, kind: "cells" };
}

/** 清空选区内各格的内容（一笔事务；缺的格本来就空，跳过）。 */
export function clearRangeChanges(model: TableModel, range: TableRange): ChangeSpec[] {
  const b = rangeBounds(range);
  const changes: ChangeSpec[] = [];
  for (let r = b.top; r <= b.bottom; r++) {
    for (let c = b.left; c <= b.right; c++) {
      const cell = model.rows[r]?.cells[c];
      if (cell && cell.text) changes.push({ from: cell.from, to: cell.to, insert: "" });
    }
  }
  return changes;
}

/**
 * Delete / Backspace 作用于选区：`cells` 清空内容；`rows` / `columns` 删掉这几行 / 列（表头行
 * 与最后一列删不掉时退为清空）；`table` 删除整表。返回 null = 无事可做。
 */
export function deleteRangeEdit(
  state: EditorState,
  model: TableModel,
  range: TableRange,
): { changes: ChangeSpec[]; focus: CellCoord | null; anchor?: number } | null {
  const b = rangeBounds(range);
  let structural: TableEdit | null = null;
  if (range.kind === "table") {
    const edit = deleteTableEdit(state, model);
    return { changes: edit.changes, focus: null, anchor: edit.anchor };
  }
  if (range.kind === "rows") structural = deleteRowsEdit(state, model, Math.max(1, b.top), b.bottom, 0);
  if (range.kind === "columns") structural = deleteColumnsEdit(state, model, b.left, b.right, 0);
  if (structural) return { changes: structural.changes, focus: { row: structural.row, col: structural.col } };
  const changes = clearRangeChanges(model, range);
  return changes.length ? { changes, focus: range.anchor } : null;
}

function rangeTexts(model: TableModel, range: TableRange): string[][] {
  const b = rangeBounds(range);
  const out: string[][] = [];
  for (let r = b.top; r <= b.bottom; r++) {
    const row: string[] = [];
    for (let c = b.left; c <= b.right; c++) row.push(model.rows[r]?.cells[c]?.text ?? "");
    out.push(row);
  }
  return out;
}

/** 选区的纯文本矩阵：去掉 Markdown 标记，`<br>` 记为换行（粘进表格软件的是字，不是记号）。 */
export function rangePlainCells(model: TableModel, range: TableRange): string[][] {
  return rangeTexts(model, range).map((row) => row.map((text) => inlineToPlainText(parseInlineMarkdown(text)).trim()));
}

/** 选区写成一张独立的 GFM 表（首行作表头，沿用各列原有对齐）——带格式复制就渲染它。 */
export function rangeMarkdown(model: TableModel, range: TableRange): string {
  const b = rangeBounds(range);
  const rows = rangeTexts(model, range).map((row) => `| ${row.join(" | ")} |`);
  const delimiter = `| ${Array.from({ length: b.right - b.left + 1 }, (_, i) => delimiterCell(model.align[b.left + i] ?? null, 3)).join(" | ")} |`;
  return [rows[0], delimiter, ...rows.slice(1)].join("\n");
}
