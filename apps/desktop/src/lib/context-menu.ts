/**
 * 右键上下文菜单的**数据模型与纯逻辑**（PRD-07-059，[TDD-07 §4.7]）。
 *
 * 一条贯穿全批次的口径：**菜单是数据，不是 JSX**。每个表面只声明「此刻这个对象上
 * 有哪些动作」，定位、翻转、键盘导航、关闭、无障碍语义全部由唯一宿主
 * （`components/ContextMenuHost.tsx`）负责——此前文件树与编辑器各写了一遍关闭监听与
 * 视口夹取，且两处都用**常量估算菜单尺寸**（文件树 200×180、编辑器 240×400）去夹，
 * 菜单条目一多就顶出屏幕，且估值与真实尺寸永远对不上（改一条目就得改常量）。
 *
 * 本模块只放纯函数（无 React、无 IPC、无副作用），便于无头测试；
 * 尺寸由宿主**测量真实 DOM** 后传入，不再有估算常量。
 */

import type { ReactNode } from "react";

/** 一条可执行项。 */
export interface ContextMenuAction {
  kind: "action";
  /** 稳定 id（测试与去重用；同一菜单内唯一）。 */
  id: string;
  label: string;
  icon?: ReactNode;
  /** 右侧快捷键提示（已格式化，如 `⌘C`）。 */
  shortcut?: string;
  /**
   * 悬停提示：条目标签放不下、但用户需要看得到的**同一件事的全貌**（如库切换器里
   * 那条的完整路径——两个库同名时，只有路径分得清是哪一个）。不放第二种信息。
   */
  title?: string;
  /** 勾选态：有值即以 `menuitemcheckbox` 语义呈现（含未勾选时的占位槽）。 */
  checked?: boolean;
  disabled?: boolean;
  /** 破坏性动作（红色呈现）。 */
  danger?: boolean;
  onSelect: () => void;
}

/** 分组分隔线。 */
export interface ContextMenuSeparator {
  kind: "separator";
  id: string;
}

/**
 * 分组标题（如库切换器的「本机库」「某账户的库」）：只是一行说明，不可聚焦、不可执行。
 * 它下面一条可执行项都没有时整行丢掉（`buildMenu`）——不给用户看一个空组的标题。
 */
export interface ContextMenuHeading {
  kind: "heading";
  id: string;
  label: string;
}

/** 二级菜单（如「导出为 ▸」「文本格式 ▸」）。 */
export interface ContextMenuSubmenu {
  kind: "submenu";
  id: string;
  label: string;
  icon?: ReactNode;
  disabled?: boolean;
  items: ContextMenuItem[];
}

export type ContextMenuItem =
  | ContextMenuAction
  | ContextMenuSeparator
  | ContextMenuSubmenu
  | ContextMenuHeading;

/**
 * 构建期条目：允许写 `null` / `false` 表示「这一项在此上下文不适用」，
 * 于是各表面可以用 `条件 && item` 直接拼，不必先攒数组再过滤。
 */
export type ContextMenuEntry = ContextMenuItem | null | false | undefined;

/** 分隔线构造器（id 仅用于 React key 与测试断言）。 */
export function separator(id: string): ContextMenuSeparator {
  return { kind: "separator", id };
}

/** 分组标题构造器。 */
export function heading(id: string, label: string): ContextMenuHeading {
  return { kind: "heading", id, label };
}

/**
 * 归一化菜单：丢掉不适用项与空子菜单，并把首尾及连续的分隔线收敛掉；下面没有条目的分组
 * 标题（紧跟着分隔线、另一个标题或菜单末尾）一并丢掉。
 *
 * 没有这一步，「按条件拼菜单」必然在某些上下文留下**两条挨着的分隔线**、
 * 以分隔线收尾的菜单或一个空组的标题——那是把内部条件泄露给用户看。
 */
