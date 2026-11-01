//! 云客户端的默认装配（ADR-037、ADR-155）。
//!
//! 单一构建后云不再是「另一个风味」，而是**同一个二进制里显式可选的服务**。本模块把
//! `luna-cloud` 插件缝进默认装配，并守住 ADR-037 的替代护栏：
//!
//! **未登录时，客户端不自发产生任何云网络行为。**
//!
//! 精确边界（散文易夸大，故在此写死）：
//! - **启动、面板挂载、后台**：未登录时零云请求。插件 `setup` 只构造 client 不发请求；
//!   `account_state` 读钥匙链、`catalog_status` 读本地状态；唯一的后台任务（`luna-cloud`
//!   的目录同步循环）每次醒来先问 `ConnectedProbe`，未登录就一个字节也不发。
//!   > 已登录时，启动装配（[`reconfigure_cloud_stack`]）会为对话上下文窗拉一次服务端
//!   > 模型清单，并触发一次目录同步（[`spawn_catalog_sync`]，超龄才真的拉）；之后目录同步
//!   > 循环最多每六小时拉一次（ADR-050 修订 2026-09-05 ③）。
//! - **需要会话的命令**：`client::authed` 在无 refresh token 时**先本地短路**返回
//!   `NotLoggedIn`——未登录时一个字节都不会发出。
//! - **默认态的两组显式动作**：「登录」（`account_sign_in_*`）与「下载模型」（安装命令：下载前按龄
//!   刷新一次本地模型目录，下载时先问后端制品面的镜像，ADR-145 ⑥ 修订 2026-10-07）。「检查更新」
//!   （ADR-170）需要登录，不在例外里。它们只是不受「未登录时不自发联网」
//!   限制，仍受更高优先级的**严格本地硬闸**约束；严格本地下一个字节都不发。
//!   该例外清单由 `luna_cloud::commands` 的命令面测试**机器钉死**。
//!
//! # 两条门
//! 1. **严格本地模式**（AGENTS.md §7）：拦一切联网型动作；
//! 2. **未登录**：没有会话时任何云路径都不得发起请求，AI 全在本机。
//!
//! 两条都不满足才允许走云。判据全部**本地可答**（读设置内存态 + 读钥匙链），
//! 判断本身绝不联网——否则「判断要不要联网」这一步先违反了不变量。

use std::sync::{Arc, OnceLock};

use tauri::{AppHandle, Manager as _, Runtime};

use crate::artifact_fetcher::{
    ArtifactFetcher, ArtifactProgress, ArtifactRequest, DirectArtifactFetcher,
};

/// 算力落点变了 —— **一次换齐全部消费者**的唯一入口。
///
/// # 为什么必须只有这一个函数
///
/// 服务器地址有六个各自独立捕获它的消费者：账户客户端、目录客户端、制品面 fetcher、
/// 批式转写、实时转写、对话 Provider。散在各处各换各的，迟早有人只改其中一处——
/// 结果是转写打新服务器、而账户与模型下载仍打旧的。收成一个函数之后，漏改的代价是
/// "这一处没写进来"，而不是"某个分支的条件写漏了"。
///
/// **两把公钥与地址同批换**：它们都由云客户端从账户与信任锚派生（`CloudClient::reconfigure`）。
///
/// # 两类触发源，一个入口
///
/// 1. **账户变化**（选定服务器 / 登录 / 退出 / 注销 / 重钉，`luna_cloud` 的账户观察者，
///    见 `crate::default_assembly`）——登录态决定 [`compute_endpoint_source`] 派不派得出端点；
/// 2. **隐私档**（`privacy.strict_local`：它不改「连哪台」，但决定「连不连」）。
///
/// **启动也走这里**（`lib::run` 的 `app.manage(state)` 之后调用一次）。
///
/// [`compute_endpoint_source`]: crate::ipc::compute_endpoint_source
pub fn reconfigure_cloud_stack(app: &AppHandle, state: &crate::ipc::AppState) {
    // 门控只求值一次，往下传：对话端点、落点展示与门控本身从此读同一个答案。
    // 各求各的必然有一天门控放行而界面说「在本机跑」，或者反过来。
    let availability = evaluate_for(app);
    // 对话与嵌入打同一台服务端、用同一份账户会话。
    let bearer: Arc<dyn luna_provider::asr_remote::BearerSource> =
        Arc::new(crate::cloud_bearer::CloudBearer::new(app.clone()));

    // ① 账户/会话客户端：按记住的服务器与它的钉住公钥重新派生；宿主据此记下「服务器是谁」。
    let config = match app.try_state::<luna_cloud::commands::CloudState>() {
        Some(cloud) => {
            cloud.0.reconfigure();
            cloud.0.config()
        }
        None => Arc::new(luna_cloud::CloudConfig::derive(
            luna_cloud::config::DEFAULT_API_BASE,
            None,
        )),
    };
    state.set_cloud_server(ServerRef::of(&config.api_base));
    // 账户可用的 AI 能力（R4b）：与服务器同一刻记下，下面各路装配都按它分流。
    state.set_cloud_capabilities(
        app.try_state::<luna_cloud::commands::CloudState>()
            .and_then(|cloud| cloud.0.allowed_capabilities()),
    );
    // 这台服务端配好了哪几项（ADR-041 ①）：没配上游的能力同样在本机跑，与权益同一刻记下。
    state.set_cloud_deployment(
        app.try_state::<luna_cloud::commands::CloudState>()
            .and_then(|cloud| cloud.0.deployment_facts()),
    );
    // ② 目录客户端（含防回滚水位的服务端分桶）。**全部目录都要换**：每份各有自己的水位与
    // 缓存，漏掉一份的表现是上一台服务端的内容继续生效——屏幕上写的服务器与实际用的模板、
    // 可装的主题对不上。成组的动作只在 `Catalogs` 里写一次。
    if let Some(catalogs) = app.try_state::<luna_cloud::commands::Catalogs>() {
        catalogs.reconfigure(&config.api_base, config.catalog_pubkey);
    }
    // ③ 制品下载（模型字节从哪来）。
    if let Some(cell) = app.try_state::<crate::cloud_artifact::CloudAddress>() {
        crate::cloud_artifact::set_address(&cell.0, &config.api_base);
    }
    // ④⑤ 转写两条通道。外发闸先对齐再重装——重装只换新会话用的句柄，
    // 正在跑的那条腿仍持有自己的 `Arc`（`OutboundGate` 的论证）。
    state.sync_outbound_gate();
    if let Err(e) = state.refresh_asr_assembly(app) {
        eprintln!("[设置] 换服务端后 ASR 装配对齐失败：{e}");
    }
    // ⑥ 对话 / 翻译。**默认档在前、实时档在后**——注册顺序即兜底顺序，反过来的话每一条
    // 没有 `preferred_provider` 的 LLM Job 都会落到那个「小模型换低延迟」的实时档上
    // （空注册表的那一次，即启动，才会真的按这个顺序追加）。
    // 端点与有效分配由装配函数从同一份设置快照解析。原生服务端请求省略具体模型，由服务端
    // 托管路由；旧 `service:<model>` 判不兼容，不解释也不迁移。
    state.register_llm_provider(availability, Arc::clone(&bearer));
    state.refresh_realtime_llm_provider(availability);
    // ⑦ 嵌入。它与对话同一台服务端、同一份会话；落点变了它必须一起走——不然用户读到
    // 「AI 由服务端提供」，而全库重建索引仍在他自己那台老机器上跑分钟级。
    crate::ipc::refresh_embed_provider(state, availability, bearer);
    // ⑧ 对话上下文窗：落点变了，「窗有多大」的答案也变了（本机问引擎、服务端问服务端）。
    // 派到后台是因为两边都要 await，而本函数在设置写入与启动装配的同步路径上。
    let app_for_catalogs = app.clone();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        crate::ipc::refresh_chat_context_window(&app).await;
    });
    // ⑨ 两份目录：连上 / 换了服务端 / 登录成功，都是「这台服务端的目录该拉一遍」的时刻
    // （ADR-050 修订 ③）。只在门控放行时做——未登录且未配置时一个字节都不发（ADR-037）；
    // 新鲜度按 6 小时判，启动那次若刚刷过就不重复。
    if availability.allowed() {
        spawn_catalog_sync(&app_for_catalogs);
        spawn_capabilities_refresh(&app_for_catalogs);
        spawn_usage_refresh(&app_for_catalogs);
    }
}

