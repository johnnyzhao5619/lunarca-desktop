// @vitest-environment happy-dom
/**
 * 已全库忽略的 AI 标签清单（PRD-16-051）：只渲染 Rust 下发的事实，恢复交回 Rust。
 */

import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { IgnoredAiTags } from "./IgnoredAiTags";

afterEach(cleanup);
beforeEach(resetBindingsDouble);

function mount() {
  return render(withI18n(createElement(IgnoredAiTags)));
}

describe("IgnoredAiTags", () => {
  it("没有忽略时整块不出现", async () => {
    setCommandReply("ignoredAiTags", { status: "ok", data: [] });
    const view = mount();
    await vi.waitFor(() => expect(commandMock("ignoredAiTags")).toHaveBeenCalled());
    expect(view.container.textContent).toBe("");
  });

  it("展开后列出每个标签涉及的篇数，恢复交回 Rust 并重取", async () => {
    setCommandReply("ignoredAiTags", {
      status: "ok",
      data: [{ tag: "功能/网页端", note_count: 3 }],
    });
    setCommandReply("setAiTagIgnored", { status: "ok", data: null });
    mount();
    fireEvent.click(await screen.findByText(t("workspace:tags.ignoredAi", { count: 1 })));
    expect(screen.getByText("#功能/网页端")).toBeTruthy();
    expect(screen.getByText(t("workspace:tags.ignoredAiNotes", { count: 3 }))).toBeTruthy();
    fireEvent.click(screen.getByText(t("workspace:tags.restoreAiTag")));
    expect(commandMock("setAiTagIgnored")).toHaveBeenCalledWith("功能/网页端", false);
    await vi.waitFor(() => expect(commandMock("ignoredAiTags")).toHaveBeenCalledTimes(2));
  });
});
