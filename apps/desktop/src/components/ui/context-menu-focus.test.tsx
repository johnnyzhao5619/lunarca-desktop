// @vitest-environment happy-dom
/**
 * 菜单面板的初始焦点时序（PRD-07-059 键盘可达）。
 *
 * 真机 2026-09-02：Tab 到页头 ⋯、Enter 打开菜单后方向键不动、Esc 关不掉、焦点也回不到按钮。
 * 根因是**在落位之前聚焦**：未落位的面板 `data-placed="false"` 是 `visibility: hidden`，
 * 浏览器对不可见元素的 `focus()` 静默不做；而点击这类离散事件里，React 会在落位的同步
 * 重渲染之前先冲掉上一次提交的 passive effect，于是旧的 `useEffect` 聚焦恰好跑在看不见
 * 的那一帧。happy-dom 既不算样式、`act` 的冲刷顺序也与离散事件不同，**复现不了旧缺陷**
 * （反向验证在这里是绿的）——本用例只钉住契约本身：面板收到焦点那一刻已经落位。
 * 缺陷是否真消失以真机第 4 条为准。
 */
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render } from "@testing-library/react";
import { ContextMenuPanel } from "./ContextMenu";
import type { ContextMenuItem } from "../../lib/context-menu";

afterEach(() => {
  cleanup();
});

function items(onFirst: () => void): ContextMenuItem[] {
  return [
    { kind: "action", id: "first", label: "第一项", onSelect: onFirst },
    { kind: "separator", id: "sep" },
    { kind: "action", id: "second", label: "第二项", onSelect: () => {} },
  ];
}

describe("ContextMenuPanel 初始焦点", () => {
  it("落位之后才聚焦面板：收到焦点那一刻 data-placed 已是 true", () => {
    const placedAtFocus: string[] = [];
    const { container } = render(
      <ContextMenuPanel
        items={items(() => {})}
        placement={{ kind: "anchor", point: { x: 10, y: 10 } }}
        onDismiss={() => {}}
        onCloseSelf={() => {}}
        label="菜单"
        autoFocus
      />,
    );
    const panel = container.querySelector<HTMLElement>('[role="menu"]');
    expect(panel).not.toBeNull();
    // 面板已在 render 内同步聚焦（layout effect）：此刻活动元素就是它，且已落位。
    expect(document.activeElement).toBe(panel);
    expect(panel?.getAttribute("data-placed")).toBe("true");
    panel?.addEventListener("focus", () => {
      placedAtFocus.push(panel.getAttribute("data-placed") ?? "");
    });
    panel?.blur();
    panel?.focus();
    expect(placedAtFocus).toEqual(["true"]);
  });

  it("方向键把焦点移到第一个可用项，Esc 交给宿主关闭", () => {
    const onCloseSelf = vi.fn();
    const { container } = render(
      <ContextMenuPanel
        items={items(() => {})}
        placement={{ kind: "anchor", point: { x: 10, y: 10 } }}
        onDismiss={() => {}}
        onCloseSelf={onCloseSelf}
        label="菜单"
        autoFocus
      />,
    );
    const panel = container.querySelector<HTMLElement>('[role="menu"]')!;
    fireEvent.keyDown(panel, { key: "ArrowDown" });
    expect(document.activeElement?.textContent).toBe("第一项");
    fireEvent.keyDown(document.activeElement!, { key: "Escape" });
    expect(onCloseSelf).toHaveBeenCalledTimes(1);
  });
});
