/**
 * 录音表面「以谁为准」的纯判定（H2 / M2，ADR-003：判定不带副作用，headless 可测）。
 *
 * 控制器是唯一事实源，它的状态**转移**经 `recording://state-changed` 全窗广播；会话自己的
 * Channel 只通向发起会话的那一扇窗。两条路合流到同一个 `state`，这里回答两个问题：
 * ① 这条广播是不是说给本表面听的；② 这次状态变化有没有跨过会话边界（横幅该不该清）。
 */

import type { RecordingState } from "../../ipc/bindings";
import type { RecordingMode } from "./use-recording";

/**
 * 广播的快照是否该被本表面采纳为权威状态。
 *
 * 录音 HUD 与字幕面板共用同一个控制器：一场字幕会话在跑时，录音 HUD 不该把它画成
 * 「录音中 / 停止」。会话态只认与本表面同形态的；`idle` 恒采纳——它是终态，也是
 * 「异常结束 → 复位」要送达每一扇窗的那一条（`RecordingState::idle()` 的 purpose 恒为
 * record，不能拿它做判据）。
 */
export function acceptsBroadcastState(mode: RecordingMode, next: RecordingState): boolean {
  return next.status === "idle" || next.purpose === mode;
}

/**
 * 从 `previous` 到 `next` 是否跨过了**会话边界**——横幅（转写已停 / 转译已停 / 重连中）
 * 与降级档位都绑定在会话实例上，跨过边界即清（M2）。
 *
 * 边界 = 会话结束（进入 `idle` / `error`）或换了一场会话（`session_id` 不同）。同一场会话
 * 内的 recording ⇄ paused ⇄ stopping 都不算：横幅说的事在暂停时仍然成立。
 */
export function crossesSessionBoundary(previous: RecordingState, next: RecordingState): boolean {
  if (next.status === "idle" || next.status === "error") {
    return previous.status !== next.status || previous.session_id !== next.session_id;
  }
  return previous.session_id !== next.session_id;
}
