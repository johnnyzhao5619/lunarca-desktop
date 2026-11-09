/** 端到端收尾：停掉全局准备起的服务端与静态预览，删掉临时的块存储目录。 */

import { existsSync, readFileSync, rmSync } from "node:fs";
import { join, resolve } from "node:path";

export default async function globalTeardown(): Promise<void> {
  const file = join(resolve(import.meta.dirname), ".state/env.json");
  if (!existsSync(file)) return;
  const state = JSON.parse(readFileSync(file, "utf8")) as { pids: number[]; blobRoot: string; stateFiles?: string[] };
  for (const pid of state.pids) {
    try {
      process.kill(pid, "SIGTERM");
    } catch {
      // 已经退出。
    }
  }
  rmSync(state.blobRoot, { recursive: true, force: true });
  // 种子那台「桌面」的凭据文件（刷新令牌）随服务端一起作废。
  for (const stateFile of state.stateFiles ?? []) rmSync(stateFile, { force: true });
  rmSync(file, { force: true });
}
