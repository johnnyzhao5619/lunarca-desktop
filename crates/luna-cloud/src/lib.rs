//! luna-cloud —— 官方构建商业模块：tauri 插件「lunarca-cloud」。
//!
//! 纵切 1 · M2（桌面登录闭环）：device-code 登录 / 会话与凭据（系统钥匙链）/
//! 设备管理 / GDPR 注销 / Entitlement Lease 本地验签。
//! 纵切 1 · M3（目录 + 制品）：签名目录客户端状态机（[`catalog`]）与制品面
//! 纯 HTTP 客户端（[`artifact`]；upstream `ArtifactFetcher` trait 的适配器在
//! app-official——分层裁决：luna-cloud 不背 luna-app 的 whisper FFI 构建）。
//! 装配经 upstream 的 `AppAssembly::with_plugin`（TDD-14 §3.2 ①）进入官方构建；
//! OSS 构建零此符号（商业仓 CI 双风味 `nm` 断言，TDD-13 §4）。
//!
//! 结构：
//! - [`account`]：登录了哪一台服务器、登录到哪一步（唯一事实源，ADR-155）。
//! - [`config`]：由账户与信任锚派生的 API 根地址 + Lease/目录两把验签公钥。
//! - [`store`]：凭据存储（钥匙链 service `"lunarca-cloud"`，access/refresh/profile 分条）。
//! - [`client`]：cp-api HTTP 客户端（refresh 轮换 / 401 静默重试语义与服务端对齐）。
//! - [`billing`]：SaaS 收单的价目 / 结账 / 账单管理（协议 v10，纵切 3 C2）。
//! - [`account_data`]：导出我的账户数据、注销前的后果预览与回执（协议 v14，纵切 3 C6）。
//! - [`lease`]：Lease JWT 离线验签（Ed25519 公钥侧）。
//! - [`catalog`]：目录客户端状态机（验签/过期/防回滚/缓存回退 + 严格本地拦截）。
//! - [`artifact`]：制品面客户端（镜像 resolve + 断点续传 + sha256 终验）。
//! - [`trust`]：信任锚（组织配置文件解析 + 两把签名公钥的钉住状态，ADR-060/071）。
//! - [`commands`]：tauri 命令薄层 + typed DTO。
//!
//! 日志纪律：本 crate 不打印任何 token / 邮箱 / 密码（与产品日志零 PII 同线）。

pub mod account;
pub mod account_data;
pub mod artifact;
pub mod billing;
pub mod browser;
pub mod catalog;
pub mod client;
pub mod commands;
pub mod config;
pub mod connect_link;
pub mod device_limit;
pub mod error;
pub mod fingerprint;
pub mod first_contact;
pub mod lease;
#[cfg(test)]
mod opaque_mock;
pub mod protocol;
pub mod store;
pub mod sync_client;
pub mod tls_root;
pub mod trust;
pub mod usage;

use std::collections::HashSet;
use std::sync::{Arc, OnceLock};

use tauri::Manager as _;

pub use account_data::ExportDestinationPicker;
pub use catalog::{
    AudienceContext, AudienceProvider, CatalogClient, CatalogDelivery, CatalogKind,
    StrictLocalProbe,
};
pub use client::CloudClient;
pub use config::CloudConfig;
pub use error::CloudError;

/// 插件名（IPC 命令前缀 `plugin:luna-cloud|…`；与前端绑定生成一致）。
///
/// **必须等于 crate 名**：Tauri v2 的 ACL 命名空间取自 crate 的 `links`（= 包名
/// `luna-cloud`），而权限匹配用的是**运行时插件名**。二者不一致时权限声明得再全也对
/// 不上号，所有命令被 ACL 静默拒绝——前端拿到不带 `code` 的拒绝对象，UI 显示
/// `error.undefined`。这正是纵切 1.5 真机验收踩到的坑（原名 `lunarca-cloud` 与包名
/// `luna-cloud` 不一致）。改名须同批重生成前端绑定。
pub const PLUGIN_NAME: &str = "luna-cloud";

/// 生成的 TS 绑定落位（提交入库；重新生成后 `git diff` 应为空，与 upstream 同纪律）。
pub const BINDINGS_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../apps/desktop/src/ipc/cloud-bindings.ts"
);

