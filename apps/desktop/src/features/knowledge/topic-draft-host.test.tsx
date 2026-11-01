// @vitest-environment happy-dom
/**
 * 新建主题页的承接对话框（ADR-134）：先预览、再确认；确认带回原 operation ID 与预览指纹；
 * 放弃与关后迟到的结果都清理封存载荷，不会悄悄写入。后台已备好的初稿（PRD-16-060）直接展示、
 * 不调模型，关掉不清、「放弃」才清。
 */

import { createElement } from "react";
import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { NAVIGATE_EVENT } from "../../lib/navigate";
import { TopicDraftHost } from "./TopicDraftHost";
import { requestTopicDraft } from "./topic-draft";

afterEach(cleanup);
beforeEach(resetBindingsDouble);

const PLANNED = {
  status: "ok",
  data: {
    status: "planned",
    operation_id: "draft-1",
    target_path: "Knowledge/项目.md",
    result_hash: "hash-1",
    items: [
      {
        action: "append",
        text: "原型已经完成",
        sources: [{ path: "Notes/周会.md", title: "周会", evidence_index: 0 }],
        transcript_record: false,
        quick_note_record: false,
        related_text: null,
      },
      {
        action: "append",
        text: "灰度十月结束",
        sources: [{ path: "Transcripts/会议.md", title: "会议", evidence_index: 1 }],
        transcript_record: true,
        quick_note_record: false,
        related_text: null,
      },
    ],
    input_complete: true,
    omitted_sources: 0,
  },
};

function mount() {
  return render(withI18n(createElement(TopicDraftHost)));
}

describe("TopicDraftHost", () => {
  it("只凭标签准备，逐条预览后以原 ID 与指纹确认，并打开新页", async () => {
    setCommandReply("prepareTopicDraft", PLANNED);
    setCommandReply("confirmTopicDraft", {
      status: "ok",
      data: { status: "published", path: "Knowledge/项目.md" },
    });
    const routes: string[] = [];
    const onNavigate = (event: Event) => {
      routes.push((event as CustomEvent<string>).detail);
      event.preventDefault();
    };
    window.addEventListener(NAVIGATE_EVENT, onNavigate);
    mount();
    act(() => requestTopicDraft("项目"));
    expect(commandMock("topicDraftReady")).toHaveBeenCalledWith("项目");
    await vi.waitFor(() => expect(commandMock("prepareTopicDraft")).toHaveBeenCalledWith("项目"));
    expect(await screen.findByText("原型已经完成")).toBeTruthy();
    expect(screen.getByText(t("workspace:topicEvidence.action.transcript"))).toBeTruthy();
    fireEvent.click(screen.getByText(t("workspace:topicDraft.create")));
    expect(commandMock("confirmTopicDraft")).toHaveBeenCalledWith("draft-1", "hash-1");
    await vi.waitFor(() => expect(routes).toEqual(["topics://Knowledge/项目.md"]));
    expect(screen.queryByRole("dialog")).toBeNull();
    window.removeEventListener(NAVIGATE_EVENT, onNavigate);
  });

  it("放弃会清理封存的初稿，不写入", async () => {
    setCommandReply("prepareTopicDraft", PLANNED);
    setCommandReply("discardTopicDraft", { status: "ok", data: null });
    mount();
    act(() => requestTopicDraft("项目"));
    fireEvent.click(await screen.findByText(t("workspace:topicDraft.discard")));
    expect(commandMock("discardTopicDraft")).toHaveBeenCalledWith("draft-1");
    expect(commandMock("confirmTopicDraft")).not.toHaveBeenCalled();
  });

  it("关掉之后才到的结果也会被清理", async () => {
    let release: (value: unknown) => void = () => {};
    setCommandReply("prepareTopicDraft", () => new Promise((resolve) => (release = resolve)));
    setCommandReply("discardTopicDraft", { status: "ok", data: null });
    mount();
    act(() => requestTopicDraft("项目"));
    expect(await screen.findByText(t("workspace:topicDraft.preparing"))).toBeTruthy();
    fireEvent.click(screen.getByLabelText(t("workspace:topicEvidence.close")));
    await act(async () => release(PLANNED));
    await vi.waitFor(() =>
      expect(commandMock("discardTopicDraft")).toHaveBeenCalledWith("draft-1"),
    );
    expect(screen.queryByText("原型已经完成")).toBeNull();
  });

  it("后台已备好初稿时直接展示，不调模型；关掉不清、确认照旧带原 ID 与指纹", async () => {
    setCommandReply("topicDraftReady", PLANNED);
    setCommandReply("confirmTopicDraft", {
      status: "ok",
      data: { status: "published", path: "Knowledge/项目.md" },
    });
    mount();
    act(() => requestTopicDraft("项目"));
    expect(await screen.findByText("原型已经完成")).toBeTruthy();
    expect(screen.getByText(t("workspace:topicDraft.prepared"))).toBeTruthy();
    expect(commandMock("prepareTopicDraft")).not.toHaveBeenCalled();
    fireEvent.click(screen.getByLabelText(t("workspace:topicEvidence.close")));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(commandMock("discardTopicDraft")).not.toHaveBeenCalled();
    act(() => requestTopicDraft("项目"));
    fireEvent.click(await screen.findByText(t("workspace:topicDraft.create")));
    expect(commandMock("confirmTopicDraft")).toHaveBeenCalledWith("draft-1", "hash-1");
  });

  it("后台备好的初稿点「放弃」才清掉", async () => {
    setCommandReply("topicDraftReady", PLANNED);
    setCommandReply("discardTopicDraft", { status: "ok", data: null });
    mount();
    act(() => requestTopicDraft("项目"));
    fireEvent.click(await screen.findByText(t("workspace:topicDraft.discard")));
    expect(commandMock("discardTopicDraft")).toHaveBeenCalledWith("draft-1");
  });

  it("标签已有主题页时不另造，给打开入口", async () => {
    setCommandReply("prepareTopicDraft", {
      status: "ok",
      data: { status: "exists", path: "Knowledge/已有.md" },
    });
    mount();
    act(() => requestTopicDraft("项目"));
    expect(await screen.findByText(t("workspace:topicDraft.openExisting"))).toBeTruthy();
    expect(commandMock("confirmTopicDraft")).not.toHaveBeenCalled();
  });
});
