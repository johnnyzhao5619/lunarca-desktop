/**
 * 绝对日期时间与整数的本地化格式化——**全应用唯一入口**。
 *
 * **`locale` 必须由调用方注入（来自 `i18n.language`），绝不省略**：七处此前直接写
 * `new Date(ms).toLocaleString()`，那读的是**系统 locale**——用户把界面切成英文、系统还是
 * 中文时，回收站里每一行的删除时间仍是中文格式。界面语言与时间格式属于同一件事，不该有
 * 两个来源（TDD-11 §1.4b）。
 *
 * 与 `format-relative-time` 同一取舍：本地化交给 `Intl`，**不新增一套要逐语言维护的
 * i18n 键**。`now` / `locale` 由调用方注入，故这里全是纯函数、可 headless 测试。
 *
 * 体积不在此列：`formatBytes` 曾寄居在本文件里（名不副实，且与 `lib/format-bytes.ts`
 * 那份口径相左），现已并入 [`lib/format-bytes`](./format-bytes.ts) 唯一一份。
 */

/** epoch 毫秒 → 本地化的「日期 + 时间」。非有限/非正 → 空串（调用方据此不渲染那一行）。 */
export function formatDateTime(
  ms: number | string | null | undefined,
  locale: string,
): string {
  const value = typeof ms === "string" ? Number.parseInt(ms, 10) : ms;
  if (value == null || !Number.isFinite(value) || value <= 0) {
    return "";
  }
  return new Date(value).toLocaleString(locale);
}

/**
 * 云侧时刻 → 本地化的「日期 + 时间」。
 *
 * 与 [`formatDateTime`] 的区别只在**输入单位**：服务端下发的是 Unix **秒**（`lease_expires_at`）
 * 或 RFC 3339 **字符串**（`DeviceDto.last_seen_at`），而后者按毫秒整数解析会把
 * `"2026-09-10T…"` 读成 2026 毫秒。认不出的值返回空串，调用方据此不渲染那一行。
 */
export function formatInstant(
  value: number | string | null | undefined,
  locale: string,
): string {
  if (value == null) return "";
  const ms = typeof value === "number" ? value * 1000 : Date.parse(value);
  return Number.isFinite(ms) && ms > 0 ? new Date(ms).toLocaleString(locale) : "";
}

/** 毫秒时间戳 → 本地化**日期**（清单行副信息用：按修改时间排序的浏览场景里，
    精确到秒的时刻是噪音；完整时刻由调用方按需放进 `title`）。 */
export function formatDate(ms: number | string | null | undefined, locale: string): string {
  const value = typeof ms === "string" ? Number.parseInt(ms, 10) : ms;
  if (value == null || !Number.isFinite(value) || value <= 0) {
    return "";
  }
  return new Date(value).toLocaleDateString(locale);
}

/** 整数 → 本地化千分位（用量统计）。非有限 → `"0"`，而不是 `"NaN"`。 */
export function formatCount(value: number | string | null | undefined, locale: string): string {
  const n = typeof value === "string" ? Number.parseInt(value, 10) : value;
  return n != null && Number.isFinite(n) ? n.toLocaleString(locale) : "0";
}

/** `YYYY-MM-DD`（日历日，无时区）→ 本地时区的那一天零点；认不出 → `null`。 */
function localDay(iso: string | null | undefined): Date | null {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(iso ?? "");
  if (!match) return null;
  const date = new Date(Number(match[1]), Number(match[2]) - 1, Number(match[3]));
  return date.getMonth() === Number(match[2]) - 1 ? date : null;
}

/**
 * 日历日（`YYYY-MM-DD`，如计划发布日期）→ 本地化「月 日」（`10月24日` / `Oct 24`）。
 * 按本地日历日解析，不经 UTC——否则西半球会显示成前一天。认不出 → 空串。
 */
export function formatMonthDay(iso: string | null | undefined, locale: string): string {
  const date = localDay(iso);
  return date ? date.toLocaleDateString(locale, { month: "short", day: "numeric" }) : "";
}

/** 月历的标题：本地化「年 月」（`2026年10月` / `October 2026`）。`month` 从 0 起。 */
export function formatMonthTitle(year: number, month: number, locale: string): string {
  return new Date(year, month, 1).toLocaleDateString(locale, { year: "numeric", month: "long" });
}

/** 一周七天的短名，从 `weekStartsOn`（0 = 周日，1 = 周一）起排。 */
export function weekdayNames(locale: string, weekStartsOn: 0 | 1): string[] {
  // 2026-10-04 是周日：从它起连取七天，名字交给 Intl。
  return Array.from({ length: 7 }, (_, index) =>
    new Date(2026, 9, 4 + weekStartsOn + index).toLocaleDateString(locale, { weekday: "short" }),
  );
}
