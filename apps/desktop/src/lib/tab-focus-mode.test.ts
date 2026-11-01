import { afterEach, describe, expect, it, vi } from "vitest";
import { setTabFocusMode, subscribeTabFocusMode, tabFocusModeEnabled, toggleTabFocusMode } from "./tab-focus-mode";

afterEach(() => setTabFocusMode(false));

describe("Tab 键移动焦点（会话级一份）", () => {
  it("默认关（Tab = 缩进）；切换来回", () => {
    expect(tabFocusModeEnabled()).toBe(false);
    toggleTabFocusMode();
    expect(tabFocusModeEnabled()).toBe(true);
    toggleTabFocusMode();
    expect(tabFocusModeEnabled()).toBe(false);
  });

  it("变了才通知订阅者（各编辑器与状态栏），同值不重复通知", () => {
    const listener = vi.fn();
    const off = subscribeTabFocusMode(listener);
    setTabFocusMode(true);
    setTabFocusMode(true);
    setTabFocusMode(false);
    off();
    setTabFocusMode(true);
    expect(listener).toHaveBeenCalledTimes(2);
  });
});
