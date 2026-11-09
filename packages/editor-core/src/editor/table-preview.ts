// Live Preview 表格就地渲染（PRD-06-011 P1 项，Obsidian 对标）：
// 整块替换为真实 <table> widget，单元格输入直接映射至 CM6 源码范围；
// 非空正文选区跨入表格时恢复源码，保留文本选择与整表复制语义。
// 事实源始终是 md 文本；单元格行内内容复用共享解析基线 `@lunarca/editor-core/markdown`（DRY）。
//
// 块级 replace 装饰不允许由 ViewPlugin 提供（CM6 约束），故与 frontmatter 折叠
// 同构走 StateField（文档/选区变化即重建；表格块数量级小，全文档扫描可承受）。

import { syntaxTree } from "@codemirror/language";
import type { EditorState, Extension, Range } from "@codemirror/state";
import { Prec, StateField } from "@codemirror/state";
import { Decoration, type DecorationSet, EditorView, keymap, WidgetType } from "@codemirror/view";
import { parseInlineMarkdown, type MdInline } from "../markdown/markdown";
import { renderMathInto } from "../render/math-render";
import { undo, redo, isolateHistory } from "@codemirror/commands";
import { collectTables, focusTableCellEffect, tableAt, type TableModel } from "./table-model";
import { insertColumnEdit, insertRowEdit, moveColumnToEdit, moveRowToEdit, type TableEdit } from "./table-structure";
import { shouldStartDrag } from "../input/drag-threshold";
import {
  coversTable, deleteRangeEdit, extendRange, isSingleCell, rangeContains, rangeBounds, rangeMarkdown,
  wholeColumnsRange, wholeRowsRange, wholeTableRange, type CellCoord, type TableRange,
} from "./table-range";
import { applyTableSpec, cellCoordsAt, cellSelection, insertRowStep, rowCellInsertSpec, separateFromTableAboveSpec,
  tableCellText, tableEnterAboveSpec, tableEnterStep, tableExitSpec, tableStepSpec, tableVerticalStep, type TableStep } from "./table-edit";
import { isImeCompositionKey, NO_COMPOSITION, type CompositionState } from "../input/ime-composition";

/** 表格菜单的作用对象：右键的那一格，或行 / 列把手。 */
export interface TableMenuTarget {
  tableFrom: number;
  row: number;
  col: number;
  scope: "cell" | "row" | "column";
}

export interface TableLabels {
  cell: (row: number, column: number) => string;
  rowHandle: (row: number) => string;
  columnHandle: (column: number) => string;
  appendRow: string;
  appendColumn: string;
  /** 复制多格选区：交出选区写成的独立 GFM 表，写剪贴板（TSV + 带格式）归编辑器一侧。 */
  copyRange?: (markdown: string) => void;
  /** 打开行 / 列把手菜单（菜单条目与文案归编辑器一侧，`table-menu.ts`）。 */
  openMenu?: (event: MouseEvent, target: TableMenuTarget) => void;
  pasteCell?: (view: EditorView, input: HTMLTextAreaElement, model: TableModel, row: number, column: number, text: string, html: string | null) => Promise<boolean>;
}
const models = new WeakMap<HTMLElement, TableModel>();
/** 每张表的输入框按 [行][列] 排好，增量更新时按坐标直取，不查 DOM。 */
const cellInputs = new WeakMap<HTMLElement, HTMLTextAreaElement[][]>();
/** 网格态下每张表的矩形选区（`table-range.ts`）。 */
const ranges = new WeakMap<HTMLElement, TableRange>();
/** 鼠标在某格按下、还没松开：拖进别的格就变成区域选择。 */
let dragOrigin: { wrap: HTMLElement; cell: CellCoord } | null = null;

function renderCell(text: string, into: HTMLElement) {
  // `<br>` 由解析器产出 `break`（与阅读视图、导出同一份判定）。
  renderInlineNodes(parseInlineMarkdown(text), into);
}

function inputCellText(text: string): string { return text.replace(/<br\s*\/?\s*>/giu, "\n"); }

/**
 * 输入框里的字写回源码时的形态：GFM 单元格的首尾空白不属于内容，写进源码只会变成越积越多的
 * 对齐空格；输入框里照留——否则刚敲的空格被重绘吃掉，「New York」打成「NewYork」（2026-10-03）。
 */
function sourceCellText(value: string): string { return tableCellText(value).trim(); }

