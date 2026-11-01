//! 设置命令域（PRD-09/TDD-05 §7.1，IPC 层，从 `ipc/mod.rs` 纵切）。
//!
//! 职责边界：设置注册表的投影（DTO）、读写与重置、密钥项三态、以及**设置副作用**
//! （改一项之后要重装什么）。注册表本身是 [`luna_config::SETTINGS`]，此处不复制口径。
//!
//! 跨模块可见性：命令与本域 DTO `pub(crate)`；共享 helper 经 `use super::*` 从 `mod.rs` 取。

use super::*;

/// 设置值 DTO（TOML 基础类型：bool / int / string）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum SettingValueDto {
    /// 布尔。
    Bool(bool),
    /// 整数。
    Int(i32),
    /// 字符串。
    Str(String),
    /// 有序字符串列表（TDD-05 §7.5a；wire 形态 `{kind:"string_list", value:string[]}`）。
    StringList(Vec<String>),
}

#[derive(Debug, Clone, Serialize, Type)]
pub struct SettingGroupResetPlanDto {
    pub fingerprint: String,
    pub changes: Vec<SettingSpecDto>,
}

fn setting_group_reset_plan(
    settings: &Settings,
    group: &str,
    scope: ResetScope,
    vault: Option<&Path>,
) -> Result<SettingGroupResetPlanDto, String> {
    use sha2::{Digest, Sha256};
    let changes: Vec<_> = settings_spec_dtos(settings)?
        .into_iter()
        .filter(|spec| {
            spec.group == group
                && spec.available
                && spec.resettable
                && spec.modified
                && (matches!(scope, ResetScope::All) || spec.visibility == "standard")
        })
        .collect();
    let bytes =
        serde_json::to_vec(&(group, scope, vault, &changes)).map_err(|err| err.to_string())?;
    Ok(SettingGroupResetPlanDto {
        fingerprint: format!("{:x}", Sha256::digest(bytes)),
        changes,
    })
}

#[tauri::command]
#[specta::specta]
pub(crate) fn preview_setting_group_reset(
    state: tauri::State<'_, AppState>,
    group: String,
    scope: ResetScope,
) -> Result<SettingGroupResetPlanDto, String> {
    let vault = state
        .vault
        .lock()
        .map_err(|_| "Vault lock poisoned".to_owned())?;
    let settings = state.settings_snapshot();
    setting_group_reset_plan(&settings, &group, scope, vault.as_ref().map(|v| v.root()))
}

/// 活动栏布局里，所属一方应用未启用的空间不在设置页出现（ADR-165 ④）：选项与当前值里都拿掉。
fn without_unavailable_spaces(
    settings: &Settings,
    key: &str,
    current: SettingValue,
    control: SettingControlDto,
) -> (SettingValue, SettingControlDto) {
    if key != RAIL_LAYOUT_SETTING_KEY {
        return (current, control);
    }
    let hidden = luna_config::apps::unavailable_spaces(settings);
    let SettingControlDto::OrderedToggleList {
        options,
        hidden_prefix,
        min_visible,
    } = control
    else {
        return (current, control);
    };
    let current = match current {
        SettingValue::StringList(tokens) => SettingValue::StringList(
            tokens
                .into_iter()
                .filter(|token| {
                    let id = token.strip_prefix(hidden_prefix.as_str()).unwrap_or(token);
                    !hidden.contains(&id)
                })
                .collect(),
        ),
        other => other,
    };
    let control = SettingControlDto::OrderedToggleList {
        options: options
            .into_iter()
            .filter(|option| !hidden.contains(&option.id.as_str()))
            .collect(),
        hidden_prefix,
        min_visible,
    };
    (current, control)
}

/// 设置页写回活动栏布局时，把它看不到的空间放回原处（[`without_unavailable_spaces`] 的反方向）。
fn keep_unavailable_rail_slots(
    settings: &Settings,
    key: &str,
    value: SettingValueDto,
) -> SettingValueDto {
    let SettingValueDto::StringList(next) = value else {
        return value;
    };
    if key != RAIL_LAYOUT_SETTING_KEY {
        return SettingValueDto::StringList(next);
    }
    let previous = settings
        .get(RAIL_LAYOUT_SETTING_KEY)
        .and_then(|value| value.as_string_list().map(<[String]>::to_vec))
        .unwrap_or_default();
    let frozen = luna_config::apps::unavailable_spaces(settings);
    SettingValueDto::StringList(luna_config::workspace_space::keep_frozen_slots(
        &previous, &next, &frozen,
    ))
}

/// 活动栏布局的设置键（与 `rail.rs` 同一个键）。
const RAIL_LAYOUT_SETTING_KEY: &str = "workspace.rail_layout";

