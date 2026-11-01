import type { i18n as I18nInstance } from "i18next";

/** 缺失翻译时的通用兜底文案 key（`notifications:fallback.{title,body}`）。 */
const FALLBACK_KEY = "notifications:fallback";

/**
 * 解析通知的本地化基准 key（title/body/action 均挂在其下）。
 *
 * 后端只保证 `i18n_key` 字符串本身稳定（`notifications:<kind>`），不保证前端已登记
 * 对应翻译——新增通知 kind 忘记补 locale 词条时，绝不能把这串技术 key 直接渲染给用户
 * （曾在通知历史面板发生：裸显示 `notifications:recording_retranscribed`）。
 */
export function resolveNotificationBaseKey(i18nKey: string, i18n: I18nInstance): string {
  return i18n.exists(`${i18nKey}.title`) ? i18nKey : FALLBACK_KEY;
}
