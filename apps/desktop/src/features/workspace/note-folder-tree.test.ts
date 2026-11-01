import { describe, expect, it } from "vitest";
import { canDropInto, listHasNothing } from "./note-folder-tree";
import { allInDir, folderChoices, groupByFolder } from "./note-folder-tree";

const item = (path: string) => ({ path });

describe("功能视图列表的文件夹分组（PRD-07-062）", () => {
  it("按归档目录裁掉前缀——否则整棵树只有一个根节点，每次都要多展开一层", () => {
    const tree = groupByFolder(
      [item("录音/a.md"), item("录音/项目A/b.md"), item("录音/项目A/子/c.md")],
      "录音",
    );
    expect(tree.items.map((i) => i.path)).toEqual(["录音/a.md"]);
    expect(tree.folders.map((f) => f.name)).toEqual(["项目A"]);
    expect(tree.folders[0].path).toBe("录音/项目A");
    expect(tree.folders[0].items.map((i) => i.path)).toEqual(["录音/项目A/b.md"]);
    expect(tree.folders[0].folders[0].name).toBe("子");
  });

  it("折叠时显示的是含子孙的总数", () => {
    const tree = groupByFolder(
      [item("录音/项目A/b.md"), item("录音/项目A/子/c.md"), item("录音/项目A/子/d.md")],
      "录音",
    );
    expect(tree.folders[0].total).toBe(3);
    expect(tree.folders[0].folders[0].total).toBe(2);
  });

  it("不在归档目录下的笔记不隐藏也不假装，按真实路径分组", () => {
    // 用户可能手动把一篇录音笔记挪去别处——它仍属于这个类型，清单里不该凭空消失。
    const tree = groupByFolder([item("别处/x.md"), item("录音/a.md")], "录音");
    expect(tree.folders.map((f) => f.path)).toEqual(["别处"]);
    expect(tree.items.map((i) => i.path)).toEqual(["录音/a.md"]);
  });

  it("条目顺序原样保留——排序方式是用户选的，分组不该悄悄改掉它", () => {
    const tree = groupByFolder(
      [item("录音/项目A/z.md"), item("录音/项目A/a.md")],
      "录音",
    );
    expect(tree.folders[0].items.map((i) => i.path)).toEqual([
      "录音/项目A/z.md",
      "录音/项目A/a.md",
    ]);
  });

  it("文件夹按名称升序，逐层生效", () => {
    const tree = groupByFolder(
      [item("录音/B/1.md"), item("录音/A/1.md"), item("录音/A/z/1.md"), item("录音/A/b/1.md")],
      "录音",
    );
    expect(tree.folders.map((f) => f.name)).toEqual(["A", "B"]);
    expect(tree.folders[0].folders.map((f) => f.name)).toEqual(["b", "z"]);
  });

  it("归档目录带首尾斜杠也照常归一", () => {
    const tree = groupByFolder([item("录音/项目A/b.md")], "/录音/");
    expect(tree.folders[0].path).toBe("录音/项目A");
  });

  it("空归档目录按 Vault 根分组", () => {
    const tree = groupByFolder([item("a.md"), item("x/b.md")], "");
    expect(tree.items.map((i) => i.path)).toEqual(["a.md"]);
    expect(tree.folders[0].path).toBe("x");
  });
});

