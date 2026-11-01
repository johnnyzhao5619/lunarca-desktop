/**
 * 同步命令与同步轮次的稳定码 → 文案（ADR-148；TDD-16 §9）。
 *
 * 认得的码走 `cloud:sync.error.<code>`；认不出的（服务端新增的码、`server` 兜底）给一句
 * 通用说法——不把裸码写给用户，他既读不懂也无从处置。
 */

type Translate = (key: string, options?: Record<string, unknown>) => string;

export function syncErrorText(code: string, t: Translate): string {
  return t(`sync.error.${code}`, { defaultValue: "" }) || t("sync.error.other");
}
