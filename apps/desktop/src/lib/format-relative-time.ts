/**
 * 相对时间格式化（「3 分钟前 / 昨天 / 2026-07-18」）。
 *
 * 走 `Intl.RelativeTimeFormat` 而非自己拼中文——本地化由运行时按 locale 负责，**不新增
 * 一套要逐语言维护的 i18n 键**（同 `toLocaleDateString` 的取舍）。
 *
 * `now` 由调用方注入，故本函数是纯函数、可 headless 测试（Date.now 不入实现）。
 */

const SECOND = 1_000;
const MINUTE = 60 * SECOND;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;
/** 超过这个跨度改显绝对日期——「23 天前」不如「07-02」好定位。 */
const ABSOLUTE_AFTER = 7 * DAY;

/**
 * `ms` = epoch 毫秒（`null`/非有限/非正 → 空串，调用方据此不渲染该行，绝不显示 "Invalid Date"）。
 * 未来时间（时钟回拨/跨设备时间差）夹到 0，显示为「刚刚」而不是「3 分钟后」。
 */
export function formatRelativeTime(
  ms: number | null | undefined,
  now: number,
  locale: string,
): string {
  if (ms == null || !Number.isFinite(ms) || ms <= 0) {
    return "";
  }
  const diff = Math.max(0, now - ms);
  if (diff >= ABSOLUTE_AFTER) {
    return new Date(ms).toLocaleDateString(locale);
  }
  const rtf = new Intl.RelativeTimeFormat(locale, { numeric: "auto" });
  if (diff < MINUTE) {
    return rtf.format(-Math.floor(diff / SECOND), "second");
  }
  if (diff < HOUR) {
    return rtf.format(-Math.floor(diff / MINUTE), "minute");
  }
  if (diff < DAY) {
    return rtf.format(-Math.floor(diff / HOUR), "hour");
  }
  return rtf.format(-Math.floor(diff / DAY), "day");
}
