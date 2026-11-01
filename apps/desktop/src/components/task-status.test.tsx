import { describe, expect, it } from "vitest";
import { taskStatusTone, type TaskStatusKind } from "./TaskStatus";

describe("taskStatusTone", () => {
  it.each([
    ["queued", "ai"],
    ["processing", "ai"],
    ["needs_action", "attention"],
    ["completed", "success"],
    ["failed", "error"],
    ["degraded", "warning"],
    ["cancelled", "muted"],
  ] as const)("maps %s to %s", (state, tone) => {
    expect(taskStatusTone(state as TaskStatusKind)).toBe(tone);
  });
});
