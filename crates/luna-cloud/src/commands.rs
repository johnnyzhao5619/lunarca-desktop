//! tauri 命令层：薄封装 [`CloudClient`]（业务在 client.rs；此处只做参数装配与状态取用）。
//!
//! 命令经 tauri-specta 与 upstream 同思路生成 typed 绑定（TDD-14 §3.2 ①(b)：
//! 不另起 IPC 通道），前端只 invoke + 渲染（ADR-003）。

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::account::{
    AccountChange, AccountStateDto, CodeKind, CodeSignInDto, PasswordSignInDto, SignInStepDto,
    SignInTarget,
};
use crate::account_data::{
    AccountDeleteDto, AccountDeletionPreviewDto, AccountExportDto, ExportDestinationPicker,
};
use crate::billing::BillingOfferDto;
use crate::browser;
use crate::catalog::{CatalogClient, CatalogStatusDto};
use crate::client::{
    CloudClient, DeviceDto, EntitlementsDto, LoginPollStatus, LogoutDto, WebSessionDto,
};
use crate::device_limit::DeviceLimitResolutionDto;
use crate::error::CloudError;
use crate::trust::TrustStatusDto;
use crate::usage::{SpendCapDto, UsageDto};

/// 插件托管状态。
pub struct CloudState(pub Arc<CloudClient>);

/// 保存对话框（装配方注入，见 [`crate::init`]）：账户数据导出用。
pub struct ExportPickerState(pub ExportDestinationPicker);

/// 这台服务端的全部签名目录（托管状态；ADR-017：一套基础设施、三份目录）。
///
/// **成组的动作只在这里写一次**：换服务端、换钥重钉、按受众重选、按龄刷新、用户的「刷新目录」。
/// 此前两份目录在五处被逐个点名，加第三份时漏掉任何一处，表现都是上一台服务端（或旧身份）的
/// 内容静默继续生效，而没有任何东西会报错。
///
/// - `models`：本地模型目录（M3 / ADR-145），条目由 `luna-model-catalog` 宽松解析；
/// - `prompts`：能力提示词目录（ADR-050）。模板正文是**运营侧产品配置**，但「此刻生效的是哪一版、
///   有没有因清单版本落差而回落随包」是用户能核对的事实（ADR-050 修订 2026-09-04），故有只读的
///   [`prompt_catalog_status`]；
/// - `themes`：主题目录（ADR-159 ③，纵切 7a）。宿主在用户打开「获取更多」时经 [`CatalogClient::current`]
///   取条目、按 `luna-content-schema` 逐条解析；观察者只通知宿主「交付变了，重读」（封禁要在运行中生效）；
/// - `templates`：模板目录（ADR-159 ③ ⑥，纵切 7b），同主题目录的取法与观察者。
///
/// 字段都是共享句柄，克隆一份给后台循环与宿主用，指向的是同一组目录客户端。
#[derive(Clone)]
pub struct Catalogs {
    pub models: Arc<CatalogClient<serde_json::Value>>,
    pub prompts: Arc<CatalogClient<luna_core::CapabilityPromptSet>>,
    pub themes: Arc<CatalogClient<serde_json::Value>>,
    pub templates: Arc<CatalogClient<serde_json::Value>>,
}

impl Catalogs {
    /// 换了一台服务端（切目标 + 切分桶），见 [`CatalogClient::reconfigure`]。
    pub fn reconfigure(&self, api_base: &str, pubkey: Option<ed25519_dalek::VerifyingKey>) {
        self.models.reconfigure(api_base.to_owned(), pubkey);
        self.prompts.reconfigure(api_base.to_owned(), pubkey);
        self.themes.reconfigure(api_base.to_owned(), pubkey);
        self.templates.reconfigure(api_base.to_owned(), pubkey);
    }

    /// 同一台服务端换了身份（本桶就地清零），见 [`CatalogClient::reset_for_repin`]。
    pub fn reset_for_repin(&self) {
        self.models.reset_for_repin();
        self.prompts.reset_for_repin();
        self.themes.reset_for_repin();
        self.templates.reset_for_repin();
    }

    /// 受众变了（界面语言），对缓存重选一次变体，见 [`CatalogClient::reselect`]。
    pub fn reselect(&self) {
        self.models.reselect();
        self.prompts.reselect();
        self.themes.reselect();
        self.templates.reselect();
    }

    /// 自发的那条：超龄才真的拉，失败只留状态。
    pub async fn refresh_if_stale(&self, max_age_secs: i64) {
        let _ = self.models.refresh_if_stale(max_age_secs).await;
        let _ = self.prompts.refresh_if_stale(max_age_secs).await;
        let _ = self.themes.refresh_if_stale(max_age_secs).await;
        let _ = self.templates.refresh_if_stale(max_age_secs).await;
    }

