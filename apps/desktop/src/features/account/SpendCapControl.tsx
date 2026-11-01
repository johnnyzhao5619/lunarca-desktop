/**
 * 本月预付上限（PRD-13-040 ④，纵切 3 C4）——「订阅与用量」一段里的一块，不新开分段（ADR-121 ⑩）。
 *
 * **只管预付 credits**：订阅包含的额度从不受它影响（用户裁决 2026-10-14）。到达上限后需要预付的
 * 任务不再开始，进行中的停在已产出处、按实计费（PRD-13-019 流式硬边界）。周期是 UTC 自然月。
 *
 * 前端零业务（ADR-003）：本月花了多少、上限多少、离上限多近、credit ↔ 微 credit 的换算与校验全在
 * Rust（`usage` / `spend_cap_set`）；这里只渲染、只把输入的数交回去。锚点 `settings-section-spend_cap`
 * 是「已达到本月预付上限」失败卡、任务前预估的不足提示与上限通知的落点（`settings://account/spend_cap`）。
 */

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { FactRow } from "../../components/ui/FactRow";
import type { CloudError, SpendCapDto, UsageAlertDto } from "../../ipc/cloud-bindings";
import { errorMessage } from "../../lib/cloud-error";
import { formatCount, formatDate } from "../../lib/format-datetime";

/** 深链尾段与本块锚点同一个名字（`settings://account/<它>`）。 */
export const SPEND_CAP_SECTION = "spend_cap";

/**
 * 档位徽标文案（档位是服务端判定的那一格，协议 v12；`none` 不出徽标）。穷尽 Record——Rust 新增一档
 * 时 tsc 当场红。`data-alert` 直接写档位名，色调归 `account.css`。
 */
export const USAGE_ALERT_COPY: Record<Exclude<UsageAlertDto, "none">, string> = {
  p80: "usage.alert.p80",
  p95: "usage.alert.p95",
  exhausted: "usage.alert.exhausted",
};

/** 输入框里的初值：设了上限就是那个数，没设就空着。 */
function draftOf(cap: SpendCapDto): string {
  return cap.state === "set" ? String(cap.limit_credits ?? 0) : "";
}

export function SpendCapControl({
  cap,
  pending = false,
  error = null,
  onSave,
}: {
  cap: SpendCapDto;
  /** 设 / 清上限在途。 */
  pending?: boolean;
  /** 上一次设 / 清失败的原因。 */
  error?: CloudError | null;
  /** 交回新的上限（credits）；`null` = 清除。 */
  onSave: (limitCredits: number | null) => void;
}) {
  const { t, i18n } = useTranslation("cloud");
  const locale = i18n.language;
  const [draft, setDraft] = useState(() => draftOf(cap));
  // 重取之后（设完、别处改了、换了月份）输入框回到服务端的那个数。
  useEffect(() => setDraft(draftOf(cap)), [cap]);
  // 输入框交来的是文本：空着或不是数时不给保存——绝不能把「没填」当成「清除」交出去。
  const value = draft.trim() === "" ? Number.NaN : Number(draft);
  const savable = Number.isFinite(value) && !(cap.state === "set" && value === cap.limit_credits);
  const spent = formatCount(cap.spent_credits ?? 0, locale);
  const resets = formatDate(Date.parse(cap.period_ends_at), locale);

  return (
    <div
      id={`settings-section-${SPEND_CAP_SECTION}`}
      className="luna-settings-panel__ledger"
      role="group"
      aria-label={t("usage.spend_cap.title")}
      tabIndex={-1}
    >
      <h3 className="luna-settings-panel__ledger-title">
        {resets ? t("usage.spend_cap.heading", { date: resets }) : t("usage.spend_cap.title")}
      </h3>
      <div className="luna-settings-panel__rows">
        <FactRow
          label={t("usage.spend_cap.spent_label")}
          value={
            cap.state === "set" ? (
              <>
                <progress
                  className="luna-progress"
                  value={cap.spent_credits ?? 0}
                  max={cap.limit_credits ?? 0}
                />
                <span className="cloud-account__amount">
                  {t("usage.spend_cap.amount", {
                    spent,
                    limit: formatCount(cap.limit_credits ?? 0, locale),
                  })}
                </span>
                {cap.alert !== "none" ? (
                  <span className="cloud-account__badge" data-alert={cap.alert}>
                    {t(USAGE_ALERT_COPY[cap.alert])}
                  </span>
                ) : null}
              </>
            ) : (
              t("usage.spend_cap.amount_unset", { spent })
            )
          }
        />
      </div>
      <label className="cloud-account__field">
        <span>{t("usage.spend_cap.input_label")}</span>
        <input
          type="number"
          className="luna-input"
          min={0}
          step="any"
          inputMode="decimal"
          value={draft}
          disabled={pending}
          onChange={(event) => setDraft(event.currentTarget.value)}
        />
      </label>
      <div className="luna-settings-panel__actions">
        <button
          type="button"
          className="luna-btn luna-btn--secondary"
          disabled={pending || !savable}
          aria-busy={pending}
          onClick={() => onSave(value)}
        >
          {t("usage.spend_cap.save")}
        </button>
        {cap.state === "set" ? (
          <button
            type="button"
            className="luna-btn luna-btn--secondary"
            disabled={pending}
            onClick={() => onSave(null)}
          >
            {t("usage.spend_cap.clear")}
          </button>
        ) : null}
      </div>
      {error ? (
        <p className="luna-settings-panel__hint" role="status">
          {errorMessage(error, t)}
        </p>
      ) : null}
      <p className="luna-settings-panel__hint">{t("usage.spend_cap.hint")}</p>
    </div>
  );
}
