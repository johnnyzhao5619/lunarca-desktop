//! SaaS 收单的客户端一侧（纵切 3 C2，协议 v10）：价目、结账、账单管理的线上形状与界面 DTO。
//!
//! # 三件事，全在服务端说了算
//!
//! - **卖不卖**：capabilities 的 `billing.enabled`（[`crate::protocol::BillingDto`]）。为假是私有化
//!   的正常形态——界面一个升级 / 购买 / 管理入口都不出现，而不是摆一排点了 503 的按钮。
//! - **卖什么、多少钱、账怎么算**：公开的 `GET /v1/billing/plans`。客户端不持有任何 Stripe price
//!   id（ADR-073 ⑥），结账只回传那一项的 `plan_id`。
//! - **付款与退订**：都在 Stripe 的托管页上（`POST /v1/billing/checkout` / `portal` 只交回一个
//!   URL，PRD-13-017 取消期末生效也由那一侧执行）。我方进程里
//!   不出现卡号；桌面端只负责把那个 URL 交给系统浏览器——**URL 本身不进日志**（它带着会话标识）。
//!
//! 线上形状与界面 DTO 分开：线上是 `i64`（货币最小单位、微 credit），而 specta 禁 64 位整数入 TS；
//! 界面要的是 credits 而不是微 credit——换算在这里做一次，前端不再算（ADR-003）。

use serde::{Deserialize, Serialize};

use crate::error::CloudError;

/// 1 credit = 10⁶ 微 credit（协议定死的量纲，TDD-16 §7.1b；
/// ADR-074 ⑨：一次部署一个锚、改锚是币制重定）。
/// 客户端里一切微 credit 换算（价目、用量、月度上限、预估、删除预览）只读这一个常量：协议 v17
/// 起价目不再随带换算比——同一个量纲有两处来源，就有「两处不一致时信谁」这个本不该存在的问题。
pub const MICRO_CREDITS_PER_CREDIT: i64 = 1_000_000;

/// 微 credit → credit（按协议量纲 [`MICRO_CREDITS_PER_CREDIT`]；界面只见 credit，换算只在
/// Rust，ADR-003）。
#[must_use]
pub fn micro_to_credits(micro: i64) -> f64 {
    micro as f64 / MICRO_CREDITS_PER_CREDIT as f64
}

/// 用户输入的 credit 数 → 线上的微 credit（月度上限，纵切 3 C4）。负数、非有限值、大到 `i64`
/// 存不下的一律 [`CloudError::InvalidSpendCap`]——发请求前就拒，不让服务端替我们说「读不懂」。
/// 不足一个微 credit 的零头四舍五入（credit 精度远到不了那里）。
///
/// # Errors
/// 见上。
pub fn credits_to_micro(credits: f64) -> Result<i64, CloudError> {
    if !credits.is_finite() || credits < 0.0 {
        return Err(CloudError::InvalidSpendCap);
    }
    let micro = (credits * MICRO_CREDITS_PER_CREDIT as f64).round();
    // `i64::MAX as f64` 向上取整到 2^63，故用 `<` 而不是 `<=`。
    if micro < i64::MAX as f64 {
        Ok(micro as i64)
    } else {
        Err(CloudError::InvalidSpendCap)
    }
}

/// 这台服务器的收单状态（账户页「升级与购买」一节的唯一数据源）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum BillingOfferDto {
    /// 本部署未启用收单（私有化）：不出现任何升级 / 购买 / 管理订阅入口。
    Disabled,
    /// 在售的档与包 + 账单口径 + 这个账户此刻的订阅处境与付款处境。
    Enabled {
        plans: BillingPlansDto,
        standing: SubscriptionStandingDto,
        /// 付款处境（权益 `billing_standing`，协议 v11）；权益那一次取不到 = `None`（不出横幅）。
        payment: Option<BillingStandingDto>,
        /// 能不能下单（「订阅」与「购买」两种按钮）。由 [`purchasable`] 判定：账户冻结时为假
        /// ——服务端那时对结账回 403，摆着按钮只会让用户点一次失败。
        purchasable: bool,
    },
}

