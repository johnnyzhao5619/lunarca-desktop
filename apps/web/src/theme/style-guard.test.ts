/// <reference types="vite/client" />
/**
 * 网页的 CSS 守护（TDD-15 §5 L3–L6，`npm run lint:css`）：判据与桌面同一份（`@lunarca/lint-config/css-guards`），
 * 对网页自己的入口逐条跑一遍，另加网页的两条——只用登记过的 Token、类名有规则也有消费者。
 */

import { describe, expect, it } from "vitest";
import {
  colorLiteralViolations,
  dedicatedBlocks,
  importantUses,
  layeredCmContentBox,
  layeredCmLinePadding,
  parseCssRules,
  physicalDirectionLocations,
  staticTokenReferences,
  stripCommentsKeepLines,
} from "@lunarca/lint-config/css-guards";
import { runtimeStyleCss, sourceDisplayPath, styleGraph } from "./style-source";

const CODE = import.meta.glob("../**/*.{ts,tsx}", { eager: true, import: "default", query: "?raw" }) as Record<string, string>;
const PACKAGE_CODE = import.meta.glob("../../../../packages/editor-core/src/**/*.{ts,tsx}", {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

const { reachable } = styleGraph;
const ownSources = reachable.filter((source) => !source.path.startsWith("../../../../packages/"));

describe("网页样式入口 · 完整导入图与层叠契约", () => {
  it("入口声明与桌面相同的外层与 Token 子层顺序", () => {
    expect(styleGraph.entrySource).toContain("@layer reset, tokens, base, primitives, layout, integrations, features;");
    expect(styleGraph.entrySource).toContain("@layer tokens.primitive, tokens.base, tokens.family, tokens.accessibility;");
  });

  it("每一份样式（含共享样式包的）恰好由入口到达一次，且每条边都声明 layer", () => {
    const reached = reachable.map((source) => source.path).sort();
    expect(reached).toEqual(styleGraph.allPaths.filter((path) => path !== "../styles/index.css"));
    expect(styleGraph.entryImports).toHaveLength(styleGraph.entryImportCount);
    expect(styleGraph.entryImports.every((entry) => entry.layer.length > 0)).toBe(true);
    expect(reached.some((path) => path.endsWith("/packages/styles/src/tokens/base.css"))).toBe(true);
  });

  it("运行态聚合结果里看得到 Token 与网页自己的规则（防空图假绿）", () => {
    expect(runtimeStyleCss).toContain("--space-4");
    expect(runtimeStyleCss).toContain(".luna-web-auth");
    expect(runtimeStyleCss).not.toMatch(/@import\s+["'](?:\.|\/|@)/);
  });
});

describe("网页样式 · Token、方向、选择器、优先级", () => {
  it("组件区不出现颜色字面量", () => {
    const violations = colorLiteralViolations(reachable);
    expect(violations, violations.join("\n")).toEqual([]);
  });

  it("静态阶梯与语义色没有悬垂引用", () => {
    const report = staticTokenReferences(runtimeStyleCss);
    expect(report.missing, report.missing.join("\n")).toEqual([]);
    expect(report.referenced.size).toBeGreaterThan(20);
  });

  it("网页自己的样式只引用有定义的自定义属性（任何 `var(--…)`）", () => {
    const declared = new Set([...runtimeStyleCss.matchAll(/(--[\w-]+)\s*:/g)].map((match) => match[1]));
    const missing = ownSources.flatMap((source) =>
      [...stripCommentsKeepLines(source.text).matchAll(/var\((--[\w-]+)/g)]
        .map((match) => match[1])
        .filter((name) => !declared.has(name))
        .map((name) => `${sourceDisplayPath(source.path)}: var(${name})`),
    );
    expect(missing, missing.join("\n")).toEqual([]);
  });

  it("不写物理方向属性", () => {
    const offenders = reachable.flatMap((source) =>
      physicalDirectionLocations(source.text, source.path).map(
        (entry) => `${sourceDisplayPath(source.path)}:${entry.line} ${entry.property}`,
      ),
    );
    expect(offenders, offenders.join("\n")).toEqual([]);
  });

  it("一个选择器只有一个专属定义块", () => {
    const rules = reachable.flatMap((source) => parseCssRules(source.text, source.path));
    const offenders = [...dedicatedBlocks(rules).entries()]
      .filter(([, occurrences]) => occurrences.length > 1)
      .map(([key]) => key);
    expect(offenders, offenders.join("\n")).toEqual([]);
    expect(rules.length).toBeGreaterThan(50);
  });

  it("网页自己的样式不用 `!important`（共享包里的无障碍与 `[hidden]` 两处例外随包登记）", () => {
    const own = importantUses(ownSources).map((use) => `${sourceDisplayPath(use.path)}:${use.property}`);
    expect(own).toEqual([]);
  });

  it("CM 自带类名的收编条（`@lunarca/styles/codemirror.css`）以 integrations 层到达，`.cm-line` / `.cm-content` 的物理内距只有它写", () => {
    const integration = reachable.find((source) => source.path.endsWith("/packages/styles/src/codemirror.css"));
    expect(integration?.layer).toBe("integrations");
    expect(integration?.text).toContain(".luna-md-surface .cm-line");
    const offenders = reachable
      .filter((source) => source !== integration)
      .flatMap((source) =>
        [...layeredCmLinePadding(source.text), ...layeredCmContentBox(source.text)].map((hit) => `${sourceDisplayPath(source.path)}: ${hit}`),
      );
    expect(offenders, offenders.join("\n")).toEqual([]);
  });

  it("判据真的会红（反向验证）", () => {
    const fixture = [{ path: "fixture.css", layer: "features", text: ".luna-web-x {\n  margin-left: 1px;\n  color: #fff;\n}\n.luna-web-x { padding: 0; }" }];
    expect(colorLiteralViolations(fixture)).toHaveLength(1);
    expect(physicalDirectionLocations(fixture[0].text)).toHaveLength(1);
    expect([...dedicatedBlocks(parseCssRules(fixture[0].text, "fixture.css")).values()].some((list) => list.length > 1)).toBe(true);
  });
});

/** 源码里写到的 `luna-web-*` 类名（className 字面量、模板里的字面片段都算）。 */
function usedClassNames(code: Record<string, string>): Set<string> {
  const out = new Set<string>();
  for (const [path, source] of Object.entries(code)) {
    if (path.includes(".test.")) continue;
    for (const match of source.matchAll(/luna-web-[a-z0-9]+(?:-[a-z0-9]+)*(?:__[a-z0-9]+(?:-[a-z0-9]+)*)?(?:--[a-z0-9]+(?:-[a-z0-9]+)*)?/g)) {
      out.add(match[0]);
    }
  }
  return out;
}

function declaredClassNames(css: string): Set<string> {
  return new Set(
    [...stripCommentsKeepLines(css).matchAll(/\.(luna-web-[a-z0-9]+(?:-[a-z0-9]+)*(?:__[a-z0-9]+(?:-[a-z0-9]+)*)?(?:--[a-z0-9]+(?:-[a-z0-9]+)*)?)/g)].map(
      (match) => match[1],
    ),
  );
}

describe("网页样式 · 类名有规则也有消费者", () => {
  const declared = new Set(ownSources.flatMap((source) => [...declaredClassNames(source.text)]));
  const used = usedClassNames({ ...CODE, ...PACKAGE_CODE });

  it("代码里用到的 `luna-web-*` 类都有规则（否则那一行代码没有生效）", () => {
    const dangling = [...used].filter((name) => !declared.has(name)).sort();
    expect(dangling).toEqual([]);
  });

  it("样式表里的 `luna-web-*` 类都有消费者（否则是死规则）", () => {
    const dead = [...declared].filter((name) => !used.has(name)).sort();
    expect(dead).toEqual([]);
    expect(declared.size).toBeGreaterThan(10);
  });
});