describe("移动目标", () => {
  it("候选包含根目录与全部子文件夹，带缩进深度", () => {
    const tree = groupByFolder([item("录音/A/子/1.md"), item("录音/B/1.md")], "录音");
    expect(folderChoices(tree, "录音")).toEqual([
      { path: "录音", depth: 0, name: "" },
      { path: "录音/A", depth: 1, name: "A" },
      { path: "录音/A/子", depth: 2, name: "子" },
      { path: "录音/B", depth: 1, name: "B" },
    ]);
  });

  it("不在目标目录里的对象可以移过去（该项可点）", () => {
    expect(allInDir(["录音/a.md"], "录音/项目A")).toBe(false);
    expect(allInDir(["录音/项目A/a.md"], "")).toBe(false);
    // 一批里只要还有一个不在那儿，这一项就该是可点的。
    expect(allInDir(["录音/a.md", "录音/项目A/b.md"], "录音")).toBe(false);
  });

  it("整批已经在目标目录里就置灰（而不是发一条什么都不改的命令）", () => {
    expect(allInDir(["录音/a.md"], "录音")).toBe(true);
    expect(allInDir(["录音/a.md"], "/录音/")).toBe(true);
    expect(allInDir(["录音/a.md", "录音/b.md"], "录音")).toBe(true);
  });
});

describe("真实目录（含空目录）并进树里（PRD-07-062，2026-09-08）", () => {
  const items = [{ path: "Recordings/A/一.md" }];

  it("没有条目的目录也生成节点，且计数为 0", () => {
    const tree = groupByFolder(items, "Recordings", ["Recordings/A", "Recordings/空文件夹"]);
    const names = tree.folders.map((folder) => folder.name).sort();
    expect(names).toEqual(["A", "空文件夹"]);
    const empty = tree.folders.find((folder) => folder.name === "空文件夹");
    expect(empty?.total).toBe(0);
    expect(empty?.items).toEqual([]);
  });

  it("有条目的目录计数不受影响（目录表不该重复计数）", () => {
    const tree = groupByFolder(items, "Recordings", ["Recordings/A"]);
    expect(tree.folders.find((folder) => folder.name === "A")?.total).toBe(1);
  });

  it("空目录也进「移动到 ▸」候选——「先建文件夹再移进去」这条路要走得通", () => {
    const tree = groupByFolder(items, "Recordings", ["Recordings/空文件夹"]);
    const paths = folderChoices(tree, "Recordings").map((choice) => choice.path);
    expect(paths).toContain("Recordings/空文件夹");
  });

  it("不给目录表时行为与从前完全一致（调用方未接线不该改变现状）", () => {
    expect(groupByFolder(items, "Recordings")).toEqual(groupByFolder(items, "Recordings", []));
  });

  it("嵌套空目录逐层建节点", () => {
    const tree = groupByFolder([], "Recordings", ["Recordings/外", "Recordings/外/内"]);
    expect(tree.folders).toHaveLength(1);
    expect(tree.folders[0]?.folders.map((folder) => folder.name)).toEqual(["内"]);
  });
});

describe("空态判据（PRD-07-062，2026-09-08）", () => {
  it("零条目但有目录 → 不是空态，树要渲染出来（否则新建的文件夹永远看不见）", () => {
    expect(listHasNothing({ itemCount: 0, dirCount: 1, creating: false })).toBe(false);
  });

  it("零条目零目录但正在输入新建行 → 不是空态（输入行长在树里）", () => {
    expect(listHasNothing({ itemCount: 0, dirCount: 0, creating: true })).toBe(false);
  });

  it("真的什么都没有 → 空态", () => {
    expect(listHasNothing({ itemCount: 0, dirCount: 0, creating: false })).toBe(true);
  });

  it("筛选中零命中 → 空态，即使有目录（树被平铺，空目录本就不显示）", () => {
    expect(listHasNothing({ itemCount: 0, dirCount: 3, creating: false, filtering: true })).toBe(true);
  });
});

describe("拖进分节的判据（PRD-07-062，2026-09-08）", () => {
  it("有一条不在目标目录里就能放", () => {
    expect(canDropInto(["Recordings/a.md", "Recordings/A/b.md"], "Recordings/A")).toBe(true);
  });

  it("整批都已在目标目录里 → 不能放（高亮不亮、松手不发命令）", () => {
    expect(canDropInto(["Recordings/A/b.md"], "Recordings/A")).toBe(false);
  });

  it("空批不能放", () => {
    expect(canDropInto([], "Recordings/A")).toBe(false);
  });
});
