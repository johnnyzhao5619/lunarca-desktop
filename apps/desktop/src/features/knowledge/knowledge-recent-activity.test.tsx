// @vitest-environment happy-dom
/**
 * 工作台「继续阅读 / 近 7 天」（PRD-16-067 / TDD-07 §4.1e）：判定在 Rust，这里只证明「有内容
 * 才显示」、只报非零的那几类，以及入口导航到对应主题页或笔记。
 */

import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { NAVIGATE_EVENT } from "../../lib/navigate";
import { KnowledgeRecentActivity } from "./KnowledgeRecentActivity";

afterEach(cleanup);
beforeEach(resetBindingsDouble);

const QUIET_WEEK = {
  notes: 0,
  quick_notes: 0,
  recordings: 0,
  claims: 0,
  conflicts: 0,
  topics: [],
  topics_omitted: 0,
  unfiled: [],
  unfiled_count: 0,
};

function mount() {
  return render(withI18n(createElement(KnowledgeRecentActivity)));
}

function reply(continueReading: unknown, week: Record<string, unknown>) {
  setCommandReply("knowledgeRecentActivity", {
    status: "ok",
    data: { continue_reading: continueReading, week: { ...QUIET_WEEK, ...week } },
  });
}

describe("KnowledgeRecentActivity", () => {
  it("没有继续阅读、这 7 天也什么都没发生时不渲染", async () => {
    reply(null, {});
    const { container } = mount();
    await vi.waitFor(() => expect(commandMock("knowledgeRecentActivity")).toHaveBeenCalled());
    expect(container.querySelector(".luna-knowledge-recent")).toBeNull();
  });

  it("只有继续阅读时不渲染空的「近 7 天」", async () => {
    reply({ path: "Knowledge/项目.md", title: "项目" }, {});
    mount();
    expect(
      await screen.findByText(t("workspace:knowledgeRecent.continue", { title: "项目" })),
    ).toBeTruthy();
    expect(screen.queryByText(t("workspace:knowledgeRecent.weekTitle"))).toBeNull();
  });

  it("只报非零的几类来源，结论与分歧如实给出，主题与未归来源都能打开", async () => {
    reply(null, {
      notes: 2,
      recordings: 1,
      claims: 5,
      conflicts: 1,
      topics: [{ path: "Knowledge/周报.md", title: "周报", claims: 3, conflicts: 1 }],
      unfiled: [{ path: "Notes/新想法.md", title: "新想法" }],
      unfiled_count: 3,
    });
    const routes: string[] = [];
    const onNavigate = (event: Event) => {
      routes.push((event as CustomEvent<string>).detail);
      event.preventDefault();
    };
    window.addEventListener(NAVIGATE_EVENT, onNavigate);
    mount();
    const parts = [
      t("workspace:knowledgeRecent.weekNotes", { count: 2 }),
      t("workspace:knowledgeRecent.weekRecordings", { count: 1 }),
    ].join(t("workspace:knowledgeRecent.partSeparator"));
    expect(
      await screen.findByText(t("workspace:knowledgeRecent.weekSources", { count: 3, parts })),
    ).toBeTruthy();
    expect(
      screen.getByText(
        t("workspace:knowledgeRecent.weekClaimsWithConflicts", { count: 5, conflicts: 1 }),
      ),
    ).toBeTruthy();
    expect(screen.getByText(t("workspace:knowledgeRecent.weekUnfiled", { count: 3 }))).toBeTruthy();
    expect(screen.getByText(t("workspace:knowledgeRecent.weekUnfiledMore", { count: 2 }))).toBeTruthy();
    fireEvent.click(screen.getByText("周报"));
    fireEvent.click(screen.getByText("新想法"));
    expect(routes).toEqual(["topics://Knowledge/周报.md", "notes://Notes/新想法.md"]);
    window.removeEventListener(NAVIGATE_EVENT, onNavigate);
  });
});
