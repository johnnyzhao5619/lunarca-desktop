/// <reference types="vite/client" />
// 「笔记 → 全部笔记」只渲染**一棵真实的 Vault 树**（ADR-087 ① / PRD-06-048 ①/④/⑥）。
//
// 这一批拆掉的是一套**第二归属事实源**：文件树此前按「全部 / 知识 / 资料」切成三段，
// 同一个目录能出现在两个区段里、切一次段控就像换了一个库——而磁盘上什么都没变。身份
// 改由**对象自己**表达（行内徽标，判定全在 Rust）。
//
// 仓库没有 jsdom（见 `draft-distill-entry.test.ts` 先例），故按源码断言；这里要拦的恰好
// 都是源码层面的复发：某个分区函数被重新引入、某条判据又改回"拿目录比路径猜"。

import { describe, expect, it } from "vitest";

const sources = import.meta.glob("./{NoteTree.tsx,note-tree-view.ts}", {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

const TREE = sources["./NoteTree.tsx"] ?? "";
const VIEW = sources["./note-tree-view.ts"] ?? "";

/** 只看会执行的代码：注释里解释「它已退役」是说明，不是残留。 */
function code(source: string): string {
  return source
    .replace(/\/\*[\s\S]*?\*\//g, "\n")
    .split("\n")
    .filter((line) => !/^\s*(\/\/|\*)/.test(line))
    .join("\n");
}

const ALL_SOURCES = import.meta.glob("../../{app,features,components,hooks,lib}/**/*.{ts,tsx}", {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

describe("真实 Vault 树（ADR-087 ①）", () => {
  it("扫描面非空", () => {
    expect(TREE.length).toBeGreaterThan(1000);
    expect(VIEW.length).toBeGreaterThan(200);
  });

  it("三态分区连同它的全部零件零残留", () => {
    const everywhere = Object.entries(ALL_SOURCES).filter(([path]) => !path.includes(".test."));
    for (const retired of [
      "partitionTree",
      "TREE_MODES",
      "DEFAULT_TREE_MODE",
      "visibleSections",
      "labelsSections",
      "knowledgeRenderGroups",
      "knowledgeRootTarget",
      "clearsSelectionOnFocus",
      "presetTreeMode",
      "note-tree-partition",
      "useMaterialDirs",
      "useQuickNoteDir",
      "useKnowledgeDir",
    ]) {
      const hits = everywhere
        .filter(([, source]) => code(source).includes(retired))
        .map(([path]) => path);
      expect(hits, `三态分区的 ${retired} 又回来了：\n${hits.join("\n")}`).toEqual([]);
    }
  });

  it("树渲染的是 Rust 下发的顶层节点本身，落点是真实 Vault 根", () => {
    // 不裁 `notes_dir` 包裹层、不生成虚拟根：新建 / 拖回根部 / 背景右键都写到真实根。
    expect(code(TREE)).toContain('const rootTarget = "";');
    expect(code(TREE)).toContain('renderTreeItems(rendered, "")');
  });

  it("「隐藏待整理」仍在，且只改变这一次看到什么", () => {
    // PRD-06-052：视图过滤留着，但它不持久化、不移动文件、不生成虚拟根。
    expect(code(TREE)).toContain("hideDraftNodes(tree?.children ?? [])");
    expect(code(TREE)).toContain("useState(false)");
    expect(code(VIEW)).toContain("export function hideDraftNodes");
    // 开关的渲染条件恒按**未过滤**的树算，否则开一次就再也关不回来。
    expect(code(TREE)).toContain("containsDraft(tree?.children ?? [])");
  });

  it("菜单能力来自 Rust 已判定的事实，不再按归档目录猜", () => {
    // 目录被改名、笔记被移出归档目录——拿目录比路径会给出一个点下去必被拒的动作
    // （PRD-06-048 ⑥ / ADR-003）。
    expect(code(TREE)).not.toContain("archiveDirs");
    // 菜单直接吃 `NoteTreeDto`：中间那层手抄字段的内联结构一旦回来，Rust 每多下发
    // 一位能力就要记得在它里面再补一次，忘了不会有任何东西变红——菜单只是悄悄少一项。
    expect(code(TREE)).toContain("function openNodeMenu(event: ReactMouseEvent, node: NoteTreeDto)");
    expect(code(TREE)).toContain("openNodeMenu(event, item)");
  });

  it("身份长在行上：资料与待整理各有一枚徽标，判定全来自 Rust", () => {
    // 「资料」此前是靠**树被切成知识段 / 资料段**表达的，而那是一套第二归属事实源。
    // 段没了，标记就得回到行上——否则真实树里资料与知识长得一模一样，而 `is_source_material`
    // 白下发一趟（PRD-06-048 ②/⑥）。
    expect(code(TREE)).toContain("item.is_source_material &&");
    expect(code(TREE)).toContain('className="luna-source-badge"');
    expect(code(TREE)).toContain("item.draft &&");
    expect(code(TREE)).toContain('className="luna-draft-badge"');
    // 文案与页头那枚徽标同源，不在树里另写一句。
    expect(code(TREE)).toContain("t(noteTypeLabelKey(item.note_type))");
    expect(code(TREE)).toContain('t("panel.noteInfoSourceHint")');
  });

  it("「隐藏待整理」把这一屏滤空时，空态说的是过滤而不是「库是空的」", () => {
    // 用户明明有笔记，却被告知「暂无 Markdown 笔记」并递上「新建笔记」——话是假的，
    // 给的出路也是错的（他要的是把过滤关掉）。四类空态见 PRD-07 §3.4。
    expect(code(TREE)).toContain(") : hideDrafts ? (");
    expect(code(TREE)).toContain('t("tree.emptyFiltered")');
    expect(code(TREE)).toContain("onClick={() => setHideDrafts(false)}");
  });

  it("文件夹右键不再按目录分流「多条合成一篇」与「逐篇各出一篇」", () => {
    // 速记会被移动、普通笔记也会被标记为待整理——`QuickNotes/` 从来不等于成熟度。
    expect(code(TREE)).toContain("distillFolderAction(node.path, t)");
    expect(code(TREE)).not.toContain("quickNoteFolderAction");
  });

  it("选中路径变化时展开它的整条祖先链——否则「在全部笔记中显示」什么都不发生", () => {
    // 那一行根本没被渲染（子节点只在祖先展开时才画），只把它标成选中等于没反应。
    expect(code(TREE)).toContain("ancestorDirsOf(selectedPath)");
    // 依赖只有选中路径：跟着别的东西重跑，用户收起某一层会被下一次渲染立刻撤销。
    expect(code(TREE)).toContain("}, [selectedPath]);");
  });

  it("前端不再有任何「拿路径比配置目录」的判据（ADR-003）", () => {
    // 三态分区退役后，这一类判断**一条都不剩**：落点取节点自身的真实 `path`，
    // 各项能力由 Rust 下发的节点字段回答。于是那套路径归一（`normalizeVaultPath`
    // / `sameVaultPath` / `isUnderVaultDir`）连同它的模块一并删除——留着就是一份没有
    // 消费者的第二判定源，下一个人会照着它再写一条前端判据（ADR-046 同精神）。
    const everywhere = Object.entries(ALL_SOURCES).filter(([path]) => !path.includes(".test."));
    for (const retired of ["normalizeVaultPath", "sameVaultPath", "isUnderVaultDir", "vault-path"]) {
      const hits = everywhere
        .filter(([, source]) => code(source).includes(retired))
        .map(([path]) => path);
      expect(hits, `前端又出现了路径归一判据：${retired}\n${hits.join("\n")}`).toEqual([]);
    }
  });
});

describe("「多条待整理 → 一篇知识笔记」迁到查询投影（PRD-06-047 / TDD-07 §2.1）", () => {
  const inbox = Object.values(
    import.meta.glob("../capture/DraftInbox.tsx", {
      eager: true,
      import: "default",
      query: "?raw",
    }) as Record<string, string>,
  )[0] as unknown as string;

  const host = Object.values(
    import.meta.glob("../distill/use-distill-batch.ts", {
      eager: true,
      import: "default",
      query: "?raw",
    }) as Record<string, string>,
  )[0] as unknown as string;
  const app = Object.values(
    import.meta.glob("../../app/WorkspaceApp.tsx", {
      eager: true,
      import: "default",
      query: "?raw",
    }) as Record<string, string>,
  )[0] as unknown as string;
  const chatList = Object.values(
    import.meta.glob("../chat/ChatHistoryList.tsx", {
      eager: true,
      import: "default",
      query: "?raw",
    }) as Record<string, string>,
  )[0] as unknown as string;
  const chatView = Object.values(
    import.meta.glob("../chat/ChatView.tsx", {
      eager: true,
      import: "default",
      query: "?raw",
    }) as Record<string, string>,
  )[0] as unknown as string;
  const selectionHook = Object.values(
    import.meta.glob("./use-note-selection.ts", {
      eager: true,
      import: "default",
      query: "?raw",
    }) as Record<string, string>,
  )[0] as unknown as string;

  it("扫描面非空", () => {
    expect(chatList.length).toBeGreaterThan(500);
    expect(chatView.length).toBeGreaterThan(500);
    expect(inbox.length).toBeGreaterThan(500);
    expect(host.length).toBeGreaterThan(500);
    expect(app.length).toBeGreaterThan(500);
    expect(selectionHook.length).toBeGreaterThan(500);
  });

  it("入口在速记查询投影里，作用于选中的那一批真实 paths", () => {
    // 2026-09-09 用户裁决：速记不再自带一套行首勾选框与「已选 N 条」工具条——同一件事
    // 在应用里于是有两种做法与两种外观。多选统一为共享原语（Cmd/Ctrl 加选、Shift 连选），
    // 「合并为一篇」随之回到右键菜单，与录音 / 转写 / 文件树同一条路。
    expect(code(inbox)).not.toContain('type="checkbox"');
    expect(code(inbox)).not.toContain("requestMergeNotes(");
    expect(code(inbox)).toContain("selection.targetsFor(note.path)");
    expect(code(inbox)).toContain("selection.contextClick(note.path)");
  });

  it("资格与限额全部来自 Rust，前端不写第二份判据", () => {
    // 三道判据：逐条 `can_merge` 与条数区间 `merge_min` / `merge_max`，全部由 Rust 下发。
    // 抄一个 `>= 2` 在前端就是同一条规则的第二处定义，改了一边不会有任何东西变红。
    expect(code(app)).toContain("targets.length >= draftMeta.merge_min");
    expect(code(app)).toContain("targets.length <= draftMeta.merge_max");
    expect(code(app)).toContain("item.path === path && item.can_merge");
    // 区间是下发的数，不是这里的字面量。
    expect(code(app)).not.toMatch(/targets\.length\s*[<>]=?\s*[1-9]/u);
  });

  it("AI 对话不另拼一份菜单：条目集与限额同样来自那个唯一构建器", () => {
    // 2026-09-09：`ChatHistoryList` 此前自己拼了一份行右键，其中五项与共享构建器逐字
    // 相同——会话归档就是库里的一篇 Markdown，同一条笔记在这儿与在文件树里能做的事
    // 不该分叉。菜单改由两个宿主用 `useNoteMenu` 装配，视图只把落点与那一批交出去。
    expect(code(chatList)).not.toContain("buildMenu(");
    expect(code(chatList)).not.toContain("requestDistillNotes(");
    expect(code(chatList)).toContain("selection.targetsFor(entry.path)");
    expect(code(chatList)).toContain("onItemContextMenu(event, entry, targets)");
    // 两个宿主（主侧栏 / 面板弹层）都走同一个构建器，且条数区间取自 Rust 下发的那一对。
    for (const host of [app, chatView]) {
      expect(code(host)).toContain("mergeBounds.min");
      expect(code(host)).toContain("mergeBounds.max");
      expect(code(host)).not.toMatch(/targets\.length\s*[<>]=?\s*[1-9]/u);
    }
  });

  it("执行仍走唯一内核，且命令按 paths 而不是文件夹范围", () => {
    expect(code(host)).toContain("commands.mergeNotes(paths)");
    expect(code(host)).not.toContain("distillQuickNotes");
  });

  it("按列表顺序提交，不按点选的先后", () => {
    // Rust 保持交来的顺序（去重后），而产物的正文与来源区都按这个顺序排。交点选顺序的话，
    // 同样几条笔记先点哪一条都会喂给模型一份不同的输入——同一次选择产出不同的结果，而
    // 用户看不见这个变量。顺序现由共享选择原语保证：`ordered` 是「按所见顺序」。
    expect(code(selectionHook)).toContain("orderedSelection");
    expect(code(inbox)).toContain("selection.ordered");
  });

  it("全仓不再有按物理速记目录决定动作的地方", () => {
    const hits = Object.entries(ALL_SOURCES)
      .filter(([path]) => !path.includes(".test."))
      .filter(([, source]) => code(source).includes("QUICK_NOTE_MERGE_EVENT"))
      .map(([path]) => path);
    expect(hits).toEqual([]);
  });
});
