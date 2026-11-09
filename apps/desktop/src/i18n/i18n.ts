import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import { EDITOR_CORE_LOCALES, EDITOR_CORE_NAMESPACE } from "@lunarca/editor-core/i18n";
import { readBootAppearance } from "../lib/boot";
import common from "./locales/zh-CN/common.json";
import settings from "./locales/zh-CN/settings.json";
import onboarding from "./locales/zh-CN/onboarding.json";
import notifications from "./locales/zh-CN/notifications.json";
import workspace from "./locales/zh-CN/workspace.json";
import command from "./locales/zh-CN/command.json";
import usage from "./locales/zh-CN/usage.json";
import popup from "./locales/zh-CN/popup.json";
import voiceInput from "./locales/zh-CN/voice-input.json";
import chat from "./locales/zh-CN/chat.json";
import editor from "./locales/zh-CN/editor.json";
import recording from "./locales/zh-CN/recording.json";
import caption from "./locales/zh-CN/caption.json";
import playback from "./locales/zh-CN/playback.json";
import transcribe from "./locales/zh-CN/transcribe.json";
import cloud from "./locales/zh-CN/cloud.json";
import enCommon from "./locales/en-US/common.json";
import enSettings from "./locales/en-US/settings.json";
import enOnboarding from "./locales/en-US/onboarding.json";
import enNotifications from "./locales/en-US/notifications.json";
import enWorkspace from "./locales/en-US/workspace.json";
import enCommand from "./locales/en-US/command.json";
import enUsage from "./locales/en-US/usage.json";
import enPopup from "./locales/en-US/popup.json";
import enVoiceInput from "./locales/en-US/voice-input.json";
import enChat from "./locales/en-US/chat.json";
import enEditor from "./locales/en-US/editor.json";
import enRecording from "./locales/en-US/recording.json";
import enCaption from "./locales/en-US/caption.json";
import enPlayback from "./locales/en-US/playback.json";
import enTranscribe from "./locales/en-US/transcribe.json";
import enCloud from "./locales/en-US/cloud.json";

export const SUPPORTED_LANGUAGES = ["zh-CN", "en-US"] as const;
export type SupportedLanguage = (typeof SUPPORTED_LANGUAGES)[number];

export function resolveSupportedLanguage(value: string): SupportedLanguage {
  const raw = value === "system" ? navigator.language : value;
  return raw.toLowerCase().startsWith("zh") ? "zh-CN" : "en-US";
}

void i18n.use(initReactI18next).init({
  // 首帧语言取 Rust 注入的外观快照（PRD-14-066）：用户显式选了英文而系统是中文时，
  // 此前首帧先按系统画中文、`useAppearance` 读回设置后再切英文——闪一下。
  lng: resolveSupportedLanguage(readBootAppearance()?.language ?? "system"),
  fallbackLng: "zh-CN",
  ns: ["common", "settings", "onboarding", "notifications", "workspace", "command", "usage", "popup", "voice-input", "chat", "editor", "recording", "caption", "playback", "transcribe", EDITOR_CORE_NAMESPACE],
  defaultNS: "common",
  resources: {
    "zh-CN": {
      common,
      settings,
      onboarding,
      notifications,
      workspace,
      command,
      usage,
      popup,
      "voice-input": voiceInput,
      chat,
      editor,
      recording,
      caption,
      playback,
      transcribe,
      // 云客户端（账户/订阅/目录）——ADR-037 后为普通命名空间，不再由叠加层
      // `addResourceBundle` 运行时挂载。
      cloud,
      // 共享渲染包自带的文案（TDD-17 §2）：资源归包，并进同一个实例。
      [EDITOR_CORE_NAMESPACE]: EDITOR_CORE_LOCALES["zh-CN"],
    },
    "en-US": {
      common: enCommon,
      settings: enSettings,
      onboarding: enOnboarding,
      notifications: enNotifications,
      workspace: enWorkspace,
      command: enCommand,
      usage: enUsage,
      popup: enPopup,
      "voice-input": enVoiceInput,
      chat: enChat,
      editor: enEditor,
      recording: enRecording,
      caption: enCaption,
      playback: enPlayback,
      transcribe: enTranscribe,
      cloud: enCloud,
      [EDITOR_CORE_NAMESPACE]: EDITOR_CORE_LOCALES["en-US"],
    },
  },
  interpolation: { escapeValue: false },
});

export { i18n };
