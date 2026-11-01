// @vitest-environment happy-dom
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { HudActionMenu, nextHudMenuItemIndex } from "./HudActionMenu";

afterEach(cleanup);

describe("HudActionMenu", () => {
  it("starts collapsed with one accessible low-frequency action trigger", () => {
    const html = renderToStaticMarkup(
      createElement(HudActionMenu, {
        label: "More actions",
        items: [{ id: "save", label: "Save", onSelect: vi.fn() }],
      }),
    );

    expect(html).toContain('aria-haspopup="menu"');
    expect(html).toContain('aria-expanded="false"');
    expect(html).toContain('aria-label="More actions"');
    expect(html).not.toContain('role="menu"');
    expect(html).not.toContain("Save");
  });

  it("wraps keyboard navigation and skips disabled actions", () => {
    const items = [
      { id: "first", label: "First", onSelect: vi.fn() },
      { id: "disabled", label: "Disabled", disabled: true, onSelect: vi.fn() },
      { id: "last", label: "Last", onSelect: vi.fn() },
    ];

    expect(nextHudMenuItemIndex(items, null, 1)).toBe(0);
    expect(nextHudMenuItemIndex(items, null, -1)).toBe(2);
    expect(nextHudMenuItemIndex(items, 0, 1)).toBe(2);
    expect(nextHudMenuItemIndex(items, 2, 1)).toBe(0);
    expect(nextHudMenuItemIndex(items, 0, -1)).toBe(2);
  });

  it("主菜单认 ⌘K / Ctrl+K 开合，焦点落到第一项；不是主菜单的不认（ADR-137 V8）", () => {
    const items = [{ id: "save", label: "Save", onSelect: vi.fn() }];
    render(createElement(HudActionMenu, { label: "More", items, shortcut: true }));
    fireEvent.keyDown(window, { key: "k", metaKey: true });
    expect(screen.getByRole("menu")).toBeTruthy();
    expect(document.activeElement?.textContent).toBe("Save");
    fireEvent.keyDown(window, { key: "K", ctrlKey: true });
    expect(screen.queryByRole("menu")).toBeNull();
    // 带 Shift 的是别的组合，不归它。
    fireEvent.keyDown(window, { key: "k", metaKey: true, shiftKey: true });
    expect(screen.queryByRole("menu")).toBeNull();
    cleanup();

    render(createElement(HudActionMenu, { label: "More", items }));
    fireEvent.keyDown(window, { key: "k", metaKey: true });
    expect(screen.queryByRole("menu")).toBeNull();
  });
});
