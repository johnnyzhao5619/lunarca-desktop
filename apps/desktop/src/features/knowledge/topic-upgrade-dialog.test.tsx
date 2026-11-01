// @vitest-environment happy-dom
/**
 * 旧主题批量升级（TDD-06 §3.9.5）：选择范围 → 逐页影响 → 带指纹确认 → 逐页回执与撤销。
 * 判据是回传的参数与逐页呈现，不是源码字符串。
 */

import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { TopicUpgradeDialog } from "./TopicUpgradeDialog";

afterEach(cleanup);
beforeEach(resetBindingsDouble);

describe("TopicUpgradeDialog", () => {
  it("只预览勾选的页，只确认能升级的页，逐页可撤销", async () => {
    setCommandReply("topicUpgradeCandidates", {
      status: "ok",
      data: {
        pages: [
          { path: "Knowledge/甲.md", title: "甲" },
          { path: "Knowledge/乙.md", title: "乙" },
          { path: "Knowledge/丙.md", title: "丙" },
        ],
        omitted: 0,
      },
    });
    setCommandReply("previewTopicUpgrade", {
      status: "ok",
      data: [
        {
          status: "ready",
          path: "Knowledge/甲.md",
          before_hash: "b1",
          after_hash: "a1",
          scope_title: "项目 · 甲",
          scope_includes: ["项目/甲"],
          user_edited: false,
          unverified_lines: 2,
        },
        { status: "rejected", path: "Knowledge/乙.md", code: "missing_scope" },
      ],
    });
    setCommandReply("applyTopicUpgrade", {
      status: "ok",
      data: [
        {
          status: "upgraded",
          path: "Knowledge/甲.md",
          backup_id: "bk-1",
          index_updated: true,
          baseline_recorded: true,
          maintenance: "ready",
        },
      ],
    });
    setCommandReply("restoreTopicUpgrade", {
      status: "ok",
      data: { status: "restored", path: "Knowledge/甲.md" },
    });
    render(withI18n(createElement(TopicUpgradeDialog, { onClose: vi.fn() })));
    // 默认全选；取消「丙」后只预览两页。
    fireEvent.click(await screen.findByText("丙"));
    fireEvent.click(screen.getByText(t("workspace:topicUpgrade.preview", { count: 2 })));
    expect(commandMock("previewTopicUpgrade")).toHaveBeenCalledWith([
      "Knowledge/甲.md",
      "Knowledge/乙.md",
    ]);
    expect(
      await screen.findByText(
        t("workspace:topicEvidence.upgradeImpact", { scope: "项目 · 甲", count: 2 }),
      ),
    ).toBeTruthy();
    expect(screen.getByText(t("workspace:topicEvidence.upgradeRejection.missing_scope"))).toBeTruthy();
    // 只确认能升级的那一页，且带回预览时的前后指纹。
    fireEvent.click(screen.getByText(t("workspace:topicUpgrade.confirm", { count: 1 })));
    expect(commandMock("applyTopicUpgrade")).toHaveBeenCalledWith([
      { path: "Knowledge/甲.md", before_hash: "b1", after_hash: "a1" },
    ]);
    expect(await screen.findByText(t("workspace:topicEvidence.upgraded"))).toBeTruthy();
    fireEvent.click(screen.getByText(t("workspace:topicEvidence.upgradeUndo")));
    expect(commandMock("restoreTopicUpgrade")).toHaveBeenCalledWith("bk-1");
    expect(await screen.findByText(t("workspace:topicEvidence.upgradeRestored"))).toBeTruthy();
  });

  it("超出一批上限的页如实计数", async () => {
    setCommandReply("topicUpgradeCandidates", {
      status: "ok",
      data: { pages: [{ path: "Knowledge/甲.md", title: "甲" }], omitted: 7 },
    });
    render(withI18n(createElement(TopicUpgradeDialog, { onClose: vi.fn() })));
    expect(await screen.findByText(t("workspace:topicUpgrade.omitted", { count: 7 }))).toBeTruthy();
  });
});
