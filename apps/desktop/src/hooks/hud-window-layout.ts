export type HudHeightMeasurement = {
  viewportHeight: number;
  contentHeight: number;
  availableHeight: number;
  /** Hidden/preflight flex layout may compress this content without increasing scrollHeight. */
  minContentHeight?: number;
};

/**
 * 返回 HUD 应增长到的高度；`null` 表示保持用户当前尺寸。
 * 该规则从不缩小窗口，也不会超过当前屏幕可用高度。
 */
export function nextHudWindowHeight({
  viewportHeight,
  contentHeight,
  availableHeight,
  minContentHeight = 0,
}: HudHeightMeasurement): number | null {
  const current = Math.max(0, Math.ceil(viewportHeight));
  const content = Math.max(0, Math.ceil(contentHeight), Math.ceil(minContentHeight));
  const available = Math.max(current, Math.floor(availableHeight));
  if (content <= current + 1) return null;

  const target = Math.min(content, available);
  return target > current + 1 ? target : null;
}

/**
 * HUD 真正需要的内容高度（两种量法取大）：
 * - `scrollHeight`：捕捉嵌套子元素撑破父级的溢出；
 * - `contentBottom + paddingBottom`：最低一个后代盒子的底边（视口坐标）加根元素的下内边距。
 *
 * 单靠 `scrollHeight` 不够：内容溢出时 WebKit 的 scrollHeight 不含根元素的下内边距，
 * 按它扩容正好长到内容底边——真机 2026-09-07 两轮：译文动作条贴死窗口下缘、补一次
 * padding 后仍差几像素（滚动区里的 textarea 只占它的最小盒、盒外的边距算法也不一致）。
 * 直接量盒子底边不猜浏览器怎么算溢出。
 */
export function requiredHudContentHeight({
  scrollHeight,
  contentBottom,
  paddingBottom,
}: {
  scrollHeight: number;
  contentBottom: number;
  paddingBottom: number;
}): number {
  return Math.max(Math.ceil(scrollHeight), Math.ceil(contentBottom + paddingBottom));
}
