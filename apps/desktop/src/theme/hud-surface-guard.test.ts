/// <reference types="vite/client" />
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import chatViewRaw from "../features/chat/ChatView.tsx?raw";
import captionPanelRaw from "../features/caption/CaptionPanel.tsx?raw";
import popupTranslateRaw from "../features/popup-translate/PopupTranslateView.tsx?raw";
import recordingHudRaw from "../features/recording/RecordingHud.tsx?raw";
import voiceHudRaw from "../features/voice-input/VoiceInputHud.tsx?raw";
import quickCaptureRaw from "../features/capture/TextQuickNoteHud.tsx?raw";
import mirrorWindowRaw from "../features/caption/CaptionTranslationWindow.tsx?raw";
import useRecordingRaw from "../features/recording/use-recording.ts?raw";
import { runtimeStyleCss as cssRaw } from "./style-source";
import hudSurfaceRaw from "../components/ui/HudSurface.tsx?raw";
import hudHeaderRaw from "../components/ui/HudHeader.tsx?raw";
import chatCssRaw from "../features/chat/chat.css?raw";

const LIB_RS = readFileSync(resolve(__dirname, "../../../../src-tauri/src/lib.rs"), "utf8");

// 判据只看规则，不看注释——收编处的注释会点名旧选择器（那是历史记录，不是规则）。
const cssRules = cssRaw.replace(/\/\*[\s\S]*?\*\//g, "");

const HUD_RAWS: Array<[string, string]> = [
  ["popup-translate/PopupTranslateView", popupTranslateRaw],
  ["voice-input/VoiceInputHud", voiceHudRaw],
  ["recording/RecordingHud", recordingHudRaw],
  ["caption/CaptionPanel", captionPanelRaw],
  ["chat/ChatView", chatViewRaw],
  // 第六扇（PRD-06-058 B″）：ADR-111 的「已知代价 ①」承诺"新 HUD 挂上组件即自动长对"，
  // 而"自动长对"只有在它**真的进了这张表**时才成立——表不加一行，新窗就是不受契约管的。
  ["capture/TextQuickNoteHud", quickCaptureRaw],
];

/**
 * HUD 表面契约守护（ADR-111 ②③，批 B）。
 *
 * ① 五扇悬浮面的外壳（根几何/拖动/自动扩容/Esc）只有一份实现：`HudSurface`。
 *    feature 组件不再自拼外壳，也不再各自挂 window 级 Esc 监听。
 * ② §3.3e：features 层压过 primitives 层是 Cascade Layer 的既定顺序，一条覆盖全窗的
 *    裸元素选择器（`.luna-popup button`）就能让原语声明整层失效——窗口根类之下不得
 *    直接跟 button/input/textarea/select。
 */
describe("HUD 表面契约（ADR-111 ②③）", () => {
  it("每扇 HUD 的根都是 HudSurface，且 Esc 通道由外壳统一拥有", () => {
    for (const [name, raw] of HUD_RAWS) {
      expect(raw, name).toContain("<HudSurface");
      expect(raw, name).toContain("onDismiss=");
      // Esc 不再各写各的：feature 里不许再出现自己的 window 级键盘监听。
      // （元素级 onKeyDown 的分层让路——@ 候选收起等——不在此列。）
      expect(raw, name).not.toMatch(/window\.addEventListener\(\s*"keydown"/);
    }
  });

  it("外壳声明只有一份：feature 根类不再自绘外壳", () => {
    // 抓手光标是自绘外壳的指纹：此前四扇窗各有一份 `:active { cursor: grabbing }`。
    for (const stale of [
      ".luna-popup:active",
      ".luna-voice:active",
      ".luna-rec-hud:active",
      ".luna-caption:active",
    ]) {
      expect(cssRules).not.toContain(stale);
    }
    expect(cssRaw).toMatch(/\.luna-hud-surface\s*\{[^}]*block-size:\s*var\(--size-full\)/s);
    expect(cssRaw).toMatch(/\.luna-hud-surface\[data-drag-root\]\s*\{[^}]*user-select:\s*none/s);
  });

  it("§3.3e：窗口根类之下不得直接跟裸元素选择器（button/input/textarea/select）", () => {
    // 窗口根的射程覆盖全窗，其下的裸元素选择器必然击中挂原语类的控件，并在 features
    // 层把原语整层压掉（`.luna-popup button` 事故的形状）。带中间类的局部选择器
    // （`.luna-popup__input textarea`）只管自己的容器，不在此列。
    const banned =
      /\.luna-(?:popup|voice|rec-hud|caption|chat|quick-capture|hud-surface)(?:\[[^\]]*\])?\s+(?:button|input|textarea|select)\b/;
    const hit = cssRules.match(banned);
    expect(hit, hit ? `发现窗口根下的裸元素选择器：${hit[0]}` : undefined).toBeNull();
    // 桌面透镜与它的控制条（ADR-137 H8）：此前 `.luna-desktop-lens__actions button` 与
    // `.luna-desktop-lens-trigger button` 两条元素兜底给按钮上皮，按钮现在全是原语。
    const lensHit = cssRules.match(/\.luna-desktop-lens[\w-]*\s+(?:button|input|textarea|select)\b/);
    expect(lensHit, lensHit ? `透镜下的裸元素选择器：${lensHit[0]}` : undefined).toBeNull();
  });

  it("窗口栏归 HudHeader 五槽契约：六个悬浮面全用它，身份图标两两不同（ADR-111 ①② 批 C）", () => {
    for (const [name, raw] of HUD_RAWS) {
      expect(raw, name).toContain("<HudHeader");
    }
    expect(mirrorWindowRaw).toContain("<HudHeader");
    // 身份图标：从每扇窗的 HudHeader icon 属性现读，五扇主窗两两不同——
    // 「弹窗翻译与实时字幕共用同一枚 Languages」正是本条要拦的历史事故。
    const icons = HUD_RAWS.map(([name, raw]) => {
      const match = raw.match(/<HudHeader[\s\S]*?icon=\{<(\w+Icon)/);
      expect(match, `${name} 的 HudHeader 未声明身份图标`).not.toBeNull();
      return match![1];
    });
    expect(new Set(icons).size, `身份图标撞车：${icons.join(" / ")}`).toBe(icons.length);
  });

  it("录音关闭恒在、关窗不停会话（ADR-111 已知代价 ③）", () => {
    // 关闭键不再包在 `!active` 条件里；语义由 use-recording 的 dismiss 分流
    //（会话中只藏窗、不复位状态）。字幕刻意保持「仅非会话态可关」——理由见组件注释。
    expect(recordingHudRaw).toContain('label={t("action.closePanel")}');
    expect(recordingHudRaw).not.toMatch(/!active &&[\s\S]{0,400}action\.closePanel/);
    expect(useRecordingRaw).toContain('if (mode === "caption") return;');
  });

  it("动作层收敛：HUD 文字按钮一律挂 .luna-btn 形态，三份按钮壳零残留（ADR-111 ② 批 D）", () => {
    // `.luna-caption__btn` / `.luna-rec-hud__btn` 曾是九行逐字相同的按钮壳；
    // 字幕的「开始」还烧着 `--color-status-active`（状态色 ≠ 主按钮色）。
    for (const stale of [".luna-caption__btn", ".luna-rec-hud__btn"]) {
      expect(cssRules).not.toContain(stale);
    }
    expect(captionPanelRaw).toContain('className="luna-btn luna-btn--primary luna-caption__primary"');
    expect(recordingHudRaw).toContain("luna-btn luna-btn--primary luna-rec-hud__start");
    expect(voiceHudRaw).toContain('className="luna-btn luna-btn--primary luna-voice__land"');
  });

  it("HUD 内的注册表键按注册表形态渲染：落点是下拉，不是按钮三连（AGENTS §4 批 D）", () => {
    // `voice_input.landing_mode` 在 Rust 注册表里声明为 Select{Dropdown}；选项值同源
    // useSettingOptions，不在前端手抄。按钮三连是按文案猜出来的形态，已删。
    expect(voiceHudRaw).toContain('useSettingOptions("voice_input.landing_mode")');
    expect(voiceHudRaw).not.toContain("aria-pressed={state.landing_mode");
  });

  it("反向覆盖已随根因一起拔除：不再有为绕开元素选择器而写的原语再声明", () => {
    for (const stale of [
      ".luna-popup .luna-icon-btn",
      ".luna-popup .luna-hud-menu__item",
      ".luna-popup .luna-result-actions__primary {",
    ]) {
      expect(cssRules).not.toContain(stale);
    }
  });

  it("浮层材质只有一个定义点：透明窗 + 圆角细边面，系统投影（ADR-137 V1）", () => {
    // Rust：每扇悬浮窗与便签的窗口链里都挂 `hud_chrome()`（透明 + 阴影），不各写一串。
    // 按窗口标题键定位构造链（最后一次出现在 builder 里；前面那次是改语言时的换标题表）。
    for (const key of [
      "window-title-popup-translate",
      "window-title-voice-input",
      "window-title-quick-chat",
      "window-title-text-quick-note",
      "window-title-recording",
      "window-title-desktop-note",
    ]) {
      const start = LIB_RS.lastIndexOf(`"${key}"`);
      const chain = LIB_RS.slice(start, LIB_RS.indexOf(".build()", start));
      expect(chain, key).toContain(".hud_chrome()");
      expect(chain, key).not.toContain(".decorations(false)");
    }
    // 前端：透明窗下根元素不得再铺底色；圆角与细边归外壳，feature 不再自补一份。
    expect(cssRules).toMatch(/html:has\(\.luna-hud-surface\)[^{]*\{[^}]*background:\s*transparent/s);
    expect(cssRules).toMatch(/\.luna-hud-surface\s*\{[^}]*border-radius:\s*var\(--hud-radius\)/s);
    expect(chatCssRaw.replace(/\/\*[\s\S]*?\*\//g, "")).not.toMatch(/\.luna-chat-panel\s*\{/);
  });

  it("外壳退场：没有拖动指示条，窗口名只进无障碍树（ADR-137 V2）", () => {
    expect(hudSurfaceRaw).not.toContain("luna-drag-handle");
    expect(cssRules).not.toContain(".luna-drag-handle");
    expect(hudHeaderRaw).toContain('<strong className="luna-visually-hidden">{title}</strong>');
    // 状态行与内容之间不再画分隔线。
    expect(cssRules).not.toMatch(/\.luna-hud-header\s*\{[^}]*border-block-end/s);
  });
});