/// 登录态下后台取一次权益，记下账户可用的 AI 能力（R4b）。清单变了由
/// [`luna_cloud::commands::fetch_entitlements`] 触发再一次重装；没变则到此为止（不会循环）。
fn spawn_capabilities_refresh(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let (Some(cloud), Some(observer)) = (
            app.try_state::<luna_cloud::commands::CloudState>(),
            app.try_state::<luna_cloud::commands::AccountObserverState>(),
        ) else {
            return;
        };
        if let Err(err) = luna_cloud::commands::fetch_entitlements(&cloud.0, &observer).await {
            eprintln!("[cloud] 取账户可用能力失败（按全部提供路由）：{err}");
        }
    });
}

/// 登录态下后台取一次用量（纵切 3 C4 启动预热）：不打开账户页也能在 80% / 95% / 用尽 / 上限时收到
/// 提醒。之后由 `luna-cloud` 的后台循环每 15 分钟再取（同样只在连着时）。取数走唯一那条
/// [`luna_cloud::commands::fetch_usage`]，轮次变了由它通知 [`notice_usage_alerts`]。
fn spawn_usage_refresh(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let (Some(cloud), Some(observer)) = (
            app.try_state::<luna_cloud::commands::CloudState>(),
            app.try_state::<luna_cloud::commands::AccountObserverState>(),
        ) else {
            return;
        };
        if let Err(err) = luna_cloud::commands::fetch_usage(&cloud.0, &observer).await {
            eprintln!("[cloud] 启动预热取用量失败（下一次刷新再取）：{err}");
        }
    });
}

/// 用量阈值通知已发到哪一步的落盘位置（此刻账户的提醒标记目录下，[`crate::account_scope::marker_dir`]；
/// 跨重启保持，同一轮同一档只提醒一次）。按账户分：轮次 id 形如 `spend_cap@2026-10`，两个账户会撞上。
pub(crate) const USAGE_NOTICE_FILE: &str = "usage-alert-notice.json";

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct UsageNoticeMarker {
    /// 预警轮次 id → 这一轮已提醒到的最高档（见 `luna_cloud::usage::UsageAlertEpisode`）。只留档位
    /// 高于 `none` 且仍出现在最近一次答复里的轮次：换周期 / 再买一包 / 新的自然月即新的一轮，旧的丢掉。
    notified: std::collections::BTreeMap<String, luna_cloud::client::UsageAlertDto>,
}

/// 要发的一条用量阈值通知：关于哪一类、到了哪一档。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UsageNotice {
    /// `subscription` / `prepaid` / `spend_cap`（通知种类名的中段）。
    subject: &'static str,
    level: luna_cloud::client::UsageAlertDto,
}

impl UsageNotice {
    fn event(self) -> crate::notifications::NotificationEvent {
        let kind = format!("usage_{}_{}", self.subject, self.level.as_str());
        let section = if self.subject == "spend_cap" {
            "spend_cap"
        } else {
            "usage"
        };
        crate::notifications::NotificationEvent::usage_threshold(
            &kind,
            self.level >= luna_cloud::client::UsageAlertDto::P95,
            section,
        )
    }
}

/// 预警针对的那一类有没有文案（服务端将来新增的账本种类没有——记标记、不发通知）。
fn usage_notice_subject(subject: &luna_cloud::usage::AlertSubject) -> Option<&'static str> {
    match subject {
        luna_cloud::usage::AlertSubject::SpendCap => Some("spend_cap"),
        luna_cloud::usage::AlertSubject::Ledger { kind } => match kind.as_str() {
            "subscription" => Some("subscription"),
            "prepaid" => Some("prepaid"),
            _ => None,
        },
    }
}

