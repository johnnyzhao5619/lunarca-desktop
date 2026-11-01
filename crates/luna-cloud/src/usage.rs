//! 成本可预期性的客户端一侧（纵切 3 C4，协议 v12；PRD-13-040 ②③④）。
//!
//! 三件事都以服务端为准，客户端只换算、只认出、只呈现：
//!
//! - **阈值预警（③）**：`GET /v1/usage` 的每一维与月度上限都带服务端算好的档位（`none` / `p80` /
//!   `p95` / `exhausted`，[`UsageAlertDto`]——客户端唯一的一份档位），每本账另带一个「这一轮」的标识
//!   `alert_key`（**不透明串，只用来去重**：订阅账本是计数周期的起点、首次预留前为 `unstarted`，预付
//!   账本是累计购入）。客户端不再自己拿阈值比一遍——账户页的徽标色调、阈值通知与同步存储的
//!   「快满了 / 满了」读的是同一份服务端判定（ADR-121 ⑦）。本模块把一次取数认成若干**预警轮次**（[`UsageAlertEpisode`]），去重与
//!   发通知归装配层（落盘标记，见 `src-tauri` 的 `cloud::notice_usage_alerts`）。
//! - **月度上限（④）**：**只管预付 credits**，订阅包含的额度从不受它影响（用户裁决 2026-10-14）。
//!   周期是 UTC 自然月。`PUT /v1/spend-cap`；credit ↔ 微 credit 的换算与校验在 [`crate::billing`]。
//! - **任务前预估（②）**：`POST /v1/usage/estimate`。结果是「约」——最终账单以服务端计量为准，界面
//!   不得把它说成定数。
//!
//! 线上形状与界面 DTO 分开：线上是微 credit（`i64`），specta 禁 64 位整数入 TS，界面要的也是 credit
//! ——换算在这里做一次（ADR-003）。

use serde::{Deserialize, Serialize};

use crate::billing::micro_to_credits;
use crate::client::{usage_ratio, UsageAlertDto, UsageDimensionDto, UsageLedgerDto};

/// 预付账本在线上的计量维度：微 credit。界面上换算成 credit 并改名为 [`PREPAID_DIMENSION`]——
/// 把 `600000000` 摆给用户看，等于让他自己去数六个零。
const PREPAID_WIRE_DIMENSION: &str = "micro_credits";
/// 换算之后的预付维度名（`cloud:usage.dimension.credits`）。
pub const PREPAID_DIMENSION: &str = "credits";
/// 预付账本的种类名（`UsageLedgerDto.kind`）。
pub const PREPAID_LEDGER: &str = "prepaid";

// ---- 线上形状（cp-api `GET /v1/usage`，协议 v12）----

#[derive(Debug, Deserialize)]
pub(crate) struct UsageWire {
    ledgers: Vec<LedgerWire>,
    spend_cap: SpendCapWire,
}

#[derive(Debug, Deserialize)]
struct LedgerWire {
    kind: String,
    period: String,
    /// 这一轮预警的标识，**不透明**：只拿来认「还是不是同一轮」（订阅账本是计数周期的起点——周期按
    /// 锚点滚动而不是按自然月，故不解析它；预付账本是累计购入，再买一包即新的一轮）。
    alert_key: String,
    /// 维度直接按界面形状解（档位 `alert` 必填，见 [`UsageDimensionDto::alert`]）。
    dimensions: Vec<UsageDimensionDto>,
}

/// 月度上限的线上形状（`GET /v1/usage` 的 `spend_cap`，也是 `PUT /v1/spend-cap` 的答复）。
#[derive(Debug, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum SpendCapWire {
    Unset {
        spent_micro_credits: i64,
        period_key: String,
        period_ends_at: String,
    },
    Set {
        limit_micro_credits: i64,
        spent_micro_credits: i64,
        period_key: String,
        period_ends_at: String,
        alert: UsageAlertDto,
    },
}

