import { describe, expect, it } from "vitest";
import { findCueMatches, splitByMatch, stepMatch } from "./transcript-find";

const cues = [
  { text: "Just a little update about the cutover", translation: "关于切换的简短更新" },
  { text: "maps and fixed time emulator", translation: null },
  { text: "Maybe if you can get an update", translation: "如果能拿到更新" },
];

describe("回放视图内查找（PRD-04-028 补记）", () => {
  it("大小写不敏感地找已加载 cue，空查询没有命中", () => {
    expect(findCueMatches(cues, "UPDATE", "source")).toEqual([0, 2]);
    expect(findCueMatches(cues, "  ", "source")).toEqual([]);
  });

  it("只在看得见的那一层里找：仅译文模式不命中原文，仅原文模式不命中译文", () => {
    expect(findCueMatches(cues, "更新", "source")).toEqual([]);
    expect(findCueMatches(cues, "更新", "translation")).toEqual([0, 2]);
    // 仅译文模式下没有译文的行回退显示原文，于是原文可命中。
    expect(findCueMatches(cues, "emulator", "translation")).toEqual([1]);
    expect(findCueMatches(cues, "update", "both")).toEqual([0, 2]);
  });

  it("按命中切片，命中片段保留原文大小写", () => {
    expect(splitByMatch("Update the update", "update")).toEqual([
      { text: "Update", hit: true },
      { text: " the ", hit: false },
      { text: "update", hit: true },
    ]);
    expect(splitByMatch("nothing", "")).toEqual([{ text: "nothing", hit: false }]);
  });

  it("命中之间环绕步进；没有命中恒为 -1", () => {
    expect(stepMatch(-1, 3, 1)).toBe(0);
    expect(stepMatch(-1, 3, -1)).toBe(2);
    expect(stepMatch(2, 3, 1)).toBe(0);
    expect(stepMatch(0, 3, -1)).toBe(2);
    expect(stepMatch(0, 0, 1)).toBe(-1);
  });
});
