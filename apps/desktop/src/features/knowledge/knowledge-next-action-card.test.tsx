// @vitest-environment happy-dom
/**
 * 推荐卡的三态与主动作（PRD-16-039 / PRD-16-053）：「还在查」「查不到」与「没有要做的事」
 * 是三句不同的话；有推荐时它是这一屏唯一的实色主按钮。
 */

import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { KnowledgeNextActionCard } from "./KnowledgeNextActionCard";

afterEach(cleanup);
beforeEach(resetBindingsDouble);

const checkupAction = {
  suggestion_token: "tok",
  kind: "checkup_finding",
  count: 2,
  action: "open_checkup",
  blocker: null,
  target: null,
  maps: [],
  map_review: null,
  topic: null,
  tags: [],
};

function mount() {
  return render(withI18n(createElement(KnowledgeNextActionCard, { scope: "overview" })));
}

describe("KnowledgeNextActionCard", () => {
  it("标签语言不一致：列出 Rust 给的受影响标签（C16）", async () => {
    setCommandReply("knowledgeNextAction", {
      status: "ok",
      data: {
        ...checkupAction,
        kind: "tag_language_mismatch",
        action: "rescan_tags_force",
        count: 3,
        tags: ["architecture", "ops"],
      },
    });
    mount();
    const list = await screen.findByRole("list", { name: t("workspace:graph.nextAction.tagLanguageSample") });
    expect(list.textContent).toContain("#architecture");
    expect(list.textContent).toContain("#ops");
  });

  it("取不到时说失败并给重试，不把出错说成「暂时不用整理」", async () => {
    setCommandReply("knowledgeNextAction", { status: "error", error: "IO" });
    mount();

    expect(await screen.findByText(t("workspace:graph.nextAction.loadError"))).toBeTruthy();
    expect(screen.queryByText(t("workspace:graph.nextAction.empty"))).toBeNull();

    setCommandReply("knowledgeNextAction", { status: "ok", data: null });
    fireEvent.click(screen.getByText(t("common:action.retry")));
    expect(await screen.findByText(t("workspace:graph.nextAction.empty"))).toBeTruthy();
  });

  it("有推荐时主动作是实色主按钮", async () => {
    setCommandReply("knowledgeNextAction", { status: "ok", data: checkupAction });
    mount();

    const cta = await screen.findByText(t("workspace:graph.nextAction.maintenanceCheck"));
    expect(cta.closest("button")?.className).toContain("luna-btn--primary");
  });
});
