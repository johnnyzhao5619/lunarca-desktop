//! 账户数据权利的客户端一侧（纵切 3 C6，协议 v14；用户可见规则见
//! PRD-13-020 与 `PRD-13-041`）：导出我的账户数据、注销前的后果预览、
//! 注销回执里的保留范围。
//!
//! # 三件事
//!
//! - **导出**（访问权 / 可携带权）：`GET /v1/account/export` 是一份可能很大的 JSON 附件。Rust 弹保存
//!   对话框、把响应体**流式**写进用户选的位置（[`stream_to_file`]）；用户取消对话框时一个字节都不发。
//!   导出里没有笔记——笔记在用户自己的库里，服务端只有它读不懂的同步密文。
//! - **注销前预览**：`GET /v1/account/deletion-preview` 说清删了之后会失去什么（在扣费的订阅、预付
//!   余额）与留下什么（计费与用量记录的保留年限），以及**此刻能不能删**（协议 v17 `deletion`：组织里
//!   唯一的所有者、组织还有别的成员时不能删）。微 credit 在这里换算成 credit，前端不再算（ADR-003）。
//! - **注销回执**：`DELETE /v1/account` 只回保留年限，文案由界面按语言渲染；503 `billing_unavailable`
//!   = Stripe 退不掉订阅、什么都没删（[`CloudError::BillingUnavailable`]）；409 `last_org_owner` =
//!   预览之后处境变了、服务端照样拦下，什么都没删（[`CloudError::LastOrgOwner`]）。

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt as _;

use crate::billing::micro_to_credits;
use crate::client::CloudClient;
use crate::error::CloudError;

/// 注销时这个账户的订阅处境（线上形状与界面形状同一份：`state` 标签 + snake_case）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DeletionSubscriptionDto {
    /// 没有在扣费的订阅（免费档、组织、已退订到期）。
    None,
    /// 有在扣费的订阅：注销会立即退订，本期不按比例退款。`tier` 与权益 `tier` 同一套取值
    /// （`pro` / `max`），界面按它取档位名，认不出走与权益同一条兜底。
    Active { tier: String },
}

/// 此刻能不能注销（线上形状与界面形状同一份；判定在服务端，客户端只照着渲染，ADR-003）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AccountDeletionDto {
    Allowed,
    /// 不能删：确认按钮不给，界面按 `reason` 说清下一步。
    Blocked {
        reason: DeletionBlockedReasonDto,
    },
}

/// 不能注销的成因（协议 v17；认不出的成因是读不懂，不猜一个）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum DeletionBlockedReasonDto {
    /// 这个账户是某个组织里唯一的所有者，而组织还有其他成员：删了就没人能管这个组织。
    /// 下一步是先把所有者转给别的成员，或请管理员处理。
    LastOrgOwner,
}

/// 注销前的后果预览（界面形状）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct AccountDeletionPreviewDto {
    /// 此刻能不能删（不能删时界面不给确认按钮）。
    pub deletion: AccountDeletionDto,
    pub subscription: DeletionSubscriptionDto,
    /// 注销会作废的预付余额（credits，由线上的微 credit 换算；f64 同 `PrepaidPackDto.credits`）。
    pub prepaid_credits: f64,
    /// 计费与用量记录按法定义务保留几年（不带邮箱）。
    pub billing_retention_years: u32,
}

/// `GET /v1/account/deletion-preview` 的线上形状。
#[derive(Debug, Deserialize)]
pub(crate) struct DeletionPreviewWire {
    deletion: AccountDeletionDto,
    subscription: DeletionSubscriptionDto,
    prepaid_micro_credits: i64,
    billing_retention_years: u32,
}

impl DeletionPreviewWire {
    pub(crate) fn into_dto(self) -> AccountDeletionPreviewDto {
        AccountDeletionPreviewDto {
            deletion: self.deletion,
            subscription: self.subscription,
            prepaid_credits: micro_to_credits(self.prepaid_micro_credits),
            billing_retention_years: self.billing_retention_years,
        }
    }
}