/// tauri-specta builder（运行时挂载与 headless 导出共用同一工厂，DRY——
/// 与 upstream `ipc::builder()` 同范式）。
#[must_use]
pub fn specta_builder<R: tauri::Runtime>() -> tauri_specta::Builder<R> {
    tauri_specta::Builder::<R>::new()
        .plugin_name(PLUGIN_NAME)
        .commands(tauri_specta::collect_commands![
            commands::account_state,
            commands::account_sign_in_begin,
            commands::account_sign_in_password,
            commands::account_sign_in_code,
            commands::account_sign_in_set_password,
            commands::account_sign_in_request_code,
            commands::account_change_password,
            commands::account_sign_in_browser,
            commands::account_sign_in_reopen_browser,
            commands::account_sign_in_cancel,
            commands::account_sign_in_resolve_device_limit,
            commands::account_sign_out,
            commands::devices_list,
            commands::device_revoke,
            commands::account_web_access,
            commands::account_set_web_access,
            commands::account_web_sessions,
            commands::account_revoke_web_session,
            commands::account_revoke_all_web_sessions,
            commands::account_deletion_preview,
            commands::account_delete,
            commands::account_export,
            commands::entitlements,
            commands::usage,
            commands::spend_cap_set,
            commands::billing_offer,
            commands::billing_available,
            commands::billing_checkout,
            commands::billing_portal,
            commands::billing_verify_card,
            commands::catalog_status,
            commands::prompt_catalog_status,
            commands::trust_status,
            commands::trust_confirm_change,
        ])
}

/// 账户变化的通知回调（选定服务器 / 登录 / 退出 / 注销 / 等浏览器）。
///
/// # 为什么必须有这个接缝
///
/// 「有没有会话」决定算力落点：登录了哪台服务器，AI 就由谁提供；未登录全在本机。没有这个
/// 回调时，登录态变化不会触发 Provider 重装，界面与执行路径会分叉。`luna-cloud` 不知道
/// 「算力落点」这个概念，也不该知道——它只负责在账户变了的时候说一声、说清是哪一种。
pub type AccountObserver = Arc<dyn Fn(account::AccountChange) + Send + Sync + 'static>;

/// 会话**被动失效**（refresh 被拒、设备被撤销）时的回调。
pub type SessionObserver = Arc<dyn Fn() + Send + Sync + 'static>;

/// 提示词目录装载后**用户必须知道**的事实（ADR-050 修订 2026-09-04）。
///
/// 只有两种：整份被拒（连 v1 清单都不齐，沿用随包默认）、按旧清单部分接受（比它新的
/// 必备键回落随包）。齐备接受与撤销不在此列——那是常态，设置页的状态命令答得了。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptOverlayAlert {
    /// 整份被拒：`key` = 第一个缺失的 v1 必备键（`scope.name`）。
    Rejected { version: u32, key: String },
    /// 部分接受：齐备到 `manifest_version`，`missing` 从随包取。
    Degraded {
        version: u32,
        manifest_version: u32,
        missing: Vec<String>,
    },
}

/// 提示词目录告警的观察者（装配方注入，发通知中心；`luna-cloud` 不知道通知中心是什么）。
///
/// 与 [`StrictLocalProbe`] / [`SessionObserver`] 同一形态的注入接缝。此前「被拒」只有
/// 一行 stderr：运营发了一版、客户端没用上、两边都不报错，界面上没有任何信号
/// （ADR-050 自己记的缺口，ADR-083 ⑦ 第一次常态触发它）。
pub type PromptOverlayObserver = Arc<dyn Fn(&PromptOverlayAlert) + Send + Sync + 'static>;

/// 「此刻是否连着服务端」探针（装配方注入）：`true` = 已登录且不在严格本地（ADR-155：算力与
/// 在线数据都跟着账户走，没有「配了地址没登录」这一档）。
///
/// **后台目录同步只在它为 `true` 时发生**（ADR-050 修订 2026-09-05 ③）。ADR-037 的不变量
/// 是「未登录时零云网络行为」——同步在未登录时一个字节都不发；登录之后它是一条可解释、
/// 可在设置页看到时刻的后台动作，与租约续期同一性质。
/// 判据仍由宿主的 `CloudGate` 给（同 [`StrictLocalProbe`]：`luna-cloud` 不自造设置系统）。
pub type ConnectedProbe = Arc<dyn Fn() -> bool + Send + Sync + 'static>;

