import { describe, expect, it } from "vitest";
import { isNearBottom } from "./use-stick-to-bottom";

describe("isNearBottom", () => {
  it("treats exact bottom as near", () => {
    expect(isNearBottom(900, 100, 1000)).toBe(true);
  });

  it("treats within threshold as near", () => {
    expect(isNearBottom(860, 100, 1000)).toBe(true);
  });

  it("treats scrolled-up positions as not near", () => {
    expect(isNearBottom(500, 100, 1000)).toBe(false);
  });

  it("treats short non-scrollable content as near", () => {
    expect(isNearBottom(0, 400, 300)).toBe(true);
  });
});