    /// 无条件拉全部，返回**模型目录**的状态（调用方再按自己关心的那几份各自判）。
    ///
    /// # Errors
    /// 严格本地模式 → [`CloudError::StrictLocalBlocked`]（全部在发请求前被拦下）。
    pub async fn refresh(&self) -> Result<CatalogStatusDto, CloudError> {
        let models = self.models.refresh().await;
        // 另两份没有界面事实要随这个按钮报，结果如实忽略，但**不跳过它们**——跳过就等于永远刷不到。
        let _ = self.prompts.refresh().await;
        let _ = self.themes.refresh().await;
        let _ = self.templates.refresh().await;
        models
    }

    /// 用户的「检查更新」（「应用」页头，ADR-170 ③）：无条件拉全部；**任一份验签失败时顺带核对一次信任锚**
    /// （ADR-060 开头点名的那条指错方向的报错）。返回 `true` = 这台服务端换了身份。
    ///
    /// 「目录验签失败」与「这台服务端换了身份」在客户端看到的现象**完全一样**，而后者才是真正发生的事——用户
    /// 拿着一句 `bad_signature` 去查网络、查服务端，唯独想不到该去核对公钥。这一次额外的取钥只发生在**已经失败**
    /// 的路径上，且只由用户显式动作触发，不扩大 ADR-037 的不变量。核对不上（连不上 / 端点没这条路）时如实答
    /// 「没换」：那时我们并不知道钥变没变，编一个更具体的原因只是把一句诚实的错换成一句好听的错。
    /// 哪些失败算「验签失败」见 [`rejected_signature`]。
    ///
    /// # Errors
    /// 同 [`Self::refresh`]。
    pub async fn refresh_checking_trust(&self, cloud: &CloudClient) -> Result<bool, CloudError> {
        self.refresh().await?;
        let rejected_signature = [
            &self.models.status(),
            &self.prompts.status(),
            &self.themes.status(),
            &self.templates.status(),
        ]
        .iter()
        .any(|status| rejected_signature(status.last_error.as_deref()));
        if !rejected_signature {
            return Ok(false);
        }
        Ok(matches!(
            cloud.check_trust_anchor().await,
            Err(CloudError::TrustAnchorChanged)
        ))
    }
}

/// 能力提示词目录状态：目录客户端的状态机 + 注册表此刻的事实（零网络）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct PromptCatalogStatusDto {
    /// 目录客户端状态（验签 / 过期 / 防回滚 / 上次刷新）。
    pub catalog: CatalogStatusDto,
    /// 当前生效的提示词版本；`0` = 随包默认。
    pub prompt_version: u32,
    /// 已验签覆盖齐备到的 manifest 版本；`None` = 没有覆盖在场。
    pub manifest_version: Option<u32>,
    /// 本客户端要求的 manifest 版本。
    pub client_manifest_version: u32,
    /// 覆盖里缺、正从随包取的必备键（`scope.name`）。
    pub missing: Vec<String>,
}

/// 账户变化观察者（装配方注入，见 [`crate::AccountObserver`]）。
///
/// **每一次账户变化都通知一次，一个都不能漏**：选定服务器、登录成功、退出、注销、开始 /
/// 结束等浏览器。漏掉哪一个，界面与「算力落点」就会停在旧答案上——界面写着新的、字节走着旧的。
///
/// 用 `State` 参数取而不是 `AppHandle`：`State` 不进 tauri-specta 的 TS 签名。插件 `setup`
/// 里恒 manage，故命令可达时它必定在场。
pub struct AccountObserverState(pub crate::AccountObserver);

impl AccountObserverState {
    fn notify(&self, change: AccountChange) {
        (self.0)(change);
    }
}

/// 这台设备在服务端设备列表里的名字与平台（设备码与应用内登录同一口径）。
fn this_device() -> (String, String) {
    let device_name = gethostname::gethostname()
        .to_string_lossy()
        .trim()
        .to_owned();
    let device_name = if device_name.is_empty() {
        "Lunarca".to_owned()
    } else {
        device_name
    };
    (device_name, std::env::consts::OS.to_owned())
}

/// 应用内登录成功是一次会话变化；撞上设备上限是一次展示变化（账户状态转到「选一台移除」，纵切 3 C5）；
/// 要先确认身份时服务器已选定、两钥待确认，同样按会话变化通知（目录与界面跟着换）。
fn notify_if_signed_in(observer: &AccountObserverState, outcome: &PasswordSignInDto) {
    match outcome {
        PasswordSignInDto::SignedIn | PasswordSignInDto::NeedsTrustConfirmation { .. } => {
            observer.notify(AccountChange::Session);
        }
        PasswordSignInDto::DeviceLimitReached => observer.notify(AccountChange::Display),
    }
}

/// 账户全量状态（**零网络**）。
#[tauri::command]
#[specta::specta]
pub async fn account_state(state: State<'_, CloudState>) -> Result<AccountStateDto, CloudError> {
    Ok(state.0.account_state())
}

