//! 单一通知汇聚点（PRD-15 / TDD-05 §6.1）。
//!
//! 事件源只上报结构化事件；分类、路由、去重、脱敏和 typed event 统一在这里完成。

use serde::{Deserialize, Serialize};
use specta::Type;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri_specta::Event;

use crate::usage::{
    notification_store_for_usage_store, now_ms, HistoryCursor, NotificationHistoryRecord,
    NotificationHistoryStore,
};

static NEXT_NOTIFICATION_ID: AtomicU64 = AtomicU64::new(1);

/// 用户在场状态。MVP-0 先以前台应用内提示为主，保留后台系统横幅路由接缝。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Presence {
    /// App 前台或主窗口可见。
    Foreground,
    /// App 常驻后台。
    Background,
}

/// 通知分类（PRD-15 五类）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotificationClass {
    /// 长任务正常完成。
    Completion,
    /// 失败但无需即时决策。
    Failure,
    /// 高危系统健康事件。
    SystemHealth,
    /// 需要用户决策。
    UserAction,
    /// 可忽略信息。
    Info,
}

impl NotificationClass {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Completion => "completion",
            Self::Failure => "failure",
            Self::SystemHealth => "system_health",
            Self::UserAction => "user_action",
            Self::Info => "info",
        }
    }
}

/// 通知严重度。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotificationSeverity {
    /// 普通信息。
    Info,
    /// 需要注意。
    Warning,
    /// 错误或失败。
    Error,
    /// 高危系统健康。
    Critical,
}

impl NotificationSeverity {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
            Self::Critical => "critical",
        }
    }
}

/// 呈现通道（同一事件可多通道）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotificationChannel {
    /// OS 系统通知。
    System,
    /// 应用内瞬时提示。
    Toast,
    /// 应用内持久 banner。
    Banner,
    /// 就地提示。
    Inline,
    /// 状态栏/角标/历史入口。
    Status,
}

impl NotificationChannel {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Toast => "toast",
            Self::Banner => "banner",
            Self::Inline => "inline",
            Self::Status => "status",
        }
    }
}

/// 深链目标。不得携带笔记文件名或正文；Vault 内容目标后续用脱敏 id。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NotificationTarget {
    /// 设置页。
    Settings(String),
    /// 队列/活动视图。
    Queue,
    /// 工作区根。
    Workspace,
    /// 自定义安全 route。
    Route(String),
}

impl NotificationTarget {
    #[must_use]
    pub fn as_route(&self) -> String {
        match self {
            Self::Settings(section) => format!("settings://{section}"),
            Self::Queue => "queue://current".to_string(),
            Self::Workspace => "workspace://current".to_string(),
            Self::Route(route) => route.clone(),
        }
    }
}

/// 私有输入载荷。只允许进入策略测试/内部判断，不能进入 DTO 或系统通知文本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivatePayload {
    _text: String,
}

impl PrivatePayload {
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self { _text: text.into() }
    }
}

/// 带「诊断」一节的通知种类（2026-10-08 D3，PRD-15）。
///
/// 这几类通知的正文（`.body`）只说**用户影响与能做什么**；版本号、缺项、管理员该做的事
/// 进 `.diagnostic`，在详情里按需展开。声明在这里而不是让前端按正文切句——切句会在另一种
/// 语言里切错，也会把参数切丢。历史记录不存诊断，按种类在投影时派生，故不需要新字段。
const DIAGNOSTIC_KINDS: &[&str] = &[
    "prompt_catalog_rejected",
    "prompt_catalog_degraded",
    "recording_mic_silent_input",
];

/// 一条通知的诊断文案 key（`notifications:<kind>.diagnostic`）；该种类没有诊断时为 `None`。
/// 诊断与正文共用同一份脱敏参数，不得新增正文、剪贴板或密钥类参数。
#[must_use]
pub fn diagnostic_i18n_key(i18n_key: &str) -> Option<String> {
    let kind = i18n_key.strip_prefix("notifications:")?;
    DIAGNOSTIC_KINDS
        .contains(&kind)
        .then(|| format!("{i18n_key}.diagnostic"))
}

/// 内部结构化通知事件。
#[derive(Debug, Clone)]
pub struct NotificationEvent {
    id: String,
    class: NotificationClass,
    severity: NotificationSeverity,
    source_group: String,
    target: Option<NotificationTarget>,
    i18n_key: String,
    params: HashMap<String, String>,
    _private_payload: Option<PrivatePayload>,
}

impl NotificationEvent {
    #[must_use]
    pub fn system_health(
        kind: impl Into<String>,
        target: NotificationTarget,
        private_payload: PrivatePayload,
    ) -> Self {
        let kind = kind.into();
        Self::new(
            NotificationClass::SystemHealth,
            NotificationSeverity::Critical,
            kind,
            Some(target),
        )
        .with_private_payload(private_payload)
    }

    /// 长任务正常完成（PRD-15 Completion 类）：前台走 toast + 历史，不走高危 banner。
    /// 用于「转写已优化完成」等成功事件——**绝非** SystemHealth/Critical（那是核心能力
    /// 失效才用），避免完成事件被渲染成红色严重告警（语义与配色不符）。
    #[must_use]
    pub fn completion(
        kind: impl Into<String>,
        target: NotificationTarget,
        private_payload: PrivatePayload,
    ) -> Self {
        Self::new(
            NotificationClass::Completion,
            NotificationSeverity::Info,
            kind,
            Some(target),
        )
        .with_private_payload(private_payload)
    }

    /// 信息类提示（PRD-15「知道更好、忽略无碍」）：前台瞬时 toast + 历史，绝不打断、
    /// 不着健康色。用于「外放建议佩戴耳机」等采集环境建议（PRD-04-052）。
    #[must_use]
    pub fn info(kind: impl Into<String>) -> Self {
        Self::new(
            NotificationClass::Info,
            NotificationSeverity::Info,
            kind,
            None,
        )
    }

