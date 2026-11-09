//! 应用自身的更新（PRD-09-051，TDD-13 §5；客户端分发 D2）。
//!
//! 安装包从官网下载、没有 Apple 公证，所以更新通道就是 `tauri-plugin-updater`：清单与更新包放在
//! `lunarca.app/releases/`，更新包用发布者自己的 minisign 私钥签名，客户端用编进 `tauri.conf.json`
//! 的公钥验签，验不过一律不装。应用自己下载的更新不带隔离属性，只有第一次安装需要在系统设置里放行。
//!
//! 联网纪律（TDD-12 §1）：检查更新是 opt-in 的下行联网。`update.policy` 缺省「仅手动」——只在用户点
//! 「检查更新」时联网；首次启动发一条**本地**通知问一次要不要启动时检查（提问本身不联网），答应了
//! 才改成「启动时检查、由你确认」。严格本地开着时一律不联网、也不问（与「应用」页的检查更新同一道闸，
//! [`super::apps::refuse_if_strict_local`]）。
//!
//! 与「应用」页的 `app_update`（Lunarca 下载的组件）是两回事：这里更新的是 Lunarca 本身。

use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::Manager as _;
use tauri_plugin_updater::{Update, UpdaterExt as _};

use tauri_specta::Event as _;

use super::restart_guard::{self, CODE_BLOCKED_CAPTURE};
use super::settings::{set_setting_value, SettingValueDto};
use super::{AppState, SettingsChanged};
use crate::notifications::{NotificationCenter, NotificationEvent, NotificationTarget};

/// 启动后多久做那一次检查：让开冷启动最忙的那几秒，也不和首屏抢网络。
const STARTUP_CHECK_DELAY: Duration = Duration::from_secs(20);

/// 一个可装的新版本（发给界面）。发布说明是发布者写的纯文本，界面按文本显示。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AppReleaseDto {
    pub version: String,
    pub current_version: String,
    pub notes: Option<String>,
    /// 发布日期（RFC 3339）；清单没写就是 `None`。
    pub date: Option<String>,
}

/// `update.startup_check_prompt` 的两个非空取值（空 = 还没问）。
const PROMPT_NOTIFIED: &str = "notified";
const PROMPT_ANSWERED: &str = "answered";

/// 查到但还没装的那一版：「下载并安装」装的就是它，不再查第二次（两次查的结果可能不同）。
#[derive(Default)]
pub(crate) struct PendingRelease(Mutex<Option<Update>>);

/// 更新策略（`update.policy`）。未知取值按默认处理（设置加载器已经拦过非法值，这里只是兜住）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UpdatePolicy {
    CheckAndConfirm,
    Automatic,
    Manual,
}

impl UpdatePolicy {
    pub(crate) fn parse(value: &str) -> Self {
        match value {
            "automatic" => Self::Automatic,
            "manual" => Self::Manual,
            _ => Self::CheckAndConfirm,
        }
    }
}

fn dto(update: &Update) -> AppReleaseDto {
    AppReleaseDto {
        version: update.version.clone(),
        current_version: update.current_version.clone(),
        notes: update.body.clone(),
        date: update.date.map(|date| date.to_string()),
    }
}

async fn fetch(app: &tauri::AppHandle) -> Result<Option<Update>, String> {
    let updater = app.updater().map_err(|_| "update_unavailable".to_owned())?;
    updater
        .check()
        .await
        .map_err(|_| "update_check_failed".to_owned())
}