pub(crate) fn settings_spec_dtos(settings: &Settings) -> Result<Vec<SettingSpecDto>, String> {
    SETTINGS
        .iter()
        .map(|spec| {
            let current = settings
                .get(spec.key)
                .unwrap_or_else(|| spec.default.to_value());
            let (shown_value, control) = without_unavailable_spaces(
                settings,
                spec.key,
                current.clone(),
                SettingControlDto::from(&spec.control),
            );
            let shortcut_conflict = existing_shortcut_conflict(settings, spec, &current);
            Ok(SettingSpecDto {
                key: spec.key.to_string(),
                group: luna_config::setting_page(spec.key).as_str().to_string(),
                scope: spec.scope.as_str().to_string(),
                default_value: SettingValueDto::try_from(spec.default.to_value())?,
                modified: current != spec.default.to_value(),
                available: spec.available(settings),
                resettable: spec.resettable(),
                current_value: SettingValueDto::try_from(shown_value)?,
                requires_restart: spec.requires_restart,
                surface: SettingSurfaceDto::of(spec),
                visibility: spec.visibility().as_str().to_owned(),
                section: luna_config::setting_page_section(spec).map(str::to_owned),
                i18n_key: spec.i18n_key.to_string(),
                desc_i18n_key: match (spec.key, current.as_str()) {
                    ("vault.delete_mode", Some("trash")) => {
                        "settings:vault.delete_mode_desc_trash".to_owned()
                    }
                    ("vault.delete_mode", Some("vault_trash")) => {
                        "settings:vault.delete_mode_desc_vault_trash".to_owned()
                    }
                    ("vault.delete_mode", Some("permanent")) => {
                        "settings:vault.delete_mode_desc_permanent".to_owned()
                    }
                    _ => spec.desc_i18n_key(),
                },
                desc_presentation: spec.desc_presentation().into(),
                control,
                shortcut_conflict,
            })
        })
        .collect()
}

/// 一个快捷键项此刻与哪一项撞键（与改键时同一个判据 [`luna_config::shortcut::conflicting_key`]）。
fn existing_shortcut_conflict(
    settings: &Settings,
    spec: &luna_config::SettingSpec,
    current: &SettingValue,
) -> Option<String> {
    if !matches!(spec.control, luna_config::SettingControl::Shortcut { .. }) {
        return None;
    }
    let value = current.as_str().filter(|value| !value.trim().is_empty())?;
    luna_config::shortcut::conflicting_key(settings, spec.key, value).map(str::to_owned)
}

pub(crate) fn set_setting_value(
    i18n: BackendCopy<'_>,
    settings: &Mutex<Settings>,
    persistence: &SettingsPersistence,
    key: &str,
    value: SettingValueDto,
) -> Result<(), String> {
    if key.starts_with("ai.assignment.") {
        return Err(coded_error_with(
            i18n,
            "SETTING_DOMAIN_CONTROL",
            "setting-domain-control-assign",
        ));
    }
    let spec = setting_spec(key).ok_or_else(|| format!("Unknown setting: {key}"))?;
    // **先归一化再校验**（`luna_config::normalize` 的文档给出顺序理由）：地址少打
    // `http://` 是机器能无歧义补全的事，补完回显给用户看，而不是让他为此再跑一趟。
    // 与配置文件加载走同一对函数——两处各写一遍，迟早出现"文件里能存、界面上存不进"。
    let value = luna_config::normalize(spec, SettingValue::from(value));
    // 控件契约校验（选项/范围/路径/快捷键语法/文本格式，`luna_config::validate` 单一口径）。
    // 错误串以**稳定 code 开头**，前端据此取本地化文案（同 EXPORT_FAILED 等既有约定）。
    luna_config::validate(spec, &value)
        .map_err(|rejection| format!("{}: {} {}", rejection.code(), key, rejection.reason()))?;
    let mut guard = settings.lock().map_err(|_| "Settings 锁中毒".to_string())?;
    // 快捷键内部冲突（PRD-09-003 ①）：**阻止保存**，并如实告知被谁占用。
    if matches!(spec.control, luna_config::SettingControl::Shortcut { .. }) {
        if let Some(text) = value.as_str() {
            if let Some(other) = luna_config::shortcut::conflicting_key(&guard, key, text) {
                return Err(format!("SETTING_SHORTCUT_CONFLICT: {other}"));
            }
        }
    }
    let mut next = guard.clone();
    next.set(key, value);
    if spec.scope == SettingScope::Device {
        persist_device_settings(&next, persistence)?;
    }
    *guard = next;
    Ok(())
}

pub(crate) fn reset_setting_value(
    i18n: BackendCopy<'_>,
    settings: &Mutex<Settings>,
    persistence: &SettingsPersistence,
    key: &str,
) -> Result<(), String> {
    if key.starts_with("ai.assignment.") {
        return Err(coded_error_with(
            i18n,
            "SETTING_DOMAIN_CONTROL",
            "setting-domain-control-clear",
        ));
    }
    let spec = setting_spec(key).ok_or_else(|| format!("Unknown setting: {key}"))?;
    if !spec.resettable() {
        return Err(coded_error_with(
            i18n,
            "SETTING_DOMAIN_CONTROL",
            "setting-domain-control-reset",
        ));
    }
    let mut guard = settings.lock().map_err(|_| "Settings 锁中毒".to_string())?;
    let mut next = guard.clone();
    next.reset(key);
    if spec.scope == SettingScope::Device {
        persist_device_settings(&next, persistence)?;
    }
    *guard = next;
    Ok(())
}

