/**
 * 活动栏布局的纯判定（PRD-07-003 / ADR-087 ⑥）：给定 Rust 归一化后的布局、当前所在空间与
 * 强制显示集合，算出此刻该画哪几格、哪几格是"临时显示"。
 *
 * **顺序、显隐、锚点都不在这里判**——它们由 Rust 从 `workspace.rail_layout` 归一化后下发
 * （`workspace_rail_layout`）。本模块只做两件前端才知道的事：用户此刻站在哪、录音是不是
 * 正在进行。收成纯函数是为了让「临时显示」这条规则可无头测试。
 */

import type { RailLayoutItemDto } from "../../ipc/bindings";
import { isAnchor, isHiddenByDefault, PRIMARY_SPACES, type WorkspaceSpace } from "./workspace-ia";

/**
 * 布局尚未从 Rust 回来时的兜底：表内默认顺序，除默认隐藏的空间（创作，ADR-162 ①）外全部显示。
 *
 * 它与 Rust 注册表的默认值同形（PRD-07-002 的默认顺序与默认隐藏在两处各写一份，由
 * `workspace-ia.test` 与 `luna-config` 各自的测试钉住）。
 */
export function fallbackRailLayout(): RailLayoutItemDto[] {
  return PRIMARY_SPACES.map((space) => ({
    space,
    visible: !isHiddenByDefault(space),
    locked: isAnchor(space),
  }));
}

/**
 * 活动栏此刻该画哪几格：用户让显示的 ∪ 当前所在的 ∪ 强制显示的（录音进行中的「录音」），
 * **按配置位置**——被临时显示的那一格出现在它原来的位置上，不追加到末尾（PRD-07-003）。
 */
export function visibleRail(
  items: readonly RailLayoutItemDto[],
  active: WorkspaceSpace,
  force: ReadonlySet<WorkspaceSpace>,
): RailLayoutItemDto[] {
  return items.filter((item) => item.visible || item.space === active || force.has(item.space));
}

/** 这一格是不是"用户其实收起了它、只因当前在场或强制显示才画出来"。 */
export function isTemporarilyShown(
  item: RailLayoutItemDto,
  active: WorkspaceSpace,
  force: ReadonlySet<WorkspaceSpace>,
): boolean {
  return !item.visible && (item.space === active || force.has(item.space));
}

/** 用户收起的那几格（右键「显示已隐藏的空间 ▸」的候选）。 */
export function hiddenRailItems(items: readonly RailLayoutItemDto[]): RailLayoutItemDto[] {
  return items.filter((item) => !item.visible);
}

/**
 * 拖拽落点 → 新的可见顺序（纯函数）：把 `dragged` 挪到「插到第 `insertBefore` 个之前」
 * （`insertBefore === order.length` = 末尾）。位置不变 → 原样返回同一引用，调用方据此不发请求。
 */
export function moveRailItemToIndex(
  order: readonly WorkspaceSpace[],
  dragged: WorkspaceSpace,
  insertBefore: number,
): readonly WorkspaceSpace[] {
  const from = order.indexOf(dragged);
  if (from < 0 || !Number.isInteger(insertBefore) || insertBefore < 0 || insertBefore > order.length) {
    return order;
  }
  const without = order.filter((space) => space !== dragged);
  const at = insertBefore > from ? insertBefore - 1 : insertBefore;
  const next = [...without.slice(0, at), dragged, ...without.slice(at)];
  return next.every((space, index) => space === order[index]) ? order : next;
}

/**
 * 活动栏滚动箭头一下滚到哪（纯函数）：翻大半屏；翻完只剩不到四分之一屏（多半是尾部落区的
 * 空白）就直接到头——否则箭头还挂着、再点一下只滚过一段空白（真机 2026-10-06）。
 */
export function railScrollTarget(scrollTop: number, clientHeight: number, scrollHeight: number, direction: 1 | -1): number {
  const max = Math.max(0, scrollHeight - clientHeight);
  const target = scrollTop + direction * clientHeight * 0.8;
  const snap = clientHeight * 0.25;
  if (target >= max - snap) return max;
  if (target <= snap) return 0;
  return target;
}
