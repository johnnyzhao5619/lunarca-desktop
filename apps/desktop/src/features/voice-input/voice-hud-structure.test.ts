/// <reference types="vite/client" />
/**
 * 语音 HUD 的分层守护（2026-09-07 拆分）：组件只渲染，业务在 `use-voice-input.ts`。
 *
 * 拆之前 `VoiceInputHud.tsx` 有 1381 行，把会话生命周期、事件订阅、电平通道、热键桥、
 * 模型修复、落地与设置读写全塞在一个组件里；录音 HUD 早已把这些抽进 `use-recording.ts`。
 * 这条锁的是分工不回退：组件文件里不许再出现 `commands.` / `events.` 直接调用，hook 里
 * 不许出现 JSX。
 */
import { describe, expect, it } from "vitest";
import hudRaw from "./VoiceInputHud.tsx?raw";
import hookRaw from "./use-voice-input.ts?raw";

function stripComments(source: string): string {
  return source
    .split("\n")
    .filter((line) => !line.trim().startsWith("*") && !line.trim().startsWith("//"))
    .map((line) => line.split("//")[0] ?? "")
    .join("\n");
}

describe("语音 HUD：组件只渲染，业务归 hook", () => {
  it("组件文件不直接调 commands / events，也不再自己拼 Channel", () => {
    const code = stripComments(hudRaw);
    expect(code).not.toMatch(/\bcommands\./);
    expect(code).not.toMatch(/\bevents\./);
    expect(code).not.toContain("new Channel<");
    expect(code).toContain("useVoiceInput()");
  });

  it("空闲态的提示就是圆钮的名字，承载实际快捷键（PRD-02-015）；只有一张表面", () => {
    const code = stripComments(hudRaw);
    expect(code).toContain('"action.start_hold"');
    expect(code).toContain('"action.start_toggle"');
    // 不再有「迷你 / 标准」两套渲染：两份 HudSurface、空闲说明句、大计时都退役了。
    expect(code.match(/<HudSurface/g)?.length).toBe(1);
    expect(code).not.toContain("preview.empty_");
    expect(code).not.toContain("effectiveLayout");
    // 计时只在采集中出现（ADR-137 V4：空闲不显示 0:00）。
    expect(code).toMatch(/statusTone === "live" \? \([\s\S]*?luna-voice__clock/);
  });

  it("hook 文件不含 JSX，且仍是每次唤起都重读会话状态的那一份订阅", () => {
    const code = stripComments(hookRaw);
    // 泛型（`Channel<ArrayBuffer>`）也长着尖括号，故看闭合标签与 className，不看开标签。
    expect(code).not.toMatch(/<\/[A-Za-z]/);
    expect(code).not.toContain("className=");
    expect(code).toContain("events.voiceInputHudOpened.listen");
    expect(code).toContain("window.__lunaVoiceHotkey =");
  });
});