    /// 带落点的信息类提示：toast 上多一枚动作按钮直达 `target`（如「查看队列」）。
    /// 与 [`Self::info`] 同类同档，只是有地方可去。
    #[must_use]
    pub fn info_at(kind: impl Into<String>, target: NotificationTarget) -> Self {
        Self::new(
            NotificationClass::Info,
            NotificationSeverity::Info,
            kind,
            Some(target),
        )
    }

    /// 文案插值参数（脱敏值：计数、模型名之类；绝不放正文 / 文件名，PRD-15-050）。
    #[must_use]
    pub fn with_param(mut self, key: &str, value: impl Into<String>) -> Self {
        self.params.insert(key.to_owned(), value.into());
        self
    }

    /// 会话被动失效（refresh 被拒 / 设备被撤销）：AI 请求会在 401 上失败，用户必须知道
    /// 「为什么坏了、去哪重新登录」。
    ///
    /// 落点是账户页——登录的唯一入口（ADR-155）。
    #[must_use]
    pub fn session_expired() -> Self {
        Self::new(
            NotificationClass::SystemHealth,
            NotificationSeverity::Warning,
            "session_expired",
            Some(NotificationTarget::Settings("account".into())),
        )
    }

    /// 服务端轮换了签名密钥、客户端沿链验过并自动换了钥（ADR-124 ⑥）。
    ///
    /// # 为什么这一条必须存在，而不是「卡上写一句就够了」
    ///
    /// 换钥要走三重重置，用户因此**被登出一次**。需要解释的时刻是登出的那一刻（真机
    /// 2026-09-11：「退出登录，需要重新登录，看不到密钥」）；本条是那一刻唯一还在屏幕上的东西。
    ///
    /// 类与档取 `session_expired` 那一组：对用户而言两者的后果一模一样（会话没了、去这里
    /// 重新登录），分成两种档次只会制造一个他看不见的区别。
    ///
    /// 不带公钥、不带邮箱——通知历史会落盘，而那两样都不该进它（PRD-15-050）。
    ///
    /// 落点同 [`Self::session_expired`]：账户页。
    #[must_use]
    pub fn trust_rotated() -> Self {
        Self::new(
            NotificationClass::SystemHealth,
            NotificationSeverity::Warning,
            "trust_rotated",
            Some(NotificationTarget::Settings("account".into())),
        )
    }

    /// 浏览器里批准完成、客户端已拿到会话：前台 toast 一次，让用户知道可以回来用了。
    /// 不带邮箱等身份字段——通知历史会落盘，账户标识不该进它。
    #[must_use]
    pub fn signed_in() -> Self {
        Self::new(
            NotificationClass::Completion,
            NotificationSeverity::Info,
            "signed_in",
            Some(NotificationTarget::Settings("account".into())),
        )
    }

    #[must_use]
    pub fn config_recovered() -> Self {
        Self::new(
            NotificationClass::SystemHealth,
            NotificationSeverity::Critical,
            "config_recovered",
            Some(NotificationTarget::Settings("general".into())),
        )
    }

    /// 个别设置项取值过期、已回退默认（其余设置照常生效）。
    ///
    /// 与 [`Self::config_recovered`] 严格区分：那个是"整份配置结构坏了"，这个是
    /// "几项取值不认识了"——后者远比前者常见（枚举改名、降级、手改打错），
    /// 严重度只到 Warning，且要如实点名是哪几项，否则用户只会觉得"设置自己变了"。
    #[must_use]
    pub fn config_values_reset(keys: &[&str]) -> Self {
        let mut event = Self::new(
            NotificationClass::SystemHealth,
            NotificationSeverity::Warning,
            "config_values_reset",
            Some(NotificationTarget::Settings("general".into())),
        );
        event
            .params
            .insert("count".to_string(), keys.len().to_string());
        event.params.insert("keys".to_string(), keys.join("、"));
        event
    }

    /// 云同步转只读 / 清除云副本的倒计时（PRD-13-015：开始一次，期满前 14 / 7 / 1 天与当天各一次）。
    /// 需要用户决定续订与否，故是 UserAction；落点账户页。参数只有天数与日期。
    #[must_use]
    pub fn sync_read_only(kind: &str, days: i64, date: &str) -> Self {
        Self::new(
            NotificationClass::UserAction,
            NotificationSeverity::Warning,
            kind,
            Some(NotificationTarget::Settings("account".into())),
        )
        .with_param("days", days.to_string())
        .with_param("date", date)
    }

    /// 云端库在另一台设备上被删除了（ADR-151 修订 2026-10-15）：这台设备已自动停止同步它、本地
    /// 文件都在。事实陈述、无须决定，故是信息类；落点账户页（「云端库」一节里能恢复）。参数只有
    /// 库的显示名（库清单里的名字，不是笔记内容）。
    #[must_use]
    pub fn sync_vault_deleted_remotely(name: &str) -> Self {
        Self::info_at(
            "sync_vault_deleted_remotely",
            NotificationTarget::Settings("account".into()),
        )
        .with_param("name", name)
    }

    /// 付款失败、进入催缴宽限（纵切 3 C3，PRD-13-016）：服务照常，`date` 前没更新付款方式就退订并
    /// 降到免费档。要用户去更新付款方式，故是 UserAction；落点账户页「升级与购买」一节（那里有
    /// 「管理订阅」）。参数只有日期。
    #[must_use]
    pub fn billing_past_due(date: &str) -> Self {
        Self::new(
            NotificationClass::UserAction,
            NotificationSeverity::Warning,
            "billing_past_due",
            Some(NotificationTarget::Settings("account/billing".into())),
        )
        .with_param("date", date)
    }

