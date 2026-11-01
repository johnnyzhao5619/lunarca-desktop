#!/usr/bin/env node
// 删除 Vite 可再生成的前端构建目录；不碰依赖、Rust 缓存、IPC 生成物或安装包。
import { existsSync, rmSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const frontendDist = join(repoRoot, "apps", "desktop", "dist");

if (!existsSync(frontendDist)) {
  console.log("前端构建目录已是干净状态：apps/desktop/dist 不存在。");
  process.exit(0);
}

rmSync(frontendDist, { recursive: true, force: true });
console.log("已删除前端构建目录：apps/desktop/dist。");
