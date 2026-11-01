// 表格结构操作（纯函数，对标方案 T2）：插入 / 删除 / 移动行列、对齐、排序、删除整表、整理源码。
//
// **只改受影响的行**（ADR-138 ② 2026-10-03 修订）：增删行只动那一行；增删列每行都要动，但只在
// 该列边界处插删，不碰其它格的空白；对齐只改分隔行；排序只重排行、不改行内字节。整表按列宽
// 补齐是一个显式命令（`tidyTableEdit`），不再是任何结构动作的副作用——此前加一行就让整张表
// 出现在 git diff 里（Obsidian 用户对自动补齐抱怨最多的一点）。
//
// 每个操作返回一组变更与落点单元格坐标；落点由调用方在变更落地后按坐标换算（`applyTableEdit`）。

import type { ChangeSpec, EditorState } from "@codemirror/state";
import { inlineToPlainText, parseInlineMarkdown, type TableAlign } from "../../lib/markdown";
import { blankTableRow, delimiterCell, formatTableText, modelCells } from "./table-edit";
import { splitRowCells, type TableCellModel, type TableModel } from "./table-model";

export interface TableEdit {
  changes: ChangeSpec[];
  /** 落点单元格（变更后的坐标）。 */
  row: number;
  col: number;
}

interface TableLine {
  from: number;
  to: number;
  text: string;
  cells: TableCellModel[];
  delimiter: boolean;
}

/** 表格的全部源码行（按文档顺序：表头、分隔行、各正文行）。 */
function tableLines(state: EditorState, model: TableModel): TableLine[] {
  const out: TableLine[] = [];
  const first = state.doc.lineAt(model.from).number;
  const last = state.doc.lineAt(model.to).number;
  for (let n = first; n <= last; n++) {
    const line = state.doc.line(n);
    out.push({ from: line.from, to: line.to, text: line.text, cells: splitRowCells(line.text, line.from), delimiter: n === first + 1 });
  }
  return out;
}

/** 模型行号（0 = 表头）→ 源码行序号（分隔行占第 1 行）。 */
function lineIndexOf(row: number): number {
  return row === 0 ? 0 : row + 1;
}

function hasLeadingPipe(text: string): boolean {
  return text.trimStart().startsWith("|");
}

function hasTrailingPipe(line: TableLine): boolean {
  const last = line.cells[line.cells.length - 1];
  return last !== undefined && line.text[last.segTo - line.from] === "|";
}

function canonicalLine(cells: string[]): string {
  return `| ${cells.join(" | ")} |`;
}

/** 一行各格的原始文字（分隔行取 `---` / `:-:` 原样）。 */
function lineCellTexts(line: TableLine): string[] {
  return line.cells.map((cell) => cell.text);
}

function emptyCellFor(line: TableLine): string {
  return line.delimiter ? "---" : "";
}

/** 在一行的第 `at` 格之前插一格（`at` 可以等于格数，即追加；短行先补齐到 `at`）。 */
function insertCellChange(line: TableLine, at: number, text: string): ChangeSpec {
  if (at < line.cells.length && hasLeadingPipe(line.text)) {
    return { from: line.cells[at].segFrom, insert: ` ${text} |` };
  }
  if (at >= line.cells.length) {
    const end = line.from + line.text.replace(/\s+$/u, "").length;
    let insert = hasTrailingPipe(line) || line.cells.length === 0 ? "" : " |";
    for (let c = line.cells.length; c <= at; c++) insert += ` ${c === at ? text : emptyCellFor(line)} |`;
    return { from: end, insert };
  }
  // 无行首管道的写法在行首插格会被读成「行首管道」而丢一列：整行按标准写法重写。
  const texts = lineCellTexts(line);
  texts.splice(at, 0, text);
  return { from: line.from, to: line.to, insert: canonicalLine(texts) };
}

/** 删掉一行的第 `at` 格（连同它后面的管道；没有后管道的末格连同前面的管道）。 */
function removeCellChange(line: TableLine, at: number): { from: number; to: number } | null {
  const cell = line.cells[at];
  if (!cell) return null;
  if (line.text[cell.segTo - line.from] === "|") return { from: cell.segFrom, to: cell.segTo + 1 };
  return { from: Math.max(line.from, cell.segFrom - 1), to: cell.segTo };
}

/** 在模型第 `at` 行的位置插入一个空行（`at` ≥ 1：表头之上不能插）；落点为新行的 `col` 列。 */
export function insertRowEdit(state: EditorState, model: TableModel, at: number, col: number): TableEdit | null {
  if (at < 1 || at > model.rows.length) return null;
  const lines = tableLines(state, model);
  const previous = lines[at === 1 ? 1 : lineIndexOf(at - 1)];
  return { changes: [{ from: previous.to, insert: `\n${blankTableRow(model.columnCount)}` }], row: at, col };
}

