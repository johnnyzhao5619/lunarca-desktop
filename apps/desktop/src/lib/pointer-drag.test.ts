import { describe, expect, it } from "vitest";
import { DROP_ID_ATTR } from "./pointer-drag";

describe("落点约定", () => {
  it("落点属性名是稳定契约（两个消费方靠它对齐，改名即断）", () => {
    expect(DROP_ID_ATTR).toBe("data-drop-id");
  });
});
