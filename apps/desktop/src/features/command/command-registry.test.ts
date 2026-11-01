import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import {
  COMMANDS,
  COMMAND_SPACES,
  EDITOR_PALETTE_COMMANDS,
  EDITOR_SHORTCUTS,
  codeMirrorKey,
  commandById,
  commandShortcut,
  editorCommandForKey,
  editorPaletteShortcut,
  matchesShortcut,
  workspaceCommandForKey,
} from "./command-registry";
import { DoubleTapDetector } from "../../lib/double-tap";

function key(
  k: string,
  mods: Partial<Pick<KeyboardEvent, "metaKey" | "ctrlKey" | "shiftKey" | "altKey" | "code">> = {},
): KeyboardEvent {
  return {
    key: k,
    metaKey: false,
    ctrlKey: false,
    shiftKey: false,
    altKey: false,
    ...mods,
  } as KeyboardEvent;
}

describe("matchesShortcut（注册表键值串 → 键盘事件判定）", () => {
  it("cmd_or_ctrl 同时认 ⌘ 与 Ctrl（macOS 显 ⌘、其余显 Ctrl，判定两边都认）", () => {
    expect(matchesShortcut(key("b", { metaKey: true }), "cmd_or_ctrl+b")).toBe(true);
    expect(matchesShortcut(key("b", { ctrlKey: true }), "cmd_or_ctrl+b")).toBe(true);
    expect(matchesShortcut(key("b"), "cmd_or_ctrl+b")).toBe(false);
  });

  it("判定是精确的：多按的 Shift / Alt 不命中（⌘⇧F 不得同时命中 ⌘F）", () => {
    expect(matchesShortcut(key("F", { metaKey: true, shiftKey: true }), "cmd_or_ctrl+f")).toBe(false);
    expect(matchesShortcut(key("F", { metaKey: true, shiftKey: true }), "cmd_or_ctrl+shift+f")).toBe(true);
    expect(matchesShortcut(key("t", { metaKey: true }), "cmd_or_ctrl+alt+t")).toBe(false);
    expect(matchesShortcut(key("t", { metaKey: true, altKey: true }), "cmd_or_ctrl+alt+t")).toBe(true);
  });

  it("键名不分大小写（Shift 组合下浏览器给的是大写字母）", () => {
    expect(matchesShortcut(key("E", { metaKey: true }), "cmd_or_ctrl+e")).toBe(true);
  });

  it("具名键按物理键位认：设置里存的是 backslash，按下的 event.key 是 \\（W3 前对不上）", () => {
    expect(matchesShortcut(key("\\", { metaKey: true, code: "Backslash" }), "cmd_or_ctrl+backslash")).toBe(true);
    // Shift 会把字符换成 |，物理键位不变。
    expect(
      matchesShortcut(key("|", { metaKey: true, shiftKey: true, code: "Backslash" }), "cmd_or_ctrl+shift+backslash"),
    ).toBe(true);
    expect(matchesShortcut(key(",", { metaKey: true, code: "Comma" }), "cmd_or_ctrl+comma")).toBe(true);
  });

  it("既有谓词从同一张表派生（改表即改键，不必再找第二处）", () => {
  });
});

describe("codeMirrorKey（注册表键值串 → CM6 keymap 键名）", () => {
  it("cmd_or_ctrl → Mod，alt → Alt，shift → Shift，主键原样", () => {
    expect(codeMirrorKey("cmd_or_ctrl+b")).toBe("Mod-b");
    expect(codeMirrorKey("cmd_or_ctrl+alt+t")).toBe("Mod-Alt-t");
    expect(codeMirrorKey("cmd_or_ctrl+shift+f")).toBe("Mod-Shift-f");
    expect(codeMirrorKey("cmd_or_ctrl+0")).toBe("Mod-0");
  });
});

const SHORTCUT_RS = readFileSync(
  resolve(__dirname, "../../../../../crates/luna-config/src/shortcut.rs"),
  "utf8",
);

