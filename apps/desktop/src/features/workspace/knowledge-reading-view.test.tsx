// @vitest-environment happy-dom
/**
 * 知识阅读视图的长页大纲（TDD-07 §4.1e）：短页不给入口；长页点标题滚到正文里同序的那一个。
 */

import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { ContextMenuHost } from "../../components/ContextMenuHost";
import { KnowledgeReadingView } from "./KnowledgeReadingView";

afterEach(cleanup);
beforeEach(resetBindingsDouble);

function mount(
  content: string,
  summary: unknown = null,
  { onBack = vi.fn(), read }: { onBack?: () => void; read?: unknown } = {},
) {
  setCommandReply("readNote", read ?? { status: "ok", data: { content } });
  setCommandReply("topicState", {
    status: "ok",
    data: { ownership: "not_topic", knowledge_map: false },
  });
  setCommandReply("topicCurationStatus", {
    status: "ok",
    data: {
      maintenance: "not_topic",
      pending: null,
      pending_operation_id: null,
      apply_phase: "idle",
      transcript_sources: { authorized: false, available: 0 },
    },
  });
  setCommandReply("topicReadingSummary", { status: "ok", data: summary });
  setCommandReply("recordKnowledgeVisit", { status: "ok", data: null });
  return render(
    withI18n(
      createElement(
        ContextMenuHost,
        null,
        createElement(KnowledgeReadingView, {
          path: "Knowledge/项目.md",
          onOpenPath: vi.fn(),
          onOpenTag: vi.fn(),
          onStatus: vi.fn(),
          onBack,
        }),
      ),
    ),
  );
}

describe("KnowledgeReadingView 大纲与回访", () => {
  it("短页不给大纲入口", async () => {
    const view = mount("# 项目\n\n## 进展\n\n正文。\n");
    await vi.waitFor(() => expect(view.container.querySelector("h2")).toBeTruthy());
    expect(screen.queryByText(t("workspace:panel.outline"))).toBeNull();
    // 打开即交给 Rust 记为「继续阅读」的目标。
    expect(commandMock("recordKnowledgeVisit")).toHaveBeenCalledWith("Knowledge/项目.md");
  });

  it("长页点标题滚到正文同序的标题并收起", async () => {
    const view = mount("# 项目\n\n## 进展\n\n甲\n\n## 风险\n\n乙\n\n## 分歧\n\n丙\n");
    await vi.waitFor(() => expect(view.container.querySelectorAll("h2").length).toBe(3));
    const scrolled: string[] = [];
    Element.prototype.scrollIntoView = function (this: Element) {
      scrolled.push(this.textContent ?? "");
    };
    fireEvent.click(screen.getByText(t("workspace:panel.outline")));
    const nav = view.container.querySelector(".luna-knowledge-reading__outline");
    const item = [...(nav?.querySelectorAll("button") ?? [])].find((b) => b.textContent === "风险");
    fireEvent.click(item as HTMLButtonElement);
    expect(scrolled).toEqual(["风险"]);
    expect(view.container.querySelector(".luna-knowledge-reading__outline")).toBeNull();
  });

  it("点页头的分歧计数后正文换成声明清单，显示全文后回到正文", async () => {
    setCommandReply("topicClaims", {
      status: "ok",
      data: {
        claims: [
          {
            id: "c1",
            text: "发布推迟到下月",
            conflict: true,
            transcript_record: false,
            citations: [],
            conflicts_with: null,
          },
        ],
        omitted: 0,
      },
    });
    mount("# 项目\n\n## 进展\n\n甲\n\n## 风险\n\n乙\n\n## 分歧\n\n丙\n", {
      scope: { title: "项目", description: "", includes: ["项目"], excludes: [] },
      claims: 3,
      conflicts: 1,
      evidence: { current: 3, source_changed: 0, missing: 0, ambiguous: 0, partial: 0 },
      attention: [],
      attention_omitted: 0,
      attention_claims: 0,
      unverified_lines: 0,
      transcript_records: 0,
    });
    fireEvent.click(await screen.findByText(t("workspace:topicSummary.conflicts", { count: 1 })));
    expect(await screen.findByText("发布推迟到下月")).toBeTruthy();
    expect(commandMock("topicClaims")).toHaveBeenCalledWith("Knowledge/项目.md", "conflicts");
    expect(screen.queryByText(t("workspace:panel.outline"))).toBeNull();
    fireEvent.click(screen.getByText(t("workspace:topicSummary.showAll")));
    await vi.waitFor(() => expect(document.querySelectorAll("h2").length).toBe(3));
    expect(screen.getByText(t("workspace:panel.outline"))).toBeTruthy();
  });
});

describe("KnowledgeReadingView 回首页与读取失败", () => {
  it("页头的「知识库首页」把用户带回未选中态", async () => {
    const onBack = vi.fn();
    mount("# 项目\n\n正文。\n", null, { onBack });
    fireEvent.click(await screen.findByText(t("workspace:topics.home")));
    expect(onBack).toHaveBeenCalledTimes(1);
  });

  it("读不出来时说失败并给重试，而不是永远「加载中」", async () => {
    mount("", null, { read: { status: "error", error: "NOT_FOUND" } });
    expect(await screen.findByText(t("workspace:topics.readError"))).toBeTruthy();
    expect(screen.queryByText(t("common:status.loading"))).toBeNull();
    setCommandReply("readNote", { status: "ok", data: { content: "# 项目\n\n回来了。\n" } });
    fireEvent.click(screen.getByText(t("common:action.retry")));
    expect(await screen.findByText("回来了。")).toBeTruthy();
  });
});
