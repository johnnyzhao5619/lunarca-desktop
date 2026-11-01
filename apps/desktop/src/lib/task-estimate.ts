/**
 * 任务前预估的呈现（PRD-13-040 ②，纵切 3 C4）。
 *
 * 要不要估、估多少、从哪本账扣、够不够全由 Rust 答（`task_estimate` → `UsageEstimateDto`，分钟与
 * credits 已换算好，ADR-003）；这里只把那份答案拼成一句话。**每一句都带「约」**：最终账单以服务端
 * 计量为准，界面不得把预估说成定数。
 */

import type { EstimateShortfallDto, UsageEstimateDto } from "../ipc/bindings";
import { formatCount } from "./format-datetime";

type Translate = (key: string, options?: Record<string, unknown>) => string;

/** 不足的原因 → 对应的失败码：修复按钮与失败卡走同一张表（`i18n/failure.ts`）。 */
export function shortfallFailureCode(reason: EstimateShortfallDto): "QUOTA_EXHAUSTED" | "SPEND_CAP_REACHED" {
  return reason === "spend_cap" ? "SPEND_CAP_REACHED" : "QUOTA_EXHAUSTED";
}

/**
 * 「约消耗 90 分钟，约 12 credits（从预付扣）」。服务端没列出任何消耗项、或这一单本就扣不了
 * （不足那一支另有提示）时返回 `null`。
 */
export function estimateLine(estimate: UsageEstimateDto, t: Translate, locale: string): string | null {
  if (estimate.outcome.state === "insufficient" || estimate.amounts.length === 0) return null;
  const amounts = estimate.amounts
    .map((item) =>
      item.unit === "minutes"
        ? t("cloud:estimate.amount_minutes", { minutes: formatCount(item.minutes ?? 0, locale) })
        : t("cloud:estimate.amount_dimension", {
            dimension: t(`cloud:usage.dimension.${item.dimension}`, {
              defaultValue: t("cloud:usage.dimension_other"),
            }),
            amount: formatCount(item.amount ?? 0, locale),
          }),
    )
    .join(t("cloud:estimate.amount_separator"));
  const ledger =
    estimate.outcome.state === "prepaid"
      ? t("cloud:estimate.from_prepaid", { credits: formatCount(estimate.outcome.credits ?? 0, locale) })
      : t("cloud:estimate.from_subscription");
  return t("cloud:estimate.line", {
    consumption: t("cloud:estimate.consumption", { amounts }),
    ledger,
  });
}
