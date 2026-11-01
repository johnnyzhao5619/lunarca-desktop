import { describe, expect, it } from "vitest";
import { nextHudWindowHeight, requiredHudContentHeight } from "./hud-window-layout";

describe("nextHudWindowHeight", () => {
  it("never shrinks a user-enlarged HUD", () => {
    expect(
      nextHudWindowHeight({
        viewportHeight: 720,
        contentHeight: 420,
        availableHeight: 900,
      }),
    ).toBeNull();
  });

  it("grows only when a state change no longer fits", () => {
    expect(
      nextHudWindowHeight({
        viewportHeight: 180,
        contentHeight: 456,
        availableHeight: 900,
      }),
    ).toBe(456);
  });

  it("caps growth at the current screen's available height", () => {
    expect(
      nextHudWindowHeight({
        viewportHeight: 320,
        contentHeight: 1200,
        availableHeight: 860,
      }),
    ).toBe(860);
  });

  it("grows to a HUD's known preflight content height when flex layout hides overflow", () => {
    expect(
      nextHudWindowHeight({
        viewportHeight: 200,
        contentHeight: 200,
        availableHeight: 900,
        minContentHeight: 480,
      }),
    ).toBe(480);
  });
});

describe("requiredHudContentHeight", () => {
  it("溢出时补回下内边距：最低盒子底边 + padding 比 scrollHeight 大就取它", () => {
    // WebKit：溢出内容贴着 padding 边界算 scrollHeight，不含下内边距。
    expect(requiredHudContentHeight({ scrollHeight: 500, contentBottom: 500, paddingBottom: 16 })).toBe(516);
  });

  it("不溢出时不多长：盒子底边 + padding 不超过 scrollHeight 就照 scrollHeight", () => {
    expect(requiredHudContentHeight({ scrollHeight: 300, contentBottom: 260, paddingBottom: 16 })).toBe(300);
  });

  it("嵌套子元素撑破父级只有 scrollHeight 看得见，取大保住它", () => {
    expect(requiredHudContentHeight({ scrollHeight: 640, contentBottom: 400, paddingBottom: 16 })).toBe(640);
  });
});
