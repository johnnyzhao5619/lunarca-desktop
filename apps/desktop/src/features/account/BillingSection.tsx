/**
 * 「升级与购买」一节（纵切 3 C2，PRD-13-017/019/037/038）。
 *
 * **卖不卖由服务端说了算**（capabilities `billing.enabled`，协议 v10）：未启用收单的部署
 * （私有化）这一节整个不渲染——不摆点了只会 503 的按钮。卖什么、多少钱、账怎么算全来自
 * `GET /v1/billing/plans`（经 Rust 换算好的 DTO）；本组件只渲染，结账与账单管理交给 Rust
 * 打开系统浏览器（ADR-003）。
 *
 * 锚点 `settings-section-billing` 是「额度不足」失败卡上「升级或购买额度」的落点
 * （`settings://account/billing`，见 `i18n/failure.ts`），也是催缴 / 冻结通知与「账户被冻结」
 * 失败卡的落点——本节顶部的付款处境横幅说明为什么、该做什么（纵切 3 C3）。
 */

import { useTranslation } from "react-i18next";

import type {
  BillingOfferDto,
  BillingPlansDto,
  BillingRulesDto,
  BillingStandingDto,
  CloudError,
  SubscriptionPlanDto,
  SubscriptionStandingDto,
} from "../../ipc/cloud-bindings";
import { errorMessage } from "../../lib/cloud-error";
import { formatBytes, isByteDimension } from "../../lib/format-bytes";
import { formatCount, formatDate } from "../../lib/format-datetime";
import { formatMinorUnits } from "../../lib/format-money";
import { serverLabel } from "./server-label";

type Translate = (key: string, options?: Record<string, unknown>) => string;

/** 失败卡深链的尾段与本节锚点同一个名字（`settings://account/<它>`）。 */
export const BILLING_SECTION = "billing";

/** 正在交给浏览器的那一项：某个 `plan_id`，或账单管理页。 */
export type BillingPending = { kind: "checkout"; planId: string } | { kind: "portal" };

/** 已打开的托管页：回到应用时整页自动重取（见 `AccountPanel`）。 */
export type BillingOpened = "checkout" | "portal";

function amountText(dimension: string, value: number | null, locale: string, t: Translate): string {
  if (value === null) return t("billing.unlimited");
  return isByteDimension(dimension) ? formatBytes(value, locale) : formatCount(value, locale);
}

function dimensionLabel(dimension: string, t: Translate): string {
  return serverLabel(
    (key) => t(key, { defaultValue: "" }),
    `usage.dimension.${dimension}`,
    dimension,
    t("usage.dimension_other"),
  ).label;
}

/** 「$9.99 / 月 · 每月重置：输出 token 2,000,000 · 出站流量 不限」 */
function planDetail(plan: SubscriptionPlanDto, locale: string, t: Translate): string {
  const amount = formatMinorUnits(plan.price.unit_amount ?? 0, plan.price.currency, locale);
  // 认不出的周期只写价格：替服务端宣布一个我们并不知道的周期，比少一个词更糟。
  const price = t(`billing.price_per.${plan.interval}`, { price: amount, defaultValue: amount });
  const items = plan.included.dimensions
    .map((dim) =>
      t("billing.included_item", {
        dimension: dimensionLabel(dim.dimension, t),
        amount: amountText(dim.dimension, dim.limit, locale, t),
      }),
    )
    .join(t("billing.included_separator"));
  if (!items) return price;
  const period = t(`usage.period_${plan.included.period}`, { defaultValue: "" });
  const included = period ? t("billing.included", { period, items }) : items;
  return t("billing.plan_detail", { price, included });
}

