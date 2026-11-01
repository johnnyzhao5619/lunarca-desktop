import { describe, expect, it } from "vitest";
import { semanticActionSlot } from "./SemanticIndexPanel";

describe("semanticActionSlot（PRD-09-072：渐进披露只能少说，不能改说法）", () => {
  it("高级模式有待索引时给重建入口", () => {
    expect(semanticActionSlot({ advancedMode: true, pending: 36 })).toBe("rebuild");
  });

  it("标准模式隐藏重建细节，但绝不因此宣称「都已是最新」", () => {
    // 反向验证：把两个判据并回一个三元表达式，这一条会变成 "all-fresh" 而变红。
    expect(semanticActionSlot({ advancedMode: false, pending: 36 })).toBe("none");
  });

  it("真的没有待索引时两种模式都说「已是最新」", () => {
    expect(semanticActionSlot({ advancedMode: false, pending: 0 })).toBe("all-fresh");
    expect(semanticActionSlot({ advancedMode: true, pending: 0 })).toBe("all-fresh");
  });
});
