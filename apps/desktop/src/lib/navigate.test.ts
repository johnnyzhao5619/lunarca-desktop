// @vitest-environment happy-dom
/**
 * 「去某个地方」只有一扇门（`lib/navigate.ts`）。
 *
 * 这条用例钉的是那扇门里唯一的一个判断：**有人接就地办，没人接就唤起主窗**。
 *
 * 判据刻意不是"我是不是主窗"（初版如此，2026-09-09 真机翻车）：那个判断要问
 * `getCurrentWebviewWindow().label`，而它一旦取不到就只能猜一个默认值——猜"是主窗"时
 * 浮窗里的导航静默消失（便签上那颗按钮点了没反应），猜"不是"时主窗会把自己的导航绕一圈
 * Rust。而"有没有人接"是当场可观测的事实。
 */

import { afterEach, describe, expect, it, vi } from "vitest";

const openWorkspaceRoute = vi.fn();

vi.mock("../ipc/bindings", () => ({
  commands: { openWorkspaceRoute: (route: string) => openWorkspaceRoute(route) },
}));

const { goTo, openNote, navigationHandled, NAVIGATE_EVENT } = await import("./navigate");

/** 装一个「工作区壳」：收下导航并声明自己接了。 */
function withHandler(run: () => void): string[] {
  const seen: string[] = [];
  const handler = (event: Event) => {
    seen.push((event as CustomEvent<string>).detail);
    navigationHandled(event);
  };
  window.addEventListener(NAVIGATE_EVENT, handler);
  run();
  window.removeEventListener(NAVIGATE_EVENT, handler);
  return seen;
}

afterEach(() => {
  openWorkspaceRoute.mockClear();
});

describe("应用内导航", () => {
  it("有人接就地办，不绕 Rust", () => {
    expect(withHandler(() => goTo("settings://translate"))).toEqual(["settings://translate"]);
    expect(openWorkspaceRoute).not.toHaveBeenCalled();
  });

  it("没人接就唤起主窗——浮窗、便签、独立笔记窗都是这一支", () => {
    goTo("quick-notes://QuickNotes/一条.md");
    expect(openWorkspaceRoute).toHaveBeenCalledWith("quick-notes://QuickNotes/一条.md");
  });

  it("只是**旁听**的处理器不算接（它不导航，只是跟着关个抽屉）", () => {
    const listen = () => {};
    window.addEventListener(NAVIGATE_EVENT, listen);
    goTo("notifications://");
    window.removeEventListener(NAVIGATE_EVENT, listen);
    // 没有人声明接下 → 仍然要兜到主窗去，否则那次导航就凭空消失了。
    expect(openWorkspaceRoute).toHaveBeenCalledWith("notifications://");
  });

  it("`openNote` 就是拼 `notes://` 的那一句，省得各处各拼一次", () => {
    expect(withHandler(() => openNote("Notes/买菜.md"))).toEqual(["notes://Notes/买菜.md"]);
  });
});
