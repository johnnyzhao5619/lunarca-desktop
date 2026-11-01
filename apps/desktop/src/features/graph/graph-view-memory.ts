/**
 * 知识网络的视角记忆（K7「打开后返回图谱原位置」）。
 *
 * 从图谱点开笔记会切走路由、画布随之卸载；回来时按上次离开的样子接着看——节点位置、
 * 平移缩放、模拟余温与「画布 / 列表」——而不是重新铺开一遍再把视角抢回全局适配。
 * 只在本次运行的内存里、只记最近一张图，不落盘（同 `reading-memory` 的纪律）。
 */

import type { FitTransform, SimNode } from "./graph-sim";

export interface GraphViewMemory {
  readonly positions: ReadonlyMap<string, SimNode>;
  readonly transform: FitTransform;
  /** 离开时模拟的余温：静止就是 0，回来不再播布局动画。 */
  readonly alpha: number;
  /** 离开时首屏适配还没兑现（布局没静止就走了）：回来接着等静止再适配。 */
  readonly fitPending: boolean;
  readonly view: "canvas" | "list";
}

let memory: GraphViewMemory | null = null;

export function rememberGraphView(next: GraphViewMemory): void {
  memory = next;
}

export function recallGraphView(): GraphViewMemory | null {
  return memory;
}

/**
 * 这份记忆对眼前这张图还算不算数：当前节点至少一半记得位置才是「同一张图回来了」。
 * 换了库或库大变时位置和视角都对不上，照首次进入处理（重新铺开并适配）——前端没有
 * 「换库」广播，而按内容判断本就比按事件判断更不会漏。
 */
export function memoryFits(recalled: GraphViewMemory | null, nodeIds: readonly string[]): boolean {
  if (recalled === null || nodeIds.length === 0) return false;
  const known = nodeIds.filter((id) => recalled.positions.has(id)).length;
  return known * 2 >= nodeIds.length;
}