    /// 账户因付款争议转为只读（纵切 3 C3）：服务端提供的对话、翻译、转写与同步上传都会被拒——与
    /// [`Self::session_expired`] 同一组（核心能力当场失效、要知道为什么），故是 SystemHealth /
    /// Warning，录音期间也透出。落点同 [`Self::billing_past_due`]。
    #[must_use]
    pub fn billing_frozen() -> Self {
        Self::new(
            NotificationClass::SystemHealth,
            NotificationSeverity::Warning,
            "billing_frozen",
            Some(NotificationTarget::Settings("account/billing".into())),
        )
    }

    /// 催缴或冻结解除、账户回到正常（纵切 3 C3）。通知中心没有「撤回」，这一条就是前两条的了结：
    /// 信息类，知道更好、忽略无碍。
    #[must_use]
    pub fn billing_restored() -> Self {
        Self::info_at(
            "billing_restored",
            NotificationTarget::Settings("account/billing".into()),
        )
    }

    /// 用量阈值预警（纵切 3 C4，PRD-13-040 ③）：订阅额度 / 预付 credits / 本月预付上限到了 80%、95%
    /// 或用尽。`kind` 由装配层拼好（`usage_<subscription|prepaid|spend_cap>_<p80|p95|exhausted>`），
    /// 本函数只定类与档：80% 是「知道更好」的信息类；95% 与用尽要用户决定（升级、购买、调上限或改用
    /// 本机），是 UserAction / Warning。落点账户页「订阅与用量」或其中的上限控件。不带任何数字之外的
    /// 东西——那几个数用户点进去就看得见，通知历史里不必再留一份。
    #[must_use]
    pub fn usage_threshold(kind: &str, needs_decision: bool, section: &str) -> Self {
        let target = NotificationTarget::Settings(format!("account/{section}"));
        if needs_decision {
            Self::new(
                NotificationClass::UserAction,
                NotificationSeverity::Warning,
                kind,
                Some(target),
            )
        } else {
            Self::info_at(kind.to_owned(), target)
        }
    }

    #[must_use]
    pub fn permission_revoked(kind: &str) -> Self {
        let mut event = Self::new(
            NotificationClass::SystemHealth,
            NotificationSeverity::Critical,
            "permission_revoked",
            Some(NotificationTarget::Settings("privacy".into())),
        );
        event
            .params
            .insert("permission".to_string(), kind.to_string());
        event
    }

    #[must_use]
    pub fn strict_local_blocked() -> Self {
        Self::new(
            NotificationClass::Failure,
            NotificationSeverity::Warning,
            "strict_local_blocked",
            Some(NotificationTarget::Settings("privacy".into())),
        )
    }

    /// 服务端下发的能力提示词目录**整份被拒**（连 v1 清单都不齐），已回落随包默认
    /// （ADR-050 修订 2026-09-04）。功能零损失，但运营在管理台看到的是「已生效」——
    /// 这条通知是两边唯一会说话的地方。`key` 只是模板名（`scope.name`），不含正文。
    #[must_use]
    pub fn prompt_catalog_rejected(version: u32, key: &str) -> Self {
        let mut event = Self::new(
            NotificationClass::SystemHealth,
            NotificationSeverity::Warning,
            "prompt_catalog_rejected",
            Some(NotificationTarget::Settings("ai_services".into())),
        );
        event
            .params
            .insert("version".to_string(), version.to_string());
        event.params.insert("key".to_string(), key.to_string());
        event
    }

    /// 服务端下发的能力提示词目录按**旧清单**部分生效：齐备到 `manifest_version`，
    /// `missing` 条比它新的模板正从随包默认取。运营重发一版即消失。
    #[must_use]
    pub fn prompt_catalog_degraded(version: u32, manifest_version: u32, missing: usize) -> Self {
        let mut event = Self::new(
            NotificationClass::SystemHealth,
            NotificationSeverity::Warning,
            "prompt_catalog_degraded",
            Some(NotificationTarget::Settings("ai_services".into())),
        );
        event
            .params
            .insert("version".to_string(), version.to_string());
        event
            .params
            .insert("manifest".to_string(), manifest_version.to_string());
        event
            .params
            .insert("missing".to_string(), missing.to_string());
        event
    }

    /// 截屏翻译：屏幕录制权限未授予（PRD-01-007）。引导到权限设置。
    #[must_use]
    pub fn screenshot_permission_missing() -> Self {
        Self::new(
            NotificationClass::Failure,
            NotificationSeverity::Warning,
            "screenshot_permission_missing",
            Some(NotificationTarget::Settings("privacy".into())),
        )
    }

    /// 截屏翻译：框选区域未识别到文字（或 OCR 失败）。无深链目标（就地重试即可）。
    #[must_use]
    pub fn screenshot_no_text() -> Self {
        Self::new(
            NotificationClass::Failure,
            NotificationSeverity::Info,
            "screenshot_no_text",
            None,
        )
    }

    /// 译境：取景框几何写盘失败（配置目录只读 / 损坏恢复态）。本次会话位置照常可用，但重启
    /// 后不会被记住——如实告知而非静默，引导用户去数据/隐私设置排查存储可写性。
    #[must_use]
    pub fn desktop_lens_geometry_not_saved() -> Self {
        Self::new(
            NotificationClass::Failure,
            NotificationSeverity::Warning,
            "desktop_lens_geometry_not_saved",
            Some(NotificationTarget::Settings("privacy".into())),
        )
    }

    /// 译境：取景框跨越了显示器边界，单次 `screencapture -R` 无法如实表达（PRD-01-034）。
    /// 无深链目标——把取景框移回同一块屏即可就地重试。
    #[must_use]
    pub fn desktop_lens_region_offscreen() -> Self {
        Self::new(
            NotificationClass::Failure,
            NotificationSeverity::Warning,
            "desktop_lens_region_offscreen",
            None,
        )
    }