// ---- 界面 DTO ----

/// 账户页「订阅与用量」一段的全部数据（`usage` 命令）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct UsageDto {
    /// 账本列表（PRD-13-038），每本账的维度已按离上限的距离排好（最紧的一维在最前，ADR-121 ⑥）。
    pub ledgers: Vec<UsageLedgerDto>,
    /// 本月预付上限（PRD-13-040 ④）。`None` = 这个账户既没有预付余额、也没设过上限——上限只管
    /// 预付 credits，对一个没有预付的账户摆出这一块只是噪声（私有化部署恒在此列）。
    pub spend_cap: Option<SpendCapDto>,
}

/// 本月预付上限（PRD-13-040 ④）。金额都已换算成 credit；周期截止是 RFC 3339（UTC 自然月末），
/// 本地化在界面层。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SpendCapDto {
    /// 没设上限：照样报本月已从预付扣了多少。
    Unset {
        spent_credits: f64,
        period_ends_at: String,
    },
    /// 设了上限：上限、本月已从预付扣的、离上限多近。
    Set {
        limit_credits: f64,
        spent_credits: f64,
        period_ends_at: String,
        /// 离上限多近（服务端判定）。
        alert: UsageAlertDto,
    },
}

impl SpendCapWire {
    pub(crate) fn period_key(&self) -> &str {
        match self {
            Self::Unset { period_key, .. } | Self::Set { period_key, .. } => period_key,
        }
    }

    fn level(&self) -> UsageAlertDto {
        match self {
            Self::Unset { .. } => UsageAlertDto::None,
            Self::Set { alert, .. } => *alert,
        }
    }

    fn is_set(&self) -> bool {
        matches!(self, Self::Set { .. })
    }

    pub(crate) fn into_dto(self) -> SpendCapDto {
        match self {
            Self::Unset {
                spent_micro_credits,
                period_ends_at,
                ..
            } => SpendCapDto::Unset {
                spent_credits: micro_to_credits(spent_micro_credits),
                period_ends_at,
            },
            Self::Set {
                limit_micro_credits,
                spent_micro_credits,
                period_ends_at,
                alert,
                ..
            } => SpendCapDto::Set {
                limit_credits: micro_to_credits(limit_micro_credits),
                spent_credits: micro_to_credits(spent_micro_credits),
                period_ends_at,
                alert,
            },
        }
    }
}

// ---- 预警轮次（PRD-13-040 ③）----

/// 一次预警「是关于什么的」——决定通知说哪一句、落在账户页哪一处。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AlertSubject {
    /// 某一本账（`subscription` / `prepaid` / 服务端将来新增的种类）里的某一维。
    Ledger { kind: String },
    /// 用户自设的本月预付上限。
    SpendCap,
}

/// 一次取数里认出的**一轮预警**：同一个 `id` 在它的一轮之内只该提醒一次、且只升不降。
///
/// `id` = `<账本种类>:<维度>@<alert_key>` 或 `spend_cap@<周期键 YYYY-MM>`——换周期 / 再买一包 / 进入
/// 新的自然月就是新的 `id`，也就是新的一轮（`alert_key` 原样拼进去，不解析）。档位为 `none` 的也列出来：装配层据此判断哪些旧标记已经
/// 不属于任何一轮、可以丢掉。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageAlertEpisode {
    pub id: String,
    pub subject: AlertSubject,
    pub level: UsageAlertDto,
}

/// 一维在界面上的样子：预付那一维换算成 credit 并改名，其余（含档位）原样。
fn dimension_dto(dim: &UsageDimensionDto) -> UsageDimensionDto {
    if dim.dimension != PREPAID_WIRE_DIMENSION {
        return dim.clone();
    }
    let credits = |micro: f64| micro / crate::billing::MICRO_CREDITS_PER_CREDIT as f64;
    UsageDimensionDto {
        dimension: PREPAID_DIMENSION.to_owned(),
        limit: dim.limit.map(credits),
        used: credits(dim.used),
        alert: dim.alert,
    }
}