/// 纯判定：上一次落盘的标记 + 这一次认出的预警轮次 → （要记下的标记，要发的通知）。
///
/// - 每一轮只在**升到更高一档**时提醒一次，且只提醒这一次升到的最高档（从 none 直接跳到用尽，只说
///   「用尽」，不补发 80% / 95%）；
/// - 同一轮里降回去（如升级了档位、上限调高了）再升回原档，不重复提醒——那一档已经说过了；
/// - 轮次 id 变了（换周期、再买一包、新的自然月）就是新的一轮，重新从头算；
/// - 同一次取数里同一类同一档只发一条（订阅里两维同时到 80%，用户要知道的是「套餐快用完了」）。
fn usage_notice_step(
    notified: &std::collections::BTreeMap<String, luna_cloud::client::UsageAlertDto>,
    episodes: &[luna_cloud::usage::UsageAlertEpisode],
) -> (
    std::collections::BTreeMap<String, luna_cloud::client::UsageAlertDto>,
    Vec<UsageNotice>,
) {
    use luna_cloud::client::UsageAlertDto;
    let mut next = std::collections::BTreeMap::new();
    let mut notices: Vec<UsageNotice> = Vec::new();
    for episode in episodes {
        let before = notified
            .get(&episode.id)
            .copied()
            .unwrap_or(UsageAlertDto::None);
        let highest = before.max(episode.level);
        if highest > UsageAlertDto::None {
            next.insert(episode.id.clone(), highest);
        }
        if episode.level <= before {
            continue;
        }
        if let Some(subject) = usage_notice_subject(&episode.subject) {
            let notice = UsageNotice {
                subject,
                level: episode.level,
            };
            if !notices.contains(&notice) {
                notices.push(notice);
            }
        }
    }
    (next, notices)
}

/// 用量预警轮次变了（`AccountChange::UsageAlerts`，判定在 `luna_cloud::commands::fetch_usage`）→ 按落盘
/// 标记去重后发通知。先记下再发：写不进就不发，免得每次取数都弹同一条。几条取数路径（账户页、预热、
/// 后台循环、同步）可能同时到这里，读改写整段串行。
pub fn notice_usage_alerts(app: &AppHandle) {
    static MARKER_GATE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let (Some(cloud), Ok(config_dir)) = (
        app.try_state::<luna_cloud::commands::CloudState>(),
        app.path().app_config_dir(),
    ) else {
        return;
    };
    let Some(episodes) = cloud.0.usage_alerts() else {
        return;
    };
    let Some(owner) = crate::account_scope::current(&cloud.0) else {
        return;
    };
    let config_dir = crate::account_scope::marker_dir(&config_dir, &owner);
    let Ok(_gate) = MARKER_GATE.lock() else {
        return;
    };
    let path = config_dir.join(USAGE_NOTICE_FILE);
    let previous: UsageNoticeMarker = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let (notified, notices) = usage_notice_step(&previous.notified, &episodes);
    let recorded = if notified.is_empty() {
        std::fs::remove_file(&path).is_ok() || !path.exists()
    } else {
        serde_json::to_vec(&UsageNoticeMarker { notified })
            .ok()
            .is_some_and(|bytes| {
                std::fs::create_dir_all(&config_dir).is_ok() && std::fs::write(&path, bytes).is_ok()
            })
    };
    if !recorded {
        return;
    }
    let Some(center) = app.try_state::<crate::notifications::NotificationCenter>() else {
        return;
    };
    for notice in notices {
        center.publish(notice.event(), app);
    }
}

/// 付款处境通知已发到哪一步的落盘位置（此刻账户的提醒标记目录下，[`crate::account_scope::marker_dir`]；
/// 跨重启保持，同一处境只提醒一次）。按账户分：否则 A 催缴过、换成 B 时 B 会收到一条「已恢复」。
pub(crate) const BILLING_NOTICE_FILE: &str = "billing-standing-notice.json";

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct BillingNoticeMarker {
    /// 上一次提醒过的处境（`past_due:<截止>` / `frozen`）；回到正常即删文件。
    notified: String,
}

/// 付款处境变化要发的那一条通知（纵切 3 C3）。
#[derive(Debug, Clone, PartialEq, Eq)]
enum BillingNotice {
    /// 催缴宽限开始（或宽限截止换了）：截止时刻原样（RFC 3339）。
    PastDue {
        grace_ends_at: String,
    },
    Frozen,
    /// 从催缴 / 冻结回到正常。通知中心没有撤回，这一条就是那两条的了结。
    Restored,
}

/// 纯判定：上一次提醒过什么 + 这一次取到的处境 → （要记下的标记，要发的通知）。
///
/// 同一处境（含同一个宽限截止）只提醒一次——权益每次重取（启动预热、打开账户页、付款回来）
/// 都会走到这里，逐次提醒就是刷屏。截止换了（Stripe 重新排了一轮重试）算一次新的催缴。
/// 回到正常：清掉标记，之前提醒过才发一条「已恢复」。
fn billing_notice_step(
    notified: Option<&str>,
    standing: &luna_cloud::billing::BillingStandingDto,
) -> (Option<String>, Option<BillingNotice>) {
    use luna_cloud::billing::BillingStandingDto as S;
    let (marker, notice) = match standing {
        S::Good => {
            return (None, notified.map(|_| BillingNotice::Restored));
        }
        S::PastDue { grace_ends_at } => (
            format!("past_due:{grace_ends_at}"),
            BillingNotice::PastDue {
                grace_ends_at: grace_ends_at.clone(),
            },
        ),
        S::Frozen => ("frozen".to_owned(), BillingNotice::Frozen),
    };
    let fresh = notified != Some(marker.as_str());
    (Some(marker), fresh.then_some(notice))
}