    /// 启动核查：配置的默认 LLM 未安装，但本机已有其他可用模型 → 引导去模型页切换
    /// （PRD-01 §异常；不等首次生成才失败）。
    #[must_use]
    pub fn llm_default_model_missing(configured: &str) -> Self {
        let mut event = Self::new(
            NotificationClass::UserAction,
            NotificationSeverity::Warning,
            "llm_default_model_missing",
            Some(NotificationTarget::Settings("ai_services".into())),
        );
        event
            .params
            .insert("model".to_string(), configured.to_string());
        event
    }

    /// 启动核查：本机没有任何已安装的 LLM → 引导去模型页下载。
    #[must_use]
    pub fn llm_no_models_installed() -> Self {
        Self::new(
            NotificationClass::UserAction,
            NotificationSeverity::Warning,
            "llm_no_models_installed",
            Some(NotificationTarget::Settings("local_models".into())),
        )
    }

    #[must_use]
    pub fn failure(kind: impl Into<String>, target: NotificationTarget) -> Self {
        Self::new(
            NotificationClass::Failure,
            NotificationSeverity::Error,
            kind,
            Some(target),
        )
    }

    fn new(
        class: NotificationClass,
        severity: NotificationSeverity,
        kind: impl Into<String>,
        target: Option<NotificationTarget>,
    ) -> Self {
        let kind = kind.into();
        Self {
            id: next_notification_id(),
            class,
            severity,
            source_group: kind.clone(),
            i18n_key: format!("notifications:{kind}"),
            target,
            params: HashMap::new(),
            _private_payload: None,
        }
    }

    /// 给这条通知标注**它实际在哪儿执行**（i18next `context`：前端据此取到对应落点的
    /// 那条文案，渲染侧零改动）。
    ///
    /// 落点是 Rust 算出来的事实，绝不由前端从设置里再推一遍（ADR-003）——那正是
    /// 「界面说服务端、文案让你去查本机引擎」这类自相矛盾的来源。
    #[must_use]
    pub fn executed_at(mut self, location: crate::cloud::ComputeLocation) -> Self {
        self.params
            .insert("context".to_owned(), location.name().to_owned());
        self
    }

    fn with_private_payload(mut self, private_payload: PrivatePayload) -> Self {
        self._private_payload = Some(private_payload);
        self
    }

    fn dedupe_key(&self) -> String {
        let target = self
            .target
            .as_ref()
            .map(NotificationTarget::as_route)
            .unwrap_or_default();
        format!("{}:{}:{target}", self.class.as_str(), self.source_group)
    }
}

fn next_notification_id() -> String {
    let id = NEXT_NOTIFICATION_ID.fetch_add(1, Ordering::Relaxed);
    format!("notification-{id}")
}

/// 前端 typed event DTO。只包含可渲染的脱敏 key/参数/route。
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[tauri_specta(event_name = "notification://event")]
pub struct NotificationEventDto {
    pub id: String,
    pub class: String,
    pub severity: String,
    pub channels: Vec<String>,
    pub target: Option<String>,
    pub i18n_key: String,
    pub params: HashMap<String, String>,
    /// 诊断一节的 key（[`diagnostic_i18n_key`]）；`None` = 这类通知没有诊断。
    pub diagnostic_i18n_key: Option<String>,
}

impl From<NotificationEvent> for NotificationEventDto {
    /// 前台投影（历史回放 / 测试）。真实发布走 [`NotificationEventDto::with_presence`]。
    fn from(event: NotificationEvent) -> Self {
        Self::with_presence(event, Presence::Foreground)
    }
}

impl NotificationEventDto {
    /// 按在场态路由通道（PRD-15-010/012）：不在场时完成 / 失败 / 高危类带上 `system`，
    /// 前端据此经系统通知插件发出横幅（文案只在前端一份）。
    #[must_use]
    pub fn with_presence(event: NotificationEvent, presence: Presence) -> Self {
        let channels = NotificationPolicy
            .route(event.class, presence)
            .into_iter()
            .map(|channel| channel.as_str().to_string())
            .collect();

        Self {
            id: event.id,
            class: event.class.as_str().to_string(),
            severity: event.severity.as_str().to_string(),
            channels,
            target: event.target.map(|target| target.as_route()),
            diagnostic_i18n_key: diagnostic_i18n_key(&event.i18n_key),
            i18n_key: event.i18n_key,
            params: event.params,
        }
    }
}

/// 路由策略。
#[derive(Debug, Clone, Default)]
pub struct NotificationPolicy;

impl NotificationPolicy {
    #[must_use]
    pub fn route(&self, class: NotificationClass, presence: Presence) -> Vec<NotificationChannel> {
        match (class, presence) {
            (NotificationClass::SystemHealth, Presence::Foreground) => {
                vec![NotificationChannel::Banner, NotificationChannel::Status]
            }
            (NotificationClass::SystemHealth, Presence::Background) => vec![
                NotificationChannel::System,
                NotificationChannel::Banner,
                NotificationChannel::Status,
            ],
            (NotificationClass::Completion | NotificationClass::Info, Presence::Foreground) => {
                vec![NotificationChannel::Toast, NotificationChannel::Status]
            }
            (NotificationClass::Completion | NotificationClass::Info, Presence::Background) => {
                vec![NotificationChannel::System, NotificationChannel::Status]
            }
            (NotificationClass::Failure, Presence::Foreground) => {
                vec![NotificationChannel::Toast, NotificationChannel::Inline]
            }
            (NotificationClass::Failure, Presence::Background) => {
                vec![NotificationChannel::System, NotificationChannel::Inline]
            }
            (NotificationClass::UserAction, _) => {
                vec![NotificationChannel::Banner, NotificationChannel::Inline]
            }
        }
    }
}

/// 系统通知渲染结果。MVP-0 保持 key 化，不拼接私有文本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedNotification {
    pub title: String,
    pub body: String,
}

