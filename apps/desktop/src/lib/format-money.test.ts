import { describe, expect, it } from "vitest";

import { formatMinorUnits } from "./format-money";

describe("formatMinorUnits：服务端价目以货币最小单位下发", () => {
  it("两位小数币种按分换算", () => {
    expect(formatMinorUnits(999, "usd", "en-US")).toBe("$9.99");
    expect(formatMinorUnits(1000, "eur", "en-US")).toBe("€10.00");
  });

  it("零小数币种不除以 100（¥500 不能显示成 ¥5）", () => {
    expect(formatMinorUnits(500, "jpy", "en-US")).toBe("¥500");
  });

  it("币种跟服务端走、语言跟界面走", () => {
    expect(formatMinorUnits(999, "usd", "zh-CN")).toBe("US$9.99");
  });

  it("认不出的币种如实写出数额与代码，而不是抛错", () => {
    expect(formatMinorUnits(999, "not-a-code", "en-US")).toBe("999 NOT-A-CODE");
  });
});
