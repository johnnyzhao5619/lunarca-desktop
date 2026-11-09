import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import {
  collectImportSpecifiers,
  escapesPackage,
  findDependencyCycle,
  hasOpaqueDynamicImport,
  isProhibitedFeatureBarrel,
  reachesIntoPackages,
} from "./check-frontend-structure.mjs";

const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

test("collects static, side-effect, re-export and dynamic imports", () => {
  const source = `
    import type { A } from "../alpha/a";
    import "../beta/setup";
    export { C } from "../charlie/c";
    const module = import("../delta/d");
  `;
  assert.deepEqual(new Set(collectImportSpecifiers(source)), new Set([
    "../alpha/a",
    "../beta/setup",
    "../charlie/c",
    "../delta/d",
  ]));
});

test("finds a dependency cycle and ignores an acyclic graph", () => {
  const cyclic = new Map([
    ["alpha", new Set(["beta"])],
    ["beta", new Set(["charlie"])],
    ["charlie", new Set(["alpha"])],
  ]);
  assert.deepEqual(findDependencyCycle(cyclic), ["alpha", "beta", "charlie", "alpha"]);

  const acyclic = new Map([
    ["alpha", new Set(["beta", "charlie"])],
    ["beta", new Set(["charlie"])],
  ]);
  assert.equal(findDependencyCycle(acyclic), null);
});

test("rejects feature barrel indexes without rejecting application entrypoints", () => {
  assert.equal(isProhibitedFeatureBarrel("features/chat/index.ts"), true);
  assert.equal(isProhibitedFeatureBarrel("app/index.ts"), false);
  assert.equal(isProhibitedFeatureBarrel("features/chat/chat-index.ts"), false);
});

test("rejects opaque dynamic imports but accepts literal dynamic imports", () => {
  assert.equal(hasOpaqueDynamicImport('const module = import("../alpha/a");'), false);
  assert.equal(hasOpaqueDynamicImport("const module = import(`../${feature}/entry`);"), true);
});

test("shared packages may not reach outside themselves through relative imports", () => {
  const root = path.join(repositoryRoot, "packages/editor-core");
  const file = path.join(root, "src/render/MarkdownContent.tsx");
  assert.equal(escapesPackage(root, file, "../markdown/markdown"), false);
  assert.equal(escapesPackage(root, file, "react"), false);
  assert.equal(escapesPackage(root, file, "../../../../apps/desktop/src/ipc/bindings"), true);
  // 同名前缀的兄弟目录不算包内（`editor-core-x` 不是 `editor-core`）。
  assert.equal(escapesPackage(root, file, "../../../editor-core-x/src/a"), true);
});

test("app code uses shared packages by name, never by a relative path into packages/", () => {
  const file = path.join(repositoryRoot, "apps/desktop/src/features/chat/ChatMarkdown.tsx");
  assert.equal(reachesIntoPackages(file, "@lunarca/editor-core/MarkdownContent"), false);
  assert.equal(reachesIntoPackages(file, "../../components/markdown/markdown-host"), false);
  assert.equal(reachesIntoPackages(file, "../../../../../packages/editor-core/src/host"), true);
});
