import { describe, expect, it } from "vitest";
import type { NoteTreeDto } from "../../ipc/bindings";
import {
  baseNameOf,
  findTreeNode,
  canDropSelection,
  isValidDropTarget,
  parentPathOf,
  sortTreeChildren,
} from "./NoteTree";

function node(partial: Partial<NoteTreeDto> & { name: string }): NoteTreeDto {
  return {
    path: partial.name,
    is_dir: false,
    mtime_ms: 0,
    created_ms: 0,
    draft: false,
    markable_as_draft: false,
    distillable: false,
    is_source_material: false,
    note_type: null,
    can_merge: true,
    cloud_placeholder: false,
    sync_cloud_only: false,
    media: false,
    excluded_by: null,
    hidden: false,
    children: [],
    ...partial,
  };
}

describe("findTreeNode（顶栏动作取的是节点本身，不是从路径猜属性）", () => {
  const tree = [
    node({
      name: "Notes",
      path: "Notes",
      is_dir: true,
      children: [
        node({ name: "a.md", path: "Notes/a.md" }),
        node({ name: "assets", path: "Notes/assets", is_dir: true, children: [
          node({ name: "图.png", path: "Notes/assets/图.png" }),
        ] }),
      ],
    }),
    node({ name: "Recordings", path: "Recordings", is_dir: true }),
  ];

  it("按路径取到那一行，`is_dir` 直接读树上的事实", () => {
    expect(findTreeNode(tree, "Notes/a.md")?.is_dir).toBe(false);
    expect(findTreeNode(tree, "Notes/assets")?.is_dir).toBe(true);
  });

  it("非 md 附件是**文件**——用 `.md` 后缀猜的那一版会把它当成文件夹", () => {
    const asset = findTreeNode(tree, "Notes/assets/图.png");
    expect(asset?.is_dir).toBe(false);
    expect(asset?.path.endsWith(".md")).toBe(false);
  });

  it("找不到 / 无选中一律 null（调用方据此禁用动作，而不是拿默认值往下走）", () => {
    expect(findTreeNode(tree, null)).toBeNull();
    expect(findTreeNode(tree, "")).toBeNull();
    expect(findTreeNode(tree, "Notes/不存在.md")).toBeNull();
    expect(findTreeNode(tree, "别处/x.md")).toBeNull();
  });
});

describe("sortTreeChildren（PRD-06-006 排序）", () => {
  const children = [
    node({ name: "b.md", mtime_ms: 100, created_ms: 30 }),
    node({ name: "Alpha.md", mtime_ms: 300, created_ms: 10 }),
    node({ name: "目录", is_dir: true }),
    node({ name: "档案", is_dir: true }),
    node({ name: "c.md", mtime_ms: 200, created_ms: 20 }),
  ];

  it("创建时间模式：目录仍按名称，文件按创建时间降序（与修改时间是两个口径）", () => {
    expect(sortTreeChildren(children, "created").map((c) => c.name)).toEqual([
      "档案",
      "目录",
      "b.md",
      "c.md",
      "Alpha.md",
    ]);
  });

  it("名称模式：目录置顶，名称大小写不敏感", () => {
    expect(sortTreeChildren(children, "name").map((c) => c.name)).toEqual([
      "档案",
      "目录",
      "Alpha.md",
      "b.md",
      "c.md",
    ]);
  });

  it("修改时间模式：目录仍按名称，文件按 mtime 降序", () => {
    expect(sortTreeChildren(children, "mtime").map((c) => c.name)).toEqual([
      "档案",
      "目录",
      "Alpha.md",
      "c.md",
      "b.md",
    ]);
  });

  it("不改动入参数组", () => {
    const before = children.map((c) => c.name);
    sortTreeChildren(children, "mtime");
    expect(children.map((c) => c.name)).toEqual(before);
  });
});

describe("路径工具与拖拽目标合法性", () => {
  it("parentPathOf / baseNameOf", () => {
    expect(parentPathOf("a/b/c.md")).toBe("a/b");
    expect(parentPathOf("c.md")).toBe("");
    expect(baseNameOf("a/b/c.md")).toBe("c.md");
    expect(baseNameOf("c.md")).toBe("c.md");
  });

  it("拒绝落回原父目录、自身与自身后代", () => {
    expect(isValidDropTarget("a/b.md", "a")).toBe(false);
    expect(isValidDropTarget("dir", "dir")).toBe(false);
    expect(isValidDropTarget("dir", "dir/sub")).toBe(false);
    expect(isValidDropTarget("", "a")).toBe(false);
  });

  it("允许移动到其他目录与根", () => {
    expect(isValidDropTarget("a/b.md", "c")).toBe(true);
    expect(isValidDropTarget("a/b.md", "")).toBe(true);
    expect(isValidDropTarget("dirA", "dirB")).toBe(true);
  });

  it("多选拖拽：整批里只要有一项真的会被搬走就能放，移进自身则整批不能放", () => {
    // 一批里混着"已经在那儿"的项很正常（跨文件夹多选后拖回其中一个文件夹），它们由
    // Rust 按"已达成"跳过——若判据写成"每一项都合法"，这种再普通不过的选择会整批放不下去。
    expect(canDropSelection(["a/b.md", "c/d.md"], "a")).toBe(true);
    expect(canDropSelection(["a/b.md"], "a")).toBe(false);
    // 移进自身或自身后代：Rust 会整批拒绝，故高亮也不该亮。
    expect(canDropSelection(["dir", "a/b.md"], "dir/sub")).toBe(false);
    expect(canDropSelection(["dir", "a/b.md"], "dir")).toBe(false);
    // 空选择没有可放的东西。
    expect(canDropSelection([], "a")).toBe(false);
  });
});
