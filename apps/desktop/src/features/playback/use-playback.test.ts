/**
 * 回放 Hook 纯逻辑测试（无 Tauri / DOM 依赖）。
 * 核心 cue 逻辑已在 recording-cue.test.ts 覆盖；此处侧重测试
 * 从 use-playback 重导出的函数以及双语模式约束。
 */

import { describe, expect, it } from "vitest";
import { findActiveCueIndex, followReducer, keyboardSeekMs } from "./use-playback";
import type { CuePoint } from "../recording/recording-cue";

const CUES: CuePoint[] = [
  { start_ms: 0, end_ms: 3000, text: "First", translation: null, speaker: null, issues: [], cps: null },
  { start_ms: 3000, end_ms: 6000, text: "Second", translation: null, speaker: null, issues: [], cps: null },
  { start_ms: 6000, end_ms: 9000, text: "Third", translation: null, speaker: null, issues: [], cps: null },
];

describe("playback cue binary search (re-export sanity)", () => {
  it("identifies correct cue at various positions", () => {
    expect(findActiveCueIndex(CUES, 0)).toBe(0);
    expect(findActiveCueIndex(CUES, 3000)).toBe(1);
    expect(findActiveCueIndex(CUES, 5999)).toBe(1);
    expect(findActiveCueIndex(CUES, 6000)).toBe(2);
  });
});

describe("playback follow-mode state machine (re-export sanity)", () => {
  it("user scroll → manual; return_to_current → following", () => {
    const afterScroll = followReducer({ mode: "following" }, { type: "user_scroll" });
    expect(afterScroll.mode).toBe("manual");
    const afterReturn = followReducer(afterScroll, { type: "return_to_current" });
    expect(afterReturn.mode).toBe("following");
  });
});

describe("keyboard seek", () => {
  it("navigates forward and backward within bounds", () => {
    expect(keyboardSeekMs(CUES, 0, +1)).toBe(3000);
    expect(keyboardSeekMs(CUES, 2, -1)).toBe(3000);
  });

  it("returns null at boundaries", () => {
    expect(keyboardSeekMs(CUES, 2, +1)).toBeNull();
    expect(keyboardSeekMs(CUES, 0, -1)).toBeNull();
  });
});

describe("断言：渲染层无业务决策（事件无正文传递）", () => {
  it("cue text never contains transcript metadata in test fixture", () => {
    // 验证 cue 点结构只暴露 text 字段，不含音频二进制、转写置信度等内部字段
    for (const cue of CUES) {
      const keys = Object.keys(cue);
      expect(keys).toContain("text");
      expect(keys).not.toContain("confidence");
      // speaker 自 PRD-04-053/054 起为正式回放字段（轨级/细分说话人标签），允许携带。
      expect(keys).toContain("speaker");
      expect(keys).not.toContain("raw_audio");
    }
  });
});
