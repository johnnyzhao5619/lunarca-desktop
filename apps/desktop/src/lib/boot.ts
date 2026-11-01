/**
 * 首帧外观快照（PRD-14-066 / ADR-114 ④）。
 *
 * Rust 建主窗时经 `initialization_script` 注入 `window.__LUNA_BOOT__`——它在文档解析
 * 之前就把 `data-theme` 等挂到了 `<html>` 上；这里只是把同一份快照读出来，供 i18n 初始化
 * 与 React 挂载前的 `applyTheme` 取用，让首帧与 `useAppearance` 稳态读回的值一致。
 *
 * 字段名与 `src-tauri/src/boot_appearance.rs::BootAppearance` 一一对应（serde 默认
 * snake_case）。缺席（非 Tauri 宿主、测试）时返回 `null`，调用方按既有默认走。
 */
export interface BootAppearance {
  theme: string;
  theme_family: string;
  installed_theme_css: string | null;
  language: string;
  reduce_motion: boolean;
  high_contrast: boolean;
  /** 百分比整数（100 = 1×）。 */
  font_scale: number;
  /** `compact` / `comfortable`（Rust 已回落）。 */
  density: string;
}

declare global {
  interface Window {
    __LUNA_BOOT__?: BootAppearance;
  }
}

export function readBootAppearance(): BootAppearance | null {
  if (typeof window === "undefined") {
    return null;
  }
  const boot = window.__LUNA_BOOT__;
  if (!boot || typeof boot !== "object") {
    return null;
  }
  return boot;
}
