// 表格菜单（对标方案 T2）：单元格右键、行把手、列把手三种入口共用这一份条目来源。
// 条目只描述「此刻这张表的这一格 / 行 / 列能做什么」，定位、键盘与关闭归唯一宿主（PRD-07-059）；
// 每个动作都走 `table-structure.ts` 的只改受影响行的纯函数，一笔事务、一步撤销。

import type { EditorView } from "@codemirror/view";
import { buildMenu, separator, type ContextMenuEntry, type ContextMenuItem } from "../../lib/context-menu";
import type { TableAlign } from "../../lib/markdown";
import { tableAt, type TableModel } from "./table-model";
import { applyTableEdit, type TableMenuTarget } from "./table-preview";
import type { TableAiTask } from "./FloatingMenu";
import {
  alignColumnEdit,
  deleteColumnEdit,
  deleteRowEdit,
  deleteTableEdit,
  insertColumnEdit,
  insertRowEdit,
  moveColumnEdit,
  moveRowEdit,
  sortRowsEdit,
  tableToTextEdit,
  tidyTableEdit,
  type TableEdit,
} from "./table-structure";

/** 表格菜单的文案（调用方过 i18n 注入）。 */
export interface TableMenuLabels {
  insert: string;
  rowAbove: string;
  rowBelow: string;
  columnLeft: string;
  columnRight: string;
  moveRowUp: string;
  moveRowDown: string;
  moveColumnLeft: string;
  moveColumnRight: string;
  deleteRow: string;
  deleteColumn: string;
  headerRowFixed: string;
  lastColumnFixed: string;
  sortAscending: string;
  sortDescending: string;
  align: string;
  alignLeft: string;
  alignCenter: string;
  alignRight: string;
  alignNone: string;
  copyAs: string;
  copyMarkdown: string;
  copyCsv: string;
  copyRich: string;
  tidySource: string;
  toText: string;
  deleteTable: string;
  aiSummarize: string;
  aiFillColumn: string;
}

const LABEL_KEYS = [
  "insert", "rowAbove", "rowBelow", "columnLeft", "columnRight", "moveRowUp", "moveRowDown", "moveColumnLeft",
  "moveColumnRight", "deleteRow", "deleteColumn", "headerRowFixed", "lastColumnFixed", "sortAscending",
  "sortDescending", "align", "alignLeft", "alignCenter", "alignRight", "alignNone", "copyAs", "copyMarkdown",
  "copyCsv", "copyRich", "tidySource", "toText", "deleteTable", "aiSummarize", "aiFillColumn",
] as const satisfies readonly (keyof TableMenuLabels)[];

/** 从 `editor` 命名空间取 `tableEditing.menu.*`。 */
export function tableMenuLabels(t: (key: string) => string): TableMenuLabels {
  return Object.fromEntries(LABEL_KEYS.map((key) => [key, t(`tableEditing.menu.${key}`)])) as unknown as TableMenuLabels;
}

export interface TableMenuOptions {
  view: EditorView;
  target: TableMenuTarget;
  labels: TableMenuLabels;
  /** 界面语言（排序按它比较：中文按拼音、数字按大小）。 */
  locale: string;
  /** 「复制表格为 ▸」：表格源码交出去，格式转换与写剪贴板归调用方。 */
  onCopy: (source: string, format: "markdown" | "csv" | "rich") => void;
  /** 表格 AI（对标方案 T6）：打开 AI 面板跑这项任务；不给（副库窗口）就不列 AI 项。 */
  onAi?: (task: TableAiTask) => void;
}

/** 按菜单打开时记下的位置重新取这张表：菜单开着期间文档不变，取不到就是表格已不在了。 */
function currentModel(view: EditorView, tableFrom: number): TableModel | null {
  const model = tableAt(view.state, tableFrom);
  return model && model.from === tableFrom ? model : null;
}