/** 行内节点 → DOM（widget 内渲染，零 innerHTML）。链接不外开，仅样式提示。 */
function renderInlineNodes(nodes: MdInline[], into: HTMLElement) {
  for (const node of nodes) {
    switch (node.kind) {
      case "text":
        into.appendChild(document.createTextNode(node.text));
        break;
      case "strong": {
        const el = document.createElement("strong");
        renderInlineNodes(node.children, el);
        into.appendChild(el);
        break;
      }
      case "em": {
        const el = document.createElement("em");
        renderInlineNodes(node.children, el);
        into.appendChild(el);
        break;
      }
      case "del": {
        const el = document.createElement("del");
        renderInlineNodes(node.children, el);
        into.appendChild(el);
        break;
      }
      case "code": {
        const el = document.createElement("code");
        el.textContent = node.text;
        into.appendChild(el);
        break;
      }
      case "link": {
        const el = document.createElement("span");
        el.className = "luna-table-preview__link";
        el.title = node.href;
        renderInlineNodes(node.children, el);
        into.appendChild(el);
        break;
      }
      case "break":
        into.appendChild(document.createElement("br"));
        break;
      case "math": {
        const el = document.createElement("span");
        el.className = "luna-math";
        renderMathInto(el, node.tex, false);
        into.appendChild(el);
        break;
      }
      case "footnote": {
        const el = document.createElement("sup");
        el.className = "luna-footnote-ref";
        el.textContent = node.id;
        if (node.note) el.title = node.note;
        into.appendChild(el);
        break;
      }
      case "image":
        // 表格内图片按 alt 文本呈现（就地图片渲染归 Image widget 的行级语义）。
        into.appendChild(document.createTextNode(node.alt));
        break;
      case "embed":
        // 表格里的嵌入按原文呈现：格子里放不下一篇笔记。
        into.appendChild(document.createTextNode(`![[${node.target}${node.display === null ? "" : `|${node.display}`}]]`));
        break;
      default:
        break;
    }
  }
}

export class TablePreviewWidget extends WidgetType {
  constructor(
    private readonly model: TableModel,
    /** 表格块源码（widget 等价性判定：源码不变即复用 DOM）。 */
    private readonly source: string,
    private readonly labels?: TableLabels,
  ) {
    super();
  }

  override eq(other: TablePreviewWidget) {
    return other.source === this.source && other.model.from === this.model.from && other.labels === this.labels;
  }

