// `node scripts/dev.mjs` 的陈旧 bundle 护栏（E1，2026-09-01 真机验收坐实）。
//
// 现象：`target/debug/bundle/macos/Lunarca.app` 在场时，`tauri dev` 拉起的
// `target/debug/luna-app` 会把控制权交给那个 bundle 里的 app 后自行退出；tauri CLI 只看到
// 「子进程结束」→ exit 0 → dev.mjs 跟着 exit 0，连带把 Vite 一起带走。bundle 里的 app
// 继续跑、页面已加载，于是**表面上完全看不出问题**——改动根本没送进 webview，HMR 静默
// 失效，而你会以为改动生效了。一次 spike 因此白跑了一遍。
//
// 两条退出路径（转交给 bundle / 真的退出）在 tauri CLI 眼里都是 exit 0，启动器无从分辨；
// 能做的是把前提消灭掉：起 dev 前发现陈旧 bundle 就清掉，并把原因写进日志。bundle 只是
// `scripts/build-and-run.sh` 的构建产物，随时可以重建，清它没有代价。
import { existsSync, rmSync } from "node:fs";
import { resolve } from "node:path";

export const STALE_BUNDLE_REL = "target/debug/bundle";

/**
 * 起 dev 之前清掉陈旧 bundle。返回是否真的清了（供日志与测试）。
 * `io` 可注入，便于在 node:test 里用临时目录验证而不碰真实工作树。
 */
export function clearStaleBundle(repoRoot, io = { existsSync, rmSync, log: console.log }) {
  const dir = resolve(repoRoot, STALE_BUNDLE_REL);
  if (!io.existsSync(dir)) {
    return false;
  }
  io.log(
    `[dev] 发现陈旧 bundle ${STALE_BUNDLE_REL}，已清理：它在场时 dev 会把控制权转交给 bundle ` +
      "里的 app 并静默带走 Vite（HMR 失效而页面照常显示）。要跑 bundle 请用 scripts/build-and-run.sh。",
  );
  io.rmSync(dir, { recursive: true, force: true });
  return true;
}