export function tableMenuItems(options: TableMenuOptions): ContextMenuItem[] {
  const { view, target, labels } = options;
  const model = currentModel(view, target.tableFrom);
  if (!model) return [];
  const { row, col, scope } = target;
  const run = (edit: (state: typeof view.state, model: TableModel) => TableEdit | null) => () => {
    const fresh = currentModel(view, target.tableFrom);
    if (fresh) applyTableEdit(view, fresh.from, edit(view.state, fresh));
  };
  const action = (id: string, label: string, onSelect: () => void, extra: { disabled?: boolean; title?: string; danger?: boolean } = {}): ContextMenuEntry => ({
    kind: "action",
    id,
    label,
    onSelect,
    ...extra,
  });
  const header = row === 0;
  const lastColumn = model.columnCount <= 1;

  const rowEntries: ContextMenuEntry[] = [
    action("row-above", labels.rowAbove, run((s, m) => insertRowEdit(s, m, row, col)), { disabled: header, title: header ? labels.headerRowFixed : undefined }),
    action("row-below", labels.rowBelow, run((s, m) => insertRowEdit(s, m, row + 1, col))),
    action("move-row-up", labels.moveRowUp, run((s, m) => moveRowEdit(s, m, row, -1, col)), { disabled: row <= 1 }),
    action("move-row-down", labels.moveRowDown, run((s, m) => moveRowEdit(s, m, row, 1, col)), { disabled: header || row >= model.rows.length - 1 }),
    action("delete-row", labels.deleteRow, run((s, m) => deleteRowEdit(s, m, row, col)), {
      disabled: header,
      title: header ? labels.headerRowFixed : undefined,
      danger: true,
    }),
  ];
  const alignItems: ContextMenuItem[] = (
    [
      ["left", labels.alignLeft],
      ["center", labels.alignCenter],
      ["right", labels.alignRight],
      [null, labels.alignNone],
    ] as const
  ).map(([align, label]) => ({
    kind: "action",
    id: `align-${align ?? "none"}`,
    label,
    checked: (model.align[col] ?? null) === align,
    onSelect: run((s, m) => alignColumnEdit(s, m, col, align as TableAlign, row)),
  }));
  const columnEntries: ContextMenuEntry[] = [
    action("column-left", labels.columnLeft, run((s, m) => insertColumnEdit(s, m, col, row))),
    action("column-right", labels.columnRight, run((s, m) => insertColumnEdit(s, m, col + 1, row))),
    action("move-column-left", labels.moveColumnLeft, run((s, m) => moveColumnEdit(s, m, col, -1, row)), { disabled: col === 0 }),
    action("move-column-right", labels.moveColumnRight, run((s, m) => moveColumnEdit(s, m, col, 1, row)), { disabled: col >= model.columnCount - 1 }),
    action("sort-ascending", labels.sortAscending, run((s, m) => sortRowsEdit(s, m, col, false, options.locale)), { disabled: model.rows.length < 3 }),
    action("sort-descending", labels.sortDescending, run((s, m) => sortRowsEdit(s, m, col, true, options.locale)), { disabled: model.rows.length < 3 }),
    { kind: "submenu", id: "align", label: labels.align, items: alignItems },
    action("delete-column", labels.deleteColumn, run((s, m) => deleteColumnEdit(s, m, col, row)), {
      disabled: lastColumn,
      title: lastColumn ? labels.lastColumnFixed : undefined,
      danger: true,
    }),
  ];
  const onAi = options.onAi;
  const columnName = model.rows[0]?.cells[col]?.text ?? "";
  const aiEntries: ContextMenuEntry[] = onAi
    ? [
        action("ai-fill-column", labels.aiFillColumn, () => onAi({ kind: "fill_column", column: col, header: columnName }), { disabled: model.rows.length < 2 }),
        action("ai-summarize", labels.aiSummarize, () => onAi({ kind: "explain" })),
      ]
    : [];
  if (scope === "row") return buildMenu(rowEntries);
  if (scope === "column") return buildMenu([...columnEntries, separator("ai"), ...aiEntries]);

  const source = () => view.state.sliceDoc(model.from, model.to);
  return buildMenu([
    {
      kind: "submenu",
      id: "table-insert",
      label: labels.insert,
      items: buildMenu([rowEntries[0], rowEntries[1], separator("insert"), columnEntries[0], columnEntries[1]]),
    },
    separator("structure"),
    ...rowEntries.slice(2),
    separator("columns"),
    ...columnEntries.slice(2),
    separator("table"),
    {
      kind: "submenu",
      id: "copy-as",
      label: labels.copyAs,
      items: [
        { kind: "action", id: "copy-markdown", label: labels.copyMarkdown, onSelect: () => options.onCopy(source(), "markdown") },
        { kind: "action", id: "copy-csv", label: labels.copyCsv, onSelect: () => options.onCopy(source(), "csv") },
        { kind: "action", id: "copy-rich", label: labels.copyRich, onSelect: () => options.onCopy(source(), "rich") },
      ],
    },
    separator("ai"),
    ...aiEntries,
    separator("source"),
    action("tidy-source", labels.tidySource, run((_s, m) => tidyTableEdit(m, row, col))),
    action("to-text", labels.toText, () => {
      const fresh = currentModel(view, target.tableFrom);
      if (!fresh) return;
      const edit = tableToTextEdit(view.state, fresh);
      view.dispatch({ changes: edit.changes, selection: { anchor: edit.anchor }, scrollIntoView: true, userEvent: "input" });
      view.focus();
    }),
    action("delete-table", labels.deleteTable, () => {
      const fresh = currentModel(view, target.tableFrom);
      if (!fresh) return;
      const edit = deleteTableEdit(view.state, fresh);
      view.dispatch({ changes: edit.changes, selection: { anchor: edit.anchor }, scrollIntoView: true, userEvent: "delete" });
      view.focus();
    }, { danger: true }),
  ]);
}