  toDOM(view: EditorView) {
    const wrap = document.createElement("div");
    wrap.className = "luna-table-preview";
    models.set(wrap, this.model);
    const table = document.createElement("table");
    const [headerRow, ...bodyRows] = this.model.rows;

    const appendRow = (parent: HTMLElement, row: (typeof this.model.rows)[number], tag: "th" | "td") => {
      const tr = document.createElement("tr");
      for (let c = 0; c < this.model.columnCount; c++) {
        const cell = row.cells[c];
        const el = document.createElement(tag);
        const rowIndex = this.model.rows.indexOf(row);
        const align = this.model.align[c];
        if (align) {
          el.dataset.align = align;
        }
        if (this.labels?.openMenu && (rowIndex === 0 || c === 0)) {
          // 行把手挂在每行首格、列把手挂在表头各格：悬停才显，点开的是这一行 / 这一列的菜单。
          const openMenu = this.labels.openMenu;
          const handle = (scope: "row" | "column", label: string) => {
            const button = document.createElement("button");
            button.type = "button";
            button.className = `luna-table-preview__handle luna-table-preview__handle--${scope}`;
            button.textContent = scope === "row" ? "⋮" : "⋯";
            button.setAttribute("aria-label", label);
            button.title = label;
            button.addEventListener("mousedown", (event) => { event.preventDefault(); });
            button.addEventListener("pointerdown", (event) => {
              if (event.button === 0) startHandleDrag(view, wrap, button, scope, scope === "row" ? rowIndex : c, event);
            });
            button.addEventListener("click", (event) => {
              // 刚拖完松手也会补一次 click：那一下不是「点开菜单」。
              if (button.dataset.dragged) {
                delete button.dataset.dragged;
                return;
              }
              const model = models.get(wrap);
              if (!model) return;
              // 先选中这一行 / 这一列（网格态），再开菜单：菜单关了按 Delete 就删掉它们。
              const first = cellInputs.get(wrap)?.[scope === "row" ? rowIndex : 0]?.[scope === "row" ? 0 : c];
              if (first) {
                first.focus();
                enterGrid(first, scope === "row" ? wholeRowsRange(model, rowIndex, rowIndex) : wholeColumnsRange(model, c, c));
              }
              openMenu(event, { tableFrom: model.from, row: rowIndex, col: c, scope });
            });
            el.appendChild(button);
          };
          if (c === 0) handle("row", this.labels.rowHandle(rowIndex + 1));
          if (rowIndex === 0) handle("column", this.labels.columnHandle(c + 1));
        }
        const preview = document.createElement("div");
        preview.className = "luna-table-preview__content";
        if (cell) renderCell(cell.text, preview);
        el.appendChild(preview);
        if (this.labels) {
          const input = document.createElement("textarea");
          input.className = "luna-table-preview__input";
          input.rows = 1;
          input.dataset.row = String(rowIndex);
          input.dataset.column = String(c);
          input.value = inputCellText(cell?.text ?? "");
          const grid = cellInputs.get(wrap) ?? [];
          (grid[rowIndex] ??= [])[c] = input;
          cellInputs.set(wrap, grid);
          input.setAttribute("aria-label", this.labels.cell(rowIndex + 1, c + 1));
          let composition: CompositionState = { ...NO_COMPOSITION };
          let plainPaste = false;
          const locate = () => models.get(wrap)?.rows[rowIndex]?.cells[c];
          const commit = (event?: Event) => {
            const model = models.get(wrap);
            if (!model || composition.active || !wrap.isConnected) return;
            const current = locate();
            const insert = sourceCellText(input.value);
            if (insert === (current?.text ?? "")) return;
            const pasted = event instanceof InputEvent && event.inputType === "insertFromPaste";
            // 短行缺的格没有源码位置：只给这一行补管道（`rowCellInsertSpec`），不重排整表。
            // 空格子的锚点在格段正中（光标居中），从那里插入会写成 `|  a/   |`；第一次写入改为
            // 整段换成 ` 文字 `（只动这一格）。
            const spec = !current
              ? rowCellInsertSpec(view.state, model, rowIndex, c, insert)
              : current.text === "" && insert !== ""
                ? { from: current.segFrom, to: current.segTo, insert: ` ${insert} `, anchor: current.segFrom + 1 + insert.length }
                : { from: current.from, to: current.to, insert, anchor: current.from + insert.length };
            view.dispatch({ changes: { from: spec.from, to: spec.to, insert: spec.insert },
              selection: { anchor: spec.anchor }, userEvent: pasted ? "input.paste" : "input.type",
              annotations: pasted ? isolateHistory.of("full") : undefined });
          };
          input.addEventListener("compositionstart", () => { composition = { active: true, endedAt: composition.endedAt }; input.dataset.composing = "true"; });
          input.addEventListener("compositionend", (event) => { composition = { active: false, endedAt: event.timeStamp }; delete input.dataset.composing; commit(); });
          input.addEventListener("input", commit);
          input.addEventListener("paste", (event) => {
            event.stopPropagation();
            const asPlain = plainPaste || composition.active;
            plainPaste = false;
            const data = event.clipboardData;
            const model = models.get(wrap);
            if (!data || !model || !this.labels?.pasteCell || asPlain) return;
            const text = data.getData("text/plain");
            const html = data.getData("text/html");
            if (!html && !text.includes("\t")) return;
            event.preventDefault();
            // 网格态有多格选区时，从选区左上角那一格开始填（Google Sheets / Excel 同）。
            const range = input.dataset.grid === "true" ? ranges.get(wrap) : undefined;
            const start = range ? rangeBounds(range) : { top: rowIndex, left: c };
            void this.labels.pasteCell(view, input, model, start.top, start.left, text, html || null)
              .then((inserted) => { if (inserted) focusSelectedCell(view); });
          });
          input.addEventListener("focus", () => {
            const current = locate();
            const anchor = current?.to ?? models.get(wrap)?.rows[rowIndex]?.to;
            if (anchor !== undefined) view.dispatch({ selection: { anchor } });
          });
          input.addEventListener("compositionstart", () => { exitGrid(input); }, { capture: true });
          // 焦点去了编辑器里的别处才退出网格态；去了菜单（挂在 body 下）就留着选区，菜单关了焦点会回来。
          input.addEventListener("blur", (event) => {
            if (event.relatedTarget instanceof Node && view.dom.contains(event.relatedTarget)) exitGrid(input);
          });
          input.addEventListener("mousedown", (event) => {
            const active = document.activeElement;
            if (event.shiftKey && active instanceof HTMLTextAreaElement && active !== input && wrapOf(active) === wrap) {
              // Shift + 点击：从当前格扩到这一格，焦点不动。
              event.preventDefault();
              const range = ranges.get(wrap);
              enterGrid(active, { anchor: range?.anchor ?? coordsOf(active), head: coordsOf(input), kind: "cells" });
              return;
            }
            dragOrigin = { wrap, cell: coordsOf(input) };
            window.addEventListener("mouseup", () => { dragOrigin = null; }, { once: true });
          });
          input.addEventListener("mouseenter", (event) => {
            if (!dragOrigin || dragOrigin.wrap !== wrap || !(event.buttons & 1)) return;
            const here = coordsOf(input);
            if (here.row === dragOrigin.cell.row && here.col === dragOrigin.cell.col) return;
            // 拖出起点那一格：变成区域选择。焦点留在起点格（输入法与复制都由它接）。
            const origin = cellInputs.get(wrap)?.[dragOrigin.cell.row]?.[dragOrigin.cell.col];
            if (!origin) return;
            if (document.activeElement !== origin) origin.focus();
            enterGrid(origin, { anchor: dragOrigin.cell, head: here, kind: "cells" });
          });
          input.addEventListener("keydown", (event) => {
            plainPaste = (event.metaKey || event.ctrlKey) && event.shiftKey && event.key.toLowerCase() === "v";
            if (isImeCompositionKey(event, composition)) return;
            const model = models.get(wrap);
            if (!model) return;
            const mod = event.metaKey || event.ctrlKey;
            const go = (step: TableStep | null, caret: Caret = "end", grid = false) => {
              if (!step) return;
              commit();
              applyTableSpec(view, step.spec);
              focusCell(view, model.from, step.row, step.col, caret, grid);
            };
            const leave = (up: boolean) => {
              commit();
              const current = models.get(wrap) ?? model;
              const spec = up ? tableEnterAboveSpec(view.state, current) : tableExitSpec(view.state, current);
              view.dispatch({ changes: spec.insert ? { from: spec.from, insert: spec.insert } : undefined,
                selection: { anchor: spec.anchor }, scrollIntoView: true, userEvent: "input.type" });
              view.focus();
            };
            const atStart = input.selectionStart === 0 && input.selectionEnd === 0;
            const atEnd = input.selectionStart === input.value.length && input.selectionEnd === input.value.length;
            const firstLine = !input.value.slice(0, input.selectionStart).includes("\n");
            const lastLine = !input.value.slice(input.selectionEnd).includes("\n");
            const grid = input.dataset.grid === "true";
            const handled = (() => {
              // 撤销 / 重做走主文档（⌘Z / ⇧⌘Z，Windows 另有 Ctrl+Y）。
              if ((mod && event.key.toLowerCase() === "z") || (event.ctrlKey && !event.metaKey && event.key.toLowerCase() === "y")) {
                const redoing = event.shiftKey || event.key.toLowerCase() === "y";
                if ((redoing ? redo : undo)(view)) focusSelectedCell(view);
                return true;
              }
              if (event.key === "Tab") {
                // Tab 在表格里永远是换格：首格 Shift-Tab 停在原处，不让焦点按浏览器默认外逃。
                go(tableStepSpec(model, rowIndex, c, event.shiftKey));
                return true;
              }
              if (grid) {
                // 网格态：选中整格、不显示光标；输入框仍可编辑，打字（含输入法第一下）直接进入编辑。
                const range = ranges.get(wrap) ?? { anchor: { row: rowIndex, col: c }, head: { row: rowIndex, col: c }, kind: "cells" as const };
                const multi = !isSingleCell(range) || range.kind !== "cells";
                const arrows: Record<string, [number, number]> = { ArrowUp: [-1, 0], ArrowDown: [1, 0], ArrowLeft: [0, -1], ArrowRight: [0, 1] };
                if (event.shiftKey && arrows[event.key]) {
                  const [dRow, dCol] = arrows[event.key];
                  setRange(wrap, extendRange(model, range, dRow, dCol));
                  return true;
                }
                if (mod && event.key.toLowerCase() === "a") {
                  // 第一次选中整张表，再按一次交还正文全选（表格随之显示源码，复制得到完整 Markdown）。
                  if (coversTable(model, range)) {
                    view.focus();
                    view.dispatch({ selection: { anchor: 0, head: view.state.doc.length } });
                  } else setRange(wrap, wholeTableRange(model, "cells"));
                  return true;
                }
                if (multi && (event.key === "Backspace" || event.key === "Delete")) {
                  const edit = deleteRangeEdit(view.state, model, range);
                  if (edit) {
                    view.dispatch({ changes: edit.changes, selection: edit.anchor !== undefined ? { anchor: edit.anchor } : undefined,
                      scrollIntoView: true, userEvent: "delete" });
                    if (edit.focus) focusCell(view, model.from, edit.focus.row, edit.focus.col, "end", true);
                    else view.focus();
                  }
                  return true;
                }
                if (multi && mod && (event.key.toLowerCase() === "c" || event.key.toLowerCase() === "x")) {
                  this.labels?.copyRange?.(rangeMarkdown(model, range));
                  if (event.key.toLowerCase() === "x") {
                    const edit = deleteRangeEdit(view.state, model, { ...range, kind: "cells" });
                    if (edit) view.dispatch({ changes: edit.changes, userEvent: "delete.cut" });
                  }
                  return true;
                }
                switch (event.key) {
                  case "Escape": leave(false); return true;
                  case "Enter": case "F2": enterEdit(input); return true;
                  case "Backspace": case "Delete": input.value = ""; commit(); return true;
                  case "ArrowUp": if (rowIndex === 0) leave(true); else go(tableVerticalStep(model, rowIndex, c, false), "end", true); return true;
                  case "ArrowDown": if (rowIndex + 1 >= model.rows.length) leave(false); else go(tableVerticalStep(model, rowIndex, c, true), "end", true); return true;
                  case "ArrowLeft": if (c > 0) go({ row: rowIndex, col: c - 1, spec: cellSelection(model, rowIndex, c - 1) }, "end", true); return true;
                  case "ArrowRight": if (c + 1 < model.columnCount) go({ row: rowIndex, col: c + 1, spec: cellSelection(model, rowIndex, c + 1) }, "end", true); return true;
                  default:
                    if (mod && (event.key.toLowerCase() === "c" || event.key.toLowerCase() === "x")) {
                      input.select(); // 复制 / 剪切这一格的全部内容，交给系统默认动作
                      return false;
                    }
                    if (event.key.length === 1 && !mod) enterEdit(input);
                    return false;
                }
              }
              if (event.key === "Escape") { enterGrid(input); return true; }
              if (event.key === "Enter" && mod) { go(insertRowStep(model, rowIndex, c)); return true; }
              if (event.key === "Enter" && !event.shiftKey && !event.altKey) { go(tableEnterStep(model, rowIndex, c)); return true; }
              if (mod && !event.shiftKey && (event.key.toLowerCase() === "b" || event.key.toLowerCase() === "i")) {
                toggleCellMark(input, event.key.toLowerCase() === "b" ? "**" : "*");
                commit();
                return true;
              }
              if (event.shiftKey || event.altKey || mod) return false;
              if (event.key === "ArrowUp" && firstLine) {
                if (rowIndex === 0) leave(true); else go(tableVerticalStep(model, rowIndex, c, false));
                return true;
              }
              if (event.key === "ArrowDown" && lastLine) {
                if (rowIndex + 1 >= model.rows.length) leave(false); else go(tableVerticalStep(model, rowIndex, c, true));
                return true;
              }
              if (event.key === "ArrowLeft" && atStart) {
                if (c > 0) go({ row: rowIndex, col: c - 1, spec: cellSelection(model, rowIndex, c - 1) });
                else if (rowIndex > 0) go({ row: rowIndex - 1, col: model.columnCount - 1, spec: cellSelection(model, rowIndex - 1, model.columnCount - 1) });
                else leave(true);
                return true;
              }
              if (event.key === "ArrowRight" && atEnd) {
                if (c + 1 < model.columnCount) go({ row: rowIndex, col: c + 1, spec: cellSelection(model, rowIndex, c + 1) }, "start");
                else if (rowIndex + 1 < model.rows.length) go({ row: rowIndex + 1, col: 0, spec: cellSelection(model, rowIndex + 1, 0) }, "start");
                else leave(false);
                return true;
              }
              return false;
            })();
            if (handled) event.preventDefault();
          });
          input.addEventListener("copy", () => {
            // 网格态复制完把选区收回格尾：否则下一次打字会替换整格。
            if (input.dataset.grid === "true") queueMicrotask(() => { input.setSelectionRange(input.value.length, input.value.length); });
          });
          el.appendChild(input);
        }
        tr.appendChild(el);
      }
      parent.appendChild(tr);
    };

    if (headerRow) {
      const thead = document.createElement("thead");
      appendRow(thead, headerRow, "th");
      table.appendChild(thead);
    }
    const tbody = document.createElement("tbody");
    for (const row of bodyRows) {
      appendRow(tbody, row, "td");
    }
    table.appendChild(tbody);
    // 表格与两条「追加」窄条放进同一个网格：右边一条加列、下边一条加行，悬停才显（Notion / Obsidian 1.5）。
    const frame = document.createElement("div");
    frame.className = "luna-table-preview__frame";
    frame.appendChild(table);
    const labels = this.labels;
    if (labels) {
      const appendButton = (className: string, label: string, run: (model: TableModel) => TableEdit | null) => {
        const button = document.createElement("button");
        button.type = "button";
        button.className = className;
        button.textContent = "+";
        button.setAttribute("aria-label", label);
        button.title = label;
        button.addEventListener("mousedown", (event) => { event.preventDefault(); });
        button.addEventListener("click", () => {
          const model = models.get(wrap);
          if (model) applyTableEdit(view, model.from, run(model));
        });
        frame.appendChild(button);
      };
      appendButton("luna-table-preview__append luna-table-preview__append--column", labels.appendColumn,
        (model) => insertColumnEdit(view.state, model, model.columnCount, 0));
      appendButton("luna-table-preview__append luna-table-preview__append--row", labels.appendRow,
        (model) => insertRowEdit(view.state, model, model.rows.length, 0));
    }
    wrap.appendChild(frame);
    return wrap;
  }

