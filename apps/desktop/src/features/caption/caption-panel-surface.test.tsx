// @vitest-environment happy-dom
/**
 * 实时字幕的纯内容浮层（ADR-137 H6）：只有一枚工具药丸；「框体 / 沉浸」合成一根背景浓淡
 * 滑杆（写同一个设备级设置，0 = 沉浸）；开始 / 停止是药丸里唯一的主动作。
 */

import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));
vi.mock("../../hooks/use-hud-window-auto-grow", () => ({ useHudWindowAutoGrow: () => {} }));
vi.mock("../../hooks/use-window-drag", () => ({
  useWindowDrag: () => ({ onPointerDown: vi.fn() }),
}));
const setSeparate = vi.fn();
vi.mock("../recording/use-translation-mirror", () => ({
  MIRROR_SETTING_KEYS: { caption: "caption.separate_translation", record: "recording.separate_translation" },
  useTranslationMirror: () => ({
    separate: false,
    setSeparate,
    view: { cues: [], hypotheses: [] },
  }),
}));

let status = "idle";
const start = vi.fn();
const stop = vi.fn();
const setTranslate = vi.fn();
const setCaptureSource = vi.fn();
vi.mock("../recording/use-recording", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../recording/use-recording")>();
  return {
    ...actual,
    useRecording: () => ({
      state: { status, purpose: "caption", session_id: null, note_rel: null, elapsed_ms: 0, message_key: null, message_detail: null },
      transcript: [],
      hypotheses: [],
      translations: [],
      translateEnabled: false,
      translateTarget: "en",
      transcribeLang: "auto",
      detectedLang: null,
      captureSource: "input_device",
      executor: null,
      transcribeStopped: null,
      translateStopped: null,
      transcribeReconnecting: false,
      actions: new Proxy({ start, stop, setTranslate, setCaptureSource }, { get: (target, key: string) => (target as Record<string, unknown>)[key] ?? vi.fn() }),
    }),
  };
});

import { commandMock, emitEvent, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { CaptionPanel } from "./CaptionPanel";

beforeEach(() => {
  resetBindingsDouble();
  vi.clearAllMocks();
  setCommandReply("listSettings", {
    status: "ok",
    data: [{ key: "caption.frame_opacity", current_value: { kind: "int", value: 40 } }],
  });
  setCommandReply("setSetting", { status: "ok", data: null });
});
afterEach(cleanup);

describe("实时字幕浮层", () => {
  it("只有一枚工具药丸；空闲时主动作是「开始」", () => {
    status = "idle";
    const view = render(withI18n(createElement(CaptionPanel)));
    expect(view.container.querySelectorAll(".luna-caption__toolbar")).toHaveLength(1);
    fireEvent.click(screen.getByText(t("caption:action.start")));
    expect(start).toHaveBeenCalled();
    expect(view.container.querySelector(".luna-caption")?.getAttribute("data-session")).toBe("idle");
  });

  it("外观面板就地改：背景浓淡写 caption.frame_opacity，拖到 0 即沉浸", async () => {
    status = "idle";
    const view = render(withI18n(createElement(CaptionPanel)));
    fireEvent.click(screen.getByRole("button", { name: t("caption:appearance.label") }));
    const slider = view.container.querySelectorAll<HTMLInputElement>(".luna-caption__appearance-slider")[0];
    await waitFor(() => expect(slider.value).toBe("40"));
    expect(view.container.querySelector(".luna-caption")?.getAttribute("data-immersive")).toBe("false");
    fireEvent.change(slider, { target: { value: "0" } });
    expect(commandMock("setSetting")).toHaveBeenCalledWith("caption.frame_opacity", { kind: "int", value: 0 });
    expect(view.container.querySelector(".luna-caption")?.getAttribute("data-immersive")).toBe("true");
  });

  it("会话中主动作是「停止」，且不提供关闭（会话中的退出动作就是停止）", () => {
    status = "recording";
    render(withI18n(createElement(CaptionPanel)));
    fireEvent.click(screen.getByText(t("caption:action.stop")));
    expect(stop).toHaveBeenCalled();
    expect(screen.queryByLabelText(t("caption:action.closePanel"))).toBeNull();
  });

  it("显示方式三段：选「译文」写 caption.show_source=false；选「原文」即关翻译", async () => {
    status = "idle";
    render(withI18n(createElement(CaptionPanel)));
    fireEvent.click(screen.getByRole("button", { name: t("caption:display.translation") }));
    expect(commandMock("setSetting")).toHaveBeenCalledWith("caption.show_source", { kind: "bool", value: false });
  });

  it("「译文在上」随时可选：选它即切到双语并写 caption.translation_first（真机 2026-10-02）", () => {
    status = "idle";
    render(withI18n(createElement(CaptionPanel)));
    fireEvent.click(screen.getByRole("button", { name: t("caption:appearance.label") }));
    const button = screen.getByRole<HTMLButtonElement>("button", { name: t("caption:layout.translation_first") });
    expect(button.disabled).toBe(false);
    fireEvent.click(button);
    expect(setTranslate).toHaveBeenCalledWith(true);
    expect(commandMock("setSetting")).toHaveBeenCalledWith("caption.translation_first", { kind: "bool", value: true });
    fireEvent.click(screen.getByRole("button", { name: t("caption:layout.separate") }));
    expect(setSeparate).toHaveBeenCalledWith(true);
  });

  it("声音来源在面板里就能换；会话中锁住并说明原因", () => {
    status = "idle";
    const view = render(withI18n(createElement(CaptionPanel)));
    fireEvent.click(screen.getByRole("button", { name: t("caption:appearance.label") }));
    fireEvent.click(screen.getByRole("button", { name: t("settings:option.caption.capture_source.system_loopback") }));
    expect(setCaptureSource).toHaveBeenCalledWith("system_loopback");
    view.unmount();

    status = "recording";
    render(withI18n(createElement(CaptionPanel)));
    fireEvent.click(screen.getByRole("button", { name: t("caption:appearance.label") }));
    expect(
      screen.getByRole<HTMLButtonElement>("button", { name: t("settings:option.caption.capture_source.both") }).disabled,
    ).toBe(true);
    expect(screen.getByText(t("caption:source.nextSession"))).toBeTruthy();
  });

  it("窗口被收起时外观面板一并收起，下次打开不带着它（真机 2026-10-02）", () => {
    status = "idle";
    const view = render(withI18n(createElement(CaptionPanel)));
    fireEvent.click(screen.getByRole("button", { name: t("caption:appearance.label") }));
    const panel = () => view.container.querySelector(".luna-caption__appearance");
    expect(panel()).not.toBeNull();

    act(() => emitEvent("captionWindowHidden", { label: "live-caption-translation" }));
    expect(panel(), "收起的是译文窗，原文窗的面板不动").not.toBeNull();

    act(() => emitEvent("captionWindowHidden", { label: "live-caption" }));
    expect(panel()).toBeNull();
  });
});
