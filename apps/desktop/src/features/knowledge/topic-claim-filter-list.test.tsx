// @vitest-environment happy-dom
/**
 * 声明筛选清单（TDD-07 §4.1e）：清单与出处状态来自 Rust；点出处按出处 ID 交回 Rust 打开，
 * 不是有效出处时退回按路径打开；空结果说清楚，不是空白。
 */

import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { TopicClaimFilterList } from "./TopicClaimFilterList";

afterEach(cleanup);
beforeEach(resetBindingsDouble);

const TOPIC = "Knowledge/项目.md";

function mount(
  onOpenPath = vi.fn(),
  onClear = vi.fn(),
  filter: "needs_check" | "conflicts" = "needs_check",
) {
  return render(
    withI18n(
      createElement(TopicClaimFilterList, {
        path: TOPIC,
        filter,
        onClear,
        onOpenPath,
      }),
    ),
  );
}

describe("TopicClaimFilterList", () => {
  it("分歧与原说法并列，各带出处和录于时间；没记时间的出处不显示时间", async () => {
    const citation = (id: string, title: string, recorded_ms: number | null) => ({
      citation_id: id,
      source_path: `Notes/${title}.md`,
      source_title: title,
      state: "current",
      quote: title,
      recorded_ms,
    });
    setCommandReply("topicClaims", {
      status: "ok",
      data: {
        claims: [
          {
            id: "c2",
            text: "原型尚未完成",
            conflict: true,
            transcript_record: false,
            citations: [citation("c2:0", "新", null)],
            conflicts_with: {
              id: "c1",
              text: "原型已完成",
              citations: [citation("c1:0", "旧", Date.UTC(2026, 8, 1, 12))],
            },
          },
        ],
        omitted: 0,
      },
    });
    mount(vi.fn(), vi.fn(), "conflicts");
    expect(await screen.findByText("原型尚未完成")).toBeTruthy();
    expect(screen.getByText(t("workspace:topicSummary.conflictsHint"))).toBeTruthy();
    expect(screen.getByText(t("workspace:topicSummary.conflictsWith"))).toBeTruthy();
    expect(screen.getByText("原型已完成")).toBeTruthy();
    expect(screen.getByText("旧")).toBeTruthy();
    const recorded = screen.getAllByText((text) => text.startsWith(t("workspace:topicSummary.recordedOn", { date: "" }).trim()));
    expect(recorded).toHaveLength(1);
  });

  it("列出需核对的声明与出处状态；出处按 ID 打开，失效时退回按路径", async () => {
    setCommandReply("topicClaims", {
      status: "ok",
      data: {
        claims: [
          {
            id: "c1",
            text: "发布推迟到下月",
            conflict: true,
            transcript_record: false,
            citations: [
              {
                citation_id: "c1:0",
                source_path: "Notes/周会.md",
                source_title: "周会",
                state: "source_changed",
                quote: "发布推迟到下个月",
                recorded_ms: null,
              },
            ],
            conflicts_with: null,
          },
        ],
        omitted: 0,
      },
    });
    setCommandReply("openTopicCitation", {
      status: "ok",
      data: { status: "opened", outcome: "exact", title: "周会" },
    });
    const onOpenPath = vi.fn();
    const onClear = vi.fn();
    mount(onOpenPath, onClear);
    expect(await screen.findByText("发布推迟到下月")).toBeTruthy();
    expect(screen.getByText(t("workspace:topicSummary.conflictTag"))).toBeTruthy();
    expect(screen.getByText(t("workspace:topicSummary.citationState.source_changed"))).toBeTruthy();
    expect(commandMock("topicClaims")).toHaveBeenCalledWith(TOPIC, "needs_check");
    fireEvent.click(screen.getByText("周会"));
    await vi.waitFor(() =>
      expect(commandMock("openTopicCitation")).toHaveBeenCalledWith(TOPIC, "c1:0"),
    );
    expect(onOpenPath).not.toHaveBeenCalled();
    setCommandReply("openTopicCitation", { status: "ok", data: { status: "not_citation" } });
    fireEvent.click(screen.getByText("周会"));
    await vi.waitFor(() => expect(onOpenPath).toHaveBeenCalledWith("Notes/周会.md"));
    // 引句按需展开：默认不显示，展开后可见，再点收起。
    expect(screen.queryByText("发布推迟到下个月")).toBeNull();
    fireEvent.click(screen.getByText(t("workspace:topicSummary.showQuotes")));
    expect(screen.getByText("发布推迟到下个月")).toBeTruthy();
    expect(screen.getByText(t("workspace:topicSummary.quoteFrom", { title: "周会" }))).toBeTruthy();
    fireEvent.click(screen.getByText(t("workspace:topicSummary.hideQuotes")));
    expect(screen.queryByText("发布推迟到下个月")).toBeNull();
    fireEvent.click(screen.getByText(t("workspace:topicSummary.showAll")));
    expect(onClear).toHaveBeenCalled();
  });

  it("没有符合条件的声明时说清楚", async () => {
    setCommandReply("topicClaims", { status: "ok", data: { claims: [], omitted: 0 } });
    mount();
    expect(await screen.findByText(t("workspace:topicSummary.filterEmpty"))).toBeTruthy();
  });

  it("转写记载声明带身份标签", async () => {
    setCommandReply("topicClaims", {
      status: "ok",
      data: {
        claims: [
          {
            id: "t1",
            text: "灰度十月结束",
            conflict: false,
            transcript_record: true,
            citations: [],
            conflicts_with: null,
          },
        ],
        omitted: 0,
      },
    });
    mount();
    expect(await screen.findByText("灰度十月结束")).toBeTruthy();
    expect(screen.getByText(t("workspace:topicSummary.transcriptTag"))).toBeTruthy();
  });
});