/// 开始登录（ADR-158 ①）：第一步只问邮箱；按邮箱找服务器（或用用户给的地址 / 链接），返回下一步。
#[tauri::command]
#[specta::specta]
pub async fn account_sign_in_begin(
    state: State<'_, CloudState>,
    observer: State<'_, AccountObserverState>,
    target: SignInTarget,
) -> Result<SignInStepDto, CloudError> {
    let step = state.0.sign_in_begin(target).await;
    // 选定服务器（或丢掉送达的链接）即账户变化：目录与界面都要跟着换。失败时也可能已经
    // 钉住了信任锚，重读一次是幂等的。
    observer.notify(AccountChange::Session);
    step
}

/// 口令登录（ADR-158：OPAQUE，口令不离开设备；邮箱与服务器在上一步已选定）。
#[tauri::command]
#[specta::specta]
pub async fn account_sign_in_password(
    state: State<'_, CloudState>,
    observer: State<'_, AccountObserverState>,
    password: String,
) -> Result<PasswordSignInDto, CloudError> {
    let (device_name, platform) = this_device();
    let outcome = state
        .0
        .sign_in_password(&password, &device_name, &platform)
        .await?;
    notify_if_signed_in(&observer, &outcome);
    Ok(outcome)
}

/// 一次性码登录（重设 / 邀请 / 激活 / 注册验证，ADR-158 ⑥）：码对了即钉住信任，下一步设口令。
/// `code = None` → 用连接链接带来的邀请码（码留在 Rust 里，不过 IPC）。
#[tauri::command]
#[specta::specta]
pub async fn account_sign_in_code(
    state: State<'_, CloudState>,
    observer: State<'_, AccountObserverState>,
    code: Option<String>,
) -> Result<CodeSignInDto, CloudError> {
    let (device_name, platform) = this_device();
    let outcome = state
        .0
        .sign_in_code(code.as_deref(), &device_name, &platform)
        .await;
    // 握手成功即钉住了这台的身份、选定了服务器：界面跟着换。
    if outcome.is_ok() {
        observer.notify(AccountChange::Session);
    }
    outcome
}

/// 凭一次性码换来的授权设口令，随即登录（ADR-158 ⑤⑥）。
#[tauri::command]
#[specta::specta]
pub async fn account_sign_in_set_password(
    state: State<'_, CloudState>,
    observer: State<'_, AccountObserverState>,
    new_password: String,
) -> Result<PasswordSignInDto, CloudError> {
    let (device_name, platform) = this_device();
    let outcome = state
        .0
        .sign_in_set_password(&new_password, &device_name, &platform)
        .await?;
    notify_if_signed_in(&observer, &outcome);
    Ok(outcome)
}

/// 请这次登录选定的服务器往邮箱发一次性码（忘记口令 / 创建账户）。
#[tauri::command]
#[specta::specta]
pub async fn account_sign_in_request_code(
    state: State<'_, CloudState>,
    kind: CodeKind,
) -> Result<(), CloudError> {
    state.0.sign_in_request_code(kind).await
}

/// 登录状态下改口令（ADR-158 ⑤）：这台设备保持登录，其余设备退出。
#[tauri::command]
#[specta::specta]
pub async fn account_change_password(
    state: State<'_, CloudState>,
    new_password: String,
) -> Result<(), CloudError> {
    let (device_name, platform) = this_device();
    state
        .0
        .change_password(&new_password, &device_name, &platform)
        .await
}

/// 用浏览器登录（设备码）：发起配对、打开浏览器，**轮询在 Rust 里跑**，结果经账户状态推给界面。
///
/// `provider` = capabilities `auth.login.providers` 里某一项的 id（「用 Google 继续」、「用 <组织> 登录」）：
/// 浏览器直接去那一家，不经批准页的选择（ADR-171 ④）；`None` = 打开批准页。
///
/// 打开浏览器失败不阻断：配对码与地址仍在界面上，用户可手动打开。
#[tauri::command]
#[specta::specta]
pub async fn account_sign_in_browser(
    state: State<'_, CloudState>,
    observer: State<'_, AccountObserverState>,
    provider: Option<String>,
) -> Result<(), CloudError> {
    let (device_name, platform) = this_device();
    let started = state.0.login_start(device_name, platform).await?;
    let open = match provider {
        Some(id) => state.0.route_pending_to_provider(&id)?,
        None => started.verification_uri_complete,
    };
    let _ = browser::open_url(&open);
    observer.notify(AccountChange::Display);
    let client = Arc::clone(&state.0);
    let observer = Arc::clone(&observer.0);
    tauri::async_runtime::spawn(poll_browser_login(client, observer, started.next_poll_ms));
    Ok(())
}

