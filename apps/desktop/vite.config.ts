import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

// 端口用 Tauri 惯例的 1420（Vite 默认的 5173 常被 Docker Desktop 常驻占用）。
// 端口躲避（AGENTS.md §3）：`scripts/dev.mjs` 启动器探测到 1420 被占时选下一个空闲
// 端口，经 `LUNA_DEV_PORT` 注入并同步覆盖 Tauri devUrl；此处据环境变量取端口，
// 二者对齐。直接 `npm run dev`（无启动器）时回落 1420，保持既有行为。
// strictPort 仍保留：端口已由启动器保证空闲，若仍被抢占应显式失败而非静默改端口。
const devPort = Number.parseInt(process.env.LUNA_DEV_PORT ?? "", 10) || 1420;

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: devPort,
    strictPort: true,
  },
  build: {
    outDir: "dist",
    target: "es2020",
    emptyOutDir: true,
  },
  test: {
    // Vitest 默认 `css: false` 会把 CSS 导入**存根为空串**——`styles.css?raw` 因此恒为 ""，
    // 主题 Token 守护（theme-token-guard.test.ts）自写下起一直在空集合上空跑、永远绿。
    // 开启后守护才真正读到样式表；反向验证：删掉任一被引用的 Token 或写死一个颜色即变红。
    css: true,
  },
});
