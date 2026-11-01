// @vitest-environment happy-dom
/**
 * 脚本「口述一段」（ADR-162 ⑮）：开始 / 结束只发命令；识别出的原话请活跃编辑器插入并选中。
 */

import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));
/** 记下组件建的 Channel，测试里冒充 Rust 往里送消息。 */
const channels: { onmessage: ((message: unknown) => void) | null }[] = [];
vi.mock("@tauri-apps/api/core", () => ({
  Channel: class {
    onmessage: ((message: unknown) => void) | null = null;
    constructor() {
      channels.push(this);
    }
  },
}));

import { commandMock, resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { t, withI18n } from "../../i18n/i18n-test-fixture";
import { SCRIPT_INSERT_EVENT } from "../../lib/app-events";
import { ScriptDictation } from "./ScriptDictation";

afterEach(cleanup);
beforeEach(() => {
  channels.length = 0;
  resetBindingsDouble();
});

describe("脚本口述", () => {
  it("开始、看见实时的字、结束；原话插进脚本并选中", async () => {
    setCommandReply("scriptVoiceInputStart", { status: "ok", data: { status: "recording" } });
    setCommandReply("voiceInputStop", { status: "ok", data: null });
    const inserted: unknown[] = [];
    const onInsert = (event: Event) => inserted.push((event as CustomEvent).detail);
    window.addEventListener(SCRIPT_INSERT_EVENT, onInsert);
    render(withI18n(createElement(ScriptDictation, { path: "Creator/P/P-脚本.md" })));
    fireEvent.click(screen.getByRole("button", { name: t("workspace:creator.dictation.start") }));
    const stop = await screen.findByRole("button", { name: t("workspace:creator.dictation.stop") });
    act(() => channels[0].onmessage?.({ kind: "partial", data: { text: "嗯今天我们", hypothesis: "聊聊" } }));
    expect(screen.getByText("嗯今天我们聊聊")).toBeTruthy();
    fireEvent.click(stop);
    expect(commandMock("voiceInputStop")).toHaveBeenCalledWith(null);
    act(() => channels[0].onmessage?.({ kind: "field_input", data: { text: "嗯今天我们聊聊写脚本" } }));
    expect(inserted).toEqual([{ path: "Creator/P/P-脚本.md", text: "嗯今天我们聊聊写脚本", select: true }]);
    expect(await screen.findByText(t("workspace:creator.dictation.inserted"))).toBeTruthy();
    window.removeEventListener(SCRIPT_INSERT_EVENT, onInsert);
  });

  it("麦克风被占着：命令照常返回但没开起来，说清原因、按钮不变", async () => {
    setCommandReply("scriptVoiceInputStart", {
      status: "ok",
      data: { status: "mic_busy", message_key: "voice:status.recording" },
    });
    render(withI18n(createElement(ScriptDictation, { path: "Creator/P/P-脚本.md" })));
    fireEvent.click(screen.getByRole("button", { name: t("workspace:creator.dictation.start") }));
    expect(await screen.findByRole("status")).toBeTruthy();
    expect(screen.getByRole("button", { name: t("workspace:creator.dictation.start") })).toBeTruthy();
  });
});
