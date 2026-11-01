// @vitest-environment happy-dom
/**
 * 语音面板的关闭确认（PRD-02-022 的另一面）：关掉会丢东西时先问一句，不丢就直接关。
 * 「会不会丢」由 Rust 下发（`result_pending`、会话状态），前端不按落地文案键自己判。
 */

import { act, createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));
vi.mock("@tauri-apps/api/core", () => ({
  Channel: class {
    onmessage: ((payload: unknown) => void) | null = null;
  },
}));
vi.mock("../../hooks/use-hud-window-auto-grow", () => ({ useHudWindowAutoGrow: () => {} }));
vi.mock("../../hooks/use-window-drag", () => ({
  useWindowDrag: () => ({ onPointerDown: vi.fn() }),
}));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { VoiceInputHud } from "./VoiceInputHud";

afterEach(cleanup);
beforeEach(() => {
  resetBindingsDouble();
  setCommandReply("listSettings", { status: "ok", data: [] });
  setCommandReply("modelManagementSnapshot", {
    status: "ok",
    data: { models: [], assignments: [], storage: null, services: [], remote_compute: null },
  });
});

function voiceState(extra: Record<string, unknown>) {
  return {
    status: "idle",
    landing_mode: "inject",
    input_mode: "hold",
    auto_refine: true,
    translate_enabled: false,
    translate_target_lang: "en",
    message_key: "voice:status.idle",
    message_detail: null,
    result_pending: false,
    ...extra,
  };
}

async function mountWith(extra: Record<string, unknown>) {
  setCommandReply("voiceInputState", voiceState(extra));
  render(withI18n(createElement(VoiceInputHud)));
  await vi.waitFor(() => expect(commandMock("voiceInputState")).toHaveBeenCalled());
  // 让那次状态应答落进组件（Esc 的判断读的是落下来的状态）。
  await act(async () => {
    await Promise.resolve();
  });
}

describe("语音面板的关闭确认", () => {
  it("没有会话、也没有待落地结果时，Esc 直接关闭", async () => {
    await mountWith({});
    fireEvent.keyDown(window, { key: "Escape" });
    await vi.waitFor(() => expect(commandMock("voiceInputClose")).toHaveBeenCalled());
    expect(screen.queryByText(t("voice-input:close.title"))).toBeNull();
  });

  it("Rust 说还有待落地结果时，Esc 先确认、不关窗", async () => {
    await mountWith({ message_key: "voice:status.ready_to_land", result_pending: true });
    fireEvent.keyDown(window, { key: "Escape" });
    expect(await screen.findByText(t("voice-input:close.title"))).toBeTruthy();
    expect(commandMock("voiceInputClose")).not.toHaveBeenCalled();
  });
});
