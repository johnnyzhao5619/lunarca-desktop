// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from "vitest";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { commands } from "../../ipc/bindings";
import { editorSurfaceAt, logicalPoint, markEditorDropTarget } from "../../lib/editor-file-drop";
import { dropAt, type PasteAssetsContext } from "./paste-assets";
import { pasteRangeField } from "@lunarca/editor-core/paste-range";

vi.mock("../../ipc/bindings", () => ({
  commands: { importAssetPaths: vi.fn(), vaultLinkMarkdown: vi.fn(), discardPastedAssets: vi.fn() },
}));
const views: EditorView[] = [];
function mount(doc: string) {
  const parent = document.createElement("div");
  document.body.appendChild(parent);
  const view = new EditorView({ parent, state: EditorState.create({ doc, extensions: [pasteRangeField] }) });
  views.push(view);
  // happy-dom 量不了布局，posAtCoords 会抛；落点换算由 CodeMirror 负责，这里只给定位置。
  vi.spyOn(view, "posAtCoords").mockReturnValue(null);
  return view;
}
function context(notePath: string | null = "notes/a.md"): PasteAssetsContext {
  return { getNotePath: () => notePath, ensureNotePath: async () => notePath, localizeRemoteImages: () => false,
    onStatus: vi.fn(), richTextEnabled: () => true, confirmConversion: vi.fn(),
    messages: { saving: "saving", saveFirst: "saveFirst", failed: () => "failed", downloading: "downloading", converting: "converting", cancelled: "cancelled", conversionFailed: "failed", readFailed: "read failed" } };
}
afterEach(() => { views.splice(0).forEach((view) => view.destroy()); document.body.replaceChildren(); vi.resetAllMocks(); });

describe("系统文件拖进正文（B4）", () => {
  it("在落点处插入附件链接，图片用 ![]()，其余用 [名]()，多个文件各占一行", async () => {
    const view = mount("第一段\n第二段");
    vi.mocked(view.posAtCoords).mockReturnValue(3);
    vi.mocked(commands.importAssetPaths).mockResolvedValue({ status: "ok", data: ["assets/图.png", "assets/报告.pdf"] });
    await dropAt(view, { source: "system", paths: ["/tmp/图.png", "/tmp/报告.pdf"], x: 10, y: 10 }, context());
    expect(commands.importAssetPaths).toHaveBeenCalledWith("notes/a.md", ["/tmp/图.png", "/tmp/报告.pdf"]);
    expect(view.state.doc.toString()).toContain("![](assets/图.png)\n[报告.pdf](assets/报告.pdf)");
    expect(view.state.doc.toString().startsWith("第一段")).toBe(true);
    expect(view.state.doc.toString().endsWith("第二段")).toBe(true);
  });

  it("笔记还没落盘就不复制、不改正文，提示先保存", async () => {
    const view = mount("草稿");
    const ctx = context(null);
    await dropAt(view, { source: "system", paths: ["/tmp/x.png"], x: 0, y: 0 }, ctx);
    expect(commands.importAssetPaths).not.toHaveBeenCalled();
    expect(view.state.doc.toString()).toBe("草稿");
    expect(ctx.onStatus).toHaveBeenCalledWith("saveFirst");
  });

  it("全是文件夹（Rust 跳过、返回空）时正文不动", async () => {
    const view = mount("正文");
    vi.mocked(commands.importAssetPaths).mockResolvedValue({ status: "ok", data: [] });
    await dropAt(view, { source: "system", paths: ["/tmp/folder"], x: 0, y: 0 }, context());
    expect(view.state.doc.toString()).toBe("正文");
  });
});

