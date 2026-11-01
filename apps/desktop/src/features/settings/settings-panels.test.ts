/// <reference types="vite/client" />
/**
 * 页专属面板的外壳守护（PRD-09 §5 / TDD-15 §3.3）。
 *
 * **它挡住的是"再造一套外壳"**：设置页里"不是一行设置"的内容由页专属面板承载，
 * 而这些面板此前各写各的外壳——五个面板、四种"这是一块内容"的说法，分散在四个 CSS
 * 文件里（两个是分隔线、两个是描边卡片，间距与字号各不相同）。没有任何门禁会因此变红：
 * typecheck 只管类型、lint 只管 import 与 Token、样式守护只管颜色与逻辑属性。
 *
 * 判据刻意只有一条、且是结构性的：**`SettingsView` 注册的每一个页面板，根元素必须
 * 带 `.luna-settings-panel`**。间距、分隔线、标题排版由那一个类单点决定，于是"下一个
 * 面板长得不一样"这件事在结构上不可能发生，而不是靠约定。
 *
 * 刻意**不**断言"面板要不要有自己的标题"：那条判据（与设置行同页才给标题）是产品判断，
 * 依赖页文案与面板文案是否重复，机器读不出"重复"；它写在 TDD-15，由评审承担。
 */

import { describe, expect, it } from "vitest";
import { SETTINGS_PAGES } from "./settings-information-architecture";

const SOURCES = import.meta.glob("./*.tsx", {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

const VIEW = SOURCES["./SettingsView.tsx"];

/**
 * `SettingsView` 里的页面板注册面：`PAGE_PANELS` 对象字面量，每个条目形如
 * `<页>: { lead/trail/appendRows: (…) => <XxxPanel …> }`。
 *
 * 只认 `…Panel` 结尾的组件：同一条目里还会登记算力/连接**卡片**（`ComputeCard`，它是
 * 首屏答案卡不是面板）与单条设置行（`OnboardingRerunRow`，外壳归 `<ul>`）。命名是这几者
 * 在结构上唯一的区别，故把它当判据用——并由下面的数量断言兜住"改了命名而守护静默失效"。
 *
 * 解析按**条目块**而不是按行：一个条目跨多行（lead + trail），面板组件可能折到下一行。
 * 先切出 `PAGE_PANELS = { … }` 的主体，再按两格缩进的 `<页>: {` 切条目，条目内所有
 * `<XxxPanel` 都归这一页。`scripts/check-settings-registry.py` §9 按同一形状解析。
 */
const REGISTRY_BLOCK = /const PAGE_PANELS[^=]*=\s*\{\n([\s\S]*?)\n\};/;
const ENTRY_HEAD = /^ {2}([a-z_]+): \{/gm;
const PANEL_COMPONENT = /<([A-Za-z]+Panel)\b/g;

export function panelRegistrations(source: string): { page: string; component: string }[] {
  const body = REGISTRY_BLOCK.exec(source)?.[1] ?? "";
  const heads = [...body.matchAll(ENTRY_HEAD)];
  return heads.flatMap((head, index) => {
    const start = head.index ?? 0;
    const end = heads[index + 1]?.index ?? body.length;
    const block = body.slice(start, end);
    return [...block.matchAll(PANEL_COMPONENT)].map((match) => ({
      page: head[1],
      component: match[1],
    }));
  });
}

describe("设置页面板 · 唯一外壳", () => {
  const registrations = panelRegistrations(VIEW);

  it("注册面能被读出来（正则失效时本用例先红，而不是让下面的断言凭空通过）", () => {
    expect(registrations.length).toBeGreaterThanOrEqual(5);
  });

  it("每个注册的页都是合法页 id", () => {
    for (const { page } of registrations) {
      expect(SETTINGS_PAGES as readonly string[], `未登记的页 ${page}`).toContain(page);
    }
  });

  it("每个面板的根元素都用 `.luna-settings-panel`（外壳只有一处定义）", () => {
    for (const { component } of registrations) {
      const source = SOURCES[`./${component}.tsx`];
      expect(source, `找不到面板组件文件 ${component}.tsx`).toBeTruthy();
      expect(
        source.includes('className="luna-settings-panel'),
        `${component} 的根元素没有用 .luna-settings-panel——设置页会因此多出第 N 种外壳`,
      ).toBe(true);
    }
  });

  it("反向验证：注册了一个不带原语的面板即红", () => {
    const fixture = [
      "const PAGE_PANELS: Partial<Record<SettingsPageId, PagePanels>> = {",
      "  system: {",
      "    lead: () => <SomeCard />,",
      "    trail: () => (",
      "      <RogueBadPanel />",
      "    ),",
      "    appendRows: () => <SomeRow />,",
      "  },",
      "};",
    ].join("\n");
    const found = panelRegistrations(fixture);
    expect(found).toEqual([{ page: "system", component: "RogueBadPanel" }]);
    expect(SOURCES["./RogueBadPanel.tsx"]).toBeUndefined();
  });
});
