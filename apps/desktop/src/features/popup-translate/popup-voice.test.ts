import { describe, expect, it } from "vitest";

import { idlePopupVoice, isPopupVoiceActive, popupVoiceOnReopen } from "./popup-voice";

describe("弹窗翻译的语音状态跨「隐藏 → 再显示」", () => {
  it("上一次的失败不会挂在提示位上：面板重开即回待命态", () => {
    const failed = { status: "error" as const, messageKey: "status.voice_error", customMessage: "凭据被拒绝" };
    expect(popupVoiceOnReopen(failed)).toEqual(idlePopupVoice());
    expect(popupVoiceOnReopen({ status: "idle", messageKey: "status.voice_ready" })).toEqual(idlePopupVoice());
    expect(popupVoiceOnReopen({ status: "mic_busy" })).toEqual(idlePopupVoice());
  });

  it("正在进行的语音会话不被面板刷新打断", () => {
    for (const status of ["recording", "recognizing", "landing"] as const) {
      const active = { status, messageKey: `status.${status}` };
      expect(isPopupVoiceActive(active)).toBe(true);
      expect(popupVoiceOnReopen(active)).toBe(active);
    }
  });
});
