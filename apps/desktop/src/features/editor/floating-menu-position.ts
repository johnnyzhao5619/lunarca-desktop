/**
 * AI 悬浮菜单 / 选区气泡条锚定定位纯逻辑（PRD-06-020 交互重构，Notion 对标）。
 *
 * 面板锚定在选区尾部：优先出现在选区**下方**；下方空间不足翻转到**上方**；
 * 水平方向夹回编辑器可视区内。气泡条相反：优先在选区**上方**居中（不遮下一行），
 * 上方放不下再落到下方。输入输出均为相对编辑器容器的局部坐标，
 * 纯函数、无 DOM 依赖，便于无头测试锁定。
 */

/** 选区尾字符的局部矩形（相对编辑器容器）。 */
export interface SelectionAnchor {
  /** 选区尾的左缘 x。 */
  left: number;
  /** 选区尾行的顶部 y。 */
  top: number;
  /** 选区尾行的底部 y。 */
  bottom: number;
}

/** 编辑器容器尺寸。 */
export interface EditorBox {
  width: number;
  height: number;
}

/** 面板预估尺寸（宽度来自设计 Token，高度为预估上限）。 */
export interface PanelSize {
  width: number;
  height: number;
}

/** 计算结果：面板左上角局部坐标 + 摆放方向。 */
export interface FloatingMenuPlacement {
  x: number;
  y: number;
  placement: "below" | "above";
}

/** 面板与选区/容器边缘的间距（与 `--space-2` 8px 对齐；纯逻辑不读 CSS）。 */
export const FLOATING_MENU_GAP = 8;

/**
 * 计算锚定位置：下方优先、空间不足翻上方、双向夹回容器内。
 */
export function placeFloatingMenu(
  anchor: SelectionAnchor,
  editor: EditorBox,
  panel: PanelSize,
  gap: number = FLOATING_MENU_GAP,
): FloatingMenuPlacement {
  const maxX = Math.max(gap, editor.width - panel.width - gap);
  const x = Math.min(Math.max(anchor.left, gap), maxX);

  const belowY = anchor.bottom + gap;
  const fitsBelow = belowY + panel.height + gap <= editor.height;
  if (fitsBelow) {
    return { x, y: belowY, placement: "below" };
  }

  const aboveY = anchor.top - gap - panel.height;
  if (aboveY >= gap) {
    return { x, y: aboveY, placement: "above" };
  }

  // 上下都放不下（超小窗口/超高面板）：贴容器底部，保证可见可滚。
  const clampedY = Math.max(gap, editor.height - panel.height - gap);
  return { x, y: clampedY, placement: "below" };
}

/** 选区包围盒（相对编辑器容器）。 */
export interface SelectionBox {
  left: number;
  right: number;
  top: number;
  bottom: number;
}

/** 一个文档位置的坐标矩形（`EditorView.coordsAtPos` 的形状；相对编辑器容器）。 */
export interface PosRect {
  left: number;
  right: number;
  top: number;
  bottom: number;
}

/**
 * 气泡条锚定的包围盒（Notion 口径）：单行选区取首尾并集；**跨行选区只取首行**——从选区
 * 起点到正文右缘、纵向就是首行那一行。此前取整个选区的并集，多行选区的并集横跨整个正文宽、
 * 纵向从首行到末行，于是选五段文字，气泡出现在正文正中央、离用户刚放下鼠标的位置很远；
 * 而且并集的顶就是首行的顶，气泡本来就该贴着它。`contentRight` = 正文列的右缘。
 */
export function bubbleAnchorBox(start: PosRect, end: PosRect, contentRight: number): SelectionBox {
  const multiLine = end.top >= start.bottom;
  return {
    left: multiLine ? start.left : Math.min(start.left, end.left),
    right: multiLine ? Math.max(start.left, contentRight) : Math.max(start.right, end.right),
    top: Math.min(start.top, end.top),
    bottom: multiLine ? start.bottom : Math.max(start.bottom, end.bottom),
  };
}

/**
 * 气泡条定位：水平居中于选区包围盒，优先在选区**上方**（不遮下一行正文），上方放不下
 * 则落到下方；两向夹回容器内。
 */
export function placeSelectionBubble(
  box: SelectionBox,
  editor: EditorBox,
  bubble: PanelSize,
  gap: number = FLOATING_MENU_GAP,
): FloatingMenuPlacement {
  const maxX = Math.max(gap, editor.width - bubble.width - gap);
  const center = (box.left + box.right) / 2;
  const x = Math.min(Math.max(center - bubble.width / 2, gap), maxX);

  const aboveY = box.top - gap - bubble.height;
  if (aboveY >= gap) {
    return { x, y: aboveY, placement: "above" };
  }
  const belowY = box.bottom + gap;
  const maxY = Math.max(gap, editor.height - bubble.height - gap);
  return { x, y: Math.min(belowY, maxY), placement: "below" };
}

/**
 * 拖动落点夹回编辑器可视区（PRD-06-020）：用户自己摆的位置优先，但**整个面板**都得留在
 * 容器内——编辑器容器 `overflow: hidden`，越界的部分不是"露出一角"而是被直接裁掉，
 * 看起来像渲染坏了，且裁掉的往往正是落地按钮。
 *
 * 面板比容器还高时 `maxY` 退化到 `gap`（贴顶），高度由 `--floating-menu-max-block`
 * 收进内部滚动——两条一起才保证"看得见 + 点得着"。
 */
export function clampPanelPosition(
  x: number,
  y: number,
  editor: EditorBox,
  panel: PanelSize,
  gap: number = FLOATING_MENU_GAP,
): { x: number; y: number } {
  const maxX = Math.max(gap, editor.width - panel.width - gap);
  const maxY = Math.max(gap, editor.height - panel.height - gap);
  return {
    x: Math.min(Math.max(x, gap), maxX),
    y: Math.min(Math.max(y, gap), maxY),
  };
}
