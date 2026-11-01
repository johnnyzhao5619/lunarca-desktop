//! 权限编排（TDD-05 §4）：统一 `check/request` + 降级路由 + 运行期撤销检测。
//!
//! **纯逻辑优先**：权限**种类/状态/降级决策/撤销判定**零平台依赖、headless 可单测；具体的系统
//! 查询（macOS AX/AVFoundation/UN/ScreenCapture）在 [`crate::platform::macos::permissions`]。
//!
//! **降级铁律（TDD-05 §4）**：权限缺失绝不崩溃——辅助功能未授权 → 双击不可用，降级到备用热键 +
//! 剪贴板路径；注入不可用 → 放入剪贴板让用户手动粘贴；通知被拒 → 降级应用内提示（不阻断功能）。
//!
//! **运行期撤销 → 系统健康事件**：用户在系统设置撤销已授予权限时，本模块产出结构化
//! [`SystemHealthEvent`]（纯逻辑、可单测）；撤销 → NotificationCenter 的实际路由（banner/系统横幅）
//! 由 `start_permission_watch`（`lib.rs`）在检测到 [`detect_revocation`] 命中时直接投递（§6.1 已落地）。

use serde::{Deserialize, Serialize};
use specta::Type;

/// 权限种类（TDD-05 §4 表）。辅助功能、麦克风、屏幕录制已接真实查询；通知/输入监听随其功能（Phase 6/8）接入。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum PermissionKind {
    /// 辅助功能（双击复制监听 + 文本注入），CGEventTap/AX 必需。
    Accessibility,
    /// 输入监控（部分注入/监听场景）。
    InputMonitoring,
    /// 麦克风（语音输入/录音，Phase 4）。
    Microphone,
    /// 通知（后台长任务提醒，Phase 6；拒绝不阻断、降级应用内提示）。
    Notification,
    /// 屏幕录制（截屏翻译 PRD-01-007；系统音频 loopback 为 P1 后续）。
    ScreenRecording,
}

/// 权限状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum PermissionStatus {
    /// 已授予。
    Granted,
    /// 已拒绝/被撤销。
    Denied,
    /// 尚未决定（首次未询问）。
    NotDetermined,
    /// 本平台/本阶段未接入查询（诚实占位，非伪 Granted）。
    Unsupported,
}

/// 权限缺失时的降级动作（前端据此提示 + 引导）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum PermissionDegradation {
    /// 无需降级（已授予 / 不影响功能）。
    None,
    /// 双击复制不可用 → 用备用热键 `Cmd+Shift+T` + 剪贴板（辅助功能未授权）。
    DoubleCopyToHotkey,
    /// 文本注入不可用 → 放入剪贴板，用户手动粘贴。
    InjectionToClipboard,
    /// 相关功能整体不可用（如麦克风被拒 → 语音/录音不可用）。
    FeatureUnavailable,
}

/// 权限报告（命令返回；前端展示状态 + 中文引导 + 降级提示）。
#[derive(Debug, Clone, Serialize, Type)]
pub struct PermissionReport {
    /// 权限种类。
    pub kind: PermissionKind,
    /// 当前状态。
    pub status: PermissionStatus,
    /// 缺失时的降级动作。
    pub degradation: PermissionDegradation,
    /// 引导文案 i18n key（Phase 6 接 i18n 用，单一来源）。
    pub guidance_key: String,
    /// 引导文案中文占位（i18n 体系落地前直接展示；与 popup-i18n 同纪律，TDD-15）。
    pub guidance_zh: String,
}

