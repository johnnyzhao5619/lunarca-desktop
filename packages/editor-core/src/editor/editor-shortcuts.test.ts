import { describe, expect, it } from "vitest";
import { codeMirrorKey } from "./editor-shortcuts";

describe("codeMirrorKey（注册表键值串 → CM6 keymap 键名）", () => {
  it("cmd_or_ctrl → Mod，alt → Alt，shift → Shift，主键原样", () => {
    expect(codeMirrorKey("cmd_or_ctrl+b")).toBe("Mod-b");
    expect(codeMirrorKey("cmd_or_ctrl+alt+t")).toBe("Mod-Alt-t");
    expect(codeMirrorKey("cmd_or_ctrl+shift+f")).toBe("Mod-Shift-f");
    expect(codeMirrorKey("cmd_or_ctrl+0")).toBe("Mod-0");
  });
});
