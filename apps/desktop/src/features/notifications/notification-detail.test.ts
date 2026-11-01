import { describe, expect, it } from "vitest";
import { formatNotificationTime, visibleDetailFields } from "./notification-detail";

describe("formatNotificationTime", () => {
  it("解析有效 Unix 毫秒串", () => {
    expect(formatNotificationTime("0", "zh-CN")).toBe("");
    expect(formatNotificationTime("", "zh-CN")).toBe("");
    expect(formatNotificationTime("abc", "zh-CN")).toBe("");
    expect(formatNotificationTime("1700000000000", "zh-CN")).not.toBe("");
  });
});

describe("visibleDetailFields", () => {
  it("渠道与时间按存在性显隐", () => {
    expect(visibleDetailFields({ channels: ["toast"], created_at_ms: "1700000000000" })).toEqual([
      "channels",
      "time",
    ]);
  });

  it("无渠道时省略渠道行", () => {
    expect(visibleDetailFields({ channels: [], created_at_ms: "1700000000000" })).toEqual(["time"]);
  });

  it("无效时间时省略时间行", () => {
    expect(visibleDetailFields({ channels: ["banner"], created_at_ms: "0" })).toEqual(["channels"]);
  });

  // 类别与级别已经是卡片顶部那两枚徽标——同一事实在同一屏只呈现一次（PRD-07-065）。
  // 这张表此前六格里有两格是徽标的复读、一格恒定值，剩下的真信息被淹在里面。
  it("不复读顶部徽标已经说过的类别与级别", () => {
    const fields = visibleDetailFields({ channels: ["banner"], created_at_ms: "1700000000000" });
    expect(fields).not.toContain("class");
    expect(fields).not.toContain("severity");
  });

  it("两格都缺席时给空表，由组件决定整块不渲染", () => {
    expect(visibleDetailFields({ channels: [], created_at_ms: "0" })).toEqual([]);
  });

  // 打开详情即标已读（邮件式，PRD-15-061），于是「状态」那一格在用户眼里**永远是
  // 「已读」**——一格恒定值不是信息，是噪声。已读态由列表上的未读标记表达，那里它才真的会变。
  it("没有「状态」这一格：它恒为已读，等于什么都没说", () => {
    const fields = visibleDetailFields({ channels: ["banner"], created_at_ms: "1700000000000" });
    expect(fields).not.toContain("status");
  });
});
