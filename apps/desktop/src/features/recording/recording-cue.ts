/**
 * 录音 Cue 点纯逻辑：二分查找 + follow-mode 状态机 + 时长格式化。
 * 无 IO 依赖，可在 vitest 中独立测试。
 */

import type { CuePointDto, SubtitleIssueDto } from "../../ipc/bindings";

// ─── 类型 ─────────────────────────────────────────────────────────────────────

/** 已规格化（null 已消解）的 Cue 点。*/
export interface CuePoint {
  start_ms: number;
  end_ms: number;
  text: string;
  /** 译文（实时转译/二次转译产物；null = 无译文，PRD-04-025 双语回放）。 */
  translation: string | null;
  /** 说话人标签（"self"/"remote"/"remote:N"；null=无标签，PRD-04-053/054）。 */
  speaker: string | null;
  /**
   * 字幕规范问题（PRD-05-033）；空数组 = 合规。判定与阈值全在 Rust（与导出重切分同源），
   * 前端只渲染徽标（ADR-003 零业务）。
   */
  issues: SubtitleIssueDto[];
  /** 阅读速度（字符/秒）；null = 时长未知。徽标显示用，前端不自己算。 */
  cps: number | null;
}

/** Follow-mode：following = 自动跟随；manual = 用户手动滚动，暂停跟随。 */
export type FollowMode = "following" | "manual";

export interface FollowState {
  mode: FollowMode;
}

export type FollowAction =
  | { type: "user_scroll" }
  | { type: "seek_to_cue" }
  | { type: "return_to_current" };

// ─── 规格化 ───────────────────────────────────────────────────────────────────

/**
 * 将后端 DTO 转为内部 CuePoint[]，过滤掉 null 时间戳，按 start_ms 升序排列。
 * 幂等：多次调用结果一致。
 */
export function normaliseCues(dtos: CuePointDto[]): CuePoint[] {
  return dtos
    .filter((c): c is CuePointDto & { start_ms: number; end_ms: number } =>
      c.start_ms !== null && c.end_ms !== null,
    )
    .map((c) => ({
      start_ms: c.start_ms,
      end_ms: c.end_ms,
      text: c.text,
      translation: c.translation,
      speaker: c.speaker,
      issues: c.issues,
      cps: c.cps,
    }))
    .sort((a, b) => a.start_ms - b.start_ms);
}

// ─── 二分查找 ─────────────────────────────────────────────────────────────────

/**
 * 在已升序排列的 cues 中，找到最后一个 `start_ms <= positionMs` 的条目下标。
 * 若 positionMs 在所有 cue 之前，返回 -1。
 *
 * O(log n)。用于 timeupdate 回调（约 4Hz + rAF）。
 */
export function findActiveCueIndex(cues: CuePoint[], positionMs: number): number {
  if (cues.length === 0) return -1;
  let lo = 0;
  let hi = cues.length - 1;
  let result = -1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (cues[mid].start_ms <= positionMs) {
      result = mid;
      lo = mid + 1;
    } else {
      hi = mid - 1;
    }
  }
  return result;
}

// ─── Follow-mode 状态机 ───────────────────────────────────────────────────────

/**
 * 纯 reducer（无副作用）：
 * - `user_scroll`      → 切换到 manual（暂停自动跟随）
 * - `seek_to_cue`      → 保持当前模式（手动 seek 不恢复跟随）
 * - `return_to_current`→ 切换到 following（用户点击「回到当前」）
 */
export function followReducer(state: FollowState, action: FollowAction): FollowState {
  switch (action.type) {
    case "user_scroll":
      return { mode: "manual" };
    case "return_to_current":
      return { mode: "following" };
    case "seek_to_cue":
      return state;
  }
}

// ─── 辅助 ─────────────────────────────────────────────────────────────────────

/**
 * 键盘 seek：给定当前活动 cue 下标，按方向键偏移，返回目标 cue 的 start_ms。
 * @param delta +1 = 下一条，-1 = 上一条。
 */
export function keyboardSeekMs(cues: CuePoint[], activeCueIndex: number, delta: number): number | null {
  const target = activeCueIndex + delta;
  const cue = cues[target];
  if (!cue) return null;
  return cue.start_ms;
}
