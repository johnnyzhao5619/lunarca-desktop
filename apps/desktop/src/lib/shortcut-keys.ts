/**
 * 「用户按下的到底是哪个键」（PRD-09-003 / TDD-05 §1.2）：录快捷键与判定应用内快捷键**共用这一份**。
 * 两边各认各的，就会出现录得进、按不出的键（W3 前 `⌘\` 录进去存成 `backslash`，判定却拿
 * `event.key` 的 `\` 去比，永远对不上）。
 *
 * 判据是物理键位（`event.code`），不是 `event.key`：后者是当前布局与修饰键共同产出的字符，
 * 按住 Option 的 `e` 在 macOS 上是死键（`"Dead"`），空格的 `event.key` 是一个空格字符，
 * 非美式布局上的标点各不相同。这类字符串过得了语法校验，却在注册时失败。
 *
 * 这里只认词表之内的键，认不出就明说认不出（调用方提示重按），绝不把注册不上的名字写进
 * 设置。词表与 Rust `luna_config::shortcut::MAIN_KEYS` 逐字对齐，由两边各一条用例钉住。
 */

/** 具名键：物理键位 → 注册表主键名。 */
const CODE_TO_KEY: Record<string, string> = {
  Space: "space",
  Enter: "enter",
  NumpadEnter: "enter",
  Tab: "tab",
  Escape: "escape",
  Backspace: "backspace",
  Delete: "delete",
  ArrowUp: "up",
  ArrowDown: "down",
  ArrowLeft: "left",
  ArrowRight: "right",
  Home: "home",
  End: "end",
  PageUp: "pageup",
  PageDown: "pagedown",
  Insert: "insert",
  Minus: "minus",
  Equal: "equal",
  BracketLeft: "bracketleft",
  BracketRight: "bracketright",
  Backslash: "backslash",
  Semicolon: "semicolon",
  Quote: "quote",
  Comma: "comma",
  Period: "period",
  Slash: "slash",
  Backquote: "backquote",
};

/** 只按下了修饰键——还没按到主键，录制继续等着。 */
const MODIFIER_KEYS = new Set(["Control", "Shift", "Alt", "Meta"]);

export function isModifierKey(key: string): boolean {
  return MODIFIER_KEYS.has(key);
}

/**
 * 这一下按的是哪个主键。
 *
 * - 返回字符串 = 认得出，可以写进设置；
 * - 返回 `null` = 这是个修饰键或**认不出的键**，两者的处理不同，由调用方按
 *   [`isModifierKey`] 区分。
 */
export function mainKeyOf(event: Pick<KeyboardEvent, "code" | "key">): string | null {
  if (isModifierKey(event.key)) {
    return null;
  }
  const code = event.code;
  if (/^Key[A-Z]$/.test(code)) {
    return code.slice(3).toLowerCase();
  }
  if (/^Digit[0-9]$/.test(code)) {
    return code.slice(5);
  }
  // F1…F20（词表上限：F20 以上在 macOS 没有虚拟键码，双击手势绑不上）。
  if (/^F([1-9]|1[0-9]|20)$/.test(code)) {
    return code.toLowerCase();
  }
  return CODE_TO_KEY[code] ?? null;
}

/**
 * **首屏三键**：引导的「快捷键」一步与启动概览的速查表列的是同一组，按同一顺序。
 *
 * 此前引导与概览各写一遍同样的三个 key，要改哪三个键全靠人记得同时改。各自的标签仍归各自的 i18n：
 * 引导列的是设置项标签（那一行可编辑），概览列的是它自己的速查文案——共用的是"哪三个键、什么顺序"，
 * 不是怎么称呼它们。
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