/// 设备码轮询循环：等到批准、过期、取消或出错为止。
///
/// 批准 → 会话变化；过期 / 出错 → 回到未登录（配对已丢）；取消 → 配对已被别处丢掉，静默退出。
async fn poll_browser_login(
    client: Arc<CloudClient>,
    observer: crate::AccountObserver,
    first_wait_ms: u32,
) {
    let mut wait_ms = first_wait_ms;
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(u64::from(wait_ms))).await;
        match client.login_poll().await {
            Ok(poll) => match poll.status {
                LoginPollStatus::Pending | LoginPollStatus::SlowDown => {
                    wait_ms = poll.next_poll_ms.unwrap_or(wait_ms);
                }
                LoginPollStatus::Authorized => {
                    observer(AccountChange::Session);
                    return;
                }
                LoginPollStatus::Expired => {
                    observer(AccountChange::Display);
                    return;
                }
                // 设备满了（纵切 3 C5）：配对留着、停轮询；用户移除一台后由
                // `account_sign_in_resolve_device_limit` 接着轮询。
                LoginPollStatus::DeviceLimitReached => {
                    observer(AccountChange::Display);
                    return;
                }
            },
            Err(CloudError::NoPendingLogin) => return,
            Err(err) => {
                eprintln!("[account] 浏览器登录轮询失败，已放弃这次配对：{err}");
                client.login_cancel();
                observer(AccountChange::Display);
                return;
            }
        }
    }
}

/// 重新打开浏览器批准页。
#[tauri::command]
#[specta::specta]
pub async fn account_sign_in_reopen_browser(
    state: State<'_, CloudState>,
) -> Result<(), CloudError> {
    let uri = state.0.pending_verification_uri()?;
    browser::open_url(&uri)
}

/// 取消登录：丢掉进行中的浏览器配对与送达的链接。
#[tauri::command]
#[specta::specta]
pub async fn account_sign_in_cancel(
    state: State<'_, CloudState>,
    observer: State<'_, AccountObserverState>,
) -> Result<(), CloudError> {
    state.0.sign_in_cancel();
    observer.notify(AccountChange::Display);
    Ok(())
}

/// 登录撞上设备上限之后「移除这台并登录」（纵切 3 C5）：Rust 一条命令做完撤销与重发，界面只渲染。
///
/// 应用内登录原样重发挂着的那条请求（领取凭据只在 Rust 内存里留到这一步）；设备码接着轮询同一个配对——还在
/// 等浏览器时在这里重新起轮询。令牌失效 → `DeviceLimitExpired`，挂着的选择已清，界面回到登录。
#[tauri::command]
#[specta::specta]
pub async fn account_sign_in_resolve_device_limit(
    state: State<'_, CloudState>,
    observer: State<'_, AccountObserverState>,
    device_id: String,
) -> Result<DeviceLimitResolutionDto, CloudError> {
    let result = state.0.sign_in_resolve_device_limit(&device_id).await;
    match &result {
        Ok(DeviceLimitResolutionDto::SignedIn) => observer.notify(AccountChange::Session),
        Ok(DeviceLimitResolutionDto::AwaitingBrowser) => {
            observer.notify(AccountChange::Display);
            let client = Arc::clone(&state.0);
            let observer = Arc::clone(&observer.0);
            // 刚轮询过一次：按这个配对现行的间隔再等一轮，免得第一下就撞上 `slow_down`。
            let wait_ms = state.0.pending_poll_ms().unwrap_or(5_000);
            tauri::async_runtime::spawn(poll_browser_login(client, observer, wait_ms));
        }
        // 再撞上限 / 失败：选择或配对的去留已在 client 里定好，界面重读一次即可。
        Ok(DeviceLimitResolutionDto::DeviceLimitReached) | Err(_) => {
            observer.notify(AccountChange::Display);
        }
    }
    result
}

/// 退出登录（服务端吊销尽力而为，本地凭据必清）。
///
/// 无论服务端吊销成不成功都要通知：本地凭据**已经**清了，落点当场就变了。
#[tauri::command]
#[specta::specta]
pub async fn account_sign_out(
    state: State<'_, CloudState>,
    observer: State<'_, AccountObserverState>,
) -> Result<LogoutDto, CloudError> {
    let result = state.0.sign_out().await;
    observer.notify(AccountChange::Session);
    result
}

/// 设备列表（含 current 标记）。
#[tauri::command]
#[specta::specta]
pub async fn devices_list(state: State<'_, CloudState>) -> Result<Vec<DeviceDto>, CloudError> {
    state.0.devices().await
}

/// 撤销指定设备（诚实边界：只拒新凭据使用，已在对端明文落地的数据无法远程擦除）。
#[tauri::command]
#[specta::specta]
pub async fn device_revoke(
    state: State<'_, CloudState>,
    device_id: String,
) -> Result<(), CloudError> {
    state.0.revoke_device(&device_id).await
}

/// 读取官方云账户的网页访问开关。
#[tauri::command]
#[specta::specta]
pub async fn account_web_access(state: State<'_, CloudState>) -> Result<bool, CloudError> {
    state.0.web_access(None).await
}

/// 列出官方云账户在用的网页会话。
#[tauri::command]
#[specta::specta]
pub async fn account_web_sessions(
    state: State<'_, CloudState>,
) -> Result<Vec<WebSessionDto>, CloudError> {
    state.0.web_sessions().await
}

