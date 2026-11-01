/**
 * 状态栏「录音中 · 计时」持续态指示的纯判定（B2，PRD-07-045 / PRD-04-003）。
 *
 * 主窗不是录音会话的发起窗，它对录音的全部认知来自控制器的状态**转移**广播
 * （`recording://state-changed`，只在 status / session_id 变化时发，不逐帧发计时）。
 * 计时因此在本地推进：以转移那一刻的 `elapsed_ms` 为基线，录音中按墙钟累加，暂停即冻结。
 */

import type { RecordingState, RecordingStatus } from "../../ipc/bindings";

export interface RecordingIndicator {
  status: Extract<RecordingStatus, "recording" | "paused" | "stopping">;
  /** 转移那一刻会话已录时长（不含暂停间隙）。 */
  elapsedMs: number;
  /** 收到转移的本地墙钟（`Date.now()`）。 */
  receivedAt: number;
}

/** 从权威快照得出状态栏该不该亮、亮成什么；只认**录音**形态（字幕面板不归档，不进状态栏）。 */
export function recordingIndicatorFrom(
  state: RecordingState,
  receivedAt: number,
): RecordingIndicator | null {
  if (state.purpose !== "record") return null;
  if (state.status !== "recording" && state.status !== "paused" && state.status !== "stopping") {
    return null;
  }
  return { status: state.status, elapsedMs: state.elapsed_ms ?? 0, receivedAt };
}

/** 此刻该显示的时长：录音中随墙钟推进；暂停 / 停止中冻结在转移那一刻。 */
export function recordingIndicatorElapsedMs(indicator: RecordingIndicator, now: number): number {
  if (indicator.status !== "recording") return indicator.elapsedMs;
  return indicator.elapsedMs + Math.max(0, now - indicator.receivedAt);
}