#[must_use]
pub fn redact_for_system(event: &NotificationEvent) -> RenderedNotification {
    RenderedNotification {
        title: format!("{}.title", event.i18n_key),
        body: format!("{}.body", event.i18n_key),
    }
}

#[derive(Debug)]
struct DedupeWindow {
    window: Duration,
    seen: HashMap<String, Instant>,
}

impl Default for DedupeWindow {
    fn default() -> Self {
        Self {
            window: Duration::from_secs(30),
            seen: HashMap::new(),
        }
    }
}

impl DedupeWindow {
    fn should_emit(&mut self, event: &NotificationEvent) -> bool {
        let now = Instant::now();
        self.seen
            .retain(|_, last_seen| now.duration_since(*last_seen) <= self.window);

        let key = event.dedupe_key();
        if self
            .seen
            .get(&key)
            .is_some_and(|last_seen| now.duration_since(*last_seen) <= self.window)
        {
            return false;
        }

        self.seen.insert(key, now);
        true
    }
}

/// 「系统通知」总闸（`notifications.system_enabled`，PRD-15-070）；AppState 未就绪时按开处理。
fn system_channel_enabled(app: &tauri::AppHandle) -> bool {
    use tauri::Manager as _;
    app.try_state::<crate::ipc::AppState>()
        .and_then(|state| {
            state
                .settings_snapshot()
                .get_bool("notifications.system_enabled")
        })
        .unwrap_or(true)
}

/// 主窗此刻的在场态：可见、未最小化且聚焦才算在场。没有主窗（极早期）按在场处理，
/// 免得启动期的事件跑去敲系统横幅。
fn presence_of(app: &tauri::AppHandle) -> Presence {
    use tauri::Manager as _;
    let Some(main) = app.get_webview_window("main") else {
        return Presence::Foreground;
    };
    presence_from(
        main.is_focused().unwrap_or(true),
        main.is_visible().unwrap_or(true),
        main.is_minimized().unwrap_or(false),
    )
}

/// 在场判定的纯函数（可单测）：三个窗口事实 → 在场态。
#[must_use]
pub fn presence_from(focused: bool, visible: bool, minimized: bool) -> Presence {
    if visible && !minimized && focused {
        Presence::Foreground
    } else {
        Presence::Background
    }
}

/// 首启引导是否已收场（`onboarding.completed`）。AppState 未就绪时按「已收场」处理——
/// 那只发生在测试与极早期，暂扣在那种场合没有意义。
fn onboarding_completed(app: &tauri::AppHandle) -> bool {
    use tauri::Manager as _;
    app.try_state::<crate::ipc::AppState>()
        .and_then(|state| state.settings_snapshot().get_bool("onboarding.completed"))
        .unwrap_or(true)
}

/// Rust 侧单一通知汇聚点。
/// 录音期间要不要扣下这条通知（PRD-15-032，纯函数便于单测）：只有非高危类、录音在会话中、
/// 且用户没关「录音时不打扰」三者同时成立才扣；系统健康（高危）永远透出。
#[must_use]
pub fn held_while_recording(class: NotificationClass, recording: bool, enabled: bool) -> bool {
    enabled && recording && class != NotificationClass::SystemHealth
}

/// 录音控制器此刻是否在会话中（录音 / 暂停 / 停止中）。
fn recording_in_session(app: &tauri::AppHandle) -> bool {
    use tauri::Manager as _;
    app.try_state::<crate::ipc::AppState>()
        .is_some_and(|state| state.recording_in_session())
}

/// 设置 `notifications.suppress_while_recording`（默认开）。
fn suppress_while_recording_enabled(app: &tauri::AppHandle) -> bool {
    use tauri::Manager as _;
    app.try_state::<crate::ipc::AppState>()
        .and_then(|state| {
            state
                .settings_snapshot()
                .get_bool("notifications.suppress_while_recording")
        })
        .unwrap_or(true)
}

#[derive(Default)]
pub struct NotificationCenter {
    dedupe: Mutex<DedupeWindow>,
    device_history: Mutex<Option<NotificationHistoryStore>>,
    vault: Mutex<Option<Arc<Mutex<Option<luna_vault::Vault>>>>>,
    /// 首启引导收场前被暂扣的「需你决定」类通知（ADR-114 ③）；收场时按原顺序补发。
    deferred: Mutex<Vec<NotificationEvent>>,
    /// 录音 / 实时转写进行中被扣下的非高危通知（PRD-15-032）：历史已记，录音结束补发。
    held: Mutex<Vec<NotificationEvent>>,
}

impl NotificationCenter {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn configure_history(
        &self,
        device_usage: luna_vault::UsageStore,
        vault: Arc<Mutex<Option<luna_vault::Vault>>>,
    ) {
        let device_history = notification_store_for_usage_store(&device_usage).ok();
        if let Ok(mut slot) = self.device_history.lock() {
            *slot = device_history;
        }
        if let Ok(mut slot) = self.vault.lock() {
            *slot = Some(vault);
        }
    }

    pub fn publish(&self, event: NotificationEvent, app: &tauri::AppHandle) {
        // 引导没收场时，「需你决定」类一律暂扣（连历史都先不记——它们不是被错过，
        // 而是还没到时候）：启动核查发的「本机没装 LLM」说的正是引导第四步要讲的事，
        // 弹在第一页上只会抢戏。完成 / 失败 / 系统健康不扣——那是正在发生的事实。
        if event.class == NotificationClass::UserAction && !onboarding_completed(app) {
            if let Ok(mut deferred) = self.deferred.lock() {
                deferred.push(event);
            }
            return;
        }
        // 录音 / 实时转写进行中（PRD-15-032）：非高危通知扣下不弹——开会时弹横幅体验极差。
        // 历史此刻就记（铃铛角标如实），录音结束后按原顺序补发（`flush_held`）。
        if held_while_recording(
            event.class,
            recording_in_session(app),
            suppress_while_recording_enabled(app),
        ) {
            let dto = NotificationEventDto::with_presence(event.clone(), presence_of(app));
            self.append_history(&event, &dto);
            if let Ok(mut held) = self.held.lock() {
                held.push(event);
            }
            return;
        }
        self.deliver(event, app, true);
    }