/// 让一个浏览器退出 Lunarca。
#[tauri::command]
#[specta::specta]
pub async fn account_revoke_web_session(
    state: State<'_, CloudState>,
    id: String,
) -> Result<(), CloudError> {
    state.0.revoke_web_sessions(Some(&id)).await
}

/// 让所有浏览器退出 Lunarca。
#[tauri::command]
#[specta::specta]
pub async fn account_revoke_all_web_sessions(
    state: State<'_, CloudState>,
) -> Result<(), CloudError> {
    state.0.revoke_web_sessions(None).await
}

/// 更新网页访问开关。关闭会吊销该账户的全部网页会话。
#[tauri::command]
#[specta::specta]
pub async fn account_set_web_access(
    state: State<'_, CloudState>,
    enabled: bool,
) -> Result<bool, CloudError> {
    state.0.web_access(Some(enabled)).await
}

/// 注销前的后果预览（在扣费的订阅、会作废的预付余额、计费记录保留几年）。只读；取不到时界面照常
/// 允许注销，只说明细节没取到。
#[tauri::command]
#[specta::specta]
pub async fn account_deletion_preview(
    state: State<'_, CloudState>,
) -> Result<AccountDeletionPreviewDto, CloudError> {
    state.0.deletion_preview().await
}

/// GDPR 注销（两步确认在前端；本地数据不受影响）。
#[tauri::command]
#[specta::specta]
pub async fn account_delete(
    state: State<'_, CloudState>,
    observer: State<'_, AccountObserverState>,
) -> Result<AccountDeleteDto, CloudError> {
    let result = state.0.delete_account().await;
    // 注销成功即无会话可言；失败时会话还在，但本地凭据是否已被清空由 client 决定——
    // 无条件通知即可，重算一次落点是幂等的，而漏通知会留下一个说谎的界面。
    observer.notify(AccountChange::Session);
    result
}

/// 导出我的账户数据（访问权 / 可携带权）：Rust 弹保存对话框，取消即止、一个字节都不发；选好之后把
/// 服务端的导出流式写进那个位置，返回写好的路径。冻结的账户照样能导出（服务端放行）。
#[tauri::command]
#[specta::specta]
pub async fn account_export(
    state: State<'_, CloudState>,
    picker: State<'_, ExportPickerState>,
) -> Result<AccountExportDto, CloudError> {
    let today = chrono::Local::now().date_naive();
    crate::account_data::export_with_picker(&state.0, &picker.0, today).await
}

/// Entitlement + Lease（本地验签/验过期结果一并下发）。
#[tauri::command]
#[specta::specta]
pub async fn entitlements(
    state: State<'_, CloudState>,
    observer: State<'_, AccountObserverState>,
) -> Result<EntitlementsDto, CloudError> {
    fetch_entitlements(&state.0, &observer).await
}

/// 取一次权益；账户可用的 AI 能力或这台部署配好的能力变了就通知装配层重装（路由跟着换，
/// 统一登录 R4b、ADR-041 ①），付款处境变了就通知装配层发 / 收催缴与冻结通知（纵切 3 C3）。
///
/// 账户页的取数与装配层登录后的预热共用这一份——两处各判一次「变没变」，必然有一处漏通知。
/// 部署事实与权益同一时刻重取：运营登记了上游，用户下次启动或打开账户页就接上，不必重新登录。
///
/// # Errors
/// 同 [`CloudClient::entitlements`]。部署事实取不到不算失败：留着上一次的（或「还不知道」=
/// 按服务端跑得了路由），权益照常返回。
pub async fn fetch_entitlements(
    client: &CloudClient,
    observer: &AccountObserverState,
) -> Result<EntitlementsDto, CloudError> {
    let before = client.allowed_capabilities();
    let deployment_before = client.deployment_facts();
    let standing_before = client.billing_standing();
    let dto = client.entitlements().await?;
    let _ = client.refresh_deployment_facts().await;
    if client.allowed_capabilities() != before || client.deployment_facts() != deployment_before {
        observer.notify(AccountChange::Session);
    }
    if client.billing_standing() != standing_before {
        observer.notify(AccountChange::BillingStanding);
    }
    Ok(dto)
}

/// 当前账户的账本列表与本月预付上限（PRD-13-038 / PRD-13-040 ④）。需要会话——无凭据时本地短路，
/// 不发请求。
#[tauri::command]
#[specta::specta]
pub async fn usage(
    state: State<'_, CloudState>,
    observer: State<'_, AccountObserverState>,
) -> Result<UsageDto, CloudError> {
    fetch_usage(&state.0, &observer).await
}

/// 取一次用量；这一次认出的预警轮次与上一次不同就通知装配层（[`AccountChange::UsageAlerts`]），由它
/// 按落盘标记去重后发 80% / 95% / 用尽 / 上限通知（PRD-13-040 ③，纵切 3 C4）。
///
/// **`/v1/usage` 只经这一处取**：账户页、登录后的预热、后台定时刷新、同步的容量读取、蒸馏的余量
/// 预检都调它——各取各的，就必有一处取到了 95% 却没人提醒（同 [`fetch_entitlements`] 的理由）。
///
/// # Errors
/// 同 [`CloudClient::usage`]。
pub async fn fetch_usage(
    client: &CloudClient,
    observer: &AccountObserverState,
) -> Result<UsageDto, CloudError> {
    let before = client.usage_alerts();
    let dto = client.usage().await?;
    if client.usage_alerts() != before {
        observer.notify(AccountChange::UsageAlerts);
    }
    Ok(dto)
}

