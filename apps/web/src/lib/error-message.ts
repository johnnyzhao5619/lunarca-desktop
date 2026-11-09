import type { TFunction } from "i18next";
import { formatBytes } from "@lunarca/editor-core/format-bytes";

/**
 * 稳定码 → 写明原因的一句话（`group` 是语言文件里的分组：`error` / `signinError` / `ended`）。
 * 不认得的码给通用一句并带上码本身，不猜测含义。
 */
export function messageFor(t: TFunction<"web">, group: "error" | "signinError" | "ended", code: string): string {
  const key = `${group}.${code}`;
  return t(key, { defaultValue: "" }) || t(group === "ended" ? "error.generic" : `${group}.generic`, { code });
}

/**
 * 写回失败的原因（保存、新建、改名、删除，TDD-17 §3）：同 `error` 分组；`too_large` 带上限，按界面语言格式化
 * 字节数（上限缺席时不编一个数）。
 */
export function saveErrorText(t: TFunction<"web">, code: string, param: number | undefined, language: string): string {
  if (code === "too_large") {
    return param === undefined ? t("error.too_large_unknown") : t("error.too_large", { limit: formatBytes(param, language) });
  }
  if (code === "download_too_large") {
    return param === undefined
      ? t("error.download_too_large_unknown")
      : t("error.download_too_large", { limit: formatBytes(param, language) });
  }
  return messageFor(t, "error", code);
}
