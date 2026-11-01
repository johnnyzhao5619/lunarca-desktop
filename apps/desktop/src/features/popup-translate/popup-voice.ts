/**
 * 弹窗翻译面板里语音输入的状态。
 *
 * **面板是预热复用的 NSPanel，不是每次打开重建的窗口**（ADR-015）：状态跨「隐藏 → 再显示」
 * 一直留着。语音失败的那句话住在输入框下方的提示位（那里平时写的是「输入要翻译的文本…」的
 * 副标题），于是一次失败之后，用户下一次打开面板做的哪怕是纯文字翻译，看到的仍是上一次的
 * 「凭据被拒绝」——那句话此刻既不对也不可操作（2026-09-05 真机：重新登录之后它还挂着，
 * 被当成了翻译本身的失败）。故面板每次重新打开都把它收回待命态。
 */

import type { VoiceInputStatus } from "../../ipc/bindings";

export type PopupVoiceState = {
  status: VoiceInputStatus;
  messageKey?: string;
  customMessage?: string;
  /** 失败时的稳定错误码（驱动错误卡的修复深链）；认不出的码不带。 */
  failureCode?: string;
};

/** 待命态：提示位回到「输入要翻译的文本…」那句副标题。 */
export function idlePopupVoice(): PopupVoiceState {
  return { status: "idle", messageKey: "status.voice_ready" };
}

/** 语音会话是否**正在进行**（进行中不该被面板刷新打断）。 */
export function isPopupVoiceActive(state: PopupVoiceState): boolean {
  return (
    state.status === "recording" ||
    state.status === "recognizing" ||
    state.status === "landing"
  );
}

/**
 * 面板重新打开时的语音状态：进行中的会话原样留着，其余（含上一次的失败）一律收回待命态。
 */
export function popupVoiceOnReopen(current: PopupVoiceState): PopupVoiceState {
  return isPopupVoiceActive(current) ? current : idlePopupVoice();
}