/// 凭据存储的出处（装配方注入）：插件 setup 时以应用配置目录调用一次，得到云客户端的
/// [`store::CredentialStore`]。
///
/// 生产实现是本机保险箱（TDD-12 §2：钥匙串里只有一条主密钥，其余逐条加密落盘），它同时装着
/// 度量钥匙等不属于本插件的秘密，故由宿主持有、本插件只拿到自己那一个命名空间——同
/// [`StrictLocalProbe`]：`luna-cloud` 不自造存储，也不碰系统钥匙串。
pub type CredentialStoreFactory =
    Box<dyn FnOnce(&std::path::Path) -> Box<dyn store::CredentialStore> + Send + 'static>;

/// 后台同步第一次尝试前的延迟：让启动装配与缓存装载先落定。
const AUTO_REFRESH_INITIAL_DELAY: std::time::Duration = std::time::Duration::from_secs(20);
/// 后台同步的检查间隔（每次只问「距上次刷新够不够久」，够久才真的发请求）。
const AUTO_REFRESH_CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// 后台用量刷新的间隔（PRD-13-040 ③，纵切 3 C4）。登录后的那一次由宿主的启动预热做
/// （`cloud::reconfigure_cloud_stack`），故循环先睡满一个间隔再取，不与预热撞在同一刻。
const USAGE_REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// 后台用量刷新：不打开账户页也能在 80% / 95% / 用尽 / 上限时收到提醒。
async fn usage_refresh_loop(
    client: Arc<CloudClient>,
    observer: commands::AccountObserverState,
    connected: ConnectedProbe,
) {
    loop {
        tokio::time::sleep(USAGE_REFRESH_INTERVAL).await;
        usage_refresh_tick(&client, &observer, &connected).await;
    }
}

/// 后台用量刷新的一拍：**只在已登录且不在严格本地时**取（[`ConnectedProbe`]），否则一个字节都不发
/// （ADR-037 零云网络不变量）。取数走唯一那条 [`commands::fetch_usage`]——预警判定与通知都在那里。
/// 返回这一拍有没有去取（测试用）。失败只当这一拍没取到：下一拍、账户页、同步都会再取。
async fn usage_refresh_tick(
    client: &CloudClient,
    observer: &commands::AccountObserverState,
    connected: &ConnectedProbe,
) -> bool {
    if !connected() {
        return false;
    }
    let _ = commands::fetch_usage(client, observer).await;
    true
}

/// 目录的后台同步循环（stale-while-revalidate 的 revalidate 半边）。
///
/// 此前目录只在用户点「刷新目录」时才拉：运营发布一版提示词，没点过按钮的用户永远用旧版。
/// 现在已连接的客户端最多每 [`catalog::AUTO_REFRESH_MAX_AGE_SECS`] 拉一次；登录与换服务端
/// 另由宿主立刻触发一次（`cloud::reconfigure_cloud_stack`）。
async fn auto_refresh_loop(catalogs: commands::Catalogs, connected: ConnectedProbe) {
    tokio::time::sleep(AUTO_REFRESH_INITIAL_DELAY).await;
    loop {
        if connected() {
            catalogs
                .refresh_if_stale(catalog::AUTO_REFRESH_MAX_AGE_SECS)
                .await;
        }
        tokio::time::sleep(AUTO_REFRESH_CHECK_INTERVAL).await;
    }
}

/// 把一份已验签的本地模型目录逐条解析后装进 `luna_model_catalog::remote`（`None` = 回到随包）。
fn install_model_entries(payload: Option<CatalogDelivery<serde_json::Value>>) {
    let Some(CatalogDelivery {
        version, entries, ..
    }) = payload
    else {
        luna_model_catalog::remote::clear();
        return;
    };
    let (accepted, skipped) = luna_model_catalog::parse_lenient(entries);
    if skipped > 0 {
        eprintln!("[luna-cloud] 本地模型目录 v{version}：{skipped} 条本版不认识或不合法，已跳过");
    }
    luna_model_catalog::remote::install(version, accepted, skipped);
}

