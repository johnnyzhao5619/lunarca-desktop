import { describe, expect, it, vi } from "vitest";
import {
  buildMenu,
  editableKindOf,
  focusableIndexes,
  isKeyboardInvocation,
  MENU_VIEWPORT_MARGIN,
  menuHasLeadingSlot,
  placeMenu,
  placeSubmenu,
  separator,
  textFieldMenuItems,
  type ContextMenuAction,
  type ContextMenuItem,
} from "./context-menu";

/** 按 id 取可执行项（分隔线没有禁用态，断言前先收窄类型）。 */
function actionById(items: ContextMenuItem[], id: string): ContextMenuAction {
  const found = items.find((item) => item.id === id);
  if (found?.kind !== "action") {
    throw new Error(`菜单里没有可执行项 ${id}`);
  }
  return found;
}

const action = (id: string, extra: Partial<ContextMenuItem> = {}): ContextMenuItem =>
  ({ kind: "action", id, label: id, onSelect: () => {}, ...extra }) as ContextMenuItem;

describe("buildMenu", () => {
  it("丢掉不适用项与空子菜单", () => {
    const items = buildMenu([
      action("a"),
      false,
      null,
      undefined,
      { kind: "submenu", id: "empty", label: "empty", items: [] },
      action("b"),
    ]);
    expect(items.map((item) => item.id)).toEqual(["a", "b"]);
  });

  it("收敛首尾与连续分隔线（按条件拼菜单必然产生这些空分组）", () => {
    const items = buildMenu([
      separator("lead"),
      action("a"),
      separator("s1"),
      separator("s2"),
      action("b"),
      separator("trail"),
    ]);
    expect(items.map((item) => item.id)).toEqual(["a", "s1", "b"]);
  });

  it("整份为空时返回空数组（宿主据此不弹空壳菜单）", () => {
    expect(buildMenu([false, null, separator("x")])).toEqual([]);
  });
});

describe("focusableIndexes", () => {
  it("跳过分隔线与禁用项", () => {
    const items = buildMenu([
      action("a"),
      separator("s"),
      action("b", { disabled: true }),
      action("c"),
    ]);
    // 下标是**菜单内的位置**（分隔线与禁用项照旧占位，只是不可聚焦）。
    expect(focusableIndexes(items)).toEqual([0, 3]);
  });
});

describe("placeMenu", () => {
  const viewport = { width: 1000, height: 800 };
  const size = { width: 200, height: 300 };

  it("放得下就从指针处向右下展开", () => {
    expect(placeMenu({ x: 100, y: 100 }, size, viewport)).toEqual({ x: 100, y: 100 });
  });

  it("放不下则翻到指针另一侧，而不是贴边挤（挤会盖住刚点的那一行）", () => {
    expect(placeMenu({ x: 950, y: 700 }, size, viewport)).toEqual({ x: 750, y: 400 });
  });

  it("两侧都放不下才夹取，且恒留出边距", () => {
    const huge = { width: 1200, height: 900 };
    const placed = placeMenu({ x: 500, y: 400 }, huge, viewport);
    expect(placed).toEqual({ x: MENU_VIEWPORT_MARGIN, y: MENU_VIEWPORT_MARGIN });
  });
});

describe("placeSubmenu", () => {
  const viewport = { width: 1000, height: 800 };
  const size = { width: 180, height: 200 };

  it("默认贴父项右缘、与父项顶端对齐", () => {
    const placed = placeSubmenu({ left: 100, right: 300, top: 200 }, size, viewport);
    expect(placed).toEqual({ x: 296, y: 200 });
  });

  it("右侧放不下则整块翻到左缘外侧", () => {
    const placed = placeSubmenu({ left: 700, right: 900, top: 100 }, size, viewport);
    expect(placed).toEqual({ x: 524, y: 100 });
  });

  it("纵向溢出时上推到视口内", () => {
    const placed = placeSubmenu({ left: 100, right: 300, top: 700 }, size, viewport);
    expect(placed.y).toBe(800 - MENU_VIEWPORT_MARGIN - 200);
  });
});