/// 设 / 改 / 清本月预付上限（PRD-13-040 ④，纵切 3 C4）。`limit_credits = None` = 清除。credit →
/// 微 credit 的换算与校验在 Rust（非法值发请求前即 `invalid_spend_cap`）。上限只管预付 credits，
/// 订阅包含的额度从不受它影响。需要会话——无凭据时本地短路。界面设完重取一次用量（阈值档位随之重算）。
#[tauri::command]
#[specta::specta]
pub async fn spend_cap_set(
    state: State<'_, CloudState>,
    limit_credits: Option<f64>,
) -> Result<SpendCapDto, CloudError> {
    let micro = limit_credits
        .map(crate::billing::credits_to_micro)
        .transpose()?;
    state.0.set_spend_cap(micro).await
}

/// 这台服务器的收单状态 + 在售价目与账单口径（账户页「升级与购买」一节，纵切 3 C2）。
///
/// 未启用收单是一个**答案**（[`BillingOfferDto::Disabled`]），不是错误：界面据此不出现任何
/// 升级 / 购买 / 管理入口。需要会话——无凭据时本地短路，不发请求。
#[tauri::command]
#[specta::specta]
pub async fn billing_offer(
    state: State<'_, CloudState>,
    observer: State<'_, AccountObserverState>,
) -> Result<BillingOfferDto, CloudError> {
    let client = &state.0;
    // 权益（档位与付款处境）经 `fetch_entitlements` 取：它是「可用 AI 能力 / 付款处境变没变」的
    // 唯一判定处（见 `CloudClient::billing_offer`）。取不到不让整节失败。
    client
        .billing_offer(async { fetch_entitlements(client, &observer).await.ok() })
        .await
}

/// 当前服务器卖不卖（失败卡片决定「升级或购买」还是「打开账户设置」）。记得就零网络；
/// 不记得才取一次 capabilities。需要会话——无凭据时本地短路。
#[tauri::command]
#[specta::specta]
pub async fn billing_available(state: State<'_, CloudState>) -> Result<bool, CloudError> {
    state.0.billing_enabled().await
}

/// 为价目里的一项结账：Rust 建 Stripe 托管付款页并交给系统浏览器（地址不过 IPC、不进日志）。
/// 付款结果由服务端 webhook 落账，用户回到应用时账户页重取即见（PRD-13-017）。
#[tauri::command]
#[specta::specta]
pub async fn billing_checkout(
    state: State<'_, CloudState>,
    plan_id: String,
) -> Result<(), CloudError> {
    let url = state.0.billing_checkout_url(&plan_id).await?;
    browser::open_url(&url)
}

/// 打开 Stripe 托管账单管理页（改付款方式 / 改档 / 退订，取消默认期末生效，PRD-13-017）。
#[tauri::command]
#[specta::specta]
pub async fn billing_portal(state: State<'_, CloudState>) -> Result<(), CloudError> {
    let url = state.0.billing_portal_url().await?;
    browser::open_url(&url)
}

/// 零扣费的付款方式验证（免费额度因风险信号被扣下时，纵切 3 C5 / PRD-13-018 L2）：Rust 取验证页地址
/// 交给系统浏览器（地址不过 IPC、不进日志，同结账）。已不需要验证 → `verification_not_required`，
/// 界面据此重取权益。
#[tauri::command]
#[specta::specta]
pub async fn billing_verify_card(state: State<'_, CloudState>) -> Result<(), CloudError> {
    let url = state.0.billing_verify_card_url().await?;
    browser::open_url(&url)
}

/// 目录状态快照（版本 / 上次刷新 / 失败原因 / 严格本地标记，PRD-13-031）。零网络。
#[tauri::command]
#[specta::specta]
pub async fn catalog_status(catalogs: State<'_, Catalogs>) -> Result<CatalogStatusDto, CloudError> {
    Ok(catalogs.models.status())
}

/// 能力提示词目录状态（设置页「能力提示词」行，ADR-050 修订 2026-09-04）。**零网络**：
/// 只读目录客户端的本地状态机与进程级注册表。
#[tauri::command]
#[specta::specta]
pub async fn prompt_catalog_status(
    catalogs: State<'_, Catalogs>,
) -> Result<PromptCatalogStatusDto, CloudError> {
    let registry = luna_core::prompt::registry().status();
    Ok(PromptCatalogStatusDto {
        catalog: catalogs.prompts.status(),
        prompt_version: registry.version,
        manifest_version: registry.manifest_version,
        client_manifest_version: registry.client_manifest_version,
        missing: registry.missing,
    })
}

