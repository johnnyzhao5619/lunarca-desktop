/**
 * 执行记录：这一页上的一张卡片，长度由列表分页封顶（ADR-126 第 7 条）。
 *
 * 它回答「这段时间跑了什么、在哪跑的、估算省下或花掉多少」；账户页的「订阅与用量」回答
 * 「我的套餐还剩多少」——两处都叫「用量」时，用户搜一个词会得到两个地方、两个数字。
 */

import { useTranslation } from "react-i18next";

import type { StatusVisitMetrics } from "../../hooks/use-status-visit-metrics";
import type { SettingSpecDto } from "../../ipc/bindings";
import { UsageExecutionView } from "../usage/UsageExecutionView";
import { PanelSettingRow } from "./PanelSettingRow";
import type { SetValue } from "./SettingControls";

/** 两个参考单价由本面板承载；它们同时喂工作台那一格「估算为你节省」（ADR-126 第 6 条）。 */
const PRICE_KEYS = ["provider.asr_cost_per_min", "provider.llm_cost_per_million_tokens"];

/** 这个深链尾段指向执行记录这张卡（`usage`），或它承载的两个单价键。 */
export function targetsUsage(focusKey: string | null): boolean {
  return focusKey === "usage" || PRICE_KEYS.includes(focusKey ?? "");
}

/** 单价那一节该不该展开：用户指名要去其中一个键时，收着就等于没到。 */
export function pricingOpen(focusKey: string | null): boolean {
  return PRICE_KEYS.includes(focusKey ?? "");
}

export function UsageExecutionPanel({
  focusKey,
  specOf,
  setValue,
  visit,
  advancedMode,
}: {
  focusKey: string | null;
  specOf: (key: string) => SettingSpecDto | undefined;
  setValue: SetValue;
  advancedMode: boolean;
  /**
   * 到访观测由**设置视图**持有（见那边的 `PagePanelContext.usageVisit`）：本卡随页面
   * 条件渲染，切走即卸载，自己报不了那一次到访的终点。
   */
  visit: StatusVisitMetrics;
}) {
  const { t } = useTranslation("settings");
  return (
    <section
      id="settings-section-usage"
      tabIndex={-1}
      className="luna-settings-panel luna-settings-card"
      aria-label={t("models.view.usage")}
    >
      <header className="luna-settings-panel__header">
        <h2>{t("models.view.usage")}</h2>
      </header>
      <p className="luna-settings-panel__hint">{t("models.library.usage_hint")}</p>
      <UsageExecutionView
        visit={visit}
        pricing={advancedMode ? (
          // 位置由视图决定（统计之后、明细之前）：单价是"影响上面那几个数的配置"。
          // 默认收起——多数人一次都不会改它，但深链指名要去时必须是开的。
          <details className="luna-settings-panel__note" open={pricingOpen(focusKey)}>
            <summary>{t("models.pricing_title")}</summary>
            <p className="luna-settings-panel__hint">{t("models.pricing_hint")}</p>
            {PRICE_KEYS.map((key) => (
              <PanelSettingRow key={key} spec={specOf(key)} setValue={setValue} />
            ))}
          </details>
        ) : null}
      />
    </section>
  );
}