describe("插入被取消时回收这次复制进来的文件（E5 真机 2026-10-06：撤销后留下 20 MB 的孤儿图）", () => {
  it("结果回来前切走了笔记：不插入，交给 Rust 回收刚复制的附件", async () => {
    const view = mount("正文");
    let notePath = "notes/a.md";
    const ctx = { ...context(), getNotePath: () => notePath };
    vi.mocked(commands.importAssetPaths).mockImplementation(async () => {
      notePath = "notes/b.md";
      return { status: "ok", data: ["assets/图.png"] };
    });
    await dropAt(view, { source: "system", paths: ["/tmp/图.png"], x: 0, y: 0 }, ctx);
    expect(view.state.doc.toString()).toBe("正文");
    expect(commands.discardPastedAssets).toHaveBeenCalledWith("notes/a.md", ["assets/图.png"]);
  });

  it("拖的是文件树条目（库里原有的文件）：取消也绝不回收", async () => {
    const view = mount("正文");
    let notePath = "notes/a.md";
    const ctx = { ...context(), getNotePath: () => notePath };
    vi.mocked(commands.vaultLinkMarkdown).mockImplementation(async () => {
      notePath = "notes/b.md";
      return { status: "ok", data: ["[会议纪要](../会议/会议纪要.md)"] };
    });
    await dropAt(view, { source: "vault", paths: ["会议/会议纪要.md"], x: 0, y: 0 }, ctx);
    expect(commands.discardPastedAssets).not.toHaveBeenCalled();
  });

  it("正常插入时不回收", async () => {
    const view = mount("正文");
    vi.mocked(commands.importAssetPaths).mockResolvedValue({ status: "ok", data: ["assets/图.png"] });
    await dropAt(view, { source: "system", paths: ["/tmp/图.png"], x: 0, y: 0 }, context());
    expect(view.state.doc.toString()).toContain("![](assets/图.png)");
    expect(commands.discardPastedAssets).not.toHaveBeenCalled();
  });
});

describe("文件树条目拖进正文（B4）", () => {
  it("插入 Rust 给的相对链接，不复制文件", async () => {
    const view = mount("引言");
    vi.mocked(view.posAtCoords).mockReturnValue(2);
    vi.mocked(commands.vaultLinkMarkdown).mockResolvedValue({ status: "ok", data: ["[会议纪要](../会议/会议纪要.md)"] });
    await dropAt(view, { source: "vault", paths: ["会议/会议纪要.md"], x: 5, y: 5 }, context());
    expect(commands.vaultLinkMarkdown).toHaveBeenCalledWith("notes/a.md", ["会议/会议纪要.md"]);
    expect(commands.importAssetPaths).not.toHaveBeenCalled();
    expect(view.state.doc.toString()).toContain("[会议纪要](../会议/会议纪要.md)");
  });
});

describe("拖放落点分流", () => {
  it("物理像素按缩放比换算成视口坐标", () => {
    vi.spyOn(window, "devicePixelRatio", "get").mockReturnValue(2);
    expect(logicalPoint({ x: 200, y: 100 })).toEqual({ x: 100, y: 50 });
  });

  it("webview 框比视口高出的那一截（macOS 标题栏）从 y 里扣掉；一样高时不动", () => {
    vi.spyOn(window, "devicePixelRatio", "get").mockReturnValue(2);
    vi.spyOn(window, "innerHeight", "get").mockReturnValue(782);
    // 框 810 逻辑像素（含 28 的标题栏），视口 782：落点上移 28。
    expect(logicalPoint({ x: 200, y: 1000 }, 1620)).toEqual({ x: 100, y: 472 });
    expect(logicalPoint({ x: 200, y: 1000 }, 1564)).toEqual({ x: 100, y: 500 });
  });

  it("只认可见的编辑器正文；阅读态隐藏的正文不接收", () => {
    const surface = document.createElement("div");
    surface.className = "luna-editor__surface";
    const line = document.createElement("div");
    surface.appendChild(line);
    document.body.appendChild(surface);
    const hit = vi.spyOn(document, "elementFromPoint").mockReturnValue(line);
    expect(editorSurfaceAt(1, 1)).toBe(surface);
    surface.dataset.hidden = "true";
    expect(editorSurfaceAt(1, 1)).toBeNull();
    hit.mockReturnValue(document.body);
    expect(editorSurfaceAt(1, 1)).toBeNull();
  });

  it("放置态高亮只落在当前那个正文上", () => {
    const [a, b] = [document.createElement("div"), document.createElement("div")];
    for (const element of [a, b]) {
      element.className = "luna-editor__surface";
      document.body.appendChild(element);
    }
    markEditorDropTarget(a);
    expect(a.dataset.fileDrop).toBe("true");
    markEditorDropTarget(b);
    expect(a.dataset.fileDrop).toBeUndefined();
    expect(b.dataset.fileDrop).toBe("true");
    markEditorDropTarget(null);
    expect(b.dataset.fileDrop).toBeUndefined();
  });
});
