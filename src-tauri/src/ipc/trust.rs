//! 信任锚落地（IPC 层）—— 导入组织配置文件、轮换落地。
//!
//! 协议裁决是 ADR-060，落地裁决是 ADR-071；
//! 纯逻辑（文件解析、钉住状态、比对、钉住即生效）在 `luna_cloud`，理由都在那边，本模块不复述。
//! 用户确认一次变更是 `luna_cloud` 的命令（`trust_confirm_change`）——它只改钉住与凭据。
//!
//! # 为什么这两条在装配层
//!
//! 导入要开系统文件对话框；两条都要在落地之后**一次换齐全部消费者**
//! （[`crate::cloud::reconfigure_cloud_stack`]）并发通知——那些只有这一层够得着。
//!
//! # 刻意不做「重置信任锚」按钮（修 7）
//!
//! 变更确认流已经是唯一的恢复路径：服务端换了钥 → 阻断 → 用户核对 → 确认 → 重钉。
//! 再加一个 unpin 入口不会多恢复任何一种情形，只会多一个**社工降级面**——「点一下这里
//! 重置就好了」是一句攻击者也说得出口的话。SSH 删 `known_hosts` 靠 CLI，不给 GUI 一键，
//! 理由完全相同。

use super::*;

use luna_cloud::trust::{
    OfferedAnchor, OrgConfigRejection, TrustProvenance, TrustStatusDto, TrustVerdict,
};

/// 导入的结果。
///
/// **两个分支，因为它们要求用户做的事完全不同**：一个已经完成，另一个还需要他
/// 亲眼核对两组指纹之后再点一次。合并成「成功 + 一个 `needs_confirmation` 布尔」
/// 的话，忘判那个布尔的写法看起来完全正常，而它的表现是**静默覆盖了一次信任锚**。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum TrustImportDto {
    /// 已导入并钉住（新桶，或与既有钉住逐字一致）；未登录时这台服务器已被预选，登录表单
    /// 直接用它。
    Pinned {
        server_url: String,
        status: TrustStatusDto,
        tls_root: TlsRootDto,
    },
    /// 与该桶既有钉住冲突 —— **什么都没写**，等用户走变更确认（修 3）。
    NeedsConfirmation {
        server_url: String,
        status: TrustStatusDto,
    },
}

/// 配置文件里的根证书怎么处理（ADR-153）。
///
/// **只对这台服务端信任它**：钉进 `luna_cloud::tls_root`，发往这台服务端的请求只认这张根；
/// 系统信任库不动，别的主机不受影响。文件本身就是带外交付物（ADR-060 ①），不需要再核指纹。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TlsRootDto {
    /// 文件带了根证书，已为这台服务端钉住。
    Pinned,
    /// 文件里**如实没有**这一项（导出方未配 `TRUST_EXPORT_TLS_ROOT_PATH`）——服务端多半用
    /// 公网可信证书；若是自签，界面说「请改用邀请链接」。不造一个空的成功状态。
    Absent,
    /// 带了，但不是一张能解析的证书。如实说，而不是假装没带。
    Invalid,
}

