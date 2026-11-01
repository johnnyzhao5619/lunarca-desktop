import { describe, expect, it } from "vitest";

import {
  UNSCHEDULED_DROP_ID,
  dateDropId,
  dateOfDropId,
  isoDay,
  monthWeeks,
  shiftMonth,
  weekStartsOn,
} from "./creator-calendar";

describe("创作日历的格子", () => {
  it("2026 年 10 月从周一排：首行从 9 月 28 日起，补到 11 月 1 日，共五行", () => {
    const weeks = monthWeeks({ year: 2026, month: 9 }, 1, "2026-10-19");
    expect(weeks).toHaveLength(5);
    expect(weeks[0][0]).toMatchObject({ iso: "2026-09-28", inMonth: false });
    expect(weeks[0][3]).toMatchObject({ iso: "2026-10-01", day: 1, inMonth: true });
    expect(weeks[4][6]).toMatchObject({ iso: "2026-11-01", inMonth: false });
    expect(weeks.flat().filter((day) => day.today).map((day) => day.iso)).toEqual(["2026-10-19"]);
  });

  it("从周日排时首行从 9 月 27 日起", () => {
    expect(monthWeeks({ year: 2026, month: 9 }, 0, "")[0][0].iso).toBe("2026-09-27");
  });

  it("翻月跨年；英语界面一周从周日开始", () => {
    expect(shiftMonth({ year: 2026, month: 11 }, 1)).toEqual({ year: 2027, month: 0 });
    expect(shiftMonth({ year: 2026, month: 0 }, -1)).toEqual({ year: 2025, month: 11 });
    expect(weekStartsOn("en-US")).toBe(0);
    expect(weekStartsOn("zh-CN")).toBe(1);
    expect(isoDay(new Date(2026, 1, 3))).toBe("2026-02-03");
  });

  it("落点 id：某一天、未排期（清掉）、别的落点（取消）", () => {
    expect(dateOfDropId(dateDropId("2026-10-24"))).toBe("2026-10-24");
    expect(dateOfDropId(UNSCHEDULED_DROP_ID)).toBeNull();
    expect(dateOfDropId("creator-stage:idea")).toBeUndefined();
    expect(dateOfDropId("creator-date:garbage")).toBeUndefined();
  });
});
