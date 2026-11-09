// 编辑器粘贴增强（PRD-06-012，Obsidian 附件粘贴对标）：
// ① 剪贴板图片/文件 → Rust 存笔记同级 assets/ → 回填可移植相对链接 `![](assets/…)`；
// ② 选区非空时粘贴单个 http(s) URL → 包成 `[选区](url)`（编辑器通用体验）。
// 纯逻辑判定（isSingleUrl/pickFileItems 等）在共享编辑器的 `paste-text.ts`，这里只做问 Rust 的那几步。

import { EditorView } from "@codemirror/view";
import type { Extension } from "@codemirror/state";
import { commands, type HtmlPasteDto } from "../../ipc/bindings";
import { cancelPaste, freezePaste, insertFrozenPaste, pasteRangeField } from "@lunarca/editor-core/paste-range";
import { isInsideFencedCode } from "@lunarca/editor-core/code-fence-editing";
import {
  assetMarkdown,
  insertAtCursor,
  isImageAssetPath,
  isImageUrl,
  isSingleUrl,
  pickFileItems,
} from "@lunarca/editor-core/paste-text";
import type { TablePastePreview } from "@lunarca/editor-core/table-edit";
import type { EditorFileDrop } from "../../lib/editor-file-drop";

export interface PasteAssetsMessages {
  saving: string;
  saveFirst: string;
  failed: (error: string) => string;
  downloading: string;
  converting: string;
  cancelled: string;
  conversionFailed: string;
  readFailed: string;
}

export interface PasteAssetsContext {
  /** 当前笔记路径（null = 未落盘草稿）。 */
  getNotePath: () => string | null;
  /** 确保笔记已落盘并返回其路径（把草稿保存出路径供附件归属）；无法保存返回 null。 */
  ensureNotePath: () => Promise<string | null>;
  /** 粘贴远程图片链接时是否下载为本地附件（editor.localize_remote_images，PRD-06-012）。 */
  localizeRemoteImages: () => boolean;
  onStatus: (status: string) => void;
  messages: PasteAssetsMessages;
  richTextEnabled: () => boolean;
  confirmConversion: (preview: HtmlPasteDto, plain: string, tableTarget?: TablePastePreview) => Promise<PasteConversionChoice>;
}

export type PasteConversionChoice = { kind: "converted"; markdown: string } | { kind: "plain"; text: string } | null;

function readAsDataUrl(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(reader.result as string);
    reader.onerror = () => reject(reader.error ?? new Error("read failed"));
    reader.readAsDataURL(file);
  });
}

/** 多光标时粘贴一律按纯文本：结构转换、附件、链接包裹都只为一个插入点设计。 */
function hasMultipleCursors(view: EditorView): boolean {
  return view.state.selection.ranges.length > 1;
}

function wrapSelectionAsLink(view: EditorView, url: string) {
  const range = view.state.selection.main;
  const selected = view.state.sliceDoc(range.from, range.to);
  const markdown = `[${selected}](${url})`;
  view.dispatch({
    changes: { from: range.from, to: range.to, insert: markdown },
    selection: { anchor: range.from + markdown.length },
    userEvent: "input.paste",
  });
}

/**
 * 附件已落盘、插入却被取消时交给 Rust 回收（PRD-06-012 ②）：否则每取消一次就在库里留一份没人引用的
 * 文件（E5 真机 2026-10-06：粘贴大图后立刻 ⌘Z，留下一张 20 MB 的图）。Rust 此刻重判引用、按删除策略移走（可还原）。
 * 回收失败不打扰用户：那份文件仍会出现在「未被引用的附件」清单里。
 */
function discardSavedAssets(notePath: string, links: readonly string[]) {
  if (links.length > 0) void commands.discardPastedAssets(notePath, [...links]);
}