/// **降级决策（纯逻辑，可单测，TDD-05 §4）**：给定种类 + 状态 → 降级动作。
///
/// 已授予 → `None`；通知被拒**不阻断**（降级应用内提示，故 `None`）；辅助功能/输入监控缺失 →
/// 双击降级备用热键；麦克风/屏幕录制缺失 → 相关功能不可用。
#[must_use]
pub fn degradation_for(kind: PermissionKind, status: PermissionStatus) -> PermissionDegradation {
    if status == PermissionStatus::Granted {
        return PermissionDegradation::None;
    }
    match kind {
        PermissionKind::Accessibility | PermissionKind::InputMonitoring => {
            PermissionDegradation::DoubleCopyToHotkey
        }
        PermissionKind::Microphone | PermissionKind::ScreenRecording => {
            PermissionDegradation::FeatureUnavailable
        }
        // 通知被拒不阻断功能（降级应用内提示体系，PRD-15-011），故无功能降级。
        PermissionKind::Notification => PermissionDegradation::None,
    }
}

/// 引导文案 i18n key（纯映射，单一来源；Phase 6 i18n 消费）。
#[must_use]
pub fn guidance_key(kind: PermissionKind, status: PermissionStatus) -> &'static str {
    match (kind, status) {
        (_, PermissionStatus::Granted) => "permissions.granted",
        (PermissionKind::Accessibility, _) => "permissions.accessibility.denied",
        (PermissionKind::InputMonitoring, _) => "permissions.input_monitoring.denied",
        (PermissionKind::Microphone, _) => "permissions.microphone.denied",
        (PermissionKind::Notification, _) => "permissions.notification.denied",
        (PermissionKind::ScreenRecording, _) => "permissions.screen_recording.denied",
    }
}

/// 引导文案中文占位（i18n 落地前直接展示；用人话讲清"为什么需要"，PRD-00 §7）。
#[must_use]
pub fn guidance_zh(kind: PermissionKind, status: PermissionStatus) -> &'static str {
    if status == PermissionStatus::Granted {
        return "已授权。";
    }
    match kind {
        PermissionKind::Accessibility => {
            "弹窗翻译的「双击复制」与「替换原文」需要辅助功能权限来观测快捷键、把译文写回光标处。\
             未授权时可改用备用热键 Cmd+Shift+T，并用「复制译文」手动粘贴。\
             前往「系统设置 › 隐私与安全性 › 辅助功能」勾选 Lunarca。"
        }
        PermissionKind::InputMonitoring => {
            "部分注入/监听场景需要输入监控权限。未授权时降级为备用热键 + 剪贴板。"
        }
        PermissionKind::Microphone => {
            "语音输入与录音需要麦克风权限。未授权时相关功能不可用，可前往「系统设置 › 隐私与安全性 › 麦克风」开启。"
        }
        PermissionKind::Notification => {
            "通知用于你不在应用前台时提醒长任务完成；在「系统设置 › 通知」里可随时开关。关掉不影响核心功能——会降级为应用内提示。"
        }
        PermissionKind::ScreenRecording => {
            "截屏翻译（框选屏幕文字识别）与捕获系统音频（会议对端声音）需要屏幕录制权限。\
             未授权时截屏翻译不可用、录音降级为仅麦克风。\
             前往「系统设置 › 隐私与安全性 › 屏幕录制」勾选 Lunarca 后重启应用。"
        }
    }
}

/// 组装一份完整权限报告（供命令返回）。
#[must_use]
pub fn report(kind: PermissionKind, status: PermissionStatus) -> PermissionReport {
    PermissionReport {
        kind,
        status,
        degradation: degradation_for(kind, status),
        guidance_key: guidance_key(kind, status).to_owned(),
        guidance_zh: guidance_zh(kind, status).to_owned(),
    }
}

/// 系统健康事件（NotificationCenter 接缝，§6.1）。**脱敏**：只带种类 + 状态，绝不含业务正文。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct SystemHealthEvent {
    /// 事件类（如 `"permission_revoked"`）。
    pub class: &'static str,
    /// 相关权限种类。
    pub kind: PermissionKind,
    /// 变化后的状态。
    pub status: PermissionStatus,
}

