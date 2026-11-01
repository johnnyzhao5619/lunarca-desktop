/// <reference types="vite/client" />
/**
 * 二级分区**复用既有能力**，不另造第二份（ADR-087 ③/④ / ADR-088 / TDD-07 §4.2c）。
 *
 * 重组之所以危险，正是因为"给新分区做个新页面"比"把已有的那块搬过来"顺手：一旦两处
 * 各画一张图谱、各统计一遍标签、各写一套录音列表，同一件事就有了两个会分叉的实现，而
 * 界面上一切正常——没有任何东西会因此变红。
 *
 * ── 本批（G）对这份守护做的取舍 ──────────────────────────────────────────────
 *
 * **有可观察行为的，一律降级为真实渲染用例**，不再扫源码字符串：
 * - 主题页那一格的行、0 页三态、失败态与过滤无结果 → `features/knowledge/topics-view.test.tsx`；
 * - 标签那一格的来源分组与动作分流 → `./tags-view.test.tsx`；
 * - 深链落在哪一格 → `./workspace-routes.test.ts`（判定已抽成纯函数，落点是返回值）。
 * 原先那 20 条 `toContain("<TagNotesView")` / `toContain('isAt("knowledge", "themes_tags")')`
 * 之类随之删除：它们证明的是"这一行还在"，证明不了"点过去真的看得见东西"，而且每次
 * 改名都要跟着改字面量，最终只会与实现同义反复。
 *
 * **留下来的这几条都没有可观察行为可看**，故仍是结构断言，理由逐条写在用例里：
 * - **"这一格有没有人渲染"**：主区分支是 `WorkspaceApp` 里的内联条件。要观察它得把整个
 *   工作区壳渲染起来，而那需要为几十条命令编造快照——那种用例失败十有八九是替身没配对，
 *   不是导航坏了，于是它很快就会被人调松或跳过。
 * - **"这里**没有**第二份实现"**：反向条款天然不可渲染——你没法渲染一个不存在的图谱。
 * - **"控制器只有一个实例"**：两份状态在首屏上完全一样，只有过滤器与画布分叉之后才现形。
 */

import { describe, expect, it } from "vitest";
import { sectionsOf, sidebarKeyOf, locationFor } from "./workspace-ia";

const APP = Object.values(
  import.meta.glob("../../app/WorkspaceApp.tsx", {
    eager: true,
    import: "default",
    query: "?raw",
  }) as Record<string, string>,
)[0] as unknown as string;

const WORKBENCH = Object.values(
  import.meta.glob("../knowledge/KnowledgeWorkbench.tsx", {
    eager: true,
    import: "default",
    query: "?raw",
  }) as Record<string, string>,
)[0] as unknown as string;

