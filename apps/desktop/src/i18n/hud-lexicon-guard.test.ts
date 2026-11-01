import { describe, expect, it } from "vitest";
import { i18n } from "./i18n";
import zhCommon from "./locales/zh-CN/common.json";
import enCommon from "./locales/en-US/common.json";
import zhPopup from "./locales/zh-CN/popup.json";
import enPopup from "./locales/en-US/popup.json";
import zhVoice from "./locales/zh-CN/voice-input.json";
import enVoice from "./locales/en-US/voice-input.json";
import zhCaption from "./locales/zh-CN/caption.json";
import enCaption from "./locales/en-US/caption.json";
import zhRecording from "./locales/zh-CN/recording.json";
import enRecording from "./locales/en-US/recording.json";
import zhChat from "./locales/zh-CN/chat.json";
import enChat from "./locales/en-US/chat.json";

/**
 * HUD 动词表守护（ADR-111 ② 批 E1）。
 *
 * 同一个动作在五扇悬浮窗里必须是同一个动词：结束本场曾是「完成/停止/停止生成」三说、
 * 存成笔记有五种说法、空闲态有四种。动词单源在 `common:hud.*`，各 ns 的对应键用
 * i18next `$t()` 嵌套指过去，只许在动词后接宾语（`$t(...)录音`）、不许换动词。
 *
 * 语音的「完成」（`voice-input:action.stop`）**刻意不在表内**：它是"停止采集并
 * 继续处理"的提交动作，与录音/字幕的「停止」（结束会话）是两个动作——同名才是错。
 */

/** 必须以 $t 指向动词表的键（两种语言都查；宾语允许接在引用前后）。 */
const POINTERS: Array<[string, Record<string, unknown>, Record<string, unknown>, string, string]> =
  [
    ["popup", zhPopup, enPopup, "action.close", "common:hud.action.close"],
    ["popup", zhPopup, enPopup, "action.copy", "common:hud.action.copy"],
    ["popup", zhPopup, enPopup, "action.save_note", "common:hud.action.save_note"],
    ["popup", zhPopup, enPopup, "action.regenerate", "common:hud.action.regenerate"],
    ["voice-input", zhVoice, enVoice, "action.cancel", "common:hud.action.cancel"],
    ["voice-input", zhVoice, enVoice, "action.close", "common:hud.action.close"],
    ["voice-input", zhVoice, enVoice, "action.start", "common:hud.action.start"],
    ["voice-input", zhVoice, enVoice, "status.recording", "common:hud.status.listening"],
    ["caption", zhCaption, enCaption, "action.start", "common:hud.action.start"],
    ["caption", zhCaption, enCaption, "action.stop", "common:hud.action.stop"],
    ["caption", zhCaption, enCaption, "action.closePanel", "common:hud.action.close"],
    ["caption", zhCaption, enCaption, "action.saveNote", "common:hud.action.save_note"],
    ["caption", zhCaption, enCaption, "status.idle", "common:hud.status.idle"],
    ["caption", zhCaption, enCaption, "status.listening", "common:hud.status.listening"],
    ["recording", zhRecording, enRecording, "action.start", "common:hud.action.start"],
    ["recording", zhRecording, enRecording, "action.stop", "common:hud.action.stop"],
    ["recording", zhRecording, enRecording, "action.closePanel", "common:hud.action.close"],
    ["recording", zhRecording, enRecording, "status.idle", "common:hud.status.idle"],
    ["chat", zhChat, enChat, "action.close", "common:hud.action.close"],
    ["chat", zhChat, enChat, "action.stop", "common:hud.action.stop"],
    ["chat", zhChat, enChat, "action.retry", "common:hud.action.retry"],
    ["chat", zhChat, enChat, "action.regenerate", "common:hud.action.regenerate"],
  ];

function pick(bundle: Record<string, unknown>, path: string): unknown {
  return path
    .split(".")
    .reduce<unknown>(
      (node, part) =>
        node !== null && typeof node === "object"
          ? (node as Record<string, unknown>)[part]
          : undefined,
      bundle,
    );
}

describe("HUD 动词表（common:hud）", () => {
  it("动词表中英键集一致，且表内的值不再嵌套（防循环引用）", () => {
    const zh = (zhCommon as Record<string, unknown>).hud as Record<
      string,
      Record<string, string>
    >;
    const en = (enCommon as Record<string, unknown>).hud as Record<
      string,
      Record<string, string>
    >;
    expect(zh).toBeTruthy();
    expect(en).toBeTruthy();
    for (const group of ["action", "status"] as const) {
      expect(Object.keys(en[group]).sort()).toEqual(Object.keys(zh[group]).sort());
      for (const value of [...Object.values(zh[group]), ...Object.values(en[group])]) {
        expect(value).not.toContain("$t(");
      }
    }
  });

  it.each(POINTERS)("%s %s 引用 %s（两种语言）", (_ns, zh, en, key, target) => {
    for (const bundle of [zh, en]) {
      const value = pick(bundle, key);
      expect(value, `${key} 缺失`).toBeTypeOf("string");
      expect(value as string).toContain(`$t(${target})`);
    }
  });

  it("$t 嵌套必须真的解析——只查字符串不算数（运行时判据）", async () => {
    await i18n.changeLanguage("zh-CN");
    expect(i18n.t("recording:action.start")).toBe("开始录音");
    expect(i18n.t("voice-input:action.start")).toBe("开始说话");
    expect(i18n.t("chat:action.stop")).toBe("停止生成");
    expect(i18n.t("caption:action.closePanel")).toBe("关闭（Esc）");
    expect(i18n.t("recording:status.idle")).toBe("未开始");
    await i18n.changeLanguage("en-US");
    expect(i18n.t("recording:action.start")).toBe("Start recording");
    expect(i18n.t("chat:action.stop")).toBe("Stop generating");
    expect(i18n.t("caption:action.saveNote")).toBe("Save to note");
    await i18n.changeLanguage("zh-CN");
  });
});
