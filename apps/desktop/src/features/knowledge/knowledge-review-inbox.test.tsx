// @vitest-environment happy-dom
/**
 * 知识库首页「待你看」（PRD-16-059）：判定全在 Rust，这里只验渲染与回传——批量接受只碰低风险
 * 的那几页、原样回传操作 ID 与结果指纹，「不采用」走随库决定，空时整块不出现。
 */

import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { KnowledgeReviewInbox } from "./KnowledgeReviewInbox";
import { recallApplyUndo } from "./apply-undo-memory";

afterEach(cleanup);
beforeEach(resetBindingsDouble);

function item(text: string, extra: Record<string, unknown> = {}) {
  return {
    action: "append",
    text,
    sources: [{ path: "Notes/周会.md", title: "周会", evidence_index: 0 }],
    transcript_record: false,
    quick_note_record: false,
    low_risk: true,
    related_text: null,
    ...extra,
  };
}

const INBOX = {
  groups: [
    {
      topic_path: "Knowledge/项目甲.md",
      topic_title: "项目甲",
      operation_id: "op-a",
      before_hash: "before-a",
      result_hash: "result-a",
      items: [item("后端改用 PostgreSQL")],
      low_risk: true,
      input_complete: true,
    },
    {
      topic_path: "Knowledge/项目乙.md",
      topic_title: "项目乙",
      operation_id: "op-b",
      before_hash: "before-b",
      result_hash: "result-b",
      items: [item("内测推迟到十二月", { action: "conflict", low_risk: false, related_text: "十月底内测" })],
      low_risk: false,
      input_complete: false,
    },
  ],
  omitted: 0,
};

function mount() {
  return render(withI18n(createElement(KnowledgeReviewInbox)));
}

describe("KnowledgeReviewInbox", () => {
  it("没有待确认的建议时整块不出现", async () => {
    setCommandReply("knowledgeReviewInbox", { status: "ok", data: { groups: [], omitted: 0 } });
    const view = mount();
    await vi.waitFor(() => expect(commandMock("knowledgeReviewInbox")).toHaveBeenCalled());
    expect(view.container.textContent).toBe("");
  });

  it("按主题分组列出每一句，批量接受只碰低风险的那几页并记下撤销凭据", async () => {
    setCommandReply("knowledgeReviewInbox", { status: "ok", data: INBOX });
    setCommandReply("confirmSealedIngestPlan", {
      status: "ok",
      data: { status: "queued", operation_id: "op-a", target_path: "Knowledge/项目甲.md" },
    });
    mount();
    expect(await screen.findByText("后端改用 PostgreSQL")).toBeTruthy();
    expect(screen.getByText("内测推迟到十二月")).toBeTruthy();
    expect(screen.getByText(t("workspace:reviewInbox.partialInput"))).toBeTruthy();
    fireEvent.click(screen.getByText(t("workspace:reviewInbox.acceptLowRisk", { count: 1 })));
    await vi.waitFor(() =>
      expect(commandMock("confirmSealedIngestPlan")).toHaveBeenCalledWith("op-a", "result-a"),
    );
    expect(commandMock("confirmSealedIngestPlan")).toHaveBeenCalledTimes(1);
    expect(recallApplyUndo("Knowledge/项目甲.md")).toEqual({ before: "before-a", after: "result-a" });
    expect(await screen.findByText(t("workspace:reviewInbox.queued", { count: 1 }))).toBeTruthy();
  });

  it("「不采用」把这一页的计划记为忽略", async () => {
    setCommandReply("knowledgeReviewInbox", { status: "ok", data: INBOX });
    setCommandReply("decideSealedIngestPlan", { status: "ok", data: { status: "recorded" } });
    mount();
    await screen.findByText("内测推迟到十二月");
    fireEvent.click(screen.getAllByText(t("workspace:reviewInbox.ignore"))[1]);
    await vi.waitFor(() =>
      expect(commandMock("decideSealedIngestPlan")).toHaveBeenCalledWith("op-b", "ignored"),
    );
  });
});
