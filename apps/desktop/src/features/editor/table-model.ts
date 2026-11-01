// 表格模型（纯逻辑，零 DOM）：从文档中定位 GFM 表格块并解析为带**源码区间**的
// 结构化模型——每个单元格携带原始段区间（管道之间）与修剪后的内容区间，供
// Live Preview 表格就地渲染（table-preview，点击单元格定位光标）与表格编辑
// （table-edit：Tab 单元格导航 / 管道对齐格式化）共用（DRY，PRD-06-011/016）。
//
// 表格行结构按**行文本切分**（未转义 `|` 为界）而非仅依赖语法树的 TableCell 节点：
// 空单元格（`||`）不产 TableCell 节点，但编辑导航仍需可落点的位置；语法树只用来
// 定位 Table 块边界（天然跳过代码块内的伪表格）。

import { ensureSyntaxTree, syntaxTree } from "@codemirror/language";
import { Facet, StateEffect, type EditorState } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import { parseTableAlign, type TableAlign } from "../../lib/markdown";

export interface TableCellModel {
  /** 原始段区间（紧邻两侧管道之间，含空白）。 */
  segFrom: number;
  segTo: number;
  /** 修剪空白后的内容区间（空单元格时收敛为段内单点）。 */
  from: number;
  to: number;
  /** 修剪后的内容文本。 */
  text: string;
}

export interface TableRowModel {
  from: number;
  to: number;
  cells: TableCellModel[];
}

export interface TableModel {
  from: number;
  to: number;
  /** 各列对齐声明（来自分隔行 `:---:`；判定与阅读面同源，见 `lib/markdown`）。 */
  align: TableAlign[];
  /** 首行为表头行；不含分隔行。 */
  rows: TableRowModel[];
  columnCount: number;
}

/** 分隔行判定：仅由管道、冒号、横线与空白构成，且至少含一个横线。 */
const DELIMITER_ROW_RE = /^\s*\|?[\s:|-]*-[\s:|-]*\|?\s*$/u;

/** 解析一行表格源码为单元格列表（未转义 `|` 为界；外围管道外的空白段不算单元格）。 */
export function splitRowCells(lineText: string, lineFrom: number): TableCellModel[] {
  const pipes: number[] = [];
  let escapes = 0;
  for (let i = 0; i < lineText.length; i++) {
    if (lineText[i] === "|" && escapes % 2 === 0) {
      pipes.push(i);
    }
    escapes = lineText[i] === "\\" ? escapes + 1 : 0;
  }
  const segs: { from: number; to: number }[] = [];
  if (pipes.length === 0) {
    // GFM 表格正文允许单列内容省略管道；不能把这类合法行渲染成空行。
    segs.push({ from: 0, to: lineText.length });
  } else {
    const head = { from: 0, to: pipes[0] };
    if (lineText.slice(head.from, head.to).trim() !== "") {
      segs.push(head); // 无行首管道风格：首管道前的文本是第一个单元格。
    }
    for (let k = 0; k < pipes.length - 1; k++) {
      segs.push({ from: pipes[k] + 1, to: pipes[k + 1] });
    }
    const tail = { from: pipes[pipes.length - 1] + 1, to: lineText.length };
    if (lineText.slice(tail.from, tail.to).trim() !== "") {
      segs.push(tail); // 无行尾管道风格：末管道后的文本是最后一个单元格。
    }
  }
  return segs.map((seg) => {
    const raw = lineText.slice(seg.from, seg.to);
    const leading = raw.length - raw.trimStart().length;
    const text = raw.trim();
    const contentFrom = lineFrom + seg.from + leading;
    const emptyAnchor = lineFrom + seg.from + Math.floor(raw.length / 2);
    return {
      segFrom: lineFrom + seg.from,
      segTo: lineFrom + seg.to,
      from: text.length > 0 ? contentFrom : emptyAnchor,
      to: text.length > 0 ? contentFrom + text.length : emptyAnchor,
      text,
    };
  });
}

