/// <reference types="vite/client" />
/**
 * 设置页那张观测表里**每一处模板取键**的取值集合 == i18n 键集（TDD-12 §2.1）。
 *
 * **为什么必须有这一条**：设置页那张表的标签是用**模板字面量**取的——
 * `t(`workspace_metrics.surface.${row.surface}`)`。模板取键对 i18n 静态守护完全不可见：
 * 拆一个维度（本次 `knowledge_themes_tags` → `knowledge_topics` / `knowledge_tags`）而
 * 忘了改 locale，typecheck / lint / 全部用例**没有一处会红**，界面上却直接摊开一行裸 key。
 * 反过来漏掉某个维度也一样安静。
 *
 * **四处模板取键必须全覆盖，不能只守 `surface`**：同一张表还按模板取 `action` / `entry` /
 * `status` 三处标签（`LocalWorkspaceMetricsPanel.tsx`）。只守其中一处，另外三处照样能在
 * 全绿的仓库里摊开裸 key——守护的价值恰恰在于它守的是"看不见的那一类"，漏一处等于没守。
 *
 * 判据取自两个**生成物 / 唯一事实源**，不手抄第三份清单：
 * - 值域 = `bindings.ts` 里那条联合类型（tauri-specta 由 Rust 生成，改了 Rust 没重生成
 *   会先被 `git diff --exit-code` 拦下）；
 * - 消费面 = `SURFACE_OF`（`Record<DestinationKey, …>`，少一格当场编译不过）。
 *   两边取值集合必须相等——否则要么库里多出一个界面上不存在的维度，要么反过来。
 */

import { describe, expect, it } from "vitest";
import { SURFACE_OF } from "./use-workspace-metrics";

const BINDINGS = Object.values(
  import.meta.glob("../../ipc/bindings.ts", {
    eager: true,
    import: "default",
    query: "?raw",
  }) as Record<string, string>,
)[0] as unknown as string;

const LOCALES = import.meta.glob("../../i18n/locales/*/settings.json", { eager: true }) as Record<
  string,
  { default: Record<string, unknown> }
>;

/**
 * 设置页表格里按模板取键的四处标签 → 各自的值域生成物。
 *
 * 「本机状态面」那一列取的是 `workspace_metrics.status.${row.surface}`——键是**状态面
 * 的名字**（`WorkspaceMetricStatusSurfaceDto`），与上面那个全局 `surface` 不是同一个值域。
 */
const TEMPLATE_KEYED = {
  surface: "WorkspaceMetricSurfaceDto",
  action: "CreateActionIdDto",
  entry: "WorkspaceMetricEntrySurfaceDto",
  status: "WorkspaceMetricStatusSurfaceDto",
} as const;

type Subtree = keyof typeof TEMPLATE_KEYED;

/** 生成物里那条联合类型的全部字面量取值。文档注释先剥掉，免得注释里的字符串被当成取值。 */
function dtoValues(typeName: string): string[] {
  const declaration = BINDINGS.split(`export type ${typeName} =`)[1]?.split(";")[0];
  expect(declaration, `bindings.ts 里找不到 ${typeName}`).toBeDefined();
  const code = (declaration ?? "").replace(/\/\*[\s\S]*?\*\//g, " ");
  return [...code.matchAll(/"([a-z_]+)"/g)].map((match) => match[1]);
}

function surfaceDtoValues(): string[] {
  return dtoValues(TEMPLATE_KEYED.surface);
}

function labelKeys(locale: string, subtree: Subtree): string[] {
  const bundle = LOCALES[`../../i18n/locales/${locale}/settings.json`]?.default as
    | { workspace_metrics?: Record<string, Record<string, string>> }
    | undefined;
  return Object.keys(bundle?.workspace_metrics?.[subtree] ?? {});
}

function surfaceLabelKeys(locale: string): string[] {
  return labelKeys(locale, "surface");
}

describe("工作区观测维度的标签（模板取键，静态守护看不见）", () => {
  it("扫描面非空", () => {
    expect(surfaceDtoValues().length).toBeGreaterThan(5);
    expect(Object.keys(LOCALES).length).toBeGreaterThan(1);
  });

  it.each(Object.entries(TEMPLATE_KEYED))(
    "`workspace_metrics.%s.*` 的键集与 %s 值域、中英两份 locale 四方相等",
    (subtree, typeName) => {
      const values = [...dtoValues(typeName)].sort();
      expect(values.length, `${typeName} 没解析出取值`).toBeGreaterThan(2);
      for (const locale of ["zh-CN", "en-US"]) {
        expect(
          labelKeys(locale, subtree as Subtree).sort(),
          `${locale} 的 workspace_metrics.${subtree} 与 ${typeName} 值域对不上`,
        ).toEqual(values);
      }
    },
  );

  it("消费面（`SURFACE_OF`）覆盖的正是同一个值域", () => {
    // 它是 `Record<DestinationKey, …>`：少一格编译不过。这一条钉的是另一半——
    // 多出一个没有任何目的地会送出的维度，同样是库里一列永远为 0 的数。
    expect([...new Set(Object.values(SURFACE_OF))].sort()).toEqual([...surfaceDtoValues()].sort());
  });

  it("拆分后的两格各自有名字，合并分区那个键不再存在", () => {
    for (const locale of ["zh-CN", "en-US"]) {
      const keys = surfaceLabelKeys(locale);
      expect(keys).toContain("knowledge_topics");
      expect(keys).toContain("knowledge_tags");
      // ADR-046 无兼容层：拆了就不留旧取值，否则读取侧会同时认识两代维度。
      expect(keys).not.toContain("knowledge_themes_tags");
    }
  });
});
