import { describe, expect, it, vi } from "vitest";

vi.mock("../../i18n/failure", () => ({
  hasFailureMessage: (code: string) => code === "LOCAL_SESSION_BUSY",
  failureMessage: (code: string) => `failure:${code}`,
}));

import { keyToMessage } from "./voice-message";

const t = (key: string) => `voice-input:${key}`;

describe("keyToMessage · 三个麦克风表面共用的状态键映射", () => {
  it("Rust 的 voice:status.* 内部键落到 voice-input 命名空间，而不是原样吐出裸键", () => {
    expect(keyToMessage("voice:status.recognizing", "recognizing", t)).toBe("voice-input:status.recognizing");
    expect(keyToMessage("voice:status.diagnostic", "recording", t)).toBe("voice-input:status.diagnostic");
    expect(keyToMessage("voice:status.cancelled", "idle", t)).toBe("voice-input:status.cancelled");
  });

  it("注入的两种如实结局（ADR-163 ③）：目标窗口已切走、剪贴板没能还原", () => {
    expect(keyToMessage("voice:status.target_changed", "copied_fallback", t)).toBe(
      "voice-input:status.target_changed",
    );
    expect(keyToMessage("voice:status.injected_clipboard_not_restored", "idle", t)).toBe(
      "voice-input:status.injected_clipboard_not_restored",
    );
  });

  it("errors:<code> 经唯一失败注册表；认不出的码回中性兜底而不是裸码", () => {
    expect(keyToMessage("errors:local_session_busy", "error", t)).toBe("failure:LOCAL_SESSION_BUSY");
    expect(keyToMessage("errors:invalid_input", "error", t)).toBe("voice-input:status.error");
  });

  it("未知键按状态兜底", () => {
    expect(keyToMessage(null, "error", t)).toBe("voice-input:status.error");
    expect(keyToMessage(undefined, "mic_busy", t)).toBe("voice-input:status.mic_busy");
    expect(keyToMessage("voice:status.nope", "idle", t)).toBe("voice-input:status.idle");
  });
});
