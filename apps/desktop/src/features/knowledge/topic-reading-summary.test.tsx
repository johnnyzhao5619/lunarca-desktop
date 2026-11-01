// @vitest-environment happy-dom
/**
 * 主题阅读汇总的**渲染行为**（ADR-134 / TDD-07 §4.1e）：判定全在 Rust，这里只证明
 * 「有内容才显示」与「旧格式不冒充零条声明」，以及出处/标签点得动。
 */

import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { TopicReadingSummary } from "./TopicReadingSummary";

afterEach(cleanup);
beforeEach(resetBindingsDouble);

const TOPIC = "Knowledge/项目.md";

function summary(extra: Record<string, unknown> = {}) {
  return {
    status: "ok",
    data: {
      scope: { title: "项目 · A", description: "", includes: ["项目/A"], excludes: [] },
      claims: 3,
      conflicts: 0,
      evidence: { current: 3, source_changed: 0, missing: 0, ambiguous: 0, partial: 0 },
      attention: [],
      attention_omitted: 0,
      attention_claims: 0,
      unverified_lines: 0,
      transcript_records: 0,
      ...extra,
    },
  };
}

function mount(onOpenPath = vi.fn(), onOpenTag = vi.fn(), onFilterChange = vi.fn()) {
  return render(
    withI18n(
      createElement(TopicReadingSummary, {
        path: TOPIC,
        onOpenPath,
        onOpenTag,
        filter: null,
        onFilterChange,
      }),
    ),
  );
}

describe("TopicReadingSummary", () => {
  it("短页只给范围与声明数，不留分歧与待核对的空壳；标签可点", async () => {
    setCommandReply("topicReadingSummary", summary());
    const onOpenTag = vi.fn();
    mount(vi.fn(), onOpenTag);
    expect(await screen.findByText("项目 · A")).toBeTruthy();
    expect(screen.getByText(t("workspace:topicSummary.claims", { count: 3 }))).toBeTruthy();
    expect(screen.queryByText(t("workspace:topicSummary.attentionTitle"))).toBeNull();
    expect(screen.queryByText(t("workspace:topicSummary.conflicts", { count: 0 }))).toBeNull();
    expect(screen.queryByText(t("workspace:topicSummary.unverified", { count: 0 }))).toBeNull();
    fireEvent.click(screen.getByText("#项目/A"));
    expect(onOpenTag).toHaveBeenCalledWith("项目/A");
  });

  it("有分歧与需核对的来源时才显示；已不存在的来源不给打开", async () => {
    setCommandReply(
      "topicReadingSummary",
      summary({
        conflicts: 1,
        attention: [
          { source_path: "Notes/站会.md", source_title: "站会", state: "missing", claims: 1 },
          { source_path: "Notes/周会.md", source_title: "周会", state: "source_changed", claims: 2 },
        ],
      }),
    );
    const onOpenPath = vi.fn();
    mount(onOpenPath);
    expect(await screen.findByText(t("workspace:topicSummary.attentionTitle"))).toBeTruthy();
    expect(screen.getByText(t("workspace:topicSummary.conflicts", { count: 1 }))).toBeTruthy();
    expect(
      screen.getByText(t("workspace:topicSummary.state.source_changed", { count: 2 })),
    ).toBeTruthy();
    expect(screen.getByText("站会").tagName).not.toBe("BUTTON");
    fireEvent.click(screen.getByText("周会"));
    expect(onOpenPath).toHaveBeenCalledWith("Notes/周会.md");
  });

  it("旧格式没有汇总时什么都不渲染", async () => {
    setCommandReply("topicReadingSummary", { status: "ok", data: null });
    const { container } = mount();
    await vi.waitFor(() => expect(commandMock("topicReadingSummary")).toHaveBeenCalledWith(TOPIC));
    expect(container.querySelector(".luna-topic-summary")).toBeNull();
  });

  it("分歧与需核对的计数兼作筛选开关", async () => {
    setCommandReply(
      "topicReadingSummary",
      summary({
        conflicts: 1,
        attention_claims: 2,
        attention: [
          { source_path: "Notes/周会.md", source_title: "周会", state: "source_changed", claims: 2 },
        ],
      }),
    );
    const onFilterChange = vi.fn();
    mount(vi.fn(), vi.fn(), onFilterChange);
    fireEvent.click(await screen.findByText(t("workspace:topicSummary.needsCheck", { count: 2 })));
    expect(onFilterChange).toHaveBeenLastCalledWith("needs_check");
    fireEvent.click(screen.getByText(t("workspace:topicSummary.conflicts", { count: 1 })));
    expect(onFilterChange).toHaveBeenLastCalledWith("conflicts");
  });

  it("升级后的旧内容没有逐条出处时标未核验", async () => {
    setCommandReply("topicReadingSummary", summary({ unverified_lines: 2 }));
    mount();
    expect(await screen.findByText(t("workspace:topicSummary.unverified", { count: 2 }))).toBeTruthy();
  });

  it("有转写记载时给计数兼筛选开关", async () => {
    setCommandReply("topicReadingSummary", summary({ transcript_records: 1 }));
    const onFilterChange = vi.fn();
    mount(vi.fn(), vi.fn(), onFilterChange);
    fireEvent.click(
      await screen.findByText(t("workspace:topicSummary.transcriptRecords", { count: 1 })),
    );
    expect(onFilterChange).toHaveBeenLastCalledWith("transcript_records");
  });
});
