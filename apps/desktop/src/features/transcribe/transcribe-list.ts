/**
 * 转写列表的排序选项与时长呈现（PRD-07-007 / TDD-07 §4.1a）。
 *
 * **排序与筛选不在这里了**：它们随查询下发给 Rust，在 SQL 层完成。此前这里有一份
 * `filterAndSortTranscripts`——留着它就是第二套排序事实源，而它排的只是**已加载的那几页**：
 * 「按时长排序」实际是「已加载的条目里最长的」，「筛选无结果」实际是「已加载的那几页里
 * 没有」。两句话都错，且两句都不会有任何东西变红。
 *
 * 本文件只剩两样纯呈现：排序控件的选项表、时长徽标的格式化。
 */

import type { ListSortDto } from "../../ipc/bindings";
import { formatClock } from "../../lib/format-duration";

/**
 * 排序键 → 选项文案 key。
 *
 * 用 `Record<ListSortDto, …>` 而不是一个数组字面量：排序键的事实源是 Rust 的
 * `ListSort`，这里少一格当场编译不过——加了新排序键却忘了给它文案，不会静默漏在界面上。
 */
const SORT_LABEL_KEYS: Record<ListSortDto, string> = {
  modified: "sort.modified",
  name: "sort.name",
  duration: "sort.duration",
  created: "sort.created",
};

/** 排序控件的选项（顺序即呈现顺序）。 */
export const TRANSCRIBE_SORT_OPTIONS = Object.entries(SORT_LABEL_KEYS) as [ListSortDto, string][];

/** 默认「最近修改」——转写是持续产出的流水，最新的最常被打开。 */
export const DEFAULT_TRANSCRIBE_SORT: ListSortDto = "modified";

/** 录音清单默认按「录于何时」排（PRD-07-007）：它的身份时间是录制时间，改一次标题不该把它顶到最前。 */
export const DEFAULT_RECORDING_SORT: ListSortDto = "created";

/**
 * 时长徽标：0 或未知返回 `null`，调用方据此**不渲染**徽标——"不知道多长"和"0:00"是
 * 两件事，格式化不该替它撒谎。形状本身走全应用唯一的时钟式格式化。
 */
export function formatListDuration(durationMs: number | null): string | null {
  if (durationMs === null || !Number.isFinite(durationMs) || durationMs <= 0) {
    return null;
  }
  return formatClock(durationMs);
}

/**
 * 该排序档下用来做时间分组的时间（PRD-07-007）：按录制 / 创建时间排就按它分桶（没记的老笔记
 * 退到修改时间，与 Rust 的排序值同一口径），按最近修改排就按修改时间；按名称 / 时长排不分组。
 */
export function timeGroupingFor(
  sort: ListSortDto,
): ((item: { created_ms: number | null; modified_ms: number | null }) => number | null) | undefined {
  switch (sort) {
    case "created":
      return (item) => item.created_ms ?? item.modified_ms;
    case "modified":
      return (item) => item.modified_ms;
    default:
      return undefined;
  }
}

/**
 * 清单行上显示的那个时间：与这一档的分组同一个时间（按最近修改排，行上就写修改时间），按名称 /
 * 时长排时退到身份时间（录制 / 创建，没记的老笔记退到修改）。此前行上恒写创建时间，而转写默认
 * 按最近修改分组——「今天」分组下的一行写着「昨天」（真机 2026-10-06）。
 */
export function rowTimeFor(
  sort: ListSortDto,
): (item: { created_ms: number | null; modified_ms: number | null }) => number | null {
  return timeGroupingFor(sort) ?? ((item) => item.created_ms ?? item.modified_ms);
}

