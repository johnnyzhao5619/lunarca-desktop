// 插入表格的网格尺寸选择器（对标方案 T5；Google Docs / Typora 的「选几行几列」）。
//
// 一个挂在光标处的 CodeMirror 浮层：悬停或方向键选尺寸，点击 / Enter 插入，Esc 取消。
// 直接回车就是默认的 3 列 × 3 行（表头 + 两行正文）。焦点始终留在正文，键位由高优先级键表
// 在浮层开着时接管；正文一有改动或光标挪走，浮层即关。

import { Prec, StateEffect, StateField, type Extension } from "@codemirror/state";
import { EditorView, keymap, showTooltip, type Tooltip, type TooltipView } from "@codemirror/view";
import { applyTableSpec, insertTableSpec } from "./table-edit";
import { tableInsertHandler } from "./table-model";

export interface TablePickerLabels {
  /** 浮层的无障碍名称。 */
  title: string;
  /** 当前尺寸的说明，如「3 列 × 3 行」。 */
  size: (columns: number, rows: number) => string;
}

interface PickerState {
  pos: number;
  columns: number;
  rows: number;
  /** 打开时建一次、开着期间不换：改尺寸只重绘，不重建浮层 DOM（重建会再触发悬停）。 */
  tooltip: Tooltip;
}

/** 网格上限：再大就该先插再用「+」加了。 */
export const PICKER_MAX = 8;
export const PICKER_DEFAULT = { columns: 3, rows: 3 } as const;

const openPicker = StateEffect.define<number | null>();
const resizePicker = StateEffect.define<{ columns: number; rows: number }>();

function definePickerField(labels: TablePickerLabels) {
  const field: StateField<PickerState | null> = StateField.define<PickerState | null>({
    create: () => null,
    update(value, tr) {
      let next = value;
      for (const effect of tr.effects) {
        if (effect.is(openPicker)) {
          next = effect.value === null ? null : { pos: effect.value, ...PICKER_DEFAULT, tooltip: pickerTooltip(labels, field, effect.value) };
        }
        if (effect.is(resizePicker) && next) next = { ...next, ...effect.value };
      }
      if (next && next === value && (tr.docChanged || (tr.selection && tr.state.selection.main.head !== value.pos))) return null;
      return next;
    },
    provide: (f) => showTooltip.from(f, (value) => value?.tooltip ?? null),
  });
  return field;
}

function clampSize(columns: number, rows: number) {
  return {
    columns: Math.max(1, Math.min(PICKER_MAX, columns)),
    rows: Math.max(2, Math.min(PICKER_MAX, rows)),
  };
}

function insertPicked(view: EditorView, columns: number, rows: number) {
  view.dispatch({ effects: openPicker.of(null) });
  applyTableSpec(view, insertTableSpec(view.state, columns, rows), { focusCell: true });
}

function pickerTooltip(labels: TablePickerLabels, field: StateField<PickerState | null>, pos: number): Tooltip {
  return {
    pos,
    above: false,
    create(view): TooltipView {
      const dom = document.createElement("div");
      dom.className = "luna-table-picker";
      dom.setAttribute("role", "dialog");
      dom.setAttribute("aria-label", labels.title);
      const grid = document.createElement("div");
      grid.className = "luna-table-picker__grid";
      const caption = document.createElement("div");
      caption.className = "luna-table-picker__caption";
      caption.setAttribute("aria-live", "polite");
      const cells: HTMLButtonElement[] = [];
      for (let r = 1; r <= PICKER_MAX; r++) {
        for (let c = 1; c <= PICKER_MAX; c++) {
          const cell = document.createElement("button");
          cell.type = "button";
          cell.className = "luna-table-picker__cell";
          cell.dataset.row = String(r);
          cell.dataset.column = String(c);
          cell.tabIndex = -1;
          cell.setAttribute("aria-label", labels.size(c, r));
          // 不抢正文焦点：键位与输入法都还在编辑器里。
          cell.addEventListener("mousedown", (event) => { event.preventDefault(); });
          cell.addEventListener("mouseenter", () => {
            view.dispatch({ effects: resizePicker.of(clampSize(c, r)) });
          });
          cell.addEventListener("click", () => {
            const size = clampSize(c, r);
            insertPicked(view, size.columns, size.rows);
          });
          cells.push(cell);
          grid.appendChild(cell);
        }
      }
      dom.append(grid, caption);
      const paint = (current: { columns: number; rows: number }) => {
        for (const cell of cells) {
          const on = Number(cell.dataset.row) <= current.rows && Number(cell.dataset.column) <= current.columns;
          cell.toggleAttribute("data-on", on);
        }
        caption.textContent = labels.size(current.columns, current.rows);
      };
      const initial = view.state.field(field);
      if (initial) paint(initial);
      return {
        dom,
        update(update) {
          const current = update.state.field(field);
          if (current) paint(current);
        },
      };
    },
  };
}

/** 网格尺寸选择器：接管「插入表格」，浮层开着时方向键调尺寸、Enter 插入、Esc 取消。 */
export function tablePickerExtension(labels: TablePickerLabels): Extension {
  const pickerField = definePickerField(labels);
  const resize = (dColumns: number, dRows: number) => (view: EditorView) => {
    const current = view.state.field(pickerField);
    if (!current) return false;
    view.dispatch({ effects: resizePicker.of(clampSize(current.columns + dColumns, current.rows + dRows)) });
    return true;
  };
  return [
    pickerField,
    tableInsertHandler.of((view) => {
      // 从工具栏 / 菜单打开时焦点可能不在正文：键位要由编辑器接住。
      view.focus();
      view.dispatch({ effects: openPicker.of(view.state.selection.main.head) });
      return true;
    }),
    Prec.highest(
      keymap.of([
        { key: "ArrowRight", run: resize(1, 0) },
        { key: "ArrowLeft", run: resize(-1, 0) },
        { key: "ArrowDown", run: resize(0, 1) },
        { key: "ArrowUp", run: resize(0, -1) },
        {
          key: "Enter",
          run: (view) => {
            const current = view.state.field(pickerField);
            if (!current) return false;
            insertPicked(view, current.columns, current.rows);
            return true;
          },
        },
        {
          key: "Escape",
          run: (view) => {
            if (!view.state.field(pickerField)) return false;
            view.dispatch({ effects: openPicker.of(null) });
            return true;
          },
        },
      ]),
    ),
  ];
}
