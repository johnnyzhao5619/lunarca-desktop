/**
 * 网页版文案（TDD-17 §2）：与桌面同一套做法——react-i18next + JSON 资源，locale 与桌面相同
 * （zh-CN、en-US）。语言默认跟随浏览器，用户在未登录页选过就按选的（ADR-178 ⑦）；共享渲染包自带的文案（`editor-core`）并进同一个实例。
 */

import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import { EDITOR_CORE_LOCALES, EDITOR_CORE_NAMESPACE } from "@lunarca/editor-core/i18n";
import zhWeb from "./locales/zh-CN/web.json";
import enWeb from "./locales/en-US/web.json";

export const SUPPORTED_LANGUAGES = ["zh-CN", "en-US"] as const;
export type SupportedLanguage = (typeof SUPPORTED_LANGUAGES)[number];

/** 浏览器首选语言 → 支持的 locale（与桌面 `resolveSupportedLanguage` 同一口径：中文一律简体，其余英文）。 */
export function browserLanguage(languages: readonly string[]): SupportedLanguage {
  return languages[0]?.toLowerCase().startsWith("zh") ? "zh-CN" : "en-US";
}

const LANGUAGE_KEY = "luna:web-language";

/** 用户选过的语言；浏览器存储不可用或值认不出时 `null`（ADR-177 ⑤：只存偏好、不存内容）。 */
export function storedLanguage(storage: Pick<Storage, "getItem">): SupportedLanguage | null {
  try {
    const value = storage.getItem(LANGUAGE_KEY);
    return SUPPORTED_LANGUAGES.find((item) => item === value) ?? null;
  } catch {
    return null;
  }
}

export const language =
  (typeof window === "undefined" ? null : storedLanguage(window.localStorage)) ??
  browserLanguage(typeof navigator === "undefined" ? [] : navigator.languages ?? [navigator.language]);

/** 切换界面语言：立即重渲染，并记在浏览器里；记不下来也照常切换。 */
export function chooseLanguage(next: SupportedLanguage) {
  try {
    window.localStorage.setItem(LANGUAGE_KEY, next);
  } catch {
    // 无痕模式等存不了：这次会话照常切换，下次回到跟随浏览器。
  }
  document.documentElement.lang = next;
  void i18n.changeLanguage(next);
}

void i18n.use(initReactI18next).init({
  lng: language,
  fallbackLng: "en-US",
  ns: ["web", EDITOR_CORE_NAMESPACE],
  defaultNS: "web",
  resources: {
    "zh-CN": { web: zhWeb, [EDITOR_CORE_NAMESPACE]: EDITOR_CORE_LOCALES["zh-CN"] },
    "en-US": { web: enWeb, [EDITOR_CORE_NAMESPACE]: EDITOR_CORE_LOCALES["en-US"] },
  },
  interpolation: { escapeValue: false },
});

export default i18n;
