// @vitest-environment happy-dom
/**
 * 创作看板的列表与表格视图（PRD-19-003）：同一份数据，动作交回宿主。
 * 列表的组可折叠；表格的阶段下拉只给显示的列，停在收起列里的项目额外列出它的真实阶段。
 */

import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import type { ChannelDto, ProjectCardDto, ProjectStageDto } from "../../ipc/bindings";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import type { PointerDragHandle } from "../../lib/pointer-drag";
import { columnsOf } from "./creator-board";
import { BoardColumnsView, ListView, TableView, type CreatorViewProps } from "./CreatorViews";
import { CreatorCalendar } from "./CreatorCalendar";

afterEach(cleanup);
const CATALOG: ChannelDto[] = [
  { id: "youtube", name: null, format: "long_video", limits: { title_max: 100, body_max: 5000, tags_max: null, tag_max: null, clip_max_secs: null }, style: null, builtin: true, ai_notice: true },
  { id: "bilibili", name: null, format: "long_video", limits: { title_max: 80, body_max: 2000, tags_max: 10, tag_max: 20, clip_max_secs: null }, style: null, builtin: true, ai_notice: true },
  { id: "douyin", name: null, format: "short_video", limits: { title_max: null, body_max: null, tags_max: null, tag_max: null, clip_max_secs: null }, style: null, builtin: true, ai_notice: true },
];
beforeEach(() => {
  resetBindingsDouble();
  setCommandReply("creatorChannels", { status: "ok", data: CATALOG });
});

function card(path: string, stage: ProjectStageDto, column: ProjectStageDto = stage): ProjectCardDto {
  return {
    path,
    title: path,
    stage,
    column,
    folder: "Creator/P",
    modified_ms: 0,
    channels: ["youtube", "bilibili"],
    publish_date: null,
    signals: { script_seconds: null, quotes: 0, publish_done: 0, publish_total: 6, next_action: null },
  };
}

const DRAG: PointerDragHandle<ProjectCardDto> = {
  payload: null,
  position: { x: 0, y: 0 },
  dropId: null,
  begin: () => {},
};

function props(overrides: Partial<CreatorViewProps> = {}): CreatorViewProps {
  const stages: ProjectStageDto[] = ["idea", "writing"];
  return {
    columns: columnsOf(stages, [card("甲", "idea"), card("乙", "editing", "writing")]),
    stages,
    drag: DRAG,
    hoverStage: null,
    moving: null,
    onCardClick: vi.fn(),
    onCardMenu: vi.fn(),
    onColumnMenu: vi.fn(),
    onMove: vi.fn(),
    ...overrides,
  };
}

describe("列表视图", () => {
  it("按显示列分组，组头可折叠；点行交回宿主打开", () => {
    const onCardClick = vi.fn();
    render(withI18n(createElement(ListView, props({ onCardClick }))));
    const ideaGroup = screen.getByRole("button", { name: new RegExp(t("workspace:creator.stage.idea")) });
    expect(ideaGroup.getAttribute("aria-expanded")).toBe("true");
    fireEvent.click(screen.getByText("甲"));
    expect(onCardClick).toHaveBeenCalledTimes(1);
    fireEvent.click(ideaGroup);
    expect(ideaGroup.getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryByText("甲")).toBeNull();
    // 借住的那一行照样标出真实阶段。
    expect(
      screen.getByText(t("workspace:creator.atStage", { stage: t("workspace:creator.stage.editing") })),
    ).toBeTruthy();
  });
});

describe("表格视图", () => {
  it("一行一个项目；阶段下拉只给显示的列，收起列里的项目额外列出真实阶段；改选交回宿主", () => {
    const onMove = vi.fn();
    render(withI18n(createElement(TableView, props({ onMove }))));
    expect(screen.getAllByRole("row")).toHaveLength(3);
    expect(screen.getByRole("columnheader", { name: t("workspace:creator.table.date") })).toBeTruthy();
    const parked = screen.getByRole("combobox", {
      name: t("workspace:creator.table.stageOf", { title: "乙" }),
    }) as HTMLSelectElement;
    expect(parked.value).toBe("editing");
    expect([...parked.options].map((option) => option.value)).toEqual(["editing", "idea", "writing"]);
    fireEvent.change(parked, { target: { value: "idea" } });
    expect(onMove).toHaveBeenCalledWith(expect.objectContaining({ path: "乙" }), "idea");
    const plain = screen.getByRole("combobox", {
      name: t("workspace:creator.table.stageOf", { title: "甲" }),
    }) as HTMLSelectElement;
    expect([...plain.options].map((option) => option.value)).toEqual(["idea", "writing"]);
  });
});

