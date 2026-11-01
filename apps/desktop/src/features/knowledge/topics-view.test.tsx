// @vitest-environment happy-dom
/**
 * 「知识库 → 主题页」目录的**渲染行为**（ADR-088 / TDD-07 §4.1d / PRD-16-016 / PRD-07-024）。
 *
 * **判据是渲染出来的那几行，不是源码里有没有某个词**：这一格全部的风险都长在
 * "Rust 下发的事实有没有真的显示出来"和"0 页时说的是哪一句话"上——而这两件事在源码
 * 字符串扫描下完全不可见。0 页三态尤其如此：三条分支的文案各不相同，写反一条不会让
 * 任何东西变红，用户却会对着一句答非所问的解释。
 */

import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, emitEvent, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { TopicsView } from "./TopicsView";
import { ContextMenuHost } from "../../components/ContextMenuHost";

afterEach(cleanup);
beforeEach(() => {
  resetBindingsDouble();
  // 批量升级入口的候选清单：默认没有旧格式页（各用例按需覆盖）。
  setCommandReply("topicUpgradeCandidates", { status: "ok", data: { pages: [], omitted: 0 } });
});

function row(overrides: Record<string, unknown> = {}) {
  return {
    path: "Notes/交易计划.md",
    title: "交易计划",
    summary: "围绕双均线的一轮复盘。",
    ownership: "ai_owned",
    candidate_source_count: { kind: "lower_bound", count: 0 },
    ready_item_count: { kind: "not_computed" },
    updated_ms: null,
    related: [],
    ...overrides,
  };
}

type DirectoryFixture = {
  rows: { path: string }[];
  groups?: { name: string | null; paths: string[] }[];
  recent?: string[];
  empty: unknown;
};

/** 目录要用页头「…」菜单（地图文件动作），故挂在唯一菜单宿主里渲染。 */
function view(props: Partial<Parameters<typeof TopicsView>[0]> = {}) {
  return withI18n(
    createElement(
      ContextMenuHost,
      null,
      createElement(TopicsView, { onOpen: vi.fn(), onRowMenu: vi.fn(), onOpenHome: vi.fn(), ...props }),
    ),
  );
}

function mount(directory: DirectoryFixture, props: Partial<Parameters<typeof TopicsView>[0]> = {}) {
  // 没写分组的夹具 = 这些页都没有已确认标签，Rust 会把它们放进唯一的「其他主题」组。
  const groups =
    directory.groups ??
    (directory.rows.length > 0 ? [{ name: null, paths: directory.rows.map((r) => r.path) }] : []);
  setCommandReply("topicDirectory", {
    status: "ok",
    data: { ...directory, groups, recent: directory.recent ?? [] },
  });
  // 知识地图入口自问自答（不存在即整块不渲染）；这里默认"还没有地图"。
  setCommandReply("knowledgeMapEntry", { status: "ok", data: null });
  // 队列快照是普通数组（非 Result 形状），需显式登记——替身默认的 `{status,data}` 形状不对。
  setCommandReply("processingQueueSnapshot", []);
  return render(view(props));
}

describe("分组：与知识地图同一个来源（PRD-16-050）", () => {
  it("按 Rust 下发的分组与顺序列出，一页可在多组出现，页头计数按页去重", async () => {
    const a = row({ path: "Knowledge/A.md", title: "A" });
    const c = row({ path: "Knowledge/C.md", title: "C" });
    const loose = row({ path: "Knowledge/散页.md", title: "散页" });
    mount({
      rows: [a, c, loose],
      groups: [
        { name: "项目", paths: [a.path, c.path] },
        { name: "阅读", paths: [c.path] },
        { name: null, paths: [loose.path] },
      ],
      empty: null,
    });
    const headings = (await screen.findAllByRole("heading", { level: 3 })).map((h) => h.textContent);
    expect(headings).toEqual(["项目", "阅读", t("workspace:topics.otherGroup")]);
    expect(screen.getAllByText("C")).toHaveLength(2);
    expect(screen.getByText("3")).toBeTruthy();
  });

  it("只有「其他主题」一组时不摆孤零零的组标题", async () => {
    mount({ rows: [row()], empty: null });
    await screen.findByText("交易计划");
    expect(screen.queryByRole("heading", { level: 3 })).toBeNull();
  });

  it("过滤后变空的组不出现", async () => {
    const a = row({ path: "Knowledge/A.md", title: "A", summary: "甲" });
    const b = row({ path: "Knowledge/B.md", title: "B", summary: "乙" });
    mount({
      rows: [a, b],
      groups: [
        { name: "项目", paths: [a.path] },
        { name: "阅读", paths: [b.path] },
      ],
      empty: null,
    });
    fireEvent.change(await screen.findByPlaceholderText(t("workspace:topics.filterPlaceholder")), {
      target: { value: "甲" },
    });
    expect(screen.getByRole("heading", { level: 3 }).textContent).toBe("项目");
  });
});

