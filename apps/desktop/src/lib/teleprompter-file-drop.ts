/**
 * 从系统拖文件进提词器首页（ADR-176 ⑨，PRD-20-022）：与 `project-file-drop` 同一套做法——主窗口
 * 只有一个系统拖放监听（`use-media-drop`），落点在带 `data-teleprompter-drop` 的区域上时，把文件
 * 交给那个区域转成提词稿；落点在别处照原逻辑处理。浮窗是另一扇窗，自己订阅自己的拖放。
 *
 * 收哪些文件、怎么转、建在哪全由 Rust 判（`teleprompter_import_files`）。
 */

export const TELEPROMPTER_FILE_DROP_EVENT = "luna:teleprompter-file-drop";

/** 接收拖放的区域。 */
export const TELEPROMPTER_DROP_ATTR = "data-teleprompter-drop";

const FILE_DROP_ATTR = "data-file-drop";

export interface TeleprompterFileDrop {
  /** 系统给的绝对路径。 */
  paths: string[];
}

/** 视口坐标下的提词器落点；不在提词器区域上为 `null`。 */
export function teleprompterDropAt(x: number, y: number): HTMLElement | null {
  return document.elementFromPoint(x, y)?.closest<HTMLElement>(`[${TELEPROMPTER_DROP_ATTR}]`) ?? null;
}

/** 标出正在接收拖放的提词器区域；`null` = 都不是。 */
export function markTeleprompterDropTarget(target: HTMLElement | null): void {
  for (const element of document.querySelectorAll<HTMLElement>(`[${TELEPROMPTER_DROP_ATTR}][${FILE_DROP_ATTR}]`)) {
    if (element !== target) element.removeAttribute(FILE_DROP_ATTR);
  }
  target?.setAttribute(FILE_DROP_ATTR, "true");
}

/** 把一次拖放交给落点下的提词器区域；不在提词器区域上返回 false（调用方照原逻辑处理）。 */
export function dropOnTeleprompter(paths: readonly string[], x: number, y: number): boolean {
  const target = teleprompterDropAt(x, y);
  if (!target) return false;
  const detail: TeleprompterFileDrop = { paths: [...paths] };
  target.dispatchEvent(new CustomEvent(TELEPROMPTER_FILE_DROP_EVENT, { detail }));
  return true;
}
