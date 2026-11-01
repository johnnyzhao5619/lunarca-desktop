// `⌘E` 三态切换的行为锁定（PRD-06-017）。
//
// 真机现场：按 `⌘E` **三个模式全都没反应**。根因是监听挂在编辑器自己的 DOM 子树上，
// 而键盘事件只沿「焦点元素 → 祖先」冒泡——从文件树点开一篇笔记，焦点还在侧栏；进了
// 阅读模式更是无处可获焦（只读 React 视图），焦点落在 `document.body`。两种情形下事件
// 都到不了监听器。这组用例锁的是修好之后的两条**行为**：按下之后落到哪个模式、以及
// 哪一个编辑器该响应。

import { describe, expect, it } from "vitest";
import { respondsToEditorShortcuts, toggledViewMode } from "./view-mode";
import { editorCommandForKey, workspaceCommandForKey } from "../command/command-registry";

/** 默认键位（真值由 Rust 注册表给；W3b 起可改键，判定读设置真值）。 */
const EDITOR_DEFAULTS = {
  "shortcuts.toggle_reading_view": "cmd_or_ctrl+e",
  "shortcuts.ai_composer": "cmd_or_ctrl+j",
};

function isToggleReadingViewShortcut(event: KeyboardEvent): boolean {
  return editorCommandForKey(event, EDITOR_DEFAULTS) === "toggle_reading_view";
}

/** 真实 KeyboardEvent 的四个修饰键恒为布尔——补全后再交给谓词，别用半个事件试探。 */
function keyEvent(init: Partial<KeyboardEvent> & { key: string }): KeyboardEvent {
  return {
    metaKey: false,
    ctrlKey: false,
    shiftKey: false,
    altKey: false,
    ...init,
  } as KeyboardEvent;
}

describe("⌘E 的键判定", () => {
  it("Cmd+E / Ctrl+E 命中（跨平台同一枚键）", () => {
    expect(isToggleReadingViewShortcut(keyEvent({ key: "e", metaKey: true }))).toBe(true);
    expect(isToggleReadingViewShortcut(keyEvent({ key: "e", ctrlKey: true }))).toBe(true);
    // 大写锁定 / Shift 之外的大小写差异不该改变判定。
    expect(isToggleReadingViewShortcut(keyEvent({ key: "E", metaKey: true }))).toBe(true);
  });

  it("光秃秃的 e 与带 Shift/Alt 的组合不命中（正常输入不许被吃掉）", () => {
    expect(isToggleReadingViewShortcut(keyEvent({ key: "e" }))).toBe(false);
    expect(
      isToggleReadingViewShortcut(keyEvent({ key: "e", metaKey: true, shiftKey: true })),
    ).toBe(false);
    expect(isToggleReadingViewShortcut(keyEvent({ key: "e", metaKey: true, altKey: true }))).toBe(
      false,
    );
  });

  /** 同一枚按键不得被两条应用内快捷键同时认领（注册表是这件事的唯一核对处）。 */
  it("与其它应用内快捷键互不重叠", () => {
    const cmdE = keyEvent({ key: "e", metaKey: true });
    expect(
      workspaceCommandForKey(cmdE, {
        "shortcuts.quick_switcher": "cmd_or_ctrl+o",
        "shortcuts.toggle_sidebar": "cmd_or_ctrl+backslash",
        "shortcuts.vault_search": "cmd_or_ctrl+shift+f",
      }),
    ).toBeNull();
    for (const other of [
      keyEvent({ key: "o", metaKey: true }),
      keyEvent({ key: "\\", metaKey: true }),
      keyEvent({ key: "f", metaKey: true, shiftKey: true }),
    ]) {
      expect(isToggleReadingViewShortcut(other)).toBe(false);
    }
  });
});

describe("三态往返", () => {
  it("编辑型 → 阅读", () => {
    expect(toggledViewMode("live", "live")).toBe("reading");
    expect(toggledViewMode("source", "source")).toBe("reading");
  });

  /** 从阅读切回**最近一次的编辑型模式**：源码用户读一眼再切回来不该被改成 Live。 */
  it("阅读 → 最近编辑型模式", () => {
    expect(toggledViewMode("reading", "live")).toBe("live");
    expect(toggledViewMode("reading", "source")).toBe("source");
  });

  it("三个模式各自都能出得来（往返闭合）", () => {
    for (const start of ["live", "source"] as const) {
      const reading = toggledViewMode(start, start);
      expect(reading).toBe("reading");
      expect(toggledViewMode(reading, start)).toBe(start);
    }
  });
});

describe("作用域：只有活跃编辑器响应", () => {
  it("后台 pane 不响应（多 pane 时一次按键只切换活跃那一个）", () => {
    expect(respondsToEditorShortcuts({ active: false, path: "Notes/a.md" })).toBe(false);
    expect(respondsToEditorShortcuts({ active: true, path: "Notes/a.md" })).toBe(true);
  });

  it("空标签与文件夹页签不响应（没有正文可切）", () => {
    expect(respondsToEditorShortcuts({ active: true, path: null })).toBe(false);
    expect(respondsToEditorShortcuts({ active: true, path: "Notes/子目录" })).toBe(false);
  });
});
