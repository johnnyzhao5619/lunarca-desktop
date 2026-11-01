// @vitest-environment happy-dom
/**
 * 录音/转写页「能不能作主题依据」的说明（PRD-16-058 / PRD-16-065）：判定全在 Rust，这里只验
 * 三种结论各自渲染对的话与对的动作。
 */

import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { TranscriptKnowledgeNotice } from "./TranscriptKnowledgeNotice";

afterEach(cleanup);
beforeEach(resetBindingsDouble);

function mount(onRetranscribe: (() => void) | null = vi.fn()) {
  return render(
    withI18n(
      createElement(TranscriptKnowledgeNotice, {
        noteRel: "Recordings/周会.md",
        onRetranscribe,
        retranscribing: false,
      }),
    ),
  );
}

describe("TranscriptKnowledgeNotice", () => {
  it("没什么要说时整块不渲染", async () => {
    setCommandReply("transcriptKnowledgeNotice", { status: "ok", data: { status: "none" } });
    const view = mount();
    await vi.waitFor(() => expect(commandMock("transcriptKnowledgeNotice")).toHaveBeenCalled());
    expect(view.container.textContent).toBe("");
  });

  it("只有实时稿：说它暂不能作依据，并给重新转写", async () => {
    setCommandReply("transcriptKnowledgeNotice", {
      status: "ok",
      data: { status: "preview_only" },
    });
    const onRetranscribe = vi.fn();
    mount(onRetranscribe);
    expect(await screen.findByText(t("playback:knowledge.previewOnly"))).toBeTruthy();
    fireEvent.click(screen.getByText(t("playback:secondPass.run")));
    expect(onRetranscribe).toHaveBeenCalledTimes(1);
  });

  it("没有本地媒体时只说明、不给点了没用的重新转写", async () => {
    setCommandReply("transcriptKnowledgeNotice", {
      status: "ok",
      data: { status: "preview_only" },
    });
    mount(null);
    expect(await screen.findByText(t("playback:knowledge.previewOnly"))).toBeTruthy();
    expect(screen.queryByText(t("playback:secondPass.run"))).toBeNull();
  });

  it("在这里问一次：直说在哪处理，「允许」与「暂不」同等分量，选了即记下", async () => {
    setCommandReply("transcriptKnowledgeNotice", {
      status: "ok",
      data: { status: "ask", location: "organization" },
    });
    setCommandReply("setTranscriptSources", { status: "ok", data: null });
    mount();
    expect(await screen.findByText(t("playback:knowledge.askTitle"))).toBeTruthy();
    expect(
      screen.getByText(
        t("playback:knowledge.askBody", { where: t("playback:knowledge.where.organization") }),
      ),
    ).toBeTruthy();
    const allow = screen.getByText(t("playback:knowledge.allow")).closest("button");
    const notNow = screen.getByText(t("playback:knowledge.notNow")).closest("button");
    expect(allow?.className).toBe(notNow?.className);
    fireEvent.click(notNow as HTMLElement);
    expect(commandMock("setTranscriptSources")).toHaveBeenCalledWith("not_now");
  });
});