/// 这个账户的付款处境（权益 `billing_standing`，协议 v11 必填；纵切 3 C3）。线上形状与界面
/// 形状同一份：`state` 标签 + snake_case，宽限截止是 RFC 3339 字符串（同 `DeviceDto.created_at`，
/// 本地化在界面层）。私有化部署恒为 `good`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum BillingStandingDto {
    Good,
    /// 付款失败、Stripe 正在重试：服务照常，`grace_ends_at` 前仍没付上就退订并降到免费档
    /// （PRD-13-016）。本地数据不受影响。
    PastDue {
        grace_ends_at: String,
    },
    /// 付款争议（拒付）：账户只读——不消耗、不删除，直到运营人工处理。
    Frozen,
}

/// 这个账户的免费额度处境（权益 `free_quota`，协议 v13 必填；纵切 3 C5，
/// PRD-13-018）。线上形状与界面形状同一份。
///
/// 只用于账户页的说明，**不**拿它预先拦请求：服务端预留才是判官（扣下时回 402
/// `free_quota_withheld`），用户验证完付款方式或运营放行之后不该等客户端重取权益才恢复。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum FreeQuotaDto {
    /// 这个账户不在免费档上（订阅、组织），或这台部署不发免费额度：没有可说的。
    NotApplicable,
    /// 免费额度照常发放。
    Granted,
    /// 免费额度被扣下；预付 credits 与付费订阅照常可用。
    Withheld { reason: FreeQuotaWithheldReasonDto },
}

/// 免费额度为什么被扣下（L1 / L2，PRD-13-018）。每一条对应一个不同的下一步。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum FreeQuotaWithheldReasonDto {
    /// 这台设备上已有别的账户领过免费额度（免费额度挂「账户 × 设备」）。下一步：用预付或订阅；
    /// 认为判错了就联系支持申诉——申诉不要求提供更多个人信息。
    SharedDevice,
    /// 风险信号触发了一次零扣费的付款方式验证（L2）。下一步：验证付款方式（不扣费）。
    VerificationRequired,
}

/// 付款方式验证（`POST /v1/billing/verify-card`）失败 → 这一侧的说法：409 `verification_not_required`
/// 是「已经不需要验证了」（界面据此重取权益），503 `billing_disabled` 照收单一族说；404 不改写——这一项
/// 没有「已下架」可言，照通用服务端错误说。
pub(crate) fn map_verify_card_error(err: CloudError) -> CloudError {
    match err {
        CloudError::Server {
            status: 409,
            ref error_code,
        } if error_code == "verification_not_required" => CloudError::VerificationNotRequired,
        other => {
            let unchanged = other.clone();
            map_billing_error(other, unchanged)
        }
    }
}

/// 能不能下单：只有冻结时不能（催缴期照常可买——服务照常，用户裁决 2026-10-14）；付款处境不知道时
/// 按能处理，真被拒时结账回 403，界面照 `account_frozen` 说明。
#[must_use]
pub fn purchasable(payment: Option<&BillingStandingDto>) -> bool {
    !matches!(payment, Some(BillingStandingDto::Frozen))
}

/// 这个账户与在售订阅档的关系——决定「订阅」与「管理订阅」给不给（[`subscription_standing`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionStandingDto {
    /// 档位取不到（权益那一次失败）：订阅与管理都不给——不知道就不冒「第二份订阅」的险。
    Unknown,
    /// 当前档不是在售的任何订阅档（免费、组织）：可以订阅，没有可管理的订阅。
    NotSubscribed,
    /// 正在某个在售订阅档上：再结一次账会得到**第二份**订阅，改档与退订只走账单管理页。
    Subscribed,
}

/// 在售价目（`GET /v1/billing/plans`）。两个列表而不是「一个列表 + 按种类才有值的字段」——
/// 与服务端同一裁决（ADR-046 ④）：每个字段在它出现的地方恒有值。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct BillingPlansDto {
    /// 订阅档，服务端已按档位从低到高排好。
    pub subscriptions: Vec<SubscriptionPlanDto>,
    /// 预付包，服务端已按价格从低到高排好。
    pub prepaid_packs: Vec<PrepaidPackDto>,
    pub rules: BillingRulesDto,
}