/// 付款处境变了（`AccountChange::BillingStanding`，判定在 `luna_cloud::commands::fetch_entitlements`）
/// → 按落盘的提醒标记去重后发通知。先记下再发：写不进就不发，免得每次重取都弹同一条。
pub fn notice_billing_standing(app: &AppHandle) {
    let (Some(cloud), Ok(config_dir)) = (
        app.try_state::<luna_cloud::commands::CloudState>(),
        app.path().app_config_dir(),
    ) else {
        return;
    };
    let Some(standing) = cloud.0.billing_standing() else {
        return;
    };
    let Some(owner) = crate::account_scope::current(&cloud.0) else {
        return;
    };
    let config_dir = crate::account_scope::marker_dir(&config_dir, &owner);
    let path = config_dir.join(BILLING_NOTICE_FILE);
    let previous: Option<BillingNoticeMarker> = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    let (marker, notice) =
        billing_notice_step(previous.as_ref().map(|m| m.notified.as_str()), &standing);
    let recorded = match marker {
        None => std::fs::remove_file(&path).is_ok() || !path.exists(),
        Some(notified) => serde_json::to_vec(&BillingNoticeMarker { notified })
            .ok()
            .is_some_and(|bytes| {
                std::fs::create_dir_all(&config_dir).is_ok() && std::fs::write(&path, bytes).is_ok()
            }),
    };
    let (Some(notice), true) = (notice, recorded) else {
        return;
    };
    let event = match notice {
        BillingNotice::PastDue { grace_ends_at } => {
            // 日期按本机时区只写到天（与同步只读宽限的提醒同一口径）；认不出就整串不写。
            let date = chrono::DateTime::parse_from_rfc3339(&grace_ends_at)
                .map(|t| {
                    t.with_timezone(&chrono::Local)
                        .format("%Y-%m-%d")
                        .to_string()
                })
                .unwrap_or_default();
            crate::notifications::NotificationEvent::billing_past_due(&date)
        }
        BillingNotice::Frozen => crate::notifications::NotificationEvent::billing_frozen(),
        BillingNotice::Restored => crate::notifications::NotificationEvent::billing_restored(),
    };
    if let Some(center) = app.try_state::<crate::notifications::NotificationCenter>() {
        center.publish(event, app);
    }
}

/// 后台拉一遍全部目录（模型、能力提示词、主题、模板），按新鲜度判要不要真的发请求。
///
/// 用户显式的「检查更新」走 `check_content_updates`（无条件拉、报错进界面，ADR-170 ③）；这里是自发的那条，
/// 严格本地下静默跳过、失败只留状态。
pub fn spawn_catalog_sync(app: &AppHandle) {
    let Some(catalogs) = app.try_state::<luna_cloud::commands::Catalogs>() else {
        return;
    };
    let catalogs = catalogs.inner().clone();
    tauri::async_runtime::spawn(async move {
        catalogs
            .refresh_if_stale(luna_cloud::catalog::AUTO_REFRESH_MAX_AGE_SECS)
            .await;
    });
}

/// 刚登录：无条件拉一遍全部目录（ADR-170 ①）——登录前缓存刚刷过的话，按新鲜度判会把运营在这之间发布的内容
/// 挡在外面（2026-10-23 真机：「获取更多模板」整块不出现）。只在门控放行时做；失败只留状态。
pub fn spawn_catalog_refresh_on_sign_in(app: &AppHandle) {
    if !evaluate_for(app).allowed() {
        return;
    }
    let Some(catalogs) = app.try_state::<luna_cloud::commands::Catalogs>() else {
        return;
    };
    let catalogs = catalogs.inner().clone();
    tauri::async_runtime::spawn(async move {
        let _ = catalogs.refresh().await;
    });
}

/// 云是否可用的门控判据（本地可答，绝不联网）。
#[derive(Clone)]
pub struct CloudGate {
    inner: Arc<dyn Fn() -> CloudAvailability + Send + Sync>,
}

/// 门控结论。区分「为什么不可用」是为了让上层给出**可执行**的提示，
/// 而不是笼统的「云不可用」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudAvailability {
    /// 已登录：允许走云。
    Allowed,
    /// 严格本地模式拦截（用户显式选择的隐私档）。
    BlockedByStrictLocal,
    /// 未登录 —— ADR-037 的默认态，AI 全在本机。
    SignedOut,
}

/// 此刻云客户端指向的那台服务器（账户记住的那一台；调试构建的 env 覆盖已计入）。
///
/// 宿主不自己解析地址：它是云客户端派生出来的事实，由 [`reconfigure_cloud_stack`] 每次换齐
/// 时记一份，端点与落点都从这里取。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerRef {
    pub origin: String,
    /// 内置官方后端（落点为官方云）；否则为组织服务器。
    pub official: bool,
}

impl ServerRef {
    #[must_use]
    pub fn of(api_base: &str) -> Self {
        Self {
            origin: api_base.to_owned(),
            official: api_base == luna_cloud::config::DEFAULT_API_BASE,
        }
    }
}

impl CloudAvailability {
    #[must_use]
    pub fn allowed(self) -> bool {
        matches!(self, Self::Allowed)
    }
}

/// **AI 算力此刻落在哪**——界面的单一事实源。
///
/// # 与 [`CloudAvailability`] 的分工
///
/// 那个回答「**能不能**走云」（门控，用于拦截）；这个回答「**在哪跑**」（状态，用于
/// 展示）。两者共用同一套判据、后者由前者派生——各算一遍必然有一天两处说不一样的话
/// （门控放行而界面说「在本机跑」，或者反过来）。
///
/// # 为什么要有这个类型
///
/// 此前「算力在哪」这件事没有名字：它散在四个设置项里，谁想知道就自己组合一遍
/// （`strict_local` + 登录态 + `api_base` 是否为空 + 转写开关）。于是每个界面各判一次、
/// 各判各的，而用户看到的却是同一件事的不同说法。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum ComputeLocation {
    /// 严格本地：全部在这台设备上跑，任何外发都被硬拦（用户显式选的隐私档）。
    StrictLocal,
    /// 本机：没登录、也没连组织服务端。算力在设备上，但用户随时可以连。
    ///
    /// 这是 ADR-037 的默认态——**不产生任何云网络行为**。
    OnDevice,
    /// 官方云（内置地址，用户不必填）。
    OfficialCloud,
    /// 组织服务端（私有化部署，地址由部署方给）。
    Organization,
}

