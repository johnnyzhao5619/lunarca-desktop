/**
 * 文件树同层的显示顺序（PRD-06-006 排序；PRD-21-011「与桌面相同的排序」）：桌面文件树与网页版共用这一份。
 * 目录置顶 + 名称自然序；时间档下文件按该时间降序（目录仍按名称）。
 */

export type TreeSortMode = "name" | "mtime" | "created";

/** 排序只看这几样：节点是不是目录、名字，以及（时间档下）修改 / 创建时间。 */
export interface SortableTreeNode {
  is_dir: boolean;
  name: string;
  mtime_ms?: number | null;
  created_ms?: number | null;
}

export function sortTreeChildren<T extends SortableTreeNode>(children: readonly T[], mode: TreeSortMode): T[] {
  return [...children].sort((a, b) => {
    if (a.is_dir !== b.is_dir) {
      return a.is_dir ? -1 : 1;
    }
    if (mode !== "name" && !a.is_dir) {
      const delta =
        mode === "mtime" ? (b.mtime_ms ?? 0) - (a.mtime_ms ?? 0) : (b.created_ms ?? 0) - (a.created_ms ?? 0);
      if (delta !== 0) {
        return delta;
      }
    }
    return a.name.localeCompare(b.name, undefined, {
      numeric: true,
      sensitivity: "base",
    });
  });
}
