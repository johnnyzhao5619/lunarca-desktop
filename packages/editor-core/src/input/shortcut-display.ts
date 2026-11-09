/**
 * 快捷键展示（单一来源，PRD-09-003）：把注册表键值串（如 "cmd_or_ctrl+shift+r"）转为平台习惯的可读符号。
 * 桌面的设置页录制器与各处快捷键提示、桌面与网页共用的格式工具条都从这里取（TDD-17 §2）。
 *
 * 双击型快捷键（PRD-09 §2.6 双击键）：值带 `double_tap:` 前缀（如 `double_tap:cmd_or_ctrl+c`），语法与
 * Rust `luna_config::shortcut::DOUBLE_TAP_PREFIX` 对齐。
 */

export const isMacPlatform =
  typeof window !== "undefined" && /Mac|iPhone|iPod|iPad/i.test(navigator.userAgent);

/** 双击型快捷键值前缀（与 Rust `DOUBLE_TAP_PREFIX` 同语法）。 */
export const DOUBLE_TAP_PREFIX = "double_tap:";

/** 解析后的快捷键值：是否双击型 + 组合词元。 */
export interface ShortcutValue {
  doubleTap: boolean;
  parts: string[];
}

/** 拆解设置值（含 `double_tap:` 前缀识别；空串 → 空词元）。 */
export function parseShortcutValue(value: string): ShortcutValue {
  const doubleTap = value.startsWith(DOUBLE_TAP_PREFIX);
  const combo = doubleTap ? value.slice(DOUBLE_TAP_PREFIX.length) : value;
  return { doubleTap, parts: parseShortcut(combo) };
}

export const keyDisplayMap: Record<string, string> = {
  cmd_or_ctrl: isMacPlatform ? "⌘" : "Ctrl",
  cmd: "⌘",
  ctrl: "Ctrl",
  shift: "⇧",
  alt: "⌥",
  option: "⌥",
  enter: "↵",
  space: "Space",
  backspace: "⌫",
  delete: "⌦",
  escape: "Esc",
  esc: "Esc",
  up: "↑",
  down: "↓",
  left: "←",
  right: "→",
  tab: "⇥",
  // 标点类主键：设置里存的是键名（与 Rust `MAIN_KEYS` 同一套），展示成键帽上印的那个符号——
  // 不写就会大写回显成 `⌘COMMA`（W3 真机发现：⌘, 改成走设置后就这样显示）。
  minus: "-",
  equal: "=",
  bracketleft: "[",
  bracketright: "]",
  backslash: "\\",
  semicolon: ";",
  quote: "'",
  comma: ",",
  period: ".",
  slash: "/",
  backquote: "`",
};

/** 拆分键值串为词元（空串 → []）。 */
export function parseShortcut(value: string): string[] {
  if (!value) return [];
  return value
    .split("+")
    .map((part) => part.trim())
    .filter(Boolean);
}

/** 单个词元 → 展示符号（未知词元大写回显）。 */
export function displayKey(part: string): string {
  return keyDisplayMap[part] ?? part.toUpperCase();
}

/** 整串键值 → 展示串（macOS 习惯无分隔符，其余平台以 + 连接）。空值返回空串。
 *  双击型（`double_tap:` 前缀）追加 `×2` 后缀（locale 中性；设置页录制器另用 i18n 徽标）。 */
export function formatShortcut(value: string): string {
  const { doubleTap, parts } = parseShortcutValue(value);
  const keys = parts.map(displayKey);
  if (keys.length === 0) return "";
  const combo = keys.join(isMacPlatform ? "" : "+");
  return doubleTap ? `${combo} ×2` : combo;
}