/// **检查更新**：联网问一次有没有新版本；有就记下来等用户点「下载并安装」。
///
/// # Errors
/// 严格本地拦截（`strict_local_blocked`）、更新通道不可用（`update_unavailable`）、联网或清单
/// 不符预期（`update_check_failed`）。
#[tauri::command]
#[specta::specta]
pub(crate) async fn app_release_check(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<Option<AppReleaseDto>, String> {
    super::apps::refuse_if_strict_local(&app, &state)?;
    let update = fetch(&app).await?;
    let view = update.as_ref().map(dto);
    if let Some(pending) = app.try_state::<PendingRelease>() {
        *pending
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = update;
    }
    Ok(view)
}

const CODE_INSTALL_FAILED: &str = "update_install_failed";

/// 下载 → 重启闸门（录音判据、全部编辑窗口保存）→ 安装 → 重启。只有失败才返回，返回值是稳定错误码。
///
/// 下载放在冻结输入之前：它可能要好几分钟，用户这段时间照常编辑，只有最后保存与安装的几秒才冻结。
async fn install_and_restart(app: &tauri::AppHandle, state: &AppState, update: &Update) -> String {
    if restart_guard::capture_blocks_restart(state) {
        return CODE_BLOCKED_CAPTURE.to_owned();
    }
    let Ok(bytes) = update.download(|_, _| {}, || {}).await else {
        return CODE_INSTALL_FAILED.to_owned();
    };
    let clearance = match restart_guard::clear_for_restart(app, state).await {
        Ok(clearance) => clearance,
        Err(code) => return code.to_owned(),
    };
    if update.install(bytes).is_err() {
        clearance.abandon(app);
        return CODE_INSTALL_FAILED.to_owned();
    }
    // 从异步线程调用：走 `ExitRequested` / `Exit`，随包推理引擎照常收尾。
    app.restart()
}

/// **下载并安装**刚查到的那一版，全部编辑窗口保存成功后才安装并重启。验签不过、下载失败都不装；
/// 保存不成功就留在当前应用，这一版继续留着，处理完可以再点一次。
///
/// # Errors
/// 严格本地拦截、没有待装的版本（`no_pending_update`，先检查更新）、重启闸门拦下
/// （`restart_*`，见 `restart_guard`）、下载或验签失败（`update_install_failed`）。
#[tauri::command]
#[specta::specta]
pub(crate) async fn app_release_install(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    super::apps::refuse_if_strict_local(&app, &state)?;
    // 取走即占位：并发的第二次点击拿到 `no_pending_update`，不会装两遍。
    let pending = app.try_state::<PendingRelease>();
    let update = pending
        .as_ref()
        .and_then(|pending| {
            pending
                .0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take()
        })
        .ok_or_else(|| "no_pending_update".to_owned())?;
    let code = install_and_restart(&app, &state, &update).await;
    if let Some(pending) = pending {
        *pending.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(update);
    }
    Err(code)
}

/// 后台装不装。Windows 的安装器会直接结束当前进程，等于不经保存协调的重启，所以「自动」在那里也
/// 只提醒，由用户点「下载并安装」；其它平台装完不重启，下次启动才生效。
fn installs_unattended(policy: UpdatePolicy, install_exits_process: bool) -> bool {
    policy == UpdatePolicy::Automatic && !install_exits_process
}

/// 「关于与更新」要不要显示提问卡「启动时自动检查更新？」：还没答过、策略仍是缺省的仅手动、
/// 严格本地没开。用户先在下拉里自己选了别的策略，就等于答过了，不再问。
fn should_ask(policy: UpdatePolicy, prompt: &str, strict_local: bool) -> bool {
    !strict_local && policy == UpdatePolicy::Manual && prompt != PROMPT_ANSWERED
}

fn write_setting(
    app: &tauri::AppHandle,
    state: &AppState,
    key: &str,
    value: &str,
) -> Result<(), String> {
    set_setting_value(
        state.copy(),
        &state.settings,
        &state.settings_persistence,
        key,
        SettingValueDto::Str(value.to_owned()),
    )?;
    let _ = SettingsChanged {
        key: key.to_owned(),
    }
    .emit(app);
    Ok(())
}

/// 提问卡显示与否（判定在 Rust，ADR-003）。
#[tauri::command]
#[specta::specta]
pub(crate) fn app_release_startup_check_prompt(state: tauri::State<'_, AppState>) -> bool {
    let settings = state.settings_snapshot();
    should_ask(
        UpdatePolicy::parse(&settings.update_policy()),
        &settings.update_startup_check_prompt(),
        state.strict_local(),
    )
}

/// 提问卡的两个出口：`enable = true` → 「启动时检查、由你确认」；`false` → 保持仅手动。两种都记作答过。
///
/// # Errors
/// 设置落盘失败。
#[tauri::command]
#[specta::specta]
pub(crate) fn app_release_answer_startup_check(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    enable: bool,
) -> Result<(), String> {
    if enable {
        write_setting(&app, &state, "update.policy", "check_and_confirm")?;
    }
    write_setting(&app, &state, "update.startup_check_prompt", PROMPT_ANSWERED)
}

/// 启动后的那一步（严格本地开着时什么都不做）：
/// - 仅手动（缺省）且从没问过：发一条本地通知问要不要启动时检查，**不联网**；之后不再每次启动都发；
/// - 启动时检查、由用户确认：查到新版本发一条通知，带到通用设置里看说明、点安装；
/// - 自动：查到就下载安装，装好发一条通知，下次启动即是新版本（Windows 例外，见 [`installs_unattended`]）。
///
/// 只查一次、失败不重试不提示——这是用户答应过的顺手一查，随时可以手动检查。
pub(crate) fn spawn_startup_check(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(STARTUP_CHECK_DELAY).await;
        let Some(state) = app.try_state::<AppState>() else {
            return;
        };
        if state.strict_local() {
            return;
        }
        let settings = state.settings_snapshot();
        let policy = UpdatePolicy::parse(&settings.update_policy());
        if policy == UpdatePolicy::Manual {
            if settings.update_startup_check_prompt().is_empty() {
                publish(&app, "app_release_ask");
                let _ = write_setting(&app, &state, "update.startup_check_prompt", PROMPT_NOTIFIED);
            }
            return;
        }
        let Ok(Some(update)) = fetch(&app).await else {
            return;
        };
        let notice = if installs_unattended(policy, cfg!(windows)) {
            if update.download_and_install(|_, _| {}, || {}).await.is_err() {
                return;
            }
            "app_release_installed"
        } else {
            if let Some(pending) = app.try_state::<PendingRelease>() {
                *pending
                    .0
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(update);
            }
            "app_release_available"
        };
        publish(&app, notice);
    });
}

