import { describe, expect, it } from "vitest";
import { recallReadingScroll, rememberReadingScroll } from "./reading-memory";

describe("阅读位置记忆", () => {
  it("按路径记住最近一次位置，互不串页", () => {
    rememberReadingScroll("Knowledge/甲.md", 120);
    rememberReadingScroll("Knowledge/乙.md", 40);
    rememberReadingScroll("Knowledge/甲.md", 360);
    expect(recallReadingScroll("Knowledge/甲.md")).toBe(360);
    expect(recallReadingScroll("Knowledge/乙.md")).toBe(40);
    expect(recallReadingScroll("Knowledge/丙.md")).toBeUndefined();
  });

  it("超过上限时忘掉最久没碰的，不无界增长", () => {
    rememberReadingScroll("Knowledge/最早.md", 1);
    for (let index = 0; index < 60; index += 1) {
      rememberReadingScroll(`Knowledge/页${index}.md`, index);
    }
    expect(recallReadingScroll("Knowledge/最早.md")).toBeUndefined();
    expect(recallReadingScroll("Knowledge/页59.md")).toBe(59);
  });
});
