import { describe, expect, it } from "vitest";
import type { TagSummaryDto } from "../../ipc/bindings";
import { resolveTagSelection } from "./use-tag-browser";

function summary(tag: string, count: number): TagSummaryDto {
  return { tag, direct_count: count, note_count: count, ai_only: false };
}

describe("resolveTagSelection", () => {
  const tags = [summary("项目", 8), summary("交易", 5)];

  it("首次进入选中后端排序的第一项，主区不落入无意义空白", () => {
    expect(resolveTagSelection(tags, null)).toBe("项目");
  });

  it("刷新保留现有选择，并把大小写归并到后端展示原形", () => {
    expect(resolveTagSelection(tags, "交易")).toBe("交易");
    expect(resolveTagSelection([summary("Data_Security", 3)], "data_security")).toBe(
      "Data_Security",
    );
  });

  it("标签被删除后落到新的第一项；空库没有伪选择", () => {
    expect(resolveTagSelection(tags, "已删除")).toBe("项目");
    expect(resolveTagSelection([], "已删除")).toBeNull();
  });
});
