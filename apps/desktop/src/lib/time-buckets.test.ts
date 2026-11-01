import { describe, expect, it } from "vitest";
import { bucketByTime } from "./time-buckets";

describe("共享时间分桶（速记流与录音 / 转写清单同源）", () => {
  const now = new Date(2026, 8, 3, 15, 0).getTime(); // 周四
  it("按桶序输出、桶内保持传入顺序、空桶不出现、没记时间归更早", () => {
    const rows = [
      { id: "a", t: new Date(2026, 8, 3, 9, 0).getTime() },
      { id: "b", t: new Date(2026, 8, 3, 8, 0).getTime() },
      { id: "c", t: new Date(2026, 8, 1, 8, 0).getTime() },
      { id: "d", t: null },
    ];
    expect(
      bucketByTime(rows, (r) => r.t, now).map((b) => [b.id, b.items.map((r) => r.id)]),
    ).toEqual([
      ["today", ["a", "b"]],
      ["thisWeek", ["c"]],
      ["earlier", ["d"]],
    ]);
  });
});