impl ComputeLocation {
    /// 算力是不是在设备之外。
    #[must_use]
    pub fn is_server(self) -> bool {
        matches!(self, Self::OfficialCloud | Self::Organization)
    }

    /// 稳定 snake_case 名（与 serde 表示一致，不随文案变）。
    ///
    /// 失败类通知带着它过 IPC：一条「批处理中止」的文案必须说清**这次是在哪儿跑的**。
    /// 此前那几条把根因写死成「请检查本机 AI 引擎是否在运行」——而连着组织服务端的用户
    /// 本机根本没跑引擎，那句话把他推向一个与故障毫无关系的方向（ADR-059 ③）。
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::StrictLocal => "strict_local",
            Self::OnDevice => "on_device",
            Self::OfficialCloud => "official_cloud",
            Self::Organization => "organization",
        }
    }
}

/// 对话与翻译的正文此刻发往哪，**以及那个地址是从哪来的**。
///
/// # 为什么地址必须带着来源一起走
///
/// 落点判定曾经只拿到一个字符串，判据是「非空 = 组织服务端」。那在当时是对的：非空端点
/// 只可能来自用户填的地址。而端点解析收口之后，「登录了 Lunarca 云、一个地址都没填」这一
/// 档也会派生出非空端点（内置官方地址）——于是 `OfficialCloud` 当场变成**不可达分支**，
/// 每个订阅用户的首屏都会写「你所在组织自建的服务端」。
///
/// 一个裸字符串答不出「这台是谁的」，所以类型里必须有这一维。同一个值同时携带地址与来源，
/// 两者也就不可能各自演化出不同的答案。
///
/// 非本机只有 Lunarca 服务端这一种（官方云 / 组织服务端），都走原生推理面；第三方兼容
/// 端点已移除（ADR-155）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComputeEndpoint {
    /// 全在本机：严格本地，或未登录（ADR-037 默认态）。
    OnDevice,
    /// 组织服务器（已登录）。
    LunarcaServer(String),
    /// Lunarca 官方云（已登录）。
    OfficialCloud(String),
}

impl ComputeEndpoint {
    /// 要打的那个地址；`OnDevice` 为空串。
    ///
    /// Lunarca 两档给的是**控制面基址**（不带任何路径后缀）——原生面的
    /// `/inference/v1` 由 Provider 自己拼，那是它与服务端之间的契约，不该漏进设置与界面。
    #[must_use]
    pub fn url(&self) -> &str {
        match self {
            Self::OnDevice => "",
            Self::LunarcaServer(url) | Self::OfficialCloud(url) => url,
        }
    }

    /// 是否打向 Lunarca 服务端（原生推理面）。
    ///
    /// **穷尽 `match` 而不是 `matches!`**：新增一种落点时编译器立刻红，逼作者回答
    /// 「它是不是服务端」。紧挨着的 `taken_over_capabilities` 已经是穷尽的，两者必须同一形态。
    #[must_use]
    pub fn is_server(&self) -> bool {
        match self {
            Self::LunarcaServer(_) | Self::OfficialCloud(_) => true,
            Self::OnDevice => false,
        }
    }
}

/// 由门控结论 + **解析出的对话端点**派生出「算力在哪」。
///
/// 判据顺序即优先级：严格本地压倒一切（用户显式选的）；其余看端点——登录了哪台服务器，
/// 算力就在哪；未登录全在本机。
#[must_use]
pub fn compute_location(
    availability: CloudAvailability,
    endpoint: &ComputeEndpoint,
) -> ComputeLocation {
    if matches!(availability, CloudAvailability::BlockedByStrictLocal) {
        return ComputeLocation::StrictLocal;
    }
    match endpoint {
        ComputeEndpoint::LunarcaServer(_) => ComputeLocation::Organization,
        ComputeEndpoint::OfficialCloud(_) => ComputeLocation::OfficialCloud,
        ComputeEndpoint::OnDevice => ComputeLocation::OnDevice,
    }
}

impl CloudGate {
    /// 以任意判据构造（测试注入用）。
    #[must_use]
    pub fn new(f: impl Fn() -> CloudAvailability + Send + Sync + 'static) -> Self {
        Self { inner: Arc::new(f) }
    }

    /// 恒不可用（未登录）—— 无云装配时的默认。
    #[must_use]
    pub fn never() -> Self {
        Self::new(|| CloudAvailability::SignedOut)
    }

    #[must_use]
    pub fn evaluate(&self) -> CloudAvailability {
        (self.inner)()
    }
}

/// 从 app 句柄构造生产门控。
///
/// `handle` 经 `OnceLock` 延迟注入：装配闭包在 `setup` 之前就要构造，而 `AppHandle`
/// 与 `AppState` 到 setup 才存在。**句柄未就绪时按「未登录」处理**（安全默认；
/// 正常时序不可达）。
#[must_use]
pub fn production_gate<R: Runtime>(handle: Arc<OnceLock<AppHandle<R>>>) -> CloudGate {
    CloudGate::new(move || {
        let Some(app) = handle.get() else {
            return CloudAvailability::SignedOut;
        };
        evaluate_for(app)
    })
}

/// 对一个**已就绪**的句柄评估门控。
///
/// 抽出来是因为有两个调用方：[`production_gate`]（句柄经 `OnceLock` 延迟注入）与
/// 状态查询（命令层，句柄现成）。判据只此一份——各写一遍必然有一天门控放行而界面
/// 说「在本机跑」，或者反过来。
#[must_use]
pub fn evaluate_for<R: Runtime>(app: &AppHandle<R>) -> CloudAvailability {
    // 严格本地：读**与应用同一份内存态**的设置。
    //
    // 这里曾是一处跨仓 workaround：官方构建在独立仓时拿不到 `AppState` 的私有
    // 探针，只能重新解析写穿后的配置文件，并因此带着「内存与文件可能短暂分叉」的
    // 已知边界。ADR-037 合并进同一棵树后该 workaround 退役——单一事实源，不再有
    // 分叉窗口。
    let strict_local = app
        .try_state::<crate::ipc::AppState>()
        .is_none_or(|state| state.strict_local());
    // 有会话即可走云。`logged_in` 只读保险箱里 refresh 那一条、不联网——「判断要不要
    // 联网」这一步本身绝不能联网；本闭包每 15 分钟被后台目录同步问一次，而它不看档案。
    availability(strict_local, || {
        app.try_state::<luna_cloud::commands::CloudState>()
            .is_some_and(|state| state.0.logged_in())
    })
}