async function handleFilePaste(
  view: EditorView,
  items: DataTransferItem[],
  ctx: PasteAssetsContext,
) {
  const id = freezePaste(view);
  const notePath = ctx.getNotePath() ?? (await ctx.ensureNotePath());
  if (!notePath) {
    cancelPaste(view, id);
    ctx.onStatus(ctx.messages.saveFirst);
    return;
  }
  ctx.onStatus(ctx.messages.saving);
  const inserts: string[] = [];
  const saved: string[] = [];
  for (const item of items) {
    const file = item.getAsFile();
    if (!file) {
      continue;
    }
    try {
      const dataUrl = await readAsDataUrl(file);
      const result = await commands.savePastedAsset(notePath, dataUrl);
      if (result.status === "ok") {
        saved.push(result.data);
        inserts.push(assetMarkdown(result.data, file.type.startsWith("image/")));
      } else {
        ctx.onStatus(ctx.messages.failed(result.error));
      }
    } catch (error) {
      ctx.onStatus(ctx.messages.failed(String(error)));
    }
  }
  if (inserts.length === 0) { cancelPaste(view, id); return; }
  if (ctx.getNotePath() !== notePath || !insertFrozenPaste(view, id, inserts.join("\n"), true)) {
    cancelPaste(view, id); ctx.onStatus(ctx.messages.cancelled);
    discardSavedAssets(notePath, saved);
  }
}

/**
 * 拖进正文（对标方案 B4）：在落点处插入 Markdown，多条各占一行。落点用拖放时的视口坐标换算成
 * 文档位置，并先冻结住——等 Rust 复制 / 算链接的这段时间里用户继续打字，插入点跟着走。
 * - 系统文件：复制进笔记的附件目录（与粘贴同一落点规则），插附件链接；
 * - 文件树条目：插指向它们的标准相对链接（链接由 Rust 生成）。
 */
export async function dropAt(view: EditorView, drop: EditorFileDrop, ctx: PasteAssetsContext) {
  if (drop.paths.length === 0) return;
  const pos = view.posAtCoords({ x: drop.x, y: drop.y }) ?? view.state.selection.main.head;
  const id = freezePaste(view, { from: pos, to: pos });
  const notePath = ctx.getNotePath() ?? (await ctx.ensureNotePath());
  if (!notePath) {
    cancelPaste(view, id);
    ctx.onStatus(ctx.messages.saveFirst);
    return;
  }
  if (drop.source === "system") ctx.onStatus(ctx.messages.saving);
  const result =
    drop.source === "system"
      ? await commands.importAssetPaths(notePath, drop.paths)
      : await commands.vaultLinkMarkdown(notePath, drop.paths);
  if (result.status !== "ok" || result.data.length === 0) {
    cancelPaste(view, id);
    ctx.onStatus(result.status === "ok" ? "" : ctx.messages.failed(result.error));
    return;
  }
  ctx.onStatus("");
  const markdown =
    drop.source === "system" ? result.data.map((relPath) => assetMarkdown(relPath, isImageAssetPath(relPath))) : result.data;
  if (ctx.getNotePath() !== notePath || !insertFrozenPaste(view, id, markdown.join("\n"), true)) {
    cancelPaste(view, id);
    ctx.onStatus(ctx.messages.cancelled);
    // 只回收**这次复制进来**的文件；文件树条目是库里原有的笔记 / 附件，只是插了指向它们的链接。
    if (drop.source === "system") discardSavedAssets(notePath, result.data);
    return;
  }
  view.focus();
}

/** 远程图片链接 → 下载为本地附件并插入 `![](local)`；失败则退回插入原始链接文本（不丢内容）。 */
async function handleRemoteImagePaste(view: EditorView, url: string, ctx: PasteAssetsContext) {
  const id = freezePaste(view);
  const notePath = ctx.getNotePath() ?? (await ctx.ensureNotePath());
  if (!notePath) {
    ctx.onStatus(ctx.messages.saveFirst);
    insertFrozenPaste(view, id, url);
    return;
  }
  ctx.onStatus(ctx.messages.downloading);
  const result = await commands.downloadRemoteAsset(notePath, url);
  if (result.status !== "ok") {
    ctx.onStatus(ctx.messages.failed(result.error));
  }
  if (ctx.getNotePath() !== notePath || !insertFrozenPaste(view, id, result.status === "ok" ? assetMarkdown(result.data, true) : url)) {
    cancelPaste(view, id); ctx.onStatus(ctx.messages.cancelled);
    if (result.status === "ok") discardSavedAssets(notePath, [result.data]);
  }
}

