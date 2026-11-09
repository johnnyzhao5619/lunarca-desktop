#!/usr/bin/env node
// Lunarca 开发启动器：端口躲避策略（AGENTS.md §3）。
//
// 背景：Tauri 在启动 `beforeDevCommand`（跑 Vite）之前，就已从 tauri.conf.json 读死
// `build.devUrl` 的端口；若 Vite 因端口被占改用别的端口，webview 会连到失效端口。
// 因此动态换端口必须**同时**协调 Vite 监听端口与 Tauri 的 devUrl —— 二者不能各自为政。
//
// 本启动器：探测 1420 起首个空闲端口 P → 以 `LUNA_DEV_PORT=P` 注入环境（Vite 读取）
// → `tauri dev --config` 覆盖 devUrl 到 `http://localhost:P`（Tauri 读取），
// 二者对齐同一端口。1420 空闲时行为与旧默认完全一致。
//
// CLI 取自 `resolveTauriCli()`：**优先仓库钉版本的 `@tauri-apps/cli`**，回落 `cargo tauri`。

import net from "node:net";
import { spawn, spawnSync } from "node:child_process";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import { clearStaleBundle } from "./dev-stale-bundle.mjs";

const BASE_PORT = 1420;
const MAX_TRIES = 32;
const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");

/**
 * 解析 tauri CLI —— **优先用仓库自己钉版本的那个**。
 *
 * 此前只认 `cargo tauri`，而 `cargo-tauri` 是一个装在 `~/.cargo/bin/` 的**机器级二进制**
 * （`cargo install tauri-cli`），版本**在本仓任何地方都没有钉死**。而 `@tauri-apps/cli` 就在
 * `apps/desktop/package.json` 里钉着版本、在仓库根 `npm ci` 时一起装好——两者并存时，
 * 真正决定构建行为的却是那个没人管版本的。这与 `rust-toolchain.toml` 头注释写的
 * 「防"从第一天就环境漂"」是同一条纪律，只是当时漏掉了 tauri CLI 这一环。
 *
 * 触发修复的现场（2026-08-08）：`~/.cargo/bin` 被整目录清空（自托管 runner 重装工具链），
 * rustup 重装后只恢复了它自己的 shim，`cargo-tauri` 就此消失——而仓库里那个 CLI 一直好好的。
 *
 * 回落到 `cargo tauri` 是为了不打断已经习惯它的人；两者都没有时**给可执行的修复指令**，
 * 而不是让调用方自己去猜。
 */
function resolveTauriCli() {
  // 直接用 node 跑包入口，而不是 `.bin/tauri`：Windows 上那是 sh 脚本，不经 shell 无法
  // spawn；改走 `.cmd` 又得开 shell，`--config` 的 JSON 引号会被 cmd 拆坏。
  // 从声明它的工作区（apps/desktop）按 Node 解析规则找：npm workspaces 把它提升到仓库根的
  // node_modules，但提升与否由 npm 决定，不写死目录。
  const local = resolveWorkspaceModule("@tauri-apps/cli/tauri.js");
  if (local) {
    return { command: process.execPath, prefix: [local], label: "@tauri-apps/cli（仓库钉版本）" };
  }
  const probe = spawnSync("cargo", ["tauri", "--version"], { stdio: "ignore" });
  if (probe.status === 0) {
    return { command: "cargo", prefix: ["tauri"], label: "cargo-tauri（机器全局）" };
  }
  return null;
}

/** 按 apps/desktop 的解析视角找一个依赖文件；没装返回 null。 */
function resolveWorkspaceModule(specifier) {
  try {
    return createRequire(resolve(REPO_ROOT, "apps/desktop/package.json")).resolve(specifier);
  } catch {
    return null;
  }
}

/** 端口是否可被本机监听（回环）。listen 成功即空闲，失败即占用。 */
function isFree(port) {
  return new Promise((done) => {
    const server = net.createServer();
    server.once("error", () => done(false));
    server.once("listening", () => server.close(() => done(true)));
    server.listen(port, "127.0.0.1");
  });
}

async function firstFreePort(exclude = new Set()) {
  for (let offset = 0; offset < MAX_TRIES; offset += 1) {
    const port = BASE_PORT + offset;
    if (exclude.has(port)) {
      continue;
    }
    // eslint-disable-next-line no-await-in-loop -- 顺序探测，命中即止
    if (await isFree(port)) {
      return port;
    }
  }
  throw new Error(
    `[dev] 在 [${BASE_PORT}, ${BASE_PORT + MAX_TRIES}) 未找到空闲端口`,
  );
}

