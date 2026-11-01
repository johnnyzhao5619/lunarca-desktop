import { describe, expect, it } from "vitest";
import { fuzzyMatch } from "../command/fuzzy";
import { notificationSearchText } from "./notification-search";

const identity = { i18n_key: "notifications:recording_disk_low", target: "settings://notes" };
const labels = {
  title: "磁盘空间不足",
  body: "录音仍在继续，建议尽快结束以确保音频安全。",
  classLabel: "系统健康",
  severityLabel: "警告",
};

describe("notificationSearchText", () => {
  // 这条判据就是这个模块存在的理由：此前搜索基质里**没有一个汉字**。
  it("中文界面下能被界面上读得到的词搜到", () => {
    const text = notificationSearchText(identity, labels);
    for (const query of ["磁盘", "录音", "系统健康", "警告"]) {
      expect(fuzzyMatch(query, text), `搜不到「${query}」`).not.toBeNull();
    }
  });

  it("技术 key 与深链目标仍可搜（排查时按 kind 找那一条）", () => {
    const text = notificationSearchText(identity, labels);
    expect(fuzzyMatch("recording_disk_low", text)).not.toBeNull();
    expect(fuzzyMatch("settings://", text)).not.toBeNull();
  });

  it("没有深链的通知不留空段", () => {
    const text = notificationSearchText({ ...identity, target: null }, labels);
    expect(text.includes("  ")).toBe(false);
    expect(text.endsWith(" ")).toBe(false);
  });

  it("可读文字排在技术串前面", () => {
    const text = notificationSearchText(identity, labels);
    expect(text.indexOf("磁盘空间不足")).toBeLessThan(text.indexOf("recording_disk_low"));
  });
});