fn publish(app: &tauri::AppHandle, notice: &'static str) {
    if let Some(center) = app.try_state::<NotificationCenter>() {
        center.publish(
            NotificationEvent::info_at(notice, NotificationTarget::Settings("general".into())),
            app,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_parses_every_registered_option_and_defaults_to_confirm() {
        for option in luna_config::registry::UPDATE_POLICIES {
            let parsed = UpdatePolicy::parse(option);
            let expected = match *option {
                "automatic" => UpdatePolicy::Automatic,
                "manual" => UpdatePolicy::Manual,
                _ => UpdatePolicy::CheckAndConfirm,
            };
            assert_eq!(parsed, expected, "{option}");
        }
        assert_eq!(
            UpdatePolicy::parse("garbage"),
            UpdatePolicy::CheckAndConfirm
        );
    }

    /// 缺省不联网：没答过的仅手动才问；答过、自己选了别的策略、严格本地开着都不问。
    #[test]
    fn asks_only_while_unanswered_manual_and_not_strict_local() {
        assert!(should_ask(UpdatePolicy::Manual, "", false));
        assert!(should_ask(UpdatePolicy::Manual, PROMPT_NOTIFIED, false));
        assert!(!should_ask(UpdatePolicy::Manual, PROMPT_ANSWERED, false));
        assert!(!should_ask(UpdatePolicy::CheckAndConfirm, "", false));
        assert!(!should_ask(UpdatePolicy::Manual, "", true));
        assert_eq!(
            UpdatePolicy::parse(&luna_config::Settings::default().update_policy()),
            UpdatePolicy::Manual,
            "检查更新是 opt-in 联网：缺省必须是仅手动"
        );
    }

    #[test]
    fn automatic_never_installs_unattended_when_the_installer_ends_the_process() {
        assert!(installs_unattended(UpdatePolicy::Automatic, false));
        assert!(!installs_unattended(UpdatePolicy::Automatic, true));
        assert!(!installs_unattended(UpdatePolicy::CheckAndConfirm, false));
    }

    /// 权限弹窗、设置页与隐私说明讲的要是同一件事（v1.1.0 发布判据）：默认本机，开启服务端转写才外发。
    #[test]
    fn capture_usage_strings_cover_local_default_and_chosen_server() {
        let plist = include_str!("../../Info.plist");
        let settings = include_str!("../../../apps/desktop/src/i18n/locales/zh-CN/settings.json");
        let setting_name = "服务端转写";
        assert!(settings.contains(&format!("\"remote_asr_enabled\": \"{setting_name}\"")));
        for key in [
            "NSMicrophoneUsageDescription",
            "NSAudioCaptureUsageDescription",
        ] {
            let value = plist
                .split(&format!("<key>{key}</key>"))
                .nth(1)
                .and_then(|rest| rest.split("</string>").next())
                .expect(key);
            assert!(value.contains("默认在本机转写"), "{key}");
            assert!(value.contains(setting_name), "{key}");
            assert!(value.contains("发送到你选择的服务端"), "{key}");
            for stale in ["全程本地", "不上传"] {
                assert!(!value.contains(stale), "{key} 不能写 {stale}");
            }
        }
    }

    #[test]
    fn updater_endpoint_is_the_official_release_manifest() {
        let conf = include_str!("../../tauri.conf.json");
        assert!(
            conf.contains("https://lunarca.app/releases/latest.json"),
            "更新清单地址必须是官网发布目录（客户端分发 D4）"
        );
    }
}