/// 把一份已验签的提示词目录装进进程级注册表（`None` = 回到随包默认）。
///
/// **装不进去不是错误路径的失败，而是一条必须说出来的事实**：连 v1 清单都不齐时整份
/// 拒绝、沿用随包默认；按旧清单签的目录则**部分接受**（新键回落随包）。两种情形都留一条
/// `warn` 并经 [`PromptOverlayObserver`] 上报（每个事实每进程只报一次）。静默吞掉的话，
/// 运营在管理台看到「已生效」，而客户端用的仍是发版那天的模板——两边都没有任何东西会报错。
fn install_prompt_sets(
    payload: Option<CatalogDelivery<luna_core::CapabilityPromptSet>>,
    observer: &PromptOverlayObserver,
) {
    let registry = luna_core::prompt::registry();
    let Some(CatalogDelivery {
        version,
        variant,
        entries: sets,
    }) = payload
    else {
        registry.clear_overlay();
        return;
    };
    // 版本号在 wire 上是 u64（与目录版本同源），注册表侧收 u32：目录版本是服务端小步
    // 单调计数，饱和裁剪只可能发生在一个根本不该出现的量级上，且裁剪后仍单调。
    let clamped = u32::try_from(version).unwrap_or(u32::MAX);
    match registry.install_overlay(clamped, variant, sets) {
        Err(rejected) => {
            let luna_core::PromptOverlayRejected::MissingTemplate { scope, template } = &rejected;
            let key = format!("{}.{}", scope.as_str(), template);
            if !should_report_overlay_rejection(reported_prompt_overlay_rejections(), clamped, &key)
            {
                return;
            }
            eprintln!("[luna-cloud] 提示词目录被拒：`{key}` 缺失，沿用随包默认（目录 v{clamped}）");
            observer(&PromptOverlayAlert::Rejected {
                version: clamped,
                key,
            });
        }
        Ok(installed) if !installed.missing.is_empty() => {
            let dedupe_key = format!("degraded@{}", installed.manifest_version);
            if !should_report_overlay_rejection(
                reported_prompt_overlay_rejections(),
                clamped,
                &dedupe_key,
            ) {
                return;
            }
            eprintln!(
                "[luna-cloud] 提示词目录 v{clamped} 按旧清单（v{}）部分生效：{} 条新模板回落随包默认",
                installed.manifest_version,
                installed.missing.len()
            );
            observer(&PromptOverlayAlert::Degraded {
                version: clamped,
                manifest_version: installed.manifest_version,
                missing: installed.missing,
            });
        }
        Ok(_) => {}
    }
}

/// 已经说过的降级提示（`目录版本 + 缺失模板`）。
///
/// **降噪不是消音**：这条提示本身是对的（目录不完备 → 回落随包默认，功能零损失），
/// 静默掉就回到了「运营在管理台看到『已生效』，客户端用的仍是发版那天的模板」那种两边
/// 都不报错的状态。它要被说，但**只说一次**。
///
/// 为什么会重复：同一份目录会经**三条**路各装一次（插件 setup 的
/// [`CatalogClient::attach_state_path`] 装载缓存 / 启动装配 [`CatalogClient::reconfigure`]
/// 切桶后重装 / 显式 [`CatalogClient::refresh`] 拉到新目录），每条都如实通知观察者——
/// 通知本身没有错，错的是把「同一件事」重复喊三遍。故去重的键取**目录版本 + 缺失模板**：
/// 服务端推了新版目录却仍漏同一条时，那是一件**新的**事实，该再说一次。
fn reported_prompt_overlay_rejections() -> &'static std::sync::Mutex<HashSet<(u32, String)>> {
    static SEEN: OnceLock<std::sync::Mutex<HashSet<(u32, String)>>> = OnceLock::new();
    SEEN.get_or_init(|| std::sync::Mutex::new(HashSet::new()))
}

/// 这条降级提示此刻**该不该说**（`true` = 说；同一条只在第一次为真）。
///
/// 抽成纯状态判定是为了让「只喊一次」可无头测试——重复刷屏这种缺陷在单元测试里看不见，
/// 只有启动日志里连着三行一模一样的话才暴露，而那时它已经上了真机。
fn should_report_overlay_rejection(
    seen: &std::sync::Mutex<HashSet<(u32, String)>>,
    version: u32,
    key: &str,
) -> bool {
    seen.lock()
        .map_or(true, |mut seen| seen.insert((version, key.to_owned())))
}