/// 信任锚状态（**零网络**：只读本地钉住状态）。
///
/// 判定（未钉住 / 已钉住 / 变更阻断 / env 旁路 / 官方云）全在 Rust 侧算好（ADR-003），
/// 前端只渲染对应的那一块并把两组完整 hex 摆出来供用户逐字核对。
#[tauri::command]
#[specta::specta]
pub async fn trust_status(state: State<'_, CloudState>) -> Result<TrustStatusDto, CloudError> {
    Ok(state.0.trust_status())
}

/// 确认一次信任锚变更（用户已核对两组完整公钥）。重钉时全部目录的防回滚水位与缓存一并
/// 作废、该桶会话清掉（[`CloudClient::adopt_anchor`]）。
#[tauri::command]
#[specta::specta]
pub async fn trust_confirm_change(
    state: State<'_, CloudState>,
    catalogs: State<'_, Catalogs>,
    observer: State<'_, AccountObserverState>,
) -> Result<TrustStatusDto, CloudError> {
    let (status, is_repin) = state.0.confirm_trust_change()?;
    if is_repin {
        catalogs.reset_for_repin();
    }
    observer.notify(AccountChange::Session);
    Ok(status)
}

/// 目录被拒的原因码里，哪些可能意味着「这台服务端换了身份」。
///
/// 密钥分层（ADR-169 E2）之后，钉住的那把是**根密钥**：服务端换了根，先表现为密钥清单的根签名不对
/// （`bad_keys`），轮不到目录本身的 `bad_signature`。只认后者就永远查不到——2026-10-23 真机：09-13 钉的锚，
/// 模型与提示词目录一直写「被拒：签名凭据无法验证」，却从没把人引到核对服务器身份那一步。
fn rejected_signature(last_error: Option<&str>) -> bool {
    matches!(last_error, Some("bad_signature" | "bad_keys"))
}

#[cfg(test)]
mod surface_tests {
    //! **云命令面的联网姿态清单**（ADR-037 替代护栏的机器强制半边）。
    //!
    //! ADR-037 把「OSS 构建零云符号」的构建期断言换成了运行期不变量「未登录时零云网络
    //! 行为」。运行期不变量的风险在于**新增一个命令就可能悄悄破坏
    //! 它**——散文写得再清楚也拦不住。故本测试把命令面钉死：任何人新增/删除云命令，
    //! 本用例立刻变红，逼他在这张表里回答「未配置时这条命令会不会联网」。
    //!
    //! 姿态只有三种，多一种都要先想清楚：
    //! - `LocalOnly`：不碰网络（读钥匙链 / 读本地状态 / 起浏览器）；
    //! - `RequiresSession`：`client::authed` 在无 refresh token 时**先本地短路**返回
    //!   `NotLoggedIn`，未登录状态下一个字节都不会发出；
    //! - `ExplicitUserAction`：未登录也能联网，**但只在用户显式点击后**——这是不变量
    //!   的两处有意例外，UI 上均为明示动作（「登录」「刷新目录」），绝非后台自发。

    #[derive(Debug, PartialEq, Eq)]
    enum Reach {
        LocalOnly,
        RequiresSession,
        ExplicitUserAction,
    }

    /// 命令 → 未配置云时的联网姿态。**顺序即 `specta_builder` 的注册顺序**。
    const SURFACE: &[(&str, Reach)] = &[
        ("account_state", Reach::LocalOnly),
        ("account_sign_in_begin", Reach::ExplicitUserAction),
        ("account_sign_in_password", Reach::ExplicitUserAction),
        ("account_sign_in_code", Reach::ExplicitUserAction),
        ("account_sign_in_set_password", Reach::ExplicitUserAction),
        ("account_sign_in_request_code", Reach::ExplicitUserAction),
        // 先判会话（`ensure_access_token` 无 refresh 即 `NotLoggedIn`），未登录零网络。
        ("account_change_password", Reach::RequiresSession),
        ("account_sign_in_browser", Reach::ExplicitUserAction),
        ("account_sign_in_reopen_browser", Reach::LocalOnly),
        ("account_sign_in_cancel", Reach::LocalOnly),
        // 撞上设备上限后用户点「移除这台并登录」才发（登录流程的一条完成路径，ADR-155 ⑧）。
        (
            "account_sign_in_resolve_device_limit",
            Reach::ExplicitUserAction,
        ),
        ("account_sign_out", Reach::RequiresSession),
        ("devices_list", Reach::RequiresSession),
        ("device_revoke", Reach::RequiresSession),
        ("account_web_access", Reach::RequiresSession),
        ("account_web_sessions", Reach::RequiresSession),
        ("account_revoke_web_session", Reach::RequiresSession),
        ("account_revoke_all_web_sessions", Reach::RequiresSession),
        ("account_set_web_access", Reach::RequiresSession),
        ("account_deletion_preview", Reach::RequiresSession),
        ("account_delete", Reach::RequiresSession),
        // 会话判在保存对话框之前：未登录时连对话框都不弹，更不发请求。
        ("account_export", Reach::RequiresSession),
        ("entitlements", Reach::RequiresSession),
        ("usage", Reach::RequiresSession),
        // 上限的换算校验在发请求前，非法值零网络；合法值经 `authed` 先判会话。
        ("spend_cap_set", Reach::RequiresSession),
        // 价目与 capabilities 本是公开端点，但这四条都先判会话再发（`ensure_billing_reach` /
        // `authed`）：账户页只在登录后呈现收单，未登录时一个字节都不发。
        ("billing_offer", Reach::RequiresSession),
        ("billing_available", Reach::RequiresSession),
        ("billing_checkout", Reach::RequiresSession),
        ("billing_portal", Reach::RequiresSession),
        ("billing_verify_card", Reach::RequiresSession),
        // `service_models` 不再是命令：ADR-048 后前端没有任何消费者（对话上下文预算走
        // Rust 侧 `CloudClient::service_models`，不经 IPC）。留一个没人调用的命令面
        // 就是留一条没人守的联网通路。
        ("catalog_status", Reach::LocalOnly),
        // 零网络：只读提示词目录客户端的本地状态 + 进程级注册表（ADR-050 修订）。
        ("prompt_catalog_status", Reach::LocalOnly),
        // **零网络**：只读本地钉住状态。取公钥只发生在登录与「检查更新」（验签失败那一支，
        // `Catalogs::refresh_checking_trust`）里，两者都已是用户显式动作（ADR-060 ②、ADR-071、ADR-170）。
        ("trust_status", Reach::LocalOnly),
        // 零网络：只改本地钉住与凭据（确认是用户看过两组完整公钥之后的显式动作）。
        ("trust_confirm_change", Reach::LocalOnly),
    ];

