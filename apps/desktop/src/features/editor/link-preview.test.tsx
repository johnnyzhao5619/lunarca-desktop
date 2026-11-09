// @vitest-environment happy-dom
/**
 * ⌘ 悬停链接预览（对标方案 C1）：按住 ⌘ 才弹、指向哪篇问 Rust、指不到如实说、Esc 收起。
 */

import { createElement, useRef } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { LinkPreviewPopover, linkRefFromDataset, useLinkPreview } from "./LinkPreview";
import { readingHeadings } from "@lunarca/editor-core/reading-view";

function Harness({ onOpenPath }: { onOpenPath: (path: string) => void }) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const controller = useLinkPreview(hostRef, "Notes/当前.md", linkRefFromDataset);
  return createElement(
    "div",
    { ref: hostRef },
    createElement("button", { className: "luna-wiki-link", "data-link-kind": "wiki", "data-link-target": "周报#结论" }, "周报"),
    createElement("button", { className: "luna-markdown__link", "data-link-kind": "md", "data-link-target": "../会议.md" }, "会议"),
    createElement(LinkPreviewPopover, { controller, onOpenPath }),
  );
}

const onOpenPath = vi.fn();
afterEach(cleanup);
beforeEach(() => {
  resetBindingsDouble();
  onOpenPath.mockReset();
  setCommandReply("vaultResolveWikilink", { status: "ok", data: { path: "Notes/周报.md", exists: true, line: 3, heading_index: 0 } });
  setCommandReply("readNote", { status: "ok", data: { path: "Notes/周报.md", content: "# 结论\n\n本周完成三件事。" } });
});
const mount = () => render(withI18n(createElement(Harness, { onOpenPath })));

describe("⌘ 悬停链接预览", () => {
  it("不按 ⌘ 只悬停不弹；按住 ⌘ 悬停才弹出目标笔记", async () => {
    mount();
    fireEvent.mouseMove(screen.getByText("周报"));
    expect(screen.queryByRole("dialog")).toBeNull();
    fireEvent.mouseMove(screen.getByText("周报"), { metaKey: true });
    expect(await screen.findByText("本周完成三件事。")).toBeTruthy();
    expect(commandMock("vaultResolveWikilink")).toHaveBeenCalledWith("Notes/当前.md", "周报#结论");
    expect(commandMock("readNote")).toHaveBeenCalledWith("Notes/周报.md");
  });

  it("先悬停再按 ⌘ 也弹；Esc 收起", async () => {
    mount();
    fireEvent.mouseMove(screen.getByText("周报"));
    fireEvent.keyDown(window, { key: "Meta" });
    expect(await screen.findByRole("dialog")).toBeTruthy();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("md 链接走同一条解析命令；指不到如实说「还没有」", async () => {
    setCommandReply("vaultLinkNoteTarget", { status: "ok", data: null });
    mount();
    fireEvent.mouseMove(screen.getByText("会议"), { ctrlKey: true });
    expect(await screen.findByText(t("editor:linkPreview.missing"))).toBeTruthy();
    expect(commandMock("vaultLinkNoteTarget")).toHaveBeenCalledWith("Notes/当前.md", "../会议.md");
  });

  it("「打开」打开那一篇并收起预览", async () => {
    mount();
    fireEvent.mouseMove(screen.getByText("周报"), { metaKey: true });
    fireEvent.click(await screen.findByText(t("editor:linkPreview.open")));
    expect(onOpenPath).toHaveBeenCalledWith("Notes/周报.md");
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("移出链接一会儿后收起，移进预览框则留着", async () => {
    vi.useFakeTimers();
    try {
      mount();
      fireEvent.mouseMove(screen.getByText("周报"), { metaKey: true });
      await act(async () => {
        await vi.runOnlyPendingTimersAsync();
      });
      const dialog = screen.getByRole("dialog");
      // 浏览器里的真实次序：React 的 onMouseEnter（由 mouseout 合成）先到，宿主原生的
      // mouseleave 后到，且它的 relatedTarget 正是预览框。
      fireEvent.mouseEnter(dialog);
      // 只派原生 mouseleave：testing-library 的 fireEvent.mouseLeave 会顺带派 mouseout，让 React
      // 再合成一次 onMouseEnter，把这条缺陷遮住。
      screen.getByText("周报").parentElement!.dispatchEvent(new MouseEvent("mouseleave", { relatedTarget: dialog }));
      await act(async () => {
        vi.advanceTimersByTime(1000);
      });
      expect(screen.queryByRole("dialog")).not.toBeNull();
      fireEvent.mouseLeave(dialog);
      await act(async () => {
        vi.advanceTimersByTime(1000);
      });
      expect(screen.queryByRole("dialog")).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("阅读面标题序号", () => {
  it("只数正文标题，「属性」块的小标题不算", () => {
    const root = document.createElement("div");
    root.innerHTML = '<section><h4>属性</h4></section><div class="luna-markdown"><h1>甲</h1><h2>乙</h2></div>';
    expect(readingHeadings(root).map((h) => h.textContent)).toEqual(["甲", "乙"]);
  });
});
