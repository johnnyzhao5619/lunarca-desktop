import { describe, expect, it } from "vitest";
import { positionToStore, shouldPersist } from "./playback-position";

describe("播放位置写盘判据（PRD-04-068）", () => {
  it("第一次总要写；之后要同时满足间隔与位移", () => {
    expect(shouldPersist(null, 0, 0)).toBe(true);
    const last = { atMs: 10_000, positionMs: 20_000 };
    expect(shouldPersist(last, 12_000, 22_000)).toBe(false);
    expect(shouldPersist(last, 16_000, 20_500)).toBe(false);
    expect(shouldPersist(last, 16_000, 26_000)).toBe(true);
  });

  it("听完或几乎听完存 0，下次从头", () => {
    expect(positionToStore(59_000, 60_000)).toBe(0);
    expect(positionToStore(60_000, 60_000)).toBe(0);
    expect(positionToStore(30_500.7, 60_000)).toBe(30_500);
    expect(positionToStore(-5, 60_000)).toBe(0);
  });
});