/// 注销回执：服务端**留下了什么**。删掉的东西（同步密文、密钥信封、账户元数据）不必回传——那是注销
/// 本身的定义；要明示的是保留范围（PRD-13 §1.6）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct AccountDeleteDto {
    /// 计费与用量记录按法定义务保留几年（不带邮箱）。
    pub billing_retention_years: u32,
}

/// `DELETE /v1/account` 的线上形状。
#[derive(Debug, Deserialize)]
pub(crate) struct DeleteWire {
    deleted: bool,
    retained: RetainedWire,
}

#[derive(Debug, Deserialize)]
struct RetainedWire {
    billing_retention_years: u32,
}

impl DeleteWire {
    /// `deleted` 不为真是契约破损：服务端没有说删掉了，就不能当成删掉了——如实报读不懂，
    /// 调用方据此**不清会话**。
    pub(crate) fn into_dto(self, host: &str) -> Result<AccountDeleteDto, CloudError> {
        if !self.deleted {
            return Err(CloudError::UnreadableResponse {
                host: host.to_owned(),
                detail: "account deletion response did not confirm deletion".to_owned(),
            });
        }
        Ok(AccountDeleteDto {
            billing_retention_years: self.retained.billing_retention_years,
        })
    }
}

/// 注销失败 → 这一侧的说法：503 `billing_unavailable` 是「订阅退不掉、什么都没删」，不是通用的
/// 「服务端没起来」（那一档的下一步是找管理员，而这里的下一步是稍后再试）；409 `last_org_owner`
/// 是「你是组织唯一的所有者、什么都没删」（下一步是先转让所有者），不是通用的「服务端返回了错误」。
pub(crate) fn map_delete_error(err: CloudError) -> CloudError {
    match err {
        CloudError::Server {
            status: 503,
            ref error_code,
        } if error_code == "billing_unavailable" => CloudError::BillingUnavailable,
        CloudError::Server {
            status: 409,
            ref error_code,
        } if error_code == "last_org_owner" => CloudError::LastOrgOwner,
        other => other,
    }
}

/// 导出的结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum AccountExportDto {
    /// 已写好；`path` 是用户自己选的位置（界面原样显示，**不进日志**）。
    Saved { path: String },
    /// 用户取消了保存对话框：什么都没发。
    Cancelled,
}

/// 保存对话框（装配方注入：`luna-cloud` 不依赖对话框插件，同 [`crate::StrictLocalProbe`] 的分工）。
/// 入参是建议的文件名；返回用户选的路径，取消为 `None`。
pub type ExportDestinationPicker =
    Arc<dyn Fn(String) -> Pin<Box<dyn Future<Output = Option<PathBuf>> + Send>> + Send + Sync>;

/// 导出文件的默认名（`lunarca-account-export-<YYYY-MM-DD>.json`，日期取本机当天）。
#[must_use]
pub fn export_file_name(today: chrono::NaiveDate) -> String {
    format!("lunarca-account-export-{}.json", today.format("%Y-%m-%d"))
}

/// 导出：先判会话（零网络）→ 弹保存对话框 → 取消即止（一个字节都不发）→ 流式写入选中的位置。
///
/// 会话判在对话框**之前**：没登录还让用户先选一个位置，选完才说「没登录」，是让他白做一次事。
///
/// # Errors
/// 未登录 / 严格本地在对话框之前返回；其余同 [`CloudClient::export_account_to`]。
pub async fn export_with_picker(
    client: &CloudClient,
    picker: &ExportDestinationPicker,
    today: chrono::NaiveDate,
) -> Result<AccountExportDto, CloudError> {
    client.require_session()?;
    let Some(target) = picker(export_file_name(today)).await else {
        return Ok(AccountExportDto::Cancelled);
    };
    client.export_account_to(&target).await?;
    Ok(AccountExportDto::Saved {
        path: target.to_string_lossy().into_owned(),
    })
}

/// 写盘失败 → 只带 I/O 错误种类（`io::Error` 的种类串不含路径；路径里有用户名，不进诊断）。
fn write_failed(err: &std::io::Error) -> CloudError {
    CloudError::ExportWriteFailed {
        detail: err.kind().to_string(),
    }
}