describe("editableKindOf", () => {
  it("文本型输入面才算可编辑", () => {
    expect(editableKindOf({ tagName: "INPUT" })).toBe("input");
    expect(editableKindOf({ tagName: "INPUT", type: "search" })).toBe("input");
    expect(editableKindOf({ tagName: "TEXTAREA" })).toBe("textarea");
    expect(editableKindOf({ tagName: "DIV", isContentEditable: true })).toBe("contenteditable");
  });

  it("按钮/复选框等非文本 input 与禁用项不算（它们该给所在表面的菜单）", () => {
    expect(editableKindOf({ tagName: "INPUT", type: "checkbox" })).toBeNull();
    expect(editableKindOf({ tagName: "INPUT", type: "range" })).toBeNull();
    expect(editableKindOf({ tagName: "INPUT", disabled: true })).toBeNull();
    expect(editableKindOf({ tagName: "TEXTAREA", disabled: true })).toBeNull();
    expect(editableKindOf({ tagName: "DIV" })).toBeNull();
    expect(editableKindOf(null)).toBeNull();
  });
});

describe("textFieldMenuItems", () => {
  const labels = { cut: "cut", copy: "copy", paste: "paste", selectAll: "selectAll" };
  const base = {
    labels,
    shortcuts: labels,
    onCut: vi.fn(),
    onCopy: vi.fn(),
    onPaste: vi.fn(),
    onSelectAll: vi.fn(),
  };

  it("无选区时剪切与复制置灰，但仍可粘贴与全选", () => {
    const items = textFieldMenuItems({ ...base, hasSelection: false, readOnly: false });
    expect(actionById(items, "cut").disabled).toBe(true);
    expect(actionById(items, "copy").disabled).toBe(true);
    expect(actionById(items, "paste").disabled).toBeUndefined();
    expect(actionById(items, "selectAll").disabled).toBeUndefined();
  });

  it("只读字段不给剪切与粘贴（给了也写不进去）", () => {
    const items = textFieldMenuItems({ ...base, hasSelection: true, readOnly: true });
    expect(items.map((item) => item.id)).toEqual(["copy", "edit", "selectAll"]);
  });
});

describe("isKeyboardInvocation", () => {
  it("菜单键/Shift+F10 给的是 (0,0) 且 detail=0，据此改锚到目标元素", () => {
    expect(isKeyboardInvocation({ clientX: 0, clientY: 0, detail: 0 })).toBe(true);
    expect(isKeyboardInvocation({ clientX: 0, clientY: 0, detail: 1 })).toBe(false);
    expect(isKeyboardInvocation({ clientX: 12, clientY: 40, detail: 0 })).toBe(false);
  });
});

describe("menuHasLeadingSlot（同层左缘对齐判据）", () => {
  const plain: ContextMenuItem = { kind: "action", id: "a", label: "A", onSelect: () => {} };

  it("纯文本层不为不存在的图标留白", () => {
    expect(menuHasLeadingSlot([plain, separator("s"), { ...plain, id: "b" }])).toBe(false);
  });

  it("勾选项（含未勾选 false）在场即需要前导槽", () => {
    // 2026-07-27 真机反馈：勾选项与普通动作混排时文字起点参差——checked: false
    // 也占勾选槽，故同样触发对齐。
    expect(menuHasLeadingSlot([plain, { ...plain, id: "c", checked: false }])).toBe(true);
  });

  it("图标项在场（含子菜单的图标）即需要前导槽", () => {
    expect(menuHasLeadingSlot([plain, { ...plain, id: "i", icon: "x" }])).toBe(true);
    expect(
      menuHasLeadingSlot([plain, { kind: "submenu", id: "m", label: "M", icon: "x", items: [plain] }]),
    ).toBe(true);
  });
});
