// 真实树上的纯视图计算：数一数、临时藏一藏（PRD-06-052 / ADR-086）。
//
// 用例随函数从 `note-tree-partition.test.ts` 搬来；与分区绑定的那些（`partitionTree` /
// `visibleSections` / 知识裁根）随三态分区一并删除——树只有一棵真实的了（ADR-087 ①）。

import { describe, expect, it } from "vitest";
import zhWorkspace from "../../i18n/locales/zh-CN/workspace.json";
import enWorkspace from "../../i18n/locales/en-US/workspace.json";
import type { NoteTreeDto } from "../../ipc/bindings";
import { ancestorDirsOf, containsDraft, countEntries, hideDraftNodes } from "./note-tree-view";

function dir(path: string, children: NoteTreeDto[]): NoteTreeDto {
  return {
    name: path.split("/").pop() ?? path,
    path,
    is_dir: true,
    mtime_ms: 0,
    created_ms: 0,
    draft: false,
    markable_as_draft: false,
    distillable: false,
    is_source_material: false,
    note_type: null,
    can_merge: false,
    cloud_placeholder: false,
    sync_cloud_only: false,
    media: false,
    excluded_by: null,
    hidden: false,
    children,
  };
}

function file(path: string, draft = false): NoteTreeDto {
  return {
    name: path.split("/").pop() ?? path,
    path,
    is_dir: false,
    mtime_ms: 0,
    created_ms: 0,
    draft,
    markable_as_draft: !draft,
    distillable: draft,
    is_source_material: false,
    note_type: null,
    can_merge: true,
    cloud_placeholder: false,
    sync_cloud_only: false,
    media: false,
    excluded_by: null,
    hidden: false,
    children: [],
  };
}

describe("countEntries", () => {
  it("递归数文件，不数目录本身", () => {
    expect(
      countEntries([dir("A", [file("A/1.md"), dir("A/B", [file("A/B/2.md")])]), file("c.md")]),
    ).toBe(3);
  });
});

describe("「隐藏待整理」过滤（PRD-06-052 / ADR-086）", () => {
  const tree = [
    dir("Notes", [
      file("Notes/定稿.md"),
      file("Notes/草稿.md", true),
      dir("Notes/收件箱", [file("Notes/收件箱/碎片.md", true)]),
    ]),
    dir("Recordings", [file("Recordings/r.md")]),
  ];

  it("摘掉记录态笔记行——含嵌在子目录里的那些", () => {
    const kept = hideDraftNodes(tree);
    expect(countEntries(kept)).toBe(2);
    expect(kept[0].children.map((n) => n.path)).toEqual(["Notes/定稿.md", "Notes/收件箱"]);
  });

  it("目录一律保留，连同因此变空的那些——文件树不该对磁盘说谎", () => {
    // 那个文件夹还在盘上，藏掉它会让用户下一步想往里新建时找不到落点。
    const inbox = hideDraftNodes(tree)[0].children.find((n) => n.path === "Notes/收件箱");
    expect(inbox?.is_dir).toBe(true);
    expect(inbox?.children).toEqual([]);
  });

  it("原树不被就地改写（同一份 DTO 还要喂给未过滤的判据）", () => {
    hideDraftNodes(tree);
    expect(countEntries(tree)).toBe(4);
  });

  it("`containsDraft` 递归找到嵌套的记录态；全库没有记录时为假", () => {
    expect(containsDraft(tree)).toBe(true);
    expect(containsDraft(hideDraftNodes(tree))).toBe(false);
    expect(containsDraft([dir("Notes", [file("Notes/a.md")])])).toBe(false);
  });

  it("它只改变**这一次看到什么**：不移动文件、不生成虚拟根", () => {
    // 过滤后的结果仍是同一棵真实拓扑的子集——顶层还是那两个真实目录，顺序不变，
    // 没有任何「知识 / 资料」之类的合成节点混进来（ADR-087 ①）。
    const kept = hideDraftNodes(tree);
    expect(kept.map((node) => node.path)).toEqual(["Notes", "Recordings"]);
  });

  it("开关的两个文案在中英双语都在场（读屏取的是它，随按下态改写）", () => {
    for (const bundle of [zhWorkspace, enWorkspace]) {
      expect(bundle.tree.hideDrafts.length).toBeGreaterThan(0);
      expect(bundle.tree.showDrafts.length).toBeGreaterThan(0);
      expect(bundle.tree.draft.length).toBeGreaterThan(0);
      expect(bundle.tree.draftHint.length).toBeGreaterThan(0);
    }
  });
});

describe("「在全部笔记中显示」的定位（PRD-07-006）", () => {
  it("由外向内给出祖先目录链，不含它自己", () => {
    expect(ancestorDirsOf("Notes/项目/甲/纪要.md")).toEqual([
      "Notes",
      "Notes/项目",
      "Notes/项目/甲",
    ]);
  });

  it("根下的笔记没有祖先——不该为它展开任何东西", () => {
    expect(ancestorDirsOf("纪要.md")).toEqual([]);
  });

  it("多余斜杠不产生空目录名（那会展开一个磁盘上不存在的节点）", () => {
    expect(ancestorDirsOf("/Notes//项目/甲.md")).toEqual(["Notes", "Notes/项目"]);
  });
});
