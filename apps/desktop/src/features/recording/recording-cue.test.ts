import { describe, expect, it } from "vitest";
import {
  findActiveCueIndex,
  followReducer,
  keyboardSeekMs,
  normaliseCues,
  type CuePoint,
} from "./recording-cue";

// ─── fixtures ────────────────────────────────────────────────────────────────

const CUES: CuePoint[] = [
  { start_ms: 0, end_ms: 2000, text: "Hello", translation: null, speaker: null, issues: [], cps: null },
  { start_ms: 2000, end_ms: 4000, text: "World", translation: null, speaker: null, issues: [], cps: null },
  { start_ms: 5000, end_ms: 7000, text: "Bye", translation: null, speaker: null, issues: [], cps: null },
];

// ─── findActiveCueIndex ───────────────────────────────────────────────────────

describe("findActiveCueIndex", () => {
  it("returns -1 for empty list", () => {
    expect(findActiveCueIndex([], 1000)).toBe(-1);
  });

  it("returns 0 at position 0ms (first cue starts at 0)", () => {
    expect(findActiveCueIndex(CUES, 0)).toBe(0);
  });

  it("returns 0 mid-first-cue", () => {
    expect(findActiveCueIndex(CUES, 1000)).toBe(0);
  });

  it("returns 1 at cue boundary 2000ms", () => {
    expect(findActiveCueIndex(CUES, 2000)).toBe(1);
  });

  it("returns 1 inside second cue", () => {
    expect(findActiveCueIndex(CUES, 3000)).toBe(1);
  });

  it("returns 1 in gap between second and third cue", () => {
    expect(findActiveCueIndex(CUES, 4500)).toBe(1);
  });

  it("returns last cue index past end of all cues", () => {
    expect(findActiveCueIndex(CUES, 99_000)).toBe(2);
  });

  it("returns -1 before the first cue when cues start > 0", () => {
    const shifted: CuePoint[] = [{ start_ms: 500, end_ms: 1000, text: "A", translation: null, speaker: null, issues: [], cps: null }];
    expect(findActiveCueIndex(shifted, 300)).toBe(-1);
  });

  it("handles single-element list", () => {
    const single: CuePoint[] = [{ start_ms: 100, end_ms: 200, text: "X", translation: null, speaker: null, issues: [], cps: null }];
    expect(findActiveCueIndex(single, 100)).toBe(0);
    expect(findActiveCueIndex(single, 50)).toBe(-1);
  });
});

// ─── normaliseCues ────────────────────────────────────────────────────────────

describe("normaliseCues", () => {
  it("filters out entries where start_ms is null", () => {
    expect(
      normaliseCues([{ start_ms: null, end_ms: 1000, text: "x", translation: null, speaker: null, issues: [], cps: null }]),
    ).toHaveLength(0);
  });

  it("filters out entries where end_ms is null", () => {
    expect(
      normaliseCues([{ start_ms: 0, end_ms: null, text: "x", translation: null, speaker: null, issues: [], cps: null }]),
    ).toHaveLength(0);
  });

  it("sorts by start_ms ascending", () => {
    const result = normaliseCues([
      { start_ms: 2000, end_ms: 3000, text: "b", translation: null, speaker: null, issues: [], cps: null },
      { start_ms: 0, end_ms: 1000, text: "a", translation: null, speaker: null, issues: [], cps: null },
      { start_ms: 1000, end_ms: 2000, text: "c", translation: null, speaker: null, issues: [], cps: null },
    ]);
    expect(result.map((c) => c.text)).toEqual(["a", "c", "b"]);
  });

  it("passes through valid cues unchanged", () => {
    const result = normaliseCues([{ start_ms: 0, end_ms: 1000, text: "hello", translation: null, speaker: null, issues: [], cps: null }]);
    expect(result).toHaveLength(1);
    expect(result[0]).toEqual({
      start_ms: 0,
      end_ms: 1000,
      text: "hello",
      translation: null,
      speaker: null,
      issues: [],
      cps: null,
    });
  });

  it("passes translation through for bilingual playback (PRD-04-025)", () => {
    const result = normaliseCues([
      { start_ms: 0, end_ms: 1000, text: "大家好。", translation: "Hello everyone.", speaker: null, issues: [], cps: null },
    ]);
    expect(result[0].translation).toBe("Hello everyone.");
  });
});

// ─── followReducer ────────────────────────────────────────────────────────────

describe("followReducer", () => {
  it("user_scroll switches following → manual", () => {
    expect(followReducer({ mode: "following" }, { type: "user_scroll" })).toEqual({ mode: "manual" });
  });

  it("user_scroll keeps manual → manual", () => {
    expect(followReducer({ mode: "manual" }, { type: "user_scroll" })).toEqual({ mode: "manual" });
  });

  it("return_to_current switches manual → following", () => {
    expect(followReducer({ mode: "manual" }, { type: "return_to_current" })).toEqual({ mode: "following" });
  });

  it("return_to_current keeps following → following", () => {
    expect(followReducer({ mode: "following" }, { type: "return_to_current" })).toEqual({ mode: "following" });
  });

  it("seek_to_cue does not change mode in following", () => {
    expect(followReducer({ mode: "following" }, { type: "seek_to_cue" })).toEqual({ mode: "following" });
  });

  it("seek_to_cue does not change mode in manual", () => {
    expect(followReducer({ mode: "manual" }, { type: "seek_to_cue" })).toEqual({ mode: "manual" });
  });
});

// ─── keyboardSeekMs ───────────────────────────────────────────────────────────

describe("keyboardSeekMs", () => {
  it("moves to next cue", () => {
    expect(keyboardSeekMs(CUES, 0, +1)).toBe(2000);
  });

  it("moves to previous cue", () => {
    expect(keyboardSeekMs(CUES, 1, -1)).toBe(0);
  });

  it("returns null when past end", () => {
    expect(keyboardSeekMs(CUES, 2, +1)).toBeNull();
  });

  it("returns null when before start", () => {
    expect(keyboardSeekMs(CUES, 0, -1)).toBeNull();
  });
});
