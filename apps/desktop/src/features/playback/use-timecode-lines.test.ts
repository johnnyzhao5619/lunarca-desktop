import { describe, expect, it } from "vitest";
import { activeTimecodeIndex } from "./use-timecode-lines";

const lines = [
  { line: 5, startMs: 0 },
  { line: 6, startMs: 1_500 },
  { line: 7, startMs: 4_000 },
];

describe("activeTimecodeIndex", () => {
  it("第一条之前返回 -1（起播前不该高亮任何一行）", () => {
    expect(activeTimecodeIndex([{ line: 5, startMs: 900 }], 100)).toBe(-1);
  });

  it("落在某条之上时取该条", () => {
    expect(activeTimecodeIndex(lines, 0)).toBe(0);
    expect(activeTimecodeIndex(lines, 1_499)).toBe(0);
    expect(activeTimecodeIndex(lines, 1_500)).toBe(1);
    expect(activeTimecodeIndex(lines, 3_999)).toBe(1);
  });

  it("超过最后一条仍停在最后一条（尾句读完不该突然不高亮）", () => {
    expect(activeTimecodeIndex(lines, 10_000)).toBe(2);
  });

  it("空表返回 -1", () => {
    expect(activeTimecodeIndex([], 1_000)).toBe(-1);
  });
});
