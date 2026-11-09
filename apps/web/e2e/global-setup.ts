/**
 * 端到端的全局准备：开发模式服务端 + 种子 + 静态预览（见 playwright.config.ts 头注释）。
 * 起的进程号与种子结果写进 `e2e/.state/`（不入库），teardown 据此收尾。
 */

import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { randomBytes } from "node:crypto";
import { createInterface } from "node:readline";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const webRoot = resolve(import.meta.dirname, "..");
const repoRoot = resolve(webRoot, "../..");
const stateDir = join(webRoot, "e2e/.state");

export const SERVER_PORT = process.env.LUNARCA_E2E_SERVER_PORT ?? "18181";
export const WEB_PORT = process.env.LUNARCA_E2E_WEB_PORT ?? "4183";
export const ADMIN_PASSWORD = "e2e operator passphrase";

export const ACCOUNTS = [
  { email: "reader@example.com", password: "reader passphrase 1" },
  { email: "revoker@example.com", password: "revoker passphrase 2" },
  { email: "empty@example.com", password: "empty passphrase 3", noVault: true },
] as const;

function waitForLine(child: ChildProcess, pattern: RegExp, label: string, timeoutMs = 60_000): Promise<void> {
  return new Promise((resolvePromise, reject) => {
    let seen = "";
    const timer = setTimeout(() => reject(new Error(`${label} 没在 ${timeoutMs} ms 内就绪：\n${seen.slice(-2000)}`)), timeoutMs);
    const onData = (chunk: Buffer) => {
      seen += chunk.toString();
      if (pattern.test(seen)) {
        clearTimeout(timer);
        resolvePromise();
      }
    };
    child.stdout?.on("data", onData);
    child.stderr?.on("data", onData);
    child.on("exit", (code) => {
      clearTimeout(timer);
      reject(new Error(`${label} 提前退出（${code}）：\n${seen.slice(-2000)}`));
    });
  });
}

function seedBinary(): string {
  if (process.env.LUNARCA_E2E_SEED_BIN) return process.env.LUNARCA_E2E_SEED_BIN;
  const build = spawnSync("cargo", ["build", "-p", "luna-cloud", "--example", "web_e2e_seed"], {
    cwd: repoRoot,
    stdio: "inherit",
    env: { ...process.env, CARGO_INCREMENTAL: "0" },
  });
  if (build.status !== 0) throw new Error("种子工具编译失败");
  const targetDir = process.env.CARGO_TARGET_DIR ? resolve(process.env.CARGO_TARGET_DIR) : join(repoRoot, "target");
  return join(targetDir, "debug/examples/web_e2e_seed");
}

export default async function globalSetup(): Promise<() => void> {
  const serverBin = process.env.LUNARCA_SERVER_BIN;
  if (!serverBin || !existsSync(serverBin)) {
    throw new Error("端到端要一台开发模式服务端：设 LUNARCA_SERVER_BIN=<lunarca-server 可执行文件>");
  }
  if (!existsSync(join(webRoot, "dist/index.html"))) {
    throw new Error("先构建网页：npm --prefix apps/web run build:wasm && npm --prefix apps/web run build");
  }
  mkdirSync(stateDir, { recursive: true });
  const blobRoot = mkdtempSync(join(tmpdir(), "lunarca-e2e-blobs-"));
  const api = `http://127.0.0.1:${SERVER_PORT}`;

  const server = spawn(serverBin, [], {
    env: {
      ...process.env,
      LUNARCA_DEV_INMEMORY: "1",
      BIND_ADDR: `127.0.0.1:${SERVER_PORT}`,
      BASE_URL: api,
      WEB_ORIGIN: `http://127.0.0.1:${WEB_PORT}`,
      TRUSTED_PROXY_HOPS: "1",
      LUNARCA_ADMIN_PASSWORD: ADMIN_PASSWORD,
      SECURITY_HASH_KEY: randomBytes(32).toString("base64"),
      OPAQUE_SERVER_SETUP: randomBytes(64).toString("base64"),
      SYNC_STORAGE_CREDENTIAL_KEY: randomBytes(32).toString("base64"),
      SYNC_BLOB_STORE_ROOTS: blobRoot,
      RUST_LOG: "warn,lunarca_server=info,cp_api::routes::web_session=info",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  let cspReports = 0;
  for (const stream of [server.stdout, server.stderr]) {
    if (!stream) continue;
    createInterface({ input: stream }).on("line", (line) => {
      if (line.includes("web csp violation")) cspReports++;
    });
  }
  await waitForLine(server, /lunarca-server listening/, "服务端");

  const seed = seedBinary();
  const stateFiles = ACCOUNTS.map((_, index) => join(stateDir, `desktop-${index}.json`));
  const seeded = ACCOUNTS.map((account, index) => {
    rmSync(stateFiles[index], { force: true });
    const run = spawnSync(seed, [], {
      encoding: "utf8",
      env: {
        ...process.env,
        LUNARCA_E2E_API: api,
        LUNARCA_E2E_ADMIN_PASSWORD: ADMIN_PASSWORD,
        LUNARCA_E2E_BLOB_ROOT: blobRoot,
        LUNARCA_E2E_EMAIL: account.email,
        LUNARCA_E2E_PASSWORD: account.password,
        LUNARCA_E2E_STATE_FILE: stateFiles[index],
        ...("noVault" in account ? { LUNARCA_E2E_NO_VAULT: "1" } : {}),
      },
    });
    if (run.status !== 0) throw new Error(`种子失败：\n${run.stderr}`);
    return JSON.parse(run.stdout.trim().split("\n").pop() ?? "{}") as Record<string, unknown>;
  });

  const preview = spawn("node", [join(webRoot, "scripts/preview-server.mjs")], {
    env: { ...process.env, LUNARCA_API: api, PORT: WEB_PORT, HOST: "127.0.0.1" },
    stdio: ["ignore", "pipe", "pipe"],
  });
  await waitForLine(preview, /网页预览/, "静态预览");

  writeFileSync(
    join(stateDir, "env.json"),
    JSON.stringify(
      { api, web: `http://127.0.0.1:${WEB_PORT}`, pids: [server.pid, preview.pid], blobRoot, seeded, seedBin: seed, stateFiles },
      null,
      2,
    ),
  );
  server.unref();
  preview.unref();
  return () => {
    if (cspReports !== 0) throw new Error(`服务端收到 ${cspReports} 条 CSP 违规报告`);
    console.log("服务端 CSP 违规报告：0");
  };
}