export function buildMenu(entries: readonly ContextMenuEntry[]): ContextMenuItem[] {
  const kept: ContextMenuItem[] = [];
  const dropOrphanHeading = () => {
    if (kept.length > 0 && kept[kept.length - 1].kind === "heading") {
      kept.pop();
    }
  };
  for (const entry of entries) {
    if (!entry) {
      continue;
    }
    if (entry.kind === "submenu" && entry.items.length === 0) {
      continue;
    }
    if (entry.kind === "separator" || entry.kind === "heading") {
      dropOrphanHeading();
    }
    if (entry.kind === "separator" && (kept.length === 0 || kept[kept.length - 1].kind === "separator")) {
      continue;
    }
    kept.push(entry);
  }
  dropOrphanHeading();
  while (kept.length > 0 && kept[kept.length - 1].kind === "separator") {
    kept.pop();
  }
  return kept;
}

/**
 * 该菜单层是否存在带前导槽的条目（图标或勾选态）。
 *
 * 存在即**该层所有条目**都渲染定宽前导槽——否则同层混有普通动作时左缘错位，
 * 文字起点参差（2026-07-27 真机反馈「AI 关系标签」子菜单勾选项与动作项不对齐）。
 * 判定按层独立：纯文本菜单层不为不存在的图标留白。
 */
export function menuHasLeadingSlot(items: readonly ContextMenuItem[]): boolean {
  return items.some(
    (item) =>
      (item.kind === "action" || item.kind === "submenu") &&
      (Boolean(item.icon) || (item.kind === "action" && item.checked !== undefined)),
  );
}

/** 菜单中可聚焦（可被方向键选中）的条目：分隔线与禁用项跳过。 */
export function focusableIndexes(items: readonly ContextMenuItem[]): number[] {
  const indexes: number[] = [];
  items.forEach((item, index) => {
    if ((item.kind === "action" || item.kind === "submenu") && !item.disabled) {
      indexes.push(index);
    }
  });
  return indexes;
}

export interface MenuSize {
  width: number;
  height: number;
}

export interface Viewport {
  width: number;
  height: number;
}

export interface MenuPoint {
  x: number;
  y: number;
}

/** 菜单与视口边缘的最小留白（px）。 */
export const MENU_VIEWPORT_MARGIN = 8;

/** 子菜单与父项的横向叠压（px）：略微重叠，鼠标斜向移动不会掉出菜单。 */
export const SUBMENU_OVERLAP = 4;

/**
 * 主菜单定位：默认自指针向右下展开；某一轴放不下就**翻转到指针另一侧**
 * （而不是贴着边挤——那会让菜单盖住用户刚点的那一行），两侧都放不下才夹取。
 */
export function placeMenu(anchor: MenuPoint, size: MenuSize, viewport: Viewport): MenuPoint {
  return {
    x: placeAxis(anchor.x, size.width, viewport.width),
    y: placeAxis(anchor.y, size.height, viewport.height),
  };
}

function placeAxis(anchor: number, extent: number, available: number): number {
  const margin = MENU_VIEWPORT_MARGIN;
  if (anchor + extent <= available - margin) {
    return Math.max(margin, anchor);
  }
  const flipped = anchor - extent;
  if (flipped >= margin) {
    return flipped;
  }
  return Math.max(margin, available - margin - extent);
}

/** 父项在视口中的矩形（子菜单定位输入）。 */
export interface ItemRect {
  left: number;
  right: number;
  top: number;
}

/**
 * 子菜单定位：贴父项右缘展开、与父项顶端对齐；右侧放不下则整块翻到左缘外侧
 * （macOS / VS Code 同行为），纵向溢出则上推。
 */
export function placeSubmenu(item: ItemRect, size: MenuSize, viewport: Viewport): MenuPoint {
  const margin = MENU_VIEWPORT_MARGIN;
  let x = item.right - SUBMENU_OVERLAP;
  if (x + size.width > viewport.width - margin) {
    const flipped = item.left - size.width + SUBMENU_OVERLAP;
    x = flipped >= margin ? flipped : Math.max(margin, viewport.width - margin - size.width);
  }
  let y = item.top;
  if (y + size.height > viewport.height - margin) {
    y = viewport.height - margin - size.height;
  }
  return { x: Math.max(margin, x), y: Math.max(margin, y) };
}