  override updateDOM(dom: HTMLElement): boolean {
    const old = models.get(dom);
    if (!old || old.columnCount !== this.model.columnCount || old.rows.length !== this.model.rows.length) return false;
    models.set(dom, this.model);
    // 只碰文字真的变了的格：大表（500 行 × 8 列）每键遍历全部输入框要 37ms（无头测量），超一帧。
    const inputs = cellInputs.get(dom);
    for (let r = 0; r < this.model.rows.length; r++) {
      for (let c = 0; c < this.model.columnCount; c++) {
        const text = this.model.rows[r].cells[c]?.text ?? "";
        if ((old.rows[r].cells[c]?.text ?? "") === text) continue;
        const input = inputs?.[r]?.[c];
        if (!input) return false;
        // 只在内容真的变了（撤销、外部改动）时覆盖输入框；首尾空白之差不算——那是用户正在敲的字。
        if (input.dataset.composing !== "true" && sourceCellText(input.value) !== text) input.value = inputCellText(text);
        const preview = input.previousElementSibling;
        if (preview instanceof HTMLElement) {
          preview.replaceChildren(); renderCell(text, preview);
        }
      }
    }
    return true;
  }
}

type Caret = "start" | "end";

/** 把焦点送进某张表的某一格（事务落地、部件重建之后才查 DOM）；`grid` = 以网格态选中整格。 */
function focusCell(view: EditorView, tableFrom: number, row: number, col: number, caret: Caret = "end", grid: boolean | TableRange = false) {
  queueMicrotask(() => {
    for (const wrap of view.dom.querySelectorAll<HTMLElement>(".luna-table-preview")) {
      if (models.get(wrap)?.from !== tableFrom) continue;
      const input = wrap.querySelector<HTMLTextAreaElement>(`textarea[data-row="${row}"][data-column="${col}"]`);
      if (!input) continue;
      input.focus();
      const at = caret === "start" ? 0 : input.value.length;
      input.setSelectionRange(at, at);
      if (grid) enterGrid(input, grid === true ? undefined : grid);
    }
  });
}

