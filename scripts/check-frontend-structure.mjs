import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptPath = fileURLToPath(import.meta.url);
const repositoryRoot = path.resolve(path.dirname(scriptPath), "..");
const sourceRoot = path.join(repositoryRoot, "apps/desktop/src");
const kebabCase = /^[a-z][a-z0-9]*(?:-[a-z0-9]+)*$/;
const pascalCase = /^[A-Z][A-Za-z0-9]*$/;
const locale = /^[a-z]{2,3}(?:-[A-Z]{2})?$/;
export function collectImportSpecifiers(source) {
  const specifiers = new Set();
  const patterns = [
    /\b(?:import|export)\s+(?:type\s+)?[\s\S]*?\sfrom\s+["']([^"']+)["']/g,
    /\bimport\s*["']([^"']+)["']/g,
    /\bimport\s*\(\s*["']([^"']+)["']\s*\)/g,
  ];
  for (const pattern of patterns) {
    for (const match of source.matchAll(pattern)) specifiers.add(match[1]);
  }
  return [...specifiers];
}

export function hasOpaqueDynamicImport(source) {
  const withoutLiteralDynamicImports = source.replace(
    /\bimport\s*\(\s*["'][^"']+["']\s*\)/g,
    "",
  );
  return /\bimport\s*\(/.test(withoutLiteralDynamicImports);
}

export function findDependencyCycle(graph) {
  const visited = new Set();
  const active = new Set();
  const trail = [];

  function visit(node) {
    if (active.has(node)) {
      const start = trail.indexOf(node);
      return [...trail.slice(start), node];
    }
    if (visited.has(node)) return null;
    visited.add(node);
    active.add(node);
    trail.push(node);
    for (const next of graph.get(node) ?? []) {
      const cycle = visit(next);
      if (cycle) return cycle;
    }
    trail.pop();
    active.delete(node);
    return null;
  }

  for (const node of graph.keys()) {
    const cycle = visit(node);
    if (cycle) return cycle;
  }
  return null;
}

export function isProhibitedFeatureBarrel(relativePath) {
  const segments = relativePath.split(/[\\/]/);
  return segments[0] === "features" && segments.at(-1) === "index.ts";
}

function run() {
  const violations = [];
  const sourceFiles = [];

  function visit(directory) {
    for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
      if (entry.name === "node_modules" || entry.name === "dist") continue;
      const absolute = path.join(directory, entry.name);
      const relative = path.relative(sourceRoot, absolute);
      if (entry.isDirectory()) {
        const parent = path.relative(sourceRoot, directory);
        const valid = parent === path.join("i18n", "locales") ? locale.test(entry.name) : kebabCase.test(entry.name);
        if (!valid) violations.push(`目录命名不合规: ${relative}`);
        visit(absolute);
        continue;
      }
      if (!/\.(?:ts|tsx)$/.test(entry.name) || entry.name === "bindings.ts") continue;
      if (isProhibitedFeatureBarrel(relative)) {
        violations.push(`功能模块禁止 barrel index.ts: ${relative}`);
      }
      const extension = path.extname(entry.name);
      const isTest = /\.test\.(?:ts|tsx)$/.test(entry.name);
      const base = entry.name.slice(0, -extension.length).replace(/\.test$/, "");
      const valid =
        base === "index" ||
        base === "main" ||
        (extension === ".ts" && !isTest
          ? kebabCase.test(base)
          : kebabCase.test(base) || pascalCase.test(base));
      if (!valid) violations.push(`文件命名不合规: ${relative}`);
      sourceFiles.push(absolute);
    }
  }

  visit(sourceRoot);

/**
 * Feature modules may depend on lower-level feature capabilities, but never in
 * a circle. Cross-feature composition belongs to `src/app`, so a cycle is an
 * architectural violation rather than a runtime import accident.
 */
  const featureRoot = path.join(sourceRoot, "features");
  const featureEdges = new Map();
  for (const sourceFile of sourceFiles) {
    if (!sourceFile.startsWith(`${featureRoot}${path.sep}`)) continue;
    const from = path.relative(featureRoot, sourceFile).split(path.sep)[0];
    const source = fs.readFileSync(sourceFile, "utf8");
    if (hasOpaqueDynamicImport(source)) {
      violations.push(
        `功能模块禁止不可静态解析的动态 import: ${path.relative(sourceRoot, sourceFile)}`,
      );
    }
    for (const specifier of collectImportSpecifiers(source)) {
      if (!specifier.startsWith(".")) continue;
      const target = path.normalize(path.resolve(path.dirname(sourceFile), specifier));
      if (!target.startsWith(`${featureRoot}${path.sep}`)) continue;
      const to = path.relative(featureRoot, target).split(path.sep)[0];
      if (from === to) continue;
      if (!featureEdges.has(from)) featureEdges.set(from, new Set());
      featureEdges.get(from).add(to);
    }
  }

  const cycle = findDependencyCycle(featureEdges);
  if (cycle) {
    violations.push(`功能模块存在循环依赖: ${cycle.join(" -> ")}`);
  }

  if (violations.length > 0) {
    console.error(violations.join("\n"));
    process.exitCode = 1;
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === scriptPath) run();