    /// 真正投递：算在场态、（可选）记历史、去重、分类开关、发事件。
    fn deliver(&self, event: NotificationEvent, app: &tauri::AppHandle, record_history: bool) {
        // 在场态（PRD-15-010）：主窗可见且聚焦 = 在场；否则不在场，完成 / 失败 / 高危类
        // 多带一条 `system` 通道。`notifications.system_enabled` 关着时一律按在场路由。
        let presence = if system_channel_enabled(app) {
            presence_of(app)
        } else {
            Presence::Foreground
        };
        let dto = NotificationEventDto::with_presence(event.clone(), presence);
        // 排障开关（`LUNA_DEBUG_NOTIFICATIONS=1`）：只打事件种类、在场态与通道，不含任何正文。
        if std::env::var_os("LUNA_DEBUG_NOTIFICATIONS").is_some() {
            eprintln!(
                "[notify] kind={} class={} presence={presence:?} channels={:?}",
                event.i18n_key,
                event.class.as_str(),
                dto.channels
            );
        }
        if record_history {
            self.append_history(&event, &dto);
        }
        let should_emit = self
            .dedupe
            .lock()
            .expect("notification dedupe lock")
            .should_emit(&event);

        if should_emit && !self.class_muted(&event, app) {
            let _ = dto.emit(app);
        }
    }

    /// 引导收场：把暂扣的通知按原顺序真正发出。
    pub fn flush_deferred(&self, app: &tauri::AppHandle) {
        let pending: Vec<NotificationEvent> = self
            .deferred
            .lock()
            .map(|mut deferred| std::mem::take(&mut *deferred))
            .unwrap_or_default();
        for event in pending {
            self.publish(event, app);
        }
    }

    /// 录音结束：把扣下的通知按原顺序发出（历史扣下时已记，此处不再记）。
    pub fn flush_held(&self, app: &tauri::AppHandle) {
        let pending: Vec<NotificationEvent> = self
            .held
            .lock()
            .map(|mut held| std::mem::take(&mut *held))
            .unwrap_or_default();
        for event in pending {
            self.deliver(event, app, false);
        }
    }

    /// 录音扣下队列长度（测试与诊断）。
    #[must_use]
    pub fn held_len(&self) -> usize {
        self.held.lock().map(|h| h.len()).unwrap_or(0)
    }

    /// 暂扣队列长度（测试与诊断）。
    #[must_use]
    pub fn deferred_len(&self) -> usize {
        self.deferred.lock().map(|d| d.len()).unwrap_or(0)
    }

    /// 分类开关（PRD-09-060 / PRD-15-001）：关掉「任务完成通知」即**不再前台打扰**，
    /// 但历史照记（PRD-09 §3 验收：关掉后仍可在通知历史中心看到）。
    ///
    /// 系统健康（高危）类**不可关**——关掉等于自废安全网（PRD-15-070），故只判 Completion。
    /// 此前该设置登记了却无人读取：用户关掉开关，完成通知照弹。
    fn class_muted(&self, event: &NotificationEvent, app: &tauri::AppHandle) -> bool {
        use tauri::Manager as _;
        if event.class != NotificationClass::Completion {
            return false;
        }
        !app.try_state::<crate::ipc::AppState>()
            .and_then(|state| {
                state
                    .settings_snapshot()
                    .get_bool("notifications.completed_enabled")
            })
            .unwrap_or(true)
    }

    /// 一页历史 = **设备级 ∪ 当前 Vault** 的合并投影（PRD-15-060），时间倒序。
    ///
    /// 两个库各取一页再合并截断：这是合并投影唯一正确的翻页方式——各取 `limit` 条，
    /// 合并后前 `limit` 条一定是全局最新的那 `limit` 条。
    ///
    /// **排序键必须与游标键逐字相同**（`created_at_ms` 降序、同毫秒按 `id` 降序）：
    /// 此前只按时间排，同一毫秒写进的多条在两个库之间没有确定次序，翻页会跳条或重复。
    pub fn history(
        &self,
        limit: usize,
        after: Option<&HistoryCursor>,
    ) -> Vec<NotificationHistoryRecord> {
        let mut records = self
            .device_history
            .lock()
            .ok()
            .and_then(|store| {
                store
                    .as_ref()
                    .map(|store| store.list(limit, after).unwrap_or_default())
            })
            .unwrap_or_default();
        if let Some(store) = self.current_vault_history_store() {
            records.extend(store.list(limit, after).unwrap_or_default());
        }
        records.sort_by(|a, b| {
            b.created_at_ms
                .cmp(&a.created_at_ms)
                .then_with(|| b.id.cmp(&a.id))
        });
        records.truncate(limit);
        records
    }

    /// 全部条数（合并投影）——页头说的是"一共有多少条"，不是"这一页装了多少条"。
    pub fn total(&self) -> u64 {
        let device = self
            .device_history
            .lock()
            .ok()
            .and_then(|store| {
                store
                    .as_ref()
                    .map(|store| store.total().unwrap_or_default())
            })
            .unwrap_or_default();
        let vault = self
            .current_vault_history_store()
            .map_or(0, |store| store.total().unwrap_or_default());
        device + vault
    }

    pub fn unread_count(&self) -> u64 {
        let device = self
            .device_history
            .lock()
            .ok()
            .and_then(|store| {
                store
                    .as_ref()
                    .map(|store| store.unread_count().unwrap_or_default())
            })
            .unwrap_or_default();
        let vault = self
            .current_vault_history_store()
            .map_or(0, |store| store.unread_count().unwrap_or_default());
        device + vault
    }