/**
 * 网格态（第一次 Esc）：选中整格、隐藏光标，方向键在格间移动，Enter / F2 回到编辑，再按 Esc 离开表格。
 * 焦点始终留在这一格的输入框里——中文输入法的第一下由它直接接住，不需要先「进入编辑」。
 */
function enterGrid(input: HTMLTextAreaElement, range?: TableRange) {
  input.dataset.grid = "true";
  input.setSelectionRange(input.value.length, input.value.length);
  const wrap = wrapOf(input);
  const here = coordsOf(input);
  if (wrap) setRange(wrap, range ?? { anchor: here, head: here, kind: "cells" });
}

function exitGrid(input: HTMLTextAreaElement) {
  delete input.dataset.grid;
  const wrap = wrapOf(input);
  if (wrap) setRange(wrap, null);
}

function wrapOf(input: HTMLElement): HTMLElement | null {
  return input.closest<HTMLElement>(".luna-table-preview");
}

function coordsOf(input: HTMLTextAreaElement): CellCoord {
  return { row: Number(input.dataset.row), col: Number(input.dataset.column) };
}

/** 网格态的矩形选区：选中的格挂 `aria-selected`（样式与读屏同一个事实）。 */
function setRange(wrap: HTMLElement, range: TableRange | null) {
  if (range) ranges.set(wrap, range);
  else ranges.delete(wrap);
  for (const row of cellInputs.get(wrap) ?? []) {
    for (const input of row ?? []) {
      const cell = input?.closest("th, td");
      if (!cell) continue;
      if (range && rangeContains(range, Number(input.dataset.row), Number(input.dataset.column))) cell.setAttribute("aria-selected", "true");
      else cell.removeAttribute("aria-selected");
    }
  }
}

