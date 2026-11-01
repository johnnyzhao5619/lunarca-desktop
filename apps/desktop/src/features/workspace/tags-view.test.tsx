// @vitest-environment happy-dom
/**
 * 「知识库 → 标签」分区的**渲染行为**：来源分组（PRD-16-043 / PRD-06-046）与动作分流
 * （PRD-06-033）。
 *
 * **这两件事都只在渲染出来之后才看得见**。分组写成"过滤掉一半"与写成"分两个容器"
 * 在源码里都是几行 `filter`，差别全在屏幕上；动作分流更是如此——次选被折进「更多」
 * 与摆在旁边，源码差一个 `<details>`，而 `PRD-06-047` 承诺的那个入口成不成立就在这一差。
 */

import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { ContextMenuHost } from "../../components/ContextMenuHost";
import { TagTreePanel, TagNotesView } from "./TagsView";
import type { TagBrowserController } from "./use-tag-browser";

afterEach(cleanup);
beforeEach(resetBindingsDouble);

function summary(tag: string, aiOnly: boolean) {
  return { tag, direct_count: 2, note_count: 2, ai_only: aiOnly };
}

function controllerOf(overrides: Partial<TagBrowserController> = {}): TagBrowserController {
  return {
    summaries: [summary("项目", false), summary("自动主题", true)],
    aiTagsEnabled: true,
    topicActions: null,
    selectedTag: "项目",
    selectedSummary: summary("项目", false),
    notes: [],
    summaryState: "ready",
    noteState: "ready",
    truncated: false,
    selectTag: vi.fn(),
    refresh: vi.fn(),
    ...overrides,
  };
}

function mountTree(controller: TagBrowserController) {
  return render(
    withI18n(
      createElement(ContextMenuHost, null, createElement(TagTreePanel, { controller })),
    ),
  );
}

describe("标签树按来源分两组（PRD-16-043）", () => {
  it("能力开着时两组都在，各自报名字", () => {
    mountTree(controllerOf());

    expect(screen.getByText(t("workspace:tags.groupUser"))).toBeTruthy();
    expect(screen.getByText(t("workspace:tags.groupAi"))).toBeTruthy();
    expect(screen.getByText("#项目")).toBeTruthy();
    expect(screen.getByText("#自动主题")).toBeTruthy();
  });

  it("折叠 AI 组只收窄导航，不动当前选择——那是「看不见」，不是「不存在」", () => {
    const selectTag = vi.fn();
    mountTree(controllerOf({ selectedTag: "自动主题", selectTag }));

    fireEvent.click(screen.getByText(t("workspace:tags.groupAi")));

    expect(screen.queryByText("#自动主题")).toBeNull();
    expect(screen.getByText("#项目")).toBeTruthy();
    // 折叠不是过滤：它不改写父标签含全部子孙的语义，也不该顶掉用户选中的那一条。
    expect(selectTag).not.toHaveBeenCalled();
  });

  it("不另立第二个控件：旧的「只看用户标签」开关已不存在", () => {
    // PRD-06-046 的那枚开关与本分组做的是同一件事；同屏并存时用户无从判断两者是与
    // 关系还是或关系，故它的呈现形态**就是**这个折叠钮。
    mountTree(controllerOf());
    expect(screen.queryByText(t("workspace:tags.onlyUser"))).toBeNull();
  });

  it("能力关掉时没有「AI 标签」分组——但一条标签都不许因此消失", () => {
    mountTree(controllerOf({ aiTagsEnabled: false }));

    expect(screen.queryByText(t("workspace:tags.groupAi"))).toBeNull();
    expect(screen.queryByText(t("workspace:tags.groupUser"))).toBeNull();
    // 既有的 AI 标签仍是真标签（仍是图谱的边、仍能被 `tag:` 检索到）：把它们连同分组
    // 一起拿掉就成了隐藏，而 PRD-16-043 明写「这不是隐藏，也不是降级」。
    expect(screen.getByText("#自动主题")).toBeTruthy();
    expect(screen.getByText("#项目")).toBeTruthy();
  });

  it("一组为 0 时给**组内**空态，不用整个分区的空态盖掉另一组（PRD-07-024）", () => {
    mountTree(controllerOf({ summaries: [summary("自动主题", true)] }));
    expect(screen.getByText(t("workspace:tags.groupUserEmpty"))).toBeTruthy();

    cleanup();
    mountTree(controllerOf({ summaries: [summary("项目", false)] }));
    expect(screen.getByText(t("workspace:tags.groupAiEmpty"))).toBeTruthy();
    // AI 组为 0 且开关开着时指向既有的全库补齐入口（不新造第二套）。
    expect(screen.getByText(t("workspace:checkup.action.rescan"))).toBeTruthy();
  });

  it("过滤跨两组：用户问的是「库里有没有这条标签」", () => {
    mountTree(controllerOf());

    fireEvent.change(screen.getByLabelText(t("workspace:tags.filterPlaceholder")), {
      target: { value: "自动" },
    });

    expect(screen.getByText("#自动主题")).toBeTruthy();
    expect(screen.queryByText(t("workspace:tags.groupAi"))).toBeNull();
  });
});

function mountNotes(controller: TagBrowserController) {
  setCommandReply("noteTypeArchiveDir", { status: "ok", data: null });
  setCommandReply("graphTagExclusions", { status: "ok", data: [] });
  return render(
    withI18n(
      createElement(
        ContextMenuHost,
        null,
        createElement(TagNotesView, {
          controller,
          selectedPath: null,
          onSelect: vi.fn(),
          onSearchTag: vi.fn(),
          onOpenTopic: vi.fn(),
        }),
      ),
    ),
  );
}