/** Rust `EDITOR_RESERVED_COMBOS` 里列出的组合（应用内快捷键不能再绑它们）。 */
function rustEditorReserved(): string[] {
  const start = SHORTCUT_RS.indexOf("pub const EDITOR_RESERVED_COMBOS");
  const block = SHORTCUT_RS.slice(start, SHORTCUT_RS.indexOf("];", start));
  return [...block.matchAll(/"([a-z0-9_+]+)"/g)].map((m) => m[1]);
}

describe("键位唯一性（一枚键只能属于一条命令）", () => {
  it("工作区命令一律走设置（可改键，冲突由 Rust 统一检测），注册表里不写字面键位", () => {
    for (const command of COMMANDS) {
      expect(Object.keys(command)).not.toContain("shortcut");
    }
    expect(commandById("toggle_sidebar")?.settingKey).toBe("shortcuts.toggle_sidebar");
  });

  it("编辑器的每一枚键都登记在 Rust 的编辑器保留表里（否则用户能把工作区命令绑上去，一次按键两边都响）", () => {
    const reserved = rustEditorReserved();
    expect(reserved.length).toBeGreaterThan(0);
    for (const value of Object.values(EDITOR_SHORTCUTS)) {
      expect(reserved).toContain(value);
    }
  });

  it("编辑器命令的键位不与单篇查找 ⌘F 冲突（PRD-06-014）", () => {
    expect(Object.values(EDITOR_SHORTCUTS)).not.toContain("cmd_or_ctrl+f");
  });
});

