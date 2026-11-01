/// <reference types="vite/client" />
/**
 * HUD 档位契约守护（ADR-112 ③，2026-09-08 落地）。
 *
 * 几何与 clamp 只在 Rust（`hud_tier_height` / `hud_set_tier`）；前端只有一个触发点
 * （`useHudWindowAutoGrow` 的 `tier`），三扇内容驱动 HUD 都经 `HudSurface.tier` 交档位。
 * 拦的是回退：某扇窗自己 `setSize` 缩窗、或 Rust 之外再出现第二份几何表。
 */
import { describe, expect, it } from "vitest";
import hookRaw from "../hooks/use-hud-window-auto-grow.ts?raw";
import surfaceRaw from "../components/ui/HudSurface.tsx?raw";
import popupRaw from "../features/popup-translate/PopupTranslateView.tsx?raw";
import voiceRaw from "../features/voice-input/VoiceInputHud.tsx?raw";
import recordingRaw from "../features/recording/RecordingHud.tsx?raw";

describe("HUD 档位（ADR-112 ③）", () => {
  it("档位跃迁只经 hud_set_tier 一个入口，且只在档位变化时触发", () => {
    expect(hookRaw).toContain("commands.hudSetTier(tier)");
    expect(hookRaw).toContain("lastTierRef.current === tier) return;");
    expect(surfaceRaw).toContain("tier,");
  });

  it("三扇内容驱动 HUD 都按状态交档位", () => {
    expect(popupRaw).toContain('tier={resultsVisible ? "result" : "input"}');
    // 语音只有一张实时表面（ADR-137）：配置 = panel 档本身，收起设置是跃迁回 pill/card——
    // 档内只扩容不缩，收起要缩回去只能走跃迁（真机 2026-09-08）。
    expect(voiceRaw).toContain('const tier = configOpen ? "panel" : card ? "card" : "pill";');
    expect(voiceRaw).toContain("tier={tier}");
    expect(voiceRaw).not.toMatch(/tier=\{[^}]*"settings"/);
    // 控制条里没有下拉菜单（常用操作都常驻成图标），档位只随设置与实时转写变。
    expect(recordingRaw).toContain('tier={settingsOpen ? "panel" : transcribeEnabled ? "card" : "bar"}');
  });

  it("前端没有第二份几何表，也不自己缩窗", () => {
    for (const raw of [popupRaw, voiceRaw, recordingRaw]) {
      expect(raw).not.toMatch(/setSize\(/);
      expect(raw).not.toMatch(/LogicalSize/);
    }
  });
});