/// 门控的判据本身：严格本地先判，**判中即返回、不读会话**——严格本地下连保险箱里的云端凭据
/// 都不碰（TDD-12 §2）。
fn availability(strict_local: bool, signed_in: impl FnOnce() -> bool) -> CloudAvailability {
    if strict_local {
        return CloudAvailability::BlockedByStrictLocal;
    }
    if signed_in() {
        CloudAvailability::Allowed
    } else {
        CloudAvailability::SignedOut
    }
}

/// 云感知的工件下载（ADR-037：`ArtifactFetcher` 从「风味接缝」转正为**同一二进制内的
/// 双路径**）。
///
/// - 严格本地 → **直连**之外什么都不发（上层安装命令另有硬闸）；
/// - 其余（已连接，或未连接）→ 走制品面（digest → 有时效镜像集 → 断点续传 → 本地
///   sha256 终验），全部镜像失败再回落直连。
///
/// **未连接也先问后端的镜像**（ADR-145 ⑥ 修订 2026-10-07，用户裁决：模型存在 Lunarca
/// 后端的部署机上，防止上游下载链接失效）。这不破 ADR-037 的零云网络不变量：它只在
/// **下载**这个用户显式动作里发生（下载本身就要联网），镜像解析与字节端点都是公开只读、
/// 不带凭据；启动、后台、面板挂载仍一个字节都不发。
pub struct CloudAwareArtifactFetcher {
    direct: DirectArtifactFetcher,
    cloud: Option<Arc<dyn ArtifactFetcher>>,
    gate: CloudGate,
}

impl CloudAwareArtifactFetcher {
    #[must_use]
    pub fn new(cloud: Option<Arc<dyn ArtifactFetcher>>, gate: CloudGate) -> Self {
        Self {
            direct: DirectArtifactFetcher,
            cloud,
            gate,
        }
    }
}

#[async_trait::async_trait]
impl ArtifactFetcher for CloudAwareArtifactFetcher {
    async fn fetch(
        &self,
        request: ArtifactRequest<'_>,
        on_progress: &ArtifactProgress<'_>,
    ) -> Result<(), String> {
        match (&self.cloud, self.gate.evaluate()) {
            (Some(cloud), CloudAvailability::Allowed | CloudAvailability::SignedOut) => {
                cloud.fetch(request, on_progress).await
            }
            // 严格本地 / 无云实现：直连（严格本地下上层命令早已拦截，这里不再多发一次）。
            _ => self.direct.fetch(request, on_progress).await,
        }
    }
}

#[cfg(test)]
mod tests {
    /// 付款处境通知（纵切 3 C3）：同一处境只提醒一次，截止换了算新的一轮，回到正常清标记并
    /// 只在提醒过时发「已恢复」。
    #[test]
    fn billing_notices_fire_once_per_standing_and_resolve_on_good() {
        use super::{billing_notice_step, BillingNotice};
        use luna_cloud::billing::BillingStandingDto as S;

        let due = |at: &str| S::PastDue {
            grace_ends_at: at.to_owned(),
        };
        let first = "2026-10-28T00:00:00Z";

        // 一直正常：不记、不发。
        assert_eq!(billing_notice_step(None, &S::Good), (None, None));
        // 进入催缴：记下并提醒一次；同一截止再取到不再提醒。
        let (marker, notice) = billing_notice_step(None, &due(first));
        assert_eq!(
            notice,
            Some(BillingNotice::PastDue {
                grace_ends_at: first.to_owned()
            })
        );
        let marker = marker.unwrap();
        assert_eq!(
            billing_notice_step(Some(&marker), &due(first)),
            (Some(marker.clone()), None),
            "同一处境重取（启动预热、打开账户页）不得刷屏"
        );
        // 截止换了：新的一轮催缴，再提醒一次。
        let (_, again) = billing_notice_step(Some(&marker), &due("2026-11-05T00:00:00Z"));
        assert!(matches!(again, Some(BillingNotice::PastDue { .. })));
        // 催缴转冻结：提醒冻结。
        let (frozen, notice) = billing_notice_step(Some(&marker), &S::Frozen);
        assert_eq!(notice, Some(BillingNotice::Frozen));
        assert_eq!(billing_notice_step(frozen.as_deref(), &S::Frozen).1, None);
        // 回到正常：标记清掉，发一条「已恢复」；之后再正常不再发。
        assert_eq!(
            billing_notice_step(frozen.as_deref(), &S::Good),
            (None, Some(BillingNotice::Restored))
        );
        // 清掉之后再进入同一处境，照常提醒（下一次真的又出事了）。
        assert!(billing_notice_step(None, &S::Frozen).1.is_some());
    }