/** 账单口径（PRD-13-037/038）：每一句都读服务端那份规则，不另写一份说明。 */
function BillingRules({ rules }: { rules: BillingRulesDto }) {
  const { t, i18n } = useTranslation("cloud");
  const ledgerName = (kind: string) =>
    t(`usage.ledger_${kind}`, { defaultValue: "" }) || t("usage.ledger_other");
  return (
    <div className="luna-settings-panel__ledger">
      <h3 className="luna-settings-panel__ledger-title">{t("billing.rules_title")}</h3>
      <ul className="cloud-account__rules">
        {rules.ledger_order.length > 0 ? (
          <li>
            {t("billing.rules.order", {
              order: rules.ledger_order.map(ledgerName).join(t("billing.rules.order_separator")),
            })}
          </li>
        ) : null}
        <li>{t(rules.splits_across_ledgers ? "billing.rules.split" : "billing.rules.no_split")}</li>
        <li>
          {t(
            rules.prepaid_credits_expire
              ? "billing.rules.prepaid_expires"
              : "billing.rules.prepaid_never_expires",
          )}
        </li>
        {rules.minimum_billable_units.map((unit) => {
          const amount = amountText(unit.dimension, unit.unit ?? 0, i18n.language, t);
          return (
            <li key={unit.dimension}>
              {t("billing.rules.minimum_unit", {
                dimension: dimensionLabel(unit.dimension, t),
                unit: amount,
              })}
            </li>
          );
        })}
      </ul>
    </div>
  );
}

/**
 * 付款处境横幅（纵切 3 C3，PRD-13-016）：催缴给截止日期与「更新付款方式」（打开账单管理页），冻结只说明、不给任何
 * 下单入口（能不能下单由 Rust 的 `purchasable` 说了算）。正常或不知道时不渲染。
 */
function PaymentStanding({
  payment,
  busy,
  onPortal,
}: {
  payment: BillingStandingDto | null;
  busy: boolean;
  onPortal: () => void;
}) {
  const { t, i18n } = useTranslation("cloud");
  if (payment === null || payment.state === "good") return null;
  if (payment.state === "frozen") {
    return (
      <p className="cloud-account__standing" data-tone="error" role="status">
        {t("billing.standing.frozen")}
      </p>
    );
  }
  return (
    <div className="cloud-account__standing" data-tone="warning" role="status">
      <p className="cloud-account__standing-text">
        {t("billing.standing.past_due", {
          date: formatDate(Date.parse(payment.grace_ends_at), i18n.language),
        })}
      </p>
      <button
        type="button"
        className="luna-btn luna-btn--secondary luna-btn--sm"
        disabled={busy}
        onClick={onPortal}
      >
        {t("billing.standing.past_due_action")}
      </button>
    </div>
  );
}

function PlanList({
  plans,
  standing,
  purchasable,
  pending,
  onCheckout,
  onPortal,
}: {
  plans: BillingPlansDto;
  standing: SubscriptionStandingDto;
  /** 能不能下单（Rust 判定，冻结时为假）：为假时「订阅」「购买」一律不给。 */
  purchasable: boolean;
  pending: BillingPending | null;
  onCheckout: (planId: string) => void;
  onPortal: () => void;
}) {
  const { t, i18n } = useTranslation("cloud");
  const locale = i18n.language;
  // 给不给「订阅」/「管理订阅」由 Rust 判定（`subscription_standing`）：已订阅时再结一次账
  // 会得到第二份订阅，档位不知道时两样都不给。这里只按答案渲染。
  const busy = pending !== null;
  const isPending = (planId: string) => pending?.kind === "checkout" && pending.planId === planId;

  return (
    <>
      {plans.subscriptions.length > 0 ? (
        <div className="luna-settings-panel__ledger">
          <h3 className="luna-settings-panel__ledger-title">{t("billing.subscriptions_title")}</h3>
          <ul className="luna-settings-panel__rows">
            {plans.subscriptions.map((plan) => (
              <li key={plan.id} className="luna-settings-panel__row">
                <div className="luna-settings-panel__row-label">
                  <span className="luna-settings-panel__row-title cloud-account__plan-name">
                    {plan.display_name}
                    {plan.current ? (
                      <span className="cloud-account__badge">{t("billing.current")}</span>
                    ) : null}
                  </span>
                  <p className="luna-settings-panel__row-desc">{planDetail(plan, locale, t)}</p>
                </div>
                {purchasable && standing === "not_subscribed" ? (
                  <button
                    type="button"
                    className="luna-btn luna-btn--secondary luna-btn--sm"
                    disabled={busy}
                    aria-busy={isPending(plan.id)}
                    onClick={() => onCheckout(plan.id)}
                  >
                    {t("billing.subscribe")}
                  </button>
                ) : (
                  <span />
                )}
              </li>
            ))}
          </ul>
          {standing === "unknown" ? (
            <p className="luna-settings-panel__hint">{t("billing.tier_unknown_hint")}</p>
          ) : standing === "subscribed" ? (
            <>
              <p className="luna-settings-panel__hint">{t("billing.switch_hint")}</p>
              <div className="luna-settings-panel__actions">
                <button
                  type="button"
                  className="luna-btn luna-btn--secondary"
                  disabled={busy}
                  aria-busy={pending?.kind === "portal"}
                  onClick={onPortal}
                >
                  {t("billing.manage")}
                </button>
              </div>
              <p className="luna-settings-panel__hint">{t("billing.manage_hint")}</p>
            </>
          ) : null}
        </div>
      ) : null}
      {plans.prepaid_packs.length > 0 ? (
        <div className="luna-settings-panel__ledger">
          <h3 className="luna-settings-panel__ledger-title">{t("billing.packs_title")}</h3>
          <ul className="luna-settings-panel__rows">
            {plans.prepaid_packs.map((pack) => (
              <li key={pack.id} className="luna-settings-panel__row">
                <div className="luna-settings-panel__row-label">
                  <span className="luna-settings-panel__row-title">{pack.display_name}</span>
                  <p className="luna-settings-panel__row-desc">
                    {t("billing.pack_detail", {
                      price: formatMinorUnits(pack.price.unit_amount ?? 0, pack.price.currency, locale),
                      credits: formatCount(pack.credits ?? 0, locale),
                    })}
                  </p>
                </div>
                {purchasable ? (
                  <button
                    type="button"
                    className="luna-btn luna-btn--secondary luna-btn--sm"
                    disabled={busy}
                    aria-busy={isPending(pack.id)}
                    onClick={() => onCheckout(pack.id)}
                  >
                    {t("billing.buy")}
                  </button>
                ) : (
                  <span />
                )}
              </li>
            ))}
          </ul>
        </div>
      ) : null}
    </>
  );
}

