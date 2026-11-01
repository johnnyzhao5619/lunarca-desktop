import assert from "node:assert/strict";
import test from "node:test";
import {
  collectImportSpecifiers,
  findDependencyCycle,
  hasOpaqueDynamicImport,
  isProhibitedFeatureBarrel,
} from "./check-frontend-structure.mjs";

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
