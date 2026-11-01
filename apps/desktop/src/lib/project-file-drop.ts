/**
 * 从系统拖进创作项目（ADR-167 ③）：与 `tree-file-drop` 同一套做法——主窗口只有一个系统拖放
 * 监听（`use-media-drop`），落点在带 `data-project-drop` 的区域（项目概览、「添加来源」的
 * 文件页）上时，把文件交给那个区域作为这个项目的来源导入；落点在别处照原逻辑处理。
 *
 * 收哪些文件、建在哪、链进哪篇全由 Rust 判（`creator_import_material`）。
 */

export const PROJECT_FILE_DROP_EVENT = "luna:project-file-drop";

/** 接收拖放的区域；值是项目主页的库相对路径。 */
export const PROJECT_DROP_ATTR = "data-project-drop";

const FILE_DROP_ATTR = "data-file-drop";

export interface ProjectFileDrop {
  /** 系统给的绝对路径。 */
  paths: string[];
  /** 项目主页。 */
  home: string;
}

/** 视口坐标下的项目落点；不在任何项目区域上为 `null`。 */
export function projectDropAt(x: number, y: number): HTMLElement | null {
  return document.elementFromPoint(x, y)?.closest<HTMLElement>(`[${PROJECT_DROP_ATTR}]`) ?? null;
}

/** 标出正在接收拖放的项目区域；`null` = 都不是。 */
export function markProjectDropTarget(target: HTMLElement | null): void {
  for (const element of document.querySelectorAll<HTMLElement>(`[${PROJECT_DROP_ATTR}][${FILE_DROP_ATTR}]`)) {
    if (element !== target) element.removeAttribute(FILE_DROP_ATTR);
  }
  target?.setAttribute(FILE_DROP_ATTR, "true");
}

/** 把一次拖放交给落点下的项目区域；不在项目区域上返回 false（调用方照原逻辑处理）。 */
export function dropOnProject(paths: readonly string[], x: number, y: number): boolean {
  const target = projectDropAt(x, y);
  if (!target) return false;
  const detail: ProjectFileDrop = { paths: [...paths], home: target.getAttribute(PROJECT_DROP_ATTR) ?? "" };
  target.dispatchEvent(new CustomEvent(PROJECT_FILE_DROP_EVENT, { detail }));
  return true;
}
