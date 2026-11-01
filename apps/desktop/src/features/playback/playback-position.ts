/**
 * 每篇的播放位置跨重启记住（PRD-04-068）——纯判据，可无头测试。
 *
 * - 存：`timeupdate` 每 {@link PERSIST_INTERVAL_MS} 写一次（位置至少变了 {@link PERSIST_MIN_DELTA_MS}），
 *   暂停 / 换篇 / 播完立刻写；播完或离结尾不到 {@link NEAR_END_MS} 时存 0——下次从头。
 * - 取：装载时只**定位**不起播（PRD-04-064 ⑦：起播必须是显式意图）。
 */

/** 播放中两次写盘的最小间隔。 */
export const PERSIST_INTERVAL_MS = 5_000;
/** 位置变化小于它不写（暂停时 `timeupdate` 仍会零星到达）。 */
export const PERSIST_MIN_DELTA_MS = 1_000;
/** 离结尾不到这么多就算听完。 */
export const NEAR_END_MS = 3_000;

export interface PersistMark {
  atMs: number;
  positionMs: number;
}

/** 这一次 `timeupdate` 要不要写盘。 */
export function shouldPersist(last: PersistMark | null, atMs: number, positionMs: number): boolean {
  if (last === null) return true;
  return (
    atMs - last.atMs >= PERSIST_INTERVAL_MS &&
    Math.abs(positionMs - last.positionMs) >= PERSIST_MIN_DELTA_MS
  );
}

/** 该存哪个值：听完（或几乎听完）存 0，否则存当前位置。 */
export function positionToStore(positionMs: number, durationMs: number): number {
  if (positionMs <= 0) return 0;
  if (durationMs > 0 && durationMs - positionMs <= NEAR_END_MS) return 0;
  return Math.floor(positionMs);
}
