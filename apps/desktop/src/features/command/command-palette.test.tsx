// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { withI18n } from "../../i18n/i18n-test-fixture";
import { EDITOR_COMMAND_EVENT, type EditorCommandDetail } from "../../lib/app-events";
import { CommandPalette } from "./CommandPalette";
import type { CommandContext, CommandSpace } from "./command-registry";

afterEach(() => {
  cleanup();
});

function context(overrides: Partial<CommandContext> = {}): CommandContext {
  return {
    openQuickSwitcher: vi.fn(),
    openCommandPalette: vi.fn(),
    openVaultSearch: vi.fn(),
    toggleSidebar: vi.fn(),
    createNewNote: vi.fn(),
    captureTextQuickNote: vi.fn(),
    splitRight: vi.fn(),
    openEmptyTab: vi.fn(),
    reopenClosedTab: vi.fn(),
    goToSpace: vi.fn(),
    openOverview: vi.fn(),
    createCreatorProject: vi.fn(),
    createSwipe: vi.fn(),
    openSwipeLibrary: vi.fn(),
    openCreatorProjectTeleprompter: vi.fn(),
    openCreatorProjectPublish: vi.fn(),
    openActiveNoteInTeleprompter: vi.fn(),
    createTeleprompterDraft: vi.fn(),
    ...overrides,
  };
}

function open(
  hasActiveEditor: boolean,
  ctx = context(),
  unavailableSpaces: ReadonlySet<CommandSpace> = new Set(),
  hasCreatorProject = false,
) {
  const onClose = vi.fn();
  render(
    withI18n(
      <CommandPalette
        open
        onClose={onClose}
        context={ctx}
        bindings={{ "shortcuts.quick_switcher": "cmd_or_ctrl+o" }}
        hasActiveEditor={hasActiveEditor}
        unavailableSpaces={unavailableSpaces}
        hasCreatorProject={hasCreatorProject}
      />,
    ),
  );
  return { onClose, ctx };
}

describe("命令面板（PRD-07-041）", () => {
  it("在搜索框里按 Enter 就执行当前行——不必先 Tab 到列表", () => {
    const received: string[] = [];
    const listener = (event: Event) => {
      received.push((event as CustomEvent<EditorCommandDetail>).detail.id);
    };
    window.addEventListener(EDITOR_COMMAND_EVENT, listener);
    const { onClose } = open(true);
    const input = screen.getByRole("searchbox");
    fireEvent.change(input, { target: { value: "切换" } });
    fireEvent.keyDown(input, { key: "Enter" });
    window.removeEventListener(EDITOR_COMMAND_EVENT, listener);
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(received).toEqual(["toggle_reading_view"]);
  });

  it("点击行执行工作区命令：经 run(context) 到达 shell", () => {
    const { ctx } = open(false);
    fireEvent.click(screen.getByText("新建笔记"));
    expect(ctx.createNewNote).toHaveBeenCalledTimes(1);
  });

  it("所属应用未启用的空间不列「前往」（ADR-165 ④），启用了就列", () => {
    open(false, context(), new Set<CommandSpace>(["creator"]));
    expect(screen.queryByText("前往：创作")).toBeNull();
    expect(screen.getByText("前往：应用")).toBeTruthy();
    cleanup();
    open(false);
    expect(screen.getByText("前往：创作")).toBeTruthy();
  });

  it("「创作：」命令组随应用启用出现；项目的提词器与发布清单只在打开着项目时列", () => {
    open(false, context(), new Set<CommandSpace>(["creator"]));
    expect(screen.queryByText("创作：新建视频项目")).toBeNull();
    cleanup();
    const { ctx } = open(false);
    expect(screen.getByText("创作：新建灵感")).toBeTruthy();
    expect(screen.queryByText("创作：打开这个项目的提词器")).toBeNull();
    fireEvent.click(screen.getByText("创作：新建视频项目"));
    expect(ctx.createCreatorProject).toHaveBeenCalledTimes(1);
    cleanup();
    const withProject = open(false, context(), new Set(), true);
    fireEvent.click(screen.getByText("创作：打开这个项目的发布清单"));
    expect(withProject.ctx.openCreatorProjectPublish).toHaveBeenCalledTimes(1);
  });

  it("没有活跃编辑器时不列编辑器命令；面板不列自己", () => {
    open(false);
    expect(screen.queryByText("切换编辑 / 阅读")).toBeNull();
    expect(screen.queryByText("命令面板", { selector: "strong span" })).toBeNull();
  });
});
