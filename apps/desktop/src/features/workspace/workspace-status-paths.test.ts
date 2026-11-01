/// <reference types="vite/client" />
// 三条状态路径各有唯一入口（PRD-07-055/056 / ADR-087 ②）。
//
// | 用户问题 | 唯一入口 | 宿主 |
// | 有什么值得知道 | 状态栏铃铛 / `notifications://` | 应用级通知抽屉 |
// | 现在什么在处理 | 顶栏队列入口（常驻） | 处理队列面板 |
// | 一段时间做了什么、花了多少 | `settings://ai_services/usage` | 设置内用量与执行记录 |
//
// 本文件守的是「这三条路**不经过任何聚合页**就走得通」。「动态」目的地已删，而删除是否
// 安全，取决于这三条路是否各自到位——那正是当初允许删它的前提，删后仍要一直成立。
//
// **这里仍是源码断言，理由是它拦的那两件事没有可观察行为**：「入口悄悄变回条件渲染」
// 与「某处又生成了 `activity://`」都是**全仓范围的缺席/在场**，渲染任何一个组件都看不见。
// 深链落在哪一格这半边已随批次 G 降级为真实行为（`./workspace-routes.test.ts`），本文件
// 因此只留执行侧那一句。

import { describe, expect, it } from "vitest";
import { parseWorkspaceRoute } from "./workspace-routes";

