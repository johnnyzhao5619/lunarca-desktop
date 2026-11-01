// @vitest-environment happy-dom
/**
 * 创作侧栏（ADR-165 S6）：项目清单按阶段筛选只是这一处的显示；已归档的项目收在「已归档」一节，
 * 默认收起，点开才列出、点一个照常进项目。归档与否来自 Rust（`board.archived`）。每一行右键或
 * 「···」打开它的菜单（ADR-167 ①）：项目是项目菜单，灵感交给宿主的笔记菜单。
 */
import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import type { CreatorBoardDto, ProjectCardDto, ProjectStageDto } from "../../ipc/bindings";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { ConfirmHost } from "../../components/ConfirmHost";
import { ContextMenuHost } from "../../components/ContextMenuHost";
import { PromptHost } from "../../components/PromptHost";
import { CreatorSidebar } from "./CreatorSidebar";
import type { CreatorBoardState } from "./use-creator-board";

function card(title: string, stage: ProjectStageDto): ProjectCardDto {
  return {
    path: `Creator/${title}/${title}.md`,
    title,
    stage,
    column: stage,
    folder: `Creator/${title}`,
    modified_ms: 1,
    channels: ["youtube"],
    publish_date: null,
    signals: { duration_secs: null, quotes: 0, publish_done: 0, publish_total: 0, next_action: null },
  } as unknown as ProjectCardDto;
}

const BOARD: CreatorBoardDto = {
  view: "board",
  stages: ["idea", "writing"],
  hidden_stages: ["published"],
  projects: [card("甲", "idea"), card("乙", "writing")],
  archived: [card("旧", "published")],
};

const projectNav = { onOpen: vi.fn(), onRenamed: vi.fn(), onDeleted: vi.fn(), onChanged: vi.fn() };

function show(onOpenProject = vi.fn(), onSwipeMenu = vi.fn()) {
  const board = { board: BOARD } as unknown as CreatorBoardState;
  render(
    withI18n(
      createElement(
        ConfirmHost,
        null,
        createElement(
          PromptHost,
          null,
          createElement(
            ContextMenuHost,
            null,
            createElement(CreatorSidebar, {
              board,
              revision: 0,
              enabled: true,
              activePath: null,
              onOpen: () => {},
              onOpenProject,
              onCreateProject: () => {},
              onCreateSwipe: () => {},
              onOpenLibrary: () => {},
              projectNav,
              onSwipeMenu,
            }),
          ),
        ),
      ),
    ),
  );
  return onOpenProject;
}

beforeEach(() => {
  resetBindingsDouble();
  setCommandReply("search", { status: "ok", data: [] });
  setCommandReply("vaultDeleteMode", "trash");
});
afterEach(cleanup);

describe("创作侧栏", () => {
  it("按阶段筛选只留那个阶段的项目；没有时说一句", async () => {
    show();
    const list = within(screen.getByRole("listbox", { name: t("workspace:creator.projects") }));
    expect(list.getByText("甲")).toBeTruthy();
    expect(list.getByText("乙")).toBeTruthy();
    const filter = screen.getByRole("combobox", { name: t("workspace:creator.sidebar.filterLabel") });
    fireEvent.change(filter, { target: { value: "writing" } });
    const filtered = within(screen.getByRole("listbox", { name: t("workspace:creator.projects") }));
    expect(filtered.queryByText("甲")).toBeNull();
    expect(filtered.getByText("乙")).toBeTruthy();
    fireEvent.change(filter, { target: { value: "published" } });
    expect(await screen.findByText(t("workspace:creator.sidebar.filterEmpty"))).toBeTruthy();
  });

  it("已归档的项目不在清单里，收在默认收起的「已归档」里，点开可进项目", () => {
    const onOpenProject = show();
    expect(screen.queryByText("旧")).toBeNull();
    const toggle = screen.getByRole("button", { name: new RegExp(t("workspace:creator.sidebar.archived")) });
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(toggle);
    fireEvent.click(screen.getByText("旧"));
    expect(onOpenProject).toHaveBeenCalledWith("Creator/旧/旧.md", "旧");
  });

  it("项目行右键与「···」打开同一份项目菜单；已归档的给「取消归档」；选「打开发布」交给宿主", async () => {
    show();
    const row = screen.getByText("甲").closest("li") as HTMLElement;
    fireEvent.contextMenu(row);
    const items = (await screen.findAllByRole("menuitem")).map((item) => item.textContent);
    expect(items).toContain(t("workspace:creator.projectMenu.delete"));
    expect(items).toContain(t("workspace:creator.projectMenu.archive"));
    fireEvent.click(screen.getByRole("menuitem", { name: t("workspace:creator.projectMenu.openPublish") }));
    expect(projectNav.onOpen).toHaveBeenCalledWith("Creator/甲/甲.md", "publish");

    fireEvent.click(screen.getByRole("button", { name: new RegExp(t("workspace:creator.sidebar.archived")) }));
    fireEvent.click(screen.getByRole("button", { name: t("workspace:creator.sidebar.more", { name: "旧" }) }));
    expect(await screen.findByRole("menuitem", { name: t("workspace:creator.projectMenu.restore") })).toBeTruthy();
  });

  it("灵感行右键交给宿主的笔记菜单", async () => {
    setCommandReply("search", {
      status: "ok",
      data: [{ path: "Swipes/开头.md", title: "开头", modified_ms: 1 }],
    });
    const onSwipeMenu = vi.fn();
    show(vi.fn(), onSwipeMenu);
    const row = (await screen.findByText("开头")).closest("li") as HTMLElement;
    fireEvent.contextMenu(row);
    expect(onSwipeMenu).toHaveBeenCalledWith(expect.anything(), "Swipes/开头.md");
  });
});
