/** 编辑器的三种模式：与桌面同名同序（PRD-06-017），网页默认实时预览（PRD-21-020）。 */
export const EDITOR_MODES = ["live", "source", "reading"] as const;
export type EditorMode = (typeof EDITOR_MODES)[number];
export const DEFAULT_EDITOR_MODE: EditorMode = "live";

/**
 * 写入闸里删除还放不放行（TDD-16 §9）：容量已满时放行（删掉能腾出空间）；库只读与账户冻结时「不能写，
 * 也不能删」，界面上不给删除入口。
 */
export function deleteAllowed(gate: string | null): boolean {
  return gate === null || gate === "sync_quota_exceeded";
}
