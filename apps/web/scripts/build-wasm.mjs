#!/usr/bin/env node
// 网页版的 WASM 构建（ADR-177 ⑥、TDD-17 §2）：`luna-web` 编成 wasm32-unknown-unknown 的 release 产物，
// 再用 `wasm-bindgen --target web` 生成 ES 模块绑定，输出到 `apps/web/wasm/`（生成物，不入库）。
//
// wasm-bindgen CLI 的版本必须与 Cargo.lock 里 `wasm-bindgen` 库的版本**逐字相同**：两边差一个补丁号，
// 生成的胶水代码与 WASM 里的符号表就对不上，而那只在浏览器里运行时才炸。这里先核对，不对就停。
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, rmSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const webRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = resolve(webRoot, "../..");
const outDir = join(webRoot, "wasm");

function fail(message) {
  console.error(`build:wasm 失败：${message}`);
  process.exit(1);
}

function lockedVersion() {
  const lock = readFileSync(join(repoRoot, "Cargo.lock"), "utf8");
  const match = /\[\[package\]\]\nname = "wasm-bindgen"\nversion = "([^"]+)"/.exec(lock);
  if (!match) fail("Cargo.lock 里找不到 wasm-bindgen");
  return match[1];
}

const wanted = lockedVersion();
const probe = spawnSync("wasm-bindgen", ["--version"], { encoding: "utf8" });
if (probe.error || probe.status !== 0) {
  fail(`找不到 wasm-bindgen CLI；装与 Cargo.lock 同版本的：cargo install wasm-bindgen-cli --locked --version ${wanted}`);
}
const installed = probe.stdout.trim().split(/\s+/).pop();
if (installed !== wanted) {
  fail(`wasm-bindgen CLI 是 ${installed}，Cargo.lock 里是 ${wanted}；重装：cargo install wasm-bindgen-cli --locked --version ${wanted}`);
}

const cargo = spawnSync("cargo", ["build", "-p", "luna-web", "--release", "--target", "wasm32-unknown-unknown"], {
  cwd: repoRoot,
  stdio: "inherit",
});
if (cargo.status !== 0) fail("cargo build 没有通过");

const targetDir = process.env.CARGO_TARGET_DIR ? resolve(process.env.CARGO_TARGET_DIR) : join(repoRoot, "target");
const wasm = join(targetDir, "wasm32-unknown-unknown/release/luna_web.wasm");
if (!existsSync(wasm)) fail(`产物不在预期位置：${wasm}`);

rmSync(outDir, { recursive: true, force: true });
mkdirSync(outDir, { recursive: true });
const bindgen = spawnSync("wasm-bindgen", ["--target", "web", "--out-dir", outDir, "--out-name", "luna_web", wasm], {
  stdio: "inherit",
});
if (bindgen.status !== 0) fail("wasm-bindgen 没有通过");
console.log(`build:wasm 完成：${outDir}`);
