/**
 * 创作看板「日历」视图的纯函数（PRD-19-040）：排出一个月的格子、在落点 id 与日期之间互转。
 *
 * 哪一天是计划发布日期由 Rust 判定下发（`ProjectCardDto.publish_date`，ISO 日历日）；这里只管
 * 呈现几何——一个月排成几行、每格是哪一天，写日期仍走 `creator_set_publish_date`（ADR-003）。
 */

/** 日历格子落点 id 前缀（`data-drop-id` 是全应用共用的落点属性）。 */
const DATE_DROP_PREFIX = "creator-date:";
/** 「未排期」那一栏的落点：拖到这里 = 清掉排期。 */
export const UNSCHEDULED_DROP_ID = `${DATE_DROP_PREFIX}none`;

/** 月历上的一格。 */
export interface CalendarDay {
  /** `YYYY-MM-DD`。 */
  iso: string;
  day: number;
  /** 属于正在看的那个月（首尾补齐的邻月日子为 `false`）。 */
  inMonth: boolean;
  today: boolean;
}

/** 正在看的月份（`month` 从 0 起）。 */
export interface CalendarMonth {
  year: number;
  month: number;
}

const pad = (value: number) => String(value).padStart(2, "0");

/** 本地日期 → `YYYY-MM-DD`（按本地日历日，不经 UTC）。 */
export function isoDay(date: Date): string {
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

/** 一周从哪天开始：英语界面从周日，其余从周一。 */
export function weekStartsOn(locale: string): 0 | 1 {
  return locale.toLowerCase().startsWith("en") ? 0 : 1;
}

/** 某个日期所在的月份。 */
export function monthOf(date: Date): CalendarMonth {
  return { year: date.getFullYear(), month: date.getMonth() };
}

/** 往前 / 往后翻几个月。 */
export function shiftMonth(current: CalendarMonth, delta: number): CalendarMonth {
  const date = new Date(current.year, current.month + delta, 1);
  return monthOf(date);
}

/** 一个月排成整周：首行从这个月第一天所在的那一周开始，末行补到周末。 */
export function monthWeeks(current: CalendarMonth, weekStart: 0 | 1, todayIso: string): CalendarDay[][] {
  const first = new Date(current.year, current.month, 1);
  const lead = (first.getDay() - weekStart + 7) % 7;
  const daysInMonth = new Date(current.year, current.month + 1, 0).getDate();
  const cells = Math.ceil((lead + daysInMonth) / 7) * 7;
  const weeks: CalendarDay[][] = [];
  for (let index = 0; index < cells; index += 1) {
    const date = new Date(current.year, current.month, 1 - lead + index);
    const iso = isoDay(date);
    if (index % 7 === 0) weeks.push([]);
    weeks[weeks.length - 1].push({
      iso,
      day: date.getDate(),
      inMonth: date.getMonth() === current.month,
      today: iso === todayIso,
    });
  }
  return weeks;
}

/** 日期 → 那一格的落点 id。 */
export function dateDropId(iso: string): string {
  return `${DATE_DROP_PREFIX}${iso}`;
}

/**
 * 落点 id → 要写的日期：某一天为 `YYYY-MM-DD`，「未排期」为 `null`（清掉），不是日历落点为
 * `undefined`（松手即取消）。
 */
export function dateOfDropId(dropId: string): string | null | undefined {
  if (dropId === UNSCHEDULED_DROP_ID) return null;
  if (!dropId.startsWith(DATE_DROP_PREFIX)) return undefined;
  const iso = dropId.slice(DATE_DROP_PREFIX.length);
  return /^\d{4}-\d{2}-\d{2}$/.test(iso) ? iso : undefined;
}