/// 批量恢复默认（整组恢复的核心，**一次持锁、一次落盘**）。
///
/// 逐项调 [`reset_setting_value`] 会把整组恢复变成 N 次配置文件写入，中途失败还会留下
/// 「改了一半」的状态；批量恢复本就该是一次事务（PRD-09-052）。
pub(crate) fn reset_setting_values(
    settings: &Mutex<Settings>,
    persistence: &SettingsPersistence,
    keys: &[&'static str],
) -> Result<(), String> {
    let mut guard = settings.lock().map_err(|_| "Settings 锁中毒".to_string())?;
    let mut next = guard.clone();
    let mut touches_device = false;
    for key in keys {
        let spec = setting_spec(key).ok_or_else(|| format!("Unknown setting: {key}"))?;
        next.reset(key);
        touches_device |= spec.scope == SettingScope::Device;
    }
    if touches_device {
        persist_device_settings(&next, persistence)?;
    }
    *guard = next;
    Ok(())
}

/// 设置文件落盘失败的稳定码（设备级 `config.toml` 与库级 `.luna/settings.toml` 共用）。
pub(crate) const SETTING_PERSIST: &str = "SETTING_PERSIST";

pub(crate) fn persist_device_settings(
    settings: &Settings,
    persistence: &SettingsPersistence,
) -> Result<(), String> {
    match persistence {
        SettingsPersistence::Writable(path) => settings
            .save_path(path)
            // 稳定码 + 细节（同 `Rejection::code()` 的 `<CODE>: <细节>` 约定）：前端按码说人话，
            // 此前这里是一句英文原文，落进「未知错误」原样甩给用户（真机 2026-10-06）。
            .map_err(|e| format!("{SETTING_PERSIST}: {e}")),
        // 配置目录都拿不到时才走这里；不阻断用户操作，只是重启不留存。
        SettingsPersistence::MemoryOnly => Ok(()),
    }
}

/// 列出设置注册表及当前生效值。
#[tauri::command]
#[specta::specta]
pub(crate) fn list_settings(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<SettingSpecDto>, String> {
    state
        .settings
        .lock()
        .map(|settings| settings_spec_dtos(&settings))
        .map_err(|_| "Settings 锁中毒".to_string())?
}

/// 把设置里的路由策略预设投影到队列的 Router（启动一次 + 每次改动）。认不出的值按默认。
pub(crate) fn apply_route_strategy(state: &AppState) {
    let strategy = super::models::route_strategy(&state.settings_snapshot());
    state.queue_handle().route_strategy().set(strategy);
}

/// 某项改动后的**热生效副作用**（改值与恢复默认共用；单项与整组恢复亦共用）。
///
/// 抽出的理由不只是省行数：`set_setting`/`reset_setting` 曾各抄一份同样的分支，
/// 新增一处热切逻辑就必然漏掉另一处（改键生效、恢复默认却不生效，是最难察觉的一类 bug）。
pub(crate) fn apply_setting_side_effects(app: &tauri::AppHandle, state: &AppState, key: &str) {
    // 创建动作的可用性快照跟着设置走（TDD-07 §2.2「Vault/设置/音频/算力状态变化后由
    // Rust 失效并发布」）。
    //
    // **不挑键**：算力分配、严格本地、上云同意、笔记库路径……能影响某个动作能不能用
    // 的键不止一个，而"哪些键会影响"正是这份快照自己要回答的问题——在这里再维护一张
    // 键名白名单，就是把同一个判断写第二遍，且漏掉新键时表现为"改了设置界面还灰着"。
    // 重算是一次纯读（不弹权限、不联网、不加载权重），代价配得上这份省心。
    super::create_actions::publish_create_action_states(app);
    // 路由策略预设（PRD-10-008）：改一次即热生效，下一个 Job 就按新顺序排候选。
    if key == "ai.route_strategy" {
        apply_route_strategy(state);
    }
    // 工作区观测的开关：关掉只停止新增，**不删历史**（删除是另一颗按钮，且要二次确认）。
    if key == "privacy.local_workspace_metrics" {
        if let Some(metrics) = app.try_state::<crate::workspace_metrics::LocalWorkspaceMetrics>() {
            metrics.set_enabled(
                state
                    .settings_snapshot()
                    .get_bool("privacy.local_workspace_metrics")
                    .unwrap_or(false),
            );
        }
        // 开关翻转之后面板上那句话必须立刻跟上（`enabled` 与存储状态正交，两者一起发）。
        emit_workspace_metrics_availability_changed(app);
    }
    if key == "privacy.local_knowledge_metrics" {
        if state
            .settings_snapshot()
            .get_bool("privacy.local_knowledge_metrics")
            .unwrap_or(false)
        {
            if let Some(metrics) = app.try_state::<crate::metrics::LocalKnowledgeMetrics>() {
                // 授权投影同步更新；Keychain/SQLite 激活仍留在 blocking worker。
                metrics.enable();
            }
            activate_local_knowledge_metrics(app, true);
        } else if let Some(metrics) = app.try_state::<crate::metrics::LocalKnowledgeMetrics>() {
            metrics.disable(metric_now_ms());
        }
    }
    // 改全局快捷键 → 立即重注册 + 重建双击手势表（值可能在两种语法间切换，无需重启，
    // PRD-09-001 / TDD-05 §1.2/§3）。
    if crate::is_global_shortcut_setting(key) || crate::is_double_tap_setting(key) {
        crate::reregister_global_shortcuts(app);
        crate::apply_double_tap_settings(app);
    }
    // 改细分聚类阈值 → 重建细分 Provider 热生效（模型未装则维持未注册，TDD-03 §10.2）。
    if key == "diarize.cluster_threshold" {
        state.register_diarize_provider_if_available(app);
    }
    // 隐私档 / 服务端转写开关 → 立刻重装 ASR 两条通道。
    //
    // 这两项此前都没挂在这里：批式那条只在启动时注册过一次，流式那条只跟着「模型分配
    // 变更」走。于是改开关要重启才生效，而**打开严格本地后已装的云腿根本不会消失**
    // ——隐私档显示「仅本地」，下一次录音照样往外发。
    //
    // 闸要**先于**重装对齐：重装只换新会话用的句柄，正在跑的那条腿仍持有自己的 `Arc`，
    // 只有闸能当场把它掐断（`OutboundGate` 的论证）。
    //
    // **「算力落点变了」走 `reconfigure_cloud_stack` 一个入口**，不在这里逐个消费者分发。
    //
    // 设置里唯一牵动落点的是隐私档：它不改「连哪一台」（那是账户的事，ADR-155），但决定
    // 「连不连」——`compute_endpoint_source` 在严格本地下恒回 `OnDevice`。此前它另起一条分支
    // 只重注册对话 Provider，而落点其实牵动同样那六个消费者——两条分支各维护一份"要动
    // 谁"的清单，迟早分叉。
    //
    // 不 `return`：这是一个普通分支，与其余分支并列。
    if key == "privacy.strict_local" {
        crate::cloud::reconfigure_cloud_stack(app, state);
    }
    // 服务端转写开关只改「音频要不要外发」，不改任何端点——故只对齐外发闸与 ASR 两条通道，
    // 不惊动账户/目录/制品/对话。
    if key == "provider.remote_asr_enabled" {
        state.sync_outbound_gate();
        if let Err(e) = state.refresh_asr_assembly(app) {
            // 重装失败不改变闸已经生效的事实；下次任务会如实报原因。
            eprintln!("[设置] ASR 装配对齐失败：{e}");
        }
    }
    if key == "labs.translate_space_enabled" {
        if !state.desktop_lens_enabled() {
            crate::hide_desktop_lens(app);
        }
        crate::reregister_global_shortcuts(app);
        crate::apply_double_tap_settings(app);
    }
}

/// 设置一个非密钥配置项。密钥必须走系统钥匙链，不写入 TOML。
#[tauri::command]
#[specta::specta]
pub(crate) fn set_setting(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    key: String,
    value: SettingValueDto,
) -> Result<SettingValueDto, String> {
    let scope = setting_spec(&key)
        .ok_or_else(|| format!("Unknown setting: {key}"))?
        .scope;
    if scope == SettingScope::Vault && state.vault_root().is_none() {
        return Err(state.vault_not_open());
    }
    let value = keep_unavailable_rail_slots(&state.settings_snapshot(), &key, value);
    if key == super::themes::THEME_FAMILY_KEY {
        super::themes::check_theme_family(&app, &value)?;
    }
    set_setting_value(
        state.copy(),
        &state.settings,
        &state.settings_persistence,
        &key,
        value,
    )?;
    if scope == SettingScope::Vault {
        persist_vault_settings(&state)?;
    }
    // 返回 Rust 已归一化、校验并落盘的权威值。前端不能拿原始输入自行重演
    // `luna_config::normalize`；例如 `10.0.0.62:8443` 实际保存为 HTTPS，输入框也必须
    // 立即回显 HTTPS，即使它恰好与本次编辑前的值相同、changed 事件不会造成值变化。
    let saved = state
        .settings_snapshot()
        .get(&key)
        .ok_or_else(|| format!("Setting disappeared after write: {key}"))?;
    apply_setting_side_effects(&app, &state, &key);
    let _ = SettingsChanged { key }.emit(&app);
    SettingValueDto::try_from(saved)
}

/// 恢复单项默认。
#[tauri::command]
#[specta::specta]
pub(crate) fn reset_setting(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    key: String,
) -> Result<(), String> {
    let scope = setting_spec(&key)
        .ok_or_else(|| format!("Unknown setting: {key}"))?
        .scope;
    if scope == SettingScope::Vault && state.vault_root().is_none() {
        return Err(state.vault_not_open());
    }
    reset_setting_value(
        state.copy(),
        &state.settings,
        &state.settings_persistence,
        &key,
    )?;
    if scope == SettingScope::Vault {
        persist_vault_settings(&state)?;
    }
    apply_setting_side_effects(&app, &state, &key);
    let _ = SettingsChanged { key }.emit(&app);
    Ok(())
}

/// 恢复**整组**默认（PRD-09-052 分层粒度：单项 / 单组 / 全量）。
///
/// 只作用于该组内**用户可见**的项（内部状态如首启进度、HUD 几何有专属交互面，不该被
/// 「恢复本组默认」顺手清掉）；密钥项本就不走 TOML。返回实际恢复的项数供 UI 如实反馈。
///
/// # Errors
/// 分组不存在、含 Vault 级项而未打开 Vault、或写盘失败时返回错误。
#[tauri::command]
#[specta::specta]
pub(crate) fn reset_setting_group(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    group: String,
    scope: ResetScope,
    fingerprint: String,
) -> Result<u32, String> {
    let vault = state
        .vault
        .lock()
        .map_err(|_| "Vault lock poisoned".to_owned())?;
    let mut settings = state
        .settings
        .lock()
        .map_err(|_| "Settings lock poisoned".to_owned())?;
    let root = vault.as_ref().map(|v| v.root());
    let plan = setting_group_reset_plan(&settings, &group, scope, root)?;
    if plan.fingerprint != fingerprint {
        return Err(coded_error_with(
            state.copy(),
            "SETTING_RESET_STALE",
            "setting-reset-stale",
        ));
    }
    let keys: Vec<_> = plan
        .changes
        .iter()
        .filter_map(|item| setting_spec(&item.key).map(|spec| spec.key))
        .collect();
    if keys.is_empty() {
        return Ok(0);
    }
    let needs_vault = keys
        .iter()
        .filter_map(|key| setting_spec(key))
        .any(|spec| spec.scope == SettingScope::Vault);
    if needs_vault && root.is_none() {
        return Err(state.vault_not_open());
    }
    let mut next = settings.clone();
    for key in &keys {
        next.reset(key);
    }
    if needs_vault {
        next.save_path_for_scope(
            &vault_settings_path(root.expect("checked vault")),
            SettingScope::Vault,
        )
        .map_err(|err| format!("{SETTING_PERSIST}: {err}"))?;
    }
    if keys
        .iter()
        .filter_map(|key| setting_spec(key))
        .any(|spec| spec.scope == SettingScope::Device)
    {
        if let Err(error) = persist_device_settings(&next, &state.settings_persistence) {
            if needs_vault {
                settings
                    .save_path_for_scope(
                        &vault_settings_path(root.expect("checked vault")),
                        SettingScope::Vault,
                    )
                    .map_err(|rollback| {
                        format!("{SETTING_PERSIST}: {error}; rollback: {rollback}")
                    })?;
            }
            return Err(error);
        }
    }
    *settings = next;
    drop(settings);
    drop(vault);
    for key in &keys {
        apply_setting_side_effects(&app, &state, key);
        let _ = SettingsChanged {
            key: (*key).to_owned(),
        }
        .emit(&app);
    }
    Ok(u32::try_from(keys.len()).unwrap_or(u32::MAX))
}

/// 录快捷键期间收起 / 装回全部全局键（PRD-09-003；理由见 `suspend_global_shortcuts`）。
///
/// 调用方在录制结束的每一条出路上都要调 `active: false`（录到了 / Esc / 点到别处 / 组件
/// 卸载）。兜底：任何一次 `set_setting` / `reset_setting` 都会重注册。
#[tauri::command]
#[specta::specta]
pub(crate) fn set_shortcut_capture(app: tauri::AppHandle, active: bool) {
    if active {
        crate::suspend_global_shortcuts(&app);
    } else {
        crate::resume_global_shortcuts(&app);
    }
}

/// 快捷键运行诊断（PRD-09-003 ②③）：注册失败（被他应用占用）、双击型绑定的按键观测
/// 状态（缺辅助功能权限 / 启动失败）、系统保留键警告——每键至多一条，按严重度取第一条。
///
/// 拉诊断前先让按键观测与当前授权对齐（`ensure_key_tap`）：用户从系统设置勾完权限切回来，
/// 这一次读取就把 tap 建起来并报"已生效"，不必等下一轮权限轮询。
#[tauri::command]
#[specta::specta]
pub(crate) fn shortcut_diagnostics(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<ShortcutDiagnosticDto>, String> {
    let settings = state.settings_snapshot();
    let failures = crate::shortcut_registration_failures(&app);
    let tap = crate::ensure_key_tap(&app);
    let mut out = Vec::new();
    for key in luna_config::shortcut::shortcut_keys() {
        let code = if let Some(code) = failures.get(key) {
            Some((*code).to_owned())
        } else {
            let binding = settings
                .get_str(key)
                .and_then(|value| luna_config::shortcut::parse_binding(&value).ok());
            match binding {
                Some(binding) if binding.double_tap && crate::is_global_shortcut_setting(key) => {
                    match tap {
                        crate::KeyTapStatus::Running => binding
                            .combo
                            .is_reserved_system()
                            .then(|| "reserved_system".to_owned()),
                        crate::KeyTapStatus::AccessibilityMissing => {
                            Some("accessibility_required".to_owned())
                        }
                        crate::KeyTapStatus::Failed => Some("tap_failed".to_owned()),
                        crate::KeyTapStatus::Unsupported => {
                            Some("double_tap_unsupported".to_owned())
                        }
                    }
                }
                Some(binding) => binding
                    .combo
                    .is_reserved_system()
                    .then(|| "reserved_system".to_owned()),
                None => None,
            }
        };
        if let Some(code) = code {
            out.push(ShortcutDiagnosticDto {
                key: key.to_owned(),
                code,
            });
        }
    }
    Ok(out)
}

/// 同步原生窗口外观（NSWindow/NSPanel 边框与原生控件明暗）。
///
/// 前端 theme.ts 负责 webview 内明暗；此命令把解析后的明暗（`"dark"`/`"light"`/`"system"`）
/// 投影到 App 级 `NSAppearance`（TDD-11 §主题）。非 macOS 平台为 no-op。
#[tauri::command]
#[specta::specta]
pub(crate) fn set_native_theme(app: tauri::AppHandle, theme: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        app.run_on_main_thread(move || {
            // SAFETY: run_on_main_thread 保证在主线程执行 AppKit 调用。
            unsafe {
                crate::platform::macos::apply_app_appearance(&theme);
            }
        })
        .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (&app, &theme);
        Ok(())
    }
}

/// 同步后端 Fluent 语言（前端解析 `system` 后传入具体语言）。
#[tauri::command]
#[specta::specta]
pub(crate) fn set_backend_language(app: tauri::AppHandle, language: String) -> Result<(), String> {
    let Some(i18n) = backend_copy_arc(&app) else {
        return Err(coded_error(
            &app,
            "BACKEND_I18N_UNAVAILABLE",
            "backend-i18n-unavailable",
        ));
    };
    i18n.set_language(language);
    // 已存在的 HUD 窗口标题随语言换（E3）。
    crate::retitle_hud_windows(&app);
    // 能力提示词的 locale 变体与灰度受众跟着界面语言走（ADR-050）：注册表只认这一个
    // 事实源，而已缓存的目录要按新受众**重选一次**变体——不联网，只对本地副本重跑选择。
    let resolved = i18n.language();
    luna_core::prompt::registry().set_locale(Some(resolved));
    if let Some(catalogs) = app.try_state::<luna_cloud::commands::Catalogs>() {
        catalogs.reselect();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_preview_tracks_scope_values_and_vault_identity() {
        let mut settings = Settings::with_defaults();
        settings.set("appearance.theme", "dark");
        settings.set("appearance.font_scale", "1.2");
        let plan = setting_group_reset_plan(
            &settings,
            "appearance",
            ResetScope::All,
            Some(Path::new("/a")),
        )
        .unwrap();
        assert!(!plan.changes.is_empty());
        assert!(plan
            .changes
            .iter()
            .all(|item| item.modified && item.resettable && item.available));
        assert_eq!(
            plan.fingerprint,
            setting_group_reset_plan(
                &settings,
                "appearance",
                ResetScope::All,
                Some(Path::new("/a"))
            )
            .unwrap()
            .fingerprint
        );
        assert_ne!(
            plan.fingerprint,
            setting_group_reset_plan(
                &settings,
                "appearance",
                ResetScope::All,
                Some(Path::new("/b"))
            )
            .unwrap()
            .fingerprint
        );
        settings.set("appearance.theme", "light");
        assert_ne!(
            plan.fingerprint,
            setting_group_reset_plan(
                &settings,
                "appearance",
                ResetScope::All,
                Some(Path::new("/a"))
            )
            .unwrap()
            .fingerprint
        );
        let standard =
            setting_group_reset_plan(&settings, "appearance", ResetScope::Standard, None).unwrap();
        assert!(standard
            .changes
            .iter()
            .all(|item| item.visibility == "standard"));
        let dtos = settings_spec_dtos(&settings).unwrap();
        for key in ["storage.vault_root", "storage.models_root"] {
            assert!(!dtos.iter().find(|item| item.key == key).unwrap().resettable);
        }
    }

    #[test]
    fn deletion_warning_follows_the_authoritative_current_strategy() {
        let mut settings = Settings::with_defaults();
        for mode in ["trash", "vault_trash", "permanent"] {
            settings.set("vault.delete_mode", mode);
            let dtos = settings_spec_dtos(&settings).unwrap();
            let spec = dtos
                .iter()
                .find(|item| item.key == "vault.delete_mode")
                .unwrap();
            assert_eq!(
                spec.desc_i18n_key,
                format!("settings:vault.delete_mode_desc_{mode}")
            );
            assert!(matches!(
                spec.desc_presentation,
                SettingDescPresentationDto::Inline
            ));
        }
    }

    #[test]
    fn an_existing_collision_is_reported_on_both_shortcuts() {
        // W3b：升级加了新的默认键，可能撞上用户早就绑好的全局键（真机现场：语音输入 = ⌘E）。
        let mut settings = Settings::with_defaults();
        settings.set("shortcuts.voice_input", "cmd_or_ctrl+e");
        let dtos = settings_spec_dtos(&settings).expect("settings projection");
        let conflict = |key: &str| {
            dtos.iter()
                .find(|spec| spec.key == key)
                .unwrap()
                .shortcut_conflict
                .clone()
        };
        assert_eq!(
            conflict("shortcuts.voice_input").as_deref(),
            Some("shortcuts.toggle_reading_view")
        );
        assert_eq!(
            conflict("shortcuts.toggle_reading_view").as_deref(),
            Some("shortcuts.voice_input")
        );
        assert_eq!(conflict("shortcuts.command_palette"), None);
        assert_eq!(conflict("appearance.theme"), None, "非快捷键项恒为空");
    }

    #[test]
    fn settings_projection_compares_ordered_values_and_owns_capability_availability() {
        let mut settings = Settings::default();
        let initial = settings_spec_dtos(&settings).expect("settings projection");
        assert!(
            initial
                .iter()
                .find(|spec| spec.key == "ai.route_strategy")
                .unwrap()
                .resettable
        );
        assert!(
            !initial
                .iter()
                .find(|spec| spec.key == "ai.assignment.default_llm")
                .unwrap()
                .resettable
        );
        assert!(
            !initial
                .iter()
                .find(|spec| spec.key == "settings.advanced_mode")
                .unwrap()
                .resettable
        );
        let list = initial
            .iter()
            .find(|spec| spec.key == "workspace.quick_start_actions")
            .expect("ordered setting");
        assert!(!list.modified);
        let default = settings.get(&list.key).unwrap_or_else(|| {
            setting_spec(&list.key)
                .expect("registered")
                .default
                .to_value()
        });
        settings.set(&list.key, default.clone());
        assert!(
            !settings_spec_dtos(&settings)
                .expect("projection")
                .iter()
                .find(|spec| spec.key == list.key)
                .expect("ordered setting")
                .modified
        );
        let mut reordered = default.as_string_list().expect("string list").to_vec();
        reordered.reverse();
        settings.set(&list.key, SettingValue::StringList(reordered));
        assert!(
            settings_spec_dtos(&settings)
                .expect("projection")
                .iter()
                .find(|spec| spec.key == list.key)
                .expect("ordered setting")
                .modified
        );
        assert!(initial
            .iter()
            .filter(|spec| spec.key.starts_with("shortcuts.desktop_lens"))
            .all(|spec| !spec.available));
        settings.set("labs.translate_space_enabled", SettingValue::Bool(true));
        assert!(settings_spec_dtos(&settings)
            .expect("projection")
            .iter()
            .filter(|spec| spec.key.starts_with("shortcuts.desktop_lens"))
            .all(|spec| spec.available));
    }

    /// 隐私档、服务端转写开关、服务端地址——三项都必须触发 ASR 重装。
    ///
    /// 这条钉的是**副作用分发器里那几个 key**，而不是重装本身（后者要真 `AppHandle`）。
    /// 上一版三项一个都没挂：改开关要重启才生效，而打开严格本地后已装的云腿不会消失。
    /// 漏挂不会报错，只会让开关看起来生效、实际没有。
    #[test]
    fn asr_reassembly_is_wired_to_every_key_that_can_change_it() {
        let dispatcher_src = source_without_tests(include_str!("settings.rs"));
        let dispatcher = dispatcher_src
            .split("fn apply_setting_side_effects")
            .nth(1)
            .expect("找不到副作用分发器")
            .split("\n/// 设置一个非密钥配置项")
            .next()
            .expect("分发器边界变了");
        for key in ["privacy.strict_local", "provider.remote_asr_enabled"] {
            assert!(
                dispatcher.contains(key),
                "设置项 {key} 改变后不会重装 ASR——它能改变「音频要不要外发」或者「发去哪」"
            );
        }
        assert!(
            dispatcher.contains("sync_outbound_gate"),
            "副作用里没有对齐外发闸——正在跑的会话不会被掐断"
        );
        // 隐私档这一条经 `reconfigure_cloud_stack` 走（闸与 ASR 都在那个入口里）；
        // 转写开关这一条留在本地分支。缺任一半，对应的键改了就等于没改。
        assert!(
            dispatcher.contains("reconfigure_cloud_stack"),
            "隐私档没有路由到落点重装入口——翻「仅本地」后外发闸与 ASR 都不会对齐"
        );
    }

    /// **换服务器必须一次换齐全部消费者**：六个消费者各自独立捕获地址，谁也不会因为
    /// 别人漏了而报错——只切一部分 = 半连状态，且只在下载失败时才现形。
    ///
    /// 读源码断言而不是跑装配：六个消费者要真 `AppHandle` + 真钥匙链 + 真模型库才跑得起来，
    /// 而本测试要拦的是**有人新增第七个消费者却忘了写进来**——那是源码层面的遗漏。
    /// 同手法在 `luna_cloud::commands` 的联网姿态清单里已用过。
    #[test]
    fn switching_servers_reconfigures_every_consumer_at_once() {
        let cloud_src = source_without_tests(include_str!("../cloud.rs"));
        let body = cloud_src
            .split("pub fn reconfigure_cloud_stack")
            .nth(1)
            .expect("找不到换服务端入口")
            .split("\n/// 云是否可用的门控判据")
            .next()
            .expect("入口边界变了");

        // 逐个点名。措辞即缺陷现象——漏掉哪一个，用户看到的就是那一句。
        for (needle, symptom) in [
            (
                "CloudState",
                "账户/会话仍打旧服务端：改完地址还显示着旧账号，第一次真实请求才被踹出去",
            ),
            (
                "catalogs.reconfigure(",
                "目录仍打旧服务端：刷新出来的是旧服务器的模型清单、能力提示词仍是上一台那一版、\
                 「获取更多主题」列的是旧服务器的主题——屏幕上写的服务器与实际用的内容对不上，\
                 而两边都不会报错（ADR-050）。每一份都换到由下一条测试守着",
            ),
            (
                "cloud_artifact::set_address",
                "模型下载仍打旧服务端——最隐蔽的一个，只在下载失败那一刻才现形",
            ),
            (
                "refresh_asr_assembly",
                "转写两条通道仍打旧服务端：音频发去了用户以为已经不再使用的那台机器",
            ),
            (
                "register_llm_provider",
                "对话/翻译仍打旧服务端：正文发去了旧服务器",
            ),
            (
                "catalog_pubkey",
                "公钥没跟着地址换：新服务端的目录用旧公钥验，必然「签名校验失败」，\
                 而用户刚做的动作只是改了个地址",
            ),
            (
                "refresh_embed_provider",
                "语义索引仍在旧落点跑：设置页写着「AI 由服务端提供」，而索引身份没换、\
                 全库重建也不会被触发——两台服务端的向量混进同一份索引，症状只是「搜得\
                 不太准」，不报错也无法归因（TDD-14 §3.3.1 ⑥）",
            ),
        ] {
            assert!(
                body.contains(needle),
                "换服务端时漏了 {needle} —— {symptom}"
            );
        }
        assert!(
            body.contains("sync_outbound_gate"),
            "换服务端时没有对齐外发闸——正在跑的那条腿仍持有旧句柄，掐不断"
        );
    }

    /// **目录的成组动作必须点到每一份目录**（ADR-017 一套基础设施、多份目录）。
    ///
    /// 换服务端、换钥重钉、按受众重选、按龄刷新、用户的「刷新目录」都收在 `luna_cloud::Catalogs`
    /// 里各写一次。漏掉哪一份，那一份的表现都不是报错：换服务端后它仍是上一台的内容、重钉后它
    /// 仍信旧身份验过的副本、「刷新目录」后它**永远刷不到**（「管理台发布了，客户端没变」）。
    /// 读源码断言，同上一条的手法：要拦的是有人新增第四份目录、或新增一个成组动作，却漏写一份。
    #[test]
    fn catalog_group_actions_cover_every_catalog() {
        let commands_src = include_str!("../../../crates/luna-cloud/src/commands.rs");
        let fields: Vec<&str> = commands_src
            .split("pub struct Catalogs {")
            .nth(1)
            .expect("找不到 Catalogs")
            .split("\n}")
            .next()
            .expect("Catalogs 边界变了")
            .lines()
            .filter_map(|line| line.trim().strip_prefix("pub ")?.split(':').next())
            .collect();
        assert_eq!(
            fields,
            ["models", "prompts", "themes", "templates"],
            "目录份数变了：本条与各成组动作一起改"
        );
        let group = commands_src
            .split("impl Catalogs {")
            .nth(1)
            .expect("找不到 Catalogs 的成组动作")
            .split("\n}\n")
            .next()
            .expect("成组动作边界变了");
        let methods: Vec<&str> = group.split("    pub ").skip(1).collect();
        assert_eq!(
            methods.len(),
            6,
            "成组动作的个数变了：确认新的那个也点到了每一份目录"
        );
        for method in methods {
            let name = method.split('(').next().unwrap_or_default();
            for field in &fields {
                assert!(
                    method.contains(&format!("self.{field}.")),
                    "Catalogs 的 {name} 漏了 {field} —— 那一份目录在这个动作里被静默跳过"
                );
            }
        }
        // 「检查更新」（ADR-170 ③）走的就是成组的那一个，而不是自己再点一遍名。
        let check = include_str!("content_market.rs")
            .split("pub(crate) async fn check_content_updates")
            .nth(1)
            .expect("找不到「检查更新」命令")
            .split("\n}")
            .next()
            .expect("命令边界变了");
        assert!(
            check.contains(".refresh_checking_trust("),
            "「检查更新」没有走 Catalogs::refresh_checking_trust —— 漏刷的那一份永远到不了客户端"
        );
        assert!(
            group.contains("self.refresh().await"),
            "refresh_checking_trust 没有走成组的 refresh"
        );
    }

    /// 隐私档翻转后必须重注册对话 Provider——否则它停在翻之前那一个。
    ///
    /// 断言的落点从「分发器里出现 `register_llm_provider`」改成了「隐私档路由到那个唯一
    /// 入口，而入口里有它」：重注册已收进 `reconfigure_cloud_stack`（隐私档牵动的不止对话
    /// 这一个消费者）。**不变量没变，机制变了**——只钉住机制的话，收口重构会让这条测试
    /// 变红而不变量其实完好；只钉住不变量则要跨两个文件断言，故两段各断一半。
    #[test]
    fn flipping_strict_local_reassembles_the_chat_provider() {
        let dispatcher_src = source_without_tests(include_str!("settings.rs"));
        let dispatcher = dispatcher_src
            .split("fn apply_setting_side_effects")
            .nth(1)
            .expect("找不到副作用分发器")
            .split("\n/// 设置一个非密钥配置项")
            .next()
            .expect("分发器边界变了");
        let strict_local_branch = dispatcher
            .split("privacy.strict_local")
            .nth(1)
            .expect("分发器里没有隐私档分支");
        assert!(
            strict_local_branch.contains("reconfigure_cloud_stack"),
            "翻隐私档后不走落点重装入口——对话 Provider 停在翻之前那一个：\
             严格本地下它仍指着远端，关掉后又卡在本机"
        );
        assert!(
            source_without_tests(include_str!("../cloud.rs"))
                .split("pub fn reconfigure_cloud_stack")
                .nth(1)
                .expect("找不到落点重装入口")
                .contains("register_llm_provider"),
            "落点重装入口里没有对话 Provider——隐私档路由过去了也白搭"
        );
    }
}
