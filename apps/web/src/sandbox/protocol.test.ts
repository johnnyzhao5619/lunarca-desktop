/** 沙箱页收到的请求（TDD-17 §6.1）：形状不对的一律丢弃。来源校验在沙箱页脚本里（只认 `window.parent`）。 */

import { describe, expect, it } from "vitest";
import { parseRenderRequest } from "./protocol";

describe("parseRenderRequest", () => {
  it("收：正整数 id、字符串源码与字体、已知主题", () => {
    const request = { id: 3, source: "graph TD", theme: "dark", font: "serif" };
    expect(parseRenderRequest(request)).toEqual(request);
    expect(parseRenderRequest({ ...request, theme: "default" })).toEqual({ ...request, theme: "default" });
  });

  it("丢：缺字段、多字段、类型不对、未知主题", () => {
    const request = { id: 3, source: "graph TD", theme: "dark", font: "serif" };
    for (const data of [
      null,
      "graph TD",
      { ...request, id: -1 },
      { ...request, id: "3" },
      { ...request, source: 1 },
      { ...request, theme: "forest" },
      { ...request, font: null },
      { id: 3, source: "graph TD", theme: "dark" },
      { ...request, code: "alert(1)" },
    ]) {
      expect(parseRenderRequest(data), JSON.stringify(data)).toBeNull();
    }
  });
});