const workspaceApps = import.meta.glob("../../app/WorkspaceApp.tsx", {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

const WORKSPACE_APP = Object.values(workspaceApps)[0] ?? "";

/** 顶栏队列入口（按钮 + 面板锚点）：自带队列订阅，工作区根只摆放它。 */
const queueEntries = import.meta.glob("../processing/ProcessingQueueEntry.tsx", {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;
const QUEUE_ENTRY = Object.values(queueEntries)[0] ?? "";

const queuePanels = import.meta.glob("../processing/ProcessingQueuePanel.tsx", {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

const QUEUE_PANEL = Object.values(queuePanels)[0] ?? "";

const dashboards = import.meta.glob("../dashboard/DashboardView.tsx", {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

const DASHBOARD = Object.values(dashboards)[0] ?? "";

/** 全部产品源码（不含测试与生成物）——`activity://` 生成点清零的扫描面。 */
const ALL_SOURCES = import.meta.glob("../../{app,features,components,hooks,lib}/**/*.{ts,tsx}", {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

/**
 * 只看会执行的代码：注释里解释「这个前缀已退役」是说明，不是生成点。
 *
 * **行注释按整行剔除，不能用 `\/\/.*` 正则**：`activity://` 自己就含 `//`，那条正则会从
 * scheme 中间把代码截断，于是**真正的生成点反而扫不到**——写这条时它当场让两条断言假绿。
 */
function stripComments(source: string): string {
  return source
    .replace(/\/\*[\s\S]*?\*\//g, "\n")
    .split("\n")
    .filter((line) => !/^\s*(\/\/|\*)/.test(line))
    .join("\n");
}

describe("现在什么在处理 → 顶栏队列入口常驻（PRD-07-055）", () => {
  it("扫描面非空", () => {
    expect(WORKSPACE_APP.length).toBeGreaterThan(1000);
    expect(QUEUE_PANEL.length).toBeGreaterThan(500);
  });

  it("入口不再按队列长度条件渲染——空闲时这条分流不得没有入口", () => {
    // 迁移前：`{processingQueue.length > 0 && (<div className="luna-proc-queue-anchor">…)}`。
    // 于是"是不是卡住了"这个问题在**队列为空时**根本没有地方问。
    expect(WORKSPACE_APP).not.toContain("{processingQueue.length > 0 && (");
    // 根无条件摆放入口；入口自己的锚点也不按队列长度条件渲染。
    expect(WORKSPACE_APP).toMatch(/\n\s+<ProcessingQueueEntry\n/);
    expect(QUEUE_ENTRY.length).toBeGreaterThan(500);
    expect(QUEUE_ENTRY).not.toMatch(/items\.length > 0 && \(|queueActive > 0 && \(\s*<div className="luna-proc-queue-anchor"/);
    expect(QUEUE_ENTRY).toContain('<div className="luna-proc-queue-anchor" ref={anchorRef}>');
  });

  it("空队列打开的是面板自带的真实空态，不是一个空壳", () => {
    expect(QUEUE_PANEL).toContain("items.length === 0 ?");
    expect(QUEUE_PANEL).toContain('t("processing.empty")');
  });

  it("`queue://` 只开面板、不切目的地（它是任务态，不是一个内容空间）", () => {
    // 「不切目的地」现在是一个**可直接断言的返回值**：`location: null`（批次 G 把深链
    // 判定抽成了纯函数 `parseWorkspaceRoute`）。此前这里切的是 `WorkspaceApp` 那条
    // if/else-if 链的源码文本——判定一改形状它就失效，而失效的表现是用例还绿着。
    expect(parseWorkspaceRoute("queue://")).toEqual({ kind: "queue", location: null });
    // 执行那一半仍在壳里：意图 → 打开顶栏面板。
    const handler = WORKSPACE_APP.split("const onNavigate = (event: Event) => {")[1] ?? "";
    expect(handler.split('case "queue":')[1]?.split("break;")[0]).toContain("setQueueOpen(true)");
  });

  it("启动概览的队列脉搏改派发 `queue://`，直达同一个面板", () => {
    expect(DASHBOARD).toContain('goTo("queue://")');
  });
});

describe("一段时间做了什么 → 设置内用量与执行记录（PRD-07-056）", () => {
  it("启动概览成效看板给出通往执行记录的下一步", () => {
    expect(DASHBOARD).toContain('goTo("settings://ai_services/usage")');
  });

  it("`settings://ai_services/usage` 是全仓唯一的用量深链", () => {
    const generators = Object.entries(ALL_SOURCES)
      .filter(([path]) => !path.includes(".test."))
      .flatMap(([path, source]) => {
        const code = stripComments(source);
        return [...code.matchAll(/["'`]([a-z_]+:\/\/[^"'`]*usage[^"'`]*)["'`]/g)].map(
          (m) => `${path}: ${m[1]}`,
        );
      });
    expect(
      generators,
      `用量只有一条深链；出现第二条就意味着两处入口迟早说两套话：\n${generators.join("\n")}`,
    ).toEqual(["../dashboard/DashboardView.tsx: settings://ai_services/usage"]);
  });
});

describe("「动态」与 `activity://` 零残留（ADR-087 ② / ADR-046）", () => {
  it("产品代码里既不生成、也不消费这条路由", () => {
    // 生成点在批次 1 归零，消费分支与 Rust 白名单在批次 2 一并删除。**不留静默重定向**：
    // 留一条"自动改送到用量页"的分支，等于让一个已经不存在的心智继续在代码里活着，
    // 而调用方永远不会发现自己用的是旧路由（Rust 侧由
    // `retired_activity_route_is_rejected_not_redirected` 守同一条裁决）。
    const hits = Object.entries(ALL_SOURCES)
      .filter(([path]) => !path.includes(".test."))
      .flatMap(([path, source]) =>
        [...stripComments(source).matchAll(/activity:\/\//g)].map(() => path),
      );
    expect(hits, `「动态」路由必须零残留：\n${hits.join("\n")}`).toEqual([]);
  });

  it("「动态」聚合页本身也没了——目录、组件与它的 i18n 命名空间一并退役", () => {
    const paths = Object.keys(ALL_SOURCES);
    expect(paths.filter((path) => path.includes("/activity/"))).toEqual([]);
    expect(paths.filter((path) => path.includes("ActivityCenter"))).toEqual([]);
  });
});
