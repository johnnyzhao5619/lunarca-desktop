import { describe, expect, it } from "vitest";
import {
  EDITOR_TIER_COMPACT_MIN,
  EDITOR_TIER_WIDE_MIN,
  editorTierFor,
} from "./editor-tier";

describe("editorTierFor（编辑器宽度三档）", () => {
  it("阈值含下界：恰好到线就升档", () => {
    expect(editorTierFor(EDITOR_TIER_WIDE_MIN)).toBe("wide");
    expect(editorTierFor(EDITOR_TIER_WIDE_MIN - 1)).toBe("compact");
    expect(editorTierFor(EDITOR_TIER_COMPACT_MIN)).toBe("compact");
    expect(editorTierFor(EDITOR_TIER_COMPACT_MIN - 1)).toBe("narrow");
  });

  it("两个阈值有序（compact 必须比 wide 窄，否则 compact 档永远出不来）", () => {
    expect(EDITOR_TIER_COMPACT_MIN).toBeLessThan(EDITOR_TIER_WIDE_MIN);
  });

  it("0 宽度落最窄档（首帧真实宽度由 layout effect 在绘制前测出，不会以 0 呈现）", () => {
    expect(editorTierFor(0)).toBe("narrow");
  });
});