/// 主题目录的交付变了（验签换上一份、缓存装载、换服务端或重钉时撤销）：宿主据此让外观与两个主题面板重读。
///
/// 主题目录没有别的观察者——条目由宿主经 `CatalogClient::current()` 按需取——但**封禁要在运行中生效**：
/// 2026-10-21 真机，刷新后清单已写「Lunarca 正在使用」，窗口却仍是被封的那套配色，直到重启。被封的主题
/// 不改设置（解除后要自动回来），于是没有任何一个设置变更事件会让外观重读。
pub type CatalogChangedObserver = Arc<dyn Fn() + Send + Sync + 'static>;

/// 宿主注入的全部钩子（装配期一次性，[`init`] 的唯一参数）。字段各自的含义见 [`init`]。
pub struct CloudHooks {
    pub probe: StrictLocalProbe,
    pub on_account_changed: AccountObserver,
    pub on_prompt_overlay: PromptOverlayObserver,
    pub on_themes_changed: CatalogChangedObserver,
    pub on_templates_changed: CatalogChangedObserver,
    pub connected: ConnectedProbe,
    pub audience: AudienceProvider,
    pub pick_export_destination: ExportDestinationPicker,
    pub credentials: CredentialStoreFactory,
}

/// 插件入口：
/// `AppAssembly::default().with_plugin(move |b| b.plugin(luna_cloud::init(CloudHooks { … })))`。
///
/// `probe` = 严格本地探针（TDD-14 §3.1：所有出站请求在发出前阻断）。设置事实源在宿主，
/// 探针由装配方注入——luna-cloud 不自造设置系统；也因此**没有无探针的入口**。
///
/// 服务器与验签公钥不由宿主注入：它们是账户与信任锚的事实（[`account`] / [`trust`]），
/// 本插件自己持久化、自己派生（[`CloudClient::reconfigure`]）。
///
/// `luna-cloud` 不依赖 `tracing`（它是插件层，日志走宿主），上面只用 `eprintln!` 留痕
/// ——正文零泄漏（只打域名与模板名，不含任何提示词内容）。
///
/// `pick_export_destination` = 保存对话框（账户数据导出用，纵切 3 C6）：对话框插件在宿主，
/// 同探针一样由装配方注入。
///
/// `credentials` = 凭据存储的出处（[`CredentialStoreFactory`]）。应用配置目录解析不出来时
/// 退回进程内存储：能登录，但会话不跨重启——比装配失败、整个插件起不来好。
///
/// `on_themes_changed` / `on_templates_changed` = 主题目录 / 模板目录交付变了（[`CatalogChangedObserver`]）。
pub fn init<R: tauri::Runtime>(hooks: CloudHooks) -> tauri::plugin::TauriPlugin<R> {
    let CloudHooks {
        probe,
        on_account_changed,
        on_prompt_overlay,
        on_themes_changed,
        on_templates_changed,
        connected,
        audience,
        pick_export_destination,
        credentials,
    } = hooks;
    let builder = specta_builder::<R>();
    tauri::plugin::Builder::new(PLUGIN_NAME)
        .invoke_handler(builder.invoke_handler())
        .setup(move |app, _api| {
            app.manage(commands::AccountObserverState(Arc::clone(
                &on_account_changed,
            )));
            app.manage(commands::ExportPickerState(Arc::clone(
                &pick_export_destination,
            )));
            let store = match app.path().app_config_dir() {
                Ok(dir) => credentials(&dir),
                Err(_) => {
                    eprintln!("[cloud] app config dir unavailable; session will not persist");
                    Box::new(store::MemoryStore::new())
                }
            };
            let client = Arc::new(CloudClient::new(store, Arc::clone(&probe)));
            // 信任锚：钉住值必须跨重启存活（它是用户做过的一次安全决定，不是缓存）。
            // 与目录状态**分文件**——理由见 `trust::TRUST_STATE_FILE`。
            if let Ok(dir) = app.path().app_data_dir() {
                client
                    .trust()
                    .attach_path(dir.join(trust::TRUST_STATE_FILE));
                // 自签部署的根证书钉住（ADR-153）：装载即登记，启动后第一个请求就用得上。
                client
                    .tls_roots()
                    .attach_path(dir.join(tls_root::TLS_ROOT_STATE_FILE));
                // 记住的服务器：装载后按它与它的钉住公钥派生配置。
                client
                    .account()
                    .attach_path(dir.join(account::ACCOUNT_STATE_FILE));
                // 设备指纹读不出硬件标识时的退路随机 id（纵切 3 C5）：落在同一个应用数据目录。
                client
                    .fingerprint()
                    .attach_fallback_path(dir.join(fingerprint::FALLBACK_ID_FILE));
            }
            client.reconfigure();
            let config = client.config();
            tauri::async_runtime::spawn(usage_refresh_loop(
                Arc::clone(&client),
                commands::AccountObserverState(Arc::clone(&on_account_changed)),
                Arc::clone(&connected),
            ));
            app.manage(commands::CloudState(client));
            // 目录客户端：防回滚水位/缓存目录落应用数据目录小文件（经 app 路径 API）。
            // 三份目录、一套基础设施（ADR-017）——只有端点、条目形状与分桶后缀不同。
            // 本地模型目录验签通过即逐条解析、装进进程级覆盖层（ADR-145 ①③）；单条认不出只跳过。
            let models = CatalogClient::new(
                catalog::CatalogKind::LocalModels,
                config.api_base.clone(),
                config.catalog_pubkey,
                Arc::clone(&probe),
            )
            .with_observer(Arc::new(install_model_entries))
            .with_audience(Arc::clone(&audience));
            // 提示词目录验签通过即装进进程级 [`PromptRegistry`]（ADR-050）。装不进去
            // （连 v1 清单都不齐）记一条日志、发一条告警并沿用随包默认——目录刷新是增强
            // 不是依赖；按旧清单签的目录部分生效，同样告警一次。
            let alert = Arc::clone(&on_prompt_overlay);
            let prompts = CatalogClient::new(
                catalog::CatalogKind::Prompts,
                config.api_base.clone(),
                config.catalog_pubkey,
                Arc::clone(&probe),
            )
            .with_observer(Arc::new(move |payload| {
                install_prompt_sets(payload, &alert)
            }))
            .with_audience(Arc::clone(&audience));
            // 主题目录（ADR-159 ③）：条目由宿主按需经 `current()` 取；交付一变只通知宿主重读，封禁才能在
            // 运行中生效（见 [`CatalogChangedObserver`]）。
            let themes = CatalogClient::new(
                catalog::CatalogKind::Themes,
                config.api_base.clone(),
                config.catalog_pubkey,
                Arc::clone(&probe),
            )
            .with_observer(Arc::new(move |_| on_themes_changed()))
            .with_audience(Arc::clone(&audience));
            // 模板目录（ADR-159 ③ ⑥，纵切 7b）：同主题目录——条目按需取，交付一变通知宿主（封禁运行中生效）。
            let templates = CatalogClient::new(
                catalog::CatalogKind::Templates,
                config.api_base.clone(),
                config.catalog_pubkey,
                Arc::clone(&probe),
            )
            .with_observer(Arc::new(move |_| on_templates_changed()))
            .with_audience(audience);
            if let Ok(dir) = app.path().app_data_dir() {
                let state_file = dir.join(catalog::CATALOG_STATE_FILE);
                models.attach_state_path(state_file.clone());
                prompts.attach_state_path(state_file.clone());
                themes.attach_state_path(state_file.clone());
                templates.attach_state_path(state_file);
            }
            let catalogs = commands::Catalogs {
                models: Arc::new(models),
                prompts: Arc::new(prompts),
                themes: Arc::new(themes),
                templates: Arc::new(templates),
            };
            // 后台同步（ADR-050 修订 ③）：已连接时最多每 6 小时拉一次全部目录。
            tauri::async_runtime::spawn(auto_refresh_loop(
                catalogs.clone(),
                Arc::clone(&connected),
            ));
            app.manage(catalogs);
            Ok(())
        })
        .build()
}

