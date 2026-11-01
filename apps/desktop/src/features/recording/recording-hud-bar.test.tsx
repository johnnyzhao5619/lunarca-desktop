// @vitest-environment happy-dom
/**
 * 录音控制条（ADR-137 H5）：一行就是整扇窗；停止是中性强调；归档后给「已保存 · 打开」。
 * 会话与命令归 `useRecording`（这里替成受控的假实现），本文件只看渲染与回执。
 */

import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));
vi.mock("../../hooks/use-hud-window-auto-grow", () => ({ useHudWindowAutoGrow: () => {} }));
vi.mock("../../hooks/use-window-drag", () => ({
  useWindowDrag: () => ({ onPointerDown: vi.fn() }),
}));
vi.mock("./use-translation-mirror", () => ({
  useTranslationMirror: () => ({ separate: false, setSeparate: vi.fn() }),
}));

let controller: Record<string, unknown> = {};
vi.mock("./use-recording", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./use-recording")>();
  return { ...actual, useRecording: () => controller };
});

import { resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { NAVIGATE_EVENT } from "../../lib/navigate";
import { RecordingHud } from "./RecordingHud";

function fakeRecording(status: string, extra: Record<string, unknown> = {}) {
  return {
    state: {
      status,
      purpose: "record",
      session_id: status === "idle" ? null : "s1",
      note_rel: null,
      elapsed_ms: 12_000,
      message_key: `recording:status.${status}`,
      message_detail: null,
    },
    levels: [],
    systemLevels: [],
    degradedTier: null,
    degradedReason: null,
    executor: null,
    transcribeStopped: null,
    translateStopped: null,
    transcribeReconnecting: false,
    transcript: [],
    hypotheses: [],
    translations: [],
    microphoneMuted: false,
    archivedNote: null,
    clearArchivedNote: vi.fn(),
    transcribeEnabled: false,
    translateEnabled: false,
    translateTarget: "en",
    transcribeLang: "auto",
    detectedLang: null,
    devices: [],
    captureSource: "input_device",
    inputDevice: "",
    actions: new Proxy({}, { get: () => vi.fn(() => Promise.resolve({ ok: true })) }),
    ...extra,
  };
}

beforeEach(() => {
  resetBindingsDouble();
  setCommandReply("listSettings", { status: "ok", data: [] });
});
afterEach(cleanup);

describe("录音控制条", () => {
  it("空闲：身份位是唯一的「开始录音」，同一行是当前配置，不显示 00:00", () => {
    controller = fakeRecording("idle");
    const view = render(withI18n(createElement(RecordingHud)));
    expect(view.container.querySelectorAll(".luna-hud-header")).toHaveLength(1);
    expect(screen.getByText(t("recording:action.start"))).toBeTruthy();
    expect(view.container.querySelector(".luna-rec-hud__config-summary")).not.toBeNull();
    expect(view.container.querySelector(".luna-rec-hud__timer")).toBeNull();
  });

  it("录音中：计时占身份位，暂停 / 书签 / 停止在同一行，停止是中性强调", () => {
    controller = fakeRecording("recording");
    const view = render(withI18n(createElement(RecordingHud)));
    const header = view.container.querySelector(".luna-hud-header")!;
    expect(header.querySelector(".luna-rec-hud__timer")).not.toBeNull();
    expect(within(header as HTMLElement).getByLabelText(t("recording:action.pause"))).toBeTruthy();
    const stop = screen.getByText(t("recording:action.stop")).closest("button")!;
    expect(stop.className).toContain("luna-btn--secondary");
    expect(stop.className).not.toContain("luna-btn--danger");
  });

  it("归档之后给「已保存 · 打开」，打开去录音详情", () => {
    controller = fakeRecording("idle", { archivedNote: "Recordings/周会.md" });
    const seen: string[] = [];
    const onNavigate = (event: Event) => {
      seen.push((event as CustomEvent<string>).detail);
      event.preventDefault();
    };
    window.addEventListener(NAVIGATE_EVENT, onNavigate);
    render(withI18n(createElement(RecordingHud)));
    expect(screen.getByText(t("recording:status.saved"))).toBeTruthy();
    fireEvent.click(screen.getByText(t("recording:action.openRecording")));
    expect(seen).toEqual(["recording://Recordings/周会.md"]);
    window.removeEventListener(NAVIGATE_EVENT, onNavigate);
  });
});