/// 一本账的维度按「离上限的距离」降序重排（ADR-121 ⑥：最紧的一维在最前，第一行就是答案；行内徽标
/// 只贴这一行，界面按顺序取）。
///
/// **排序归客户端、档位归服务端**：距离是两个数相除，谁算都一样；而「到没到 80%」是一道判定，
/// 只能有一处——它在服务端（协议 v12），每一维的 `alert` 原样带到界面（ADR-121 ⑦）。
/// 稳定排序：无上限的维度没有距离可言，一律排到最后且保持服务端给的相对顺序。
fn rank_usage_dimensions(dims: &[UsageDimensionDto]) -> Vec<UsageDimensionDto> {
    let mut ranked: Vec<UsageDimensionDto> = dims.iter().map(dimension_dto).collect();
    ranked.sort_by(|a, b| match (usage_ratio(a), usage_ratio(b)) {
        (Some(x), Some(y)) => y.total_cmp(&x),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    ranked
}

impl UsageWire {
    /// 线上 → （界面 DTO，这一次认出的预警轮次）。两者出自同一份答复：页面与通知不可能各说一套。
    pub(crate) fn into_parts(self) -> (UsageDto, Vec<UsageAlertEpisode>) {
        let mut episodes = Vec::new();
        for ledger in &self.ledgers {
            for dim in &ledger.dimensions {
                episodes.push(UsageAlertEpisode {
                    id: format!("{}:{}@{}", ledger.kind, dim.dimension, ledger.alert_key),
                    subject: AlertSubject::Ledger {
                        kind: ledger.kind.clone(),
                    },
                    level: dim.alert,
                });
            }
        }
        episodes.push(UsageAlertEpisode {
            id: format!("spend_cap@{}", self.spend_cap.period_key()),
            subject: AlertSubject::SpendCap,
            level: self.spend_cap.level(),
        });
        let has_prepaid = self.ledgers.iter().any(|l| l.kind == PREPAID_LEDGER);
        let ledgers = self
            .ledgers
            .iter()
            .map(|ledger| UsageLedgerDto {
                kind: ledger.kind.clone(),
                period: ledger.period.clone(),
                dimensions: rank_usage_dimensions(&ledger.dimensions),
            })
            .collect();
        let spend_cap = (has_prepaid || self.spend_cap.is_set()).then(|| self.spend_cap.into_dto());
        (UsageDto { ledgers, spend_cap }, episodes)
    }
}

// ---- 任务前预估（PRD-13-040 ②）----

/// 要预估的量（`POST /v1/usage/estimate` 的 `quantity`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EstimateQuantity {
    /// 转写：音频秒数。
    AsrSeconds(u64),
    /// 文本生成：输入与输出 token。
    Tokens { input: u64, output: u64 },
}

impl EstimateQuantity {
    pub(crate) fn wire(self) -> serde_json::Value {
        match self {
            Self::AsrSeconds(seconds) => serde_json::json!({ "asr_seconds": seconds }),
            Self::Tokens { input, output } => serde_json::json!({
                "input_tokens": input,
                "output_tokens": output,
            }),
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct EstimateWire {
    dimensions: Vec<EstimateDimensionWire>,
    outcome: EstimateOutcomeWire,
}

#[derive(Debug, Deserialize)]
struct EstimateDimensionWire {
    dimension: String,
    amount: i64,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum EstimateOutcomeWire {
    Subscription,
    Prepaid { micro_credits: i64 },
    Insufficient { reason: EstimateShortfallDto },
}

/// 一次任务前预估（界面形状）。**全部是「约」**：最终账单以服务端计量为准。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct UsageEstimateDto {
    /// 约消耗多少（服务端按计量维度给出；转写那一维已换算成分钟）。
    pub amounts: Vec<EstimateAmountDto>,
    pub outcome: EstimateOutcomeDto,
}

/// 预估的一项消耗。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "unit", rename_all = "snake_case")]
pub enum EstimateAmountDto {
    /// 转写：约多少分钟（向上取整到整分钟——服务端本就按分钟取整计费）。
    Minutes { minutes: f64 },
    /// 其余计量维度原样（`tokens_in` / `tokens_out` / …）。
    Dimension { dimension: String, amount: f64 },
}

/// 这一单会从哪本账扣，或为什么扣不了。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum EstimateOutcomeDto {
    /// 订阅额度够：从订阅扣，不动预付。
    Subscription,
    /// 从预付 credits 扣，约这么多。
    Prepaid { credits: f64 },
    /// 不够：在起跑前说清，而不是跑到一半失败。
    Insufficient { reason: EstimateShortfallDto },
}

/// 不够的原因——两者的下一步不同：额度不够去升级或购买，撞上自设上限去调整上限。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum EstimateShortfallDto {
    Quota,
    SpendCap,
}

/// 转写的计量维度（与服务端 `usage_events` 同名，TDD-16 §7.2）。
const ASR_DIMENSION: &str = "asr_seconds";

impl EstimateWire {
    pub(crate) fn into_dto(self) -> UsageEstimateDto {
        let amounts = self
            .dimensions
            .into_iter()
            .filter(|dim| dim.amount > 0)
            .map(|dim| {
                if dim.dimension == ASR_DIMENSION {
                    EstimateAmountDto::Minutes {
                        minutes: (dim.amount as f64 / 60.0).ceil(),
                    }
                } else {
                    EstimateAmountDto::Dimension {
                        dimension: dim.dimension,
                        amount: dim.amount as f64,
                    }
                }
            })
            .collect();
        let outcome = match self.outcome {
            EstimateOutcomeWire::Subscription => EstimateOutcomeDto::Subscription,
            EstimateOutcomeWire::Prepaid { micro_credits } => EstimateOutcomeDto::Prepaid {
                credits: micro_to_credits(micro_credits),
            },
            EstimateOutcomeWire::Insufficient { reason } => {
                EstimateOutcomeDto::Insufficient { reason }
            }
        };
        UsageEstimateDto { amounts, outcome }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const USAGE: &str = r#"{
        "ledgers":[
            {"kind":"subscription","period":"monthly","alert_key":"2026-10-07T14:00:00Z","dimensions":[
                {"dimension":"tokens_in","limit":1000,"used":100,"alert":"none"},
                {"dimension":"tokens_out","limit":1000,"used":960,"alert":"p95"},
                {"dimension":"egress_bytes","limit":null,"used":5,"alert":"none"}]},
            {"kind":"prepaid","period":"none","alert_key":"600000000","dimensions":[
                {"dimension":"micro_credits","limit":600000000,"used":510000000,"alert":"p80"}]}],
        "spend_cap":{"state":"set","limit_micro_credits":50000000,"spent_micro_credits":49000000,
            "period_key":"2026-10","period_ends_at":"2026-11-01T00:00:00Z","alert":"p95"}
    }"#;

    fn parts(raw: &str) -> (UsageDto, Vec<UsageAlertEpisode>) {
        serde_json::from_str::<UsageWire>(raw).unwrap().into_parts()
    }

    fn usage_json() -> serde_json::Value {
        serde_json::from_str(USAGE).unwrap()
    }

    /// 协议 v12 的三格（维度档位、账本轮次、月度上限）都是**必填**：缺了就是读不懂，不猜一个。
    #[test]
    fn v12_fields_are_required() {
        assert!(serde_json::from_value::<UsageWire>(usage_json()).is_ok());
        let mut no_dim_alert = usage_json();
        no_dim_alert["ledgers"][0]["dimensions"][0]
            .as_object_mut()
            .unwrap()
            .remove("alert");
        let mut no_alert_key = usage_json();
        no_alert_key["ledgers"][0]
            .as_object_mut()
            .unwrap()
            .remove("alert_key");
        let mut no_cap = usage_json();
        no_cap.as_object_mut().unwrap().remove("spend_cap");
        let mut odd_level = usage_json();
        odd_level["ledgers"][0]["dimensions"][1]["alert"] = "p90".into();
        let mut set_without_alert = usage_json();
        set_without_alert["spend_cap"]
            .as_object_mut()
            .unwrap()
            .remove("alert");
        for broken in [
            no_dim_alert,
            no_alert_key,
            no_cap,
            odd_level,
            set_without_alert,
        ] {
            assert!(
                serde_json::from_value::<UsageWire>(broken.clone()).is_err(),
                "{broken}"
            );
        }
    }

    /// 每一维的档位原样来自服务端（排序之后跟着它那一维走）；预付那一维换算成 credit 并改名。
    #[test]
    fn levels_follow_the_server_and_prepaid_reads_in_credits() {
        let (dto, _) = parts(USAGE);
        let order: Vec<(&str, UsageAlertDto)> = dto.ledgers[0]
            .dimensions
            .iter()
            .map(|d| (d.dimension.as_str(), d.alert))
            .collect();
        assert_eq!(
            order,
            [
                ("tokens_out", UsageAlertDto::P95),
                ("tokens_in", UsageAlertDto::None),
                ("egress_bytes", UsageAlertDto::None)
            ]
        );
        let prepaid = &dto.ledgers[1].dimensions[0];
        assert_eq!(prepaid.dimension, PREPAID_DIMENSION);
        assert_eq!(prepaid.limit, Some(600.0));
        assert_eq!(prepaid.used, 510.0);
        assert_eq!(prepaid.alert, UsageAlertDto::P80);
    }

    /// 档位只认服务端：比例过了 80% 而服务端说 `none`，就是 `none`——判定只有一处。
    #[test]
    fn the_client_does_not_second_guess_the_server_level() {
        let mut raw = usage_json();
        raw["ledgers"][0]["dimensions"][1]["alert"] = "none".into();
        let (dto, _) = parts(&raw.to_string());
        // 最紧的仍是 tokens_out（排序归客户端），它的档位是服务端说的 none。
        assert_eq!(dto.ledgers[0].dimensions[0].dimension, "tokens_out");
        assert_eq!(dto.ledgers[0].dimensions[0].alert, UsageAlertDto::None);
        // 不透明的轮次标识原样进 id（订阅周期按锚点滚动，首次预留前是 `unstarted`）。
        raw["ledgers"][0]["alert_key"] = "unstarted".into();
        let (_, episodes) = parts(&raw.to_string());
        assert_eq!(episodes[0].id, "subscription:tokens_in@unstarted");
    }

    #[test]
    fn spend_cap_converts_and_shows_only_when_it_matters() {
        let (dto, _) = parts(USAGE);
        assert_eq!(
            dto.spend_cap,
            Some(SpendCapDto::Set {
                limit_credits: 50.0,
                spent_credits: 49.0,
                period_ends_at: "2026-11-01T00:00:00Z".into(),
                alert: UsageAlertDto::P95,
            })
        );
        let unset_cap = serde_json::json!({
            "state": "unset", "spent_micro_credits": 1_500_000,
            "period_key": "2026-10", "period_ends_at": "2026-11-01T00:00:00Z"
        });
        // 有预付、没设上限：照样报本月花了多少（界面说「未设上限」）。
        let mut unset = usage_json();
        unset["spend_cap"] = unset_cap.clone();
        assert_eq!(
            parts(&unset.to_string()).0.spend_cap,
            Some(SpendCapDto::Unset {
                spent_credits: 1.5,
                period_ends_at: "2026-11-01T00:00:00Z".into(),
            })
        );
        // 没有预付账本、也没设上限：上限那一块不出现。
        let mut no_prepaid = unset;
        no_prepaid["ledgers"].as_array_mut().unwrap().truncate(1);
        assert_eq!(parts(&no_prepaid.to_string()).0.spend_cap, None);
        // 没有预付账本、但设过上限：照样出现（用户写下的预算不能看不见）。
        let mut capped = usage_json();
        capped["ledgers"].as_array_mut().unwrap().truncate(1);
        assert!(matches!(
            parts(&capped.to_string()).0.spend_cap,
            Some(SpendCapDto::Set { .. })
        ));
    }

    /// 每一维、每一个上限都是一轮，`id` 带着它的轮次标识；档位为 none 的也列出（用于丢旧标记）。
    #[test]
    fn episodes_carry_their_round_and_cover_every_dimension_and_the_cap() {
        let (_, episodes) = parts(USAGE);
        let ids: Vec<(&str, UsageAlertDto)> =
            episodes.iter().map(|e| (e.id.as_str(), e.level)).collect();
        assert_eq!(
            ids,
            [
                (
                    "subscription:tokens_in@2026-10-07T14:00:00Z",
                    UsageAlertDto::None
                ),
                (
                    "subscription:tokens_out@2026-10-07T14:00:00Z",
                    UsageAlertDto::P95
                ),
                (
                    "subscription:egress_bytes@2026-10-07T14:00:00Z",
                    UsageAlertDto::None
                ),
                ("prepaid:micro_credits@600000000", UsageAlertDto::P80),
                ("spend_cap@2026-10", UsageAlertDto::P95),
            ]
        );
        assert_eq!(episodes[4].subject, AlertSubject::SpendCap);
        assert_eq!(
            episodes[3].subject,
            AlertSubject::Ledger {
                kind: "prepaid".into()
            }
        );
        assert!(
            UsageAlertDto::Exhausted > UsageAlertDto::P95
                && UsageAlertDto::P95 > UsageAlertDto::P80
        );
    }

    #[test]
    fn estimates_decode_and_convert_minutes_and_credits() {
        let decode = |raw: &str| {
            serde_json::from_str::<EstimateWire>(raw)
                .unwrap()
                .into_dto()
        };
        let asr = decode(
            r#"{"dimensions":[{"dimension":"asr_seconds","amount":3601}],
                "outcome":{"state":"prepaid","micro_credits":2500000}}"#,
        );
        assert_eq!(asr.amounts, [EstimateAmountDto::Minutes { minutes: 61.0 }]);
        assert_eq!(asr.outcome, EstimateOutcomeDto::Prepaid { credits: 2.5 });
        let llm = decode(
            r#"{"dimensions":[{"dimension":"tokens_in","amount":1200},{"dimension":"tokens_out","amount":0}],
                "outcome":{"state":"subscription"}}"#,
        );
        assert_eq!(
            llm.amounts,
            [EstimateAmountDto::Dimension {
                dimension: "tokens_in".into(),
                amount: 1200.0
            }],
            "为 0 的那一维不列"
        );
        assert_eq!(llm.outcome, EstimateOutcomeDto::Subscription);
        for (raw, reason) in [
            ("quota", EstimateShortfallDto::Quota),
            ("spend_cap", EstimateShortfallDto::SpendCap),
        ] {
            let short = decode(&format!(
                r#"{{"dimensions":[],"outcome":{{"state":"insufficient","reason":"{raw}"}}}}"#
            ));
            assert_eq!(short.outcome, EstimateOutcomeDto::Insufficient { reason });
        }
        assert!(serde_json::from_str::<EstimateWire>(
            r#"{"dimensions":[],"outcome":{"state":"insufficient","reason":"mystery"}}"#
        )
        .is_err());
        assert_eq!(
            EstimateQuantity::Tokens {
                input: 3,
                output: 4
            }
            .wire(),
            serde_json::json!({"input_tokens":3,"output_tokens":4})
        );
        assert_eq!(
            EstimateQuantity::AsrSeconds(90).wire(),
            serde_json::json!({"asr_seconds":90})
        );
    }
}