/// 展示价。**只供展示**：真正扣多少以 Stripe 付款页上的数字为准。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct PriceDto {
    /// 货币最小单位（美分 / 欧分 / 日元本身）。f64 而不是整数：specta 禁 64 位整数入 TS，
    /// 而 TS 的 `number` 在 2^53 内精确。
    pub unit_amount: f64,
    /// ISO 4217 货币代码（Stripe 的小写原文，原样交给 `Intl`）。
    pub currency: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct SubscriptionPlanDto {
    /// 结账时回传的标识（`plan_id`）。
    pub id: String,
    /// `pro` / `max`（与权益 `tier` 同一套取值）。
    pub tier: String,
    /// 运营录入的展示名。
    pub display_name: String,
    pub price: PriceDto,
    /// 计费周期（服务端恒为 `month`）。
    pub interval: String,
    pub included: IncludedQuotaDto,
    /// 是不是这个账户此刻所在的档（由 [`subscription_standing`] 按权益 `tier` 标出）。
    pub current: bool,
}

/// 这一档含的额度：与 `/v1/usage` 订阅账本同一套读法。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct IncludedQuotaDto {
    /// 重置周期（`monthly` 等，与账本 `period` 同一套取值）。
    pub period: String,
    pub dimensions: Vec<IncludedDimensionDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct IncludedDimensionDto {
    pub dimension: String,
    /// `None` = 不限（同 [`crate::client::UsageDimensionDto::limit`] 的 f64 理由）。
    pub limit: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct PrepaidPackDto {
    /// 结账时回传的标识（`plan_id`）。
    pub id: String,
    pub display_name: String,
    pub price: PriceDto,
    /// 买下后到账的 credits（由线上的微 credit 按 [`MICRO_CREDITS_PER_CREDIT`] 换算）。
    pub credits: f64,
}

/// 账单口径（PRD-13-037/038）。服务端每一格读的都是判定代码
/// 用的那份常量；客户端原样呈现，不另写一份说明。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct BillingRulesDto {
    /// 扣减顺序（账本种类，如 `["subscription", "prepaid"]`）。
    pub ledger_order: Vec<String>,
    /// 一次任务能否在两本账之间拆分（服务端恒为假：整笔只记在一本账上）。
    pub splits_across_ledgers: bool,
    /// 预付 credits 会不会过期（服务端恒为假）。
    pub prepaid_credits_expire: bool,
    /// 有取整的维度及其最小计费单元。**只列单元大于 1 的**：按 1 计的维度就是按实计，
    /// 列出来只是一行「不取整」的噪声。
    pub minimum_billable_units: Vec<MinimumBillableUnitDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct MinimumBillableUnitDto {
    pub dimension: String,
    /// 该维度原生单位下的最小计费单元（如 `asr_seconds` 的 60 = 不足一分钟按一分钟）。
    pub unit: f64,
}

/// 结账 / 账单管理托管页的地址（只在 Rust 内流转，不过 IPC、不进日志）。
#[derive(Debug, Deserialize)]
pub(crate) struct HostedSessionWire {
    pub url: String,
}

// ---- 线上形状（cp-api `PlansResponse`）----

#[derive(Debug, Deserialize)]
pub(crate) struct PlansWire {
    subscriptions: Vec<SubscriptionPlanWire>,
    prepaid_packs: Vec<PrepaidPackWire>,
    billing_rules: BillingRulesWire,
}

#[derive(Debug, Deserialize)]
struct PriceWire {
    unit_amount: i64,
    currency: String,
}

#[derive(Debug, Deserialize)]
struct SubscriptionPlanWire {
    id: String,
    tier: String,
    display_name: String,
    price: PriceWire,
    interval: String,
    included: IncludedQuotaWire,
}

#[derive(Debug, Deserialize)]
struct IncludedQuotaWire {
    period: String,
    dimensions: Vec<IncludedDimensionWire>,
}

#[derive(Debug, Deserialize)]
struct IncludedDimensionWire {
    dimension: String,
    limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct PrepaidPackWire {
    id: String,
    display_name: String,
    price: PriceWire,
    grant_micro_credits: i64,
}

#[derive(Debug, Deserialize)]
struct BillingRulesWire {
    ledger_order: Vec<String>,
    splits_across_ledgers: bool,
    prepaid_credits_expire: bool,
    minimum_billable_units: Vec<MinimumBillableUnitWire>,
}

#[derive(Debug, Deserialize)]
struct MinimumBillableUnitWire {
    dimension: String,
    unit: i64,
}

impl From<PriceWire> for PriceDto {
    fn from(wire: PriceWire) -> Self {
        Self {
            unit_amount: wire.unit_amount as f64,
            currency: wire.currency,
        }
    }
}

impl PlansWire {
    /// 线上 → 界面。
    pub(crate) fn into_dto(self) -> BillingPlansDto {
        BillingPlansDto {
            subscriptions: self
                .subscriptions
                .into_iter()
                .map(|plan| SubscriptionPlanDto {
                    id: plan.id,
                    tier: plan.tier,
                    display_name: plan.display_name,
                    price: plan.price.into(),
                    interval: plan.interval,
                    current: false,
                    included: IncludedQuotaDto {
                        period: plan.included.period,
                        dimensions: plan
                            .included
                            .dimensions
                            .into_iter()
                            .map(|dim| IncludedDimensionDto {
                                dimension: dim.dimension,
                                limit: dim.limit.map(|limit| limit as f64),
                            })
                            .collect(),
                    },
                })
                .collect(),
            prepaid_packs: self
                .prepaid_packs
                .into_iter()
                .map(|pack| PrepaidPackDto {
                    id: pack.id,
                    display_name: pack.display_name,
                    price: pack.price.into(),
                    credits: micro_to_credits(pack.grant_micro_credits),
                })
                .collect(),
            rules: BillingRulesDto {
                ledger_order: self.billing_rules.ledger_order,
                splits_across_ledgers: self.billing_rules.splits_across_ledgers,
                prepaid_credits_expire: self.billing_rules.prepaid_credits_expire,
                minimum_billable_units: self
                    .billing_rules
                    .minimum_billable_units
                    .into_iter()
                    .filter(|unit| unit.unit > 1)
                    .map(|unit| MinimumBillableUnitDto {
                        dimension: unit.dimension,
                        unit: unit.unit as f64,
                    })
                    .collect(),
            },
        }
    }
}

/// 按权益里的当前档位标出 `current`，并给出订阅处境（`tier = None` = 档位取不到）。
///
/// 判据只有一条：当前档等于某个**在售**订阅档的 `tier` 才算已订阅。免费与组织（私有化）档不在
/// 售卖列表里，于是天然落在「未订阅」——不在客户端另维护一张「哪些档是付费档」的名单。
pub fn subscription_standing(
    plans: &mut BillingPlansDto,
    tier: Option<&str>,
) -> SubscriptionStandingDto {
    for plan in &mut plans.subscriptions {
        plan.current = tier == Some(plan.tier.as_str());
    }
    match tier {
        None => SubscriptionStandingDto::Unknown,
        Some(_) if plans.subscriptions.iter().any(|plan| plan.current) => {
            SubscriptionStandingDto::Subscribed
        }
        Some(_) => SubscriptionStandingDto::NotSubscribed,
    }
}

/// 服务端的 billing 端点失败 → 这一侧的说法。
///
/// 不并进通用的 `Server { status }`：通用那一档对 404 说「这个地址上没有 Lunarca 服务端」，
/// 而这里的 404 各有确切含义——结账时是「这一项已不在售」，管理订阅时是「这个账户还没付过款」。
pub(crate) fn map_billing_error(err: CloudError, not_found: CloudError) -> CloudError {
    match err {
        CloudError::Server { status: 404, .. } => not_found,
        CloudError::Server {
            status: 503,
            ref error_code,
        } if error_code == "billing_disabled" => CloudError::BillingDisabled,
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLANS: &str = r#"{
        "subscriptions":[{"id":"7d1c0e9e-0000-4000-8000-000000000001","tier":"pro","display_name":"Pro",
            "price":{"unit_amount":999,"currency":"usd"},"interval":"month",
            "included":{"period":"monthly","dimensions":[
                {"dimension":"tokens_out","limit":2000000},{"dimension":"egress_bytes","limit":null}]}}],
        "prepaid_packs":[{"id":"7d1c0e9e-0000-4000-8000-000000000002","display_name":"Pack",
            "price":{"unit_amount":500,"currency":"usd"},"grant_micro_credits":600000000}],
        "billing_rules":{"ledger_order":["subscription","prepaid"],"splits_across_ledgers":false,
            "prepaid_credits_expire":false,
            "minimum_billable_units":[{"dimension":"tokens_in","unit":1},{"dimension":"asr_seconds","unit":60}]}
    }"#;

    #[test]
    fn plans_map_credits_limits_and_only_rounded_units() {
        let wire: PlansWire = serde_json::from_str(PLANS).unwrap();
        let dto = wire.into_dto();
        assert_eq!(dto.subscriptions[0].price.unit_amount, 999.0);
        assert_eq!(
            dto.subscriptions[0].included.dimensions[0].limit,
            Some(2_000_000.0)
        );
        assert_eq!(
            dto.subscriptions[0].included.dimensions[1].limit, None,
            "不限必须仍是 None"
        );
        assert_eq!(
            dto.prepaid_packs[0].credits, 600.0,
            "微 credit 换算成 credit"
        );
        let units: Vec<&str> = dto
            .rules
            .minimum_billable_units
            .iter()
            .map(|u| u.dimension.as_str())
            .collect();
        assert_eq!(units, vec!["asr_seconds"], "按 1 计的维度不列");
        assert_eq!(dto.rules.ledger_order, vec!["subscription", "prepaid"]);
    }

    fn plans() -> BillingPlansDto {
        let wire: PlansWire = serde_json::from_str(PLANS).unwrap();
        wire.into_dto()
    }

    #[test]
    fn standing_unknown_tier_offers_neither_subscribe_nor_manage() {
        let mut plans = plans();
        assert_eq!(
            subscription_standing(&mut plans, None),
            SubscriptionStandingDto::Unknown
        );
        assert!(plans.subscriptions.iter().all(|plan| !plan.current));
    }

    #[test]
    fn standing_tier_not_on_sale_is_not_subscribed() {
        for tier in ["free", "organization"] {
            let mut plans = plans();
            assert_eq!(
                subscription_standing(&mut plans, Some(tier)),
                SubscriptionStandingDto::NotSubscribed,
                "{tier}"
            );
            assert!(plans.subscriptions.iter().all(|plan| !plan.current));
        }
    }

    #[test]
    fn standing_on_a_sold_tier_is_subscribed_and_marks_that_plan() {
        let mut plans = plans();
        assert_eq!(
            subscription_standing(&mut plans, Some("pro")),
            SubscriptionStandingDto::Subscribed
        );
        assert!(plans.subscriptions[0].current);
    }

    #[test]
    fn billing_standing_decodes_all_three_wire_states() {
        let decode = |raw: &str| serde_json::from_str::<BillingStandingDto>(raw).unwrap();
        assert_eq!(decode(r#"{"state":"good"}"#), BillingStandingDto::Good);
        assert_eq!(
            decode(r#"{"state":"past_due","grace_ends_at":"2026-10-28T00:00:00Z"}"#),
            BillingStandingDto::PastDue {
                grace_ends_at: "2026-10-28T00:00:00Z".into()
            }
        );
        assert_eq!(decode(r#"{"state":"frozen"}"#), BillingStandingDto::Frozen);
        // 必填：催缴缺截止时刻、认不出的状态都是读不懂，不猜一个。
        assert!(serde_json::from_str::<BillingStandingDto>(r#"{"state":"past_due"}"#).is_err());
        assert!(serde_json::from_str::<BillingStandingDto>(r#"{"state":"suspended"}"#).is_err());
    }

    /// 免费额度处境（纵切 3 C5）：三种线上形状都认；`withheld` 缺成因、认不出的成因与状态都是读不懂，
    /// 不猜一个。
    #[test]
    fn free_quota_decodes_all_wire_states_and_rejects_the_rest() {
        let decode = |raw: &str| serde_json::from_str::<FreeQuotaDto>(raw);
        assert_eq!(
            decode(r#"{"state":"not_applicable"}"#).unwrap(),
            FreeQuotaDto::NotApplicable
        );
        assert_eq!(
            decode(r#"{"state":"granted"}"#).unwrap(),
            FreeQuotaDto::Granted
        );
        assert_eq!(
            decode(r#"{"state":"withheld","reason":"shared_device"}"#).unwrap(),
            FreeQuotaDto::Withheld {
                reason: FreeQuotaWithheldReasonDto::SharedDevice
            }
        );
        assert_eq!(
            decode(r#"{"state":"withheld","reason":"verification_required"}"#).unwrap(),
            FreeQuotaDto::Withheld {
                reason: FreeQuotaWithheldReasonDto::VerificationRequired
            }
        );
        for bad in [
            r#"{"state":"withheld"}"#,
            r#"{"state":"withheld","reason":"vpn"}"#,
            r#"{"state":"revoked"}"#,
        ] {
            assert!(decode(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn verify_card_errors_say_what_happened() {
        let not_required = CloudError::Server {
            status: 409,
            error_code: "verification_not_required".into(),
        };
        assert!(matches!(
            map_verify_card_error(not_required),
            CloudError::VerificationNotRequired
        ));
        let disabled = CloudError::Server {
            status: 503,
            error_code: "billing_disabled".into(),
        };
        assert!(matches!(
            map_verify_card_error(disabled),
            CloudError::BillingDisabled
        ));
        // 别的 409 / 404 不改写。
        for (status, code) in [(409, "conflict"), (404, "not_found")] {
            let err = CloudError::Server {
                status,
                error_code: code.into(),
            };
            assert!(
                matches!(map_verify_card_error(err), CloudError::Server { status: s, .. } if s == status),
                "{status}"
            );
        }
        // 冻结照冻结说（`check_status` 已认出）。
        assert!(matches!(
            map_verify_card_error(CloudError::AccountFrozen),
            CloudError::AccountFrozen
        ));
    }

    #[test]
    fn only_a_frozen_account_cannot_buy() {
        assert!(!purchasable(Some(&BillingStandingDto::Frozen)));
        assert!(purchasable(Some(&BillingStandingDto::Good)));
        assert!(purchasable(Some(&BillingStandingDto::PastDue {
            grace_ends_at: "2026-10-28T00:00:00Z".into()
        })));
        assert!(purchasable(None), "不知道时按能处理，服务端拒了再说明");
    }

    /// 月度上限的 credit ↔ 微 credit（纵切 3 C4）：换算只在这里，非法输入发请求前就拒。
    #[test]
    fn spend_cap_credits_convert_both_ways_and_reject_nonsense() {
        assert_eq!(credits_to_micro(50.0).unwrap(), 50_000_000);
        assert_eq!(
            credits_to_micro(0.0).unwrap(),
            0,
            "0 是合法上限：一分预付都不许花"
        );
        assert_eq!(credits_to_micro(1.234_567_8).unwrap(), 1_234_568);
        assert_eq!(micro_to_credits(12_500_000), 12.5);
        for bad in [-1.0, f64::NAN, f64::INFINITY, 1e20] {
            assert!(
                matches!(credits_to_micro(bad), Err(CloudError::InvalidSpendCap)),
                "{bad}"
            );
        }
    }

    /// 协议 v17：价目不带换算比，到账 credits 只按协议量纲换算（客户端没有第二处来源）。
    #[test]
    fn plans_without_a_ratio_convert_by_the_protocol_constant() {
        assert!(
            !PLANS.contains("micro_credits_per_credit"),
            "夹具就是 v17 的线上形状"
        );
        let pack = |grant: i64| {
            let body = PLANS.replace("600000000", &grant.to_string());
            let wire: PlansWire = serde_json::from_str(&body).unwrap();
            wire.into_dto().prepaid_packs[0].credits
        };
        assert_eq!(pack(MICRO_CREDITS_PER_CREDIT), 1.0);
        assert_eq!(pack(2_500_000), 2.5);
        assert_eq!(pack(600_000_000), micro_to_credits(600_000_000));
    }

    #[test]
    fn billing_errors_say_what_the_404_means_here() {
        let not_found = CloudError::Server {
            status: 404,
            error_code: "not_found".into(),
        };
        assert!(matches!(
            map_billing_error(not_found, CloudError::NoBillingAccount),
            CloudError::NoBillingAccount
        ));
        let disabled = CloudError::Server {
            status: 503,
            error_code: "billing_disabled".into(),
        };
        assert!(matches!(
            map_billing_error(disabled, CloudError::PlanUnavailable),
            CloudError::BillingDisabled
        ));
        // Stripe 那一侧不可用不是「未启用收单」：照通用服务端错误说。
        let stripe = CloudError::Server {
            status: 503,
            error_code: "stripe_unavailable".into(),
        };
        assert!(matches!(
            map_billing_error(stripe, CloudError::PlanUnavailable),
            CloudError::Server { status: 503, .. }
        ));
    }
}