/**
 * 纯呈现：`offer === null` 且无错 = 还没取回（不渲染，免得私有化部署闪一下空节）；
 * `disabled` = 本部署不卖，整节不渲染。
 */
export function BillingSection({
  offer,
  error,
  pending = null,
  actionError = null,
  opened = null,
  onCheckout,
  onPortal,
}: {
  offer: BillingOfferDto | null;
  /** 取价目失败（已与其余区块同码的失败去重，见 `AccountPanel`）。 */
  error?: CloudError;
  pending?: BillingPending | null;
  /** 结账 / 账单管理这一下失败的原因。 */
  actionError?: CloudError | null;
  opened?: BillingOpened | null;
  onCheckout: (planId: string) => void;
  onPortal: () => void;
}) {
  const { t } = useTranslation("cloud");
  if (error === undefined && (offer === null || offer.state === "disabled")) return null;
  const enabled = offer?.state === "enabled" ? offer : null;
  const empty =
    enabled !== null && enabled.plans.subscriptions.length === 0 && enabled.plans.prepaid_packs.length === 0;
  return (
    <section
      id={`settings-section-${BILLING_SECTION}`}
      className="luna-settings-panel__section"
      aria-label={t("billing.title")}
      tabIndex={-1}
    >
      <h2 className="luna-settings-panel__section-title">{t("billing.title")}</h2>
      {enabled ? (
        <PaymentStanding payment={enabled.payment} busy={pending !== null} onPortal={onPortal} />
      ) : null}
      {error ? (
        <p className="luna-settings-panel__hint" role="status">
          {errorMessage(error, t)}
        </p>
      ) : null}
      {actionError ? (
        <p className="luna-settings-panel__hint" role="status">
          {errorMessage(actionError, t)}
        </p>
      ) : null}
      {opened ? (
        <p className="luna-settings-panel__hint" role="status">
          {t(opened === "portal" ? "billing.portal_opened" : "billing.opened")}
        </p>
      ) : null}
      {enabled === null ? null : empty ? (
        <p className="luna-empty">{t("billing.empty")}</p>
      ) : (
        <PlanList
          plans={enabled.plans}
          standing={enabled.standing}
          purchasable={enabled.purchasable}
          pending={pending}
          onCheckout={onCheckout}
          onPortal={onPortal}
        />
      )}
      {enabled === null ? null : <BillingRules rules={enabled.plans.rules} />}
    </section>
  );
}
