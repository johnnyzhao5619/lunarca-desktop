/** Rust 拦下应用主动重启时的稳定错误码（安装更新、重置应用共用，TDD-13 §5）。 */
export const RESTART_BLOCK_CODES = [
  "restart_blocked_capture",
  "restart_save_failed",
  "restart_save_timeout",
] as const;

export type RestartBlockCode = (typeof RESTART_BLOCK_CODES)[number];

export function isRestartBlockCode(raw: string): raw is RestartBlockCode {
  return (RESTART_BLOCK_CODES as readonly string[]).includes(raw);
}
