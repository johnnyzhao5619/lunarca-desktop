import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptPath = fileURLToPath(import.meta.url);
const repositoryRoot = path.resolve(path.dirname(scriptPath), "..");
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

/**
 * 共享包（`packages/<名>/src`）里的一条相对 import 是否越出了包自己的目录。包靠宿主注入端口
 * 拿宿主能力（TDD-17 §2），反向依赖桌面代码会让「网页端能直接复用」这件事静默失效。
 */
export function escapesPackage(packageRoot, sourceFile, specifier) {
  if (!specifier.startsWith(".")) return false;
  const target = path.resolve(path.dirname(sourceFile), specifier);
  return target !== packageRoot && !target.startsWith(`${packageRoot}${path.sep}`);
}

/**
 * 应用代码是否用相对路径伸进了 `packages/`：共享包只经包名（`@lunarca/<名>/…`）与它声明的
 * `exports` 入口使用，绕过去等于把包的内部文件当成了公开接口。
 */
export function reachesIntoPackages(sourceFile, specifier) {
  if (!specifier.startsWith(".")) return false;
  const target = path.resolve(path.dirname(sourceFile), specifier);
  return target.startsWith(`${path.join(repositoryRoot, "packages")}${path.sep}`);
}

/** 前端源码根：桌面应用 + 网页应用 + 每个共享包。门禁不因文件挪进包里就漏检。 */
function sourceRoots() {
  const roots = [
    { kind: "app", dir: path.join(repositoryRoot, "apps/desktop/src") },
    { kind: "app", dir: path.join(repositoryRoot, "apps/web/src") },
  ];
  const packagesDir = path.join(repositoryRoot, "packages");
  if (fs.existsSync(packagesDir)) {
    for (const entry of fs.readdirSync(packagesDir, { withFileTypes: true })) {
      if (!entry.isDirectory()) continue;
      const packageRoot = path.join(packagesDir, entry.name);
      roots.push({ kind: "package", dir: path.join(packageRoot, "src"), packageRoot });
    }
  }
  return roots;
}

function run() {
  const violations = [];

  for (const root of sourceRoots()) {
    if (!fs.existsSync(root.dir)) {
      violations.push(`源码根不存在: ${path.relative(repositoryRoot, root.dir)}`);
      continue;
    }
    const sourceFiles = [];

    function visit(directory) {
      for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
        if (entry.name === "node_modules" || entry.name === "dist") continue;
        const absolute = path.join(directory, entry.name);
        const relative = path.relative(root.dir, absolute);
        const shown = path.relative(repositoryRoot, absolute);
        if (entry.isDirectory()) {
          const valid = path.basename(directory) === "locales" ? locale.test(entry.name) : kebabCase.test(entry.name);
          if (!valid) violations.push(`目录命名不合规: ${shown}`);
          visit(absolute);
          continue;
        }
        // 包里的 CSS 不在这里管：宿主的样式守护（`lint:css`）把每个共享包的 CSS 都纳入扫描面，
        // 要求它恰好被宿主入口到达一次、同样受 Token / 层级 / 选择器约束（`theme/style-source.ts`）。
        if (!/\.(?:ts|tsx)$/.test(entry.name) || entry.name === "bindings.ts") continue;
        if (root.kind === "app" && isProhibitedFeatureBarrel(relative)) {
          violations.push(`功能模块禁止 barrel index.ts: ${shown}`);
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
        if (!valid) violations.push(`文件命名不合规: ${shown}`);
        sourceFiles.push(absolute);
      }
    }

    visit(root.dir);

    for (const sourceFile of sourceFiles) {
      const source = fs.readFileSync(sourceFile, "utf8");
      const shown = path.relative(repositoryRoot, sourceFile);
      for (const specifier of collectImportSpecifiers(source)) {
        if (root.kind === "package" && escapesPackage(root.packageRoot, sourceFile, specifier)) {
          violations.push(`共享包不得用相对路径依赖包外代码: ${shown} -> ${specifier}`);
        }
        if (root.kind === "app" && reachesIntoPackages(sourceFile, specifier)) {
          violations.push(`共享包只经包名使用，不得相对路径伸进 packages/: ${shown} -> ${specifier}`);
        }
      }
    }

    if (root.kind === "app") checkFeatureCycles(root.dir, sourceFiles, violations);
  }

  if (violations.length > 0) {
    console.error(violations.join("\n"));
    process.exitCode = 1;
  }
}

/**
 * Feature modules may depend on lower-level feature capabilities, but never in
 * a circle. Cross-feature composition belongs to `src/app`, so a cycle is an
 * architectural violation rather than a runtime import accident.
 */
function checkFeatureCycles(sourceRoot, sourceFiles, violations) {
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
}

if (process.argv[1] && path.resolve(process.argv[1]) === scriptPath) run();