describe("目录行：Rust 下发的四件事都要显示出来", () => {
  it("页名 + 一句摘要 + 所有权 + 候选来源下界", async () => {
    mount({ rows: [row({ candidate_source_count: { kind: "lower_bound", count: 2 }, ownership: "user_owned" })], empty: null });

    expect(await screen.findByText("交易计划")).toBeTruthy();
    expect(screen.getByText("围绕双均线的一轮复盘。")).toBeTruthy();
    // 所有权用的就是页头徽标那两句话——同一事实不另起一套措辞。
    expect(screen.getByText(t("workspace:topic.user"))).toBeTruthy();
    expect(screen.getByText(t("workspace:topics.candidateLowerBound", { count: 2 }))).toBeTruthy();
  });

  it("候选下界为 0 时不显示徽标，也不伪称已无待处理来源", async () => {
    mount({ rows: [row()], empty: null });

    await screen.findByText("交易计划");
    expect(screen.queryByText(t("workspace:topics.candidateLowerBound", { count: 0 }))).toBeNull();
  });

  it("Rust 返回精确候选数时不再显示下界符号", async () => {
    mount({ rows: [row({ candidate_source_count: { kind: "exact", count: 2 } })], empty: null });

    expect(await screen.findByText(t("workspace:topics.candidateExact", { count: 2 }))).toBeTruthy();
    expect(screen.queryByText(t("workspace:topics.candidateLowerBound", { count: 2 }))).toBeNull();
  });

  it("摘要为空即留空，不编一句", async () => {
    mount({ rows: [row({ summary: "" })], empty: null });

    await screen.findByText("交易计划");
    // 首页行自带一句说明，它不是主题页的摘要。
    expect(
      document.querySelector(".luna-topics__row:not(.luna-topics__row--home) .luna-topics__summary"),
    ).toBeNull();
  });

  it("首行是知识库首页：未选中任何一页时它是当前项，点它回到首页", async () => {
    const onOpenHome = vi.fn();
    mount({ rows: [row()], empty: null }, { onOpenHome });

    const home = (await screen.findByText(t("workspace:topics.home"))).closest("li");
    expect(home?.getAttribute("aria-current")).toBe("true");
    fireEvent.click(screen.getByText(t("workspace:topics.home")));
    expect(onOpenHome).toHaveBeenCalledTimes(1);
  });

  it("选中一页后首页行不再是当前项", async () => {
    mount({ rows: [row()], empty: null }, { selectedPath: "Notes/交易计划.md" });

    const home = (await screen.findByText(t("workspace:topics.home"))).closest("li");
    expect(home?.getAttribute("aria-current")).toBeNull();
  });

  it("点一行打开的是那一篇真实 Markdown", async () => {
    const onOpen = vi.fn();
    mount({ rows: [row()], empty: null }, { onOpen });

    fireEvent.click(await screen.findByText("交易计划"));
    expect(onOpen).toHaveBeenCalledWith("Notes/交易计划.md");
  });

  it("行菜单在右键与可聚焦按钮上都开得出来（只留右键 = 对键盘用户不存在）", async () => {
    const onRowMenu = vi.fn();
    mount({ rows: [row()], empty: null }, { onRowMenu });

    await screen.findByText("交易计划");
    fireEvent.contextMenu(
      document.querySelector(".luna-topics__row:not(.luna-topics__row--home)") as Element,
    );
    fireEvent.click(screen.getByLabelText(t("workspace:topics.rowMenu", { title: "交易计划" })));
    expect(onRowMenu).toHaveBeenCalledTimes(2);
    // 两次拿到的是同一行的事实（同一份能力开关由壳装配成同一份菜单）。
    expect(onRowMenu.mock.calls[0][1]).toEqual(onRowMenu.mock.calls[1][1]);
  });
});