describe("动作分流（PRD-06-033）：按资格渲染，次选恒为同屏可见的第二枚按钮", () => {
  const eligible = {
    topic_page: null,
    can_create_topic: true,
    notes: ["a.md", "b.md", "c.md"],
    dismissed: false,
    suggestion_token: "tok",
  };
  const withTopicPage = {
    topic_page: "Notes/项目.md",
    can_create_topic: false,
    notes: [],
    dismissed: false,
    suggestion_token: "",
  };

  it("符合主题页判据 → 首选「合成主题页」，次选「汇总成一篇笔记」", () => {
    mountNotes(controllerOf({ topicActions: eligible }));

    expect(screen.getByText(t("workspace:tags.createTopic"))).toBeTruthy();
    // 次选**同屏可见**：不折进「更多」、也不只留右键。
    expect(screen.getByText(t("workspace:tags.mergeIntoNote"))).toBeTruthy();
  });

  it("两枚动作名下各有一行产物去向（活页 / 快照）", () => {
    mountNotes(controllerOf({ topicActions: eligible }));

    expect(screen.getByText(t("workspace:tags.livingPageHint"))).toBeTruthy();
    expect(screen.getByText(t("workspace:tags.mergeIntoNoteHint"))).toBeTruthy();
  });

  it("被 ✕ 过的标签：首选降为普通可用态（去掉建议色面）但不消失", () => {
    // 主次只由 AI 色面表达（「建议」徽标已收敛为工作台横幅独有，真机第九轮）。
    mountNotes(controllerOf({ topicActions: { ...eligible, dismissed: false } }));
    expect(
      screen.getByText(t("workspace:tags.createTopic")).closest("button")?.className,
    ).toContain("luna-tag-action--suggested");

    cleanup();
    mountNotes(controllerOf({ topicActions: { ...eligible, dismissed: true } }));
    // 他关掉的是一条建议，不是一个能力（PRD-16-016）。
    expect(screen.getByText(t("workspace:tags.createTopic"))).toBeTruthy();
    expect(
      screen.getByText(t("workspace:tags.createTopic")).closest("button")?.className,
    ).not.toContain("luna-tag-action--suggested");
  });

  it("已有主题页 → 产物行（页名+摘要+维护状态）替代第三张动作卡（真机第九轮）", async () => {
    // 产物行的数据与主题页分区同一命令（`topic_directory`），不另抄标题/摘要。
    setCommandReply("topicDirectory", {
      status: "ok",
      data: {
        rows: [
          {
            path: "Notes/项目.md",
            title: "项目",
            summary: "关于项目我们已知的一句话",
            ownership: "ai_owned",
            candidate_source_count: { kind: "lower_bound", count: 0 },
            ready_item_count: { kind: "not_computed" },
          },
        ],
        empty: null,
      },
    });
    mountNotes(
      controllerOf({
        topicActions: {
          topic_page: "Notes/项目.md",
          can_create_topic: false,
          notes: [],
          dismissed: false,
          suggestion_token: "",
        },
      }),
    );

    // 活页是目的地不是动作：呈现为一行内容（页名 + 摘要 + 维护状态），不再是动作卡。
    expect(await screen.findByText("关于项目我们已知的一句话")).toBeTruthy();
    expect(screen.getByText(t("workspace:topic.ai"))).toBeTruthy();
    expect(screen.getByText(t("workspace:tags.mergeIntoNote"))).toBeTruthy();
    expect(screen.queryByText(t("workspace:tags.createTopic"))).toBeNull();
  });

  it("主题页被人工改过：产物行说「已被修改」", async () => {
    setCommandReply("topicDirectory", {
      status: "ok",
      data: {
        rows: [
          {
            path: "Notes/项目.md",
            title: "项目",
            summary: "关于项目我们已知的一句话",
            ownership: "user_owned",
            candidate_source_count: { kind: "lower_bound", count: 0 },
            ready_item_count: { kind: "not_computed" },
          },
        ],
        empty: null,
      },
    });
    mountNotes(controllerOf({ topicActions: withTopicPage }));

    expect(await screen.findByText(t("workspace:topic.user"))).toBeTruthy();
    expect(screen.queryByText(t("workspace:topic.ai"))).toBeNull();
  });

  it("维护状态取不到时不猜：不说「Lunarca 维护」", async () => {
    setCommandReply("topicDirectory", { status: "error", error: "IO" });
    mountNotes(controllerOf({ topicActions: withTopicPage }));

    expect(await screen.findByText("项目")).toBeTruthy();
    expect(screen.queryByText(t("workspace:topic.ai"))).toBeNull();
  });

  it("都不符合 → 只有「汇总成一篇笔记」（PRD-06-047 的入口逐字保留）", () => {
    mountNotes(
      controllerOf({
        topicActions: {
          topic_page: null,
          can_create_topic: false,
          notes: [],
          dismissed: false,
          suggestion_token: "",
        },
      }),
    );

    expect(screen.getByText(t("workspace:tags.mergeIntoNote"))).toBeTruthy();
    expect(screen.queryByText(t("workspace:tags.createTopic"))).toBeNull();
    expect(screen.queryByText(t("workspace:tags.openTopic"))).toBeNull();
  });
});