describe("卡片信号", () => {
  it("计划日期、渠道、口播时长、引文数、发布进度与下一步都按 Rust 给的那一份显示；没有的不出现", async () => {
    const rich: ProjectCardDto = {
      ...card("丙", "writing"),
      channels: ["youtube"],
      publish_date: "2026-10-24",
      signals: { script_seconds: 135, quotes: 3, publish_done: 2, publish_total: 5, next_action: "open_teleprompter" },
    };
    render(
      withI18n(
        createElement(BoardColumnsView, props({ columns: columnsOf(["idea", "writing"], [card("甲", "idea"), rich]) })),
      ),
    );
    expect(screen.getByText(t("workspace:creator.card.script", { duration: "2:15" }))).toBeTruthy();
    expect(screen.getByText(t("workspace:creator.card.quotes", { count: 3 }))).toBeTruthy();
    expect(screen.getByText(t("workspace:creator.card.publish", { done: 2, total: 5 }))).toBeTruthy();
    expect(
      screen.getByText(
        t("workspace:creator.card.next", { action: t("workspace:creator.nextAction.open_teleprompter") }),
      ),
    ).toBeTruthy();
    // 渠道名按目录取（内置渠道用译名）。
    expect(await screen.findByText(t("workspace:creator.channel.youtube"))).toBeTruthy();
    expect(document.querySelector(".luna-creator__card-meta")?.textContent).toContain(t("workspace:creator.channel.bilibili"));
    // 丰富的那张五项；没排期、没脚本、没引文、发布还没开始的那张只有平台一项。
    const metas = document.querySelectorAll(".luna-creator__card-meta");
    expect([...metas].map((meta) => meta.children.length)).toEqual([1, 5]);
  });
});

describe("日历视图", () => {
  it("排了期的放进那一天，没排期的列在「未排期」；翻月只换格子", () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(new Date(2026, 9, 19, 12));
    const scheduled = { ...card("排了期", "writing"), publish_date: "2026-10-24" };
    const onOpen = vi.fn();
    render(
      withI18n(
        createElement(CreatorCalendar, {
          projects: [scheduled, card("没排期", "idea")],
          moving: null,
          onOpen,
          onCardMenu: vi.fn(),
          onSetDate: vi.fn(),
        }),
      ),
    );
    const day = screen.getByRole("gridcell", { name: "2026-10-24" });
    expect(day.textContent).toContain("排了期");
    const unscheduled = screen.getByRole("region", { name: t("workspace:creator.calendar.unscheduled") });
    expect(unscheduled.textContent).toContain("没排期");
    fireEvent.click(screen.getByRole("button", { name: "排了期" }));
    expect(onOpen).toHaveBeenCalledWith(scheduled);
    fireEvent.click(screen.getByRole("button", { name: t("workspace:creator.calendar.next") }));
    expect(screen.queryByRole("gridcell", { name: "2026-10-24" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: t("workspace:creator.calendar.today") }));
    expect(screen.getByRole("gridcell", { name: "2026-10-24" })).toBeTruthy();
    vi.useRealTimers();
  });
});

describe("看板回到前台", () => {
  it("每次显示看板都重读一次（项目里改的渠道、日期、发布进度只写了项目主页，不触发库变动重读）", async () => {
    const { CreatorBoard } = await import("./CreatorBoard");
    const { ContextMenuHost } = await import("../../components/ContextMenuHost");
    const { ConfirmHost } = await import("../../components/ConfirmHost");
    const { PromptHost } = await import("../../components/PromptHost");
    const reload = vi.fn();
    const state = {
      board: null,
      loading: false,
      error: null,
      reload,
      moveProject: vi.fn(),
      setStageVisible: vi.fn(),
      setView: vi.fn(),
      setPublishDate: vi.fn(),
    };
    const props = {
      state,
      onCreateProject: vi.fn(),
      onOpenColumnSettings: vi.fn(),
      onOpenProject: vi.fn(),
      projectNav: { onOpen: vi.fn(), onRenamed: vi.fn(), onDeleted: vi.fn(), onChanged: vi.fn() },
    };
    const host = () =>
      withI18n(
        createElement(
          ConfirmHost,
          null,
          createElement(PromptHost, null, createElement(ContextMenuHost, null, createElement(CreatorBoard, props))),
        ),
      );
    const view = render(host());
    expect(reload).toHaveBeenCalledTimes(1);
    view.unmount();
    render(host());
    expect(reload).toHaveBeenCalledTimes(2);
  });
});
