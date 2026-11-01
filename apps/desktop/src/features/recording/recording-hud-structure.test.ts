/// <reference types="vite/client" />
/**
 * 录音 HUD 控制条形态的结构守护（PRD-04-003 修订 2026-09-07，ADR-112 ④）。
 *
 * 钉住四件事（ADR-137 H5 起是一行控制条）：① 「● 计时」占状态行身份位、只在会话开着时
 * 出现（空闲不显示 00:00，那时身份位是「开始录音」）；② 字幕只在「实时转写」开着时渲染；
 * ③ 配置摘要只在空闲行里（会话中走「⋯ → 录音设置」），停止键不再横在改设置的路上；
 * ④ 读屏 live 区不再挂整个根，字幕视图关掉播报；状态文案兜底一律 `defaultValue`。
 */
import { describe, expect, it } from "vitest";
import hudRaw from "./RecordingHud.tsx?raw";

function stripComments(source: string): string {
  return source
    .split("\n")
    .filter((line) => !line.trim().startsWith("*") && !line.trim().startsWith("//"))
    .map((line) => line.split("//")[0] ?? "")
    .join("\n");
}

const code = stripComments(hudRaw);

describe("录音 HUD 控制条形态", () => {
  it("计时占状态行身份位，且只在会话开着时出现；空闲时身份位是唯一的「开始录音」", () => {
    expect(code).toContain("const clock = sessionOpen ?");
    expect(code).toMatch(/const lead =\s*clock \?\?/);
    expect(code).toContain("lead={lead}");
    // 不再另起一条控制条：动作都在同一行的动作区里。
    expect(code).not.toContain('className="luna-rec-hud__controls"');
    expect(code).toContain("{controls}");
  });

  it("字幕画布只在实时转写开着时渲染，且不进读屏 live 区", () => {
    expect(code).toMatch(/\{transcribeEnabled && !settingsOpen && \(\s*<section className="luna-rec-hud__live-text"/);
    expect(code).toContain("announce={false}");
    // 根元素不再整体 aria-live；状态一句自己是 live 区。
    const surface = code.slice(code.indexOf("<HudSurface"), code.indexOf("onDismiss="));
    expect(surface).not.toContain("aria-live");
    expect(code).toContain('className="luna-visually-hidden" role="status"');
  });

  it("配置摘要只在空闲行里；会话中设置是一枚常驻图标，不进二级菜单", () => {
    const lane = code.slice(code.indexOf("const lane ="), code.indexOf("const stopButton"));
    expect(lane).toContain("luna-rec-hud__config-summary");
    expect(code).toContain('label={t("action.settings")}');
    // 真机 2026-10-02：「⋯」把一行高的控制条撑大，且静音 / 设置是常用操作。
    expect(code).not.toContain("<HudActionMenu");
    // 「停止」是结束并保存：中性强调，不是危险色（ADR-137 V3）。
    expect(code).toContain('className="luna-btn luna-btn--secondary luna-rec-hud__stop"');
  });

  it("横幅只有一个渲染点，且状态文案兜底走 defaultValue", () => {
    expect(code.split('className="luna-rec-hud__banner"').length - 1).toBe(1);
    expect(code).toContain("pickRecordingBanner(");
    expect(code).not.toMatch(/t\([^)]*\)\s*\|\|\s*t\(/);
  });
});
