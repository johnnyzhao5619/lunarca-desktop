import { describe, expect, it } from "vitest";
import type { TagSummaryDto } from "../../ipc/bindings";
import { ancestorPaths, buildTagTree } from "./tag-tree";

/** 构造一条后端摘要（默认直接数与整支去重数相同）。 */
function tag(name: string, count: number, aiOnly = false, directCount = count): TagSummaryDto {
  return { tag: name, direct_count: directCount, note_count: count, ai_only: aiOnly };
}

describe("buildTagTree（PRD-06-033 嵌套标签树）", () => {
  it("只连接后端摘要，父级展示 Rust 给出的去重数", () => {
    const tree = buildTagTree([
      tag("工作", 4, false, 2),
      tag("工作/评审", 3),
      tag("生活", 1),
    ]);
    expect(tree.map((node) => node.path)).toEqual(["工作", "生活"]);
    const work = tree[0];
    expect(work.directCount).toBe(2);
    expect(work.noteCount).toBe(4);
    expect(work.children[0]).toMatchObject({
      path: "工作/评审",
      directCount: 3,
      noteCount: 3,
    });
  });

  it("连接 Rust 已补齐的中间层（directCount 为 0）", () => {
    const tree = buildTagTree([
      tag("a", 4, false, 0),
      tag("a/b", 4, false, 0),
      tag("a/b/c", 4),
    ]);
    expect(tree[0]).toMatchObject({ path: "a", directCount: 0, noteCount: 4 });
    expect(tree[0].children[0]).toMatchObject({ path: "a/b", directCount: 0, noteCount: 4 });
    expect(tree[0].children[0].children[0]).toMatchObject({ path: "a/b/c", directCount: 4 });
  });

  it("排序按整支去重数降序，同数按路径升序", () => {
    const tree = buildTagTree([
      tag("b", 1),
      tag("a", 1),
      tag("hot", 9, false, 0),
      tag("hot/x", 9),
    ]);
    expect(tree.map((node) => node.path)).toEqual(["hot", "a", "b"]);
  });

  it("直接采用 Rust 给出的整支 AI 来源事实", () => {
    const tree = buildTagTree([
      tag("项目", 4, false, 0),
      tag("项目/luna", 2),
      tag("项目/自动", 3, true),
    ]);
    expect(tree[0]).toMatchObject({ path: "项目", aiOnly: false });
    expect(tree[0].children.find((node) => node.path === "项目/自动")).toMatchObject({
      aiOnly: true,
    });

    const pure = buildTagTree([tag("机器", 1, true, 0), tag("机器/生成", 1, true)]);
    expect(pure[0]).toMatchObject({ path: "机器", aiOnly: true });
    expect(pure[0].children[0]).toMatchObject({ path: "机器/生成", aiOnly: true });
  });
});

describe("ancestorPaths", () => {
  it("返回逐级祖先，不含自身", () => {
    expect(ancestorPaths("a/b/c")).toEqual(["a", "a/b"]);
    expect(ancestorPaths("单层")).toEqual([]);
  });
});
