/**
 * 编辑器的固定键位（TDD-07 §4.2）：键位在这里定义一次，CM6 keymap（经 `codeMirrorKey`）与右键菜单、
 * 格式工具栏的提示都从本表派生。桌面与网页同一张表——两个宿主上同一个动作按同一枚键。
 *
 * 可改键的整篇动作（⌘E / ⌘J）不在这里：那是宿主的设置项，由宿主的命令注册表管。
 * 每一枚键都要登记在 Rust 的编辑器保留表里（`EDITOR_RESERVED_COMBOS`），桌面的命令注册表用例钉住这一点。
 */

/** 编辑器作用域命令的 id。 */
export type EditorCommandId =
  | "save"
  | "bold"
  | "italic"
  | "insert_link"
  | "insert_table"
  | "heading_0"
  | "heading_1"
  | "heading_2"
  | "heading_3"
  | "heading_4"
  | "heading_5"
  | "heading_6"
  | "tab_focus_mode";

export const EDITOR_SHORTCUTS: Readonly<Record<EditorCommandId, string>> = {
  save: "cmd_or_ctrl+s",
  bold: "cmd_or_ctrl+b",
  italic: "cmd_or_ctrl+i",
  insert_link: "cmd_or_ctrl+k",
  insert_table: "cmd_or_ctrl+alt+t",
  // Typora 风格段落快捷键：⌘0 正文、⌘1–⌘6 标题（PRD-06-014）。
  heading_0: "cmd_or_ctrl+0",
  heading_1: "cmd_or_ctrl+1",
  heading_2: "cmd_or_ctrl+2",
  heading_3: "cmd_or_ctrl+3",
  heading_4: "cmd_or_ctrl+4",
  heading_5: "cmd_or_ctrl+5",
  heading_6: "cmd_or_ctrl+6",
  // 「Tab 键移动焦点」开关（VS Code 同义，Windows 为 Ctrl+M）：⌘⇧M。不用 CodeMirror 在 macOS 上
  // 公布的 ⇧⌥M——⌥ 加字母在 mac 上是输入字符（⇧⌥M = Â），经拼音输入法直接上屏，真机按下去
  // 打出一个「Â」而不切换（2026-10-06）。开关本身是宿主会话级的那一份（`EditorHost.toggleTabFocusMode`）。
  tab_focus_mode: "cmd_or_ctrl+shift+m",
};

export function editorShortcut(id: EditorCommandId): string {
  return EDITOR_SHORTCUTS[id];
}

/** 键值串里的修饰键记号（`cmd_or_ctrl+shift+f` 这种写法）；其余部分是主键。 */
export const SHORTCUT_MODIFIER_TOKENS: ReadonlySet<string> = new Set(["cmd_or_ctrl", "shift", "alt"]);

/**
 * 注册表键值串 → CodeMirror keymap 的键名（`cmd_or_ctrl+alt+t` → `Mod-Alt-t`）。
 *
 * CM 的 `Mod` 就是 macOS 的 ⌘ / 其余平台的 Ctrl，与 `cmd_or_ctrl` 同义；修饰键顺序
 * 按 CM 文档的 `Shift-Alt-Mod` 惯例无关紧要，但这里固定输出顺序以便测试断言稳定。
 */
export function codeMirrorKey(value: string): string {
  const parts = value
    .split("+")
    .map((part) => part.trim())
    .filter(Boolean);
  const modifiers: string[] = [];
  if (parts.includes("cmd_or_ctrl")) {
    modifiers.push("Mod");
  }
  if (parts.includes("alt")) {
    modifiers.push("Alt");
  }
  if (parts.includes("shift")) {
    modifiers.push("Shift");
  }
  const keys = parts.filter((part) => !SHORTCUT_MODIFIER_TOKENS.has(part));
  return [...modifiers, ...keys].join("-");
}
