/**
 * 金额展示格式化。
 *
 * 货币符号与小数位交给 `Intl`，不在 JSX 里写 `$` 和 `toFixed(3)`：前者是硬编码文案
 * （中文界面照样吐美元符号，且位置在各 locale 里并不一致），后者给出的 `$0.000` 既不是
 * 任何货币的写法，也把"估算"伪装成了精确到毫的账面数字。
 *
 * 币种由数据契约给定（`estimated_savings_usd` 是 USD），不由界面猜。
 */

/** 小额估算：低于 1 元的部分仍要看得见，故允许两位有效小数。 */
export function formatUsd(value: number, locale: string): string {
  return new Intl.NumberFormat(locale, {
    style: "currency",
    currency: "USD",
    minimumFractionDigits: 2,
    maximumFractionDigits: value !== 0 && Math.abs(value) < 0.01 ? 4 : 2,
  }).format(value);
}

/**
 * 服务端价目的展示价：`amount` 是**货币最小单位**（美分 / 欧分；日元这类零小数币种就是它本身），
 * `currency` 是服务端给的 ISO 4217 代码（Stripe 原文小写）。
 *
 * 小数位不由界面写死成 2：换算用的是 `Intl` 对这个币种自己的小数位（USD 2、JPY 0），
 * 否则 ¥500 会被显示成 ¥5。币种认不出时 `Intl` 抛 `RangeError`——那时如实写出数额与代码，
 * 而不是让整块价目崩掉。
 */
export function formatMinorUnits(amount: number, currency: string, locale: string): string {
  const code = currency.toUpperCase();
  try {
    const format = new Intl.NumberFormat(locale, { style: "currency", currency: code });
    const digits = format.resolvedOptions().maximumFractionDigits ?? 2;
    return format.format(amount / 10 ** digits);
  } catch {
    return `${amount.toLocaleString(locale)} ${code}`;
  }
}