describe("0 页空态的三态（PRD-16-016，判据在 Rust，前端只渲染）", () => {
  it("① 有合格标签簇 → 建议一行 + [合成主题页]", async () => {
    mount({
      rows: [],
      empty: {
        kind: "suggestion",
        suggestion: {
          suggestion_token: "tok",
          tag: "项目 X",
          notes: ["a.md", "b.md", "c.md"],
          // 精确篇数与截断清单长度不同：文案必须报前者（PRD-16-051）。
          note_count: 7,
          draft_items: null,
        },
      },
    });

    expect(
      await screen.findByText(t("workspace:dashboard.knowledgePulse.topic", { count: 7, tag: "项目 X" })),
    ).toBeTruthy();
    expect(screen.getByText(t("workspace:dashboard.knowledgePulse.topicCreate"))).toBeTruthy();
  });

  it("① 后台已备好初稿时如实报条数，主按钮改为查看初稿（PRD-16-060）", async () => {
    mount({
      rows: [],
      empty: {
        kind: "suggestion",
        suggestion: { suggestion_token: "tok", tag: "项目 X", notes: ["a.md"], note_count: 3, draft_items: 4 },
      },
    });

    expect(
      await screen.findByText(t("workspace:dashboard.knowledgePulse.topicDraftReady", { count: 4 })),
    ).toBeTruthy();
    expect(screen.getByText(t("workspace:dashboard.knowledgePulse.topicReview"))).toBeTruthy();
    expect(screen.queryByText(t("workspace:dashboard.knowledgePulse.topicCreate"))).toBeNull();
  });

  it("① 后台初稿那一件从在跑到结束时重取一次（初稿只写 sidecar，没有 vault://changed）", async () => {
    mount({
      rows: [],
      empty: {
        kind: "suggestion",
        suggestion: { suggestion_token: "tok", tag: "项目 X", notes: ["a.md"], note_count: 3, draft_items: null },
      },
    });
    await screen.findByText(t("workspace:dashboard.knowledgePulse.topicCreate"));
    const calls = commandMock("topicDirectory").mock.calls.length;
    const task = {
      id: "1",
      kind: "topic_draft_prepare",
      label: "#项目 X",
      note_rel: null,
      status: { state: "running", data: { step: "filing", done: 0, total: 1 } },
      executed_by: null,
      queue: null,
      restored: false,
    };
    emitEvent("processingQueue", { items: [task] });
    expect(commandMock("topicDirectory").mock.calls.length).toBe(calls);
    emitEvent("processingQueue", { items: [{ ...task, status: { state: "done", data: { note_rel: null, awaiting_review: 0 } } }] });
    await vi.waitFor(() => expect(commandMock("topicDirectory").mock.calls.length).toBe(calls + 1));
  });

  it("① 的那一行**不承载 ✕**——分区空态不是一枚可以被静音的提醒", async () => {
    mount({
      rows: [],
      empty: {
        kind: "suggestion",
        suggestion: { suggestion_token: "tok", tag: "项目 X", notes: ["a.md"], note_count: 1, draft_items: null },
      },
    });

    await screen.findByText(t("workspace:dashboard.knowledgePulse.topicCreate"));
    // `✕` 消掉的只是启动概览与图谱侧栏那两枚主动冒出来的提醒；这里恒显。
    expect(screen.queryByLabelText(t("workspace:dashboard.knowledgePulse.topicIgnore"))).toBeNull();
  });

  it("① 不上报曝光——恒显的解释不进「这条提醒被看见了吗」的分母", async () => {
    const { commands } = await import("../../ipc/bindings-test-double");
    mount({
      rows: [],
      empty: {
        kind: "suggestion",
        suggestion: { suggestion_token: "tok", tag: "项目 X", notes: ["a.md"], note_count: 1, draft_items: null },
      },
    });

    await screen.findByText(t("workspace:dashboard.knowledgePulse.topicCreate"));
    expect(commands.knowledgeSuggestionShown).not.toHaveBeenCalled();
  });

  it("② 无簇但有「还没有标签的知识笔记」→ 直说这件事，并通向全库补齐", async () => {
    mount({ rows: [], empty: { kind: "untagged", count: 12 } });

    // 文案与处置动作沿用体检里那条发现自己的（PRD-16-030 的补类），不另写一份。
    expect(await screen.findByText(t("workspace:checkup.card.untagged.title"))).toBeTruthy();
    expect(screen.getByText(t("workspace:checkup.card.untagged.desc", { count: 12 }))).toBeTruthy();
    expect(screen.getByText(t("workspace:checkup.action.rescan"))).toBeTruthy();
  });

  it("③ 无话可说 → 纯教学空态，一个字都不提标签补齐", async () => {
    mount({ rows: [], empty: { kind: "teaching" } });

    expect(await screen.findByText(t("workspace:topics.emptyTitle"))).toBeTruthy();
    expect(screen.getByText(t("workspace:topics.emptyHint"))).toBeTruthy();
    // 总开关关掉的用户不该在这里被反复提起那个能力（PRD-16-030 同一纪律）。
    expect(screen.queryByText(t("workspace:checkup.action.rescan"))).toBeNull();
    expect(screen.queryByText(t("workspace:dashboard.knowledgePulse.topicCreate"))).toBeNull();
  });
});

