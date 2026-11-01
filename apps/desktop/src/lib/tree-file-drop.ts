/**
 * 从系统拖进文件树（PRD-07-008，笔记库对标轮 W5a）：与 `editor-file-drop` 同一套做法——主窗口
 * 只有一个系统拖放监听（`use-media-drop`），落点在文件树上时把文件交给**文件树**复制进那个
 * 文件夹；落点在别处照原逻辑处理。
 *
 * 落点沿用树内拖拽的 `data-drop-id`（文件夹行 = 它自己，笔记行 = 它所在的文件夹，树的空白处
 * = 库根 `""`），收不收、叫什么名字全由 Rust 判（`import_files_into_folder`）。
 */

import { DROP_ID_ATTR } from "./pointer-drag";

export const TREE_FILE_DROP_EVENT = "luna:tree-file-drop";

const TREE_SELECTOR = ".luna-note-tree";
const FILE_DROP_ATTR = "data-file-drop";

export interface TreeFileDrop {
  /** 系统给的绝对路径。 */
  paths: string[];
  /** 目标文件夹（库相对路径，`""` = 库根）。 */
  folder: string;
}

interface TreeDropHit {
  tree: HTMLElement;
  folder: string;
}

/** 视口坐标下的文件树落点；不在文件树上为 `null`。 */
export function treeDropAt(x: number, y: number): TreeDropHit | null {
  const element = document.elementFromPoint(x, y);
  const tree = element?.closest<HTMLElement>(TREE_SELECTOR);
  if (!element || !tree) return null;
  const zone = element.closest<HTMLElement>(`[${DROP_ID_ATTR}]`);
  return { tree, folder: zone?.getAttribute(DROP_ID_ATTR) ?? "" };
}

/** 标出正在接收拖放的那个文件夹（库根 = 整棵树）；`null` = 都不是。 */
export function markTreeDropTarget(hit: TreeDropHit | null): void {
  const target = hit
    ? hit.folder === ""
      ? hit.tree
      : hit.tree.querySelector<HTMLElement>(`[data-dir="true"][${DROP_ID_ATTR}="${CSS.escape(hit.folder)}"]`)
    : null;
  for (const element of document.querySelectorAll<HTMLElement>(`[${FILE_DROP_ATTR}]`)) {
    if (element !== target && element.closest(TREE_SELECTOR)) element.removeAttribute(FILE_DROP_ATTR);
  }
  target?.setAttribute(FILE_DROP_ATTR, "true");
}

/** 把一次拖放交给落点下的文件树；不在文件树上返回 false（调用方照原逻辑处理）。 */
export function dropOnTree(paths: readonly string[], x: number, y: number): boolean {
  const hit = treeDropAt(x, y);
  if (!hit) return false;
  const detail: TreeFileDrop = { paths: [...paths], folder: hit.folder };
  hit.tree.dispatchEvent(new CustomEvent(TREE_FILE_DROP_EVENT, { detail }));
  return true;
}