/// Headless 导出 TS 绑定（`cargo run -p luna-cloud --bin export-cloud-bindings`）。
///
/// # Errors
/// 类型导出或写盘失败时返回错误。
pub fn export_bindings() -> Result<(), Box<dyn std::error::Error>> {
    specta_builder::<tauri::Wry>()
        .export(specta_typescript::Typescript::default(), BINDINGS_PATH)?;
    normalize_bindings(BINDINGS_PATH)?;
    Ok(())
}

/// 与 upstream 同一口径的产物规范化：去行尾空白，保证生成物确定、与格式化工具一致
/// （upstream lib.rs `normalize_bindings` 的理由原样适用）。
fn normalize_bindings(path: &str) -> std::io::Result<()> {
    let raw = std::fs::read_to_string(path)?;
    let mut out = String::with_capacity(raw.len());
    for line in raw.lines() {
        out.push_str(line.trim_end());
        out.push('\n');
    }
    std::fs::write(path, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 同一份目录会经三条路各装一次（缓存装载 / 换服务端切桶 / 显式刷新），启动日志里
    /// 因此连打三行一模一样的话（真机验收实测）。收敛为**每份目录只说一次**。
    #[test]
    fn the_same_degradation_notice_is_said_once_not_once_per_install_path() {
        let seen = std::sync::Mutex::new(HashSet::new());

        assert!(should_report_overlay_rejection(
            &seen,
            7,
            "translate.live_context"
        ));
        // 后两条装载路径复述同一件事——不再刷屏。
        assert!(!should_report_overlay_rejection(
            &seen,
            7,
            "translate.live_context"
        ));
        assert!(!should_report_overlay_rejection(
            &seen,
            7,
            "translate.live_context"
        ));
    }

    /// 后台用量刷新（纵切 3 C4）：未登录或严格本地时这一拍什么都不做——一个字节都不发、
    /// 观察者一次都不被叫；连着时走唯一的取数路径，取不到也不报错（下一拍再来）。
    #[tokio::test]
    async fn usage_refresh_is_a_no_op_when_not_connected() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        // 一个只计数、什么都不答的端口：发出任何请求都会被数到。
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_server = Arc::clone(&hits);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if stream.is_ok() {
                    hits_server.fetch_add(1, Ordering::SeqCst);
                }
            }
        });
        let client = CloudClient::new(Box::new(store::MemoryStore::new()), Arc::new(|| false));
        client.account().set_server(account::AccountServer {
            origin: format!("http://{address}"),
            display_name: None,
        });
        client.reconfigure();
        let notified = Arc::new(AtomicUsize::new(0));
        let notified_observer = Arc::clone(&notified);
        let observer = commands::AccountObserverState(Arc::new(move |_| {
            notified_observer.fetch_add(1, Ordering::SeqCst);
        }));

        let offline: ConnectedProbe = Arc::new(|| false);
        assert!(!usage_refresh_tick(&client, &observer, &offline).await);
        // 连着（探针为真）但本机没有会话：`authed` 在发请求前本地短路。
        let online: ConnectedProbe = Arc::new(|| true);
        assert!(usage_refresh_tick(&client, &observer, &online).await);

        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(
            hits.load(Ordering::SeqCst),
            0,
            "未连接 / 无会话时发出了请求"
        );
        assert_eq!(notified.load(Ordering::SeqCst), 0);
        assert_eq!(client.usage_alerts(), None);
    }

    /// **反向条款：降噪不是消音**。同一份目录里的**另一处**缺失、以及服务端推了新版
    /// 目录后**仍然**漏同一条，都是新的事实，必须照样说出来——否则这条门禁守住的就不是
    /// 「不刷屏」，而是「运营永远看不见自己漏了什么」。
    #[test]
    fn a_different_fact_is_still_reported() {
        let seen = std::sync::Mutex::new(HashSet::new());
        assert!(should_report_overlay_rejection(
            &seen,
            7,
            "translate.live_context"
        ));

        // 同一份目录，另一处缺失。
        assert!(should_report_overlay_rejection(&seen, 7, "chat.system"));
        // 新版目录，仍漏同一条 —— 服务端刚推过一版，这是新消息。
        assert!(should_report_overlay_rejection(
            &seen,
            8,
            "translate.live_context"
        ));
    }
}
