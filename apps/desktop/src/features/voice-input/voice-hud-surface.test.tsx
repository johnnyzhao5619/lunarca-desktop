// @vitest-environment happy-dom
/**
 * 语音的单一实时表面（ADR-137 / PRD-02-018）：空闲一行、结果一个主动作、出错一定有出路。
 */

import { act, createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

type FakeChannel = { onmessage: ((payload: unknown) => void) | null };
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

function voiceState(extra: Record<string, unknown> = {}) {
  return {
    status: "idle",
    landing_mode: "inject",
    input_mode: "toggle",
    auto_refine: true,
    translate_enabled: false,
    translate_target_lang: "en",
    message_key: "voice:status.idle",
    message_detail: null,
    result_pending: false,
    ...extra,
  };
}

async function flush() {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
  });
}

async function mount() {
  setCommandReply("voiceInputState", voiceState());
  const view = render(withI18n(createElement(VoiceInputHud)));
  await vi.waitFor(() => expect(commandMock("voiceInputState")).toHaveBeenCalled());
  await flush();
  return view;
}

/** 点圆钮开一场（点按模式），把 Rust 的那条事件通道交给用例。 */
async function startSession() {
  setCommandReply("voiceInputStart", { status: "ok", data: voiceState({ status: "recording" }) });
  fireEvent.click(screen.getByRole("button", { name: t("voice-input:action.start") }));
  await vi.waitFor(() => expect(commandMock("voiceInputStart")).toHaveBeenCalled());
  await flush();
  // 事件通道就是交给 `voiceInputStart` 的那一条（另一条是电平通道）。
  const channel = commandMock("voiceInputStart").mock.calls[0][2] as FakeChannel;
  return (payload: unknown) =>
    act(async () => {
      channel.onmessage?.(payload);
      await Promise.resolve();
    });
}

describe("语音实时表面", () => {
  it("空闲只有一行：提示 + 落点胶囊，不显示 0:00，不再有第二张表面", async () => {
    const view = await mount();
    expect(view.container.querySelectorAll(".luna-hud-surface")).toHaveLength(1);
    expect(view.container.querySelector(".luna-voice__destination")?.textContent).toBe(
      t("voice-input:landing.inject"),
    );
    expect(view.container.querySelector(".luna-voice__clock")).toBeNull();
    expect(view.container.querySelector("textarea")).toBeNull();
    // 固定位置与语音设置常驻成图标；空闲时没有「⋯」——胶囊一行放不下下拉菜单（真机 2026-10-02）。
    expect(screen.getByRole("button", { name: t("voice-input:action.pin") })).toBeTruthy();
    expect(screen.getByRole("button", { name: t("voice-input:action.settings") })).toBeTruthy();
    expect(screen.queryByRole("button", { name: t("voice-input:action.more") })).toBeNull();
  });

  it("识别完成：编辑区 + 唯一主动作（默认去处），点它或 ⌘↵ 都落地", async () => {
    await mount();
    const emit = await startSession();
    await emit({
      kind: "transcript",
      data: { raw: "你好", final_text: "你好。", refined: true, output_kind: "refined" },
    });
    await emit({
      kind: "state",
      data: voiceState({ message_key: "voice:status.ready_to_land", result_pending: true }),
    });
    const editor = screen.getByLabelText(t("voice-input:preview.edit_label"));
    expect((editor as HTMLTextAreaElement).value).toBe("你好。");
    setCommandReply("voiceInputLand", { status: "ok", data: voiceState({ message_key: "voice:status.injected" }) });
    fireEvent.keyDown(editor, { key: "Enter", metaKey: true });
    await vi.waitFor(() => expect(commandMock("voiceInputLand")).toHaveBeenCalledWith("inject", null));
    // 落地成功给一次回执（status），文案就是 Rust 给的那句。
    expect(await screen.findByText(t("voice-input:status.injected"))).toBeTruthy();
  });

  it("出错一定有出路：注册表没有深链的码，回执线给「重说」", async () => {
    await mount();
    const emit = await startSession();
    await emit({ kind: "failed", data: { code: "SOMETHING_UNREGISTERED" } });
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain(t("voice-input:action.retry"));
  });
});
