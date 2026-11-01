/**
 * 按本地时间分桶（今天 / 昨天 / 本周 / 更早）：速记流（PRD-06-058 A）与录音 / 转写清单的
 * 时间分组（PRD-07-007）共用这一份——两处答的是同一个问题"这条是什么时候的"。
 *
 * 纯函数、零 IPC：分桶只是把 Rust 已排好序的列表切成几段，不改顺序、不算业务（ADR-003）。
 */

export type TimeBucketId = "today" | "yesterday" | "thisWeek" | "earlier";

export interface TimeBucket<T> {
  id: TimeBucketId;
  items: T[];
}

const BUCKET_ORDER: readonly TimeBucketId[] = ["today", "yesterday", "thisWeek", "earlier"];

const DAY_MS = 24 * 60 * 60 * 1000;

/**
 * 该时刻所在**本地自然日**的零点。
 *
 * 导出而不是私有：通知历史的同类折叠要按"同一天"归组（跨天的重复上报不该并成一行，
 * 那会让"这件事什么时候发生的"无从回答），问的是与分桶同一个"本地日"，判据只能有一处。
 */
export function startOfLocalDay(ms: number): number {
  const date = new Date(ms);
  date.setHours(0, 0, 0, 0);
  return date.getTime();
}

function startOfLocalWeek(ms: number): number {
  const day = new Date(ms).getDay(); // 0 = 周日
  const mondayOffset = (day + 6) % 7;
  return startOfLocalDay(ms) - mondayOffset * DAY_MS;
}

/** `ms` 为空（没记时间）一律归「更早」——不猜一个时间。 */
export function bucketOf(ms: number | null, now: number): TimeBucketId {
  if (ms === null) {
    return "earlier";
  }
  const today = startOfLocalDay(now);
  if (ms >= today) {
    return "today";
  }
  if (ms >= today - DAY_MS) {
    return "yesterday";
  }
  if (ms >= startOfLocalWeek(now)) {
    return "thisWeek";
  }
  return "earlier";
}

/** 按 `timeOf` 取出的时间分桶；桶内保持传入顺序，空桶不出现。 */
export function bucketByTime<T>(
  items: readonly T[],
  timeOf: (item: T) => number | null,
  now: number,
): TimeBucket<T>[] {
  const byBucket = new Map<TimeBucketId, T[]>();
  for (const item of items) {
    const id = bucketOf(timeOf(item), now);
    const bucket = byBucket.get(id);
    if (bucket) {
      bucket.push(item);
    } else {
      byBucket.set(id, [item]);
    }
  }
  return BUCKET_ORDER.flatMap((id) => {
    const bucketItems = byBucket.get(id);
    return bucketItems ? [{ id, items: bucketItems }] : [];
  });
}