    /// 从本文件源码提取全部 `#[tauri::command]` 的函数名（与 ip-domain 对着 upstream
    /// 源文件断言 Capability 同一手法：读源码而非引运行时，零额外依赖）。
    fn declared_commands() -> Vec<String> {
        let src = include_str!("commands.rs");
        let mut names = Vec::new();
        let mut armed = false;
        for line in src.lines() {
            let line = line.trim();
            if line == "#[tauri::command]" {
                armed = true;
                continue;
            }
            if !armed {
                continue;
            }
            if let Some(rest) = line
                .strip_prefix("pub async fn ")
                .or_else(|| line.strip_prefix("pub fn "))
            {
                if let Some(name) = rest.split(['(', '<']).next() {
                    names.push(name.trim().to_owned());
                    armed = false;
                }
            }
        }
        names
    }

    #[test]
    fn command_surface_is_pinned_with_network_posture() {
        let declared = declared_commands();
        let pinned: Vec<String> = SURFACE.iter().map(|(name, _)| (*name).to_owned()).collect();
        assert_eq!(
            declared, pinned,
            "云命令面变了。请在 SURFACE 表中登记，并回答：\n             未登录时，这条命令会不会发出网络请求？\n             若会且不是用户显式点击触发的，它就破坏了 ADR-037 的零云网络行为不变量。"
        );
    }

    /// 例外只允许**两组**用户显式动作：登录（开始与各条完成路径）、刷新目录。
    /// 多出一条 = 不变量在被稀释。
    ///
    /// 姿态的判据是**谁触发**，不是打哪个端点：面板挂载时自动调的命令必须 `RequiresSession`
    /// 或 `LocalOnly`，否则打开页面就出网。
    #[test]
    fn only_explicit_user_actions_may_reach_the_cloud_unconfigured() {
        let exceptions: Vec<&str> = SURFACE
            .iter()
            .filter(|(_, reach)| *reach == Reach::ExplicitUserAction)
            .map(|(name, _)| *name)
            .collect();
        assert_eq!(
            exceptions,
            vec![
                "account_sign_in_begin",
                "account_sign_in_password",
                "account_sign_in_code",
                "account_sign_in_set_password",
                "account_sign_in_request_code",
                "account_sign_in_browser",
                "account_sign_in_resolve_device_limit",
            ],
            "未登录时可联网的命令集合变了——这是 ADR-037 不变量的例外清单，\n             每加一条都必须先在 ADR 里写明理由与用户可见性，不能顺手放行。"
        );
    }
}

#[cfg(test)]
mod trust_tests {
    use super::rejected_signature;

    /// 根签名不对（`bad_keys`）与目录签名不对（`bad_signature`）都要去核对服务器身份；
    /// 过期、断网、服务端没发布这类与身份无关的原因不触发那一次额外的取钥。
    #[test]
    fn both_signature_layers_lead_to_the_identity_check() {
        assert!(rejected_signature(Some("bad_signature")));
        assert!(rejected_signature(Some("bad_keys")));
        for other in [
            "expired",
            "keys_expired",
            "network",
            "no_active_catalog",
            "rollback",
        ] {
            assert!(!rejected_signature(Some(other)), "{other}");
        }
        assert!(!rejected_signature(None));
    }
}
