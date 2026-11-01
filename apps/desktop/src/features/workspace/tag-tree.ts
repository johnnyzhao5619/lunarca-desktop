// 嵌套标签树（纯呈现逻辑，PRD-06-033）：把 Rust 已给全的层级摘要接成父子关系。
// 父级去重笔记数、中间层与来源全部由后端负责；前端不再相加猜业务事实。

import type { TagSummaryDto } from "../../ipc/bindings";

export interface TagTreeNode {
  /** 本级名（`a/b` 的 `b`）。 */
  name: string;
  /** 完整标签路径（`a/b`）。 */
  path: string;
  /** 恰好直挂此标签的去重笔记数（中间层可为 0）。 */
  directCount: number;
  /** 含全部子孙的去重笔记数；与点开后的过滤结果同口径。 */
  noteCount: number;
  /** 这一支完全由 AI 标注产生（PRD-06-046）；事实由 Rust 下发。 */
  aiOnly: boolean;
  children: TagTreeNode[];
}

function sortNodes(nodes: TagTreeNode[]): TagTreeNode[] {
  const sorted = [...nodes].sort(
    (a, b) => b.noteCount - a.noteCount || a.path.localeCompare(b.path),
  );
  return sorted.map((node) => ({ ...node, children: sortNodes(node.children) }));
}

/** Rust 层级摘要 → 展示树；缺父节点视为契约错误，节点留在根层而不在前端补造事实。 */
export function buildTagTree(summaries: readonly TagSummaryDto[]): TagTreeNode[] {
  const roots: TagTreeNode[] = [];
  const nodes = new Map<string, TagTreeNode>();
  const ordered = [...summaries].sort(
    (a, b) => a.tag.split("/").length - b.tag.split("/").length || a.tag.localeCompare(b.tag),
  );
  for (const summary of ordered) {
    const parts = summary.tag.split("/").filter((part) => part.length > 0);
    if (parts.length === 0) continue;
    const parentPath = parts.slice(0, -1).join("/").toLowerCase();
    const parent = parentPath ? nodes.get(parentPath) : undefined;
    const node: TagTreeNode = {
      // **落在根层的节点报完整路径**：本函数也服务**子集**（按来源分组、按输入过滤），
      // 而子集里 `项目/自动` 的父节点可能不在场。只报末级名的话，它在那一组里就成了
      // 一条叫「自动」的标签——读起来像另一条标签，而它不是。
      name: (parent ? parts[parts.length - 1] : summary.tag) ?? summary.tag,
      path: summary.tag,
      directCount: summary.direct_count,
      noteCount: summary.note_count,
      aiOnly: summary.ai_only,
      children: [],
    };
    nodes.set(summary.tag.toLowerCase(), node);
    if (parent) parent.children.push(node);
    else roots.push(node);
  }
  return sortNodes(roots);
}

/** 选中某标签时需要展开的祖先路径（`a/b/c` → `a`、`a/b`）。 */
export function ancestorPaths(tag: string): string[] {
  const parts = tag.split("/").filter((part) => part.length > 0);
  const out: string[] = [];
  let path = "";
  for (const part of parts.slice(0, -1)) {
    path = path.length > 0 ? `${path}/${part}` : part;
    out.push(path);
  }
  return out;
}
