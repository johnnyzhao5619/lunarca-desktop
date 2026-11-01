/**
 * 拖进编辑器正文（对标方案 B4）的两个来源共用一条路：
 * - **系统文件**：主窗口只有一个系统拖放监听（`use-media-drop`），落点在某个编辑器正文上时，
 *   文件交给**那个编辑器**插成附件；落点在别处仍按媒体导入转写。
 * - **文件树条目**：树的指针拖拽松手在正文上（不在任何文件夹落点上）时，插入指向它们的链接。
 *
 * 落点用视口坐标找元素：整窗的放置态描边与拖影都是 `pointer-events: none`，不挡命中测试。
 */

export const EDITOR_FILE_DROP_EVENT = "luna:editor-file-drop";

export interface EditorFileDrop {
  /** `system` = 系统里的绝对路径（复制成附件）；`vault` = 库内相对路径（插链接）。 */
  source: "system" | "vault";
  paths: string[];
  /** 视口坐标（逻辑像素）。 */
  x: number;
  y: number;
}

/** 视口坐标下的编辑器正文（阅读模式的正文是隐藏的，不算）。 */
export function editorSurfaceAt(x: number, y: number): HTMLElement | null {
  const element = document.elementFromPoint(x, y);
  return element?.closest<HTMLElement>(".luna-editor__surface:not([data-hidden])") ?? null;
}

/**
 * 系统给的物理像素坐标 → 视口逻辑像素。
 *
 * `webviewHeight` 是 webview **框**的高（物理像素，`getCurrentWebview().size()`）。wry 在 macOS 上
 * 按框高翻转 y（`frame.height - draggingLocation.y`），而主窗口的 webview 框铺满整扇窗、连标题栏
 * 那一截也算在内，页面视口却从标题栏下面开始——坐标于是整体偏下一个标题栏的高度（W5a 真机：
 * 松手在「日记」上，落点认成了下一行「项目」；编辑器拖入附件同样插低一行，只是正文区大看不出）。
 * 差多少就是「框高 − 视口高」；两者一样高时校正为 0。
 */
export function logicalPoint(
  position: { x: number; y: number },
  webviewHeight?: number,
): { x: number; y: number } {
  const ratio = window.devicePixelRatio || 1;
  const inset = webviewHeight === undefined ? 0 : Math.max(0, webviewHeight / ratio - window.innerHeight);
  return { x: position.x / ratio, y: position.y / ratio - inset };
}

/** 标出正在接收拖放的那个编辑器（放置态高亮）；`null` = 都不是。 */
export function markEditorDropTarget(surface: HTMLElement | null): void {
  for (const element of document.querySelectorAll<HTMLElement>(".luna-editor__surface[data-file-drop]")) {
    if (element !== surface) delete element.dataset.fileDrop;
  }
  if (surface) surface.dataset.fileDrop = "true";
}

/** 把一次拖放交给落点下的编辑器正文；落点不在正文上返回 false（调用方照原逻辑处理）。 */
export function dropOnEditor(source: EditorFileDrop["source"], paths: readonly string[], x: number, y: number): boolean {
  const surface = editorSurfaceAt(x, y);
  if (!surface) return false;
  const detail: EditorFileDrop = { source, paths: [...paths], x, y };
  surface.dispatchEvent(new CustomEvent(EDITOR_FILE_DROP_EVENT, { detail }));
  return true;
}