/// **运行期撤销检测（纯逻辑，可单测，TDD-05 §4）**：上次已授予、本次变为拒绝 → 产出"权限被撤销"
/// 健康事件（提前主动告知 + 降级，而非到下次使用才崩溃式失败，PRD-15-020）。其它跃迁不上报。
#[must_use]
pub fn detect_revocation(
    previous: PermissionStatus,
    current: PermissionStatus,
    kind: PermissionKind,
) -> Option<SystemHealthEvent> {
    if previous == PermissionStatus::Granted && current == PermissionStatus::Denied {
        Some(SystemHealthEvent {
            class: "permission_revoked",
            kind,
            status: current,
        })
    } else {
        None
    }
}

/// 跨平台权限管理（TDD-05 §4）。macOS = [`crate::platform::macos::permissions`]；Windows 后续。
pub trait PermissionManager: Send + Sync {
    /// 查询某权限当前状态（不弹框、廉价，可轮询）。
    fn check(&self, kind: PermissionKind) -> PermissionStatus;

    /// 请求授权：尽力而为——能弹系统授权框则弹，否则打开对应系统设置面板引导用户手动开启。
    fn request(&self, kind: PermissionKind);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn granted_never_degrades() {
        for kind in [
            PermissionKind::Accessibility,
            PermissionKind::Microphone,
            PermissionKind::Notification,
            PermissionKind::ScreenRecording,
            PermissionKind::InputMonitoring,
        ] {
            assert_eq!(
                degradation_for(kind, PermissionStatus::Granted),
                PermissionDegradation::None
            );
        }
    }

    #[test]
    fn accessibility_denied_falls_back_to_hotkey() {
        assert_eq!(
            degradation_for(PermissionKind::Accessibility, PermissionStatus::Denied),
            PermissionDegradation::DoubleCopyToHotkey
        );
        assert_eq!(
            degradation_for(
                PermissionKind::Accessibility,
                PermissionStatus::NotDetermined
            ),
            PermissionDegradation::DoubleCopyToHotkey
        );
    }

    #[test]
    fn notification_denied_does_not_block() {
        // 通知被拒不阻断功能（降级应用内提示）。
        assert_eq!(
            degradation_for(PermissionKind::Notification, PermissionStatus::Denied),
            PermissionDegradation::None
        );
    }

    #[test]
    fn microphone_denied_disables_feature() {
        assert_eq!(
            degradation_for(PermissionKind::Microphone, PermissionStatus::Denied),
            PermissionDegradation::FeatureUnavailable
        );
    }

    #[test]
    fn revocation_only_on_granted_to_denied() {
        // 撤销：已授予 → 拒绝。
        let ev = detect_revocation(
            PermissionStatus::Granted,
            PermissionStatus::Denied,
            PermissionKind::Accessibility,
        );
        assert_eq!(
            ev,
            Some(SystemHealthEvent {
                class: "permission_revoked",
                kind: PermissionKind::Accessibility,
                status: PermissionStatus::Denied,
            })
        );
        // 其它跃迁不上报。
        assert!(detect_revocation(
            PermissionStatus::Denied,
            PermissionStatus::Granted,
            PermissionKind::Accessibility
        )
        .is_none());
        assert!(detect_revocation(
            PermissionStatus::NotDetermined,
            PermissionStatus::Denied,
            PermissionKind::Accessibility
        )
        .is_none());
        assert!(detect_revocation(
            PermissionStatus::Granted,
            PermissionStatus::Granted,
            PermissionKind::Accessibility
        )
        .is_none());
    }

    #[test]
    fn report_carries_degradation_and_guidance() {
        let r = report(PermissionKind::Accessibility, PermissionStatus::Denied);
        assert_eq!(r.degradation, PermissionDegradation::DoubleCopyToHotkey);
        assert_eq!(r.guidance_key, "permissions.accessibility.denied");
        assert!(!r.guidance_zh.is_empty());
    }
}
