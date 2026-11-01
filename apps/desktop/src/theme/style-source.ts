/// <reference types="vite/client" />

import runtimeCss from "../styles/index.css?inline";

export type StyleSource = {
  path: string;
  text: string;
  layer: string;
};

const entryPath = "../styles/index.css";
const rawSources = import.meta.glob("../**/*.css", {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

const IMPORT = /@import\s+["']([^"']+)["']\s+layer\(([^)]+)\)\s*;/g;

function resolveLocalCssPath(from: string, target: string): string {
  if (!/^\.\.?\//.test(target) || !target.endsWith(".css")) {
    throw new Error(`样式入口只允许本地相对 CSS：${target}`);
  }
  const fromWithinSrc = from.replace(/^\.\.\//, "");
  const parts = [...fromWithinSrc.split("/").slice(0, -1), ...target.split("/")];
  const resolved: string[] = [];
  for (const part of parts) {
    if (!part || part === ".") continue;
    if (part === "..") {
      if (resolved.length === 0) throw new Error(`样式 import 越出 src：${from} -> ${target}`);
      resolved.pop();
    } else {
      resolved.push(part);
    }
  }
  return `../${resolved.join("/")}`;
}

function importsOf(path: string): Array<{ path: string; layer: string }> {
  const source = rawSources[path];
  if (source === undefined) throw new Error(`样式源不存在：${path}`);
  return [...source.matchAll(IMPORT)].map((match) => ({
    path: resolveLocalCssPath(path, match[1]),
    layer: match[2].trim(),
  }));
}

function collectSources(): StyleSource[] {
  const visited = new Set<string>();
  const visiting = new Set<string>();
  const collected: StyleSource[] = [];

  const visit = (path: string, layer: string) => {
    if (visiting.has(path)) throw new Error(`样式 import 图存在环：${[...visiting, path].join(" -> ")}`);
    if (visited.has(path)) throw new Error(`样式文件被入口重复导入：${path}`);
    if (rawSources[path] === undefined) throw new Error(`入口导入了不存在的样式文件：${path}`);

    visiting.add(path);
    const imports = importsOf(path);
    if (path !== entryPath && imports.length > 0) {
      throw new Error(`叶子样式不得继续导入 CSS：${path}`);
    }
    if (path !== entryPath) collected.push({ path, text: rawSources[path], layer });
    for (const child of imports) visit(child.path, child.layer);
    visiting.delete(path);
    visited.add(path);
  };

  visit(entryPath, "");
  return collected;
}

export const styleEntrySource = rawSources[entryPath];
export const styleEntryImports = importsOf(entryPath);
export const styleEntryImportCount = [...styleEntrySource.matchAll(/@import\b/g)].length;
export const reachableStyleSources = collectSources();
export const allProjectStylePaths = Object.keys(rawSources).sort();
export const runtimeStyleCss = runtimeCss;

export function sourceLocation(source: StyleSource, line: number): string {
  return sourcePathLocation(source.path, line);
}

export function sourcePathLocation(path: string, line: number): string {
  return `${path.replace(/^\.\.\//, "src/")}:${line}`;
}
