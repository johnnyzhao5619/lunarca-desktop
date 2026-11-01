import { describe, expect, it } from "vitest";
import { bucketDrafts, bucketOf } from "./draft-inbox";

/** 2026-08-24 是周一；14:30 本地时间。 */
const MONDAY = new Date(2026, 7, 24, 14, 30, 0).getTime();
const DAY = 24 * 60 * 60 * 1000;
/** 2026-08-27 是周四。 */
const THURSDAY = new Date(2026, 7, 27, 9, 0, 0).getTime();

describe("记录收件箱 · 时间桶分组（PRD-06-058）", () => {
  it("按本地日历日边界分桶，而不是按「距今多少小时」", () => {
    // 今天凌晨 0:05 记的那一条仍属「今天」——它距今 14 小时，按滚动窗口会掉进昨天，
    // 而用户心里它就是今天记的。这是本模块存在的理由。
    expect(bucketOf(new Date(2026, 7, 24, 0, 5).getTime(), MONDAY)).toBe("today");
    // 昨天 23:55（距今 15 分钟）属「昨天」，同理。
    expect(bucketOf(new Date(2026, 7, 23, 23, 55).getTime(), MONDAY)).toBe("yesterday");
  });

  it("周一那天的「昨天」（周日）不掉进「更早」", () => {
    // 周日属上一自然周，但 today/yesterday 优先命中——否则每个周一都会出现
    // 「昨天记的东西被摆进更早」这种当场露馅的错。
    expect(bucketOf(MONDAY - DAY, MONDAY)).toBe("yesterday");
    // 上周六（同为上一自然周，且不是昨天）才是「更早」。
    expect(bucketOf(MONDAY - 2 * DAY, MONDAY)).toBe("earlier");
  });

  it("本周桶收的是本自然周内、且早于昨天的条目", () => {
    // 周四看：周一/周二属「本周」，上周日属「更早」。
    expect(bucketOf(MONDAY, THURSDAY)).toBe("thisWeek");
    expect(bucketOf(new Date(2026, 7, 25, 10, 0).getTime(), THURSDAY)).toBe("thisWeek");
    expect(bucketOf(new Date(2026, 7, 23, 10, 0).getTime(), THURSDAY)).toBe("earlier");
  });

  it("未来时间（时钟回拨/跨设备时差）按今天处理，不另开一个桶", () => {
    expect(bucketOf(MONDAY + 3 * DAY, MONDAY)).toBe("today");
  });

  it("取不到修改时间（索引缺省）按「更早」，不谎报成刚发生", () => {
    expect(bucketOf(null, MONDAY)).toBe("earlier");
  });

  it("空桶被丢弃，非空桶按新→旧固定顺序，桶内保留输入顺序", () => {
    const items = [
      { modified_ms: MONDAY, id: "a" },
      { modified_ms: MONDAY - 60_000, id: "b" },
      { modified_ms: MONDAY - 30 * DAY, id: "c" },
    ];
    const buckets = bucketDrafts(items, MONDAY);
    expect(buckets.map((b) => b.id)).toEqual(["today", "earlier"]);
    expect(buckets[0].items.map((i) => i.id)).toEqual(["a", "b"]);
    expect(buckets[1].items.map((i) => i.id)).toEqual(["c"]);
  });

  it("空输入不产出任何桶（空态由调用方渲染，不是四个空标题）", () => {
    expect(bucketDrafts([], MONDAY)).toEqual([]);
  });
});
