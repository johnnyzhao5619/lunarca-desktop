import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { clearStaleBundle, STALE_BUNDLE_REL } from "./dev-stale-bundle.mjs";

// E1：bundle 在场 → 清掉并留日志；不在场 → 什么都不做、不刷日志。
test("clearStaleBundle removes a stale bundle and says why", () => {
  const root = mkdtempSync(join(tmpdir(), "luna-dev-bundle-"));
  try {
    const bundle = join(root, STALE_BUNDLE_REL, "macos", "Lunarca.app");
    mkdirSync(bundle, { recursive: true });
    writeFileSync(join(bundle, "marker"), "");
    const logs = [];
    const removed = clearStaleBundle(root, { existsSync, rmSync, log: (line) => logs.push(line) });
    assert.equal(removed, true);
    assert.equal(existsSync(join(root, STALE_BUNDLE_REL)), false);
    assert.equal(logs.length, 1);
    assert.match(logs[0], /陈旧 bundle/);
    assert.match(logs[0], /Vite/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("clearStaleBundle is a no-op without a bundle", () => {
  const root = mkdtempSync(join(tmpdir(), "luna-dev-bundle-"));
  try {
    const logs = [];
    const removed = clearStaleBundle(root, { existsSync, rmSync, log: (line) => logs.push(line) });
    assert.equal(removed, false);
    assert.deepEqual(logs, []);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