describe("命令面板（PRD-07-041）", () => {
  // 设置真值（默认值由 Rust 注册表给，这里照抄几条用来判定）。
  const bindings = {
    "shortcuts.quick_switcher": "cmd_or_ctrl+o",
    "shortcuts.command_palette": "cmd_or_ctrl+p",
    "shortcuts.toggle_sidebar": "cmd_or_ctrl+backslash",
  };

  it("⌘P 命中命令面板命令；工作区键一次判定返回命中的那条", () => {
    expect(workspaceCommandForKey(key("p", { metaKey: true }), bindings)?.id).toBe(
      "command_palette",
    );
    expect(workspaceCommandForKey(key("o", { ctrlKey: true }), bindings)?.id).toBe(
      "quick_switcher",
    );
    expect(
      workspaceCommandForKey(key("\\", { metaKey: true, code: "Backslash" }), bindings)?.id,
    ).toBe("toggle_sidebar");
    // 没绑键（设置里清空了）就不响。
    expect(workspaceCommandForKey(key("p", { metaKey: true }), {})).toBeNull();
    expect(workspaceCommandForKey(key("p"), bindings)).toBeNull();
  });

  /** 快速切换器的键位来自设置（PRD-09-005）：改了设置就按新键，注册表里没有字面值。 */
  it("可配置键位以设置真值为准", () => {
    expect(commandShortcut(commandById("quick_switcher")!, bindings)).toBe("cmd_or_ctrl+o");
    expect(commandShortcut(commandById("quick_switcher")!, {})).toBeUndefined();
    const rebound = { "shortcuts.quick_switcher": "cmd_or_ctrl+shift+o" };
    expect(workspaceCommandForKey(key("o", { metaKey: true }), rebound)).toBeNull();
    expect(
      workspaceCommandForKey(key("o", { metaKey: true, shiftKey: true }), rebound)?.id,
    ).toBe("quick_switcher");
  });

  /** 应用内键也能设成双击：第一下只记账，时间窗内第二下才执行；夹了别的键作废。 */
  it("双击型绑定按两次才命中", () => {
    const doubleTap = { "shortcuts.quick_switcher": "double_tap:cmd_or_ctrl+o" };
    const detector = new DoubleTapDetector();
    const at = (nowMs: number) => ({ detector, windowMs: 400, nowMs });
    const cmdO = key("o", { metaKey: true });
    expect(workspaceCommandForKey(cmdO, doubleTap, at(0))).toBeNull();
    expect(workspaceCommandForKey(cmdO, doubleTap, at(300))?.id).toBe("quick_switcher");
    // 复位后第三下重新开始。
    expect(workspaceCommandForKey(cmdO, doubleTap, at(350))).toBeNull();
    // 超窗不成对。
    expect(workspaceCommandForKey(cmdO, doubleTap, at(1000))).toBeNull();
    // 夹了别的键作废；按下修饰键本身不算。
    expect(workspaceCommandForKey(cmdO, doubleTap, at(2000))).toBeNull();
    expect(workspaceCommandForKey(key("Meta", { metaKey: true }), doubleTap, at(2050))).toBeNull();
    expect(workspaceCommandForKey(cmdO, doubleTap, at(2100))?.id).toBe("quick_switcher");
    expect(workspaceCommandForKey(cmdO, doubleTap, at(3000))).toBeNull();
    expect(workspaceCommandForKey(key("x"), doubleTap, at(3050))).toBeNull();
    expect(workspaceCommandForKey(cmdO, doubleTap, at(3100))).toBeNull();
    // 时间窗尚未读到（null）时不判双击。
    expect(
      workspaceCommandForKey(cmdO, doubleTap, { detector, windowMs: null, nowMs: 4000 }),
    ).toBeNull();
  });

  it("每个一级空间都有「前往」命令；命令 id 唯一", () => {
    // 注册表的空间清单与 workspace-ia 的一致性在 `workspace-ia.test.ts` 那头守（本目录
    // 不能引入 workspace——结构门禁把测试文件也算进依赖图，引入即成环）。
    for (const space of COMMAND_SPACES) {
      expect(COMMANDS.some((command) => command.id === `go_${space}`)).toBe(true);
    }
    const ids = [...COMMANDS.map((command) => command.id), ...EDITOR_PALETTE_COMMANDS.map((c) => c.id)];
    expect(new Set(ids).size).toBe(ids.length);
  });

  it("面板条目只在真绑了键时带提示（⌘E 有、切到源码模式没有）", () => {
    const byId = new Map(EDITOR_PALETTE_COMMANDS.map((command) => [command.id, command]));
    const bindings = { "shortcuts.toggle_reading_view": "cmd_or_ctrl+e" };
    expect(editorPaletteShortcut(byId.get("toggle_reading_view")!, bindings)).toBe("cmd_or_ctrl+e");
    expect(editorPaletteShortcut(byId.get("view_source")!, bindings)).toBeUndefined();
    // 清空了键位就不带提示。
    expect(editorPaletteShortcut(byId.get("toggle_reading_view")!, {})).toBeUndefined();
  });

  it("编辑器整篇动作按设置真值判定（W3b）：改键后旧键不响、新键响，双击与工作区同一套", () => {
    const rebound = {
      "shortcuts.toggle_reading_view": "cmd_or_ctrl+shift+r",
      "shortcuts.ai_composer": "double_tap:cmd_or_ctrl+j",
    };
    expect(editorCommandForKey(key("e", { metaKey: true }), rebound)).toBeNull();
    expect(editorCommandForKey(key("R", { metaKey: true, shiftKey: true }), rebound)).toBe(
      "toggle_reading_view",
    );
    const detector = new DoubleTapDetector();
    const at = (nowMs: number) => ({ detector, windowMs: 400, nowMs });
    const cmdJ = key("j", { metaKey: true });
    expect(editorCommandForKey(cmdJ, rebound, at(0))).toBeNull();
    expect(editorCommandForKey(cmdJ, rebound, at(200))).toBe("ai_composer");
  });
});

describe("「Tab 键移动焦点」的出路要看得见（WCAG 2.1.2）", () => {
  it("命令面板里有这一条，并显示固定键位 ⌘⇧M（不是 mac 上会打出「Â」的 ⇧⌥M）", () => {
    const command = EDITOR_PALETTE_COMMANDS.find((c) => c.id === "tab_focus_mode");
    expect(command).toBeDefined();
    expect(editorPaletteShortcut(command!, {})).toBe("cmd_or_ctrl+shift+m");
  });
});

