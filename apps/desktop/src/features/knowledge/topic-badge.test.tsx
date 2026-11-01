// @vitest-environment happy-dom
/**
 * 主题页页头徽标跟着文件现状走（E09）：外部编辑器改了正文只发 `vault://changed`，
 * 页头必须随之从「Lunarca 维护」变成「已被修改」，而不是停在旧状态、与侧栏打架。
 */

import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import {
  commandMock,
  emitEvent,
  resetBindingsDouble,
  setCommandReply,
} from "../../ipc/bindings-test-double";
import { ContextMenuHost } from "../../components/ContextMenuHost";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { TopicBadge } from "./TopicBadge";

const TOPIC = "Knowledge/项目.md";

afterEach(cleanup);
beforeEach(resetBindingsDouble);

function mount() {
  return render(
    withI18n(
      createElement(ContextMenuHost, null, createElement(TopicBadge, { notePath: TOPIC })),
    ),
  );
}

describe("TopicBadge", () => {
  it("外部改了这一页：页头随 vault://changed 变成「已被修改」；别的笔记变化不重取", async () => {
    setCommandReply("topicState", {
      status: "ok",
      data: { ownership: "ai_owned", knowledge_map: false },
    });
    mount();
    expect(await screen.findByText(t("workspace:topic.ai"))).toBeTruthy();
    const calls = commandMock("topicState").mock.calls.length;

    emitEvent("vaultChanged", { rel_path: "Notes/别的.md", deleted: false });
    expect(commandMock("topicState").mock.calls.length).toBe(calls);

    setCommandReply("topicState", {
      status: "ok",
      data: { ownership: "user_owned", knowledge_map: false },
    });
    emitEvent("vaultChanged", { rel_path: TOPIC, deleted: false });
    expect(await screen.findByText(t("workspace:topic.user"))).toBeTruthy();
  });
});
