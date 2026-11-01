import { describe, expect, it } from "vitest";
import { formatRelativeTime } from "./format-relative-time";

const NOW = Date.UTC(2026, 6, 25, 12, 0, 0);

describe("formatRelativeTime", () => {
  it("无效时间戳返回空串（调用方据此不渲染，绝不出现 Invalid Date）", () => {
    expect(formatRelativeTime(null, NOW, "en-US")).toBe("");
    expect(formatRelativeTime(undefined, NOW, "en-US")).toBe("");
    expect(formatRelativeTime(0, NOW, "en-US")).toBe("");
    expect(formatRelativeTime(Number.NaN, NOW, "en-US")).toBe("");
  });

  it("按跨度选单位：秒 → 分 → 时 → 天", () => {
    expect(formatRelativeTime(NOW - 30_000, NOW, "en-US")).toBe("30 seconds ago");
    expect(formatRelativeTime(NOW - 5 * 60_000, NOW, "en-US")).toBe("5 minutes ago");
    expect(formatRelativeTime(NOW - 3 * 3_600_000, NOW, "en-US")).toBe("3 hours ago");
    expect(formatRelativeTime(NOW - 24 * 3_600_000, NOW, "en-US")).toBe("yesterday");
  });

  it("超过一周改显绝对日期（「23 天前」不如日期好定位）", () => {
    const old = NOW - 30 * 24 * 3_600_000;
    expect(formatRelativeTime(old, NOW, "en-US")).toBe(new Date(old).toLocaleDateString("en-US"));
  });

  it("未来时间（时钟回拨）夹到 0，不出现「3 分钟后」", () => {
    expect(formatRelativeTime(NOW + 5 * 60_000, NOW, "en-US")).toBe("now");
  });

  it("跟随 locale 本地化，不自建中文文案表", () => {
    expect(formatRelativeTime(NOW - 5 * 60_000, NOW, "zh-CN")).toContain("5");
  });
});