/// 把一条响应体流式写进 `target`。
///
/// 临时文件建在 `target` **同一个目录**里（改名才是原子的，跨卷改名会退化成复制）；它在出错或被
/// 丢弃时自动删除（`tempfile::TempPath` 的析构）。只有收完、刷盘之后才改名成 `target`——原先在
/// 那里的同名文件要么被一份完整的导出替换，要么原样不动。
///
/// # Errors
/// 读响应体失败（连接中断）→ [`CloudError::Network`]；写盘失败 →
/// [`CloudError::ExportWriteFailed`]。
pub(crate) async fn stream_to_file(
    mut response: reqwest::Response,
    target: &Path,
    base: &str,
) -> Result<(), CloudError> {
    let directory = match target.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let (file, partial) = tempfile::Builder::new()
        .prefix(".lunarca-account-export-")
        .suffix(".partial")
        .tempfile_in(directory)
        .map_err(|err| write_failed(&err))?
        .into_parts();
    let mut file = tokio::fs::File::from_std(file);
    while let Some(chunk) = response
        .chunk()
        .await
        // 不走 `CloudError::body`：reqwest 把逐块读取的失败一律标成 decode，而这里根本不解析——
        // 读到一半断了就是连接断了，说成「读不懂的响应」会把人指向升级客户端。
        .map_err(|err| CloudError::network(&err, base))?
    {
        file.write_all(&chunk)
            .await
            .map_err(|err| write_failed(&err))?;
    }
    file.flush().await.map_err(|err| write_failed(&err))?;
    file.sync_all().await.map_err(|err| write_failed(&err))?;
    drop(file);
    partial
        .persist(target)
        .map_err(|err| write_failed(&err.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_converts_micro_credits_and_keeps_the_subscription() {
        let wire: DeletionPreviewWire = serde_json::from_str(
            r#"{"deletion":{"state":"allowed"},"subscription":{"state":"active","tier":"pro"},
                "prepaid_micro_credits":12500000,"billing_retention_years":7}"#,
        )
        .unwrap();
        assert_eq!(
            wire.into_dto(),
            AccountDeletionPreviewDto {
                deletion: AccountDeletionDto::Allowed,
                subscription: DeletionSubscriptionDto::Active {
                    tier: "pro".to_owned()
                },
                prepaid_credits: 12.5,
                billing_retention_years: 7,
            }
        );
        let none: DeletionPreviewWire = serde_json::from_str(
            r#"{"deletion":{"state":"allowed"},"subscription":{"state":"none"},"prepaid_micro_credits":0,"billing_retention_years":10}"#,
        )
        .unwrap();
        let dto = none.into_dto();
        assert_eq!(dto.subscription, DeletionSubscriptionDto::None);
        assert_eq!(dto.prepaid_credits, 0.0);
    }

    /// 不能删的那一态（协议 v17）：成因原样带给界面，界面据此不给确认。
    #[test]
    fn preview_carries_a_blocked_deletion_and_its_reason() {
        let wire: DeletionPreviewWire = serde_json::from_str(
            r#"{"deletion":{"state":"blocked","reason":"last_org_owner"},
                "subscription":{"state":"none"},"prepaid_micro_credits":0,"billing_retention_years":7}"#,
        )
        .unwrap();
        assert_eq!(
            wire.into_dto().deletion,
            AccountDeletionDto::Blocked {
                reason: DeletionBlockedReasonDto::LastOrgOwner
            }
        );
    }

    /// 必填：缺任何一格都读不懂（协议 v14 / v17 不留可选字段过渡，ADR-046）。`deletion` 缺席、
    /// 拦下却没说成因、成因或状态认不出，都是读不懂——不猜成「可以删」。
    #[test]
    fn preview_fields_are_required() {
        let rest = r#""subscription":{"state":"none"},"prepaid_micro_credits":0,"billing_retention_years":7"#;
        for body in [
            format!("{{{rest}}}"),
            format!(r#"{{"deletion":{{"state":"blocked"}},{rest}}}"#),
            format!(r#"{{"deletion":{{"state":"blocked","reason":"unpaid_invoice"}},{rest}}}"#),
            format!(r#"{{"deletion":{{"state":"pending"}},{rest}}}"#),
            r#"{"deletion":{"state":"allowed"},"prepaid_micro_credits":0,"billing_retention_years":7}"#.to_owned(),
            r#"{"deletion":{"state":"allowed"},"subscription":{"state":"none"},"billing_retention_years":7}"#.to_owned(),
            r#"{"deletion":{"state":"allowed"},"subscription":{"state":"none"},"prepaid_micro_credits":0}"#.to_owned(),
            r#"{"deletion":{"state":"allowed"},"subscription":{"state":"active"},"prepaid_micro_credits":0,"billing_retention_years":7}"#.to_owned(),
        ] {
            assert!(
                serde_json::from_str::<DeletionPreviewWire>(&body).is_err(),
                "{body}"
            );
        }
    }

    #[test]
    fn delete_receipt_carries_only_the_retention() {
        let wire: DeleteWire =
            serde_json::from_str(r#"{"deleted":true,"retained":{"billing_retention_years":7}}"#)
                .unwrap();
        assert_eq!(
            wire.into_dto("h").unwrap(),
            AccountDeleteDto {
                billing_retention_years: 7
            }
        );
        // 旧形状（一句服务端写死的中文）读不懂：硬切，不留兼容分支。
        assert!(serde_json::from_str::<DeleteWire>(r#"{"retained":"审计事件"}"#).is_err());
    }

    #[test]
    fn a_receipt_that_does_not_confirm_deletion_is_unreadable() {
        let wire: DeleteWire =
            serde_json::from_str(r#"{"deleted":false,"retained":{"billing_retention_years":7}}"#)
                .unwrap();
        assert!(matches!(
            wire.into_dto("h"),
            Err(CloudError::UnreadableResponse { .. })
        ));
    }

    #[test]
    fn only_503_billing_unavailable_is_its_own_error() {
        let mapped = map_delete_error(CloudError::Server {
            status: 503,
            error_code: "billing_unavailable".to_owned(),
        });
        assert!(matches!(mapped, CloudError::BillingUnavailable));
        let other = map_delete_error(CloudError::Server {
            status: 503,
            error_code: "unknown".to_owned(),
        });
        assert!(matches!(other, CloudError::Server { status: 503, .. }));
        let wrong_status = map_delete_error(CloudError::Server {
            status: 500,
            error_code: "billing_unavailable".to_owned(),
        });
        assert!(matches!(
            wrong_status,
            CloudError::Server { status: 500, .. }
        ));
    }

    /// 409 `last_org_owner`（协议 v17）：什么都没删，有自己的码；别的 409、别的状态码带同一个码都不改写。
    #[test]
    fn only_409_last_org_owner_is_its_own_error() {
        let mapped = map_delete_error(CloudError::Server {
            status: 409,
            error_code: "last_org_owner".to_owned(),
        });
        assert!(matches!(mapped, CloudError::LastOrgOwner));
        let other = map_delete_error(CloudError::Server {
            status: 409,
            error_code: "conflict".to_owned(),
        });
        assert!(matches!(other, CloudError::Server { status: 409, .. }));
        let wrong_status = map_delete_error(CloudError::Server {
            status: 400,
            error_code: "last_org_owner".to_owned(),
        });
        assert!(matches!(
            wrong_status,
            CloudError::Server { status: 400, .. }
        ));
    }

    #[test]
    fn default_file_name_carries_the_date() {
        let day = chrono::NaiveDate::from_ymd_opt(2026, 10, 4).unwrap();
        assert_eq!(
            export_file_name(day),
            "lunarca-account-export-2026-10-04.json"
        );
    }

    /// 写盘失败的诊断不含路径（路径里有用户名）。
    #[test]
    fn write_failure_detail_has_no_path() {
        let err = std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "/Users/someone/secret",
        );
        let CloudError::ExportWriteFailed { detail } = write_failed(&err) else {
            panic!("应为写盘失败");
        };
        assert!(!detail.contains("someone"), "{detail}");
    }
}