// 探测→绑定之间存在窄竞态窗（端口探测为空闲，但 Vite 真正 bind 前被别的进程抢占）。
// 自愈：若子进程在启动宽限期内以非零码退出（多半是 strictPort 撞端口），换下一个候选
// 端口重试，直到成功进入长跑或耗尽重试次数。稳定占用（如 Docker 常驻 1420）在首次探测
// 即被跳过，不触发重试路径。
const STARTUP_GRACE_MS = 8000;
const MAX_LAUNCH_ATTEMPTS = 5;

function launch(cli, port, signal) {
  return new Promise((resolve) => {
    const configOverride = JSON.stringify({
      build: { devUrl: `http://localhost:${port}` },
    });
    const child = spawn(cli.command, [...cli.prefix, "dev", "--config", configOverride], {
      cwd: REPO_ROOT,
      stdio: "inherit",
      env: { ...process.env, LUNA_DEV_PORT: String(port) },
    });
    const startedAt = Date.now();
    // 转发终止信号，避免 Ctrl+C 后 tauri/vite 子进程残留继续占用端口。
    const forward = (sig) => child.kill(sig);
    signal.forEach((sig) => process.on(sig, forward));
    child.on("exit", (code) => {
      signal.forEach((sig) => process.off(sig, forward));
      const fastFailure = code !== 0 && Date.now() - startedAt < STARTUP_GRACE_MS;
      resolve({ code: code ?? 0, fastFailure });
    });
  });
}

const SIGNALS = ["SIGINT", "SIGTERM"];

// **先确认 CLI 存在，再进换端口重试回环。**
//
// 换端口自愈的前提是"启动即失败 = 端口被抢占"。CLI 根本不存在时那个前提不成立，而回环
// 照样跑满 5 次——用户看到的是五遍「端口被占用，改用 142x」，真正的
// `error: no such command: tauri` 被自己的重试日志淹掉（真机 2026-08-08 坐实：日志里
// 端口从 1420 一路数到 1424，而一个端口都没被占）。
// 一个自愈机制把无关故障吞成自己那一类，比不自愈更糟：它给出的是一条错误的排查方向。
const cli = resolveTauriCli();
if (!cli) {
  console.error(
    "[dev] 找不到 tauri CLI。\n" +
      "  仓库自带的那个（推荐，版本随 package.json 钉死；在仓库根装全部前端工作区）：\n" +
      "    npm ci\n" +
      "  或装机器全局的：\n" +
      "    cargo install tauri-cli --locked",
  );
  process.exit(1);
}
console.log(`[dev] 使用 ${cli.label}`);
// 随包转写档（PRD-14 §6）：`bundle.resources` 指着它，缺文件时 tauri 会以一句
// 与原因相距很远的报错停下。开发期这一步几乎总是空操作（已就位即直接返回），
// 只有新克隆的工作树会真的下一次；取不回就在这里停，而不是让 tauri 去报路径错。
// Windows 的 python.org 安装只带 `python`；`python3` 多半是应用商店的占位别名（退出码 9009）。
const python = process.platform === "win32" ? "python" : "python3";
const fetched = spawnSync(python, [resolve(REPO_ROOT, "scripts/fetch-bundled-asr.py")], {
  cwd: REPO_ROOT,
  stdio: "inherit",
});
if (fetched.status !== 0) {
  console.error("[dev] 随包转写档取不回，先解决它再启动（见上方输出）。");
  process.exit(1);
}
// 陈旧 bundle 在场时 dev 会把控制权转交给它并静默带走 Vite（E1，理由见该模块头注释）。
clearStaleBundle(REPO_ROOT);

const tried = new Set();
for (let attempt = 0; attempt < MAX_LAUNCH_ATTEMPTS; attempt += 1) {
  const port = await firstFreePort(tried);
  tried.add(port);
  if (port !== BASE_PORT) {
    console.log(`[dev] 端口 ${BASE_PORT} 被占用，改用 ${port}`);
  }
  const { code, fastFailure } = await launch(cli, port, SIGNALS);
  if (!fastFailure) {
    process.exit(code);
  }
  console.log(`[dev] 端口 ${port} 启动即失败（疑似被抢占），换端口重试…`);
}
console.error(`[dev] 连续 ${MAX_LAUNCH_ATTEMPTS} 次启动失败，请检查端口占用`);
process.exit(1);
