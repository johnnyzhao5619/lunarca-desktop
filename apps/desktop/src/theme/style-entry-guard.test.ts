import { describe, expect, it } from "vitest";
import {
  allProjectStylePaths,
  reachableStyleSources,
  runtimeStyleCss,
  styleEntryImportCount,
  styleEntryImports,
  styleEntrySource,
} from "./style-source";

describe("样式入口 · 完整导入图与层叠契约", () => {
  it("入口声明固定外层与 Token 子层顺序", () => {
    expect(styleEntrySource).toContain(
      "@layer reset, tokens, base, primitives, layout, integrations, features;",
    );
    expect(styleEntrySource).toContain(
      "@layer tokens.primitive, tokens.base, tokens.family, tokens.accessibility;",
    );
  });

  it("所有受管样式恰好由入口到达一次，且每条边显式声明 layer", () => {
    const reachablePaths = reachableStyleSources.map((source) => source.path).sort();
    const expectedPaths = allProjectStylePaths.filter((path) => path !== "../styles/index.css").sort();
    expect(reachablePaths).toEqual(expectedPaths);
    expect(new Set(reachablePaths).size).toBe(reachablePaths.length);
    expect(styleEntryImports).toHaveLength(styleEntryImportCount);
    expect(styleEntryImports.length).toBeGreaterThan(20);
    expect(styleEntryImports.every((entry) => entry.layer.length > 0)).toBe(true);
    expect(new Set(styleEntryImports.map((entry) => entry.layer.split(".")[0]))).toEqual(
      new Set(["reset", "tokens", "base", "primitives", "layout", "integrations", "features"]),
    );
  });

  it("入口之外没有 CSS 继续导入，运行态聚合结果没有残留本地 @import", () => {
    expect(reachableStyleSources.every((source) => !/@import\s/.test(source.text))).toBe(true);
    expect(runtimeStyleCss).not.toMatch(/@import\s+["'](?:\.|\/)/);
  });

  it("拒绝未声明 layer 的任意 @import 语法（反向验证）", () => {
    const fixture = `${styleEntrySource}\n@import url("https://example.invalid/theme.css");`;
    expect([...fixture.matchAll(/@import\b/g)]).toHaveLength(styleEntryImportCount + 1);
    expect(styleEntryImports).toHaveLength(styleEntryImportCount);
  });

  it("守护确实看到完整样式量，避免空图或只读入口而假绿", () => {
    expect(reachableStyleSources.length).toBeGreaterThan(20);
    expect(runtimeStyleCss.length).toBeGreaterThan(300_000);
    expect(runtimeStyleCss).toContain(".luna-workspace");
    expect(runtimeStyleCss).toContain(".luna-chat");
    expect(runtimeStyleCss).toContain(".luna-settings");
  });
});