function enterEdit(input: HTMLTextAreaElement) {
  exitGrid(input);
  input.setSelectionRange(input.value.length, input.value.length);
}

/** 格内 ⌘B / ⌘I：选区两侧已有标记就去掉，否则包上；没有选区时插入一对标记、光标落在中间。 */
function toggleCellMark(input: HTMLTextAreaElement, mark: string) {
  const { selectionStart: from, selectionEnd: to, value } = input;
  const before = value.slice(Math.max(0, from - mark.length), from);
  const after = value.slice(to, to + mark.length);
  if (before === mark && after === mark && (mark !== "*" || (value[from - 2] !== "*" && value[to + 1] !== "*"))) {
    input.value = value.slice(0, from - mark.length) + value.slice(from, to) + value.slice(to + mark.length);
    input.setSelectionRange(from - mark.length, to - mark.length);
    return;
  }
  input.value = value.slice(0, from) + mark + value.slice(from, to) + mark + value.slice(to);
  input.setSelectionRange(from + mark.length, to + mark.length);
}

/**
 * 拖动行把手 / 列把手移动这一行 / 列（Obsidian 1.5.2：移动而非互换）。指针事件而非 HTML5 拖放
 * （Tauri 里两者互斥，见桌面的 `lib/pointer-drag.ts`）；位移不到起拖阈值就是点击，照常开菜单。
 * 落点是指针下那一格所在的行 / 列，以 `data-drop-target` 画出插入线；Esc 取消。
 */
