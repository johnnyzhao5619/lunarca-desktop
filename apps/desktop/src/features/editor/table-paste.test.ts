// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from "vitest";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { history, undo } from "@codemirror/commands";
import { commands, type HtmlPasteDto } from "../../ipc/bindings";
import type { PasteAssetsContext } from "./paste-assets";
import { pasteRangeField } from "./paste-range";
import { tableAt } from "./table-model";
import { tableFillSpec } from "./table-edit";
import { tablePreviewExtension } from "./table-preview";
import { pasteTableCell } from "./table-paste";

vi.mock("../../ipc/bindings", () => ({ commands: { convertClipboardHtml: vi.fn() } }));
const documentText = "前文\n\n| 名称 | 数量 |\n| --- | ---: |\n| 苹果 | 3 |\n| 梨 | 12 |\n\n后文";
const views: EditorView[] = [];
const converted: HtmlPasteDto = { markdown: "unused", block: true, warnings: ["missing_header"],
  header_markdown: "unused alternative", table_cells: [["4", "新增"], ["5", "续行"], ["6", "末行"]] };
function mount() {
  const parent = document.createElement("div"); document.body.appendChild(parent);
  const view = new EditorView({ parent, state: EditorState.create({ doc: documentText, extensions: [
    history(), markdown({ base: markdownLanguage }), pasteRangeField,
    tablePreviewExtension({ cell: (r, c) => `${r}/${c}`, rowHandle: (r) => `row ${r}`, columnHandle: (c) => `column ${c}`, appendRow: "append row", appendColumn: "append column" }),
  ] }) });
  views.push(view);
  const input = view.dom.querySelector<HTMLTextAreaElement>('textarea[data-row="1"][data-column="1"]')!;
  input.focus(); input.setSelectionRange(0, 1);
  return { view, input, model: tableAt(view.state, documentText.indexOf("苹果"))! };
}
function context(): PasteAssetsContext {
  return { getNotePath: () => "test.md", ensureNotePath: async () => "test.md", localizeRemoteImages: () => false,
    onStatus: vi.fn(), richTextEnabled: () => true, confirmConversion: vi.fn(async (preview) => ({ kind: "converted" as const, markdown: preview.markdown })),
    messages: { saving: "saving", saveFirst: "saveFirst", failed: () => "failed", downloading: "downloading", converting: "converting", cancelled: "cancelled", conversionFailed: "failed", readFailed: "read failed" } };
}
afterEach(() => { views.splice(0).forEach((view) => view.destroy()); document.body.replaceChildren(); vi.resetAllMocks(); });
describe("existing table rectangle paste", () => {
  it("撤销 / 重做行列扩展后焦点回到相应单元格，可继续输入", async () => {
    const { view, input, model } = mount();
    vi.mocked(commands.convertClipboardHtml).mockResolvedValue({ status: "ok", data: converted });
    await pasteTableCell(view, input, model, 1, 1, "4\t新增", null, context());
    const filledInput = view.dom.querySelector<HTMLTextAreaElement>('textarea[data-row="1"][data-column="1"]')!;
    filledInput.focus(); filledInput.dispatchEvent(new KeyboardEvent("keydown", { key: "z", metaKey: true, bubbles: true }));
    await Promise.resolve();
    expect(view.state.doc.toString()).toBe(documentText);
    const restored = view.dom.querySelector<HTMLTextAreaElement>('textarea[data-row="1"][data-column="1"]')!;
    expect(document.activeElement).toBe(restored);
    restored.dispatchEvent(new KeyboardEvent("keydown", { key: "z", metaKey: true, shiftKey: true, bubbles: true }));
    await Promise.resolve();
    expect(tableAt(view.state, view.state.selection.main.head)!.columnCount).toBe(3);
    expect(document.activeElement).toBe(view.dom.querySelector('textarea[data-row="1"][data-column="1"]'));
  });
  it("展示准确覆盖与扩展范围，保留矩形外内容和右对齐，一次撤销整个填充", async () => {
    const { view, input, model } = mount(); const ctx = context();
    vi.mocked(commands.convertClipboardHtml).mockResolvedValue({ status: "ok", data: converted });
    expect(await pasteTableCell(view, input, model, 1, 1, "4\t新增\n5\t续行\n6\t末行", null, ctx)).toBe(true);
    const preview = vi.mocked(ctx.confirmConversion).mock.calls[0];
    expect(preview[2]).toEqual({ row: 2, column: 2, rows: 3, columns: 2, addedRows: 1, addedColumns: 1, replacedCells: 2 });
    expect(preview[0].warnings).toEqual([]);
    expect(preview[0].header_markdown).toBeNull();
    const next = tableAt(view.state, view.state.selection.main.head)!;
    expect(next.rows.map((row) => row.cells.map((cell) => cell.text))).toEqual([
      ["名称", "数量", ""], ["苹果", "4", "新增"], ["梨", "5", "续行"], ["", "6", "末行"],
    ]);
    expect(next.align).toEqual([null, "right", null]);
    expect(view.state.doc.toString()).toMatch(/^前文\n\n/);
    expect(view.state.doc.toString()).toMatch(/\n\n后文$/);
    input.blur(); expect(undo(view)).toBe(true); expect(view.state.doc.toString()).toBe(documentText);
  });
  it("用户取消或换文档不改表格", async () => {
    const { view, input, model } = mount(); const ctx = context();
    vi.mocked(commands.convertClipboardHtml).mockResolvedValue({ status: "ok", data: converted });
    ctx.confirmConversion = vi.fn(async () => null);
    expect(await pasteTableCell(view, input, model, 1, 1, "4\t新增", null, ctx)).toBe(false);
    expect(view.state.doc.toString()).toBe(documentText);
    ctx.confirmConversion = vi.fn(async (preview) => { ctx.getNotePath = () => "other.md"; return { kind: "converted" as const, markdown: preview.markdown }; });
    expect(await pasteTableCell(view, input, model, 1, 1, "4\t新增", null, ctx)).toBe(false);
    expect(view.state.doc.toString()).toBe(documentText);
  });
  it("预览期间表内任一修改取消迟到结果；表前修改只映射位置", async () => {
    const { view, input, model } = mount(); const ctx = context();
    vi.mocked(commands.convertClipboardHtml).mockResolvedValue({ status: "ok", data: converted });
    ctx.confirmConversion = vi.fn(async (preview) => {
      view.dispatch({ changes: { from: 0, insert: "新前言\n" } }); return { kind: "converted" as const, markdown: preview.markdown };
    });
    expect(await pasteTableCell(view, input, model, 1, 1, "4\t新增", null, ctx)).toBe(true);
    expect(view.state.doc.toString().startsWith("新前言\n前文")).toBe(true);
    const second = mount();
    ctx.confirmConversion = vi.fn(async (preview) => {
      second.view.dispatch({ changes: { from: second.model.rows[2].cells[0].from, insert: "保留" } }); return { kind: "converted" as const, markdown: preview.markdown };
    });
    expect(await pasteTableCell(second.view, second.input, second.model, 1, 1, "4\t新增", null, ctx)).toBe(false);
    expect(second.view.state.doc.toString()).toBe(documentText.replace("梨", "保留梨"));
  });
  it("纯文本只替换当前格的真实选区，保留相邻格；转换异常不丢当前内容", async () => {
    const { view, input, model } = mount(); const ctx = context();
    vi.mocked(commands.convertClipboardHtml).mockResolvedValue({ status: "ok", data: converted });
    ctx.confirmConversion = vi.fn(async (_preview, plain) => ({ kind: "plain" as const, text: plain }));
    expect(await pasteTableCell(view, input, model, 1, 1, "4\t5\n6\t7", null, ctx)).toBe(true);
    expect(view.state.doc.toString()).toBe(documentText.replace("苹果 | 3", "苹果 | 4\t5<br>6\t7"));
    const second = mount(); vi.mocked(commands.convertClipboardHtml).mockRejectedValue(new Error("failed"));
    expect(await pasteTableCell(second.view, second.input, second.model, 1, 1, "4\t5", null, ctx)).toBe(false);
    expect(second.view.state.doc.toString()).toBe(documentText);
  });
  it("矩形中转义管道和多行内容不破坏 GFM；不规则来源未指定的格子保持", () => {
    const { model } = mount();
    const filled = tableFillSpec(model, 1, 0, [["甲\\|乙", "多\n行"], ["保留邻格"]])!;
    expect(filled.spec.insert).toContain("甲\\|乙");
    expect(filled.spec.insert).toContain("多<br>行");
    expect(filled.spec.insert).toContain("12");
  });
});
