// 键位展示（PRD-09-003）：设置里存的是键名，展示成键帽上印的符号。
import { describe, expect, it } from "vitest";
import { displayKey } from "./shortcut-display";
import { mainKeyOf } from "./shortcut-keys";

describe("displayKey", () => {
  it("标点类键名展示成符号（不再大写回显成 COMMA / BACKSLASH）", () => {
    expect(displayKey("comma")).toBe(",");
    expect(displayKey("backslash")).toBe("\\");
    expect(displayKey("bracketleft")).toBe("[");
    expect(displayKey("slash")).toBe("/");
    expect(displayKey("p")).toBe("P");
  });

  it("录键能录到的每个具名键，展示时都不是大写的键名本身", () => {
    const codes = ["Minus", "Equal", "BracketLeft", "BracketRight", "Backslash", "Semicolon", "Quote", "Comma", "Period", "Slash", "Backquote", "Space", "Enter", "Tab", "Escape", "Backspace", "Delete", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight"];
    for (const code of codes) {
      const name = mainKeyOf({ code, key: "" });
      if (name === null) throw new Error(`录键认不出 ${code}`);
      expect(displayKey(name)).not.toBe(name.toUpperCase());
    }
  });
});