/** 删除一行正文（表头不能删：GFM 必须有表头）。只剩表头也是合法表格。 */
export function deleteRowEdit(state: EditorState, model: TableModel, row: number, col: number): TableEdit | null {
  return deleteRowsEdit(state, model, row, row, col);
}

/** 删除连续几行正文（`top`..`bottom`，含两端；表头不在其中）。 */
export function deleteRowsEdit(state: EditorState, model: TableModel, top: number, bottom: number, col: number): TableEdit | null {
  if (top < 1 || bottom < top || bottom >= model.rows.length) return null;
  const lines = tableLines(state, model);
  const first = lines[lineIndexOf(top)];
  const last = lines[lineIndexOf(bottom)];
  return { changes: [{ from: first.from - 1, to: last.to }], row: Math.min(top, model.rows.length - 1 - (bottom - top + 1)), col };
}

/** 把一行正文上移 / 下移（`delta` = ±1）；表头不参与。两行互换原文，行内字节不变。 */
export function moveRowEdit(state: EditorState, model: TableModel, row: number, delta: number, col: number): TableEdit | null {
  const target = row + delta;
  if (row < 1 || target < 1 || target >= model.rows.length) return null;
  const lines = tableLines(state, model);
  const a = lines[lineIndexOf(row)];
  const b = lines[lineIndexOf(target)];
  return {
    changes: [
      { from: a.from, to: a.to, insert: b.text },
      { from: b.from, to: b.to, insert: a.text },
    ],
    row: target,
    col,
  };
}

/** 把一行正文移到第 `to` 行的位置（拖动行把手）；表头不参与。只改起止之间的那几行。 */
export function moveRowToEdit(state: EditorState, model: TableModel, from: number, to: number, col: number): TableEdit | null {
  if (from < 1 || to < 1 || from === to || from >= model.rows.length || to >= model.rows.length) return null;
  const lines = tableLines(state, model);
  const top = Math.min(from, to);
  const bottom = Math.max(from, to);
  const span = lines.slice(lineIndexOf(top), lineIndexOf(bottom) + 1).map((line) => line.text);
  const [moved] = span.splice(from - top, 1);
  span.splice(to - top, 0, moved);
  return {
    changes: [{ from: lines[lineIndexOf(top)].from, to: lines[lineIndexOf(bottom)].to, insert: span.join("\n") }],
    row: to,
    col,
  };
}

/** 把一列移到第 `to` 列的位置（拖动列把手）：每行只改起止之间那几格的原文顺序，对齐随列走。 */
export function moveColumnToEdit(state: EditorState, model: TableModel, from: number, to: number, row: number): TableEdit | null {
  if (from === to || from < 0 || to < 0 || from >= model.columnCount || to >= model.columnCount) return null;
  const left = Math.min(from, to);
  const right = Math.max(from, to);
  const changes: ChangeSpec[] = [];
  for (const line of tableLines(state, model)) {
    if (line.cells.length > right) {
      const raws = line.cells.slice(left, right + 1).map((cell) => line.text.slice(cell.segFrom - line.from, cell.segTo - line.from));
      const [moved] = raws.splice(from - left, 1);
      raws.splice(to - left, 0, moved);
      changes.push({ from: line.cells[left].segFrom, to: line.cells[right].segTo, insert: raws.join("|") });
    } else if (line.cells.length > left) {
      // 短行：补齐后按标准写法重写这一行，内容随列走。
      const texts = lineCellTexts(line);
      while (texts.length <= right) texts.push(emptyCellFor(line));
      const [moved] = texts.splice(from, 1);
      texts.splice(to, 0, moved);
      changes.push({ from: line.from, to: line.to, insert: canonicalLine(texts) });
    }
  }
  return { changes, row, col: to };
}

/** 在第 `at` 列之前插入一个空列（`at` 可等于列数，即追加到最右）；分隔行补 `---`。 */
export function insertColumnEdit(state: EditorState, model: TableModel, at: number, row: number): TableEdit | null {
  if (at < 0 || at > model.columnCount) return null;
  const changes = tableLines(state, model).map((line) => insertCellChange(line, at, emptyCellFor(line)));
  return { changes, row, col: at };
}

/** 删除一列（至少留一列）。每行只删这一格及其一侧的管道。 */
export function deleteColumnEdit(state: EditorState, model: TableModel, col: number, row: number): TableEdit | null {
  return deleteColumnsEdit(state, model, col, col, row);
}

/** 删除连续几列（`left`..`right`，含两端），至少留一列。每行只删这几格及其一侧的管道。 */
export function deleteColumnsEdit(state: EditorState, model: TableModel, left: number, right: number, row: number): TableEdit | null {
  if (left < 0 || right < left || right >= model.columnCount || right - left + 1 >= model.columnCount) return null;
  const changes: ChangeSpec[] = [];
  for (const line of tableLines(state, model)) {
    const first = removeCellChange(line, left);
    const last = removeCellChange(line, Math.min(right, line.cells.length - 1));
    if (first && last) changes.push({ from: first.from, to: last.to });
  }
  return { changes, row, col: Math.min(left, model.columnCount - 1 - (right - left + 1)) };
}

