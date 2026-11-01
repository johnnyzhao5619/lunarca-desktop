/**
 * 记录收件箱的**时间桶分组**（PRD-06-058 A，Apple Notes / Drafts 对标）——纯函数。
 *
 * 为什么分组而不是一列平铺：收件箱的问题不是"按什么排"，而是"哪些该现在处理"。
 * 时间桶把这个判断摆在标题上，用户扫一眼就知道「更早」那一段是欠账。
 *
 * **这不是业务判定，是呈现分组**（ADR-003 边界）：成熟度、来源、时龄警示三件事都由
 * Rust 判完下发（`DraftNoteDto.quick_note` / `.stale`），本模块只把已有的时间戳按日历
 * 边界摊成四段——那是一件纯粹的显示排版，且必须按**用户本地时区**算，故只能在这里做。
 *
 * `now` 由调用方注入，故本模块可无头测试（`Date.now` 不入实现，同 `format-relative-time`）。
 */

/** 桶标识；也是 i18n 键的末段（`drafts.bucket.<id>`）。 */
import { bucketByTime, bucketOf, type TimeBucket, type TimeBucketId } from "../../lib/time-buckets";

/** 速记流的桶就是共享的时间桶（PRD-06-058 A）；这里只保留原名，判定归 `lib/time-buckets.ts`。 */
export type DraftBucketId = TimeBucketId;
export type DraftBucket<T> = TimeBucket<T>;
export { bucketOf };

export function bucketDrafts<T extends { modified_ms: number | null }>(
  items: readonly T[],
  now: number,
): DraftBucket<T>[] {
  return bucketByTime(items, (item) => item.modified_ms, now);
}