    pub fn mark_read(&self, id: Option<&str>) -> usize {
        let mut changed = self
            .device_history
            .lock()
            .ok()
            .and_then(|store| {
                store
                    .as_ref()
                    .map(|store| store.mark_read(id).unwrap_or_default())
            })
            .unwrap_or_default();
        if let Some(store) = self.current_vault_history_store() {
            changed += store.mark_read(id).unwrap_or_default();
        }
        changed
    }

    pub fn clear_history(&self) -> usize {
        let mut changed = self
            .device_history
            .lock()
            .ok()
            .and_then(|store| {
                store
                    .as_ref()
                    .map(|store| store.clear().unwrap_or_default())
            })
            .unwrap_or_default();
        if let Some(store) = self.current_vault_history_store() {
            changed += store.clear().unwrap_or_default();
        }
        changed
    }

    /// 只清掉已读的那些（`PRD-15-065`），设备级与当前 Vault 两份一起。
    ///
    /// 历史此前只有「全清」一种出口：为了不丢掉一条还没处理的未读，用户只能什么都
    /// 不清——于是越积越长、越长越不敢清。已读＝看过并处理过的，清掉不损失待办信息。
    pub fn clear_read_history(&self) -> usize {
        let mut changed = self
            .device_history
            .lock()
            .ok()
            .and_then(|store| {
                store
                    .as_ref()
                    .map(|store| store.clear_read().unwrap_or_default())
            })
            .unwrap_or_default();
        if let Some(store) = self.current_vault_history_store() {
            changed += store.clear_read().unwrap_or_default();
        }
        changed
    }

    fn append_history(&self, event: &NotificationEvent, dto: &NotificationEventDto) {
        let record = NotificationHistoryRecord {
            id: dto.id.clone(),
            class: dto.class.clone(),
            severity: dto.severity.clone(),
            channels: dto.channels.clone(),
            target: dto.target.clone(),
            i18n_key: dto.i18n_key.clone(),
            params: dto.params.clone(),
            created_at_ms: now_ms(),
            read: false,
        };
        match history_route(event) {
            HistoryRoute::Device => {
                if let Ok(slot) = self.device_history.lock() {
                    if let Some(store) = slot.as_ref() {
                        let _ = store.append(&record);
                    }
                }
            }
            HistoryRoute::Vault => {
                if let Some(store) = self.current_vault_history_store() {
                    let _ = store.append(&record);
                }
            }
        }
    }

    fn current_vault_history_store(&self) -> Option<NotificationHistoryStore> {
        let vault = self.vault.lock().ok()?.as_ref()?.clone();
        let guard = vault.lock().ok()?;
        let usage_store = guard.as_ref()?.usage_store();
        notification_store_for_usage_store(&usage_store).ok()
    }
}

enum HistoryRoute {
    Device,
    Vault,
}