    /// 用量阈值通知（纵切 3 C4，PRD-13-040 ③）：每一轮只在升档时提醒一次、只说升到的最高档；
    /// 同一轮降回再升不重复；新的一轮从头算；同类同档一次取数只发一条；没有文案的账本只记不发。
    #[test]
    fn usage_notices_fire_once_per_round_at_the_highest_new_level() {
        use super::{usage_notice_step, UsageNotice};
        use luna_cloud::client::UsageAlertDto as L;
        use luna_cloud::usage::{AlertSubject, UsageAlertEpisode};
        let ledger = |id: &str, kind: &str, level: L| UsageAlertEpisode {
            id: id.to_owned(),
            subject: AlertSubject::Ledger {
                kind: kind.to_owned(),
            },
            level,
        };
        let cap = |id: &str, level: L| UsageAlertEpisode {
            id: id.to_owned(),
            subject: AlertSubject::SpendCap,
            level,
        };
        let empty = std::collections::BTreeMap::new();

        // 一切正常：不记、不发。
        let (marker, notices) = usage_notice_step(
            &empty,
            &[ledger(
                "subscription:tokens_out@2026-10",
                "subscription",
                L::None,
            )],
        );
        assert!(marker.is_empty() && notices.is_empty());

        // 到 80%：提醒一次；同一轮再取到 80% 不再提醒。
        let round = |level| {
            [ledger(
                "subscription:tokens_out@2026-10",
                "subscription",
                level,
            )]
        };
        let (marker, notices) = usage_notice_step(&empty, &round(L::P80));
        assert_eq!(
            notices,
            [UsageNotice {
                subject: "subscription",
                level: L::P80
            }]
        );
        let (marker, notices) = usage_notice_step(&marker, &round(L::P80));
        assert!(
            notices.is_empty(),
            "同一轮同一档重取（预热、账户页、定时）不得刷屏"
        );
        // 升到 95%：再提醒一次（更高的一档）。
        let (marker, notices) = usage_notice_step(&marker, &round(L::P95));
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].level, L::P95);
        // 降回 80%（升级了档位）再升回 95%：那一档说过了，不重复。
        let (marker, notices) = usage_notice_step(&marker, &round(L::P80));
        assert!(notices.is_empty(), "降档不提醒低一档");
        let (marker, notices) = usage_notice_step(&marker, &round(L::P95));
        assert!(notices.is_empty(), "同一轮已提醒过 95%");
        // 用尽：再提醒一次。
        let (marker, notices) = usage_notice_step(&marker, &round(L::Exhausted));
        assert_eq!(notices[0].level, L::Exhausted);

        // 新的一轮（换周期）：旧标记丢掉，从头算；直接跳到 95% 只说 95%。
        let (next, notices) = usage_notice_step(
            &marker,
            &[ledger(
                "subscription:tokens_out@2026-11",
                "subscription",
                L::P95,
            )],
        );
        assert_eq!(
            notices,
            [UsageNotice {
                subject: "subscription",
                level: L::P95
            }]
        );
        assert!(!next.contains_key("subscription:tokens_out@2026-10"));

        // 同类同档一次只发一条；上限与预付各说各的；认不出的账本只记不发。
        let (marker, notices) = usage_notice_step(
            &empty,
            &[
                ledger("subscription:tokens_in@2026-10", "subscription", L::P80),
                ledger("subscription:tokens_out@2026-10", "subscription", L::P80),
                ledger("prepaid:micro_credits@600000000", "prepaid", L::Exhausted),
                ledger("team:seats@x", "team", L::P95),
                cap("spend_cap@2026-10", L::P95),
            ],
        );
        assert_eq!(
            notices,
            [
                UsageNotice {
                    subject: "subscription",
                    level: L::P80
                },
                UsageNotice {
                    subject: "prepaid",
                    level: L::Exhausted
                },
                UsageNotice {
                    subject: "spend_cap",
                    level: L::P95
                },
            ]
        );
        assert_eq!(marker.get("team:seats@x"), Some(&L::P95));
        // 通知的类与落点：95% 起要用户决定；上限落在上限控件上。
        let event = UsageNotice {
            subject: "spend_cap",
            level: L::Exhausted,
        }
        .event();
        let dto = crate::notifications::NotificationEventDto::from(event);
        assert_eq!(dto.i18n_key, "notifications:usage_spend_cap_exhausted");
        assert_eq!(dto.class, "user_action");
        assert_eq!(dto.target.as_deref(), Some("settings://account/spend_cap"));
        let info = crate::notifications::NotificationEventDto::from(
            UsageNotice {
                subject: "subscription",
                level: L::P80,
            }
            .event(),
        );
        assert_eq!(info.class, "info");
        assert_eq!(info.target.as_deref(), Some("settings://account/usage"));
    }

    /// 用量阈值通知的九个种类在中英两份通知文案里都齐（标题、正文、动作）——种类名是 Rust 拼的，
    /// 缺一条的表现是通知中心落回通用兜底句，而中英 parity 守护看不出两边**同样地**缺。
    #[test]
    fn every_usage_notice_kind_has_copy_in_both_locales() {
        use super::UsageNotice;
        use luna_cloud::client::UsageAlertDto as L;
        for locale in ["zh-CN", "en-US"] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../apps/desktop/src/i18n/locales")
                .join(locale)
                .join("notifications.json");
            let raw = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("读不到 {}：{e}", path.display()));
            let json: serde_json::Value = serde_json::from_str(&raw).expect("通知文案是合法 JSON");
            for subject in ["subscription", "prepaid", "spend_cap"] {
                for level in [L::P80, L::P95, L::Exhausted] {
                    let dto = crate::notifications::NotificationEventDto::from(
                        UsageNotice { subject, level }.event(),
                    );
                    let kind = dto.i18n_key.trim_start_matches("notifications:");
                    for field in ["title", "body", "action"] {
                        assert!(
                            json[kind][field].is_string(),
                            "{locale}/notifications.json 缺 {kind}.{field}"
                        );
                    }
                }
            }
        }
    }

    /// 「算力在哪」的判据顺序即优先级，且必须与门控**同源**。
    ///
    /// 此前这件事没有名字，谁想知道就自己组合一遍（`strict_local` + 登录态 +
    /// `api_base` + 转写开关）。于是同一件事在不同界面上说法不同，而没有任何东西
    /// 会因此变红——用户看到设置页说「在云上跑」、HUD 说「本机」，无从判断信哪个。
    #[test]
    fn where_compute_runs_follows_one_ordered_judgment() {
        use super::{compute_location, CloudAvailability, ComputeEndpoint, ComputeLocation};

        let org = || ComputeEndpoint::LunarcaServer("http://org:8090".to_owned());
        let official = || ComputeEndpoint::OfficialCloud("https://api.example".to_owned());

        // 严格本地压倒一切——它是用户显式选的隐私档，连着服务端也不算数。
        assert_eq!(
            compute_location(CloudAvailability::BlockedByStrictLocal, &org()),
            ComputeLocation::StrictLocal,
            "严格本地下仍报服务端 = 界面在说一件不成立的事"
        );
        // 没登录、也没配端点 = 本机。这是 ADR-037 的默认态，**不产生任何云网络行为**。
        assert_eq!(
            compute_location(CloudAvailability::SignedOut, &ComputeEndpoint::OnDevice),
            ComputeLocation::OnDevice
        );
        // 配了端点但没有 Lunarca 账户 —— 这**就是**私有化形态（ADR-037：私有化不要求账户，
        // 凭据由组织服务端自己发）。此前它被判成 `OnDevice`，于是首屏的隐私陈述在说谎：
        // 卡片写「AI 正在这台设备上运行」，而端点非空 → Provider 按远端构造 → 对话与翻译的
        // 正文正在离开这台设备。方向还是最坏的那个——用户读到"没出去"。
        assert_eq!(
            compute_location(CloudAvailability::SignedOut, &org()),
            ComputeLocation::Organization,
            "配了计算服务端点却报「在本机运行」——正文正在外发，而界面说没有"
        );
        // 登录官方云、没填任何地址 = 官方云。端点**非空**（内置官方地址），故这一档
        // 不能靠「端点空不空」来判——那正是收口端点解析之后 `OfficialCloud` 一度变成
        // 不可达分支的原因：每个订阅用户的首屏都会写「你所在组织自建的服务端」。
        assert_eq!(
            compute_location(CloudAvailability::Allowed, &official()),
            ComputeLocation::OfficialCloud
        );
        // 两者兼有时按**端点来源**判：对话确实打向组织服务端，而不是官方云。
        assert_eq!(
            compute_location(CloudAvailability::Allowed, &org()),
            ComputeLocation::Organization,
            "填了地址就该报组织服务端——那台才是正文去的地方"
        );
        assert!(org().is_server());
        assert!(official().is_server());
        assert!(!ComputeEndpoint::OnDevice.is_server());

        assert!(ComputeLocation::OfficialCloud.is_server());
        assert!(ComputeLocation::Organization.is_server());
        assert!(!ComputeLocation::OnDevice.is_server());
        assert!(
            !ComputeLocation::StrictLocal.is_server(),
            "严格本地算「在服务端」的话，联网指示会亮在一个不外发的形态上"
        );
    }
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingFetcher(Arc<AtomicUsize>);

    #[async_trait::async_trait]
    impl ArtifactFetcher for CountingFetcher {
        async fn fetch(
            &self,
            _request: ArtifactRequest<'_>,
            _on_progress: &ArtifactProgress<'_>,
        ) -> Result<(), String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn request(dest: &std::path::Path) -> ArtifactRequest<'_> {
        static URLS: std::sync::LazyLock<Vec<String>> =
            std::sync::LazyLock::new(|| vec!["https://example.invalid/model.bin".to_owned()]);
        ArtifactRequest {
            urls: &URLS,
            dest,
            expected_size_bytes: 1,
            expected_sha256: "00",
        }
    }

    /// ADR-145 ⑥ 修订：未连接时**下载**也先问后端的镜像（模型存在后端，防上游链接失效）。
    /// 下载是用户显式动作；不变量管的「未配置时不自发联网」由启动 / 后台那几条守。
    #[tokio::test]
    async fn not_configured_download_asks_the_backend_mirror_first() {
        let calls = Arc::new(AtomicUsize::new(0));
        let fetcher = CloudAwareArtifactFetcher::new(
            Some(Arc::new(CountingFetcher(Arc::clone(&calls)))),
            CloudGate::new(|| CloudAvailability::SignedOut),
        );
        let dir = std::env::temp_dir().join("luna-cloud-gate-test");
        let _ = std::fs::create_dir_all(&dir);
        fetcher
            .fetch(request(&dir.join("a.part")), &|_| {})
            .await
            .expect("未连接的下载应走制品面（内部全败再回落直连）");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    /// 严格本地同样不得走云（两条门各自独立生效）。
    #[tokio::test]
    async fn strict_local_never_touches_the_cloud_path() {
        let calls = Arc::new(AtomicUsize::new(0));
        let fetcher = CloudAwareArtifactFetcher::new(
            Some(Arc::new(CountingFetcher(Arc::clone(&calls)))),
            CloudGate::new(|| CloudAvailability::BlockedByStrictLocal),
        );
        let dir = std::env::temp_dir().join("luna-cloud-gate-test");
        let _ = std::fs::create_dir_all(&dir);
        let _ = fetcher.fetch(request(&dir.join("b.part")), &|_| {}).await;
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn allowed_uses_the_cloud_path() {
        let calls = Arc::new(AtomicUsize::new(0));
        let fetcher = CloudAwareArtifactFetcher::new(
            Some(Arc::new(CountingFetcher(Arc::clone(&calls)))),
            CloudGate::new(|| CloudAvailability::Allowed),
        );
        let dir = std::env::temp_dir().join("luna-cloud-gate-test");
        let _ = std::fs::create_dir_all(&dir);
        fetcher
            .fetch(request(&dir.join("c.part")), &|_| {})
            .await
            .expect("门控允许时应走云路径");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    /// 严格本地下门控不读会话：保险箱里的云端凭据一条都不解。
    #[test]
    fn strict_local_gate_never_reads_cloud_credentials() {
        assert_eq!(
            availability(true, || panic!("严格本地下不得读会话")),
            CloudAvailability::BlockedByStrictLocal
        );
        assert_eq!(availability(false, || true), CloudAvailability::Allowed);
        assert_eq!(availability(false, || false), CloudAvailability::SignedOut);
    }

    #[test]
    fn never_gate_reports_not_configured() {
        assert_eq!(CloudGate::never().evaluate(), CloudAvailability::SignedOut);
        assert!(!CloudAvailability::SignedOut.allowed());
        assert!(!CloudAvailability::BlockedByStrictLocal.allowed());
        assert!(CloudAvailability::Allowed.allowed());
    }
}
