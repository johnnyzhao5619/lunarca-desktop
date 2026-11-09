/**
 * 样式导入图（TDD-15 §3.3）：从宿主的唯一样式入口出发，按 `@import "…" layer(…)` 走完整张图，
 * 交出每一份被到达的样式及其所在 layer。CSS 守护（Token、方向属性、选择器唯一、`!important`……）
 * 都只读这张图——读入口能到达的东西，而不是目录里碰巧有的东西。
 *
 * 宿主（桌面、网页）各自用 `import.meta.glob` 收齐自己的 CSS 与共享样式包的 CSS 交进来；共享包的
 * 文件以包名导入（`@import "@lunarca/styles/tokens/base.css" layer(…)`），这里按 `packages` 把包名
 * 前缀换回 glob 的键。一份文件被导入两次、叶子样式再导入 CSS、导入了不存在的文件，都当场抛错。
 */

export type StyleSource = {
  /** glob 的键（相对宿主 `theme/` 目录）。 */
  path: string;
  text: string;
  layer: string;
};

export interface StyleGraphInput {
  /** 入口在 `sources` 里的键。 */
  entry: string;
  /** glob 键 → 原文：宿主自己的 CSS 与共享包的 CSS 都在里面。 */
  sources: Record<string, string>;
  /** 包名前缀 → glob 键前缀，如 `"@lunarca/styles/"` → `"../../../../packages/styles/src/"`。 */
  packages: Record<string, string>;
}

export interface StyleGraph {
  entrySource: string;
  entryImports: Array<{ path: string; layer: string }>;
  entryImportCount: number;
  reachable: StyleSource[];
  /** `sources` 的全部键（排过序）：守护据此判「每一份都恰好由入口到达一次」。 */
  allPaths: string[];
}

const IMPORT = /@import\s+["']([^"']+)["']\s+layer\(([^)]+)\)\s*;/g;

/** 把 glob 键（`../a/./b/../c.css` 之类）规整成唯一写法；开头的 `..` 保留。 */
function normalize(path: string): string {
  const out: string[] = [];
  for (const part of path.split("/")) {
    if (!part || part === ".") continue;
    if (part === ".." && out.length > 0 && out[out.length - 1] !== "..") {
      out.pop();
    } else {
      out.push(part);
    }
  }
  return out.join("/");
}

function resolveImport(input: StyleGraphInput, from: string, target: string): string {
  if (!target.endsWith(".css")) throw new Error(`样式入口只允许导入 CSS：${target}`);
  for (const [name, prefix] of Object.entries(input.packages)) {
    if (target.startsWith(name)) return normalize(`${prefix}${target.slice(name.length)}`);
  }
  if (!/^\.\.?\//.test(target)) throw new Error(`样式只允许本地相对路径或登记过的共享包：${target}`);
  const dir = from.split("/").slice(0, -1).join("/");
  return normalize(`${dir}/${target}`);
}

function importsOf(input: StyleGraphInput, path: string): Array<{ path: string; layer: string }> {
  const source = input.sources[path];
  if (source === undefined) throw new Error(`样式源不存在：${path}`);
  return [...source.matchAll(IMPORT)].map((match) => ({
    path: resolveImport(input, path, match[1]),
    layer: match[2].trim(),
  }));
}

export function buildStyleGraph(input: StyleGraphInput): StyleGraph {
  const sources = Object.fromEntries(Object.entries(input.sources).map(([key, text]) => [normalize(key), text]));
  const normalized: StyleGraphInput = { ...input, entry: normalize(input.entry), sources };
  const visited = new Set<string>();
  const visiting = new Set<string>();
  const reachable: StyleSource[] = [];

  const visit = (path: string, layer: string) => {
    if (visiting.has(path)) throw new Error(`样式 import 图存在环：${[...visiting, path].join(" -> ")}`);
    if (visited.has(path)) throw new Error(`样式文件被入口重复导入：${path}`);
    if (sources[path] === undefined) throw new Error(`入口导入了不存在的样式文件：${path}`);
    visiting.add(path);
    const imports = importsOf(normalized, path);
    if (path !== normalized.entry && imports.length > 0) {
      throw new Error(`叶子样式不得继续导入 CSS：${path}`);
    }
    if (path !== normalized.entry) reachable.push({ path, text: sources[path], layer });
    for (const child of imports) visit(child.path, child.layer);
    visiting.delete(path);
    visited.add(path);
  };

  visit(normalized.entry, "");
  const entrySource = sources[normalized.entry];
  return {
    entrySource,
    entryImports: importsOf(normalized, normalized.entry),
    entryImportCount: [...entrySource.matchAll(/@import\b/g)].length,
    reachable,
    allPaths: Object.keys(sources).sort(),
  };
}
