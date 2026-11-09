/// <reference types="vite/client" />

import { buildStyleGraph, type StyleSource } from "@lunarca/lint-config/style-graph";
import runtimeCss from "../styles/index.css?inline";

export type { StyleSource };

/**
 * 网页样式入口的完整导入图（TDD-15 §3.3）：扫描面 = 网页自己的 CSS + 共享样式包的 CSS。
 * 判据与桌面同一份（`@lunarca/lint-config`），这里只决定看哪张图。
 */
const rawSources = import.meta.glob(["../**/*.css", "../../../../packages/styles/src/**/*.css"], {
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

export const styleGraph = graph;
export const runtimeStyleCss = runtimeCss;

/** glob 键 → 仓库里读得懂的位置。 */
export function sourceDisplayPath(path: string): string {
  return path.startsWith(PACKAGE_PREFIX) ? `packages/${path.slice(PACKAGE_PREFIX.length)}` : path.replace(/^\.\.\//, "apps/web/src/");
}