/**
 * 键盘唤起菜单（`Shift+F10` / 菜单键）时浏览器给的坐标是 (0,0)——直接用会把菜单
 * 甩到屏幕左上角。判据：坐标为原点且不是真实点击（`detail === 0`）。
 */
export function isKeyboardInvocation(event: {
  clientX: number;
  clientY: number;
  detail: number;
}): boolean {
  return event.detail === 0 && event.clientX === 0 && event.clientY === 0;
}

/** 可编辑目标的类别。 */
export type EditableKind = "input" | "textarea" | "contenteditable";

/**
 * 结构化探针：只取判定需要的字段，于是纯函数可无头测试
 * （真实调用点传的就是 `HTMLElement`，字段名一致）。
 */
export interface EditableProbe {
  tagName?: string;
  type?: string;
  readOnly?: boolean;
  disabled?: boolean;
  isContentEditable?: boolean;
}

/**
 * 非文本型 `input`：右键它们该给所在表面的菜单，而不是「剪切/复制/粘贴」。
 * （`type` 缺省即 `text`。）
 */
const NON_TEXT_INPUT_TYPES = new Set([
  "button",
  "checkbox",
  "color",
  "file",
  "hidden",
  "image",
  "radio",
  "range",
  "reset",
  "submit",
]);

/** 判定右键落点是否是文本输入面（据此给标准文本编辑菜单）。 */
export function editableKindOf(node: EditableProbe | null | undefined): EditableKind | null {
  if (!node) {
    return null;
  }
  const tag = node.tagName?.toLowerCase();
  if (tag === "textarea") {
    return node.disabled ? null : "textarea";
  }
  if (tag === "input") {
    const type = (node.type ?? "text").toLowerCase();
    return node.disabled || NON_TEXT_INPUT_TYPES.has(type) ? null : "input";
  }
  return node.isContentEditable ? "contenteditable" : null;
}

/** 文本编辑菜单的标签（由调用方过 i18n 注入，纯函数不碰 i18n）。 */
export interface TextFieldMenuLabels {
  cut: string;
  copy: string;
  paste: string;
  selectAll: string;
}

export interface TextFieldMenuOptions {
  labels: TextFieldMenuLabels;
  /** 快捷键提示（已格式化）。 */
  shortcuts: TextFieldMenuLabels;
  hasSelection: boolean;
  /** 只读字段（`readOnly` 输入框、只读预览区）：只给复制与全选。 */
  readOnly: boolean;
  onCut: () => void;
  onCopy: () => void;
  onPaste: () => void;
  onSelectAll: () => void;
}

/**
 * 标准文本编辑菜单——**全应用唯一一份**（PRD-07-059 ③）。
 *
 * 原生菜单被禁用后，输入框若不给回这四项，用户就只剩快捷键可用；而 webview 的
 * 原生菜单在开发构建里恰恰是「Reload / Inspect Element」，正是要根治的那个现象。
 */
export function textFieldMenuItems(options: TextFieldMenuOptions): ContextMenuItem[] {
  const { labels, shortcuts, hasSelection, readOnly } = options;
  return buildMenu([
    !readOnly && {
      kind: "action",
      id: "cut",
      label: labels.cut,
      shortcut: shortcuts.cut,
      disabled: !hasSelection,
      onSelect: options.onCut,
    },
    {
      kind: "action",
      id: "copy",
      label: labels.copy,
      shortcut: shortcuts.copy,
      disabled: !hasSelection,
      onSelect: options.onCopy,
    },
    !readOnly && {
      kind: "action",
      id: "paste",
      label: labels.paste,
      shortcut: shortcuts.paste,
      onSelect: options.onPaste,
    },
    separator("edit"),
    {
      kind: "action",
      id: "selectAll",
      label: labels.selectAll,
      shortcut: shortcuts.selectAll,
      onSelect: options.onSelectAll,
    },
  ]);
}