describe("四类空态各说各的（PRD-07-024）", () => {
  it("加载失败给重试，不是一片空白（拆分后不得比拆分前倒退）", async () => {
    setCommandReply("topicDirectory", { status: "error", error: "boom" });
    setCommandReply("knowledgeMapEntry", { status: "ok", data: null });
    render(view());

    expect(await screen.findByText(t("workspace:topics.loadError"))).toBeTruthy();
    expect(screen.getByText(t("common:action.retry"))).toBeTruthy();
  });

  it("过滤无结果与首次为空是两句不同的话", async () => {
    mount({ rows: [row()], empty: null });
    await screen.findByText("交易计划");

    fireEvent.change(screen.getByLabelText(t("workspace:topics.filterPlaceholder")), {
      target: { value: "不存在的词" },
    });

    expect(screen.getByText(t("workspace:topics.noMatch"))).toBeTruthy();
    // 「这个库还没有主题页」不该在这里出现——库里明明有一页。
    expect(screen.queryByText(t("workspace:topics.emptyTitle"))).toBeNull();
  });

  it("0 页时不摆过滤框（没有可过滤的对象）", async () => {
    mount({ rows: [], empty: { kind: "teaching" } });

    await screen.findByText(t("workspace:topics.emptyTitle"));
    expect(screen.queryByLabelText(t("workspace:topics.filterPlaceholder"))).toBeNull();
  });
});

describe("跟着写盘重取", () => {
  it("`vault://changed` 之后目录自己重查，不靠调用方记得刷新（且去抖）", async () => {
    let calls = 0;
    setCommandReply("topicDirectory", () => {
      calls += 1;
      return { status: "ok", data: { rows: [], groups: [], empty: { kind: "teaching" } } };
    });
    setCommandReply("knowledgeMapEntry", { status: "ok", data: null });
    render(view());
    await vi.waitFor(() => expect(calls).toBe(1));

    // 一次批量整理会连着写几十篇笔记：去抖之后它们合成**一次**重取，而不是几十次往返。
    for (let i = 0; i < 5; i += 1) {
      emitEvent("vaultChanged", { path: `Notes/第${i}篇.md` });
    }
    expect(calls, "写盘事件到达的那一帧不该立刻往返").toBe(1);
    await vi.waitFor(() => expect(calls).toBe(2), { timeout: 2000 });
    expect(calls, "五次写盘只该合成一次重取").toBe(2);
  });
});

