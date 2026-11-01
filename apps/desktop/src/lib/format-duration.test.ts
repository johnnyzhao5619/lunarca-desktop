import { describe, expect, it } from "vitest";
import { formatClock, formatDuration } from "./format-duration";

describe("时钟式（正在走的时间与媒体位置）", () => {
  it("0 读作 0:00", () => {
    expect(formatClock(0)).toBe("0:00");
  });

  it("分钟不补前导零", () => {
    expect(formatClock(65_000)).toBe("1:05");
  });

  it("满一小时才出现小时段", () => {
    expect(formatClock(3_599_000)).toBe("59:59");
    expect(formatClock(3_661_000)).toBe("1:01:01");
  });

  /**
   * 收编前语音 HUD 那份**不处理小时**：录到一小时十二分，它念作 `72:15`，
   * 而隔壁录音 HUD 念作 `1:12:15`。同一件事、同一屏、两种读法——这条用例锁住它。
   */
  it("超过一小时不退化成两位分钟", () => {
    expect(formatClock(4_335_000)).toBe("1:12:15");
  });

  it("空值与非数一律 0:00，不抛也不显示 NaN", () => {
    expect(formatClock(null)).toBe("0:00");
    expect(formatClock(undefined)).toBe("0:00");
    expect(formatClock(NaN)).toBe("0:00");
    expect(formatClock(-1)).toBe("0:00");
  });
});

describe("单位式（累计用量）", () => {
  it("秒级 → m/s，省略零时", () => {
    expect(formatDuration(0)).toBe("0m 0s");
    expect(formatDuration(27)).toBe("0m 27s");
    expect(formatDuration(125)).toBe("2m 5s");
  });

  it("超过一小时带 h 段", () => {
    expect(formatDuration(3785)).toBe("1h 3m 5s");
  });

  it("负数与小数被夹取/取整", () => {
    expect(formatDuration(-5)).toBe("0m 0s");
    expect(formatDuration(59.9)).toBe("0m 59s");
  });
});