function startHandleDrag(view: EditorView, wrap: HTMLElement, button: HTMLElement, scope: "row" | "column", index: number, down: PointerEvent) {
  let started = false;
  let target: number | null = null;
  const mark = (next: number | null) => {
    for (const cell of wrap.querySelectorAll("[data-drop-target]")) cell.removeAttribute("data-drop-target");
    for (const row of cellInputs.get(wrap) ?? []) {
      for (const input of row ?? []) {
        if (next === null || !input) continue;
        const at = scope === "row" ? Number(input.dataset.row) : Number(input.dataset.column);
        if (at === next) input.closest("th, td")?.setAttribute("data-drop-target", scope);
      }
    }
  };
  const finish = (commit: boolean) => {
    window.removeEventListener("pointermove", onMove);
    window.removeEventListener("pointerup", onUp);
    window.removeEventListener("keydown", onKey, true);
    mark(null);
    delete wrap.dataset.dragging;
    if (!started) return;
    button.dataset.dragged = "true";
    const model = models.get(wrap);
    if (!commit || !model || target === null) return;
    applyTableEdit(view, model.from, scope === "row" ? moveRowToEdit(view.state, model, index, target, 0) : moveColumnToEdit(view.state, model, index, target, 0));
  };
  const onMove = (event: PointerEvent) => {
    if (!started) {
      if (!shouldStartDrag(event.clientX - down.clientX, event.clientY - down.clientY)) return;
      started = true;
      wrap.dataset.dragging = scope;
    }
    const cell = document.elementFromPoint(event.clientX, event.clientY)?.closest("th, td");
    const input = cell && wrap.contains(cell) ? cell.querySelector<HTMLTextAreaElement>("textarea") : null;
    const at = input ? (scope === "row" ? Math.max(1, Number(input.dataset.row)) : Number(input.dataset.column)) : null;
    target = at === index ? null : at;
    mark(target);
  };
  const onUp = () => { finish(true); };
  const onKey = (event: KeyboardEvent) => {
    if (event.key !== "Escape") return;
    event.preventDefault();
    finish(false);
  };
  // 表头行不能移动：它的行把手只开菜单。
  if (scope === "row" && index === 0) return;
  window.addEventListener("pointermove", onMove);
  window.addEventListener("pointerup", onUp);
  window.addEventListener("keydown", onKey, true);
}

/** 右键的目标若是某张表的一格，给出它的菜单对象（行列取自输入框，不取编辑器选区）。 */
export function tableCellTargetOf(target: EventTarget | null): TableMenuTarget | null {
  if (!(target instanceof HTMLTextAreaElement) || !target.classList.contains("luna-table-preview__input")) return null;
  const wrap = target.closest<HTMLElement>(".luna-table-preview");
  const model = wrap ? models.get(wrap) : undefined;
  if (!model) return null;
  return { tableFrom: model.from, row: Number(target.dataset.row), col: Number(target.dataset.column), scope: "cell" };
}

/**
 * 落一个结构动作（`table-structure.ts`）：一笔事务、一步撤销；落点按变更后的表格换算成源码
 * 位置，再把焦点送进那一格。
 */
export function applyTableEdit(view: EditorView, tableFrom: number, edit: TableEdit | null): boolean {
  if (!edit) return false;
  const next = view.state.update({ changes: edit.changes }).state;
  const from = view.state.changes(edit.changes).mapPos(tableFrom, -1);
  const model = tableAt(next, from);
  const row = model?.rows[Math.min(edit.row, model.rows.length - 1)];
  view.dispatch({
    changes: edit.changes,
    selection: row ? { anchor: row.cells[edit.col]?.to ?? row.to } : undefined,
    scrollIntoView: true,
    userEvent: "input",
  });
  if (model) focusCell(view, from, Math.min(edit.row, model.rows.length - 1), Math.min(edit.col, model.columnCount - 1));
  return true;
}

/** 按编辑器光标所在的格聚焦（撤销 / 粘贴 / 插入表格之后）。 */
function focusSelectedCell(view: EditorView) {
  const model = tableAt(view.state, view.state.selection.main.head);
  if (!model) return;
  const { row, col } = cellCoordsAt(model, view.state.selection.main.head);
  focusCell(view, model.from, row, col);
}

/** 构建表格预览装饰（state 级纯函数，可无头测试）。 */
export function buildTablePreviewDecorations(state: EditorState, labels?: TableLabels): DecorationSet {
  const decos: Range<Decoration>[] = [];
  for (const model of collectTables(state, 0, state.doc.length)) {
    if (!state.selection.main.empty && state.selection.main.from < model.to && state.selection.main.to > model.from) continue;
    const source = state.doc.sliceString(model.from, model.to);
    decos.push(
      Decoration.replace({
        widget: new TablePreviewWidget(model, source, labels),
        block: true,
      }).range(model.from, model.to),
    );
  }
  return Decoration.set(decos, true);
}

/**
 * 从正文用 ↓ / ↑ 进表格：光标所在行的下一行 / 上一行就是表格的首行 / 末行时，焦点进该行首格。
 * 不加这一条，光标会越过整块部件，键盘根本进不了表格（只能用鼠标点）。
 */
