/**
 * 两个入口的**输入单位不同**，而它们长得几乎一样——这正是需要机器钉住的那类区别。
 *
 * `formatDateTime` 收 epoch 毫秒（本地来源），字符串走 `parseInt`；`formatInstant` 收云侧
 * 时刻：Unix **秒**或 RFC 3339 字符串。拿后者的输入喂前者，`"2026-09-10T…"` 会被读成
 * 2026 毫秒，界面上是 1970 年——而两处都不会报错。
 */

import { describe, expect, it } from "vitest";

import { formatDateTime, formatInstant, formatMonthDay, formatMonthTitle, weekdayNames } from "./format-datetime";

describe("formatInstant", () => {
  it("RFC 3339 字符串按时刻解析，而不是按毫秒整数", () => {
    const rfc3339 = "2026-09-10T00:00:00Z";
    expect(formatInstant(rfc3339, "en-US")).toBe(new Date(rfc3339).toLocaleString("en-US"));
    // 反向：同一个串交给按毫秒解析的那一枚会落到 1970 年——两个入口不可互换。
    expect(formatDateTime(rfc3339, "en-US")).toBe(new Date(2026).toLocaleString("en-US"));
  });

  it("数字按 Unix 秒解析（服务端的 lease_expires_at 就是这个单位）", () => {
    expect(formatInstant(1_757_462_400, "en-US")).toBe(
      new Date(1_757_462_400_000).toLocaleString("en-US"),
    );
  });

  it("取不到的值返回空串，调用方据此不渲染那一行", () => {
    for (const value of [null, undefined, 0, -1, "", "not a date"]) {
      expect(formatInstant(value, "en-US")).toBe("");
    }
  });

  it("本地化跟界面语言走，不读系统 locale", () => {
    const at = 1_757_462_400;
    expect(formatInstant(at, "zh-CN")).not.toBe(formatInstant(at, "en-US"));
  });
});

describe("日历日的本地化", () => {
  it("计划发布日期按本地日历日显示，认不出为空串", () => {
    expect(formatMonthDay("2026-10-24", "en-US")).toBe("Oct 24");
    expect(formatMonthDay("2026-02-30", "en-US")).toBe("");
    expect(formatMonthDay(null, "en-US")).toBe("");
    expect(formatMonthTitle(2026, 9, "en-US")).toBe("October 2026");
    expect(weekdayNames("en-US", 0)[0]).toBe("Sun");
    expect(weekdayNames("en-US", 1)[0]).toBe("Mon");
  });
});