/// 选一份组织配置文件并导入。**永不覆盖**既有钉住：冲突一律走 `trust_confirm_change`。
///
/// # Errors
/// 文件被拒时返回带稳定 code 的错误串（`TRUST_FILE_*`，前端映射 i18n）。
#[tauri::command]
#[specta::specta]
pub(crate) async fn trust_import_org_config(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<TrustImportDto, String> {
    use tauri_plugin_dialog::DialogExt as _;

    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter("Lunarca org config", &["json"])
        .pick_file(move |path| {
            let picked: Result<Option<std::path::PathBuf>, String> = match path {
                Some(p) => p.into_path().map(Some).map_err(|e| e.to_string()),
                None => Ok(None),
            };
            let _ = tx.send(picked);
        });
    let Some(path) = rx
        .await
        .map_err(|_| coded_error(&app, "DIALOG_CANCELLED", "dialog-cancelled"))??
    else {
        return Err(coded_error(&app, "DIALOG_CANCELLED", "dialog-cancelled"));
    };

    let bytes = std::fs::read(&path).map_err(|err| {
        rejection_code(&OrgConfigRejection::Unreadable {
            detail: err.to_string(),
        })
    })?;
    let config = luna_cloud::trust::parse_org_config(&bytes).map_err(|r| rejection_code(&r))?;

    let Some(cloud) = app.try_state::<luna_cloud::commands::CloudState>() else {
        return Err(coded_error(
            &app,
            "TRUST_CLOUD_UNAVAILABLE",
            "trust-cloud-unavailable",
        ));
    };
    let trust = cloud.0.trust();
    // 文件里那台的桶：此刻它还不是当前服务器，问云客户端要只会得到上一台的。
    let bucket = luna_cloud::config::server_bucket(&config.server_url);
    let offered = OfferedAnchor {
        lease_pubkey: config.lease_pubkey.clone(),
        catalog_pubkey: config.catalog_pubkey.clone(),
        deployment_id: config.deployment_id.clone(),
    };

    // 冲突不覆盖：记成一次待确认的变更，走与 TOFU 变更**同一个**确认对话框（修 3）。
    if trust.record_import_conflict(&bucket, offered.clone(), &config.server_url)
        == TrustVerdict::Changed
    {
        return Ok(TrustImportDto::NeedsConfirmation {
            server_url: config.server_url.clone(),
            status: trust.status(&bucket),
        });
    }

    // 根证书先钉：预选这台之后的第一个请求就要用上它。
    let tls_root = pin_tls_root(&cloud, &config.server_url, config.tls_root_pem.as_deref());
    cloud
        .0
        .adopt_anchor(&bucket, &offered, TrustProvenance::Imported)
        .map_err(|err| format!("TRUST_CREDENTIAL_CLEAR_FAILED: {err}"))?;
    // 导入 = 「我要登录这一台」：未登录时预选它，登录表单直接用（一次一个账户，已登录不动）。
    cloud
        .0
        .preselect_server(luna_cloud::account::AccountServer {
            origin: config.server_url.clone(),
            display_name: None,
        });
    crate::cloud::reconfigure_cloud_stack(&app, &state);
    super::account::emit_account_changed(&app);
    Ok(TrustImportDto::Pinned {
        server_url: config.server_url.clone(),
        status: trust.status(&bucket),
        tls_root,
    })
}

/// 一次**验过链**的轮换的落地（ADR-124 ③/⑥）。
///
/// 与用户确认变更走的是同一个出口（`CloudClient::adopt_anchor`：重钉 + 清该桶凭据），再加
/// 两份目录的水位与缓存重置——自动换钥与手动确认换钥在「之后发生什么」上必须一模一样。
/// 唯一的差别是来源：`Rotated` 会出现在「服务器身份」那一行上。
pub(crate) fn apply_rotation(
    app: &tauri::AppHandle,
    offered: &OfferedAnchor,
) -> Result<(), String> {
    let state = app
        .try_state::<AppState>()
        .ok_or_else(|| "TRUST_STATE_UNAVAILABLE".to_owned())?;
    let cloud = app
        .try_state::<luna_cloud::commands::CloudState>()
        .ok_or_else(|| "TRUST_CLOUD_UNAVAILABLE".to_owned())?;
    let bucket = cloud.0.server_bucket();
    cloud
        .0
        .adopt_anchor(&bucket, offered, TrustProvenance::Rotated)
        .map_err(|err| format!("TRUST_CREDENTIAL_CLEAR_FAILED: {err}"))?;
    if let Some(catalogs) = app.try_state::<luna_cloud::commands::Catalogs>() {
        catalogs.reset_for_repin();
    }
    crate::cloud::reconfigure_cloud_stack(app, &state);
    super::account::emit_account_changed(app);
    // 换钥清凭据，用户因此被登出一次——通知是那一刻屏幕上唯一的解释，故与落地**同批**发出。
    if let Some(center) = app.try_state::<crate::notifications::NotificationCenter>() {
        center.publish(
            crate::notifications::NotificationEvent::trust_rotated(),
            app,
        );
    }
    Ok(())
}

/// 配置文件带的根证书 → 为这台服务端钉住（ADR-153）。**绝不碰系统信任库**。
fn pin_tls_root(
    cloud: &luna_cloud::commands::CloudState,
    server_url: &str,
    pem: Option<&str>,
) -> TlsRootDto {
    let Some(pem) = pem else {
        return TlsRootDto::Absent;
    };
    match cloud
        .0
        .tls_roots()
        .pin(server_url, pem, luna_cloud::catalog::now_unix())
    {
        Ok(()) => TlsRootDto::Pinned,
        Err(_) => TlsRootDto::Invalid,
    }
}

/// 文件被拒 → 带稳定 code 的错误串（同 `SETTING_*` 既有约定；前端只认 code 取文案）。
///
/// 每一条都指向一个**用户能做的动作**：选错文件 / 请升级客户端 / 这份文件坏了。
/// 一句笼统的「导入失败」正是 ADR-060 起因里那类指错方向的报错。
fn rejection_code(rejection: &OrgConfigRejection) -> String {
    match rejection {
        OrgConfigRejection::Unreadable { detail } => format!("TRUST_FILE_UNREADABLE: {detail}"),
        OrgConfigRejection::Malformed => "TRUST_FILE_MALFORMED".to_owned(),
        OrgConfigRejection::NotOrgConfig { kind } => format!("TRUST_FILE_NOT_ORG_CONFIG: {kind}"),
        OrgConfigRejection::UnsupportedFormatVersion { found, supported } => {
            format!("TRUST_FILE_UNSUPPORTED_VERSION: {found} (supported {supported})")
        }
        OrgConfigRejection::BadServerUrl => "TRUST_FILE_BAD_SERVER_URL".to_owned(),
        OrgConfigRejection::BadPubkey { field } => format!("TRUST_FILE_BAD_PUBKEY: {field}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **刻意不做「重置信任锚」按钮**（修 7）。
    ///
    /// 变更确认流已是唯一恢复路径；多一个 unpin 入口只多一个社工降级面——「点一下这里
    /// 重置就好了」是一句攻击者也说得出口的话。这条纪律不能只写在注释里：下一个人加一条
    /// `trust_unpin` 命令时，本用例必须当场变红，逼他先来改这段文字。
    #[test]
    fn there_is_no_unpin_command() {
        let source = include_str!("trust.rs");
        // needle 在运行期拼出来，否则本用例自己那份禁词表就会命中——那样它恒红，
        // 而恒红的守护会在第一时间被人删掉（自指扫描的通用坑，同 `trust.rs` 那条 TLS 守护）。
        for name in ["trust_unpin", "trust_reset", "trust_forget"] {
            let forbidden = format!("fn {name}");
            assert!(
                !source.contains(&forbidden),
                "出现了 `{forbidden}` —— 撤销钉住的入口只能是「核对新指纹后确认一次」，\n\
                 一个一键重置按钮把这次核对整个跳过了（ADR-071，修 7）"
            );
        }
    }

    /// 每一条拒绝理由都必须是**可执行**的，且彼此不同码。
    #[test]
    fn every_rejection_names_a_distinct_actionable_code() {
        let codes = [
            rejection_code(&OrgConfigRejection::Malformed),
            rejection_code(&OrgConfigRejection::NotOrgConfig {
                kind: "x".to_owned(),
            }),
            rejection_code(&OrgConfigRejection::UnsupportedFormatVersion {
                found: 2,
                supported: 1,
            }),
            rejection_code(&OrgConfigRejection::BadServerUrl),
            rejection_code(&OrgConfigRejection::BadPubkey {
                field: "lease".to_owned(),
            }),
            rejection_code(&OrgConfigRejection::Unreadable {
                detail: "io".to_owned(),
            }),
        ];
        let mut prefixes: Vec<&str> = codes
            .iter()
            .map(|c| c.split(':').next().unwrap_or_default())
            .collect();
        prefixes.sort_unstable();
        let unique = prefixes.len();
        prefixes.dedup();
        assert_eq!(
            prefixes.len(),
            unique,
            "两条拒绝共用了同一个 code——界面只能对它们说同一句话，而下一步动作不同"
        );
        assert!(prefixes.iter().all(|p| p.starts_with("TRUST_FILE_")));
    }

    /// 本文件里**测试模块之前**的那一段源码（源码守护只扫它，免得断言被自己喂饱）。
    fn production_source() -> &'static str {
        let source = include_str!("trust.rs");
        let cut = source
            .find("\n#[cfg(test)]")
            .expect("本文件必须有测试模块，否则这条守护扫的是全文");
        &source[..cut]
    }

    /// 轮换落地必须**同批**发出那条通知。
    ///
    /// 换钥走三重重置 = 用户被登出一次，而解释这件事的那张连接卡恰恰因为他被登出收成了
    /// 一行（ADR-122 ④）——解释只在重新登录之后才看得到，可需要它的时刻是登出那一刻
    /// （真机 2026-09-11）。通知是那一刻屏幕上唯一还在的东西，掉了它不会有任何东西变红，
    /// 症状只是「用户莫名其妙被登出」，而那正是本条要拦的。
    #[test]
    fn a_landed_rotation_always_explains_itself() {
        assert!(
            production_source().contains("NotificationEvent::trust_rotated()"),
            "轮换落地不再发通知了——用户会在没有任何解释的情况下被登出"
        );
    }
}
