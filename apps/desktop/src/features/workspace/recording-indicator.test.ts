import { describe, expect, it } from "vitest";
import type { RecordingState } from "../../ipc/bindings";
import { recordingIndicatorElapsedMs, recordingIndicatorFrom } from "./recording-indicator";

function snapshot(partial: Partial<RecordingState>): RecordingState {
  return {
    status: "recording",
    purpose: "record",
    session_id: "s1",
    note_rel: "rec.md",
    elapsed_ms: 60_000,
    message_key: null,
    message_detail: null,
    ...partial,
  };
}

describe("状态栏录音指示（B2）", () => {
  it("录音 / 暂停 / 停止中亮，空闲与错误不亮", () => {
    expect(recordingIndicatorFrom(snapshot({ status: "recording" }), 1)?.status).toBe("recording");
    expect(recordingIndicatorFrom(snapshot({ status: "paused" }), 1)?.status).toBe("paused");
    expect(recordingIndicatorFrom(snapshot({ status: "stopping" }), 1)?.status).toBe("stopping");
    expect(recordingIndicatorFrom(snapshot({ status: "idle", session_id: null }), 1)).toBeNull();
    expect(recordingIndicatorFrom(snapshot({ status: "error" }), 1)).toBeNull();
    expect(recordingIndicatorFrom(snapshot({ status: "mic_busy" }), 1)).toBeNull();
  });

  it("字幕会话不进状态栏：它不归档，关窗也没有东西在长", () => {
    expect(recordingIndicatorFrom(snapshot({ purpose: "caption" }), 1)).toBeNull();
  });

  it("录音中按墙钟从转移基线累加；暂停与停止中冻结", () => {
    const at = 10_000;
    const live = recordingIndicatorFrom(snapshot({ status: "recording" }), at)!;
    expect(recordingIndicatorElapsedMs(live, at + 2_500)).toBe(62_500);
    // 时钟倒走（系统校时）不许把计时拉回去。
    expect(recordingIndicatorElapsedMs(live, at - 5_000)).toBe(60_000);
    const paused = recordingIndicatorFrom(snapshot({ status: "paused" }), at)!;
    expect(recordingIndicatorElapsedMs(paused, at + 9_999)).toBe(60_000);
  });
});
