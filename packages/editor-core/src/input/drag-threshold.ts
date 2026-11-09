/**
 * 「这一下是点击还是起拖」的唯一判据（纯函数）：桌面的指针拖拽基元（树内移动、标签重排）与编辑器表格
 * 把手的拖动排序共用这一个阈值，同一种手势在两处不会一处算点击、一处算拖动。
 */

/** 起拖阈值（px）：位移小于它视作点击，绝不把单击变成误拖。 */
export const DRAG_THRESHOLD_PX = 4;

/** 位移是否已达起拖阈值（纯函数，可 headless 测试）。 */
export function shouldStartDrag(dx: number, dy: number, threshold = DRAG_THRESHOLD_PX): boolean {
  return Math.hypot(dx, dy) >= threshold;
}
