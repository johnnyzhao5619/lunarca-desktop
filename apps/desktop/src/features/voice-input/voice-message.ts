/**
 * 语音输入状态键 → 人话（三个麦克风表面共用：语音 HUD、翻译窗的语音输入、以及将来任何
 * 挂上 `VoiceInputEvent` 的面）。
 *
 * **只有这一份映射**。Rust 侧（`src-tauri/src/voice_input.rs`）发的是 `voice:status.<x>`
 * 与 `errors:<code>` 这种内部键，它们**不是** i18next 的命名空间键——直接交给 `t()` 的话，
 * i18next 把 `voice` 当成一个不存在的命名空间，原样吐出「status.recognizing」这种裸键
 * （翻译窗 2026-09-05 真机）。此前这份映射只住在语音 HUD 里，翻译窗自己把键直接渲染。
 */

import type { VoiceInputStatus } from "../../ipc/bindings";
import { failureMessage, hasFailureMessage } from "../../i18n/failure";

/** `voice-input` 命名空间的 `t`。 */
export type Translate = (key: string) => string;

export function keyToMessage(
  key: string | null | undefined,
  status: VoiceInputStatus,
  t: Translate,
) {
  // 错误码（`errors:<code>`，Rust `voice_error_message_key` 现拼）→ 唯一失败注册表
  // `common:failure.*`（ADR-111 ④ 批 E2）。此前这里逐码手写 case 落到第三套文案
  // `voice-input:error.*`——switch 少接一个码的表现是那句中性兜底，ADR-109 ④ 为此立过
  // 「判据从 switch 现读」的门禁；通用映射把这个失败模式连根去掉，那条判据随之退役
  // （见 `job-error-code-channels.test.ts` 的通道注释）。认不出的码仍回中性兜底。
  if (key?.startsWith("errors:")) {
    const code = key.slice("errors:".length).toUpperCase();
    return hasFailureMessage(code) ? failureMessage(code) : t("status.error");
  }
  switch (key) {
    case "voice:status.cancelled":
      return t("status.cancelled");
    case "voice:status.copied":
      return t("status.copied");
    case "voice:status.copied_fallback":
      return t("status.copied_fallback");
    case "voice:status.injected":
      return t("status.injected");
    case "voice:status.injected_clipboard_not_restored":
      return t("status.injected_clipboard_not_restored");
    case "voice:status.target_changed":
      return t("status.target_changed");
    case "voice:status.landing":
      return t("status.landing");
    case "voice:status.ready_to_land":
      return t("status.ready_to_land");
    case "voice:status.popup_filled":
      return t("status.popup_filled");
    case "voice:status.script_filled":
      return t("status.script_filled");
    case "voice:status.saved_note":
      return t("status.saved_note");
    case "voice:status.recognizing":
      return t("status.recognizing");
    case "voice:status.refining":
      return t("status.refining");
    case "voice:status.translating":
      return t("status.translating");
    case "voice:status.recording":
      return t("status.recording");
    case "voice:status.speech_started":
      return t("status.speech_started");
    case "voice:status.diagnostic":
      return t("status.diagnostic");
    // 转写腿还没拿到本机会话额度（ADR-078 ③）：采集照常在跑，只是还没开始出字。
    // 额度到手后后端会立刻把状态改回 `voice:status.recording`，这一行不会挂着不走。
    case "voice:status.admission_wait":
      return t("status.admission_wait");
    default:
      if (status === "mic_busy") {
        return t("status.mic_busy");
      }
      if (status === "error") {
        return t("status.error");
      }
      return t("status.idle");
  }
}