/** 只看会执行的代码：注释里点名某个组件是说明，不是渲染。 */
function code(source: string): string {
  return source
    .replace(/\/\*[\s\S]*?\*\//g, "\n")
    .split("\n")
    .filter((line) => !/^\s*(\/\/|\*)/.test(line))
    .join("\n");
}

const APP_CODE = code(APP);

describe("每一格都有人渲染（缺一格 = 切过去是白屏）", () => {
  it("扫描面非空", () => {
    expect(APP_CODE.length).toBeGreaterThan(10000);
  });

  it("知识库三分区与速记 / 录音 / 转写三个空间，主区都有对应的渲染分支", () => {
    // 分区清单从 `WORKSPACE_IA` 派生而不是手抄：新增一格时这条用例**自动**开始要求
    // 它有渲染分支，而不是等某个人想起来把它加进一份平行清单。
    // 笔记与搜索不在这一圈里：它们共用同一张编辑桌，渲染条件按**编辑面**判
    //（`hasEditorSurface(location)`，PRD-07-058）而不是逐格 `isAt` ——那是裁决，不是遗漏。
    for (const section of sectionsOf("knowledge")) {
      expect(APP_CODE, `knowledge/${section.id} 主区没有渲染分支`).toContain(
        `isAt("knowledge", "${section.id}")`,
      );
    }
    for (const space of ["quick_notes", "recordings", "transcripts"] as const) {
      expect(APP_CODE, `${space} 主区没有渲染分支`).toContain(`isAt("${space}")`);
    }
    expect(APP_CODE).toContain("hasEditorSurface(location)");
  });

  it("侧栏分发键与 IA 同源（缺键当场编译不过，故这里只复核键的形状）", () => {
    // 「缺一格就编译不过」由 `Record<SidebarKey, …>` 保证——那是比任何断言都强的守护，
    // 故本用例不再去源码里数分发表的键，只钉住键本身是从 IA 推出来的那个形状。
    expect(sidebarKeyOf(locationFor("knowledge"))).toBe("knowledge/topics");
    expect(sidebarKeyOf(locationFor("knowledge", "tags"))).toBe("knowledge/tags");
    expect(sidebarKeyOf(locationFor("recordings"))).toBe("recordings");
    expect(sidebarKeyOf(locationFor("quick_notes"))).toBe("quick_notes");
  });
});

describe("反向条款：不另造第二份实现（不可渲染，故只能结构断言）", () => {
  it("工作台只组合既有知识块，不另画一张图谱或另统计一遍标签", () => {
    // 原概览分区已退役（PRD-07-024a 2026-08-31 修订），三块内容迁入主题页空选态的
    // `KnowledgeWorkbench`——反向条款照旧扫组件本体。
    expect(APP_CODE).toContain("<KnowledgeWorkbench ");
    const overview = code(WORKBENCH);
    expect(overview.length).toBeGreaterThan(200);
    for (const forbidden of [
      "<GraphView",
      "<TagNotesView",
      "<TopicsView",
      "vaultGraph(",
      "tagTree(",
    ]) {
      expect(overview, `工作台里出现了第二份实现：${forbidden}`).not.toContain(forbidden);
    }
  });

  it("主题页那一格不内嵌知识地图正文的第二份渲染（ADR-088 ③）", () => {
    // 目录里那条链接是**导航**：它打开那一篇真实 Markdown。真把地图正文画进目录，
    // 用户就有了两份看起来都像地图的东西，而只有一份是文件。
    const topics =
      APP_CODE.split('isAt("knowledge", "topics")')[1]?.split('isAt("knowledge", "tags")')[0] ?? "";
    expect(topics.length).toBeGreaterThan(40);
    for (const forbidden of ["knowledgeMapBody", "MarkdownContent"]) {
      expect(topics, `主题页目录里出现了地图正文的第二份渲染：${forbidden}`).not.toContain(
        forbidden,
      );
    }
  });

  it("图谱主区与侧栏共用同一个 controller 实例（两份状态 = 过滤器与画布各说各的）", () => {
    expect(APP_CODE).toContain("useGraphController(");
    expect(APP_CODE.match(/useGraphController\(/g)?.length).toBe(1);
  });

  it("标签树与主区清单共用同一个 controller 实例（选中与结果只有一份）", () => {
    expect(APP_CODE.match(/useTagBrowser\(/g)?.length).toBe(1);
  });
});

describe("处理队列历史点击按对象分流（真机第十轮）", () => {
  it("产物→知识库闸口、转写→转写空间、录音→录音空间、普通笔记→编辑器，无「否则一律录音」兜底", () => {
    // 此前是「transcript 否则录音」二分（队列只有转写任务时代的遗留）：归入/合成
    // 完成后点历史项会跳去录音界面。分流必须穷尽产出形态。
    const handler = APP_CODE.split("onSelectNote={(noteRel) =>")[1]?.split("onClose=")[0] ?? "";
    expect(handler.length).toBeGreaterThan(40);
    expect(handler).toContain("routeIfKnowledgeProduct(noteRel)");
    expect(handler).toContain('go("transcripts")');
    expect(handler).toContain('go("recordings")');
    expect(handler).toContain("openNoteInPanes(noteRel)");
  });
});

describe("「在笔记中显示」在每个查询投影里都通向同一篇真实 Markdown（PRD-07-006）", () => {
  it("投影的右键菜单共用同一个装配点，落点是真实 path", () => {
    // 产物行换「在知识库中打开」，其余照旧「在笔记库中打开」（ADR-096 ③）：
    // 两个分支都必须在唯一装配点里，不允许某个投影另拼一份菜单。
    expect(APP_CODE).toContain("openInNotes: product ? undefined : () => openNoteInPanes(path)");
    expect(APP_CODE).toContain(
      "openInKnowledge: product ? () => openKnowledgeProduct(path) : undefined",
    );
    // 装配点只有一处：速记 / 标签 / 录音 / 转写四类投影都经它开菜单。
    expect(APP_CODE.match(/openArchivedNoteMenu\(/g)?.length).toBeGreaterThan(4);
  });

  it("「显示」= 打开那一篇 + 落到「笔记」，不是另开一个只读预览", () => {
    const opener = APP_CODE.split("function openNoteInPanes(")[1]?.split("\n  }")[0] ?? "";
    // 打开方式（预览 / 常驻，PRD-07-035）只决定占哪个标签，打开的仍是那一篇真实 Markdown。
    expect(opener).toContain("openNote(state, path, { as: intentAs, ...options })");
    expect(opener).toContain('go("notes")');
  });

  it("主题页目录行的菜单走的是那个唯一构建器，不是一份并行清单", () => {
    // 能力开关（`topicPage: {…}`）是差异的表达方式；另写一套菜单的表现是同一条笔记
    // 在主题页分区与文件树里能做的事悄悄分叉（PRD-07-004 / PRD-07-059a）。
    const menu = APP_CODE.split("function openTopicRowMenu(")[1]?.split("\n  }")[0] ?? "";
    expect(menu.length).toBeGreaterThan(200);
    expect(menu).toContain("noteMenu({");
    expect(menu).toContain("topicPage: {");
  });
});
