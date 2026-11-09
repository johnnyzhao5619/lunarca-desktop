/**
 * 空闲锁定的时长（TDD-17 §4、PRD-21-008）：没勾「保持登录」的会话默认 30 分钟，勾了的默认不锁；
 * 「账户与会话」页可改为 15 分钟 / 1 小时 / 4 小时 / 从不。设置只存在这个浏览器里（不含任何内容）。
 */

export const IDLE_LOCK_SETTINGS = ["auto", "15m", "1h", "4h", "never"] as const;
export type IdleLockSetting = (typeof IDLE_LOCK_SETTINGS)[number];

/** localStorage 键（TDD-15 §8.2 的 `luna:<feature>-<sub>` 形状）。 */
export const IDLE_LOCK_KEY = "luna:web-idle-lock";

const MINUTE = 60_000;
const DURATIONS: Record<Exclude<IdleLockSetting, "auto" | "never">, number> = {
  "15m": 15 * MINUTE,
  "1h": 60 * MINUTE,
  "4h": 240 * MINUTE,
};
const DEFAULT_UNKEPT_MS = 30 * MINUTE;

export function readIdleLockSetting(storage: Pick<Storage, "getItem">): IdleLockSetting {
  const value = storage.getItem(IDLE_LOCK_KEY);
  return (IDLE_LOCK_SETTINGS as readonly string[]).includes(value ?? "") ? (value as IdleLockSetting) : "auto";
}

export function writeIdleLockSetting(storage: Pick<Storage, "setItem" | "removeItem">, setting: IdleLockSetting): void {
  if (setting === "auto") storage.removeItem(IDLE_LOCK_KEY);
  else storage.setItem(IDLE_LOCK_KEY, setting);
}

/** 这次会话空闲多久锁定；`null` = 不锁。 */
export function idleLockMs(setting: IdleLockSetting, keepSignedIn: boolean): number | null {
  if (setting === "never") return null;
  if (setting === "auto") return keepSignedIn ? null : DEFAULT_UNKEPT_MS;
  return DURATIONS[setting];
}