fn history_route(event: &NotificationEvent) -> HistoryRoute {
    if matches!(
        event.class,
        NotificationClass::SystemHealth | NotificationClass::Info
    ) || event
        .target
        .as_ref()
        .is_some_and(|target| target.as_route().starts_with("settings://"))
    {
        HistoryRoute::Device
    } else {
        HistoryRoute::Vault
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn settings_notifications_target_current_registry_pages() {
        use super::{NotificationEvent, NotificationTarget};
        let events = [
            NotificationEvent::signed_in(),
            NotificationEvent::config_recovered(),
            NotificationEvent::config_values_reset(&["appearance.theme"]),
            NotificationEvent::permission_revoked("microphone"),
            NotificationEvent::strict_local_blocked(),
            NotificationEvent::prompt_catalog_rejected(1, "translate.default"),
            NotificationEvent::prompt_catalog_degraded(2, 1, 1),
            NotificationEvent::screenshot_permission_missing(),
            NotificationEvent::desktop_lens_geometry_not_saved(),
            NotificationEvent::llm_default_model_missing("missing"),
            NotificationEvent::llm_no_models_installed(),
            NotificationEvent::sync_vault_deleted_remotely("笔记"),
        ];
        for event in events {
            let Some(NotificationTarget::Settings(route)) = event.target else {
                panic!("settings target required");
            };
            let page = route.split('/').next().unwrap();
            assert!(
                luna_config::registry::SETTINGS_PAGES
                    .iter()
                    .any(|entry| entry.as_str() == page),
                "unknown page: {route}"
            );
        }
    }

    /// 云端库在别处被删除：信息类、落点账户页，参数只有库的显示名（ADR-151 修订 2026-10-15）。
    #[test]
    fn a_remote_vault_deletion_is_an_info_notice_naming_the_vault() {
        use super::{NotificationEvent, NotificationEventDto};
        let dto =
            NotificationEventDto::from(NotificationEvent::sync_vault_deleted_remotely("读书"));
        assert_eq!(dto.class, "info");
        assert_eq!(dto.i18n_key, "notifications:sync_vault_deleted_remotely");
        assert_eq!(dto.target.as_deref(), Some("settings://account"));
        assert_eq!(dto.params.len(), 1);
        assert_eq!(dto.params.get("name").map(String::as_str), Some("读书"));
    }

    /// 诊断只挂在声明过的种类上，且实时事件与历史回放派生出同一个 key。
    #[test]
    fn diagnostics_are_declared_per_kind() {
        use super::{diagnostic_i18n_key, NotificationEvent, NotificationEventDto};
        let dto = NotificationEventDto::from(NotificationEvent::prompt_catalog_rejected(3, "a.b"));
        assert_eq!(
            dto.diagnostic_i18n_key.as_deref(),
            Some("notifications:prompt_catalog_rejected.diagnostic")
        );
        assert_eq!(
            diagnostic_i18n_key(&dto.i18n_key),
            dto.diagnostic_i18n_key,
            "历史回放按同一函数派生"
        );
        let plain = NotificationEventDto::from(NotificationEvent::strict_local_blocked());
        assert_eq!(plain.diagnostic_i18n_key, None);
        assert_eq!(diagnostic_i18n_key("cloud:prompt_catalog_rejected"), None);
    }

    /// 在场 = 可见 ∧ 未最小化 ∧ 聚焦；任一不成立即不在场（PRD-15-010）。
    #[test]
    fn presence_requires_visible_unminimized_and_focused() {
        use super::{presence_from, Presence};
        assert_eq!(presence_from(true, true, false), Presence::Foreground);
        assert_eq!(presence_from(false, true, false), Presence::Background);
        assert_eq!(presence_from(true, false, false), Presence::Background);
        assert_eq!(presence_from(true, true, true), Presence::Background);
    }

    /// 不在场时完成类带 `system` 通道；在场时不带（前台抑制）。
    #[test]
    fn background_completion_carries_the_system_channel() {
        use super::{
            NotificationEvent, NotificationEventDto, NotificationTarget, Presence, PrivatePayload,
        };
        let event = || {
            NotificationEvent::completion(
                "batch_done",
                NotificationTarget::Settings("x".into()),
                PrivatePayload::new(""),
            )
        };
        let away = NotificationEventDto::with_presence(event(), Presence::Background);
        assert!(away.channels.iter().any(|c| c == "system"));
        let here = NotificationEventDto::with_presence(event(), Presence::Foreground);
        assert!(!here.channels.iter().any(|c| c == "system"));
    }

    use super::*;

    #[test]
    fn health_event_always_routes_to_banner() {
        let policy = NotificationPolicy;
        let channels = policy.route(NotificationClass::SystemHealth, Presence::Foreground);

        assert!(channels.contains(&NotificationChannel::Banner));
    }

    #[test]
    fn foreground_completion_uses_toast_not_system_banner() {
        let policy = NotificationPolicy;
        let channels = policy.route(NotificationClass::Completion, Presence::Foreground);

        assert!(channels.contains(&NotificationChannel::Toast));
        assert!(!channels.contains(&NotificationChannel::System));
    }

    #[test]
    fn completion_event_is_not_system_health_and_routes_to_vault() {
        // 「转写已优化完成」等成功事件必须是 Completion/Info（非 Critical/系统健康），
        // 且落 Vault 历史（内容相关）而非设备级健康库。
        let event = NotificationEvent::completion(
            "recording_retranscribed",
            NotificationTarget::Route("recording://current".into()),
            PrivatePayload::new("note.md 正文不得外泄"),
        );
        assert_eq!(event.class, NotificationClass::Completion);
        assert_eq!(event.severity, NotificationSeverity::Info);
        assert!(matches!(history_route(&event), HistoryRoute::Vault));
        // 脱敏：私有载荷绝不进系统渲染文本。
        let rendered = redact_for_system(&event);
        assert!(!rendered.body.contains("正文不得外泄"));
    }

    #[test]
    fn redaction_removes_private_payload_text() {
        let event = NotificationEvent::system_health(
            "permission_revoked",
            NotificationTarget::Settings("privacy".into()),
            PrivatePayload::new("clipboard text must not appear"),
        );
        let rendered = redact_for_system(&event);

        assert!(!rendered.title.contains("clipboard text"));
        assert!(!rendered.body.contains("clipboard text"));
    }

    #[test]
    fn held_while_recording_spares_system_health_and_respects_the_switch() {
        assert!(held_while_recording(
            NotificationClass::Completion,
            true,
            true
        ));
        assert!(held_while_recording(NotificationClass::Failure, true, true));
        assert!(held_while_recording(
            NotificationClass::UserAction,
            true,
            true
        ));
        assert!(!held_while_recording(
            NotificationClass::SystemHealth,
            true,
            true
        ));
        assert!(!held_while_recording(
            NotificationClass::Completion,
            false,
            true
        ));
        assert!(!held_while_recording(
            NotificationClass::Completion,
            true,
            false
        ));
    }

    #[test]
    fn dedupe_suppresses_same_class_group_and_target() {
        let mut window = DedupeWindow::default();
        let first = NotificationEvent::permission_revoked("accessibility");
        let duplicate = NotificationEvent::permission_revoked("accessibility");

        assert!(window.should_emit(&first));
        assert!(!window.should_emit(&duplicate));
    }

    #[test]
    fn dto_does_not_include_private_payload() {
        let event = NotificationEvent::system_health(
            "permission_revoked",
            NotificationTarget::Settings("privacy".into()),
            PrivatePayload::new("note body must not appear"),
        );

        let dto = NotificationEventDto::from(event);
        let json = serde_json::to_string(&dto).unwrap();

        assert!(!json.contains("note body"));
        assert!(!json.contains("must not appear"));
    }

    #[test]
    fn history_route_keeps_system_health_device_level() {
        let event = NotificationEvent::permission_revoked("accessibility");
        assert!(matches!(history_route(&event), HistoryRoute::Device));
    }

    #[test]
    fn configured_history_returns_appended_records() {
        let device_dir = tempfile::tempdir().unwrap();
        let vault_dir = tempfile::tempdir().unwrap();
        let device_usage =
            luna_vault::UsageStore::open(device_dir.path().join("usage-device.sqlite")).unwrap();
        let vault = Arc::new(Mutex::new(Some(
            luna_vault::Vault::open(vault_dir.path()).unwrap(),
        )));
        let center = NotificationCenter::new();
        center.configure_history(device_usage, vault);

        center.append_history(
            &NotificationEvent::permission_revoked("accessibility"),
            &NotificationEventDto::from(NotificationEvent::permission_revoked("accessibility")),
        );

        assert_eq!(center.unread_count(), 1);
        let history = center.history(20, None);
        assert_eq!(history.len(), 1);
        let json = serde_json::to_string(&history[0]).unwrap();
        assert!(!json.contains("clipboard"));
        assert!(!json.contains("note body"));
    }
}