describe("目录即地图（PRD-16-061）：地图有的目录都有，地图文件只剩次要菜单", () => {
  it("不再另钉「知识地图」一行；页头「…」给用默认应用打开与在访达中显示", async () => {
    setCommandReply("knowledgeMapEntry", { status: "ok", data: "Notes/知识地图.md" });
    setCommandReply("topicDirectory", {
      status: "ok",
      data: { rows: [row()], groups: [{ name: null, paths: [row().path] }], recent: [], empty: null },
    });
    setCommandReply("processingQueueSnapshot", []);
    render(view());

    fireEvent.click(await screen.findByLabelText(t("workspace:topics.mapMenu")));
    fireEvent.click(await screen.findByText(t("workspace:topics.mapOpenExternal")));
    expect(commandMock("openVaultPath")).toHaveBeenCalledWith("Notes/知识地图.md");
    fireEvent.click(screen.getByLabelText(t("workspace:topics.mapMenu")));
    fireEvent.click(await screen.findByText(t("common:menu.revealInFileManager")));
    expect(commandMock("revealVaultPath")).toHaveBeenCalledWith("Notes/知识地图.md");
    // 首页行之后紧接主题页，中间没有一行地图。
    const titles = screen.getAllByRole("button").map((node) => node.textContent ?? "");
    const home = titles.findIndex((text) => text.includes(t("workspace:topics.home")));
    expect(titles[home + 1]).toContain("交易计划");
  });

  it("没有地图（或多份冲突，Rust 回 null）时不给地图菜单", async () => {
    mount({ rows: [row()], empty: null });
    await screen.findByText("交易计划");
    expect(screen.queryByLabelText(t("workspace:topics.mapMenu"))).toBeNull();
  });

  it("「最近更新」按 Rust 给的顺序列在分组之前，过滤时不出现", async () => {
    const a = row({ path: "Knowledge/A.md", title: "A", summary: "" });
    const b = row({ path: "Knowledge/B.md", title: "B", summary: "" });
    mount({ rows: [a, b], recent: ["Knowledge/B.md"], empty: null });
    const heading = await screen.findByText(t("workspace:topics.recentGroup"));
    const items = [...document.querySelectorAll(".luna-topics__title, .luna-topics__group-title")].map(
      (node) => node.textContent,
    );
    expect(items.indexOf(heading.textContent)).toBeLessThan(items.indexOf("B"));
    expect(items.filter((text) => text === "B")).toHaveLength(2);
    fireEvent.change(screen.getByPlaceholderText(t("workspace:topics.filterPlaceholder")), {
      target: { value: "B" },
    });
    expect(screen.queryByText(t("workspace:topics.recentGroup"))).toBeNull();
  });

  it("相关主题按目录页名列出，点它打开那一页；整理时间只在 Rust 给了时出现", async () => {
    const onOpen = vi.fn();
    const a = row({ path: "Knowledge/A.md", title: "项目甲", related: ["Knowledge/B.md"], updated_ms: Date.now() - 60_000 });
    const b = row({ path: "Knowledge/B.md", title: "项目乙", related: ["Knowledge/A.md"] });
    mount({ rows: [a, b], empty: null }, { onOpen });
    await screen.findAllByText("项目甲");
    expect(screen.getAllByText(t("workspace:topics.related"))).toHaveLength(2);
    fireEvent.click(screen.getByRole("button", { name: "项目乙" }));
    expect(onOpen).toHaveBeenCalledWith("Knowledge/B.md");
    expect(document.querySelectorAll(".luna-topics__updated")).toHaveLength(1);
  });
});

describe("旧格式主题的批量升级入口", () => {
  it("只有 Rust 列出旧格式页时才给入口，计数含超出一批的部分", async () => {
    mount({ rows: [row()], empty: null });
    await screen.findByText("交易计划");
    expect(screen.queryByText(t("workspace:topicUpgrade.open", { count: 0 }))).toBeNull();
    cleanup();

    setCommandReply("topicDirectory", {
      status: "ok",
      data: { rows: [row()], groups: [{ name: null, paths: [row().path] }], recent: [], empty: null },
    });
    setCommandReply("knowledgeMapEntry", { status: "ok", data: null });
    setCommandReply("processingQueueSnapshot", []);
    setCommandReply("topicUpgradeCandidates", {
      status: "ok",
      data: { pages: [{ path: "Notes/交易计划.md", title: "交易计划" }], omitted: 2 },
    });
    render(view());
    fireEvent.click(await screen.findByText(t("workspace:topicUpgrade.open", { count: 3 })));
    expect(await screen.findByRole("dialog")).toBeTruthy();
  });
});

describe("后台准备好的整理建议", () => {
  it("有待确认计划时显示精确条数，优先于候选下界", async () => {
    mount({
      rows: [
        row({
          candidate_source_count: { kind: "lower_bound", count: 3 },
          ready_item_count: { kind: "exact", count: 2 },
        }),
      ],
      empty: null,
    });
    expect(await screen.findByText(t("workspace:topics.readyItems", { count: 2 }))).toBeTruthy();
    expect(
      screen.queryByText(t("workspace:topics.candidateLowerBound", { count: 3 })),
    ).toBeNull();
  });
});
