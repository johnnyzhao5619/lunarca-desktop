/**
 * 网页版端到端（TDD-17 §8，`npm --prefix apps/web run e2e`）——本地门禁，不进开发仓 CI：服务端在私有商业仓。
 *
 * 跑法：先 `npm --prefix apps/web run build:wasm`，再
 *   LUNARCA_SERVER_BIN=<lunarca-server 可执行文件> npm --prefix apps/web run e2e
 * 全局准备（e2e/global-setup.ts）会：起开发模式服务端（`LUNARCA_DEV_INMEMORY=1`，内存存储）→ 用种子工具
 * （`cargo run -p luna-cloud --example web_e2e_seed`，或 `LUNARCA_E2E_SEED_BIN` 指定已编好的）登记存储位置、
 * 建号、建库、推笔记 → 起带 TDD-17 §6.1 全套响应头的静态预览（`scripts/preview-server.mjs`，`/api/*` 去前缀
 * 转发给服务端，同源）。浏览器用本机缓存里与 @playwright/test 同版本的 Chromium，不另下载。
 */

import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./e2e",
  // 口令登录按 IP 限流：整套连着登录时会停下来等桶回满（`e2e/helpers.ts` 的 `submitPassword`）。
  timeout: 300_000,
  expect: { timeout: 20_000 },
  fullyParallel: false,
  workers: 1,
  reporter: [["list"]],
  globalSetup: "./e2e/global-setup.ts",
  globalTeardown: "./e2e/global-teardown.ts",
  use: {
    baseURL: `http://127.0.0.1:${process.env.LUNARCA_E2E_WEB_PORT ?? "4183"}`,
    locale: "zh-CN",
    trace: "off",
  },
});
