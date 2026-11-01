import type { EditorView } from "@codemirror/view";
import { commands } from "../../ipc/bindings";
import type { PasteAssetsContext } from "./paste-assets";
import { cancelPaste, freezePaste, insertFrozenPaste, pasteRangeOf } from "./paste-range";
import { tableCellText, tableFillSpec } from "./table-edit";
import type { TableModel } from "./table-model";

/** 转换归 Rust；此处只冻结编辑范围、展示填充预览并应用 CM6 事务。 */
export async function pasteTableCell(view: EditorView, input: HTMLTextAreaElement, model: TableModel,
  row: number, column: number, text: string, html: string | null, ctx: PasteAssetsContext): Promise<boolean> {
  const cell = model.rows[row]?.cells[column];
  if (!cell) return false;
  const literal = tableCellText(input.value.slice(0, input.selectionStart) + text + input.value.slice(input.selectionEnd));
  const tableId = freezePaste(view, { from: model.from, to: model.to });
  const cellId = freezePaste(view, { from: cell.from, to: cell.to });
  const path = ctx.getNotePath();
  const current = () => view.dom.isConnected && ctx.getNotePath() === path && pasteRangeOf(view.state, tableId) !== null;
  try {
    ctx.onStatus(ctx.messages.converting);
    const result = await commands.convertClipboardHtml(html ?? "", text);
    if (!current()) { ctx.onStatus(ctx.messages.cancelled); return false; }
    if (result.status !== "ok") {
      ctx.onStatus(ctx.messages.conversionFailed);
      return insertFrozenPaste(view, cellId, literal);
    }
    const fill = result.data.table_cells && tableFillSpec(model, row, column, result.data.table_cells);
    if (!fill) {
      ctx.onStatus(text ? "" : ctx.messages.readFailed);
      return text ? insertFrozenPaste(view, cellId, literal) : false;
    }
    const preview = { ...result.data, markdown: fill.spec.insert, header_markdown: null,
      warnings: result.data.warnings.filter((warning) => warning !== "missing_header") };
    const choice = await ctx.confirmConversion(preview, text, fill.preview);
    if (!current() || choice === null) { ctx.onStatus(ctx.messages.cancelled); return false; }
    let inserted = false;
    if (choice.kind === "converted") {
      inserted = insertFrozenPaste(view, tableId, fill.spec.insert, false, fill.spec.anchor - model.from);
    } else if (text) inserted = insertFrozenPaste(view, cellId, literal);
    ctx.onStatus(inserted ? "" : ctx.messages.cancelled);
    return inserted;
  } catch {
    ctx.onStatus(ctx.messages.readFailed);
    return false;
  } finally {
    if (view.dom.isConnected) { cancelPaste(view, tableId); cancelPaste(view, cellId); }
  }
}
