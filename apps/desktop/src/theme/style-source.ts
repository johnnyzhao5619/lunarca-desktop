/// <reference types="vite/client" />

import { buildStyleGraph, type StyleSource } from "@lunarca/lint-config/style-graph";
import runtimeCss from "../styles/index.css?inline";

export type { StyleSource };

/**
 * 桌面样式入口的完整导入图（TDD-15 §3.3）。扫描面 = 桌面自己的 CSS + **每个共享包里的 CSS**：
 * 包里的样式（设计 Token、Markdown 呈现……）以包名导入，同样要恰好被入口到达一次、同样受全部
 * CSS 守护约束——写进包里却没接进入口的一份样式，在这里当场判红。
 */
const rawSources = import.meta.glob(["../**/*.css", "../../../../packages/*/src/**/*.css"], {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

const PACKAGE_PREFIX = "../../../../packages/";

const graph = buildStyleGraph({
  entry: "../styles/index.css",
  sources: rawSources,
  packages: { "@lunarca/styles/": `${PACKAGE_PREFIX}styles/src/` },
});

export const styleEntrySource = graph.entrySource;
export const styleEntryImports = graph.entryImports;
export const styleEntryImportCount = graph.entryImportCount;
export const reachableStyleSources = graph.reachable;
export const allProjectStylePaths = graph.allPaths;
export const runtimeStyleCss = runtimeCss;

export function sourceLocation(source: StyleSource, line: number): string {
  return sourcePathLocation(source.path, line);
}

/** glob 键 → 仓库里读得懂的位置：桌面的写成 `src/…`，共享包的写成 `packages/…`。 */
export function sourceDisplayPath(path: string): string {
  return path.startsWith(PACKAGE_PREFIX) ? `packages/${path.slice(PACKAGE_PREFIX.length)}` : path.replace(/^\.\.\//, "src/");
}

export function sourcePathLocation(path: string, line: number): string {
  return `${sourceDisplayPath(path)}:${line}`;
}