export type PasteIntent = "automatic" | "plain" | "markdown";

/** 键盘与右键粘贴共用；显式转换也使用同一损失预览与原范围保护。 */
export async function pasteContent(view: EditorView, text: string, html: string | null, ctx: PasteAssetsContext, intent: PasteIntent = "automatic") {
  if (!text && !html) return;
  if (intent === "plain" || hasMultipleCursors(view) || (intent === "automatic" && (!ctx.richTextEnabled() || isInsideFencedCode(view.state, view.state.selection.main.head)))) { insertAtCursor(view, text); return; }
  if (!html && !text.includes("\t")) {
    if (intent === "markdown") { insertAtCursor(view, text); return; }
    if (isImageUrl(text) && ctx.localizeRemoteImages()) { await handleRemoteImagePaste(view, text.trim(), ctx); return; }
    if (isSingleUrl(text) && !view.state.selection.main.empty) { wrapSelectionAsLink(view, text.trim()); return; }
    insertAtCursor(view, text); return;
  }
  const id = freezePaste(view);
  const path = ctx.getNotePath();
  ctx.onStatus(ctx.messages.converting);
  const result = await commands.convertClipboardHtml(html ?? "", text);
  let converted: string | null;
  let block = false;
  if (result.status === "ok") {
    const choice = result.data.warnings.length > 0 ? await ctx.confirmConversion(result.data, text) : { kind: "converted" as const, markdown: result.data.markdown || text };
    converted = choice ? choice.kind === "converted" ? choice.markdown : choice.text : null;
    block = choice?.kind === "converted" && result.data.block;
  } else {
    ctx.onStatus(ctx.messages.conversionFailed); converted = text;
  }
  if (!converted || path !== ctx.getNotePath() || !insertFrozenPaste(view, id, converted, block)) {
    cancelPaste(view, id); ctx.onStatus(ctx.messages.cancelled);
  } else if (result.status === "ok") ctx.onStatus("");
}

export async function pasteFromClipboard(view: EditorView, ctx: PasteAssetsContext, intent: PasteIntent = "automatic") {
  const id = freezePaste(view);
  const path = ctx.getNotePath();
  let result;
  try { result = await commands.readClipboardContent(); }
  catch { cancelPaste(view, id); ctx.onStatus(ctx.messages.readFailed); return; }
  if (!result.text && !result.html) { cancelPaste(view, id); ctx.onStatus(ctx.messages.readFailed); return; }
  const range = view.state.field(pasteRangeField).get(id);
  cancelPaste(view, id);
  if (!range || path !== ctx.getNotePath()) { ctx.onStatus(ctx.messages.cancelled); return; }
  view.dispatch({ selection: { anchor: range.from, head: range.to } });
  await pasteContent(view, result.text ?? "", result.html, ctx, intent);
}

export function pasteAssetsExtension(ctx: PasteAssetsContext): Extension {
  let plain = false;
  return [pasteRangeField, EditorView.domEventHandlers({
    keydown: (event) => { plain = (event.metaKey || event.ctrlKey) && event.shiftKey && event.key.toLowerCase() === "v"; return false; },
    paste: (event, view) => {
      const data = event.clipboardData;
      if (!data) {
        return false;
      }
      const text = data.getData("text/plain");
      const asPlain = plain || hasMultipleCursors(view) || !ctx.richTextEnabled() || isInsideFencedCode(view.state, view.state.selection.main.head);
      plain = false;
      if (asPlain) { event.preventDefault(); insertAtCursor(view, text); return true; }
      const fileItems = pickFileItems(data.items);
      if (fileItems.length > 0) {
        event.preventDefault();
        void handleFilePaste(view, fileItems, ctx);
        return true;
      }
      const html = data.getData("text/html");
      event.preventDefault(); void pasteContent(view, text, html || null, ctx); return true;
    },
  })];
}