function enterTableVertically(view: EditorView, down: boolean): boolean {
  const selection = view.state.selection.main;
  if (!selection.empty) return false;
  const doc = view.state.doc;
  const line = doc.lineAt(selection.head);
  const targetNumber = down ? line.number + 1 : line.number - 1;
  if (targetNumber < 1 || targetNumber > doc.lines) return false;
  // 折行的长段落：光标还在中间的视觉行时，方向键先在行内移动。
  try {
    const moved = view.moveVertically(selection, down);
    if (moved.head !== selection.head && doc.lineAt(moved.head).number === line.number) return false;
  } catch {
    // 无布局环境量不出视觉行，按逻辑行处理。
  }
  const target = doc.line(targetNumber);
  const model = tableAt(view.state, target.from);
  if (!model || (down ? model.from !== target.from : model.to !== target.to)) return false;
  const row = down ? 0 : model.rows.length - 1;
  view.dispatch({ selection: { anchor: model.rows[row].cells[0]?.to ?? model.rows[row].to }, scrollIntoView: true });
  focusCell(view, model.from, row, 0);
  return true;
}

/**
 * 紧贴表格下方的空行行首按退格：先选中整张表（网格态、`table` 选区），再按一次删除——
 * 不加这一步，退格只会把空行删掉、让下一段并进表格（Obsidian 1.5 同一做法）。
 */
function selectTableAbove(view: EditorView): boolean {
  const selection = view.state.selection.main;
  if (!selection.empty) return false;
  const line = view.state.doc.lineAt(selection.head);
  if (line.text !== "" || line.number === 1) return false;
  const above = view.state.doc.line(line.number - 1);
  const model = tableAt(view.state, above.from);
  if (!model || model.to !== above.to) return false;
  const last = model.rows.length - 1;
  view.dispatch({ selection: { anchor: model.rows[last].cells[0]?.to ?? model.rows[last].to } });
  focusCell(view, model.from, last, 0, "end", wholeTableRange(model));
  return true;
}

/**
 * 在紧贴表格的空行上打字 / 粘贴之前，先把落点挪成独立段落（规格见 `separateFromTableAboveSpec`）。
 * 放在 keydown 而不是事务过滤器里：输入法组字一旦开始，再改它所在的行会把候选冲掉——
 * 所以只在第一键（229 且尚未组字）时挪，组字中不碰文档。与随后打的字记作同一种输入，
 * 一次撤销一起回去。
 */
function separateFromTableAbove(view: EditorView): void {
  const selection = view.state.selection;
  if (view.composing || selection.ranges.length !== 1 || !selection.main.empty) return;
  const spec = separateFromTableAboveSpec(view.state, selection.main.head);
  if (!spec) return;
  view.dispatch({
    changes: { from: spec.from, to: spec.to, insert: spec.insert },
    selection: { anchor: spec.anchor },
    userEvent: "input.type",
  });
}

/** 这一下 keydown 会落下文字：可打印键（不带 ⌘ / Ctrl），或输入法的第一键。 */
function typesText(event: KeyboardEvent): boolean {
  if (event.isComposing) return false;
  return event.keyCode === 229 || (event.key.length === 1 && !event.metaKey && !event.ctrlKey);
}

export function tablePreviewExtension(labels?: TableLabels): Extension {
  const focusRequests = EditorView.updateListener.of((update) => {
    if (update.transactions.some((tr) => tr.effects.some((effect) => effect.is(focusTableCellEffect)))) {
      focusSelectedCell(update.view);
    }
  });
  const field = StateField.define<DecorationSet>({
    create(state) {
      return buildTablePreviewDecorations(state, labels);
    },
    update(deco, tr) {
      // 语法树异步推进（增量解析完成）不带 docChanged/selection，也需重建，
      // 否则长文档尾部的表格在首次解析完成前保持源码不渲染。
      // 选区只影响「非空选区跨入表格时显示源码」：前后都是空选区（光标移动）就不必重建。
      const selectionMatters = tr.selection && !(tr.startState.selection.main.empty && tr.state.selection.main.empty);
      if (tr.docChanged || selectionMatters || syntaxTree(tr.state) !== syntaxTree(tr.startState)) {
        return buildTablePreviewDecorations(tr.state, labels);
      }
      return deco;
    },
    provide(f) {
      return EditorView.decorations.from(f);
    },
  });
  const enterFromBody = Prec.high(keymap.of([
    { key: "ArrowDown", run: (view) => enterTableVertically(view, true) },
    { key: "ArrowUp", run: (view) => enterTableVertically(view, false) },
    { key: "Backspace", run: selectTableAbove },
  ]));
  // 点到表格之外：清掉各表的网格选区（焦点去了菜单时选区刻意留着，见单元格的 blur）。
  const clearRanges = EditorView.domEventHandlers({
    mousedown(event, view) {
      const inside = event.target instanceof Element ? event.target.closest(".luna-table-preview") : null;
      for (const wrap of view.dom.querySelectorAll<HTMLElement>(".luna-table-preview")) {
        if (wrap !== inside) setRange(wrap, null);
      }
      return false;
    },
  });
  const separateTyping = EditorView.domEventHandlers({
    keydown(event, view) {
      if (typesText(event)) separateFromTableAbove(view);
      return false;
    },
    paste(_event, view) {
      separateFromTableAbove(view);
      return false;
    },
  });
  return [field, focusRequests, enterFromBody, clearRanges, separateTyping];
}