/** 把一列左移 / 右移（`delta` = ±1）：每行两格互换原文（含分隔行，对齐随列走）。 */
export function moveColumnEdit(state: EditorState, model: TableModel, col: number, delta: number, row: number): TableEdit | null {
  const target = col + delta;
  if (col < 0 || target < 0 || col >= model.columnCount || target >= model.columnCount) return null;
  const changes: ChangeSpec[] = [];
  for (const line of tableLines(state, model)) {
    const a = line.cells[col];
    const b = line.cells[target];
    if (a && b) {
      changes.push(
        { from: a.segFrom, to: a.segTo, insert: line.text.slice(b.segFrom - line.from, b.segTo - line.from) },
        { from: b.segFrom, to: b.segTo, insert: line.text.slice(a.segFrom - line.from, a.segTo - line.from) },
      );
    } else if (a || b) {
      // 短行缺其中一格：补齐后按标准写法重写这一行，内容随列走。
      const texts = lineCellTexts(line);
      while (texts.length <= Math.max(col, target)) texts.push(emptyCellFor(line));
      [texts[col], texts[target]] = [texts[target], texts[col]];
      changes.push({ from: line.from, to: line.to, insert: canonicalLine(texts) });
    }
  }
  return { changes, row, col: target };
}

/** 设定一列的对齐：只改分隔行那一格（保留它原来的宽度）。 */
export function alignColumnEdit(state: EditorState, model: TableModel, col: number, align: TableAlign, row: number): TableEdit | null {
  if (col < 0 || col >= model.columnCount || (model.align[col] ?? null) === align) return null;
  const delimiter = tableLines(state, model)[1];
  const cell = delimiter.cells[col];
  if (!cell) return { changes: [insertCellChange(delimiter, col, delimiterCell(align, 3))], row, col };
  const width = Math.max(3, cell.text.length);
  return { changes: [{ from: cell.from, to: cell.to, insert: delimiterCell(align, width) }], row, col };
}

/**
 * 按某一列排序正文行（表头不动）：比较的是**去掉 Markdown 标记**的文字（`**乙**` 按「乙」排，
 * Obsidian 1.5.1 同），按界面语言比较（中文按拼音、数字按大小），空格排最后；稳定排序。
 * 只重排行，行内字节不变。已经有序时返回 null。
 */
export function sortRowsEdit(state: EditorState, model: TableModel, col: number, descending: boolean, locale: string): TableEdit | null {
  const lines = tableLines(state, model);
  const body = lines.slice(2);
  if (body.length < 2) return null;
  const collator = new Intl.Collator(locale, { numeric: true, sensitivity: "base" });
  const key = (line: TableLine) => inlineToPlainText(parseInlineMarkdown(line.cells[col]?.text ?? "")).trim();
  const sorted = body
    .map((line, index) => ({ line, index, key: key(line) }))
    .sort((a, b) => {
      if (!a.key !== !b.key) return a.key ? -1 : 1;
      const order = collator.compare(a.key, b.key);
      return (descending ? -order : order) || a.index - b.index;
    });
  if (sorted.every((entry, index) => entry.index === index)) return null;
  return {
    changes: [{ from: body[0].from, to: body[body.length - 1].to, insert: sorted.map((entry) => entry.line.text).join("\n") }],
    row: 1,
    col,
  };
}

/** 删除整张表（连同它与下文之间的一个换行）；返回删除后的光标位置。 */
export function deleteTableEdit(state: EditorState, model: TableModel): { changes: ChangeSpec[]; anchor: number } {
  if (model.to < state.doc.length) return { changes: [{ from: model.from, to: model.to + 1 }], anchor: model.from };
  const from = Math.max(0, model.from - 1);
  return { changes: [{ from, to: model.to }], anchor: from };
}

/**
 * 「表格转为文本」：每行一行、格与格之间一个制表符（单元格里的 Markdown 原样保留，`<br>` 还原为
 * 空格，`\|` 还原为 `|`），分隔行去掉。与「转为表格」互逆（制表符那一路）。
 */
export function tableToTextEdit(state: EditorState, model: TableModel): { changes: ChangeSpec[]; anchor: number } {
  const text = tableLines(state, model)
    .filter((line) => !line.delimiter)
    .map((line) => line.cells.map((cell) => cell.text.replace(/<br\s*\/?\s*>/giu, " ").replace(/\\\|/gu, "|")).join("\t"))
    .join("\n");
  return { changes: [{ from: model.from, to: model.to, insert: text }], anchor: model.from };
}

/** 「整理表格源码」：按列宽补齐（CJK 计 2 宽），整块重写——这是唯一会整表重排的动作，且由用户显式发起。 */
export function tidyTableEdit(model: TableModel, row: number, col: number): TableEdit {
  const formatted = formatTableText(modelCells(model), model.align, model.columnCount);
  return { changes: [{ from: model.from, to: model.to, insert: formatted.text }], row, col };
}