/** 从 Table 语法节点区间构建模型；结构不合法（缺分隔行）返回 null。 */
export function tableModelFromRange(state: EditorState, from: number, to: number): TableModel | null {
  const firstLine = state.doc.lineAt(from);
  const lastLine = state.doc.lineAt(to);
  const rows: TableRowModel[] = [];
  let align: TableAlign[] | null = null;
  for (let n = firstLine.number; n <= lastLine.number; n++) {
    const line = state.doc.line(n);
    if (line.text.trim() === "") {
      break; // 表格块内不应有空行；防御截断。
    }
    if (align === null && rows.length === 1 && DELIMITER_ROW_RE.test(line.text)) {
      align = parseTableAlign(line.text);
      continue;
    }
    rows.push({ from: line.from, to: line.to, cells: splitRowCells(line.text, line.from) });
  }
  if (align === null || rows.length === 0) {
    return null;
  }
  const columnCount = Math.max(align.length, ...rows.map((r) => r.cells.length));
  return { from: firstLine.from, to: lastLine.to, align, rows, columnCount };
}

/** 收集区间内的顶层表格块（语法树定位，代码块内伪表格天然排除）。 */
export function collectTables(state: EditorState, from: number, to: number): TableModel[] {
  const out: TableModel[] = [];
  // 仅顶层表格（引用块/列表内的表格行携带前缀标记，行切分语义不成立，保持源码呈现）：
  // 只走文档直属的块，不下钻——长文档每次按键都要做一遍，下钻全树代价随篇幅涨（对标方案 A5）。
  for (let node = syntaxTree(state).topNode.firstChild; node; node = node.nextSibling) {
    if (node.to < from) continue;
    if (node.from > to) break;
    if (node.name !== "Table") continue;
    const model = tableModelFromRange(state, node.from, node.to);
    if (model) {
      out.push(model);
    }
  }
  return out;
}

/** 等语法树解析到位的上限（毫秒）：只兜极端情况，正常文档增量解析一个块远低于此。 */
const TABLE_PARSE_BUDGET_MS = 500;

/**
 * 至少解析到 `pos` 所在块结尾（往下第一个空行，表格不会越过它）的语法树。`syntaxTree(state)`
 * 只是解析器在时间片内做完的那部分：新建状态只给 20 ms，长文档视口外靠空闲回调慢慢补，CPU
 * 吃紧时连短文档都可能没解析到表格——按键、菜单这类一次性动作拿它判断「是不是表格」，就会把
 * 表格当成正文（紧贴表格的空行打字，下一段并进表格）。所以这里同步补齐这一块再判断。
 */
function treeThroughBlock(state: EditorState, pos: number) {
  const doc = state.doc;
  let line = doc.lineAt(pos);
  while (line.number < doc.lines && line.text.trim() !== "") line = doc.line(line.number + 1);
  return ensureSyntaxTree(state, line.to, TABLE_PARSE_BUDGET_MS) ?? syntaxTree(state);
}

/**
 * 光标所在的顶层表格（不在表格内返回 null）。沿语法树上溯到文档直属的块，不遍历全文。
 * 语法树先补到这一块的结尾（`treeThroughBlock`），不随解析进度时有时无。
 */
export function tableAt(state: EditorState, pos: number): TableModel | null {
  const tree = treeThroughBlock(state, pos);
  for (const side of [1, -1] as const) {
    let node = tree.resolveInner(pos, side);
    while (node.parent && node.parent.name !== "Document") node = node.parent;
    if (node.name === "Table" && pos >= node.from && pos <= node.to) {
      const model = tableModelFromRange(state, node.from, node.to);
      if (model && pos >= model.from && pos <= model.to) return model;
    }
  }
  return null;
}

/**
 * 「事务落地后把焦点送进光标所在的单元格」：插入表格、源码键位的 Tab 这类从编辑器一侧发起的
 * 动作用它请求聚焦，由表格部件兑现——编辑一侧不认识部件的 DOM，两边也不互相 import。
 */
export const focusTableCellEffect = StateEffect.define<null>();

/**
 * 「插入表格」的接管者：装了网格尺寸选择器（`table-picker.ts`）就由它先问尺寸，否则直接插默认
 * 骨架。插入命令只认这个 Facet，不 import 选择器，两边不互相依赖。
 */
export const tableInsertHandler = Facet.define<(view: EditorView) => boolean>();
