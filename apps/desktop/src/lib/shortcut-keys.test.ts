/**
 * 录快捷键时「按下的到底是哪个键」（PRD-09-003 / TDD-05 §1.2）：只按物理键位认，词表外的
 * 键返回 null。词表与 Rust 那侧逐字对齐，另一半由
 * `lib.rs::every_name_the_recorder_can_produce_is_registrable` 钉住。
 */

import { describe, expect, it } from "vitest";
import { isModifierKey, mainKeyOf } from "./shortcut-keys";

/** 造一次按键：`code` 是键在键盘上的位置，`key` 是它此刻产出的字符。 */
function press(code: string, key: string) {
  return { code, key };
}

describe("录制器认得出的键", () => {
  it("字母 / 数字 / 功能键按物理位置认，不看产出的字符", () => {
    // 按住 Option 的 e 在 macOS 上产出死键，而它的物理位置仍是 KeyE。
    expect(mainKeyOf(press("KeyE", "Dead"))).toBe("e");
    expect(mainKeyOf(press("Digit3", "#"))).toBe("3");
    expect(mainKeyOf(press("F12", "F12"))).toBe("f12");
  });

  it("空格认得出：它的 `key` 是一个空格字符，不是 `\"Space\"`", () => {
    expect(mainKeyOf(press("Space", " "))).toBe("space");
  });

  it("标点按物理位置认（非美式布局上同一个位置产出的字符各不相同）", () => {
    expect(mainKeyOf(press("Minus", "-"))).toBe("minus");
    expect(mainKeyOf(press("BracketLeft", "["))).toBe("bracketleft");
    expect(mainKeyOf(press("Backquote", "`"))).toBe("backquote");
  });

  it("认不出的键返回 null，绝不编一个名字出来", () => {
    // 死键、输入法处理中的键、异形键盘的私有键：写进设置就是一个注册不上的组合。
    expect(mainKeyOf(press("Unidentified", "Dead"))).toBeNull();
    expect(mainKeyOf(press("Lang1", "Process"))).toBeNull();
    expect(mainKeyOf(press("F21", "F21"))).toBeNull();
    expect(mainKeyOf(press("F25", "F25"))).toBeNull();
    expect(mainKeyOf(press("MediaPlayPause", "MediaPlayPause"))).toBeNull();
  });

  it("修饰键本身不是主键（录制继续等着）", () => {
    for (const key of ["Control", "Shift", "Alt", "Meta"]) {
      expect(isModifierKey(key), key).toBe(true);
      expect(mainKeyOf(press(`${key}Left`, key))).toBeNull();
    }
    expect(isModifierKey("a")).toBe(false);
  });
});
