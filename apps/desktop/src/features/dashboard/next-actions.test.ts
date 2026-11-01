import { describe, expect, it } from "vitest";
import { nextActionTones } from "./next-actions";

describe("「下一步」栈的仲裁", () => {
  it("同屏至多一条 accent", () => {
    for (const setup of [true, false]) {
      for (const knowledge of [true, false]) {
        const tones = nextActionTones({ setup, knowledge });
        const accents = Object.values(tones).filter((tone) => tone === "accent");
        expect(accents, `setup=${setup} knowledge=${knowledge}`).toHaveLength(1);
      }
    }
  });

  it("装机没做完时它就是栈顶——应用本身还没装好，谈别的都早", () => {
    expect(nextActionTones({ setup: true, knowledge: true })).toEqual({
      setup: "accent",
      knowledge: "muted",
      checkup: "muted",
    });
  });

  it("装机齐了让位给知识库脉搏", () => {
    expect(nextActionTones({ setup: false, knowledge: true }).knowledge).toBe("accent");
  });

  it("前两条都没话说时体检才拿主按钮", () => {
    expect(nextActionTones({ setup: false, knowledge: false }).checkup).toBe("accent");
  });
});
