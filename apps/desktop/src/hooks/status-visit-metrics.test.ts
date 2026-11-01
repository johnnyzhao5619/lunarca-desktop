/// <reference types="vite/client" />
/**
 * 三个状态面各只在**唯一宿主**上报（PRD-07-070 / TDD-12 §2.1）。
 *
 * 这里的源码扫描只守唯一宿主；StrictMode、显式关闭与卸载语义由行为测试负责。两处各报一次
 * 的表现不是报错，是
 * **同一次到访被记成两次**，而这条指标的全部用途就是"用户到底去不去、去了有没有用起来"。
 */

import { describe, expect, it } from "vitest";

const ALL_SOURCES = import.meta.glob("../{app,features,components,hooks,lib}/**/*.{ts,tsx}", {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

/** 只看会执行的代码：注释里写着 `status_visit_finished` 是说明，不是上报点。 */
function stripComments(source: string): string {
  return source
    .replace(/\/\*[\s\S]*?\*\//g, "\n")
    .split("\n")
    .filter((line) => !/^\s*(\/\/|\*)/.test(line))
    .join("\n");
}

const PRODUCT_SOURCES = Object.entries(ALL_SOURCES).filter(([path]) => !path.includes(".test."));

describe("状态面到访的唯一宿主", () => {
  it("扫描面非空", () => {
    expect(PRODUCT_SOURCES.length).toBeGreaterThan(50);
  });

  it("`status_visit_finished` 全仓只有一个生成点（就是那个共享 hook）", () => {
    const generators = PRODUCT_SOURCES.filter(([, source]) =>
      stripComments(source).includes("status_visit_finished"),
    ).map(([path]) => path);
    expect(
      generators,
      `多一个生成点就意味着"算不算操作过"迟早出现两套判据：\n${generators.join("\n")}`,
    ).toEqual(["./use-status-visit-metrics.ts"]);
  });

  it("三个 surface 各只有一个宿主在调 `useStatusVisitMetrics`", () => {
    const hosts = PRODUCT_SOURCES.flatMap(([path, source]) =>
      [...stripComments(source).matchAll(/useStatusVisitMetrics\(\s*"([a-z]+)"/g)].map((match) => [
        match[1],
        path,
      ]),
    );
    // 宿主 = 持有"开着还是关着"那个状态的那一层。面板/视图自己是条件渲染的，关闭即卸载，
    // 报不了到访的终点；三条到达/关闭路径也只在各自的开关上汇合。
    expect(Object.fromEntries(hosts)).toEqual({
      notifications: "../features/notifications/NotificationDrawerHost.tsx",
      queue: "../app/WorkspaceApp.tsx",
      // 执行记录经过三种形态：模型面板的一个页签 → 页上入口 + 一张 sheet（宿主是持有
      // 那个开合的入口组件）→ ADR-126 第 7 条起就是这一页上的一张卡。浮层没了，"打开
      // 这一面"于是等于"来到这一页"，宿主只能是持有 `activePage` 的那一层：卡片本身随
      // 页面条件渲染、切走即卸载，报不了终点（而 `interacted` 仍由摊在卡片内容上的那组
      // 捕获监听决定，判据一字未改）。
      usage: "../features/settings/SettingsView.tsx",
    });
    expect(hosts.length, "同一个 surface 出现两次即为重复上报").toBe(3);
  });

  it("没有宿主把 `open` 写成字面量——那样这个面永远不会有一条到访", () => {
    // 上报只认显式 open → closed。传字面量 `true` 的宿主关闭时走的是卸载，那条分支永远
    // 不执行：代码在、指标恒空，而这正是"做了一半又说做完了"的形状。
    const frozen = PRODUCT_SOURCES.flatMap(([path, source]) =>
      [
        ...stripComments(source).matchAll(
          /useStatusVisitMetrics\(\s*"([a-z]+)"\s*,\s*(true|false)\s*\)/g,
        ),
      ].map((match) => `${path} 的 "${match[1]}" 传了 ${match[2]}`),
    );
    expect(frozen, `第二个实参必须是真实的开关状态：\n${frozen.join("\n")}`).toEqual([]);
  });

  it("上报发生在显式 open → closed 转换，不借 effect cleanup 猜测关闭", () => {
    const hook = ALL_SOURCES["./use-status-visit-metrics.ts"] ?? "";
    const code = stripComments(hook);
    expect(code).toContain("if (open)");
    expect(code).toContain("startedAt.current === null");
    expect(code).toContain("recordWorkspaceMetric");
    expect(code).not.toContain("return () => {");
  });

  it("`interacted` 的判据只认激活手势：Esc / Tab / 方向键都不算把面板用起来了", () => {
    const hook = ALL_SOURCES["./use-status-visit-metrics.ts"] ?? "";
    const code = stripComments(hook);
    expect(code).toContain('event.key === "Enter" || event.key === " "');
    expect(code).not.toContain('"Escape"');
    expect(code).not.toContain('"Tab"');
  });
});

describe("入口到达率的两个面口径一致（N11 / PRD-07-070）", () => {
  it("「去解决」在两个入口面上都记一条 `rejected`", () => {
    // 同一个动作在两个面上口径不同，入口到达率两面就不可比——而"用户更常从哪儿开始"
    // 正是这条指标要回答的问题。
    for (const path of [
      "../features/capture/AddContentPopover.tsx",
      "../features/dashboard/DashboardView.tsx",
    ] as const) {
      const code = stripComments(ALL_SOURCES[path] ?? "");
      expect(code, `${path} 的「去解决」没有记 rejected`).toMatch(
        /recordCreateEntry\([^)]*"rejected"/s,
      );
    }
  });
});
