import { describe, expect, it } from "vitest";
import {
  appendLevelFrames,
  compactVoiceLevels,
  decodeTrackLevels,
  decodeVoiceLevels,
  LEVEL_TRACK_PRIMARY,
  LEVEL_TRACK_SYSTEM,
  voiceLevelBucket,
} from "./voice-waveform";

describe("voice-waveform", () => {
  it("keeps quiet non-zero levels visible", () => {
    expect(voiceLevelBucket({ peak: 4 / 255, rms: 2 / 255 })).toBeGreaterThan(0);
  });

  it("decodes tauri raw byte arrays", () => {
    expect(decodeVoiceLevels([255, 128])).toEqual([{ peak: 1, rms: 128 / 255 }]);
  });

  it("bucket follows rms envelope, not peak (no full-bar saturation)", () => {
    // 真机 K4 回归：peak 顶满但 rms 中等 → 档位由 rms 决定，绝不被 peak 钉满。
    const loudPeakModerateRms = voiceLevelBucket({ peak: 1, rms: 0.16 });
    expect(loudPeakModerateRms).toBeLessThan(8);
    expect(loudPeakModerateRms).toBeGreaterThan(0);
    // rms 动态可见：强弱语音映射到不同档位。
    expect(voiceLevelBucket({ peak: 1, rms: 0.04 })).toBeLessThan(
      voiceLevelBucket({ peak: 1, rms: 0.5 }),
    );
  });

  it("keeps low speech energy visibly above the idle floor", () => {
    expect(voiceLevelBucket({ peak: 0.35, rms: 0.01 })).toBeGreaterThanOrEqual(3);
  });

  it("compacts overflowing frames to a bounded display count", () => {
    const frames = Array.from({ length: 32 }, (_, index) => ({
      peak: index === 17 ? 0.92 : 0.12,
      rms: index === 17 ? 0.58 : 0.01,
    }));

    const displayFrames = compactVoiceLevels(frames, 16);

    expect(displayFrames).toHaveLength(16);
    expect(displayFrames.some((frame) => frame.rms === 0.58)).toBe(true);
  });

  it("pads short input with quiet leading frames for a stable meter width", () => {
    const displayFrames = compactVoiceLevels([{ peak: 0.5, rms: 0.12 }], 4);

    expect(displayFrames).toEqual([
      { peak: 0, rms: 0 },
      { peak: 0, rms: 0 },
      { peak: 0, rms: 0 },
      { peak: 0.5, rms: 0.12 },
    ]);
  });

  it("pure transient (rms=0, peak>0) still lights minimal bar", () => {
    expect(voiceLevelBucket({ peak: 0.5, rms: 0 })).toBe(1);
    expect(voiceLevelBucket({ peak: 0, rms: 0 })).toBe(0);
  });

  // 录音通道轨前缀格式（TDD-02 §6 双轨电平；Rust tag_track_levels 对偶）。
  it("decodes track-tagged level messages", () => {
    expect(decodeTrackLevels([LEVEL_TRACK_SYSTEM, 255, 0])).toEqual({
      track: LEVEL_TRACK_SYSTEM,
      levels: [{ peak: 1, rms: 0 }],
    });
    expect(decodeTrackLevels(new Uint8Array([LEVEL_TRACK_PRIMARY, 128, 64, 0, 255]))).toEqual({
      track: LEVEL_TRACK_PRIMARY,
      levels: [
        { peak: 128 / 255, rms: 64 / 255 },
        { peak: 0, rms: 1 },
      ],
    });
    // 空消息 / 仅轨 id：合法，零帧。
    expect(decodeTrackLevels([])).toEqual({ track: LEVEL_TRACK_PRIMARY, levels: [] });
    expect(decodeTrackLevels([LEVEL_TRACK_SYSTEM])).toEqual({
      track: LEVEL_TRACK_SYSTEM,
      levels: [],
    });
  });

  it("appends decoded frames with rolling cap", () => {
    const acc = appendLevelFrames(
      [{ peak: 0.1, rms: 0.1 }],
      [
        { peak: 0.2, rms: 0.2 },
        { peak: 0.3, rms: 0.3 },
      ],
      2,
    );
    expect(acc).toEqual([
      { peak: 0.2, rms: 0.2 },
      { peak: 0.3, rms: 0.3 },
    ]);
  });
});
