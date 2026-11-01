// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from "vitest";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { markdown } from "@codemirror/lang-markdown";
import { commands, type HtmlPasteDto } from "../../ipc/bindings";
import { pasteContent, pasteFromClipboard, type PasteAssetsContext } from "./paste-assets";
import { pasteRangeField } from "./paste-range";
vi.mock("../../ipc/bindings", () => ({ commands: { convertClipboardHtml: vi.fn(), readClipboardContent: vi.fn() } }));
const views: EditorView[] = [];
function mount(doc = "abcd", anchor = 2) {
  const parent = document.createElement("div"); document.body.appendChild(parent);
  const view = new EditorView({ parent, state: EditorState.create({ doc, selection: { anchor }, extensions: [markdown(), pasteRangeField] }) });
  views.push(view); return view;
}
function context(): PasteAssetsContext {
  return { getNotePath: () => "test.md", ensureNotePath: async () => "test.md", localizeRemoteImages: () => false,
    onStatus: vi.fn(), richTextEnabled: () => true, confirmConversion: vi.fn(async () => null),
    messages: { saving: "saving", saveFirst: "saveFirst", failed: () => "failed", downloading: "downloading", converting: "converting", cancelled: "cancelled", conversionFailed: "failed", readFailed: "read failed" } };
}
afterEach(() => { views.splice(0).forEach((view) => view.destroy()); document.body.replaceChildren(); vi.resetAllMocks(); });
describe("structured paste integration", () => {
  it("空剪贴板不删除选区，也不假称已经粘贴纯文本", async () => {
    const view = mount(); const ctx = context(); view.dispatch({ selection: { anchor: 1, head: 3 } });
    vi.mocked(commands.readClipboardContent).mockResolvedValue({ text: null, html: null });
    await pasteFromClipboard(view, ctx, "markdown");
    expect(view.state.doc.toString()).toBe("abcd");
    expect(view.state.selection.main.from).toBe(1); expect(view.state.selection.main.to).toBe(3);
    expect(ctx.onStatus).toHaveBeenLastCalledWith("read failed");
    await pasteContent(view, "", null, ctx);
    expect(view.state.doc.toString()).toBe("abcd");
  });
  it("源码普通粘贴保持字面，显式转换复用预览且取消不改正文", async () => {
    const view = mount(); const ctx = context(); ctx.richTextEnabled = () => false;
    vi.mocked(commands.readClipboardContent).mockResolvedValue({ text: "plain", html: "<b>plain</b>" });
    await pasteFromClipboard(view, ctx);
    expect(view.state.doc.toString()).toBe("abplaincd");
    expect(commands.convertClipboardHtml).not.toHaveBeenCalled();
    vi.mocked(commands.convertClipboardHtml).mockResolvedValue({ status: "ok", data: { markdown: "**plain**", warnings: ["styles_removed"], block: false, header_markdown: null, table_cells: null } });
    await pasteFromClipboard(view, ctx, "markdown");
    expect(ctx.confirmConversion).toHaveBeenCalled();
    expect(view.state.doc.toString()).toBe("abplaincd");
  });
  it("源码显式转换等待时换光标仍落原范围，普通图片网址不触发下载", async () => {
    const view = mount(); const ctx = context(); ctx.richTextEnabled = () => false;
    let resolve!: (value: { status: "ok"; data: HtmlPasteDto }) => void;
    vi.mocked(commands.readClipboardContent).mockResolvedValue({ text: "heading", html: "<h2>heading</h2>" });
    vi.mocked(commands.convertClipboardHtml).mockImplementation(() => new Promise((done) => { resolve = done; }));
    const pending = pasteFromClipboard(view, ctx, "markdown");
    await Promise.resolve(); view.dispatch({ selection: { anchor: 4 } });
    resolve({ status: "ok", data: { markdown: "## heading", warnings: [], block: true, header_markdown: null, table_cells: null } }); await pending;
    expect(view.state.doc.toString()).toBe("ab\n\n## heading\n\ncd");
    const url = mount(); ctx.localizeRemoteImages = () => true;
    await pasteContent(url, "https://example.com/image.png", null, ctx, "markdown");
    expect(url.state.doc.toString()).toBe("abhttps://example.com/image.pngcd");
  });
  it("转换等待期间移动光标，结果仍落在原始位置", async () => {
    const view = mount(); let resolve!: (value: { status: "ok"; data: HtmlPasteDto }) => void;
    vi.mocked(commands.convertClipboardHtml).mockImplementation(() => new Promise((done) => { resolve = done; }));
    const pending = pasteContent(view, "网页", "<h1>网页</h1>", context());
    view.dispatch({ selection: { anchor: 4 } });
    resolve({ status: "ok", data: { markdown: "# 网页", warnings: [], block: true, header_markdown: null, table_cells: null } }); await pending;
    expect(view.state.doc.toString()).toBe("ab\n\n# 网页\n\ncd");
  });
  it("损失预览取消或剪贴板读取失败不会删选区", async () => {
    const view = mount(); const ctx = context(); view.dispatch({ selection: { anchor: 1, head: 3 } });
    vi.mocked(commands.convertClipboardHtml).mockResolvedValue({ status: "ok", data: { markdown: "table", warnings: ["missing_header"], block: true, header_markdown: null, table_cells: null } });
    await pasteContent(view, "plain", "<table></table>", ctx);
    expect(ctx.confirmConversion).toHaveBeenCalled(); expect(view.state.doc.toString()).toBe("abcd");
    vi.mocked(commands.readClipboardContent).mockRejectedValue(new Error("unavailable"));
    await pasteFromClipboard(view, ctx); expect(view.state.doc.toString()).toBe("abcd");
    expect(ctx.onStatus).toHaveBeenLastCalledWith("read failed");
  });
  it("代码块内保留字面文本，不请求结构转换", async () => {
    const view = mount("```html\n\n```", 8);
    await pasteContent(view, "<b>literal</b>", "<b>literal</b>", context());
    expect(commands.convertClipboardHtml).not.toHaveBeenCalled();
    expect(view.state.doc.toString()).toContain("<b>literal</b>");
  });
  it("显式选择首行表头后仍按块插入且锚定原范围", async () => {
    const view = mount(); const ctx = context();
    const alternative = "| 名称 | 数量 |\n| --- | --- |\n| 苹果 | 3 |";
    vi.mocked(commands.convertClipboardHtml).mockResolvedValue({ status: "ok", data: {
      markdown: "|  |  |\n| --- | --- |\n| 名称 | 数量 |\n| 苹果 | 3 |", warnings: ["missing_header"],
      block: true, header_markdown: alternative, table_cells: [["名称", "数量"], ["苹果", "3"]],
    } });
    ctx.confirmConversion = vi.fn(async () => ({ kind: "converted" as const, markdown: alternative }));
    await pasteContent(view, "名称\t数量\n苹果\t3", null, ctx);
    expect(view.state.doc.toString()).toBe(`ab\n\n${alternative}\n\ncd`);
  });
  it("源码和代码块不把网址选区包装为链接；右键普通网址与键盘共用规则", async () => {
    const view = mount(); view.dispatch({ selection: { anchor: 1, head: 3 } });
    await pasteContent(view, "https://example.com", null, context());
    expect(view.state.doc.toString()).toBe("a[bc](https://example.com)d");
    const source = mount(); source.dispatch({ selection: { anchor: 1, head: 3 } });
    const ctx = context(); ctx.richTextEnabled = () => false; ctx.localizeRemoteImages = () => true;
    await pasteContent(source, "https://example.com/test.png", null, ctx);
    expect(source.state.doc.toString()).toBe("ahttps://example.com/test.pngd");
    expect(commands.convertClipboardHtml).not.toHaveBeenCalled();
  });
});
