/**
 * 快捷键展示工具（单一来源）：把注册表键值串（如 "cmd_or_ctrl+shift+r"）
 * 转为平台习惯的可读符号。设置页录制器与各页面快捷键提示共用（DRY）。
 *
 * 双击型快捷键（PRD-09 §2.6 双击键）：值带 `double_tap:` 前缀（如
 * `double_tap:cmd_or_ctrl+c`），语法与 Rust `luna_config::shortcut::DOUBLE_TAP_PREFIX` 对齐。
 * 任意快捷键（全局 / 应用内）都可切到双击模式；能力不由前端按 key 判定。
 */

export const isMacPlatform =
  typeof window !== "undefined" && /Mac|iPhone|iPod|iPad/i.test(navigator.userAgent);

/** 双击型快捷键值前缀（与 Rust `DOUBLE_TAP_PREFIX` 同语法）。 */
export const DOUBLE_TAP_PREFIX = "double_tap:";

/**
 * **首屏三键**：引导的「快捷键」一步与启动概览的速查表列的是同一组，按同一顺序。
 *
 * 此前它是 `ShortcutsStep` 里的一个导出常量，注释写着"与收尾摘要、概览速查表同一组
 * （三处不各挑各的）"——而概览那边其实是自己又手写了一遍同样的三个 key。**注释声称的
 * 共用并不存在**，两处要改哪三个键全靠人记得同时改。放在这里是因为两边都已经从本模块
 * 取展示规约（`formatShortcut`），共用它不新增任何跨 feature 依赖边。
 *
 * 各自的标签仍归各自的 i18n：引导列的是设置项标签（那一行可编辑），概览列的是它自己的
 * 速查文案——共用的是"哪三个键、什么顺序"，不是怎么称呼它们。
 */
export const HEADLINE_SHORTCUT_KEYS = [
  "shortcuts.popup_translate",
  "shortcuts.voice_input",
  "shortcuts.quick_chat",
] as const;

/** 保留编辑主键（Cmd/Ctrl + 之一，无额外修饰键时仅允许双击模式，与 Rust 判定对齐）。 */
const RESERVED_EDITING_KEYS = new Set(["c", "v", "x", "a", "z"]);

/**
 * 组合是否为「保留编辑键」（Cmd/Ctrl + 单编辑键，无 Shift/Alt）。普通模式作全局热键会劫持
 * 系统复制/粘贴等，故这些组合只允许双击模式（与 Rust `is_reserved_editing_combo` 对齐）。
 */
export function isReservedEditingCombo(parts: string[]): boolean {
  const modifiers = parts.filter((p) => p === "cmd_or_ctrl" || p === "shift" || p === "alt");
  const keys = parts.filter((p) => p !== "cmd_or_ctrl" && p !== "shift" && p !== "alt");
  return (
    modifiers.length === 1 &&
    modifiers[0] === "cmd_or_ctrl" &&
    keys.length === 1 &&
    RESERVED_EDITING_KEYS.has(keys[0])
  );
}

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
