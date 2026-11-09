//! IPC 命令/事件/通道。Phase 0 往返（`app_info`/`AppReady`/`start_byte_stream`）+
//! 真实翻译（`popup_generate` / `submit_floating_menu` 经 `JobQueue` 走完整主干，
//! ADR-001/ADR-003）。
//!
//! 事实源：TDD-07 §2、TDD-00 §5.2/ADR-011、TDD-01 §5（JobQueue）。

use luna_config::{
    spec as setting_spec, LoadOutcome, SettingScope, SettingValue, Settings, SETTINGS,
};
use luna_core::{
    builtin_preset, builtin_presets, compose_job_kind, decide_branch, BranchKind, CancelToken,
    ChatMessage, ChatRole, Detection, Instruction, Job, JobEvent, JobKind, JobOrigin, Lang,
    LangContext, LanguageDetector, PartialPayload, Priority, ProviderId, ResultPayload,
    SummaryStyle, TargetLangMode, TextRef,
};
use luna_engine::{WhisperProvider, WHISPER_BATCH_PROVIDER_ID, WHISPER_PROVIDER_ID};
use luna_jobqueue::{
    AdmissionCapacity, ChatSession, ChatSessionConfig, JobHandle, JobQueue, JobSink,
    TranslationConfig,
};
use luna_provider::{HeuristicTokenCounter, LinguaDetector, ProviderRegistry};
use luna_vault::{
    read_meta, write_meta, DeleteMode, MediaRef, NoteDoc, NoteTree, NoteType, PurgeOutcome, Query,
    RestoreConflict, SearchHit, SnapshotMeta, TagSummary, TemplateVars, TrashListEntry,
    TrashPreview, TrashPreviewKind, UsageRange, UsageStore, UsageSummary, UserTemplateInfo,
    UserTemplateVars, Vault, VaultError, VaultWatcher,
};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::arbiter::{MicArbiter, MicHolder};
use crate::media_server::{resolve_note_relative_asset, MediaServer};
use crate::model_store::{
    luna_managed_weight_path, remote_model_spec, spec_weight_path, weight_manifest_path,
    ModelEntryDto, ModelStore, StorageInfoDto,
};
use crate::notifications::{NotificationCenter, NotificationEvent, NotificationEventDto};
use crate::popup::{PopupBlurState, PopupSession};
use crate::recording::{
    RecordingCaptureSource, RecordingController, RecordingEvent, RecordingStartOptions,
    RecordingState,
};
use crate::usage::HistoryCursor;
use crate::usage::{clear_merged_usage, merged_usage_records, merged_usage_summary, AppUsageSink};
use crate::voice_input::{
    merge_glossary_entries, parse_glossary_import, GlossaryEntry, LandingMode, RawBytes,
    VoiceInputController, VoiceInputEvent, VoiceInputMode, VoiceInputOutputKind, VoiceInputSink,
    VoiceInputStartOptions, VoiceInputState, VoiceQuickNoteDraft, VoiceRefineMode,
};
use tauri::ipc::Channel;
use tauri::Manager as _;
use tauri_specta::{collect_commands, collect_events, Builder, Event};

// **后端文案取用口住在自己的模块里**，且经 `pub(crate) use` 进入每个域模块的
// `use super::*`：它们此前挂在 `distill.rs` 上，于是 25 个模块里只有 3 个想起过
// `super::distill::coded_error`，其余一律 `format!("CODE: 中文")`——摩擦正是
// `ipc_error_copy_debt.txt` 那份欠账的主要成因（见 `backend_copy` 头注）。
mod backend_copy;
pub(crate) use backend_copy::*;
mod caption;
use caption::*;
mod create_actions;
use create_actions::*;
mod checkup;
use checkup::*;
mod chat;
pub(crate) use chat::refresh_chat_context_window;
use chat::*;
mod distill;
pub(crate) use distill::spawn_distill_if_configured;
use distill::*;
mod estimate;
use estimate::*;
mod graph;
pub(crate) use graph::spawn_graph_tag_refresh;
use graph::*;
mod embed;
// 落点装配要在 `ipc` 之外用它（`cloud::reconfigure_cloud_stack`）。
pub(crate) use embed::refresh_embed_provider;
use embed::*;
mod ingest;
use ingest::*;
mod knowledge;
use knowledge::*;
mod knowledge_curation;
use knowledge_curation::*;
mod local_engine;
pub(crate) use local_engine::LocalEngineStatusChanged;
use local_engine::*;
mod model_import;
use model_import::*;
mod playback;
// 保存对话框的唯一一份实现：云插件的账户数据导出（纵切 3 C6）经装配注入，同样走它。
pub(crate) use playback::pick_export_destination;
use playback::*;
mod processing;
use processing::*;
mod queue;
use queue::*;
pub(crate) mod recording;
pub(crate) use recording::post_stop_pending;
use recording::*;
mod search;
use search::*;
mod transcribe;
use transcribe::*;
mod tts;
use tts::*;
mod desktop_note;
mod panel;
pub(crate) use desktop_note::*;
use panel::*;
pub(crate) mod account;
pub(crate) use account::{handle_incoming_deep_link, AccountChanged};
mod popup;
mod providers;
use popup::*;
mod shell;
use shell::*;
mod translate;
use translate::*;
mod models;
// 只这一个给 `ipc` 之外用（`lib.rs` 的启动核查）；其余留在模块内，见 models.rs。
pub(crate) use models::compute_endpoint_source;
pub(crate) use models::current_chat_compute_location;
use models::*;
mod onboarding;
pub(crate) use onboarding::SetupStatusChanged;
mod footprint;
mod model_relocation;
mod note_export;
mod quick_note;
use footprint::*;
use model_relocation::*;
use note_export::*;
use onboarding::*;
use quick_note::*;
mod vaults;
use vaults::*;
pub(crate) use vaults::{
    close_for_account, current_vault_root, open_for_account, open_startup_vault_when_local,
    recording_blocks_switch, start_index_scan, ModelDownloadProgress,
};
pub(crate) mod settings;
pub(crate) mod themes;
use settings::*;
use themes::*;
// 启动投影入口给 lib.rs 用（glob 导入是私有的，显式再导一次）。
pub(crate) use settings::apply_route_strategy;
mod metrics;
use metrics::*;
pub(crate) use metrics::{
    activate_local_knowledge_metrics, emit_local_knowledge_metrics_availability_changed,
};
mod workspace;
use workspace::*;
pub(crate) use workspace::{close_vault_window, AppReady};
mod rail;
use rail::*;
mod app_release;
mod apps;
mod restart_guard;
pub(crate) use app_release::spawn_startup_check as spawn_release_startup_check;
pub(crate) use app_release::PendingRelease;
use app_release::*;
pub(crate) use apps::require_app;
use apps::*;
mod creator;
mod creator_channel;
mod creator_distribute;
mod creator_publish;
mod creator_quote;
mod creator_research;
mod creator_sources;
mod creator_swipe;
mod doc_import;
mod web_clip;
use creator_quote::*;
use creator_swipe::*;
use doc_import::*;
use web_clip::*;
mod teleprompter;
use creator::*;
use creator_channel::*;
use creator_distribute::*;
use creator_publish::*;
use creator_research::*;
use creator_sources::*;
pub(crate) use popup::read_clipboard_text;
use teleprompter::*;
pub(crate) use teleprompter::{mark_from_hotkey, send_text_to_teleprompter, TELEPROMPTER_LABEL};
mod glossary;
use glossary::*;
/// 信任锚：导入组织配置文件 / 确认一次变更（ADR-060、ADR-071）。
pub(crate) mod trust;
use trust::*;
mod notes;
use notes::*;
mod quick_note_service;
mod spelling;
mod word_goal;
pub(crate) use quick_note_service::install_quick_note_service;
use spelling::*;
use word_goal::*;
mod editor;
pub(crate) use editor::*;
mod usage;
use usage::*;
mod relocation;
use relocation::*;
mod switcher;
use switcher::*;
mod attachments;
mod bookmarks;
pub(crate) mod content_market;
mod duplicate;
mod exclusions;
mod file_import;
mod interop;
mod recipes;
mod template_insert;
use recipes::*;
pub(crate) mod template_library;
pub(crate) mod template_market;
mod vault_links;
use attachments::*;
use bookmarks::*;
use content_market::*;
use duplicate::*;
use exclusions::*;
use file_import::*;
use interop::*;
use template_insert::*;
use template_library::*;
use template_market::*;
use vault_links::*;
mod external_renames;
use external_renames::*;
mod conflicts;
use conflicts::*;
mod vault_files;
use vault_files::*;
mod tag_edit;
use tag_edit::*;
mod lens;
pub(crate) use lens::run_desktop_lens_capture;
use lens::*;
mod quick_note_refine;
mod voice;
// 官方同步的命令面（ADR-148）。
mod sync_cloud;
mod sync_first_login;
mod sync_ipc;
pub(crate) use caption::{
    remember_caption_window_geometry, remembered_caption_window_geometry, CaptionWindowGeometry,
    CaptionWindowHidden,
};
use sync_cloud::*;
use sync_first_login::*;
use sync_ipc::*;
use voice::*;
pub(crate) use voice::{show_voice_hud_with_preset, VoiceInputHudPresetDto};
/// 上次会话未完成的处理任务已恢复为排队态 → 拉起 worker 续跑（PRD-05-012）。
pub fn resume_processing_queue(app: tauri::AppHandle) {
    start_processing_worker_if_idle(&app);
}

/// 启动清理：抹掉上次会话遗留的抓取临时区（PRD-05-006）。
///
/// 崩溃/强退时 `process_fetch` 的清理来不及跑，半个视频（数百 MB）会静默留在缓存目录里。
/// 放在启动而非退出：退出路径不可靠（被 kill 时根本不执行），而启动是必经之路。
/// 续跑的抓取任务整件重下（与 whisper 无检查点同理），故清空不会丢进度。
pub fn purge_fetch_temp(app: &tauri::AppHandle) {
    purge_fetch_workspace(app);
}

/// 启动时回填上次会话的失败 / 取消项（PRD-04-062）。
///
/// 与索引扫描同一个坑、同一条理由：**启动装载不经 `bind_vault`**，
/// 只挂在切库那一处，历史就只对「这次启动换过库」的人出现——而这条需求的原句是
/// 「关掉应用第二天打开还想知道昨天哪几件没成」，那恰恰是**不换库**的路径。
pub fn restore_queue_history(app: &tauri::AppHandle) {
    processing::restore_queue_history(app);
}

use providers::RemoteBatchAsr;
pub use providers::{build_state_with_settings_persistence, models_root_from_settings};

/// 此刻的本地时刻：文件命名规则（新笔记兜底名、冲突副本日期）要的年月日时分秒，格式由
/// `luna_sync_core::naming` 写出——与网页版同一份。
pub(crate) fn civil_now() -> luna_sync_core::naming::CivilTime {
    use chrono::{Datelike as _, Timelike as _};
    let now = chrono::Local::now();
    luna_sync_core::naming::CivilTime {
        year: now.year(),
        month: now.month(),
        day: now.day(),
        hour: now.hour(),
        minute: now.minute(),
        second: now.second(),
    }
}

const LUNA_VAULT_ROOT_ENV: &str = "LUNA_VAULT_ROOT";
const LUNA_VAULT_PRIVATE_DIR: &str = ".luna";
/// 打开库前等 iCloud 把 `.luna/` 取回本机的上限（启动装载与切库同一个值）：正常取回一个小文件
/// 不到一秒；超过这个时间多半是取回卡住了，再等下去窗口就一直出不来。
const VAULT_CLOUD_FETCH_WAIT: std::time::Duration = std::time::Duration::from_secs(5);
/// 没等到时转入后台等待（[`open_startup_vault_when_local`]），隔多久看一次：只看文件标记，代价可忽略；
/// 取回到了最多晚这么久自动打开。
const VAULT_CLOUD_POLL: std::time::Duration = std::time::Duration::from_secs(2);
/// 时间戳式默认文件名中的内容摘要长度上限（字符，语音速记与对话归档共用）。
/// 时间戳承担主要辨识职责，摘要只做"这条说了什么"的提示，取短防止文件名爆长。
pub(crate) const NOTE_STEM_EXCERPT_CHARS: usize = 16;

/// 应用基本信息（命令响应）。`capabilities` 来自 `luna-core::Capability::ALL`。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AppInfo {
    /// 产品名。
    pub name: String,
    /// 版本。
    pub version: String,
    /// 产出这一份可执行文件的 Git 提交 SHA（由 `build.rs` 注入，禁止由前端猜测）。
    pub git_sha: String,
    /// 云端 wire 协议版本（`luna_cloud::protocol::CLIENT_PROTOCOL` 是唯一事实源）。
    pub protocol_version: u32,
    /// 构建时刻（UTC RFC 3339；可复现构建由 `SOURCE_DATE_EPOCH` 固定）。
    pub build_time: String,
    /// 本构建声明的能力名（snake_case，来自 luna-core，单一来源）。
    pub capabilities: Vec<String>,
}

/// 控件契约 DTO（注册表 [`luna_config::SettingControl`] 的投影）。
///
/// 前端按 `kind` 分发渲染，选项集合/取值范围/单位全部随规格下发——**前端不再持有第二份
/// 选项表**（ADR-003；此前 `settingOptions(key)` 在前端硬编码了 20 余个键的选项集合，
/// 与 Rust 校验各说各话）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum SettingControlDto {
    /// 开关。
    Toggle,
    /// 枚举单选（标签 i18n key = `option.<setting key>.<value>`）。
    Select {
        /// 合法取值集合。
        options: Vec<String>,
        /// 形态（`dropdown` / `segmented`）：由注册表声明，前端不按文案长度猜。
        style: String,
    },
    /// 整数（滑块 / 步进器）。
    Number {
        /// 下界（含）。
        min: i32,
        /// 上界（含）。
        max: i32,
        /// 步长。
        step: i32,
        /// 单位语义（`percent` / `millisecond` / `gibibyte` …）。
        unit: String,
        /// 显示比例，原值除以它；不改变写入值、范围与步长。
        display_divisor: i32,
        /// 显示单位由 Rust 给出，前端不按 key 或单位猜换算。
        display_unit: String,
        /// 形态（`slider` / `stepper`）。
        style: String,
    },
    /// 小数（字符串存储）。
    Decimal {
        /// 下界（含）。
        min: f64,
        /// 上界（含）。
        max: f64,
        /// 步长。
        step: f64,
        /// 单位语义。
        unit: String,
    },
    /// 自由文本。
    Text {
        /// 这一项有没有 Rust 渲染的实时预览（`null` = 没有）。只声明「有预览、问哪条命令」，
        /// 不导出文本格式与校验规则——校验仍只在写入时由 Rust 裁决（2026-10-08 C47）。
        preview: Option<TextPreviewDto>,
        /// 一段话（多行文本框，`TextFormat::Paragraph`）；单行框装不下。
        multiline: bool,
    },
    /// 路径（`absolute_dir` 给系统选择器 / `vault_relative_dir` 只收库内相对路径）。
    Path {
        /// 路径语义（字段不叫 `kind`：那是本联合的判别标签名）。
        path_kind: String,
    },
    /// 快捷键（录制器）。`scope`：`global`（任意应用前台都响应）/ `in_app`（仅本应用有焦点时）。
    /// 两者都支持双击触发；前端不再按 key 维护"哪些键能双击"的第二份名单。
    Shortcut {
        /// 响应方。
        scope: String,
        /// 触发方式编辑器的显示层级。
        trigger_mode_visibility: String,
    },
    /// 模型分配（候选、能力和就绪状态由 Rust 分配服务下发）。
    ModelAssignment {
        /// 槽位。
        slot: String,
    },
    /// 音频输入设备（候选来自系统枚举）。
    AudioInputDevice,
    /// 有序多选（`string_list`）：有限选项、无重复、顺序有意义（TDD-05 §7.5a）。
    ///
    /// **不导出 `require_each_category`**：那是**领域约束**而不是控件形态，导给前端
    /// 只会诱使它复制一份校验；最终合法性只由 Rust 裁决，前端整数组提交、就地解释拒绝码。
    OrderedMultiSelect {
        /// 合法选项全集（与 Rust 校验同源）。
        options: Vec<String>,
        /// 至少几项。
        min: u32,
        /// 至多几项。
        max: u32,
    },
    /// 有序开关列表（`string_list`）：全集有序、每项可开关、锁定项不可关（PRD-07-003 /
    /// TDD-05 §7.5a）。值是 token 列表：`<id>` = 显示，`<hidden_prefix><id>` = 隐藏。
    OrderedToggleList {
        /// 全部选项（顺序 = 默认顺序），名字键由注册表给出、前端直接 `t()`。
        options: Vec<ToggleOptionDto>,
        /// 隐藏项的值前缀——前端据此读写 token，不自己写死这个字面量。
        hidden_prefix: String,
        /// 至少显示几项（锁定项计入）；`0` = 不设下限。前端据此不让关掉最后几项。
        min_visible: u32,
    },
}

/// 有序开关列表里的一项。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ToggleOptionDto {
    /// 稳定 id（token 的主体）。
    pub id: String,
    /// 名字键（`ns:key`）——指向该对象已有的那一个名字，设置页不另起标签。
    pub label_key: String,
    /// 锁定：可排序、不可关。
    pub locked: bool,
    /// 默认是否显示（设置值里缺了这一项时取它；ADR-162 ① 创作空间默认隐藏）。
    pub default_visible: bool,
}

impl From<&luna_config::SettingControl> for SettingControlDto {
    fn from(control: &luna_config::SettingControl) -> Self {
        use luna_config::SettingControl as C;
        match *control {
            C::Toggle => Self::Toggle,
            C::Select { options, style } => Self::Select {
                options: options.iter().map(|o| (*o).to_owned()).collect(),
                style: style.as_str().to_owned(),
            },
            C::Number {
                min,
                max,
                step,
                unit,
                style,
            } => Self::Number {
                // 区间由注册表声明且已被单测约束在 i32 内（设置 IPC 以 TS number 表达整数）。
                min: i32::try_from(min).unwrap_or(i32::MIN),
                max: i32::try_from(max).unwrap_or(i32::MAX),
                step: i32::try_from(step).unwrap_or(1),
                unit: unit.as_str().to_owned(),
                display_divisor: unit.display_divisor(),
                display_unit: unit.display_unit().to_owned(),
                style: style.as_str().to_owned(),
            },
            C::Decimal {
                min,
                max,
                step,
                unit,
            } => Self::Decimal {
                min,
                max,
                step,
                unit: unit.as_str().to_owned(),
            },
            // 文本格式是**取值契约**，不是控件形态：前端渲染的仍是同一个文本框，
            // 合法性由 Rust 在写入时裁决并把归一化后的值回传（`luna_config::normalize`）。
            // 把 format 也导出到 TS 会诱使前端复制一份校验——那正是本仓一路在拆的东西。
            C::Text { format } => Self::Text {
                preview: match format {
                    luna_config::TextFormat::ExportFilenameTemplate => {
                        Some(TextPreviewDto::ExportFilename)
                    }
                    _ => None,
                },
                multiline: matches!(format, luna_config::TextFormat::Paragraph { .. }),
            },
            C::Path { kind } => Self::Path {
                path_kind: kind.as_str().to_owned(),
            },
            C::Shortcut { scope } => Self::Shortcut {
                scope: scope.as_str().to_owned(),
                trigger_mode_visibility: luna_config::SettingVisibility::Advanced
                    .as_str()
                    .to_owned(),
            },
            C::ModelAssignment { slot } => Self::ModelAssignment {
                slot: slot.as_str().to_owned(),
            },
            C::AudioInputDevice => Self::AudioInputDevice,
            C::OrderedMultiSelect {
                options, min, max, ..
            } => Self::OrderedMultiSelect {
                options: options.iter().map(|o| (*o).to_owned()).collect(),
                min: u32::try_from(min).unwrap_or(0),
                max: u32::try_from(max).unwrap_or(u32::MAX),
            },
            C::OrderedToggleList {
                options,
                hidden_prefix,
                min_visible,
            } => Self::OrderedToggleList {
                options: options
                    .iter()
                    .map(|o| ToggleOptionDto {
                        id: o.id.to_owned(),
                        label_key: o.label_key.to_owned(),
                        locked: o.locked,
                        default_visible: o.default_visible,
                    })
                    .collect(),
                hidden_prefix: hidden_prefix.to_owned(),
                min_visible: u32::try_from(min_visible).unwrap_or(u32::MAX),
            },
        }
    }
}

/// 一个设置键呈现在哪。
///
/// 取代此前的 `hidden_in_list: bool`——那个布尔把两件语义相反的事压成一个值：内部键是
/// 「界面上根本没有它」，面板承载的键是「看得见、改得了，只是入口在别处」。压平之后
/// **面板承载的键连搜索都命中不到**：转写同意（一个隐私开关）此前是设置行、搜得到，
/// 归给算力卡片之后就搜不到了。用户搜「上传」「转写」找不到它，得到的结论是"没这功能"。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SettingSurfaceDto {
    /// 普通设置行。
    Row,
    /// 由分组专属面板 / 卡片承载：不渲染成行，但搜索照常命中，命中后走 `route` 深链过去。
    Panel {
        /// `settings://<分组>/<key>`——切组 + 揭示高级 + 滚动定位到承载它的那一格。
        /// 由 Rust 拼是因为「哪个分组承载它」是注册表知识；前端拼等于把它抄第二遍。
        route: String,
    },
    /// 内部状态与全局投影控件：界面上没有它的位置，搜索也不该命中。
    Internal,
}

impl SettingSurfaceDto {
    fn of(spec: &luna_config::SettingSpec) -> Self {
        match luna_config::setting_surface(spec.key) {
            luna_config::SettingSurface::Row => Self::Row,
            luna_config::SettingSurface::Internal => Self::Internal,
            luna_config::SettingSurface::Panel => Self::Panel {
                route: format!(
                    "settings://{}/{}",
                    luna_config::setting_page(spec.key).as_str(),
                    spec.key
                ),
            },
        }
    }
}

/// 文本设置的实时预览种类（每种对应一条只读预览命令）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum TextPreviewDto {
    /// 导出文件名：`preview_export_filename(模板, 示例名)`。
    ExportFilename,
}

/// 设置说明的呈现方式（`luna_config::SettingDescPresentation` 的投影）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SettingDescPresentationDto {
    /// 常显一句（影响、风险、作用范围）。
    Inline,
    /// 按需展开：行内给可聚焦的帮助入口；搜索命中或深链定位时展开。
    Disclosure,
    /// 不显示常态说明：标签与控件已说清；说明仍参与搜索。
    LabelOnly,
}

impl From<luna_config::SettingDescPresentation> for SettingDescPresentationDto {
    fn from(value: luna_config::SettingDescPresentation) -> Self {
        match value {
            luna_config::SettingDescPresentation::Inline => Self::Inline,
            luna_config::SettingDescPresentation::Disclosure => Self::Disclosure,
            luna_config::SettingDescPresentation::LabelOnly => Self::LabelOnly,
        }
    }
}

/// 设置规格 DTO：前端只消费注册表投影，不复制业务逻辑。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SettingSpecDto {
    /// 稳定设置 key。
    pub key: String,
    /// 设置中心一级分组。
    pub group: String,
    /// 配置归属层。
    pub scope: String,
    /// 注册表默认值。
    pub default_value: SettingValueDto,
    /// 当前生效值（缺省回落默认）。
    pub current_value: SettingValueDto,
    /// 当前值是否偏离注册表默认值，由 Rust 按有序值比较。
    pub modified: bool,
    /// 所属能力当前是否可用；显示层不重复判定实验室依赖。
    pub available: bool,
    /// 是否属于通用恢复默认范围；专属领域配置通过其领域命令恢复。
    pub resettable: bool,
    /// 是否需重启生效。
    pub requires_restart: bool,
    /// 这一项呈现在哪：设置行 / 由某个面板承载（带深链）/ 内部键。
    pub surface: SettingSurfaceDto,
    /// 只读显示层级；不反映设置是否运行或是否已修改。
    pub visibility: String,
    /// 组内分节 id（`PRD-09-070` ⑦；`None` = 该组不分节）。节标题文案走
    /// `settings:section.<group>.<id>`，渲染顺序即注册表顺序。
    pub section: Option<String>,
    /// i18n 标签 key。
    pub i18n_key: String,
    /// i18n 说明 key（注册表约定派生：标签 key + `_desc`）。
    pub desc_i18n_key: String,
    /// 说明默认怎么呈现（常显 / 按需展开 / 不显示）。由注册表逐键声明，前端穷尽消费，
    /// 不按文案长度或 key 前缀猜；说明资源本身仍参与搜索（2026-10-08 D1）。
    pub desc_presentation: SettingDescPresentationDto,
    /// 控件契约（怎么编辑 + 合法取值）。
    pub control: SettingControlDto,
    /// 快捷键项：此刻的键位与哪一项**已经**撞键（那一项的 key；W3b）。改键时的冲突当场拒绝，
    /// 这里说的是早已存在的——升级加了新的默认键、或配置是手改的。非快捷键项恒为 `null`。
    pub shortcut_conflict: Option<String>,
}

/// 整组恢复的显示范围。Rust 按注册表算实际 key，前端绝不从 DOM 拼列表。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ResetScope {
    Standard,
    All,
}

/// 快捷键运行诊断 DTO（PRD-09-003 ②③）。
///
/// 内部冲突在写入时即被拒绝（`set_setting`），不出现在这里；系统保留键按静态表判定；
/// 他应用占用无法预检（操作系统不提供枚举接口），只能在注册时探测失败后记在这里。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ShortcutDiagnosticDto {
    /// 设置 key。
    pub key: String,
    /// 稳定 code：`unavailable`（被他应用占用/注册失败）/ `invalid`（解析失败）/
    /// `reserved_system`（系统保留键，警告但不阻止）/ `accessibility_required`（双击型
    /// 绑定需要辅助功能权限，按键观测尚未启动）/ `tap_failed`（已授权但按键观测启动失败）/
    /// `double_tap_unsupported`（本平台尚未接入按键观测）。
    pub code: String,
}

/// 设置变更事件。由 Rust 统一广播给所有 webview，避免多窗口语言/主题状态漂移。
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[tauri_specta(event_name = "settings://changed")]
pub struct SettingsChanged {
    /// 变更的设置 key。
    pub key: String,
}

/// 译境运行态变更。只投影“会话是否仍在运行”，不携带区域、截图或 OCR 内容。
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[tauri_specta(event_name = "desktop-lens://changed")]
pub struct DesktopLensChanged {
    /// `true` 表示镜头会话正在运行（即使用户刚点拍摄而镜框暂时隐藏）。
    pub active: bool,
}

pub(crate) fn emit_desktop_lens_changed(app: &tauri::AppHandle, active: bool) {
    let _ = (DesktopLensChanged { active }).emit(app);
}

/// Vault note type DTO（kebab-case，与 frontmatter 口径一致）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(rename_all = "kebab-case")]
pub enum NoteTypeDto {
    /// 普通笔记。
    Note,
    /// 语音速记。
    QuickNote,
    /// 录音。
    Recording,
    /// 媒体转写。
    Transcript,
    /// 对话。
    Chat,
    /// 导入的文档（ADR-164）。
    Document,
    /// 视频项目主页（创作空间，ADR-162）。
    VideoProject,
    /// 视频脚本。
    Script,
    /// 灵感条目。
    Swipe,
}

impl From<NoteType> for NoteTypeDto {
    fn from(value: NoteType) -> Self {
        match value {
            NoteType::Note => Self::Note,
            NoteType::QuickNote => Self::QuickNote,
            NoteType::Recording => Self::Recording,
            NoteType::Transcript => Self::Transcript,
            NoteType::Chat => Self::Chat,
            NoteType::Document => Self::Document,
            NoteType::VideoProject => Self::VideoProject,
            NoteType::Script => Self::Script,
            NoteType::Swipe => Self::Swipe,
        }
    }
}

impl From<NoteTypeDto> for NoteType {
    fn from(value: NoteTypeDto) -> Self {
        match value {
            NoteTypeDto::Note => Self::Note,
            NoteTypeDto::QuickNote => Self::QuickNote,
            NoteTypeDto::Recording => Self::Recording,
            NoteTypeDto::Transcript => Self::Transcript,
            NoteTypeDto::Chat => Self::Chat,
            NoteTypeDto::Document => Self::Document,
            NoteTypeDto::VideoProject => Self::VideoProject,
            NoteTypeDto::Script => Self::Script,
            NoteTypeDto::Swipe => Self::Swipe,
        }
    }
}

/// Vault delete mode DTO。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum DeleteModeDto {
    /// 系统回收站。
    Trash,
    /// Vault 内 `.trash/`。
    VaultTrash,
    /// 永久删除。
    Permanent,
}

impl From<DeleteModeDto> for DeleteMode {
    fn from(value: DeleteModeDto) -> Self {
        match value {
            DeleteModeDto::Trash => Self::Trash,
            DeleteModeDto::VaultTrash => Self::VaultTrash,
            DeleteModeDto::Permanent => Self::Permanent,
        }
    }
}

/// 反向投影：**当前生效的删除策略要下发给前端**（`vault_delete_mode`，PRD-06-009）。
///
/// 回收站页与八处删除确认卡都得知道"这一刀删到哪里去"——此前它们一概假定"删了能从
/// 回收站还原"，而默认策略是**系统废纸篓**（库内 `.trash/` 是空的）、`permanent` 策略下
/// 那句话干脆是错的。判定仍在 Rust（`vault_delete_mode_from_settings` 唯一一处），前端
/// 只按下发的这一枚值渲染。
impl From<DeleteMode> for DeleteModeDto {
    fn from(value: DeleteMode) -> Self {
        match value {
            DeleteMode::Trash => Self::Trash,
            DeleteMode::VaultTrash => Self::VaultTrash,
            DeleteMode::Permanent => Self::Permanent,
        }
    }
}

/// Media ref DTO。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct MediaRefDto {
    /// 媒体相对路径。
    pub path: String,
    /// 时长（秒）。
    pub duration: Option<f64>,
    /// 是否已转写。
    pub transcript: bool,
    /// 是否已转译。
    pub translation: bool,
    /// 转写模型。
    pub model: Option<String>,
    /// 耐久边车相对路径。
    pub segments: Option<String>,
}

impl From<MediaRef> for MediaRefDto {
    fn from(value: MediaRef) -> Self {
        Self {
            path: value.path,
            duration: value.duration,
            transcript: value.transcript,
            translation: value.translation,
            model: value.model,
            segments: value.segments,
        }
    }
}

/// Note metadata DTO。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct NoteMetaDto {
    /// luna schema version。
    pub schema: u32,
    /// 功能类型。
    pub note_type: NoteTypeDto,
    /// 这一篇是**记录态**吗（ADR-086）：页头「记录」徽标与「沉淀为知识」入口据此显示
    /// （PRD-06-052/053）。下发的是**判定结论**而不是 `luna.stage` 原始取值——
    /// 缺省规则（速记无键即记录态）只活在 Rust，前端零业务（ADR-003）。
    pub draft: bool,
    /// 创建时间（RFC3339）。
    pub created: Option<String>,
    /// 媒体映射。
    pub media: Vec<MediaRefDto>,
    /// 字数目标（W6b，`luna.word_goal`）；`null` = 没设。
    pub word_goal: Option<u32>,
}

/// 一篇笔记的**对象级事实**（右侧上下文面板的「这一篇的标签」与「笔记信息」两格，
/// PRD-07-031）。一次取齐：两格问的是同一个对象，分两条命令就是同一次往返拆成两次。
///
/// **刻意不含字数**（`luna_vault::NoteFacts` 的头注给出完整论证）：字数归状态栏
/// （PRD-07-065），且编辑中的实时字数与落盘事实必然有一刻不相等。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct NoteFactsDto {
    /// 功能类型（`luna.type` 的 kebab 键）。
    pub note_type: NoteTypeDto,
    /// 是否记录态（ADR-086 的**判定结论**，前端零业务）。
    pub draft: bool,
    /// 是否来源层（录音 / 转写 / 对话；`NoteType::is_source_material` 的判定结论，PRD-06-048②）。
    pub source_material: bool,
    /// 创建时间（RFC3339 原样字符串）；`null` = frontmatter 里没有这一键。
    pub created: Option<String>,
    /// 索引记录的文件修改时间（Unix 毫秒，字符串避免 i64 精度损失）；
    /// `null` = 这一篇还没进索引，**不猜一个时间**。
    pub modified_ms: Option<String>,
    /// 这一篇的全部标签（用户标签与 AI 标签同池，复用既有索引，不另算一份）。
    pub tags: Vec<String>,
    /// 其中可以在这里就地移除的（写在笔记开头 `tags` 里的，ADR-135）。
    pub removable_tags: Vec<String>,
}

impl From<luna_vault::NoteFacts> for NoteFactsDto {
    fn from(value: luna_vault::NoteFacts) -> Self {
        Self {
            note_type: value.note_type.into(),
            draft: value.draft,
            source_material: value.source_material,
            created: value.created,
            modified_ms: value.modified_ms.map(|ms| ms.to_string()),
            tags: value.tags,
            removable_tags: value.removable_tags,
        }
    }
}

/// Note document DTO。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct NoteDocDto {
    /// 相对路径。
    pub path: String,
    /// 完整 Markdown 内容。
    pub content: String,
    /// 派生 metadata。
    pub meta: NoteMetaDto,
}

impl From<NoteDoc> for NoteDocDto {
    fn from(value: NoteDoc) -> Self {
        Self {
            path: value.path,
            content: value.content,
            meta: NoteMetaDto {
                schema: value.meta.schema,
                note_type: value.meta.note_type.into(),
                draft: value.meta.stage().is_draft(),
                created: value.meta.created,
                media: value.meta.media.into_iter().map(Into::into).collect(),
                word_goal: value.meta.word_goal,
            },
        }
    }
}

/// Vault tree DTO。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct NoteTreeDto {
    /// 节点名。
    pub name: String,
    /// 相对路径。
    pub path: String,
    /// 是否目录。
    pub is_dir: bool,
    /// 修改时间（Unix 毫秒；取不到为 0）。供文件树按修改时间排序（PRD-06-006）。
    pub mtime_ms: f64,
    /// 创建时间（Unix 毫秒；文件系统不提供时为修改时间）。供文件树按创建时间排序（PRD-06-006 ①）。
    pub created_ms: f64,
    /// 它是不是**记录态**（ADR-086；目录恒 `false`）。树行据此淡显 + 挂「记录」徽标，
    /// 树头「隐藏记录」过滤开关也按它筛（PRD-06-052）。判定在 Rust，前端只渲染。
    pub draft: bool,
    /// 它能不能被**「标记为记录」**（PRD-06-058 反向动作；目录与非 `.md` 恒 `false`）。
    /// 树行右键据此决定要不要给这一项——前端不自己拼「知识态且非主题页且非资料」。
    pub markable_as_draft: bool,
    /// 它能不能**生成知识笔记**（= 资料 ∪ 记录，`NoteStage::allows_distill`；目录与非
    /// `.md` 恒 `false`）。树行右键据此决定要不要给这一项——前端**不拿两位布尔去拼**：
    /// 那样同一篇录音在录音清单里有这一项、在文件树里没有（2026-09-02 真机上报）。
    pub distillable: bool,
    /// 它是不是**资料**（来源轴；目录与非 `.md` 恒 `false`）。树行据此给身份徽标——前端
    /// 不再按"路径落在哪个归档目录之下"猜（ADR-003 / PRD-06-048 ⑥）。
    pub is_source_material: bool,
    /// 功能类型；目录为 `null`。行内类型徽标（录音 / 转写 / 对话）据此取词（PRD-06-048①）。
    pub note_type: Option<NoteTypeDto>,
    /// 它能不能作为「多条 → 一篇」的源（`luna_vault::merge_block`，PRD-06-047 修订后
    /// 只挡聚合页）。与 `DraftNoteDto.can_merge` 同一个函数——前端不拿"是不是记录态"
    /// 去顶替它，那种顶替会把资料挡在门外。
    pub can_merge: bool,
    /// 还在 iCloud 上没下载到本机（TDD-06 §4.1 ②）：树上显示成「未下载」，点开时请系统下载。
    pub cloud_placeholder: bool,
    /// 官方同步里只在云端（ADR-156）：点开时由同步服务按需下载。
    pub sync_cloud_only: bool,
    /// 音频或视频文件（`luna_sync::policy::is_media`，ADR-156）：右键「释放本机空间」只给它们。
    /// 前端不按扩展名自己猜。
    pub media: bool,
    /// 被哪条排除规则覆盖（PRD-06-062，V4a）；没被排除为 `null`。树行淡显与右键勾选态据此。
    pub excluded_by: Option<String>,
    /// 隐藏项（自己或上级以 `.` 开头）：只在「显示隐藏文件」时出现，树行淡显（V4b）。
    pub hidden: bool,
    /// 子节点。
    pub children: Vec<NoteTreeDto>,
}

impl From<NoteTree> for NoteTreeDto {
    fn from(value: NoteTree) -> Self {
        let media = !value.is_dir && luna_sync::policy::is_media(&value.path);
        Self {
            name: value.name,
            path: value.path,
            is_dir: value.is_dir,
            mtime_ms: value.mtime_ms as f64,
            created_ms: value.created_ms as f64,
            draft: value.draft,
            markable_as_draft: value.markable_as_draft,
            distillable: value.distillable,
            is_source_material: value.is_source_material,
            note_type: value.note_type.map(Into::into),
            can_merge: value.can_merge,
            cloud_placeholder: value.cloud_placeholder,
            sync_cloud_only: value.sync_cloud_only,
            media,
            excluded_by: value.excluded_by,
            hidden: value.hidden,
            children: value.children.into_iter().map(Into::into).collect(),
        }
    }
}

/// 记录收件箱的一行（PRD-06-058）。
/// 速记侧栏的筛选档（PRD-06-058 A′）。
///
/// 封闭枚举而不是一段查询串：档位的判据（有没有原声 / AI 补过没有 / 放了多久）全在
/// Rust，前端交出去的只有「用户点了哪一档」这个入口事实（ADR-003）。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum DraftFilterDto {
    #[default]
    All,
    WithAudio,
    Refined,
    Stale,
}

impl From<DraftFilterDto> for luna_vault::DraftFilter {
    fn from(value: DraftFilterDto) -> Self {
        match value {
            DraftFilterDto::All => Self::All,
            DraftFilterDto::WithAudio => Self::WithAudio,
            DraftFilterDto::Refined => Self::Refined,
            DraftFilterDto::Stale => Self::Stale,
        }
    }
}

#[derive(Debug, Clone, Serialize, Type)]
pub struct DraftNoteDto {
    /// 笔记相对路径。
    pub path: String,
    /// 标题（= 文件名 stem）。
    pub title: String,
    /// 单行预览（正文首段压成一行）。
    pub preview: String,
    /// 最后修改时间（epoch ms）。TS `number`——时间桶分组与相对时龄都出自它。
    pub modified_ms: f64,
    /// 它是速记（收件箱原生）还是被标记为记录的普通笔记——行首那枚来源小图标的判据。
    pub quick_note: bool,
    /// 语音速记已跑过 AI 提炼（PRD-06-058 C′）：行尾挂「已提炼」。
    pub refined: bool,
    /// 留了原始录音（PRD-02-030）：行首图标改画麦克风，右键给「重新转写」（PRD-02-032）。
    pub has_media: bool,
    /// 正文里还没勾的待办条数（PRD-06-058 G）；`0` 且 `todo_total` 为 0 = 这篇没有待办。
    pub todo_open: u32,
    /// 正文里的待办总条数。
    ///
    /// 两个数一起下发而不是只给「还剩几条」：用户要读的是进度（`2/5`），而「还剩 2 条」
    /// 在一篇只有两条待办的笔记上与「一条都没做」长得一样。
    pub todo_total: u32,
    /// 它此刻钉在桌面上（ADR-119）——右键据此在「钉到桌面 / 取消钉住」之间二选一。
    ///
    /// 下发已判定的布尔而不是那个 frontmatter 键：前端不认识 `luna.desktop_note` 的形状，
    /// 也不该为了一句菜单文案去解析它（ADR-003）。
    pub pinned_to_desktop: bool,
    /// 原始录音总时长（毫秒；没有录音时为 0）——行上「这条有多长」那一格。
    ///
    /// 零新增存储：`media.duration` 建索引时早就写好，这里只是把既有派生索引里那个数捞
    /// 出来（与转写清单同一个 SQL 表达式）。
    pub duration_ms: f64,
    /// 它已经**搁置到该催了**（修改时间早于知识体检的陈化门槛，PRD-06-055 同一常量）。
    ///
    /// 下发布尔而不是天数阈值：门槛是 Rust 的判断，前端拿去比一遍就是第二处会漂的
    /// 定义——而两级提醒（行上轻警示 / 体检积压行）必须永远说同一句话。
    pub stale: bool,
    /// 它能不能参与「多条 → 一篇」（PRD-06-047，判据 `luna_vault::merge_block`）。
    ///
    /// **对象资格，不是通行证**：它只回答"这个对象够不够格"，不承诺文件此刻可读、
    /// 正文非空，也不承诺点确认时它还是这个样子——`merge_notes` 必须用同一个
    /// 谓词重验一次（用户勾选之后、点确认之前把其中一篇沉淀掉是完全可能的）。
    pub can_merge: bool,
}

/// 记录收件箱一次取齐的结果（PRD-06-058）：列表 + 权威总数 + 汇总限额。
#[derive(Debug, Clone, Serialize, Type)]
pub struct DraftInboxDto {
    /// 全库速记总数（活动栏计数徽标用；**不受列表上限截断，也不受侧栏此刻的筛选影响**）。
    ///
    /// 它与 `matched` 是两个数、都不能省：活动栏角标承诺的是「全库还剩几条」
    /// （PRD-07-066），一筛选就跟着变的角标是在说谎；侧栏页头要说的则是「这份筛选下有
    /// 几条」，拿全库总数去配一份筛出来的列表同样对不上。
    pub total: u32,
    /// 当前筛选下的条数（侧栏页头徽标用）。无筛选时与 `total` 相等。
    pub matched: u32,
    /// **这一页**的条目（修改时间倒序）。
    pub notes: Vec<DraftNoteDto>,
    /// 下一页的键集游标；`null` = 已遍历到底（TDD-07 §4.1a）。
    ///
    /// 收件箱的语义是「全部记录态笔记」，不是「最近 N 条」——`total` 说有 700 篇而列表
    /// 只能给出前 N 篇，那个数字就成了摆设。游标不透明，前端只回传。
    pub next_cursor: Option<String>,
    /// 一次汇总最少要选几条（PRD-06-047）。
    ///
    /// **常量归 Rust**：前端照着写一份 `>= 2`，就是同一条规则的第二处定义——改了
    /// 一边不会有任何东西变红，而表现只是"按钮说可以、点下去被拒"。
    pub merge_min: u32,
    /// 一次汇总最多能选几条（与内核的截断上限同一个常量）。
    ///
    /// 有了它，超限就是**当场被拒**而不是内核静默只取前 N 条——后者会把"我选了 300
    /// 条"悄悄变成一篇只含前 200 条的汇总，且产物里看不出少了什么。
    pub merge_max: u32,
}

/// 一篇笔记此刻的成熟度动作可用性（PRD-06-053 / PRD-06-058）。
#[derive(Debug, Clone, Serialize, Type)]
pub struct NoteDraftStateDto {
    /// 它是记录态吗——页头「记录」徽标（= 一键沉淀）的判据。
    pub draft: bool,
    /// 它能被「标记为记录」吗——编辑器右键那一项的判据（判定同树行，见 `NoteTreeDto`）。
    pub markable_as_draft: bool,
}

/// 一段文本要落进**哪个目的地**（PRD-06-058 P0-1 收口）。
///
/// **这是前端唯一有资格回答的问题，也是它在这条路上唯一交出去的东西**：「用户此刻
/// 站在哪个目的地里」是导航状态，前端本来就持有；而「记录该存成什么类型、落哪个目录、
/// 按哪套惯例命名」是**判定**，只在 [`notes::destination_note_plan`] 一处（ADR-003）。
///
/// 反过来做（前端按目的地挑两个不同命令、或直接传类型字面量）等于把业务判定搬进界面层：
/// 侧栏捕获框与空白画布从此可以各自漂，而它们漂开的表现正是本条要修的那件事——用户站在
/// 「记录」里，最显眼的输入区却创建了知识笔记。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum CaptureDestinationDto {
    /// 「笔记库」目的地：动笔写 → 知识笔记（`type: note`，落 `vault.notes_dir`）。
    Notes,
    /// 「记录」目的地：随手记 → 记录（与侧栏捕获框**同一条**落盘路径）。
    Drafts,
}

/// 列表排序键（TDD-07 §4.1a）。前端只回选中的这一个键，排序在 SQL 层完成。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ListSortDto {
    /// 最近修改在前。
    #[default]
    Modified,
    /// 标题 **UTF-8 码点序**——与旧版前端 `localeCompare` 不同，中文标题的先后会变
    /// （`luna_vault::ListSort::Name` 写着为什么必须是它）。
    Name,
    /// 时长（长的在前）。
    Duration,
    /// 「录于何时」倒序（没记创建时间的老笔记退到修改时间，PRD-07-007）。
    Created,
}

impl From<ListSortDto> for luna_vault::ListSort {
    fn from(value: ListSortDto) -> Self {
        match value {
            ListSortDto::Modified => Self::Modified,
            ListSortDto::Name => Self::Name,
            ListSortDto::Duration => Self::Duration,
            ListSortDto::Created => Self::Created,
        }
    }
}

/// 一次**列表语义**的取数（TDD-07 §4.1a）：给了它就是「符合条件的全部条目、可穷尽遍历」。
///
/// 不给它就是既有的 top-N 语义。全文检索路径**不接受**它（相关度序做不出稳定游标），
/// 带着它去查全文会拿到 `LIST_CURSOR_INVALID`。
#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
pub struct ListPageDto {
    pub sort: ListSortDto,
    /// 上一页末行的 `cursor` 原样回传；`null` = 第一页。**不解析、不自行构造**。
    pub cursor: Option<String>,
}

impl From<ListPageDto> for luna_vault::ListPage {
    fn from(value: ListPageDto) -> Self {
        Self {
            sort: value.sort.into(),
            cursor: value.cursor,
        }
    }
}

/// Vault search query DTO。
#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
pub struct SearchQueryDto {
    /// 全文检索词。
    pub text: Option<String>,
    /// 类型过滤。
    pub note_type: Option<NoteTypeDto>,
    /// 标签过滤。
    pub tag: Option<String>,
    /// 路径前缀过滤。
    pub path_prefix: Option<String>,
    /// 标题 / 路径子串筛选（列表控件那只搜索框；空白 = 不筛）。
    ///
    /// 它必须走后端：留在前端的话，分页之后它只筛已加载的那几页，于是"筛出 0 条"
    /// 与"库里没有"变成同一个画面（TDD-07 §4.1a）。
    ///
    /// **可缺省**（TS 里是 `name_like?`）：本次新增的两个字段都不进既有调用方的字面量，
    /// 契约要的「top-N 消费者零改动」才是真的零——补两个 `null` 也是改动。
    #[serde(default)]
    pub name_like: Option<String>,
    /// 上限 = 页大小 / top-N 条数（语义不变，TDD-07 §4.1a）。
    ///
    /// **`null` = 由 Rust 给默认**：列表语义取 `luna_vault::LIST_PAGE_SIZE`，top-N 取 50。
    /// 页大小因此只有 Rust 那一处定义——前端连读都不需要读它，也就没有第二份数字可漂。
    pub limit: Option<u32>,
    /// 列表分页（缺省 = top-N）。可缺省，同 `name_like`。
    #[serde(default)]
    pub page: Option<ListPageDto>,
}

impl From<SearchQueryDto> for Query {
    fn from(value: SearchQueryDto) -> Self {
        Self {
            text: value.text,
            note_type: value.note_type.map(Into::into),
            tag: value.tag,
            path_prefix: value.path_prefix,
            name_like: value.name_like,
            // 前端不给这一档：排除记录态只服务主题页建议计数（PRD-16-016 修订），
            // 搜索面对记录态**不降权不排除**、只分组（PRD-06-052）。
            knowledge_only: false,
            // 同上，且更强：候选筛是**归入/合成**那条路的判据（还要排除主题页与
            // 知识地图），搜索面把它们照常搜出来——用户搜「Datadog」时那一页正是他要找的。
            ingest_candidates_only: false,
            // 0 = 交给 `luna_vault` 按语义选默认（页大小 / top-N 各一档，单一定义）。
            limit: value.limit.unwrap_or(0) as usize,
            page: value.page.map(Into::into),
        }
    }
}

/// Vault search hit DTO。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SearchHitDto {
    /// 笔记相对路径。
    pub path: String,
    /// 标题。
    pub title: String,
    /// 相关度。
    pub score: f64,
    /// 上下文片段。
    pub snippet: String,
    /// 最后修改时间（epoch ms）。TS `number`——列表排序用（PRD-07-007）。
    pub modified_ms: f64,
    /// 关联媒体总时长（ms；0 = 无媒体/未知）。转写列表显示用（排序键在 SQL 层，TDD-07 §4.1a）。
    pub duration_ms: f64,
    /// 创建时间（RFC3339 原样；`null` = frontmatter 无该键）。录音行显示「录于何时」（PRD-07-007）。
    pub created: Option<String>,
    /// 关联媒体是否至少一条已转写。录音行据此挂「未转写」（PRD-07-007）。
    pub transcribed: bool,
    /// `created` 解析成的 Unix 毫秒；`null` = 无键或解析不了。行显示与「录制时间」排序同一个数。
    pub created_ms: Option<f64>,
    /// 下一页的键集游标，**只挂在本页末行**；`null` = 没有下一页（TDD-07 §4.1a）。
    ///
    /// 消费方式恒定一句：`hits.at(-1)?.cursor ?? null`。top-N 语义（请求未给 `page`）
    /// 下恒为 `null`——那条路径给的是「最相关的前 N 条」，本来就没有下一页。
    ///
    /// **不透明**：只回传，不解析、不自行构造。
    pub cursor: Option<String>,
}

impl From<SearchHit> for SearchHitDto {
    fn from(value: SearchHit) -> Self {
        Self {
            path: value.path,
            title: value.title,
            score: value.score,
            snippet: value.snippet,
            modified_ms: value.modified_ms as f64,
            duration_ms: value.duration_ms as f64,
            created: value.created,
            transcribed: value.transcribed,
            created_ms: value.created_ms.map(|ms| ms as f64),
            cursor: value.cursor,
        }
    }
}

/// 标签浏览器层级摘要 DTO（PRD-06-033）。
#[derive(Debug, Clone, Serialize, Type)]
pub struct TagSummaryDto {
    pub tag: String,
    /// 恰好直挂本标签的去重笔记数；中间层可为 0。
    pub direct_count: u32,
    /// 本标签及全部子标签下的去重笔记数；与点开后的过滤结果同口径。
    pub note_count: u32,
    /// 这一支（自身 + 子孙）**只**由 AI 标注产生；判定在 Rust，前端只渲染。
    pub ai_only: bool,
}

impl From<&TagSummary> for TagSummaryDto {
    fn from(summary: &TagSummary) -> Self {
        Self {
            tag: summary.tag.clone(),
            direct_count: summary.direct_count,
            note_count: summary.note_count,
            ai_only: summary.ai_only,
        }
    }
}

/// 用户 Markdown 模板信息（模板注册表的库内那一层）。`path` 永远相对设置的模板目录。
#[derive(Debug, Clone)]
pub(crate) struct UserTemplateInfoDto {
    pub path: String,
    pub name: String,
}

impl From<UserTemplateInfo> for UserTemplateInfoDto {
    fn from(value: UserTemplateInfo) -> Self {
        Self {
            path: value.path,
            name: value.name,
        }
    }
}

/// 笔记引用 DTO（路径，供 Backlinks/Outgoing 面板使用）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct NoteRefDto {
    pub path: String,
}

/// 一条 `[[` 补全候选（`vault_wikilink_candidates`）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct WikiLinkCandidateDto {
    /// 笔记标题（= 文件名去扩展名）。
    pub title: String,
    /// 选中后写进 `[[ ]]` 的文字：按名字指得回这一篇就是名字，否则是不带 `.md` 的路径。
    pub insert: String,
    /// 按 frontmatter 别名命中时的那个别名，选中后写成 `[[insert|别名]]`；按标题命中为 `None`。
    pub alias: Option<String>,
    /// 所在目录（库根为空串）。
    pub folder: String,
    /// 库内相对路径。
    pub path: String,
}

impl From<luna_sync_core::links::WikiLinkCandidate> for WikiLinkCandidateDto {
    fn from(c: luna_sync_core::links::WikiLinkCandidate) -> Self {
        Self {
            title: c.title,
            insert: c.insert,
            alias: c.alias,
            folder: c.folder,
            path: c.path,
        }
    }
}

/// 一条 wikilink 的落点（`vault_resolve_wikilink`）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct WikiLinkHitDto {
    /// 指向的笔记（库内相对路径）；`exists = false` 时是点击后该新建的路径。
    pub path: String,
    /// 库里有没有这一篇：没有 = 点击即新建。
    pub exists: bool,
    /// `#片段` 所在行（1 起，按整个文件计）；无片段或片段已不在为 `None`。
    pub line: Option<u32>,
    /// 片段是标题时它是正文第几个标题（0 起，阅读视图定位用）。
    pub heading_index: Option<u32>,
}

impl From<luna_vault::WikiLinkHit> for WikiLinkHitDto {
    fn from(hit: luna_vault::WikiLinkHit) -> Self {
        Self {
            path: hit.path,
            exists: hit.exists,
            line: hit.line,
            heading_index: hit.heading_index,
        }
    }
}

/// 一条出链：目标路径 + 它此刻在不在库里（外链面板专用）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct OutgoingLinkDto {
    /// 目标的**库内路径**（`links` 表存的就是规范形，schema v7）；越出库根的断链留原样。
    pub path: String,
    /// 库里真有这一篇吗。`false` = 断链，面板呈现为「未创建」且不可点。
    pub exists: bool,
}

/// Snapshot DTO。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SnapshotDto {
    /// 快照 id。
    pub id: String,
    /// 内容 hash。
    pub hash: String,
    /// 创建时间（Unix ms）。
    pub created_ms: String,
    /// 字节数。
    pub size: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct UsageRecordDto {
    pub job_id: String,
    pub origin: String,
    pub capability: String,
    pub provider_id: String,
    /// 这一次**在哪跑的**（`device` / `local_service` / `cloud`）；`null` = 这一列出现之前
    /// 写下的历史行，或者路由阶段就没选出腿。
    ///
    /// 面板导语一直写着「在哪跑的」，而此前这张表上没有任何一格回答它——行里只有
    /// provider id 与模型名，那是两个机器串（ADR-126 第 6 条）。
    pub provider_location: Option<String>,
    pub model_id: Option<String>,
    pub audio_seconds: Option<f32>,
    pub tokens_in: Option<u32>,
    pub tokens_out: Option<u32>,
    pub est_cost_minor: Option<String>,
    pub terminal_status: String,
    /// 触顶收尾时是哪一道闸（`caller` / `upstream` / `absolute`）；`null` = 自然写完。
    ///
    /// 触顶不是失败（ADR-089 ④），`terminal_status` 两边都是 `finished`——不单独下发这一格
    /// 的话，一条被截断的记录在这张表上与一条正常完成的记录**一模一样**。
    pub output_truncated: Option<String>,
    /// 这一次打的是哪一档服务（`realtime` / `batch`，ADR-083 ②）；`null` = 这一列出现
    /// 之前写下的历史行。
    ///
    /// 与 `capability` 一起才构成一条完整的「打了哪条链」：同一个能力的两档在服务端
    /// 是两条独立的候选链，可以一条好一条坏（真机 2026-08-31 就是这个形状）。
    pub service_class: Option<String>,
    /// 失败终态的稳定错误码（`SERVER_SIDE_FAILURE` 之类）；成功与取消为 `null`。
    ///
    /// 失败记录此前只有一个 `failed`，答不出「为什么」——而这张表是本机**唯一**的
    /// 失败历史面（服务端账本只记它受理过的那些）。
    pub error_code: Option<String>,
    pub finished_at_ms: String,
}

impl From<luna_core::UsageRecord> for UsageRecordDto {
    fn from(record: luna_core::UsageRecord) -> Self {
        Self {
            job_id: record.job_id.to_string(),
            origin: serde_json::to_value(record.origin)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_else(|| "unknown".into()),
            capability: serde_json::to_value(record.capability)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_else(|| "unknown".into()),
            provider_id: record.provider_id.to_string(),
            provider_location: record
                .provider_location
                .map(|location| location.as_str().to_owned()),
            model_id: record.model_id.map(|model| model.to_string()),
            audio_seconds: record.usage.audio_seconds,
            tokens_in: record.usage.tokens_in,
            tokens_out: record.usage.tokens_out,
            est_cost_minor: record
                .usage
                .est_cost
                .map(|money| money.minor_units.to_string()),
            terminal_status: record.terminal_status,
            output_truncated: record.output_truncated.map(|by| by.key().to_owned()),
            service_class: record.service_class.map(|class| class.as_str().to_owned()),
            error_code: record.error_code,
            finished_at_ms: record.finished_at_ms.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct UsageSummaryDto {
    pub records: String,
    pub tokens_in: String,
    pub tokens_out: String,
    pub audio_seconds: f64,
    /// 其中触顶收尾的记录数——「我这个月有几篇被截断了」的答案。
    pub truncated: String,
    /// 「本机省下」估算（USD，展示性、**非计费权威**——计费口径在服务端，ADR-039）。
    ///
    /// 在 Rust 侧算好再下发，前端只渲染（ADR-003 前端零业务）。
    /// `None` = 用户没配参考单价，此时**不展示金额**（PRD-07-051）；单价的唯一事实源是
    /// `luna_config` 的价格设置（见 [`setting_price_usd`]）。
    /// 只统计**在本机跑掉的那些**（ADR-126 第 6 条）。
    pub estimated_savings_usd: Option<f64>,
    /// 「服务端跑掉的这些大概值多少」——与上一格同一把尺子、方向相反。
    pub estimated_server_cost_usd: Option<f64>,
    /// 笔记 AI 结果落地回执（PRD-06-021）：进了正文的份数 / 丢弃的份数。
    pub ai_landed: String,
    pub ai_discarded: String,
    /// 接受率（百分比，四舍五入）；一份回执都没有时 `None`。在 Rust 算好，前端只渲染。
    pub ai_accept_rate_pct: Option<u32>,
}

impl UsageSummaryDto {
    fn from_summary(
        summary: UsageSummary,
        estimated_savings_usd: Option<f64>,
        estimated_server_cost_usd: Option<f64>,
        outcomes: luna_vault::AiOutcomeSummary,
    ) -> Self {
        let total = outcomes.landed + outcomes.discarded;
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        let ai_accept_rate_pct =
            (total > 0).then(|| ((outcomes.landed as f64 / total as f64) * 100.0).round() as u32);
        Self {
            records: summary.records.to_string(),
            tokens_in: summary.tokens_in.to_string(),
            tokens_out: summary.tokens_out.to_string(),
            audio_seconds: summary.audio_seconds,
            truncated: summary.truncated.to_string(),
            estimated_savings_usd,
            estimated_server_cost_usd,
            ai_landed: outcomes.landed.to_string(),
            ai_discarded: outcomes.discarded.to_string(),
            ai_accept_rate_pct,
        }
    }
}

/// 参考单价（USD）——「本地节省」估算用，**非计费**。
///
/// **没配就是没配**（PRD-07-051）：`None` = 用户没在设置里给过参考单价。不编一个厂商
/// 报价兜底——那会让概览拿一个用户从没设过、也看不到出处的单价算出金额给他看。
/// 存了但解析不了（手改成非数值）同样按未配置处理：拿坏值算出的数比不显示更坏。
fn setting_price_usd(settings: &Settings, key: &str) -> Option<f64> {
    settings.get_str(key).and_then(|raw| {
        raw.trim()
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite() && *v >= 0.0)
    })
}

/// 「本地节省」估算：这些用量若走云端参考价要花多少钱（USD）。
///
/// **展示性、非计费权威**（ADR-039 计费口径在服务端）。用 f64 而非 `luna_core::Money`：
/// 估算要精到分以下（$0.00x），而 Money 是整数最小货币单位、且那是账本值不是估算值。
///
/// 两个单价**都没配** → `None`，消费方据此不展示金额（PRD-07-051）。只配了一个就只算
/// 那一半：少算是诚实的，编一个价格补上另一半不是。
/// 按参考单价把「一段音频 + 一批 token」折成钱。两个方向共用它——省下与花掉是同一把尺子
/// 量出来的两个数，各写一遍必然有一天口径分叉（ADR-126 第 6 条）。
fn estimate_usd(
    audio_seconds: f64,
    tokens: u64,
    asr_usd_per_min: Option<f64>,
    llm_usd_per_million: Option<f64>,
) -> Option<f64> {
    if asr_usd_per_min.is_none() && llm_usd_per_million.is_none() {
        return None;
    }
    // **这一侧没有任何可计量 → 估不出来，而不是"零"**（真机 2026-09-12）。
    //
    // 两者在屏幕上都是 `US$0.00`，而它们说的是完全不同的事：一个是「这段时间本机一个
    // 任务都没跑」，一个是「跑了但便宜到看不见」。补列之前的历史行没有执行位置、两边
    // 都不计（ADR-126 第 6 条），于是升级后的第一眼恰恰是前者——用户填了单价却看到
    // 一个零，只会以为这功能没生效。没有量就不给这一格。
    if audio_seconds <= 0.0 && tokens == 0 {
        return None;
    }
    #[allow(clippy::cast_precision_loss)]
    Some(
        (audio_seconds / 60.0) * asr_usd_per_min.unwrap_or(0.0)
            + (tokens as f64 / 1_000_000.0) * llm_usd_per_million.unwrap_or(0.0),
    )
}

/// 「本机省下多少」——**只算本机跑掉的那些**（ADR-126 第 6 条）。
///
/// 此前用的是不分位置的合计，于是服务端跑掉的任务被算进了"为你节省"——把花费说成了
/// 节省，而这个数字正是本地优先这件事的商业叙事，说反了比不说更糟。位置未知的历史行
/// 两边都不计（`local_*` 的口径就是如此）：算不出来就不算。
fn estimate_local_savings_usd(
    summary: &UsageSummary,
    asr_usd_per_min: Option<f64>,
    llm_usd_per_million: Option<f64>,
) -> Option<f64> {
    estimate_usd(
        summary.local_audio_seconds,
        summary.local_tokens,
        asr_usd_per_min,
        llm_usd_per_million,
    )
}

/// 「服务端花了多少」——同一把尺子量交给服务端跑掉的那些。
///
/// 它**不是账单**（计费口径在服务端，ADR-039），是同一组参考单价下的估算，与上面那个数
/// 对称：一个回答「没上云省了多少」，一个回答「上云的这些大概值多少」。
fn estimate_server_cost_usd(
    summary: &UsageSummary,
    asr_usd_per_min: Option<f64>,
    llm_usd_per_million: Option<f64>,
) -> Option<f64> {
    estimate_usd(
        summary.server_audio_seconds,
        summary.server_tokens,
        asr_usd_per_min,
        llm_usd_per_million,
    )
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct NotificationHistoryDto {
    pub id: String,
    pub class: String,
    pub severity: String,
    pub channels: Vec<String>,
    pub target: Option<String>,
    pub i18n_key: String,
    pub params: HashMap<String, String>,
    /// 诊断一节的 key（按种类派生，历史库不存它；见 `notifications::diagnostic_i18n_key`）。
    pub diagnostic_i18n_key: Option<String>,
    pub created_at_ms: String,
    pub read: bool,
}

impl From<crate::usage::NotificationHistoryRecord> for NotificationHistoryDto {
    fn from(record: crate::usage::NotificationHistoryRecord) -> Self {
        Self {
            id: record.id,
            class: record.class,
            severity: record.severity,
            channels: record.channels,
            target: record.target,
            diagnostic_i18n_key: crate::notifications::diagnostic_i18n_key(&record.i18n_key),
            i18n_key: record.i18n_key,
            params: record.params,
            created_at_ms: record.created_at_ms.to_string(),
            read: record.read,
        }
    }
}

impl From<SnapshotMeta> for SnapshotDto {
    fn from(value: SnapshotMeta) -> Self {
        Self {
            id: value.id,
            hash: value.hash,
            created_ms: value.created_ms.to_string(),
            size: value.size.to_string(),
        }
    }
}

/// Trash entry DTO。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct TrashEntryDto {
    /// 稳定 id。
    pub id: String,
    /// 原路径。
    pub original_path: String,
    /// 删除时间。
    pub deleted_ms: String,
    /// 是不是文件夹（列表据此给文件夹图标，而不是把目录画成一个文件）。
    pub is_dir: bool,
    /// 占用字节数（文件夹为递归求和）。u64 按本仓约定走字符串，避免 JS number 精度。
    pub byte_size: String,
    /// 还有几天自动删除；`None` = 保留期设为「永不」，这一条不会被自动清掉。
    ///
    /// 天数在 Rust 算好（ADR-003）：前端拿到的是结论，不必知道保留期是几天、也不必
    /// 自己做"向上取整还是向下"的判断——那种小数学分散到界面里，迟早两处不一致。
    pub expires_in_days: Option<u32>,
}

impl TrashEntryDto {
    fn from_entry(value: TrashListEntry, max_age_ms: Option<i64>, now_ms: i64) -> Self {
        Self {
            expires_in_days: Vault::trash_days_left(value.deleted_ms, max_age_ms, now_ms),
            deleted_ms: value.deleted_ms.to_string(),
            byte_size: value.byte_size.to_string(),
            id: value.id,
            original_path: value.original_path,
            is_dir: value.is_dir,
        }
    }
}

/// 回收站保留期（`vault.trash_retention_days`）→ 毫秒；`None` = 永不自动删除。
///
/// 归一化只在这一处：注册表里那几个取值（`7` / `30` / `90` / `never`）是字符串，认不出来
/// 的一律按默认 30 天，而不是当成"永不"——猜错方向的代价不对称：多留几天只是占点磁盘，
/// 少留会删掉用户还想要的东西。
pub(crate) fn trash_retention_ms(settings: &Settings) -> Option<i64> {
    const DAY_MS: i64 = 24 * 60 * 60 * 1_000;
    match settings
        .get_str("vault.trash_retention_days")
        .unwrap_or_else(|| "30".into())
        .as_str()
    {
        "never" => None,
        "7" => Some(7 * DAY_MS),
        "90" => Some(90 * DAY_MS),
        _ => Some(30 * DAY_MS),
    }
}

/// 回收站条目预览 DTO（PRD-06-009：永久删除前判断内容）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct TrashPreviewDto {
    /// 条目 id。
    pub id: String,
    /// 原相对路径。
    pub original_path: String,
    /// 预览类别：`note` 文本笔记 / `folder` 文件夹 / `binary` 二进制或媒体。
    pub kind: String,
    /// 文本正文（仅 `note`；可能按上限截断）。
    pub content: Option<String>,
    /// 文件夹子项相对路径（仅 `folder`）。
    pub children: Vec<String>,
    /// 条目字节数（十进制字符串，`folder` 为 "0"）。
    pub byte_size: String,
    /// 文本/子项是否因上限被截断。
    pub truncated: bool,
}

impl From<TrashPreview> for TrashPreviewDto {
    fn from(value: TrashPreview) -> Self {
        let kind = match value.kind {
            TrashPreviewKind::Note => "note",
            TrashPreviewKind::Folder => "folder",
            TrashPreviewKind::Binary => "binary",
        };
        Self {
            id: value.id,
            original_path: value.original_path,
            kind: kind.to_string(),
            content: value.content,
            children: value.children,
            byte_size: value.byte_size.to_string(),
            truncated: value.truncated,
        }
    }
}

/// 笔记写盘广播（本应用编辑器写盘 → 所有窗口）：多 pane / 弹出窗口同文件同步的
/// 单一通道（`vault://changed` 因自写回声抑制不可用，TDD-07 §4.1）。
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[tauri_specta(event_name = "note://written")]
pub struct NoteWritten {
    /// 写入的笔记 Vault 相对路径。
    pub path: String,
    /// 写入后的完整正文（接收端据此增量应用或提示冲突）。
    pub content: String,
    /// 库上下文（PRD-06-050）：`"main"` = 主库；副库窗口写盘时为其窗口 label。
    /// 路径是库相对的，不带上下文会把副库的保存注入主窗口的同名标签页。
    pub origin: String,
}

/// Vault 低频变更事件（真实 notify 流留真机验收）。
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[tauri_specta(event_name = "vault://changed")]
pub struct VaultChanged {
    /// 变更的笔记相对路径。
    pub rel_path: String,
    /// 文件是否已删除。
    pub deleted: bool,
}

/// 库已切换（ADR-025）。主窗口工作区靠整体重挂载归零前端态，但**独立 webview**
/// （快捷对话窗等）不随主窗重挂——它们据此事件向 Rust 真值重新对齐，否则切库后
/// 还显示着上一个库的会话内容（Rust 侧早已按「顶替」清空）。
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[tauri_specta(event_name = "vault://switched")]
pub struct VaultSwitched {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VaultChangeScope {
    ActiveVault,
    SecondaryVault,
}

impl VaultChangeScope {
    fn for_window(label: &str) -> Self {
        if AppState::is_secondary_window(label) {
            Self::SecondaryVault
        } else {
            Self::ActiveVault
        }
    }

    fn affects_active_vault(self) -> bool {
        self == Self::ActiveVault
    }
}

pub(crate) fn emit_vault_changed(app: &tauri::AppHandle, rel_path: &str, deleted: bool) {
    emit_vault_changed_in_scope(app, rel_path, deleted, VaultChangeScope::ActiveVault);
}

fn emit_vault_changed_in_scope(
    app: &tauri::AppHandle,
    rel_path: &str,
    deleted: bool,
    scope: VaultChangeScope,
) {
    emit_vault_changes_in_scope(app, [(rel_path.to_owned(), deleted)], scope);
}

/// 一次动作引起的**全部**库内变更（批量移动/删除的整批，单条即 N=1）。
///
/// **逐条广播、派生刷新只做一次**：`VaultChanged` 的 `rel_path` 是各消费者的过滤依据
/// （回放、蒸馏徽标、词典面板都只认自己那一条），故它必须逐条发；而知识地图刷新与度量
/// 基线重算问的是"库变了没有"——它们**与是哪一条无关**。
///
/// 这不是省几次调用的优化，是一次真机故障的根因（2026-09-02，PRD-06-059）：
/// 批量移动四个对象时逐条走单条广播 = 8 次派生刷新，每一次都要抢 Vault 锁做一遍**全库
/// 遍历**，而它们全部排在发起这次批量的那把锁后面；tokio 工作线程被这些任务占满之后，
/// 界面发出的任何一条命令（包括刷新文件树）都排不进去——表现就是"拖完之后整个应用不动了"。
pub(crate) fn emit_vault_changes(
    app: &tauri::AppHandle,
    changes: impl IntoIterator<Item = (String, bool)>,
) {
    emit_vault_changes_in_scope(app, changes, VaultChangeScope::ActiveVault);
}

fn emit_vault_changes_in_scope(
    app: &tauri::AppHandle,
    changes: impl IntoIterator<Item = (String, bool)>,
    scope: VaultChangeScope,
) {
    let changes = changes.into_iter().collect::<Vec<_>>();
    if changes.is_empty() {
        return;
    }
    let active = scope.affects_active_vault();
    if active {
        let deleted = changes
            .iter()
            .filter(|(_, deleted)| *deleted)
            .map(|(rel_path, _)| rel_path.clone())
            .collect::<Vec<_>>();
        if !deleted.is_empty() {
            if let Some(root) = app
                .try_state::<AppState>()
                .and_then(|state| state.vault_root())
            {
                record_local_knowledge_metric_deletions(app, &root, deleted);
            }
        }
    }
    for (rel_path, deleted) in changes {
        let _ = (VaultChanged { rel_path, deleted }).emit(app);
    }
    if !active {
        return;
    }
    // 本地有改动：官方同步尽快跑一轮（同步自己写入引起的这一次也会唤醒，那一轮无事可做即止）。
    if let Some(sync) = app.try_state::<Arc<crate::sync_service::SyncService>>() {
        sync.nudge();
    }
    // 主题页的正文编辑、重命名、移动、删除与外部写回最终都会收敛到同一个 Vault changed
    // 边界；去抖函数只刷新已存在地图，绝不会因普通写入或用户删地图而偷偷新建文件。
    knowledge::schedule_knowledge_map_refresh(app);
    schedule_local_knowledge_metrics_reconcile(app);
}

/// 外部编辑先同步 Rust 派生索引，再广播同一条变更边界。手工链接度量、图谱和界面由此
/// 读取同一拍事实；原生 rename 还会先搬移开放的 24h locator，避免把移动误算成删除。
fn handle_external_vault_change(app: &tauri::AppHandle, change: luna_vault::VaultChange) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let (root, deleted) = {
        let Ok(guard) = state.vault.lock() else {
            return;
        };
        let Some(vault) = guard.as_ref() else {
            return;
        };
        // 库外改名的收集要在同步索引之前：同步之后「消失的那篇」原来是什么内容就查不到了。
        record_external_change(app, vault, &change);
        let result = if let Some(from) = change.moved_from.as_deref() {
            vault.sync_external_note(from)
        } else {
            Ok(())
        }
        .and_then(|()| vault.sync_external_note(&change.rel_path));
        if let Err(error) = result {
            eprintln!("[vault] 外部变更索引同步失败：{error}");
            let _ = (VaultChanged {
                rel_path: change.rel_path,
                deleted: change.disk_hash.is_none(),
            })
            .emit(app);
            return;
        }
        (
            vault.root().to_path_buf(),
            !vault.root().join(&change.rel_path).is_file(),
        )
    };
    if !deleted {
        if let Some(from) = change.moved_from.as_ref() {
            record_local_knowledge_metric_moves(
                app,
                &root,
                [(from.clone(), change.rel_path.clone())],
            );
            emit_vault_changed(app, from, true);
        } else {
            // 外部编辑确认正文变了（监听已按内容 hash 去重）；纯移动不算来源新版本。
            // 与编辑器保存走同一条派生更新：标签、语义索引与主题候选（PRD-16-064）。
            notes::refresh_derived_after_write(app, &change.rel_path);
        }
    }
    emit_vault_changed(app, &change.rel_path, deleted);
}

/// 翻译流的前端投影事件（`JobEvent` 的精简映射；避免在 Phase 2 就把整个 luna-core
/// 领域类型耦合进 specta——待暴露更多 Job 类型时再为 core 加可选 `specta` 特性）。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "snake_case", tag = "kind", content = "data")]
pub enum TranslateEvent {
    /// 结果性质标识（PRD-01-029）：翻译/润色、目标语言、判定是否不确定、所用模板名。
    /// 仅 `popup_generate` 在流首发一条；悬浮菜单不发。
    Meta(TranslateMeta),
    /// 已开始。
    Started,
    /// 流式 token 增量。
    Token(String),
    /// 完成（最终译文）。
    Done(String),
    /// 会话缓存命中（最终译文）。
    CacheHit(String),
    /// 译文已自动复制到剪贴板（`popup_translate.auto_copy` 开启时随终态追发，PRD-01-032）。
    AutoCopied,
    /// 输入超长已按字符截断，仅处理前 N 个字符（PRD-06-025：不静默截断，显式告知质量
    /// 边界）。仅笔记 AI 悬浮菜单在流首发；弹窗翻译不发。
    Truncated(u32),
    /// 笔记 AI 悬浮菜单的本轮事实（PRD-06-020/021）：建议落地方式、执行位置与实际发送
    /// 字数。仅悬浮菜单在流首发（先于 `Truncated`）；弹窗翻译不发。
    FloatingMenu(FloatingMenuMeta),
    /// 分段摘要走到第几段（PRD-06-025）。长文分段是**多次**模型调用，中间没有 token
    /// 流——不报进度的话用户面对的是一个几十秒不动的转圈，与卡死无法区分。
    FloatingChunk { done: u32, total: u32 },
    /// 失败（稳定错误码）。
    Failed(TranslateFailure),
    /// 模型跑完了，但结果不能用（表格 AI 动作，对标方案 T6）：不给落地，面板说明原因。
    /// 与 `Failed` 分开——那是「没跑成」，这是「跑成了但不能放进正文」。
    FloatingRejected(FloatingRejectReason),
    /// 已取消。
    Cancelled,
}

/// 前端可见的稳定 Job 错误码。它是 `JobError` 的穷尽投影，新增领域错误而未同步此处会在
/// `From` 的穷尽匹配中编译失败；额外的 `COMMAND_FAILED` 只表示命令通道本身未建立。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum JobErrorCodeDto {
    ModelMissing,
    PermissionDenied,
    /// 账户会话被拒或根本没有（未登录、会话过期且刷新换不出新的一枚）→ 去账户页重新登录。
    SignInRequired,
    ProviderNotConfigured,
    ResourceExhausted,
    QuotaExhausted,
    /// 账户所在组织没开放这项能力（服务端 403 `capability_not_allowed`）。不可重试；
    /// 有本机腿时路由已自动回落，走到这里说明本机也没有可用的模型。
    CapabilityNotAllowed,
    /// 账户因付款争议被冻结为只读（服务端 403 `account_frozen`，纵切 3 C3）。不可重试；
    /// 下一步是联系支持，不给「升级」。
    AccountFrozen,
    /// 撞上用户自设的本月预付上限（服务端 402 `spend_cap_reached`，纵切 3 C4）。不可重试；
    /// 下一步是调整上限，不给「购买」。
    SpendCapReached,
    /// 这个账户此刻领不到免费额度（服务端 402 `free_quota_withheld`，纵切 3 C5）。不可重试；
    /// 下一步看成因（账户页「订阅与用量」说清），不一律给「购买」。
    FreeQuotaWithheld,
    RateLimited,
    AlreadyProcessed,
    ProviderUnavailable,
    QueueExpired,
    /// 服务端说这次转写跑完了，但结果取不回来（`JobError::ResultUnavailable`）。
    ResultUnavailable,
    StrictLocalBlocked,
    /// 服务端上这一档的候选链一个成员都没有（ADR-083 ⑧）。**结构性**：升格为一次会话级
    /// 信号 / 整批停，而不是逐句失败一百次。
    RouteLaneEmpty,
    /// 服务端受理了，但它自己那一侧没能把请求交出去（`upstream_unavailable` / `internal`）。
    /// 可重试；用户的下一步是稍后再试或改用本机模型（ADR-097）。
    ServerSideFailure,
    /// 服务端这台部署配错了（`egress_blocked` / `upstream_protocol`）。**结构性**、不可重试：
    /// 下一步是找管理员或切回本机模型（ADR-097）。
    ServerMisconfigured,
    /// 本机实时会话额度在宽限期内没腾出来（ADR-078 ①）。
    LocalSessionBusy,
    /// 服务端选中的模型把这一次的输出额度全花在推理上，正文为空（ADR-102）。可重试，
    /// 且重试是正确的第一步——每次受理都重新挑一次上游。
    EmptyAfterThinking,
    /// 输入超过了模型这一次加载的上下文窗口（本机推理引擎如实拒绝、不截断，ADR-143）。
    /// 不可重试：缩短输入才有用。
    InputTooLong,
    Timeout,
    Cancelled,
    InvalidInput,
    Upstream,
    CommandFailed,
}

impl From<&luna_core::JobError> for JobErrorCodeDto {
    fn from(error: &luna_core::JobError) -> Self {
        use luna_core::JobError;
        match error {
            JobError::ModelMissing { .. } => Self::ModelMissing,
            JobError::PermissionDenied { .. } => Self::PermissionDenied,
            JobError::Unauthorized { .. } => Self::SignInRequired,
            JobError::ProviderNotConfigured { .. } => Self::ProviderNotConfigured,
            JobError::ResourceExhausted { .. } => Self::ResourceExhausted,
            JobError::QuotaExhausted { .. } => Self::QuotaExhausted,
            JobError::CapabilityNotAllowed { .. } => Self::CapabilityNotAllowed,
            JobError::AccountFrozen { .. } => Self::AccountFrozen,
            JobError::SpendCapReached { .. } => Self::SpendCapReached,
            JobError::FreeQuotaWithheld { .. } => Self::FreeQuotaWithheld,
            JobError::RateLimited { .. } => Self::RateLimited,
            JobError::AlreadyProcessed { .. } => Self::AlreadyProcessed,
            JobError::ProviderUnavailable { .. } => Self::ProviderUnavailable,
            JobError::QueueExpired { .. } => Self::QueueExpired,
            JobError::ResultUnavailable { .. } => Self::ResultUnavailable,
            JobError::StrictLocalBlocked { .. } => Self::StrictLocalBlocked,
            JobError::RouteLaneEmpty { .. } => Self::RouteLaneEmpty,
            JobError::ServerSideFailure { .. } => Self::ServerSideFailure,
            JobError::ServerMisconfigured { .. } => Self::ServerMisconfigured,
            JobError::LocalSessionBusy { .. } => Self::LocalSessionBusy,
            JobError::Timeout => Self::Timeout,
            JobError::Cancelled => Self::Cancelled,
            JobError::InvalidInput(_) => Self::InvalidInput,
            JobError::EmptyAfterThinking { .. } => Self::EmptyAfterThinking,
            JobError::InputTooLong { .. } => Self::InputTooLong,
            JobError::Upstream { .. } => Self::Upstream,
        }
    }
}

/// 翻译失败投影：稳定错误码（`JobError::code`）。前端据 code 映射 i18n 修复文案
///（TDD-15 §4.3 错误码即文案 key），**绝不裸显错误码、模型名或上游原文**。
#[derive(Debug, Clone, Serialize, Type)]
pub struct TranslateFailure {
    /// 稳定错误码（SCREAMING_SNAKE）。
    pub code: JobErrorCodeDto,
}

impl From<&luna_core::JobError> for TranslateFailure {
    fn from(err: &luna_core::JobError) -> Self {
        Self { code: err.into() }
    }
}

/// 结果性质元数据（PRD-01-029，让用户理解某份结果是翻译还是润色、为何如此）。
#[derive(Debug, Clone, Serialize, Type)]
pub struct TranslateMeta {
    /// `"translate"` | `"refine"`。
    pub mode: String,
    /// 有效目标语言（如 `"zh"` / `"en"`）。
    pub target: String,
    /// 语言判定是否不确定（极短/混合/低置信 → 前端标注，PRD-01-004）。
    pub uncertain: bool,
    /// 所用模板的稳定 id（如 `"general"`）。
    ///
    /// 此前这里发的是**展示名**（`preset.name`，注册表里的中文），前端原样渲染——
    /// 英文界面的摘要行写着 `Translate → zh · 通用翻译`，而紧挨着的下拉正确显示 "General"：
    /// 同一个预设、同一屏、两种语言。展示名是前端 i18n 的事（`popup:preset.<id>`，
    /// 下拉本来就这么取），Rust 只发 id。
    pub preset_id: String,
}

/// 表格 AI 动作的结果为什么不能落地。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum FloatingRejectReason {
    /// 模型输出里抽不出一张合法的 GFM 表（「整理为表格」）。
    NotATable,
    /// 作用域不是一张表 / 列号越界（「按列填充」）。
    NotATableSource,
}

/// 笔记内 AI 悬浮菜单动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum FloatingMenuAction {
    /// 总结选区（无选区时前端传整篇正文，PRD-06-020）。
    Summarize,
    /// 翻译选区。
    Translate,
    /// 优化/润色选区。
    Optimize,
    /// 自定义指令（Notion "Ask AI" 心智）：按用户输入的指令改写选区/结果，
    /// 亦承担结果「追问」（PRD-06-021）——前端以上一轮输出为 text 再次提交。
    Ask,
    /// 续写（光标处作用域）：以光标前文为输入，只输出接续的内容（PRD-06-020）。
    Continue,
    /// 修正拼写 / 语法 / 标点（只改错、不改写；前端据改正稿算逐条差异，PRD-06-021）。
    Proofread,
    /// 改语气：正式。
    ToneFormal,
    /// 改语气：轻松。
    ToneCasual,
    /// 改语气：简洁。
    ToneConcise,
    /// 把选中的文字整理成一张 GFM 表（对标方案 T6）。Rust 从模型输出里抽出并校验表格，
    /// 抽不出就以 `FloatingRejected` 结束、不给落地。
    TableFromText,
    /// 总结 / 解释一张表（作用域 = 这张表的源码），结果插在表格下方。
    TableExplain,
    /// 按用户的一句指令逐行填写某一列（`options.table_column`），只改这一列。
    TableFillColumn,
    /// 视频脚本（PRD-19-014）：精简——去掉口水话与重复，说同样的事用更少的字。
    ScriptTighten,
    /// 视频脚本：重写开头——把最抓人的一点放到最前面，不编造事实。
    ScriptHook,
    /// 视频脚本：改成口语——短句、日常用词、直接对观众说。
    ScriptSpoken,
    /// 视频脚本（PRD-19-013）：整理口述——去掉口头禅、重复与说错重说，保留本人的话。
    ScriptDictation,
    /// 提词器的 AI 口播动作（ADR-176 ⑧，PRD-20-016）：改成口播稿 / 按时长压缩 / 标注停顿与强调。
    PrompterSpoken,
    PrompterFit,
    PrompterCues,
}

impl FloatingMenuAction {
    /// 这个动作在服务端算哪一项能力（wire 名）——组织按能力开关（R4b）时，悬浮菜单的
    /// 「云 / 本机」标记要按它判。翻译偶尔走改写分支，这里按用户点的那一项算。
    pub(crate) fn server_capability(self) -> &'static str {
        match self {
            Self::Summarize => "summarize",
            Self::Translate => "translate",
            _ => "refine",
        }
    }
}

impl FloatingMenuAction {
    /// 稳定 id → 动作（队列载荷回读；未知串 = `None`，按失败处理而不是猜一个动作）。
    #[must_use]
    pub(crate) fn from_tag(tag: &str) -> Option<Self> {
        Some(match tag {
            "summarize" => Self::Summarize,
            "translate" => Self::Translate,
            "optimize" => Self::Optimize,
            "proofread" => Self::Proofread,
            "tone_formal" => Self::ToneFormal,
            "tone_casual" => Self::ToneCasual,
            "tone_concise" => Self::ToneConcise,
            "ask" => Self::Ask,
            "continue" => Self::Continue,
            "table_from_text" => Self::TableFromText,
            "table_explain" => Self::TableExplain,
            "table_fill_column" => Self::TableFillColumn,
            "script_tighten" => Self::ScriptTighten,
            "script_hook" => Self::ScriptHook,
            "script_spoken" => Self::ScriptSpoken,
            "script_dictation" => Self::ScriptDictation,
            "prompter_spoken" => Self::PrompterSpoken,
            "prompter_fit" => Self::PrompterFit,
            "prompter_cues" => Self::PrompterCues,
            _ => return None,
        })
    }

    /// 稳定 id（回执与用量记录用；与 serde 序列化同一口径）。
    #[must_use]
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Summarize => "summarize",
            Self::Translate => "translate",
            Self::Optimize => "optimize",
            Self::Ask => "ask",
            Self::Continue => "continue",
            Self::Proofread => "proofread",
            Self::ToneFormal => "tone_formal",
            Self::ToneCasual => "tone_casual",
            Self::ToneConcise => "tone_concise",
            Self::TableFromText => "table_from_text",
            Self::TableExplain => "table_explain",
            Self::TableFillColumn => "table_fill_column",
            Self::ScriptTighten => "script_tighten",
            Self::ScriptHook => "script_hook",
            Self::ScriptSpoken => "script_spoken",
            Self::ScriptDictation => "script_dictation",
            Self::PrompterSpoken => "prompter_spoken",
            Self::PrompterFit => "prompter_fit",
            Self::PrompterCues => "prompter_cues",
        }
    }

    /// 是否是「改这一段」的改写类动作（替换为建议落地；可带上下文）。
    #[must_use]
    pub(crate) fn rewrites_in_place(self) -> bool {
        matches!(
            self,
            Self::Optimize
                | Self::Ask
                | Self::Proofread
                | Self::ToneFormal
                | Self::ToneCasual
                | Self::ToneConcise
                | Self::TableFromText
                | Self::TableFillColumn
                | Self::ScriptTighten
                | Self::ScriptHook
                | Self::ScriptSpoken
                | Self::ScriptDictation
                | Self::PrompterSpoken
                | Self::PrompterFit
                | Self::PrompterCues
        )
    }
}

/// 总结风格（PRD-06-020，Notion / Apple 写作工具的「要点 / 段落 / 一句话」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum FloatingSummaryStyle {
    Bullets,
    Paragraph,
    OneLine,
}

impl FloatingSummaryStyle {
    /// 稳定 id（队列载荷用；与设置取值、serde 同一口径）。
    #[must_use]
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Bullets => "bullets",
            Self::Paragraph => "paragraph",
            Self::OneLine => "one_line",
        }
    }

    /// 对应的 Job 事实（`SummaryStyle` 的压缩式三档）：账本与服务端读的是 Job 形状，
    /// 不是指令文本，故三档必须原样带进 Job。
    #[must_use]
    pub(crate) fn as_summary_style(self) -> SummaryStyle {
        match self {
            Self::Bullets => SummaryStyle::Bullets,
            Self::Paragraph => SummaryStyle::Paragraph,
            Self::OneLine => SummaryStyle::OneLine,
        }
    }

    pub(crate) fn from_setting(value: &str) -> Self {
        match value {
            "paragraph" => Self::Paragraph,
            "one_line" => Self::OneLine,
            _ => Self::Bullets,
        }
    }
}

/// 后台跑完、**等着用户拍板落地**的一份 AI 结果（PRD-06-021）。
///
/// **只存在内存里**，随进程消失：它含笔记正文的产物，落盘既踩脱敏纪律（TDD-12），
/// 又会让一份三天前的结果在重启后突然要求用户处置——而那时它依据的正文多半已经变了。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct FloatingMenuPendingDto {
    pub note_rel: String,
    pub action: FloatingMenuAction,
    pub output: String,
    /// 本轮事实**整份带上**（建议落地 / 执行位置 / 发送字数 / 分段数）：面板呈现待落地
    /// 结果时用的就是它，与前台流首那一份是同一个类型，不为后台另造一套字段。
    pub meta: FloatingMenuMeta,
}

/// 选区前后段落的原样切片（前端只切、不裁；裁剪与拼装在 Rust）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct FloatingMenuContextDto {
    pub before: String,
    pub after: String,
}

/// 悬浮菜单动作的可选参数（子菜单选择；`None` = 用设置里的默认）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct FloatingMenuOptionsDto {
    /// 翻译目标语言（BCP-47 短码）；`None` = 智能双向（设置）。
    pub target_lang: Option<String>,
    /// 总结风格；`None` = `editor.floating_summary_style`。
    pub summary_style: Option<FloatingSummaryStyle>,
    /// 「按列填充」填哪一列（0 起）；其它动作不读。
    pub table_column: Option<u32>,
    /// 「按时长压缩」的目标秒数（ADR-176 ⑧）；其它动作不读。
    pub target_seconds: Option<u32>,
}

/// 笔记 AI 悬浮菜单作用域（PRD-06-020）：选区 / 光标处 / 整篇。
///
/// 由前端**如实上报**「用户在哪儿召唤的」，而**怎么处理**（取多少上下文、建议落到哪）
/// 全在 Rust（ADR-003）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum FloatingMenuScope {
    /// 有选区：动作作用于选区。
    Selection,
    /// 无选区：作用于光标处（续写 / 起草）。
    Cursor,
    /// 整篇笔记（页头入口）。
    Doc,
}

/// AI 结果的落地方式（PRD-06-021）。Rust 按「动作 × 作用域」给出**建议**，前端把它摆成
/// 主按钮；用户仍可改选其它方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum FloatingMenuLanding {
    /// 就地替换选区。
    Replace,
    /// 插入选区下方。
    InsertBelow,
    /// 插入正文顶部（frontmatter 之后）。
    InsertTop,
    /// 插入文末。
    InsertEnd,
    /// 插入光标处。
    InsertCursor,
    /// 丢弃（只记回执，不动正文）。
    Discard,
}

impl FloatingMenuLanding {
    /// 稳定 id（回执用；与 serde 序列化同一口径）。
    #[must_use]
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Replace => "replace",
            Self::InsertBelow => "insert_below",
            Self::InsertTop => "insert_top",
            Self::InsertEnd => "insert_end",
            Self::InsertCursor => "insert_cursor",
            Self::Discard => "discard",
        }
    }
}

/// 悬浮菜单一轮生成的事实（随 `TranslateEvent::FloatingMenu` 在流首下发）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct FloatingMenuMeta {
    /// 建议落地方式（前端主按钮）。
    pub suggested_landing: FloatingMenuLanding,
    /// 这一轮是否交给服务端跑（本机 = `false`）。前端据此标注「本机 · 不联网」或「云」。
    pub on_server: bool,
    /// 实际送模型的字符数（正文 + 上下文，截断之后）。
    pub sent_chars: u32,
    /// 其中上下文（选区前后段落）的字符数；`0` = 没带上下文。
    pub context_chars: u32,
    /// 长文分段摘要的分段数（PRD-06-025）；`0` = 直接一轮。
    pub chunks: u32,
}

/// 提交前的长文策略（PRD-06-025：不静默截断，**提交前**就告诉用户会怎么处理）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum FloatingMenuStrategy {
    /// 未超上限，一轮直出。
    Direct,
    /// 整篇总结超上限：分段摘要后汇总（与蒸馏同一 map-reduce 形态）。
    Chunked,
    /// 改写类超上限：只处理前 N 字。
    Truncated,
    /// 光标处生成：只取离光标最近的一段前文。
    Tail,
}

/// 预检结果：上限、策略、预计分段数。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct FloatingMenuPreflightDto {
    pub limit: u32,
    pub strategy: FloatingMenuStrategy,
    pub chunks: u32,
}

/// 用户自定义指令预设（PRD-06-024）的前端投影。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct FloatingPresetDto {
    pub id: String,
    pub name: String,
    pub instruction: String,
    /// 库里的（可删）还是装上的模板包带来的（随包卸载，ADR-169 ⑤）。
    pub layer: TemplateLayerDto,
    /// 来自哪个模板包（展示名）；库里的为 `None`。
    pub pack_name: Option<String>,
}

impl From<luna_vault::AiPreset> for FloatingPresetDto {
    fn from(preset: luna_vault::AiPreset) -> Self {
        Self {
            id: preset.id,
            name: preset.name,
            instruction: preset.instruction,
            layer: TemplateLayerDto::Vault,
            pack_name: None,
        }
    }
}

/// 模板的前端投影（紧凑预设选择器用，PRD-01-015）。
#[derive(Debug, Clone, Serialize, Type)]
pub struct PresetInfo {
    /// 稳定 id。
    pub id: String,
    /// 展示名。
    pub name: String,
    /// 分组：`target_lang`（固定目标语言）| `style`（智能双向 + 表达方式）。由 Rust 按
    /// `target_lang_mode` 判定——此前前端按 `id.startsWith("to_")` 猜（ADR-003）。
    pub group: String,
}

/// 弹窗会话当前状态（弹窗窗口**显示时主动拉取**，决策 A）。
///
/// 不依赖可能丢失的 `PopupSessionReady` 推送（预热隐藏窗口的 webview 可能延迟到首次 show 才注册
/// 监听 → 推送早于监听就绪而丢失）。携带 `source_text` 供展示模式呈现原文 + 「编辑原文」预填
/// （PRD-01-006 选区可编辑后重译；仅回传给本应用弹窗 webview 渲染，不入日志，TDD-12 §1）。
#[derive(Debug, Clone, Serialize, Type)]
pub struct PopupSessionState {
    /// `"display"`（展示模式，有选区）| `"input"`（输入模式）。
    pub mode: String,
    /// 会话当前是否已有源文本（展示模式且有源文本 → 前端自动生成）。
    pub has_source: bool,
    /// 会话源文本（展示模式原文区 + 编辑预填；无源文本为 `None`）。
    pub source_text: Option<String>,
    /// 仅译境 OCR 会话为 `true`：结果保存必须进入用户配置的译境目录。
    pub save_to_translate_space: bool,
    /// 输入模式的未生成草稿（关窗保留、重开预填，PRD-01-005 修订）。
    pub draft: Option<String>,
    /// 这一次取选区借用了剪贴板、却没能原样还原（Windows 热键取选区，ADR-163 ②）：弹窗如实提示。
    pub clipboard_not_restored: bool,
}

/// 替换原文操作结果（PRD-01-021）：成功注入或已降级复制。
#[derive(Debug, Clone, Serialize, Type)]
pub struct PopupReplaceOutcome {
    /// `"injected"` | `"injected_clipboard_not_restored"` | `"copied_fallback"` |
    /// `"copied_target_changed"`（后两者都已把译文放进剪贴板）。
    pub kind: String,
    /// 脱敏状态文案；不得包含译文或源文。
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// 弹窗替换（注入）在 macOS 与 Windows 实现；判定的用例全平台跑。
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
enum ReplacePanelAction {
    Close,
    KeepOpen,
}

#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
fn replace_panel_action_for_kind(kind: &str) -> ReplacePanelAction {
    match kind {
        "injected" => ReplacePanelAction::Close,
        _ => ReplacePanelAction::KeepOpen,
    }
}

/// 受管本地音频 Range 服务信息。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct MediaServerInfoDto {
    /// `http://127.0.0.1:<random-port>`。
    pub base_url: String,
    /// 每进程随机 bearer token；前端请求必须放入 `Authorization: Bearer ...`。
    pub token: String,
}

/// 设备级设置持久化状态。
///
/// 曾有 `Disabled` 变体（配置损坏时禁止一切写入，以免覆盖可人工恢复的文件），
/// 已删除：它把"保护用户文件"实现成了"整个应用不可配置"——首次引导连 Vault 都写不进去，
/// 新用户第一次打开就卡死（2026-07-26 真机）。损坏文件改为**改名留档**（见
/// `archive_corrupt_config`），既留住原始内容，又不让应用失去可配置性。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsPersistence {
    /// 可写设备级配置文件路径。
    Writable(PathBuf),
    /// 配置目录不可解析（极罕见）：仅内存生效，重启不留存。
    MemoryOnly,
}

/// 应用状态：持有全局 `JobQueue`（GUI 与 CLI 共用同一主干）+ 弹窗翻译所需的语言检测器与设置，
/// 以及随包推理引擎的监管器（生命周期守护，ADR-143）。
/// 批量档 ASR Provider 缓存：`Some((模型路径, 句柄))`，按模型路径缓存避免重复加载权重。
type BatchAsrCache = Arc<Mutex<Option<(PathBuf, Arc<WhisperProvider>)>>>;

/// 本机实时档 whisper 缓存（路径 + 句柄）。与 [`BatchAsrCache`] 同形：按权重路径缓存，
/// 路径没变就不重读几百 MB。
type LocalAsrCache = Arc<Mutex<Option<(PathBuf, Arc<WhisperProvider>)>>>;

/// 流式 ASR 句柄 + 它是按什么形状组起来的。
type StreamAsrCache = Arc<Mutex<Option<(StreamAsrShape, Arc<dyn luna_core::StreamingAsr>)>>>;

/// 外发闸的进程内实现（[`luna_core::OutboundGate`]）。
///
/// 缓存成一个原子布尔，而不是每次去读 `Settings`：外发型 Provider 在**音频热路径上
/// 每窗问一次**，去抢那把全局设置锁会把隐私检查变成音频链路上的抖动源。
/// 事实源仍是设置——[`apply_setting_side_effects`] 是唯一的写入点。
/// **初值是关**（`AtomicBool::default()` = false）。装配读到设置之前的那一小段
/// 时间里，默认放行等于对严格本地的用户开了一扇窗；默认拦住最坏也只是让一个早到的
/// 任务如实报「仅本地」。注意这与 [`luna_core::AllowOutbound`] 的方向相反——那个是
/// 给「本来就不涉外发」的装配用的常量，这个是真实隐私档的投影。
#[derive(Debug, Default)]
pub(crate) struct PrivacyOutboundGate(std::sync::atomic::AtomicBool);

impl PrivacyOutboundGate {
    /// 按当前隐私档对齐。`strict_local = true` → 关闸。
    fn sync(&self, strict_local: bool) {
        self.0
            .store(!strict_local, std::sync::atomic::Ordering::Release);
    }
}

impl luna_core::OutboundGate for PrivacyOutboundGate {
    fn allows_outbound(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Acquire)
    }
}

/// 实时转写这条链路当前该长成什么样（ADR-038）。
///
/// 形状是**装配的输入**而不是事后描述：`ensure_asr_provider_ready` 先算出它，与当前
/// 已装的比对，一致就原样复用。没有这层比对的话，任何一次设置变更都会重建组合，
/// 而重建会把正在录的那个会话的登记表换掉（`StreamId` 登在旧实例里）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StreamAsrShape {
    /// 主腿是服务端实时通道（`ai.assignment.realtime_asr` = `service:asr`）。
    pub(crate) to_service: bool,
    /// 本机 whisper 腿的权重路径；`None` = 这一腿不在。
    pub(crate) local: Option<PathBuf>,
}

/// 译境的进程内会话状态。它不保存用户内容；只保证“关闭”会取消当前 OCR Job，且
/// 在镜框因单次拍摄暂时隐藏时，仪表盘仍能正确呈现为运行中。
#[derive(Default)]
struct DesktopLensSession {
    run: Mutex<DesktopLensRun>,
}

#[derive(Default)]
struct DesktopLensRun {
    active: bool,
    generation: u64,
    ocr_cancel: Option<CancelToken>,
}

impl DesktopLensSession {
    // 译境只在 macOS 实现；会话语义的用例全平台跑。
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    fn activate(&self) {
        if let Ok(mut run) = self.run.lock() {
            run.active = true;
        }
    }

    fn active(&self) -> bool {
        self.run.lock().is_ok_and(|run| run.active)
    }

    fn end(&self) {
        if let Ok(mut run) = self.run.lock() {
            run.active = false;
            run.generation = run.generation.wrapping_add(1);
            if let Some(cancel) = run.ocr_cancel.take() {
                cancel.cancel();
            }
        }
    }

    /// 登记本会话的唯一 OCR Job；关闭已先发生时立即取消，不允许新 Job 残留。
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    fn attach_ocr(&self, cancel: CancelToken) -> Option<u64> {
        if let Ok(mut run) = self.run.lock() {
            if !run.active {
                cancel.cancel();
                return None;
            }
            run.generation = run.generation.wrapping_add(1);
            if let Some(previous) = run.ocr_cancel.replace(cancel) {
                previous.cancel();
            }
            Some(run.generation)
        } else {
            cancel.cancel();
            None
        }
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    fn finish_ocr(&self, generation: u64) {
        if let Ok(mut run) = self.run.lock() {
            if run.generation == generation {
                run.ocr_cancel = None;
            }
        }
    }
}

/// 服务端肯不肯跑某项能力（见 [`AppState::server_offer`]）。不肯时分清是谁的决定：
/// 部署方没给它配上游（ADR-041 ①），还是组织没给这个账户开放（统一登录 R4b）——两者的
/// 下一步分别在运营与管理员手里，界面要说出是哪一个。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ServerOffer {
    Serves,
    NotConfigured,
    NotAllowed,
}

pub struct AppState {
    /// 后端文案的取用来源——**与 Tauri state 里那个是同一个对象**（`Arc` 共享）。
    ///
    /// # 为什么 `AppState` 也要持有它
    ///
    /// `coded_error(app, …)` 要一个 `AppHandle`，而 IPC 层大量产出人读说明的地方只持
    /// `&AppState`——纯函数、helper、`vault_not_open()` 这种被上百处调用的方法。够不着
    /// 文案来源的直接后果写在欠账台账里：批次 J 之后剩下的 36 条硬编码中文，**全部**是
    /// 这一类。摩擦即成因（`backend_copy` 模块头注早写过同一句）。
    ///
    /// **`OnceLock` 而不是构造参数**：构造口 `build_state_with_settings_persistence`
    /// 有多个测试调用点，加一个参数只是把摩擦搬个地方；而 [`BackendCopy`] 本来就是
    /// `Option`——取不到即回落到「只发码」，那是这条通路一直以来的既定行为，不是新增的
    /// 失败模式。装配点只有一处，由 `the_backend_copy_is_attached_exactly_once` 判定。
    ///
    /// **持 `Arc` 而不是复制一份**：`set_language` 原地改 `RwLock` 内容、从不替换实例，
    /// 故共享同一个对象在语言切换后仍然正确；各持一份则会在切语言那一刻分叉。
    backend_copy: std::sync::OnceLock<Arc<crate::i18n::BackendI18n>>,
    queue: Arc<JobQueue>,
    provider_registry: Arc<ProviderRegistry>,
    /// 采集侧用的流式 ASR 句柄（与 `provider_registry` 内同一 `Arc`）+ 它的形状。
    /// 录音/语音输入/实时字幕经 `StreamingAsr::register_stream` 拿会话句柄，**不关心**
    /// 背后是本机 whisper、服务端实时通道还是两者的降级组合（ADR-038）。
    /// `None` = 既没有本地模型也没有服务端通道（录音降级为纯录音）。
    asr_provider: StreamAsrCache,
    /// 本机实时档 whisper（可能同时是降级组合的兜底腿）。按权重路径缓存。
    local_asr: LocalAsrCache,
    /// 外发闸：严格本地时**当场**掐断正在进行的云端会话（装配时不构造只管新会话）。
    outbound_gate: Arc<PrivacyOutboundGate>,
    /// 批量档 ASR Provider 缓存（重转写/导入用独立模型；按模型路径缓存，避免每次重载权重）。
    /// `None` = 未加载；`Some((path, provider))` = 当前缓存的模型路径与句柄（路径变即重载）。
    batch_asr: BatchAsrCache,
    /// 本地语言检测器（弹窗翻译分支判定，TDD-03 §3.5）。构造一次、长驻复用。
    /// **依赖 `LanguageDetector` 接口而非具体实现**（装配点在 `ipc/providers.rs` 决定注入
    /// 哪个实现，便于后续引入平台原生/多档检测器而不动持有端）：实时/批量转译按句注入同一
    /// `Arc<dyn LanguageDetector>`，不重复构造。
    detector: Arc<dyn LanguageDetector>,
    /// 设置（弹窗翻译语言基准 / 严格本地等）：启动时从设备级配置文件加载（损坏回退默认，
    /// TDD-05 §7.3），无文件时用注册表默认。
    settings: Mutex<Settings>,
    /// 设备级设置能否写回磁盘。损坏恢复 / 无配置目录时禁写，避免覆盖原文件。
    settings_persistence: SettingsPersistence,
    /// 随包 `llama-server` 路由器的监管器（ADR-143）。持有即可：何时启动由模型管理决定，
    /// 退出时 `lib.rs` 的 `RunEvent::Exit` 走 SIGTERM 收掉它（不留孤儿、不在诊断报告里留崩溃）。
    local_engine: luna_engine::llama::LlamaServerSupervisor,
    /// 最近一次引擎编排的结论（加载了哪些、因内存没加载哪些），同时充当编排的串行锁：
    /// 安装 / 删除 / 启动可能同时触发编排，两次编排交错会让 preset 与结论对不上。
    local_engine_plan: tokio::sync::Mutex<crate::local_engine::EnginePlan>,
    /// 随包引擎里已装的模型（每次编排时记下）。Provider 装配据此认出指向引擎条目的裸 id；
    /// 清单变了才重装配。
    engine_models: Mutex<Vec<crate::local_engine::EngineModelRef>>,
    /// 云客户端此刻指向的服务器（账户记住的那一台）。由 [`crate::cloud::reconfigure_cloud_stack`]
    /// 每次换齐时记下；端点与落点都从这里取，宿主不另行解析地址。
    cloud_server: Mutex<crate::cloud::ServerRef>,
    /// 登录账户可用的 AI 能力（权益 `inference.capabilities`，统一登录 R4b）。`None` = 还不知道，
    /// 按「服务器全都提供」路由。与 `cloud_server` 同一时刻由重装记下。
    cloud_capabilities: Mutex<Option<Vec<String>>>,
    /// 这台服务端的部署事实（配没配上游、真有上游的能力，ADR-041 ①）。`None` = 还不知道，
    /// 按「服务端跑得了」路由。与 `cloud_capabilities` 同一时刻由重装记下。
    cloud_deployment: Mutex<Option<luna_cloud::protocol::DeploymentFacts>>,
    /// 弹窗翻译会话（源文本 + 模式 + 会话内结果缓存，决策 A / ADR-003 / PRD-01-016）。
    /// `std::sync::Mutex` 仅短暂持有（取/存源文本与缓存项），**绝不跨 `.await` 持锁**。
    popup: Arc<Mutex<PopupSession>>,
    /// 弹窗输入模式失焦收起状态机。只记录焦点事件是否满足关闭条件，不持有业务正文。
    popup_blur: Mutex<PopupBlurState>,
    /// 译境的短生命周期状态与当前 OCR 取消令牌（不持有用户内容）。
    desktop_lens: DesktopLensSession,
    /// 麦克风硬互斥仲裁器（语音输入 / 弹窗语音 / 录音共享，TDD-05 §5.3）。
    mic: Arc<Mutex<MicArbiter>>,
    /// AI 语音输入控制器（采集/转写/落地编排归 Rust，ADR-003）。
    voice_input: Arc<VoiceInputController>,
    /// 录音控制器（状态机 + 麦克风互斥 + 归档编排，PRD-04 / TDD-05 §3）。
    recording: Arc<RecordingController>,
    /// 文本朗读（TTS）控制器（播放型系统引擎，非 JobQueue；ADR-022 / TDD-05 §9）。
    /// 单一发声者，跨弹窗翻译 HUD 与笔记编辑器共享（新朗读抢占旧朗读）。
    tts: Arc<crate::tts::TtsController>,
    /// 当前 Vault（SQLite Connection 非 Sync，必须经 Mutex 串行进入）。
    /// MVP-0：启动不再自动打开默认 Vault——`None` 表示尚未选择/创建（首启引导门控，PRD-14）。
    /// 仅当 `vault.root` 设置（或 `LUNA_VAULT_ROOT`）非空时启动即打开（沿用上次选择，免重复选）。
    vault: Arc<Mutex<Option<Vault>>>,
    /// 启动时配置指着、却打不开的库根（PRD-14-062）。库一旦被绑定它就不再有意义，
    /// `vault_status` 只在未打开时下发它。
    startup_vault_missing: Option<PathBuf>,
    /// 上面那个库根不是找不到，是 `.luna/` 还在 iCloud 上没取回（启动只等几秒，见
    /// `luna_vault::icloud::wait_until_local`）：空状态要说「还在下载」并给「重试」，不是「找不到了」。
    startup_vault_in_cloud: bool,
    /// **库代际号**（ADR-025 / TDD-06 §12）：每次切库自增，异步写回在提交点据此失效。
    ///
    /// 这是跨库误写的**通用护栏**。切库瞬间总有一个 LLM 回调正要写盘（单篇蒸馏、
    /// AI 标签…），它手里的 `note_rel` 是**库相对路径**——新库存在同名路径时就会写错库。
    /// 逐类任务各写一遍「检查库还是不是那个库」堵不完（下一个异步写回又会漏），故收敛为
    /// 一个判定点：发起时 `current()`，提交前 `still_current(gen)`，不匹配即放弃。
    vault_generation: Arc<std::sync::atomic::AtomicU64>,
    /// 切库交接进行中（PRD-06-049 边界⑤）：过渡期 `vault` 短暂为 `None`，此时全局采集
    /// （语音速记 / 录音 / 弹窗翻译存笔记）撞上的错误必须说「正在切换库」而不是误导性的
    /// 「尚未选择 Vault」。由 [`VaultSwitchingGuard`] RAII 管理，任何退出路径都会复位。
    /// `Arc`：采集落盘闭包（`'static`）在**执行那一刻**读真值，不能借用 AppState。
    vault_switching: Arc<std::sync::atomic::AtomicBool>,
    /// 设备级库注册表（PRD-06-049）：这台机器知道哪些库。**配置数据不是设置项**
    /// ——不进 settings registry，「恢复默认」不该清掉用户的库清单。
    vault_registry: luna_vault::registry::VaultRegistry,
    /// 设备状态（ADR-140）：布局、树的排序与展开按库身份存在配置目录，不进库。
    pub(crate) device_state: crate::device_state::DeviceStateStore,
    /// 设备级用量库（全局功能 + 设备级通知历史）。
    device_usage: UsageStore,
    /// notify 监听句柄；drop 即停止监听。
    vault_watcher: Mutex<Option<VaultWatcher>>,
    /// 内存态 ChatSession 集合；退出即丢弃，不持久化。
    chat: Arc<Mutex<ChatSessionStore>>,
    /// 受管本地音频 HTTP Range 服务（仅 127.0.0.1）。
    media_server: MediaServer,
    /// 统一处理队列（导入 / 二次转写 / 二次翻译 / 说话人细分单 worker 顺序处理，PRD-04-032/055）。
    processing: Arc<crate::processing_queue::ProcessingQueue>,
    /// 笔记 AI 悬浮菜单进行中任务（单槽：新任务顶替并取消旧任务；`floating_menu_stop`
    /// 据此停止生成，PRD-06-021）。锁仅短暂持有，绝不跨 `.await`。
    floating_menu: Mutex<FloatingMenuRun>,
    /// 弹窗翻译进行中任务，**按结果槽位**各一槽（对比槽与主槽并行生成，PRD-01-012）：
    /// 同槽位重新生成 / 换预设顶替并取消旧任务；关窗、改源文本、`popup_stop` 全部取消。
    /// 此前弹窗没有任何取消通路：关窗后旧 Job 继续跑完并计费，同槽位再生成时旧流的
    /// token 还会写进新槽。锁仅短暂持有，绝不跨 `.await`。
    popup_runs: Mutex<std::collections::HashMap<u32, FloatingMenuRun>>,
    /// 进行中的模型安装（模型 id → 取消令牌，PRD-14-020「可暂停」）。暂停 = 取消这一次
    /// 取字节，`.part` 留在暂存目录，下次「下载」经 Range 从断点续。
    /// 同一模型同时只允许一次安装。
    model_installs: Mutex<std::collections::HashMap<String, luna_core::CancelToken>>,
    /// 后台跑完、等着落地的 AI 结果，按笔记路径存一份（PRD-06-021）。同一篇再跑一次
    /// 即顶替——待落地的永远是最后那一份，两份并存只会让用户去猜哪个是新的。
    floating_pending: Mutex<std::collections::HashMap<String, FloatingMenuPendingDto>>,
    /// 弹出编辑器独立窗口：窗口 label → 笔记 Vault 相对路径（PRD-07-036 弹出窗口）。
    /// 窗口 JS 挂载时经 `note_window_path` 按自身 label 取路径（免 URL 编码传参）；
    /// 窗口销毁时移除。`AtomicU64` 生成唯一 label 序号（`note-editor-<n>`）。
    note_windows: Mutex<HashMap<String, String>>,
    note_window_seq: std::sync::atomic::AtomicU64,
    /// 桌面便签窗口（ADR-119）：窗口 label → 那一篇速记的相对路径。与弹出编辑器同一形态，
    /// 但两张表分开——它们的窗口种类、生命周期与关闭语义都不同（便签关窗 = 取消钉住）。
    desktop_note_windows: Mutex<HashMap<String, String>>,
    desktop_note_seq: std::sync::atomic::AtomicU64,
    /// 提词器此刻放的是哪篇脚本（库相对路径；ADR-162 ⑤）。提词器只有一扇常驻面板。
    teleprompter_script: Mutex<Option<String>>,
    /// 跟读中的对齐器（ADR-162 ⑤ C4b）；没在跟读为 `None`。旁听者与「回到开头」共用这一份。
    teleprompter_follow: Mutex<Option<Arc<Mutex<luna_core::prompter_align::PrompterAligner>>>>,
    /// 还没写回脚本的录制标记（ADR-162 ⑤ C4c）；停止跟读 / 收起提词器 / 换脚本时写回并清空。
    teleprompter_marks: Mutex<Vec<teleprompter::PendingRetake>>,
    /// 取消钉住之后**隐藏起来备用**的便签窗（label 列表）。
    ///
    /// 便签窗绝不 `close()`：真机 2026-09-09 复验第 5 条——销毁一扇持有输入法会话的
    /// NSPanel 会抛 ObjC 异常穿过 Rust（`IMKCFRunLoopWakeUpReliable` → `fatal runtime
    /// error: Rust cannot catch foreign exceptions`），整个应用当场退出。仓库里其它面板
    /// 一律只 `hide()`，本条是唯一走了 `close()` 的，也是唯一崩的。
    ///
    /// 隐藏之后进这个池子等着被下一次「钉到桌面」复用——只隐藏不回收会让每钉一次就多
    /// 攒一扇看不见的 webview。
    desktop_note_idle: Mutex<Vec<String>>,
    /// 副库窗口的 Vault 实例（PRD-06-050）：窗口 label → 独立句柄。与主库互不相干——
    /// 不受主库代际号影响、不装监听、不参与设置系统；窗口销毁即摘除（`Arc` 最后一份
    /// 释放时索引连接随之关闭）。
    secondary_vaults: Mutex<HashMap<String, Arc<Mutex<Vault>>>>,
    vault_window_seq: std::sync::atomic::AtomicU64,
    /// 后台索引扫描的取消标志、轮次与现状（PRD-06-004，TDD-06 §4.1）。
    pub(crate) scan: Arc<ScanControl>,
    /// 库外改名的收集与当前提示（PRD-06-030 ⑤）。
    pub(crate) external_renames: ExternalRenameWatch,
    /// AI 关系标签生成 in-flight 登记（笔记相对路径集合）：同一笔记并发触发只留一个
    /// （保存去抖 + 手动/自动竞争防重，PRD-06-035）。锁仅短暂持有，绝不跨 `.await`。
    graph_tags_inflight: Mutex<std::collections::HashSet<String>>,
    /// 生成期间又有保存进来的笔记（BUSY 合并旗标）：本地模型一跑几十秒，打字期间的每次
    /// 自动保存都会撞上 in-flight——旧实现逐次丢弃并逐条刷日志，最后一次保存的内容还
    /// 拿不到标签（陈旧到下一次保存为止）。现在撞忙只置旗标，持有名额的那次运行结束后
    /// **补跑一次**（指纹门控让内容未变的补跑零成本）。锁仅短暂持有，绝不跨 `.await`。
    graph_tags_pending: Mutex<std::collections::HashSet<String>>,
    /// 全库 AI 关系标签重扫运行态（单例 + 进度快照，PRD-06-035 全库重扫入口）。
    /// 锁仅短暂持有，绝不跨 `.await`。
    graph_rescan: Mutex<GraphRescan>,
    /// 蒸馏 in-flight 登记（源笔记相对路径集合，PRD-06-044）：同一篇资料并发触发只留一个
    /// （自动触发与用户点徽标可能撞车）。锁仅短暂持有，绝不跨 `.await`。
    distill_inflight: Mutex<std::collections::HashSet<String>>,
    /// 蒸馏预览暂存槽（PRD-06-045）：预览产物待用户确认后落盘，退出即弃。
    /// 单槽——预览是模态操作，同时预览两篇不是真实场景。锁仅短暂持有，绝不跨 `.await`。
    distill_preview: Mutex<Option<DistillPreviewSlot>>,
    /// 语义检索的**内存向量矩阵**（PRD-16-028 / ADR-027）：懒加载，第一次查询时从
    /// `index.sqlite` 读全部向量建成。
    ///
    /// **切库 / 换模型 / 建库写入之后必须整体作废**（`embed::invalidate_matrix`，
    /// 逐一登记在 TDD-06 §12 交接表）：矩阵是「派生数据的派生数据」，任何一层变了它就是
    /// 陈旧的——而陈旧的矩阵不会报错，只会安静地按上一个库的内容排序。
    /// `Arc`：查询侧拿一份克隆就走，不在整段余弦计算期间持锁。
    embed_matrix: Mutex<Option<Arc<luna_vault::VectorMatrix>>>,
    /// 当前已核实的向量空间身份。digest 是权重文件的 sha256（引擎编排时记下），不能由模型名推断。
    /// 同步消费面（相关页/体检）只读这份已核实事实；异步入口负责刷新。
    embed_index_identity: Mutex<Option<luna_vault::EmbeddingIndexIdentity>>,
    /// 增量嵌入的 in-flight 登记（同一篇的并发触发只留一个；去抖靠指纹，见 `embed` 模块）。
    embed_inflight: Mutex<std::collections::HashSet<String>>,
    /// 全量建库运行态（单例 + 进度快照，六要素骨架同 `graph_rescan`）。
    embed_rebuild: Mutex<embed::EmbedRebuild>,
    /// 深度体检运行态（单例 + 进度快照，六要素骨架同 `embed_rebuild`，PRD-16-031）。
    checkup_run: Mutex<checkup::CheckupRun>,
    /// 对话的上下文窗（token）：**问引擎要来的模型能力 ∩ 本机档位上限**，`0` = 还没探测过
    /// （用 `TokenBudget::default()` 的 4096）。启动与换模型各刷新一次，见
    /// `chat::refresh_chat_context_window`。
    ///
    /// 存起来而不是每次建会话都探一次：`/api/show` 是 await，而会话在同步路径里建。
    chat_ctx_window: std::sync::atomic::AtomicU32,
    /// 安装/删除确认快照。前端只能提交 opaque plan id；执行前 Rust 重取事实并复核。
    model_install_plans: Mutex<HashMap<String, ModelInstallPlanDto>>,
    model_delete_plans: Mutex<HashMap<String, ModelDeletePlanDto>>,
    /// 模型存储搬移运行态（单例，PRD-10-014）：计划快照 + 进行中的取消令牌 + 最近终态。
    storage_relocation: Mutex<model_relocation::RelocationState>,
    /// 经 URL scheme 送达、尚未处理完的库内容链接（ADR-141，W4a）。要切库时界面会整个重挂载，
    /// 所以处理完才清（[`vault_links::vault_link_done`]），不是取一次就清。
    pending_vault_link: Mutex<Option<vault_links::PendingVaultLink>>,
    /// 待主窗工作区落地的导航（[`workspace::request_workspace_navigation`] 存、
    /// [`workspace::take_pending_navigation`] 取即清）。
    pending_navigation: workspace::PendingNavigation,
    /// 工件下载接缝（TDD-14 §3.2 ②）：模型安装的「取字节到本地文件」层。装配点在
    /// `ipc/providers.rs`（默认直连实现）；官方构建装配期可替换为制品面实现。
    artifact_fetcher: Arc<dyn crate::artifact_fetcher::ArtifactFetcher>,
}

/// 单槽运行态（悬浮菜单 / 弹窗每个结果槽各持一份）：generation 防止旧流结束时误清
/// 新任务的取消令牌。
#[derive(Default)]
struct FloatingMenuRun {
    generation: u64,
    cancel: Option<luna_core::CancelToken>,
}

impl AppState {
    /// 登记弹窗某槽位的一次生成（顶替并取消该槽位上一次），返回代数。
    pub(crate) fn register_popup_run(
        &self,
        slot: u32,
        cancel: luna_core::CancelToken,
    ) -> Result<u64, String> {
        let mut runs = self
            .popup_runs
            .lock()
            .map_err(|_| "弹窗运行槽锁中毒".to_string())?;
        let run = runs.entry(slot).or_default();
        if let Some(previous) = run.cancel.take() {
            previous.cancel();
        }
        run.generation += 1;
        run.cancel = Some(cancel);
        Ok(run.generation)
    }

    /// 弹窗某槽位的生成结束：只清自己那一代的令牌（已被顶替时不动）。
    pub(crate) fn finish_popup_run(&self, slot: u32, generation: u64) {
        if let Ok(mut runs) = self.popup_runs.lock() {
            if let Some(run) = runs.get_mut(&slot) {
                if run.generation == generation {
                    run.cancel = None;
                }
            }
        }
    }

    /// 取消弹窗的生成：`Some(slot)` = 只停那一槽；`None` = 全部（关窗 / 源文本变了）。
    /// 登记一次模型安装；同一模型已在装则拒绝（两条下载写同一个 `.part` 会互相踩）。
    /// 存一条送达的库内容链接（后到的覆盖先到的：用户最后点的那条才是想要的）。
    pub(crate) fn stash_vault_link(&self, link: vault_links::PendingVaultLink) {
        if let Ok(mut slot) = self.pending_vault_link.lock() {
            *slot = Some(link);
        }
    }

    /// 看一眼待处理的库内容链接（不清除）。
    pub(crate) fn peek_vault_link(&self) -> Option<vault_links::PendingVaultLink> {
        self.pending_vault_link
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
    }

    pub(crate) fn clear_vault_link(&self) {
        if let Ok(mut slot) = self.pending_vault_link.lock() {
            *slot = None;
        }
    }

    pub(crate) fn begin_model_install(
        &self,
        model_id: &str,
    ) -> Result<luna_core::CancelToken, String> {
        // 搬移进行中不开新安装（PRD-10-014）：搬移先拍了文件清单再拷贝，这时写进旧根的
        // 新文件不在清单里，切换后旧根一删就没了。反向（安装中不开搬移）在搬移预检里。
        if self
            .storage_relocation
            .lock()
            .map(|r| r.is_running())
            .unwrap_or(false)
        {
            return Err(format!("MODEL_INSTALL_BLOCKED_RELOCATION: {model_id}"));
        }
        let mut installs = self
            .model_installs
            .lock()
            .map_err(|_| "模型安装登记锁中毒".to_owned())?;
        if installs.contains_key(model_id) {
            return Err(format!("MODEL_INSTALL_IN_PROGRESS: {model_id}"));
        }
        let token = luna_core::CancelToken::new();
        installs.insert(model_id.to_owned(), token.clone());
        Ok(token)
    }

    /// 丢掉已加载的本机 whisper（实时档 / 批量档 / 流式组合）：权重旁的 CoreML 编码器
    /// 装上或移除后，下一次任务按新形态重新加载（懒加载纪律不变，只是清缓存）。
    pub(crate) fn evict_asr_caches(&self) {
        if let Ok(mut guard) = self.local_asr.lock() {
            *guard = None;
        }
        if let Ok(mut guard) = self.batch_asr.lock() {
            *guard = None;
        }
        if let Ok(mut guard) = self.asr_provider.lock() {
            *guard = None;
        }
    }

    /// 安装结束（成功 / 失败 / 暂停）即注销。
    pub(crate) fn finish_model_install(&self, model_id: &str) {
        if let Ok(mut installs) = self.model_installs.lock() {
            installs.remove(model_id);
        }
    }

    /// 暂停一次进行中的安装；返回 `false` = 没有这一条在装。
    pub(crate) fn pause_model_install(&self, model_id: &str) -> bool {
        self.model_installs
            .lock()
            .ok()
            .and_then(|installs| installs.get(model_id).cloned())
            .map(|token| {
                token.cancel();
                true
            })
            .unwrap_or(false)
    }

    pub(crate) fn cancel_popup_runs(&self, slot: Option<u32>) {
        if let Ok(mut runs) = self.popup_runs.lock() {
            for (id, run) in runs.iter_mut() {
                if slot.is_none_or(|s| s == *id) {
                    if let Some(cancel) = run.cancel.take() {
                        cancel.cancel();
                    }
                }
            }
        }
    }
    /// 云客户端此刻指向的服务器（见字段文档）。
    #[must_use]
    pub(crate) fn cloud_server(&self) -> crate::cloud::ServerRef {
        self.cloud_server
            .lock()
            .map(|server| server.clone())
            .unwrap_or_else(|_| crate::cloud::ServerRef::of(luna_cloud::config::DEFAULT_API_BASE))
    }

    pub(crate) fn set_cloud_server(&self, server: crate::cloud::ServerRef) {
        if let Ok(mut guard) = self.cloud_server.lock() {
            *guard = server;
        }
    }

    /// 记下登录账户可用的 AI 能力（重装时从云客户端取）。
    pub(crate) fn set_cloud_capabilities(&self, capabilities: Option<Vec<String>>) {
        if let Ok(mut guard) = self.cloud_capabilities.lock() {
            *guard = capabilities;
        }
    }

    /// 记下这台服务端的部署事实（重装时从云客户端取）。
    pub(crate) fn set_cloud_deployment(
        &self,
        facts: Option<luna_cloud::protocol::DeploymentFacts>,
    ) {
        if let Ok(mut guard) = self.cloud_deployment.lock() {
            *guard = facts;
        }
    }

    fn cloud_deployment(&self) -> Option<luna_cloud::protocol::DeploymentFacts> {
        self.cloud_deployment
            .lock()
            .map_or(None, |guard| guard.clone())
    }

    /// 服务端肯不肯为这个账户跑这项能力、不肯的话是谁的决定（wire 名：`chat` / `translate` /
    /// `embed` / `transcribe`…）。两样都不知道 = 肯；真被拒时服务端回错，路由当场回落本机。
    /// 部署没配在前：上游一个都没有时，组织开没开放已经无关紧要（ADR-041 ①）。
    pub(crate) fn server_offer(&self, capability: &str) -> ServerOffer {
        if !self
            .cloud_deployment()
            .is_none_or(|facts| facts.serves(capability))
        {
            return ServerOffer::NotConfigured;
        }
        let allowed = self.cloud_capabilities.lock().map_or(true, |guard| {
            guard
                .as_ref()
                .is_none_or(|list| list.iter().any(|c| c == capability))
        });
        if allowed {
            ServerOffer::Serves
        } else {
            ServerOffer::NotAllowed
        }
    }

    /// 服务器肯为这个账户跑这项能力吗（见 [`Self::server_offer`]）。
    pub(crate) fn server_serves(&self, capability: &str) -> bool {
        self.server_offer(capability) == ServerOffer::Serves
    }

    /// 这台服务端一个上游都没登记（ADR-041 ①）。不知道 = 否。
    pub(crate) fn server_setup_incomplete(&self) -> bool {
        self.cloud_deployment()
            .is_some_and(|facts| facts.setup_incomplete)
    }

    /// 服务端此刻肯为这个账户跑的能力：账户开放的 ∩ 部署配好的（给 Provider 装配过滤能力用；
    /// `None` = 两样都不知道，不限）。
    pub(crate) fn served_capabilities(&self) -> Option<Vec<String>> {
        let allowed = self
            .cloud_capabilities
            .lock()
            .map_or(None, |guard| guard.clone());
        match (allowed, self.cloud_deployment()) {
            (allowed, None) => allowed,
            (None, Some(facts)) => Some(facts.served),
            (Some(allowed), Some(facts)) => Some(
                allowed
                    .into_iter()
                    .filter(|capability| facts.serves(capability))
                    .collect(),
            ),
        }
    }

    /// 当前库代际号（异步写回发起时取，提交前用 [`AppState::still_current_vault`] 校验）。
    pub(crate) fn vault_generation(&self) -> u64 {
        self.vault_generation
            .load(std::sync::atomic::Ordering::Acquire)
    }

    /// 对话上下文窗（token）；`0` = 尚未探测。
    pub(crate) fn chat_ctx_window(&self) -> u32 {
        self.chat_ctx_window
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// 记下探测到的对话上下文窗。
    pub(crate) fn set_chat_ctx_window(&self, window: u32) {
        self.chat_ctx_window
            .store(window, std::sync::atomic::Ordering::Relaxed);
    }

    /// 发起时取的代际是否仍是当前库——`false` = 期间切过库，写回必须放弃（ADR-025）。
    pub(crate) fn still_current_vault(&self, generation: u64) -> bool {
        self.vault_generation() == generation
    }

    /// 切库时自增代际，使一切在途异步写回失效。
    ///
    /// 调用方必须持有 Vault 写锁；参数只用于让这一约束成为类型事实。最终发布也在同一把锁内
    /// 复验代际，因此“复验通过”与“切库作废”二者只能有一个先发生。
    pub(crate) fn bump_vault_generation_locked(
        &self,
        _vault: &std::sync::MutexGuard<'_, Option<Vault>>,
    ) {
        self.vault_generation
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    }

    /// 未打开 Vault 时的统一错误——**按此刻的真实原因措辞**（PRD-06-049 边界⑤）：
    /// 切库过渡期说「正在切换」（稍候即好），其余才是「尚未选择」（要去引导/设置）。
    /// 两种状态给用户的行动指引完全不同，混为一句就是误导。
    pub(crate) fn vault_not_open(&self) -> String {
        vault_absent_message(
            self.copy(),
            self.vault_switching
                .load(std::sync::atomic::Ordering::Acquire),
        )
    }

    /// 装配后端文案来源——**只在 app setup 里调一次**（见字段头注）。
    ///
    /// 重复调用被静默忽略（`OnceLock` 语义）：这不是宽容，而是「第二次调用一定是错的」
    /// 的最安全表现——覆盖会让两处 `AppState` 视角在语言切换后分叉。
    pub(crate) fn attach_backend_copy(&self, i18n: Arc<crate::i18n::BackendI18n>) {
        let _ = self.backend_copy.set(i18n);
    }

    /// 后端文案取用来源；`None` = 尚未装配（测试构造的 `AppState`，或 setup 之前）。
    pub(crate) fn copy(&self) -> BackendCopy<'_> {
        self.backend_copy.get().map(std::convert::AsRef::as_ref)
    }

    /// 同上，但给 `'static` 闭包捕获用（录音归档、后台落盘…）。
    ///
    /// 与 [`Self::copy`] 是同一个对象的两种取法，不是两份文案来源：借用形态给同步
    /// 调用，`Arc` 形态给跨线程闭包——各复制一份的话，切语言之后两者会分叉。
    pub(crate) fn backend_copy_arc(&self) -> Option<Arc<crate::i18n::BackendI18n>> {
        self.backend_copy.get().map(Arc::clone)
    }

    /// 过渡态标志句柄——给 `'static` 采集落盘闭包捕获，执行那一刻读真值。
    /// 录音控制器此刻是否在会话中（录音 / 暂停 / 停止中；PRD-15-032 抑制判据）。
    pub(crate) fn recording_in_session(&self) -> bool {
        matches!(
            self.recording.state().status,
            crate::recording::RecordingStatus::Recording
                | crate::recording::RecordingStatus::Paused
                | crate::recording::RecordingStatus::Stopping
        )
    }

    pub(crate) fn vault_switching_flag(&self) -> Arc<std::sync::atomic::AtomicBool> {
        Arc::clone(&self.vault_switching)
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn desktop_lens_activate(&self) {
        self.desktop_lens.activate();
    }

    #[must_use]
    pub(crate) fn desktop_lens_active(&self) -> bool {
        self.desktop_lens.active()
    }

    #[must_use]
    pub(crate) fn desktop_lens_enabled(&self) -> bool {
        self.settings_snapshot()
            .get_bool("labs.translate_space_enabled")
            .unwrap_or(false)
    }

    pub(crate) fn desktop_lens_end(&self) {
        self.desktop_lens.end();
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn desktop_lens_attach_ocr(&self, cancel: CancelToken) -> Option<u64> {
        self.desktop_lens.attach_ocr(cancel)
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn desktop_lens_finish_ocr(&self, generation: u64) {
        self.desktop_lens.finish_ocr(generation);
    }

    /// 随包推理引擎的监管器（ADR-143）。
    #[must_use]
    pub(crate) fn local_engine(&self) -> &luna_engine::llama::LlamaServerSupervisor {
        &self.local_engine
    }

    /// 最近一次引擎编排的结论（兼编排串行锁，见字段注释）。
    pub(crate) fn local_engine_plan(&self) -> &tokio::sync::Mutex<crate::local_engine::EnginePlan> {
        &self.local_engine_plan
    }

    /// 随包引擎已装模型的最近一次清单。
    #[must_use]
    pub(crate) fn engine_models_snapshot(&self) -> Vec<crate::local_engine::EngineModelRef> {
        self.engine_models
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default()
    }

    /// 记下引擎已装清单；返回 `true` = 清单**变了**。
    pub(crate) fn record_engine_models(
        &self,
        models: Vec<crate::local_engine::EngineModelRef>,
    ) -> bool {
        let Ok(mut guard) = self.engine_models.lock() else {
            return false;
        };
        let changed = *guard != models;
        *guard = models;
        changed
    }

    /// 随包引擎是否熔断（反复起不来，等用户在本地模型页手动重启）。
    #[must_use]
    pub(crate) fn local_engine_tripped(&self) -> bool {
        matches!(
            self.local_engine.status().state,
            luna_engine::llama::EngineState::Tripped
        )
    }

    /// 本机默认对话档此刻能不能用：槽位解析出的目标是已装、有对话能力的模型库条目，
    /// 且随包引擎没有熔断。设置引导、知识库推荐、启动核查共用这一个判据（ADR-143）。
    #[must_use]
    pub(crate) fn local_chat_ready(&self, app: &tauri::AppHandle) -> bool {
        let settings = self.settings_snapshot();
        let store = crate::model_store::ModelStore::load(self.models_root(app));
        let target = self.effective_assignment(&settings, "default_llm");
        models::assignment_readiness(&store, "default_llm", target.as_deref(), "chat", &settings)
            == "ready"
            && !self.local_engine_tripped()
    }

    /// 本机此刻能不能翻译：问的是「翻译与实时档」自己（装了专用翻译模型就指向它，否则继承
    /// 默认对话档），所需能力是 `translate`——**不是**「有没有通用对话模型」。只装了
    /// Hy-MT2 的机器翻译照常可用，拿 [`Self::local_chat_ready`] 去判会把它说成跑不起来
    /// （引导「第一个任务」就会把用户引去下 2.5 GB 的对话模型）。
    pub(crate) fn local_translate_ready(&self, app: &tauri::AppHandle) -> bool {
        let settings = self.settings_snapshot();
        let store = crate::model_store::ModelStore::load(self.models_root(app));
        let target = self.effective_assignment(&settings, "realtime_llm");
        models::assignment_readiness(
            &store,
            "realtime_llm",
            target.as_deref(),
            "translate",
            &settings,
        ) == "ready"
            && !self.local_engine_tripped()
    }

    /// Provider 侧的引擎地址事实源（每次请求现问监管器）。
    #[must_use]
    pub(crate) fn local_engine_source(&self) -> Arc<dyn luna_provider::LocalEngineSource> {
        Arc::new(crate::local_engine::SupervisorSource(
            self.local_engine.clone(),
        ))
    }

    /// 工件下载接缝句柄（`Arc` 克隆，TDD-14 §3.2 ②）：模型安装经它取字节到本地文件。
    #[must_use]
    pub(crate) fn artifact_fetcher(&self) -> Arc<dyn crate::artifact_fetcher::ArtifactFetcher> {
        Arc::clone(&self.artifact_fetcher)
    }

    /// 槽位此刻的有效目标（设备策略档按本机已装清单挑，ADR-059）。
    ///
    /// 全仓**唯一**该被问「这个槽位现在指向谁」的入口：装配、DTO 投影与就绪判定都走它，
    /// 于是「界面显示的模型」与「真正装进 Provider 的模型」不可能是两个答案。
    #[must_use]
    pub(crate) fn effective_assignment(&self, settings: &Settings, slot: &str) -> Option<String> {
        effective_assignment_target_with(settings, &self.engine_models_snapshot(), slot)
    }

    /// 当前 LunaManaged 模型根目录。安全搬移状态机落地前固定为 `<app_data>/models`；
    /// 不暴露一个会在未搬文件时直接改指针的伪 Path 设置。
    pub(crate) fn models_root(&self, app: &tauri::AppHandle) -> PathBuf {
        let app_data_dir = app.path().app_data_dir().ok();
        self.settings
            .lock()
            .map(|settings| models_root_from_settings(&settings, app_data_dir))
            .unwrap_or_else(|_| std::env::temp_dir().join("luna-models"))
    }

    /// 采集会话用哪个 VAD（ADR-012 / ADR-077）：设置选了 Silero 且权重在场 → Silero 工厂；
    /// 否则 `None` = 能量 VAD。选了却没权重时打一行日志，不静默。
    pub(crate) fn vad_factory(&self, app: &tauri::AppHandle) -> Option<luna_audio::VadFactory> {
        let settings = self.settings_snapshot();
        if settings.get_str("recording.vad_engine").as_deref() != Some("silero") {
            return None;
        }
        let sensitivity = luna_audio::VadSensitivity::from_setting(
            &settings
                .get_str("recording.vad_sensitivity")
                .unwrap_or_default(),
        );
        let root = self.models_root(app);
        let factory = crate::silero_vad::vad_factory(&root, sensitivity, 2);
        if factory.is_none() {
            eprintln!(
                "[vad] 设置选了 Silero 但权重不在场，本次会话用能量 VAD（去模型页安装 Silero VAD）"
            );
        }
        factory
    }

    /// 是否处于严格本地模式（拦截联网型动作，AGENTS.md §7）。
    ///
    /// `pub(crate)`：云门控（`crate::cloud`）读**同一份内存态**判定，不再像跨仓时期
    /// 那样重新解析配置文件（ADR-037 合并后该 workaround 退役）。
    pub(crate) fn strict_local(&self) -> bool {
        self.settings
            .lock()
            .map(|settings| settings.strict_local())
            .unwrap_or(true)
    }

    /// 取弹窗会话锁：供双击/选区原生编排把捕获到的源文本写入会话（决策 A，见 `lib` 编排）。
    #[must_use]
    pub fn popup(&self) -> &Arc<Mutex<PopupSession>> {
        &self.popup
    }

    /// 录音控制器（全局快捷键的打点动作与 HUD 走同一条控制通道）。
    pub fn recording(&self) -> &Arc<RecordingController> {
        &self.recording
    }

    /// 取弹窗失焦收起状态机锁：供窗口 show/focus 事件接线使用。
    #[must_use]
    pub fn popup_blur(&self) -> &Mutex<PopupBlurState> {
        &self.popup_blur
    }

    #[must_use]
    pub fn queue_handle(&self) -> Arc<JobQueue> {
        Arc::clone(&self.queue)
    }

    #[must_use]
    pub fn mic_handle(&self) -> Arc<Mutex<MicArbiter>> {
        Arc::clone(&self.mic)
    }

    pub fn cancel_voice_holder(&self, holder: MicHolder) -> Result<(), String> {
        self.voice_input.cancel_holder(holder)
    }

    /// 按**窗口身份**解析命令的落点库（PRD-06-050）：副库窗口（`vault-window-*`）
    /// 落它自己的 Vault 实例，其余窗口（主窗 / note-editor / HUD）恒落主库。
    /// 前端零改动零业务——窗口句柄由 Tauri 自动注入，绑定不变。
    pub(crate) fn with_vault<R>(
        &self,
        window_label: &str,
        f: impl FnOnce(&Vault) -> Result<R, String>,
    ) -> Result<R, String> {
        if window_label.starts_with(VAULT_WINDOW_PREFIX) {
            let handle = self
                .secondary_vaults
                .lock()
                .map_err(|_| "副库表锁中毒".to_string())?
                .get(window_label)
                .cloned()
                .ok_or_else(|| {
                    coded_error_with(self.copy(), "VAULT_WINDOW_GONE", "vault-window-gone")
                })?;
            let vault = handle.lock().map_err(|_| "副库锁中毒".to_string())?;
            return f(&vault);
        }
        let guard = self.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| self.vault_not_open())?;
        f(vault)
    }

    /// 该窗口是否是副库窗口（write_note 的自动 AI 打标钩子等只对主库触发）。
    pub(crate) fn is_secondary_window(label: &str) -> bool {
        label.starts_with(VAULT_WINDOW_PREFIX)
    }

    /// 按库根找已开的副库窗口 label（同库去重聚焦；主库切到该根时先关它）。
    pub(crate) fn secondary_window_for_root(&self, root: &Path) -> Option<String> {
        self.secondary_vaults
            .lock()
            .ok()?
            .iter()
            .find(|(_, v)| v.lock().is_ok_and(|v| v.root() == root))
            .map(|(label, _)| label.clone())
    }

    /// 此刻开着的副库窗口（label，库根）：换了账户后据此关掉新账户看不见的那些。
    pub(crate) fn secondary_window_roots(&self) -> Vec<(String, PathBuf)> {
        self.secondary_vaults
            .lock()
            .map(|map| {
                map.iter()
                    .filter_map(|(label, vault)| {
                        let root = vault.lock().ok()?.root().to_path_buf();
                        Some((label.clone(), root))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 副库窗口销毁时摘除其 Vault 实例。
    pub(crate) fn forget_vault_window(&self, label: &str) {
        if let Ok(mut map) = self.secondary_vaults.lock() {
            map.remove(label);
        }
    }

    /// 弹出编辑器窗口销毁时清理 label→path 映射（防悬挂条目）。
    pub(crate) fn forget_note_window(&self, label: &str) {
        if let Ok(mut map) = self.note_windows.lock() {
            map.remove(label);
        }
    }

    /// 某篇速记此刻的便签窗口 label（已钉住即返回，用于"再钉一次 = 聚焦那一扇"）。
    pub(crate) fn desktop_note_window_of(&self, path: &str) -> Option<String> {
        self.desktop_note_windows
            .lock()
            .ok()?
            .iter()
            .find(|(_, rel)| rel.as_str() == path)
            .map(|(label, _)| label.clone())
    }

    /// 记下一扇新便签窗。
    pub(crate) fn remember_desktop_note_window(&self, label: &str, path: &str) {
        if let Ok(mut map) = self.desktop_note_windows.lock() {
            map.insert(label.to_owned(), path.to_owned());
        }
    }

    pub(crate) fn forget_desktop_note_window(&self, label: &str) {
        if let Ok(mut map) = self.desktop_note_windows.lock() {
            map.remove(label);
        }
    }

    /// 取一扇隐藏着的便签窗来复用；池子空了返回 `None`（调用方新建一扇）。
    pub(crate) fn take_idle_desktop_note_window(&self) -> Option<String> {
        self.desktop_note_idle.lock().ok()?.pop()
    }

    /// 把一扇便签窗放回池子（取消钉住时；它此刻已经 `hide()` 过了）。
    pub(crate) fn park_desktop_note_window(&self, label: &str) {
        if let Ok(mut idle) = self.desktop_note_idle.lock() {
            if !idle.iter().any(|l| l == label) {
                idle.push(label.to_owned());
            }
        }
    }

    /// 此刻钉在桌面上的那几篇（速记清单一次取齐，逐行查表会拿 N 次锁）。
    pub(crate) fn pinned_desktop_note_paths(&self) -> std::collections::HashSet<String> {
        self.desktop_note_windows
            .lock()
            .map(|map| map.values().cloned().collect())
            .unwrap_or_default()
    }

    /// 便签窗口自己问"我是哪一篇"。
    pub(crate) fn desktop_note_path_of(&self, label: &str) -> Option<String> {
        self.desktop_note_windows.lock().ok()?.get(label).cloned()
    }

    #[must_use]
    pub fn vault_root(&self) -> Option<PathBuf> {
        self.vault
            .lock()
            .ok()
            .and_then(|vault| vault.as_ref().map(|v| v.root().to_path_buf()))
    }

    /// 取 Vault 共享句柄（`Arc` 克隆）：供后台扫描线程串行进入（SQLite Connection 经 Mutex）。
    #[must_use]
    pub(crate) fn vault_arc(&self) -> Arc<Mutex<Option<Vault>>> {
        Arc::clone(&self.vault)
    }

    /// 库代号的共享计数（后台任务据它判断「这一轮期间用户切过库」）。
    pub(crate) fn vault_generation_arc(&self) -> Arc<std::sync::atomic::AtomicU64> {
        Arc::clone(&self.vault_generation)
    }

    /// 列表账户看得见的库（官方同步逐库选择的来源，ADR-151 ①；库列表跟着账户走，同 ADR 修订
    /// 2026-10-15「续二」）。判嵌套、判下载落点要看**全部**记录（另一个账户的库也是这台设备上的
    /// 文件夹），那几处直接用 `vault_registry.list()`。
    pub(crate) fn known_vaults(&self) -> Vec<luna_vault::registry::VaultEntry> {
        self.vault_registry.visible()
    }

    /// 设备级库注册表（库列表跟着账户走的改写入口在 `crate::vault_lists`）。
    pub(crate) fn vault_registry(&self) -> &luna_vault::registry::VaultRegistry {
        &self.vault_registry
    }

    /// 统一处理队列（推迟的换账户切库据此等处理收尾，`crate::vault_lists`）。
    pub(crate) fn processing(&self) -> &crate::processing_queue::ProcessingQueue {
        &self.processing
    }

    /// 此刻打开的主库的身份与代号（官方同步据此判断会话是否还对得上）。没有打开的库 = `None`。
    pub(crate) fn sync_vault_identity(&self) -> Option<(String, u64)> {
        let generation = self.vault_generation();
        let guard = self.vault.lock().ok()?;
        guard.as_ref().map(|v| (v.id().to_string(), generation))
    }

    #[must_use]
    pub(crate) fn device_usage_store(&self) -> UsageStore {
        self.device_usage.clone()
    }

    fn list_usage_records(&self, range: UsageRange) -> Vec<luna_core::UsageRecord> {
        merged_usage_records(&self.device_usage, &self.vault, range)
    }

    fn usage_summary(&self) -> UsageSummary {
        merged_usage_summary(&self.device_usage, &self.vault)
    }

    /// 用量摘要 DTO：在此把「本地节省」估算算好（前端零业务，ADR-003）。价格取自设置
    /// （唯一事实源），前端只渲染 `estimated_savings_usd`。
    fn usage_summary_dto(&self) -> UsageSummaryDto {
        let summary = self.usage_summary();
        let settings = self.settings_snapshot();
        let asr = setting_price_usd(&settings, "provider.asr_cost_per_min");
        let llm = setting_price_usd(&settings, "provider.llm_cost_per_million_tokens");
        let savings = estimate_local_savings_usd(&summary, asr, llm);
        let server_cost = estimate_server_cost_usd(&summary, asr, llm);
        // 落地回执只在 Vault 级库（与 FloatingMenu 来源的用量记录同库）。
        let outcomes = self
            .vault
            .lock()
            .ok()
            .and_then(|guard| {
                guard
                    .as_ref()
                    .and_then(|vault| vault.usage_store().outcome_summary().ok())
            })
            .unwrap_or_default();
        UsageSummaryDto::from_summary(summary, savings, server_cost, outcomes)
    }

    fn clear_usage_records(&self) -> usize {
        clear_merged_usage(&self.device_usage, &self.vault)
    }

    /// 安装 Vault 监听：回调线程只发 typed event，不直接持有 `Vault` / SQLite Connection。
    /// 未打开 Vault 时跳过（select/create 后再次调用即生效）。
    /// 停止文件监听（切库收尾；丢弃句柄即停，与安装互为逆操作）。
    pub fn stop_vault_watcher(&self) {
        if let Ok(mut slot) = self.vault_watcher.lock() {
            *slot = None;
        }
    }

    pub fn install_vault_watcher(&self, app: tauri::AppHandle) {
        use tauri::Manager as _; // for `try_state`
        let root = match self.vault.lock() {
            Ok(vault) => match vault.as_ref() {
                Some(v) => v.root().to_path_buf(),
                None => return,
            },
            Err(_) => {
                eprintln!("[vault] Vault 锁中毒，跳过文件监听。");
                return;
            }
        };
        let app_for_assets = app.clone();
        match luna_vault::watch(
            &root,
            move |change| {
                // 自写回声抑制（§4 硬规则）：若当前磁盘指纹 == 该 path 的 last_written_hash，则这次事件
                // 是 Lunarca 自己的写（保存/AI 替换）触发的回声——不发 `vault://changed`，避免编辑器把自写
                // 误判为外部修改而弹冲突。判定借持有 Vault 的 AppState 完成（监听线程不直接持有 Connection）。
                if let Some(disk) = change.disk_hash.as_ref() {
                    if let Some(state) = app.try_state::<AppState>() {
                        if let Ok(guard) = state.vault.lock() {
                            if let Some(vault) = guard.as_ref() {
                                if vault.last_written_hash(&change.rel_path).as_ref() == Some(disk)
                                {
                                    return;
                                }
                            }
                        }
                    }
                }
                let app_for_change = app.clone();
                tauri::async_runtime::spawn_blocking(move || {
                    handle_external_vault_change(&app_for_change, change);
                });
            },
            // 附件（PRD-06-012 ③）：库外删掉 / 放回一张图，正文里的引用要重新判断指不指得到、
            // 文件树要跟着增删。只广播这一条边界——不进全文索引，也不触发知识地图与度量这类
            // 只跟笔记有关的派生刷新（一次 `git checkout` 能带出成百个附件事件）。
            move |asset| {
                let _ = (VaultChanged {
                    rel_path: asset.rel_path,
                    deleted: asset.deleted,
                })
                .emit(&app_for_assets);
            },
        ) {
            Ok(watcher) => {
                if let Ok(mut slot) = self.vault_watcher.lock() {
                    *slot = Some(watcher);
                }
            }
            Err(e) => eprintln!("[vault] 文件监听启动失败：{e}"),
        }
    }

    /// 由当前设置构造弹窗翻译的语言基准（母语 / 默认外语）。
    fn lang_context(&self) -> LangContext {
        let settings = self.settings_snapshot();
        lang_context_from_settings(&settings)
    }

    /// 处理队列是否有待跑任务（启动时据此决定要不要续跑，PRD-05-012）。
    #[must_use]
    pub fn has_pending_processing(&self) -> bool {
        self.processing.has_pending()
    }

    /// 启动时打不开的已配置库根（见字段注释）。
    #[must_use]
    pub(crate) fn startup_vault_missing(&self) -> Option<&Path> {
        self.startup_vault_missing.as_deref()
    }

    /// 启动时没打开那个库，是因为它的 `.luna/` 还在 iCloud 上（见字段注释）。
    #[must_use]
    pub(crate) fn startup_vault_in_cloud(&self) -> bool {
        self.startup_vault_in_cloud
    }

    pub(crate) fn settings_snapshot(&self) -> Settings {
        self.settings
            .lock()
            .map(|settings| settings.clone())
            .unwrap_or_else(|_| Settings::with_defaults())
    }
}

/// 模型分配槽的稳定字符串。键与继承关系只在此处定义，所有 Provider/IPC/设置投影共用。
const MODEL_ASSIGNMENT_SLOTS: &[(&str, &str, Option<&str>, &str)] = &[
    ("default_llm", "ai.assignment.default_llm", None, "chat"),
    // 「翻译与实时档」：全部翻译都落这里（D3），故要的是翻译能力——专用翻译模型（Hy-MT2）
    // 只声明 translate，通用对话模型四种都声明。
    (
        "realtime_llm",
        "ai.assignment.realtime_llm",
        Some("default_llm"),
        "translate",
    ),
    (
        "realtime_asr",
        "ai.assignment.realtime_asr",
        None,
        "transcribe",
    ),
    (
        "batch_asr",
        "ai.assignment.batch_asr",
        Some("realtime_asr"),
        "transcribe",
    ),
    (
        "default_embedding",
        "ai.assignment.default_embedding",
        None,
        "embed",
    ),
];

const DEVICE_POLICY_ASR: &str = crate::model_store::BUNDLED_WHISPER_ID;
/// 唯一通过分块基准、可进生产索引的本机嵌入模型（`ipc::embed` 的 profile 表）。
///
/// 它**没有**对话槽那样的「退到已装的另一个」——生产可用的本机嵌入模型当前只有这一个，
/// 换一个就是换一个向量空间（`resolve_embedding_identity`：身份变即全库重建）。所以嵌入
/// 这一档拿不到时唯一诚实的答案是「去装 bge-m3」，而不是随手换一个能返回向量的模型。
const DEVICE_POLICY_EMBED: &str = crate::model_store::RECOMMENDED_EMBED;

fn assignment_meta(slot: &str) -> Option<(&'static str, Option<&'static str>, &'static str)> {
    MODEL_ASSIGNMENT_SLOTS
        .iter()
        .find(|(candidate, ..)| *candidate == slot)
        .map(|(_, key, inherits, capability)| (*key, *inherits, *capability))
}

fn configured_assignment(settings: &Settings, slot: &str) -> Option<String> {
    let (key, _, _) = assignment_meta(slot)?;
    settings
        .get_str(key)
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// 用户没有显式选过时，这个槽位默认落在哪。
///
/// # 默认值跟着**算力落点**走
///
/// 连上服务端之后，用户期望的是「AI 就在服务端跑」，而不是再去五个槽位里逐个挑。
/// 但这条只对**转写**需要在这里表达——对话与嵌入的落点由端点解析决定
/// （`compute_endpoint_source` 派出端点即走服务端），槽位存的是发给端点的 model 串，
/// 值不必变。
///
/// 转写的默认值**额外要求一次同意**（`provider.remote_asr_enabled`，ADR-038 ④）：
/// 连服务端可能只为了对话或同步，而麦克风与会议录音是另一个量级的东西。
/// 没有那次同意时，默认仍是随包的本机兜底模型。
/// 对话档**按已装清单挑**（ADR-059/072）：`installed = None` 表示「还没问到过引擎」，
/// 那时使用推荐目录中的最小档；`Some` 时由 [`crate::model_store::device_policy_llm`] 裁决，
/// 一台没有任何可用本机对话模型的机器如实得到 `None`（「未配置」是诚实的）。
fn device_policy_target(
    settings: &Settings,
    installed: &[crate::local_engine::EngineModelRef],
    slot: &str,
) -> Option<String> {
    let transcribe_on_server = server_transcription_allowed(settings);
    match slot {
        "default_llm" => {
            crate::model_store::device_policy_llm(crate::hardware::probe_tier(), installed)
        }
        // 「翻译与实时档」：装了专用翻译模型才有自己的设备策略，否则继承默认档（见调用处）。
        "realtime_llm" => crate::model_store::device_policy_translation(installed),
        "realtime_asr" if transcribe_on_server => Some(REMOTE_ASR_TARGET.to_owned()),
        "realtime_asr" => Some(DEVICE_POLICY_ASR.to_owned()),
        "default_embedding" => Some(DEVICE_POLICY_EMBED.to_owned()),
        _ => None,
    }
}

/// 解析**不受本机已装清单影响**的槽位（转写两档 + 嵌入）。
///
/// 对话两档（`default_llm` / `realtime_llm`）不得走这里：它们的设备策略档要按已装
/// 清单挑（ADR-059），从这里问会拿到「引擎还没问到」那一档的答案，而调用方明明有
/// `AppState`。走 [`AppState::effective_assignment`]。debug 断言让走错的人当场炸，
/// 而不是在真机上表现为「界面说没装、其实装了」。
pub(crate) fn effective_assignment_target(settings: &Settings, slot: &str) -> Option<String> {
    debug_assert!(
        !matches!(slot, "default_llm" | "realtime_llm"),
        "对话槽的设备策略档依赖本机已装清单，请走 AppState::effective_assignment（ADR-059）"
    );
    effective_assignment_target_with(settings, &[], slot)
}

/// 解析配置、继承和设备策略后的目标；不检查安装/能力，运行就绪由模型目录统一裁决。
///
/// `installed` 只喂给设备策略那一层：**显式选择与继承永远原样返回**，绝不因为「那个模型
/// 没装」就换一个（TDD-04 §2.4「不暗换模型」）。
pub(crate) fn effective_assignment_target_with(
    settings: &Settings,
    installed: &[crate::local_engine::EngineModelRef],
    slot: &str,
) -> Option<String> {
    if let Some(target) = configured_assignment(settings, slot) {
        return Some(target);
    }
    // 「翻译与实时档」的设备策略先于继承：装了专用翻译模型就用它（ADR-145 推荐槽位）。
    if slot == "realtime_llm" {
        if let Some(target) = device_policy_target(settings, installed, slot) {
            return Some(target);
        }
    }
    if let Some((_, Some(parent), _)) = assignment_meta(slot) {
        return effective_assignment_target_with(settings, installed, parent);
    }
    device_policy_target(settings, installed, slot)
}

/// 这条继承链上（本槽或任一祖先）是否存在显式选择。
///
/// 服务端托管（ADR-048）只对「整条链都没有显式立场」的槽位成立：实时 LLM 未配置而
/// 默认 LLM 显式指向本机时，实时任务继承的是那个本机模型——只看槽位自身的空值就会把它
/// 标成「服务端托管」，屏幕与字节去向相反。
pub(crate) fn assignment_chain_configured(settings: &Settings, slot: &str) -> bool {
    if configured_assignment(settings, slot).is_some() {
        return true;
    }
    assignment_meta(slot)
        .and_then(|(_, parent, _)| parent)
        .is_some_and(|parent| assignment_chain_configured(settings, parent))
}

/// 本机对话目标：随包推理引擎里的一个对话条目 id（= preset 节名，ADR-143）。
///
/// 存在的理由：槽位里的裸 id 只有在引擎清单里登记为对话模型时才是本机对话目标（whisper
/// 等裸 id 不是）；把这条判据收成一个类型，装配与实时档定向就不会各写一份。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LocalLlm(pub(crate) String);

impl LocalLlm {
    /// 槽位目标 → 本机对话目标；不是引擎里的对话条目（含 `service:`）为 `None`。
    #[must_use]
    pub(crate) fn from_target(
        target: &str,
        engine: &[crate::local_engine::EngineModelRef],
    ) -> Option<Self> {
        use crate::model_target::ModelTarget;
        match ModelTarget::parse(target)? {
            ModelTarget::StoreEntry(id) => engine
                .iter()
                .any(|m| m.id == id && m.kind == crate::local_engine::EngineModelKind::Chat)
                .then(|| Self(id.to_owned())),
            ModelTarget::Service(_) => None,
        }
    }

    /// 装进注册表的 Provider（`realtime` = 实时档，只接显式定向）。
    #[must_use]
    pub(crate) fn provider(
        &self,
        state: &AppState,
        realtime: bool,
    ) -> Arc<dyn luna_core::AIProvider> {
        use luna_provider::{LlmHttpConfig, LlmHttpProvider};
        let source = state.local_engine_source();
        let config = if realtime {
            LlmHttpConfig::local_engine_realtime(source, self.0.clone())
        } else {
            LlmHttpConfig::local_engine(source, self.0.clone())
        };
        Arc::new(LlmHttpProvider::new(config))
    }

    /// 实时档定向用的 Provider id。
    #[must_use]
    pub(crate) fn realtime_provider_id(&self) -> luna_core::ProviderId {
        luna_core::ProviderId::from(luna_provider::LOCAL_LLM_REALTIME_PROVIDER_ID)
    }
}

/// 某个对话槽此刻的本机对话目标。
#[must_use]
pub(crate) fn local_llm_for_slot(
    settings: &Settings,
    installed: &[crate::local_engine::EngineModelRef],
    slot: &str,
) -> Option<LocalLlm> {
    effective_assignment_target_with(settings, installed, slot)
        .as_deref()
        .and_then(|target| LocalLlm::from_target(target, installed))
}

/// 实时翻译管线需要单独定向注册的目标。`None` = 走默认质量档。
///
/// **「与默认档同名即走默认档」只在默认档确实跑在本机时成立**（ADR-048 之后名字
/// 不再决定执行位置）：默认档由服务端托管时，`effective_llm_model` 返回的只是设备策略
/// 的本机名，与用户显式选的实时本机模型撞名就会把这条显式本机选择静默送上服务端——
/// 屏幕写「这台设备」而字节出网，是最坏的那个方向。故先判执行位置，再谈同名合并。
#[must_use]
pub(crate) fn realtime_local_llm(
    settings: &Settings,
    installed: &[crate::local_engine::EngineModelRef],
    availability: crate::cloud::CloudAvailability,
    server: &crate::cloud::ServerRef,
) -> Option<LocalLlm> {
    let default_on_device =
        !models::chat_compute_location(settings, availability, server).is_server();
    // 实时链上没有任何显式选择：默认档在服务端时完全跟随它；在本机时，装了专用翻译模型就
    // 定向到它（设备策略），否则跟随默认档。
    if configured_assignment(settings, "realtime_llm").is_none() && !default_on_device {
        return None;
    }
    let realtime = local_llm_for_slot(settings, installed, "realtime_llm")?;
    if default_on_device
        && Some(&realtime) == local_llm_for_slot(settings, installed, "default_llm").as_ref()
    {
        return None;
    }
    Some(realtime)
}

#[must_use]
fn vault_settings_path(root: &Path) -> PathBuf {
    root.join(LUNA_VAULT_PRIVATE_DIR).join("settings.toml")
}

/// 把 Vault 作用域**换源**到指定库（ADR-025 / TDD-06 §12）。
///
/// **换源不是叠加**：先把 Vault 作用域整体归零到注册表默认，再叠加本库文件里写过的键。
/// 配置文件是稀疏的，只 merge 会让新库没写的键残留上一个库的取值——归档目录、蒸馏三档
/// 指着另一个库的配置，是静默的跨库泄漏（新库连 `settings.toml` 都没有时整片残留）。
#[must_use]
fn settings_with_vault_scope_from_root(mut settings: Settings, root: &Path) -> Settings {
    settings.reset_scope_to_defaults(SettingScope::Vault);
    let path = vault_settings_path(root);
    if !path.is_file() {
        return settings;
    }
    let (vault_settings, outcome) = Settings::load_path_for_scope(&path, SettingScope::Vault);
    match outcome {
        // 退役键**不回写**：Vault 文件随库同步，另一台装着新版本的设备写下的键，这一台
        // 不认识不等于它该消失。
        LoadOutcome::Loaded | LoadOutcome::LoadedWithRetired(_) => {
            settings.merge_scope_from(&vault_settings, SettingScope::Vault);
        }
        // 个别项过期同样**照常合并**——过期项已在解析时回退默认，其余项没有理由一起丢。
        LoadOutcome::LoadedWithDropped(dropped) => {
            let keys: Vec<&str> = dropped.iter().map(|d| d.key.as_str()).collect();
            eprintln!(
                "[config] Vault 级配置 {} 项取值已过期、回退默认（path={}）：{}",
                dropped.len(),
                path.display(),
                keys.join(", ")
            );
            settings.merge_scope_from(&vault_settings, SettingScope::Vault);
        }
        LoadOutcome::CreatedDefault => {}
        LoadOutcome::Recovered(reason) => eprintln!(
            "[config] Vault 级配置结构损坏，已忽略（path={}）：{reason}",
            path.display()
        ),
    }
    settings
}

/// 换源并返回**取值真正变化了的键**（供调用方逐键广播 `settings://changed`）。
///
/// 逐键广播而非另造一个"整片刷新"事件：各订阅表面本就按 key 前缀过滤（字幕外观、回放
/// 偏好、归档目录…），逐键广播让它们的既有过滤自然生效；新增一种事件语义则要求每个
/// 订阅者再写一条分支，且新表面必然漏接。
fn merge_vault_settings_from_root(state: &AppState, root: &Path) -> Result<Vec<String>, String> {
    let mut guard = state
        .settings
        .lock()
        .map_err(|_| "Settings 锁中毒".to_string())?;
    let before = guard.clone();
    let next = settings_with_vault_scope_from_root(before.clone(), root);
    let changed = luna_config::SETTINGS
        .iter()
        .filter(|s| s.scope == SettingScope::Vault)
        .filter(|s| before.get(s.key) != next.get(s.key))
        .map(|s| s.key.to_owned())
        .collect();
    *guard = next;
    Ok(changed)
}

fn persist_vault_settings(state: &AppState) -> Result<(), String> {
    let root = state.vault_root().ok_or_else(|| state.vault_not_open())?;
    let settings = state.settings_snapshot();
    settings
        .save_path_for_scope(&vault_settings_path(&root), SettingScope::Vault)
        .map_err(|e| format!("{SETTING_PERSIST}: {e}"))
}

#[must_use]
pub(crate) fn whisper_model_path_from_settings_and_root(
    settings: &Settings,
    root: &Path,
) -> Option<PathBuf> {
    whisper_model_path_for_assignment(settings, root, "realtime_asr")
}

pub(crate) fn whisper_model_path_for_assignment(
    settings: &Settings,
    root: &Path,
    slot: &str,
) -> Option<PathBuf> {
    effective_assignment_target(settings, slot).and_then(|target| {
        let weight = if target == crate::model_store::BUNDLED_WHISPER_ID {
            luna_managed_weight_path(root, crate::model_store::BUNDLED_WHISPER_ID)
        } else if let Some(spec) = remote_model_spec(&target) {
            crate::model_store::spec_weight_path(root, spec)
        } else {
            return None;
        };
        weight.is_file().then_some(weight)
    })
}

/// 显式配置的 Vault 根：`LUNA_VAULT_ROOT`（非空）优先，否则 Device 级启动指针 `storage.vault_root`。
/// **无默认回退**——未配置返回 `None`，由首启引导选择/创建（PRD-14 门控）。
#[must_use]
fn explicit_vault_root(settings: &Settings) -> Option<PathBuf> {
    std::env::var(LUNA_VAULT_ROOT_ENV)
        .ok()
        .filter(|p| !p.trim().is_empty())
        .or_else(|| {
            settings
                .get_str("storage.vault_root")
                .filter(|p| !p.trim().is_empty())
        })
        .map(PathBuf::from)
}

/// 从设置解析快照保留份数（缺省回落 `DEFAULT_MAX_VERSIONS`）。
#[must_use]
fn snapshot_max_from_settings(settings: &Settings) -> usize {
    settings
        .get("vault.snapshot_max_versions")
        .and_then(|v| v.as_int())
        .filter(|&n| n > 0)
        .map(|n| n as usize)
        .unwrap_or(luna_vault::DEFAULT_MAX_VERSIONS)
}

#[must_use]
fn vault_delete_mode_from_settings(settings: &Settings) -> DeleteMode {
    match settings
        .get_str("vault.delete_mode")
        .unwrap_or_else(|| "trash".into())
        .as_str()
    {
        "vault_trash" | "vault-trash" => DeleteMode::VaultTrash,
        "permanent" => DeleteMode::Permanent,
        _ => DeleteMode::Trash,
    }
}

/// 各笔记类型的归档目录（Vault 相对路径，设置驱动；PRD-09 §2.6 存储/隐私）。
///
/// 新建/归档路径统一在此决定（ADR-003：前端不拼路径）；非法设置值回退注册表默认。
#[must_use]
pub(crate) fn note_type_dir(settings: &Settings, note_type: NoteType) -> String {
    let (key, fallback) = match note_type {
        NoteType::Note => ("vault.notes_dir", "Notes"),
        NoteType::QuickNote => ("vault.quick_notes_dir", "QuickNotes"),
        NoteType::Recording => ("vault.recordings_dir", "Recordings"),
        NoteType::Chat => ("vault.chats_dir", "Chats"),
        NoteType::Transcript => ("vault.transcripts_dir", "Transcripts"),
        NoteType::Document => ("vault.documents_dir", "Documents"),
        // 创作空间（ADR-162 ②）：项目文件夹直接建在创作目录下，脚本跟着项目走；
        // 单独新建的脚本也落在这里。灵感条目在创作目录下单独一个子文件夹。
        NoteType::VideoProject | NoteType::Script => ("vault.creator_dir", "Creator"),
        NoteType::Swipe => {
            let creator = safe_vault_relative_dir(settings, "vault.creator_dir", "Creator");
            return format!("{creator}/{CREATOR_SWIPE_SUBDIR}");
        }
    };
    safe_vault_relative_dir(settings, key, fallback)
}

/// 灵感条目在创作目录下的子文件夹名（与其它归档目录的默认名同为英文，见注册表）。
const CREATOR_SWIPE_SUBDIR: &str = "Swipe";

/// 知识产物（主题页 / 知识地图）的**新建**落盘目录（ADR-096 ②，`vault.knowledge_dir`）。
///
/// 只管「新生的页落哪」：存量产物身份在 frontmatter、与路径无关，**不迁移**。
/// 与 [`note_type_dir`] 分开列——产物不是一种笔记类型（`luna.type` 仍是 `note`），
/// 混进那张表会让「按类型归档」的语义多出一个不是类型的分支。
#[must_use]
pub(crate) fn knowledge_product_dir(settings: &Settings) -> String {
    safe_vault_relative_dir(settings, "vault.knowledge_dir", "Knowledge")
}

/// 文件树「资料区段」的顶层归档目录（PRD-06-048①）。
///
/// **判据是 [`NoteType::is_source_material`]，不是一份写在前端的类型清单**：
/// 前端此前自带一份 `MATERIAL_TYPES = ["recording","transcript","chat"]` 并对它逐个发一次
/// `note_type_archive_dir`——那是同一条判据的第二份事实源（ADR-024 改了分层归属时，
/// 两侧不会有任何一处变红），也是三次 IPC 换一份清单。此处一次取齐。
///
/// 配置重合时只出现一次（顺序按 [`NoteType::ALL`]，即界面上的呈现顺序）。
#[must_use]
pub(crate) fn material_archive_dirs(settings: &Settings) -> Vec<String> {
    let mut dirs: Vec<String> = Vec::new();
    for note_type in NoteType::ALL {
        if !note_type.is_source_material() {
            continue;
        }
        let dir = note_type_dir(settings, note_type);
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
}

fn safe_vault_relative_dir(settings: &Settings, key: &str, fallback: &str) -> String {
    let raw = settings.get_str(key).unwrap_or_else(|| fallback.to_owned());
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.contains('\\')
        || trimmed.chars().any(|ch| ch == '\0' || ch.is_control())
    {
        return fallback.to_owned();
    }
    let path = Path::new(trimmed);
    if path.is_absolute() {
        return fallback.to_owned();
    }
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(value) => {
                parts.push(value.to_string_lossy().into_owned());
            }
            std::path::Component::CurDir => {}
            _ => return fallback.to_owned(),
        }
    }
    if parts.is_empty() {
        fallback.to_owned()
    } else {
        parts.join("/")
    }
}

/// 返回应用信息。能力清单从 `luna-core` 派生（避免在前后端各维护一份，DRY）。
#[tauri::command]
#[specta::specta]
fn app_info() -> AppInfo {
    AppInfo {
        name: "Lunarca".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        git_sha: env!("LUNARCA_GIT_SHA").to_string(),
        protocol_version: luna_cloud::protocol::CLIENT_PROTOCOL,
        build_time: env!("LUNARCA_BUILD_TIME").to_string(),
        capabilities: luna_core::Capability::ALL
            .iter()
            .map(|c| c.name().to_string())
            .collect(),
    }
}

/// 构造 tauri-specta `Builder`（命令 + 事件集合）。
///
/// 单一工厂：`run()`（挂载到 Tauri）与 `export-bindings` 二进制（headless 导出 TS）共用，
/// 保证导出的绑定与运行时注册的命令/事件**永远一致**（DRY）。
#[must_use]
pub fn builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .commands(collect_commands![
            app_info,
            list_settings,
            theme_library,
            appearance_theme,
            install_theme_package,
            uninstall_theme_package,
            theme_market,
            install_market_theme,
            check_content_updates,
            revalidate_catalogs,
            template_market,
            install_market_template,
            local_knowledge_metrics_summary,
            clear_local_knowledge_metrics,
            retry_local_knowledge_metrics,
            export_local_knowledge_metrics,
            knowledge::knowledge_suggestion_shown,
            set_setting,
            reset_setting,
            reset_setting_group,
            preview_setting_group_reset,
            shortcut_diagnostics,
            set_shortcut_capture,
            set_native_theme,
            set_backend_language,
            compute_status,
            compute_self_check,
            models::compute_resolve_handover,
            list_models,
            model_management_snapshot,
            plan_delete_model,
            start_delete_model,
            plan_install_model,
            start_install_model,
            pause_install_model,
            install_coreml_encoder,
            remove_coreml_encoder,
            assign_model,
            clear_model_assignment,
            onboarding_state,
            onboarding_mark_step,
            onboarding_complete,
            onboarding_set_reminder_dismissed,
            onboarding_sample_available,
            onboarding_transcribe_sample,
            onboarding_funnel_enter,
            onboarding_funnel,
            setup_status,
            app_bootstrap,
            startup_mark,
            startup_diagnostics,
            hardware_summary,
            default_vault_path,
            vault_path_in_onedrive,
            vault_status,
            select_vault,
            create_vault_root,
            pick_folder,
            pick_file,
            start_byte_stream,
            read_note,
            note_facts,
            write_note,
            convert_clipboard_html,
            editor_text_to_table,
            note_properties,
            note_property_edit,
            editor_flush_reply,
            note_edit_diff,
            note_merge,
            tidy_list_levels_preview,
            tidy_list_levels_apply,
            settle_note_as_knowledge,
            mark_note_as_draft,
            note_draft_state,
            list_draft_notes,
            create_note_from_text,
            create_note,
            vault_tree,
            search,
            list_usage,
            usage_summary,
            clear_usage,
            notification_history,
            notification_mark_read,
            notification_unread_count,
            clear_notification_history,
            clear_read_notification_history,
            vault_create,
            vault_move_into,
            vault_create_dir,
            vault_list_dirs,
            vault_rename,
            vault_index_status,
            cancel_vault_scan,
            vault_undo_relocation,
            vault_delete,
            vault_delete_mode,
            snapshots,
            read_snapshot,
            restore_snapshot,
            snapshot_diff,
            vault_search,
            vault_replace,
            list_trash,
            read_trash_content,
            tag_summaries,
            list_backlinks,
            list_outgoing_links,
            vault_graph,
            note_graph_tags,
            graph_rescan_preflight,
            graph_rescan_start,
            graph_rescan_cancel,
            graph_rescan_status,
            distill_status,
            distill_notes,
            task_estimate,
            distill_template_options,
            distill_template_choices,
            distill_user_templates,
            save_distill_template,
            delete_distill_template,
            distill_preflight,
            distill_batch_start,
            promote_quick_note,
            demote_to_quick_note,
            merge_notes,
            distill_tag_notes,
            distill_preview,
            distill_apply,
            list_vaults,
            forget_vault_preflight,
            forget_vault,
            rename_vault,
            vault_switch_preflight,
            rebuild_vault_index,
            data_footprint,
            clear_vault_cache,
            reset_app,
            export_settings,
            preview_settings_import,
            import_settings,
            clear_all_local_models,
            plan_storage_relocation,
            start_storage_relocation,
            cancel_storage_relocation,
            storage_relocation_status,
            quick_note_products,
            set_quick_note_adopted,
            switch_vault,
            open_vault_window,
            vault_window_info,
            graph_tag_exclusions,
            set_graph_tag_exclusion,
            note_templates,
            create_note_from_template,
            pick_template_pack,
            install_template_pack,
            template_packs,
            uninstall_template_pack,
            copy_pack_item_to_vault,
            runnable_recipes,
            run_recipe,
            restore,
            purge,
            read_vault_meta,
            write_vault_meta,
            workspace_layout,
            set_workspace_layout,
            tree_view_state,
            set_tree_view_state,
            quick_switcher,
            record_recent_note,
            create_note_named,
            orphan_assets,
            remove_orphan_assets,
            discard_pasted_assets,
            excluded_paths,
            set_excluded_path,
            obsidian_suggestion,
            accept_obsidian_suggestion,
            dismiss_obsidian_suggestion,
            duplicate_vault_entry,
            import_files_into_folder,
            spelling_check,
            spelling_suggestions,
            learn_spelling,
            set_word_goal,
            creator_board,
            creator_set_project_stage,
            creator_create_project,
            creator_default_channels,
            creator_channel_favorites,
            creator_set_channel_favorites,
            creator_project,
            creator_project_of,
            creator_set_channels,
            creator_set_archived,
            creator_rename_project,
            creator_project_deletion,
            creator_add_text_source,
            creator_link_sources,
            creator_unlink_source,
            confirm_link_import_rights,
            creator_source_guide,
            creator_research_brief,
            creator_brief_topics,
            creator_set_topic,
            creator_draft_script,
            creator_channels,
            creator_save_channel,
            creator_delete_channel,
            creator_set_publish_date,
            creator_create_example_project,
            creator_distribution,
            creator_distribute,
            creator_recording_check,
            creator_script_retakes,
            creator_set_stage_visible,
            creator_set_board_view,
            script_outline,
            script_move_section,
            script_set_broll_done,
            creator_export_broll,
            creator_publish_sources,
            creator_export_subtitles,
            creator_import_final_cut,
            creator_project_overview,
            creator_swipe_library,
            creator_suggest_swipe_patterns,
            creator_apply_swipe_patterns,
            creator_save_quote,
            creator_project_quotes,
            creator_import_material,
            pick_material_files,
            import_material_paths,
            creator_clip_web,
            script_voice_input_start,
            creator_check_chapters,
            creator_publish_checklist,
            creator_package,
            creator_check_package,
            creator_generate_package,
            creator_save_package,
            creator_generate_chapters,
            creator_save_chapters,
            teleprompter_open,
            teleprompter_content,
            teleprompter_hide,
            teleprompter_follow_start,
            teleprompter_follow_reset,
            teleprompter_follow_seek,
            teleprompter_mark,
            teleprompter_home,
            teleprompter_new_draft,
            teleprompter_snap_to_camera,
            teleprompter_import_files,
            teleprompter_estimate_seconds,
            creator_calibrate_speaking_rate,
            creator_reset_speaking_rate,
            bookmarks,
            set_bookmark,
            render_template_for_insert,
            vault_link_pending,
            vault_link_done,
            vault_note_link,
            external_rename_prompt,
            repair_external_renames,
            dismiss_external_renames,
            conflict_copy_info,
            conflict_copies,
            resolve_conflict_copy,
            request_cloud_download,
            apps_list,
            app_install_plan,
            app_install,
            app_set_enabled,
            app_uninstall,
            apps_check_updates,
            app_update,
            app_release_check,
            app_release_install,
            app_release_startup_check_prompt,
            app_release_answer_startup_check,
            workspace_rail_layout,
            set_workspace_space_visible,
            reorder_workspace_rail,
            open_note_window,
            note_window_path,
            save_pasted_asset,
            download_remote_asset,
            import_asset_files,
            import_asset_paths,
            import_glossary_entries,
            trust_import_org_config,
            upsert_glossary_entry,
            note_type_archive_dir,
            vault_material_dirs,
            chat_attach,
            chat_reset,
            chat_send,
            chat_stop,
            chat_regenerate,
            chat_save_to_note,
            chat_prefill_take,
            chat_history_list,
            chat_open_note,
            list_chat_presets,
            save_ai_result_to_note,
            save_translate_space_result_to_note,
            media_server_info,
            vault_asset_path,
            vault_link_note_target,
            vault_link_location,
            vault_resolve_wikilinks,
            vault_resolve_wikilink,
            vault_wikilink_candidates,
            vault_link_markdown,
            vault_resolve_embeds,
            popup_stop,
            submit_floating_menu,
            floating_menu_stop,
            floating_menu_background,
            floating_menu_pending,
            floating_menu_pending_clear,
            floating_menu_landed,
            floating_menu_preflight,
            floating_menu_explain,
            floating_presets_list,
            floating_preset_save,
            floating_preset_delete,
            list_popup_presets,
            popup_session_state,
            popup_set_input,
            popup_set_draft,
            popup_generate,
            popup_copy,
            clipboard_write_rich,
            read_clipboard_text,
            read_clipboard_content,
            popup_replace,
            popup_close,
            popup_set_pinned,
            popup_screenshot_translate,
            desktop_lens_state,
            desktop_lens_toggle,
            desktop_lens_set_editing,
            desktop_lens_save_geometry,
            desktop_lens_capture,
            desktop_lens_hide,
            screenshot_overlay_confirm,
            screenshot_overlay_cancel,
            quick_chat_close,
            open_external_url,
            open_vault_path,
            reveal_vault_path,
            open_system_trash,
            open_user_templates_dir,
            check_permission,
            request_permission,
            spike_show_popup,
            popup_show_input,
            voice_input_state,
            voice_input_set_mode,
            voice_input_subscribe_levels,
            voice_input_start,
            popup_voice_input_start,
            voice_input_stop,
            voice_input_cancel,
            voice_input_land,
            voice_input_show_hud,
            voice_input_close,
            voice_input_set_hud_pinned,
            recording_state,
            subscribe_recording_levels,
            recording_start,
            recording_stop,
            recording_pause,
            recording_resume,
            recording_bookmark,
            recording_bookmark_note,
            recording_hud_dismiss,
            recording_show_hud,
            panel_text_editing,
            text_quick_note_close,
            desktop_note_content,
            desktop_note_write_body,
            desktop_note_save_frame,
            desktop_note_set_color,
            desktop_note_pin,
            desktop_note_unpin,
            hud_set_tier,
            caption_start,
            caption_show_hud,
            caption_dismiss,
            caption_translation_show,
            caption_translation_dismiss,
            caption_save_note,
            load_sync_playback,
            playback_timecode_lines,
            playback_position_load,
            playback_position_save,
            update_playback_cue,
            update_playback_transcript,
            set_speaker_name,
            edit_playback_segment,
            enqueue_retranscribe,
            enqueue_translate,
            enqueue_diarize,
            export_transcript,
            preview_export_filename,
            export_transcripts,
            export_note_text,
            export_notes_text,
            export_note_html,
            export_note_bundle,
            open_note_print_window,
            print_current_window,
            recording_set_microphone_muted,
            recording_set_transcribe,
            recording_set_translate,
            recording_set_translate_target,
            recording_set_transcribe_lang,
            list_audio_input_devices,
            pick_media_files,
            import_media_paths,
            fetch_tool_status,
            external_tool_guides,
            fetch_install_plan,
            check_fetch_tool_update,
            install_fetch_tool,
            uninstall_fetch_tool,
            install_fetch_js_runtime,
            uninstall_fetch_js_runtime,
            local_engine_status,
            local_engine_restart,
            import_local_gguf,
            hf_repo_ggufs,
            import_hf_gguf,
            probe_media_url,
            enqueue_media_url,
            refetch_note_media,
            processing_queue_snapshot,
            processing_queue_reorder,
            processing_queue_remove,
            processing_queue_cancel,
            processing_queue_retry,
            processing_queue_clear_done,
            processing_queue_clear_unsuccessful,
            tts_available,
            tts_speak,
            tts_pause,
            tts_resume,
            tts_stop,
            tts_state,
            tts_set_rate,
            related_notes,
            link_mention,
            resolve_citation,
            open_citation,
            topic_directory,
            topic_evidence_detail,
            preview_sealed_ingest_plan,
            prepare_topic_ingest_plan,
            confirm_sealed_ingest_plan,
            open_sealed_plan_source,
            open_topic_link,
            topic_curation_status,
            set_transcript_sources,
            transcript_knowledge_notice,
            knowledge_review_inbox,
            topic_draft_ready,
            topic_preview,
            topic_network,
            confirm_topic_draft,
            discard_topic_draft,
            topic_reading_summary,
            topic_claims,
            open_topic_citation,
            knowledge_recent_activity,
            record_knowledge_visit,
            decide_sealed_ingest_plan,
            topic_upgrade_candidates,
            preview_topic_upgrade,
            apply_topic_upgrade,
            restore_topic_upgrade,
            ignored_ai_tags,
            set_ai_tag_ignored,
            preview_tag_rename,
            apply_tag_rename,
            undo_tag_rename,
            edit_note_tag,
            undo_topic_apply,
            tag_topic_actions,
            topic_state,
            topic_fork_note,
            topic_review_discard,
            knowledge_pulse,
            knowledge_next_action,
            create_action_states,
            record_workspace_metric,
            local_workspace_metrics_summary,
            export_local_workspace_metrics,
            clear_local_workspace_metrics,
            knowledge_map_create,
            knowledge_overview,
            knowledge_product_paths,
            knowledge_map_entry,
            knowledge_map_apply_review,
            knowledge_map_resolve_conflict,
            mention_candidates,
            embed_preflight,
            embed_rebuild_start,
            embed_rebuild_cancel,
            embed_rebuild_status,
            checkup_report,
            checkup_ignore,
            checkup_preflight,
            checkup_deep_start,
            checkup_deep_cancel,
            checkup_deep_status,
            prepare_topic_draft,
            ignore_topic_suggestion,
            chat_set_grounding,
            chat_set_retrieval_overrides,
            chat_save_message_to_note,
            open_workspace_route,
            take_pending_navigation,
            sync_overview,
            sync_remote_vaults,
            sync_first_login_offer,
            sync_first_login_decided,
            sync_merge_preview,
            sync_enable,
            sync_join,
            sync_restore_point,
            sync_restore_to_folder,
            sync_now,
            sync_materialize,
            sync_evict,
            sync_set_vault_paused,
            sync_pause_all,
            sync_resume_all,
            sync_confirm_bulk_delete,
            sync_disable,
            sync_cloud_vaults,
            sync_cloud_vault_download,
            sync_cloud_vault_delete,
            sync_cloud_vault_restore
        ])
        .events(collect_events![
            crate::sync_service::SyncStatusChanged,
            crate::sync_service::SyncDownloadProgress,
            sync_ipc::SyncRestoreProgress,
            EditorFlushRequested,
            EditorFlushCancelled,
            AppReady,
            recording::RecordingStateChanged,
            WorkspaceNavigate,
            ChatTurnEnded,
            ChatPrefillReady,
            VaultChanged,
            onboarding::SetupStatusChanged,
            local_engine::LocalEngineStatusChanged,
            models::CoremlPhase,
            NoteWritten,
            GraphRescanProgress,
            VaultSwitched,
            crate::vault_lists::VaultsChanged,
            crate::vault_lists::VaultSwitchedByApp,
            EmbedProgress,
            CheckupProgress,
            KnowledgeReceiptDto,
            NotificationEventDto,
            ScanProgress,
            ExternalRenamesChanged,
            ModelDownloadProgress,
            StorageRelocationProgress,
            VaultLinkReceived,
            AccountChanged,
            KnowledgeMetricsAvailabilityChanged,
            WorkspaceMetricsAvailabilityChanged,
            SettingsChanged,
            themes::ThemeLibraryChanged,
            template_library::TemplateLibraryChanged,
            caption::CaptionWindowHidden,
            DesktopLensChanged,
            CreateActionStatesChanged,
            VoiceInputHudOpened,
            crate::resource_monitor::ResourceUsage,
            crate::processing_queue::ProcessingQueueEvent,
            crate::tts::TtsStateChanged
        ])
}

/// 扫源码的守护测试**必须**先经过这里：把文件末尾的 `#[cfg(test)]` 整块切掉。
///
/// **不这么做，守护会去断言自己**。本域曾有三条 `include_str!("mod.rs").split("fn X")`
/// 的守护；纵切把 `X` 搬走之后，`split` 在同一个文件里仍能命中——**命中的是测试自己那行
/// 字面量**，`nth(1)` 于是返回测试代码本身。两条当场变红（还算走运），第三条
/// **一路全绿而实际上什么都没验**：它找的键 `privacy.strict_local` 就写在它自己的数组里。
///
/// 这不是「搬代码时忘了改路径」，而是判据从一开始就把测试自身算进了扫描面。把它做成
/// 结构保证而不是纪律：扫描面里根本没有测试，`split` 找不到实现就只能 `expect` 失败。
///
/// **切掉每一段测试，不是「第一个 `#[cfg(test)]` 之后全丢」**。原实现取 `split(..).next()`，
/// 于是文件里**第一处**测试标记之后的东西一律不进扫描面——包括后面还有的生产代码。
/// 代价是实测过的：`transcribe.rs` 的首个测试块在 843 行，其后还有 **7 个
/// `#[tauri::command]`**，所有靠本函数扫它的守护对那 7 条命令一直是盲的；而在 `lib.rs`
/// 顶部加一条 `#[cfg(test)] mod` 会把 1300 行后的定义整段抹掉，`voice.rs` 那条守护当场变红。
///
/// 现在按**块**剔除：从每个 `#[cfg(test)]` 起，按花括号配平吃掉整个 `mod`，其余原样保留。
#[cfg(test)]
pub(crate) fn source_without_tests(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    while let Some(at) = rest.find("\n#[cfg(test)]") {
        out.push_str(&rest[..at]);
        // 从标记处找到该块的第一个 `{`，再按配平吃到它的 `}`。找不到花括号
        // （例如 `#[cfg(test)] use ...;`）就只跳过这一行，不吞掉后面的代码。
        let tail = &rest[at + 1..];
        let Some(open) = tail.find('{') else {
            let line_end = tail.find('\n').map_or(tail.len(), |n| n + 1);
            rest = &tail[line_end..];
            continue;
        };
        let mut depth = 0usize;
        let mut end = None;
        for (idx, ch) in tail[open..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(open + idx + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        match end {
            Some(e) => rest = &tail[e..],
            // 花括号不配平（文件被截断）：剩下的整段都不进扫描面，同旧行为的保守侧。
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod structure_tests {
    /// 一个 IPC 文件能承载多少条命令的上限。**它是代理，不是判据本身**。
    ///
    /// TDD-15 §2.4 的真判据是「多域混居」，那件事机器判不了。命令数是能拿到的最好代理：
    /// 一个文件攒到二十几条命令，几乎必然已经横跨几个域。
    ///
    /// 上限取 24。**选它的时候**（纵切收口那一批）最大的单域文件是 `transcribe.rs` 的
    /// 20 条，24 是给真实单域留的一档生长余地。如今 `transcribe.rs` 正好用满 24、
    /// `notes.rs` 23——余地基本用完了，下一次这两个域再长，该做的是**按域再纵切一次**
    /// （TDD-15 §2.4 的手法），而不是把这个数字往上抬。
    ///
    /// 抬这个数是最省事也最没用的一步：它把"该切了"这条信号本身关掉了。
    const MAX_COMMANDS_PER_FILE: usize = 24;

    /// **IPC 层不得再长出上帝文件**（TDD-15 §2.4）。
    ///
    /// `ipc/mod.rs` 曾以 10,370 行承载 **129 条命令、约 15 个域**——本地知识度量、工作区
    /// 窗口、笔记读写、搜索、用量、通知、快照回收站、设置、模型、引导、库管理、弹窗翻译、
    /// 桌面取词、语音输入……而 §2.4 里就记着 2026-07-25 把 2,335 行五个域的
    /// `ipc/recording.rs` 纵切成五个文件的先例。**先例挡不住复发，因为没有任何东西会因此
    /// 变红**：命令是一条一条加进去的，单条看都无可指摘。
    ///
    /// 本测就是那个会变红的东西。它挡不住「一个域自己写胖」——那是人来判断的事，
    /// 也不该由行数裁决（§2.4 明说判据不是行数）；它挡住的是「又把好几个域堆进同一个文件」。
    #[test]
    fn no_ipc_file_becomes_a_god_file_again() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ipc");
        let mut offenders = Vec::new();
        let mut seen = 0;
        for entry in std::fs::read_dir(&dir).expect("读得到 ipc 目录") {
            let path = entry.expect("目录项可读").path();
            if path.extension().is_none_or(|ext| ext != "rs") {
                continue;
            }
            seen += 1;
            let commands = std::fs::read_to_string(&path)
                .expect("源码可读")
                .matches("#[tauri::command]")
                .count();
            if commands > MAX_COMMANDS_PER_FILE {
                offenders.push(format!(
                    "{} 有 {commands} 条命令",
                    path.file_name().expect("有文件名").to_string_lossy()
                ));
            }
        }
        assert!(seen > 1, "只扫到 {seen} 个文件——这条断言在空跑");
        assert!(
            offenders.is_empty(),
            "{} —— 上限 {MAX_COMMANDS_PER_FILE}。按 TDD-15 §2.4 的手法纵切：\
             子模块 `use super::*` 取共享项、本域命令与 DTO 标 `pub(crate)`、\
             `mod.rs` `use <域>::*` 让命令回到 `collect_commands!` 作用域。\
             验收口径是 `bindings.ts` 零 diff——纯内部重排不得改变 IPC 契约。",
            offenders.join("；")
        );
    }

    /// 整个文件只服务主库的域：副库窗口不提供这些入口（PRD-06-050 能力边界）。
    const MAIN_VAULT_ONLY_FILES: &[(&str, &str)] = &[
        ("distill.rs", "蒸馏与速记转正是 AI 写回，副库窗口不承载 AI"),
        ("ingest.rs", "主题归入是知识库动作，副库窗口不承载 AI"),
        ("knowledge.rs", "知识库与库内问答只服务主库"),
        ("knowledge_curation.rs", "持续整理只服务主库"),
        ("note_export.rs", "导出入口只在主窗的对象菜单上"),
        (
            "playback.rs",
            "媒体服务绑主库根（TDD-12 §4），副库窗口不回放",
        ),
        ("quick_note.rs", "速记是主库的查询投影"),
        ("transcribe.rs", "转写入队落主库"),
        ("translate.rs", "浮动菜单是 AI 入口，副库窗口不承载 AI"),
    ];

    /// **接受笔记路径的命令必须按窗口身份解析落点库**（PRD-06-050、TDD-07 §2.4）。
    ///
    /// 2026-10-04 查出四条附件命令与图片 / md 链接解析直接取 `state.vault`（主库）：副库
    /// 窗口里粘贴的图片落进主库、副库笔记里的图片与链接解析到主库。单条看都无可指摘，
    /// 没有任何东西会因此变红——本测就是那个东西。命令签名带 `note_path` / `note_rel`、
    /// 体内直接锁 `state.vault` 而不经 `with_vault` 的，要么改走窗口路由，要么所在文件
    /// 整个声明为主库专属并写明理由。
    #[test]
    fn commands_taking_a_note_path_route_by_window() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ipc");
        let mut offenders = Vec::new();
        let mut checked = 0;
        let mut main_only_hits = std::collections::BTreeSet::new();
        for entry in std::fs::read_dir(&dir).expect("读得到 ipc 目录") {
            let path = entry.expect("目录项可读").path();
            if path.extension().is_none_or(|ext| ext != "rs") {
                continue;
            }
            let file = path
                .file_name()
                .expect("有文件名")
                .to_string_lossy()
                .into_owned();
            let src =
                super::source_without_tests(&std::fs::read_to_string(&path).expect("源码可读"));
            for command in src.split("#[tauri::command]").skip(1) {
                let signature = &command[..command.find('{').unwrap_or(command.len())];
                if !(signature.contains("note_path") || signature.contains("note_rel")) {
                    continue;
                }
                checked += 1;
                let direct = command.contains("state.vault.lock()");
                let routed =
                    command.contains("with_vault") || command.contains("asset_destination(");
                if !direct || routed {
                    continue;
                }
                if MAIN_VAULT_ONLY_FILES.iter().any(|(name, _)| *name == file) {
                    main_only_hits.insert(file.clone());
                    continue;
                }
                let name = signature
                    .split("fn ")
                    .nth(1)
                    .and_then(|rest| rest.split('(').next())
                    .unwrap_or("?")
                    .trim()
                    .to_owned();
                offenders.push(format!("{file}::{name}"));
            }
        }
        assert!(checked > 10, "只检查到 {checked} 条命令——这条断言在空跑");
        assert!(
            offenders.is_empty(),
            "这些命令接受笔记路径却直接取主库：{}。改为 `state.with_vault(window.label(), …)` \
             （窗口句柄由 Tauri 注入，绑定不变），或把所在文件声明进 MAIN_VAULT_ONLY_FILES 并写明理由。",
            offenders.join("、")
        );
        for (name, _) in MAIN_VAULT_ONLY_FILES {
            assert!(
                main_only_hits.contains(*name),
                "MAIN_VAULT_ONLY_FILES 里的 {name} 已没有直接取主库的笔记路径命令——陈旧条目会替同名新文件放行，删掉它"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luna_config::SettingValue;
    use std::path::PathBuf;
    use tempfile::tempdir;

    /// **缺陷 D 会红守护**：「本地节省」估算的公式钉在 Rust（前端零业务，ADR-003）。
    ///
    /// 反向验证：把 `estimate_local_savings_usd` 的公式改一处（如把 `/ 60.0` 去掉），这条当场红。
    #[test]
    fn local_savings_estimate_follows_the_reference_prices() {
        let summary = UsageSummary {
            records: 3,
            tokens_in: 1_000_000,
            tokens_out: 1_000_000,
            audio_seconds: 120.0, // 2 分钟
            est_cost_minor: 0,
            // 本机跑掉的那一半才进"省下"（ADR-126 第 6 条）；服务端那一半在下一条用例里。
            local_audio_seconds: 120.0,
            local_tokens: 2_000_000,
            server_audio_seconds: 0.0,
            server_tokens: 0,
            truncated: 0,
        };
        // 120 秒 = 2 分钟 × 0.006 = 0.012；2,000,000 token = 2 × 1.0 = 2.0 → 合计 2.012。
        let savings = estimate_local_savings_usd(&summary, Some(0.006), Some(1.0)).unwrap();
        assert!(
            (savings - 2.012).abs() < 1e-9,
            "本地节省估算公式变了：{savings}"
        );
        // **空用量 = 估不出来，而不是零**（ADR-126 第 6 条真机补充）：`US$0.00` 会被读成
        // 「跑了，但便宜到看不见」，而这里的事实是「这一侧一个任务都没有」。
        assert_eq!(
            estimate_local_savings_usd(&UsageSummary::default(), Some(0.006), Some(1.0)),
            None
        );
        // 反过来，有量就必须给数——哪怕小到四舍五入之后是 0.00。
        let tiny = UsageSummary {
            local_tokens: 1,
            ..UsageSummary::default()
        };
        assert_eq!(
            estimate_local_savings_usd(&tiny, None, Some(1.0)),
            Some(1.0 / 1_000_000.0)
        );
        // 只配了一个单价 → 只算那一半（少算是诚实的，补一个编出来的价格不是）。
        let asr_only = estimate_local_savings_usd(&summary, Some(0.006), None).unwrap();
        assert!(
            (asr_only - 0.012).abs() < 1e-9,
            "只配 ASR 时不该算上 LLM 那半"
        );
        // 一个都没配 → 不给数（PRD-07-051：未配置参考单价时不显示金额）。
        assert_eq!(estimate_local_savings_usd(&summary, None, None), None);
    }

    /// **参考单价没配就是没配**（PRD-07-051）：不回落任何厂商报价。
    ///
    /// 反向验证：把注册表默认改回 `"0.006"`，这条当场红。
    #[test]
    fn setting_price_is_none_until_the_user_configures_it() {
        let settings = Settings::with_defaults();
        assert_eq!(
            setting_price_usd(&settings, "provider.asr_cost_per_min"),
            None
        );
        assert_eq!(
            setting_price_usd(&settings, "provider.llm_cost_per_million_tokens"),
            None
        );
        // 配了就用配的那个。
        let mut configured = Settings::with_defaults();
        configured.set(
            "provider.asr_cost_per_min",
            SettingValue::Str("0.01".into()),
        );
        assert_eq!(
            setting_price_usd(&configured, "provider.asr_cost_per_min"),
            Some(0.01)
        );
        // 存了非数值（用户手改坏）→ 按未配置处理，不拿坏值算出一个数。
        let mut garbage = Settings::with_defaults();
        garbage.set(
            "provider.asr_cost_per_min",
            SettingValue::Str("not-a-number".into()),
        );
        assert_eq!(
            setting_price_usd(&garbage, "provider.asr_cost_per_min"),
            None
        );
        // 负价同样不算数，不产生负的「节省」。
        let mut negative = Settings::with_defaults();
        negative.set("provider.asr_cost_per_min", SettingValue::Str("-1".into()));
        assert_eq!(
            setting_price_usd(&negative, "provider.asr_cost_per_min"),
            None
        );
    }

    /// 保留目标 id 不得与任何模型 id 撞车。
    ///
    /// 撞车的后果是：用户装了一个恰好叫这个名字的模型，选它却被路由到服务端——
    /// 音频出了设备而界面显示的是本地模型名。前缀 `service:` 与既有命名空间
    /// （目录条目 id / `whisper-*`）不重叠正是为了这个。
    #[test]
    fn remote_asr_target_uses_a_reserved_namespace() {
        assert_eq!(REMOTE_ASR_TARGET, "service:asr");
        assert!(is_remote_asr_target(REMOTE_ASR_TARGET));
        for model_like in [
            "qwen3.5-2b-q8_0-gguf",
            "whisper-large-v3",
            "whisper-tiny-bundled",
            "bge-m3-q8_0-gguf",
            "asr",
            "service",
        ] {
            assert!(
                !is_remote_asr_target(model_like),
                "`{model_like}` 被误判成服务端目标"
            );
        }
    }

    /// 开关关掉后，仍指向服务端的槽位必须报 `missing`，**不得悄悄回落本地**。
    ///
    /// 悄悄回落的后果是用户以为音频没出设备（或出了），取决于我们悄悄选了谁——
    /// 而界面上毫无差别。如实说「你选了服务端但它没开」才是可执行的。
    #[test]
    fn remote_target_reports_missing_when_switch_is_off() {
        let temp = tempdir().unwrap();
        let store = ModelStore::for_test_root(temp.path());
        let mut on = Settings::with_defaults();
        on.set("provider.remote_asr_enabled", SettingValue::Bool(true));
        assert_eq!(
            assignment_readiness(
                &store,
                "batch_asr",
                Some(REMOTE_ASR_TARGET),
                "transcribe",
                &on
            ),
            "ready"
        );

        let off = Settings::with_defaults();
        assert_eq!(
            assignment_readiness(
                &store,
                "batch_asr",
                Some(REMOTE_ASR_TARGET),
                "transcribe",
                &off
            ),
            "missing",
            "开关关着却报 ready，用户会以为服务端在跑，而实际什么都没有"
        );
    }

    /// **只有两个转写槽位能用服务端目标**——LLM/嵌入槽位指向它必须报 `incompatible`。
    ///
    /// 不限槽位时它们会报 `ready`：界面说"就绪"、一用就失败，用户以为配好了，
    /// 直到真正要用的那一刻才发现没有。候选表不给这个选项是第一道；这条拦的是
    /// 手工改配置绕过第一道的情形。
    #[test]
    fn only_transcribe_slots_accept_the_service_target() {
        let temp = tempdir().unwrap();
        let store = ModelStore::for_test_root(temp.path());
        let mut on = Settings::with_defaults();
        on.set("provider.remote_asr_enabled", SettingValue::Bool(true));

        // 实时（ADR-038 双向流式）与离线（ADR-036 上传+任务）是两条不同的服务端通道，
        // 但对用户是同一个选择——"这件事交给服务端做"。
        for slot in ["realtime_asr", "batch_asr"] {
            assert_eq!(
                assignment_readiness(&store, slot, Some(REMOTE_ASR_TARGET), "transcribe", &on),
                "ready",
                "转写槽位 {slot} 拒绝了服务端目标"
            );
        }
        for slot in ["default_llm", "realtime_llm", "default_embedding"] {
            assert_eq!(
                assignment_readiness(&store, slot, Some(REMOTE_ASR_TARGET), "transcribe", &on),
                "incompatible",
                "槽位 {slot} 接受了服务端转写目标——那条路不存在"
            );
        }
    }

    /// 外发闸必须**当场**跟着隐私档翻转。
    ///
    /// 装配时不构造云腿只管**新**会话；一场一小时的会议中途按下「仅本地」时，正在跑的
    /// 那条腿仍持有自己的 `Arc`，只有这把闸能掐断它。闸没跟上 = 隐私档显示「仅本地」
    /// 而字节还在往外走——不报错、不留痕，只有抓包才看得见。
    #[test]
    fn the_outbound_gate_follows_the_privacy_mode() {
        use luna_core::OutboundGate;

        let gate = PrivacyOutboundGate::default();
        assert!(
            !gate.allows_outbound(),
            "闸的初值必须是**关**：装配还没读到设置的那一小段时间里，\
             默认放行等于对严格本地的用户开了一扇窗"
        );
        gate.sync(false);
        assert!(gate.allows_outbound(), "非严格本地下闸应放行");
        gate.sync(true);
        assert!(
            !gate.allows_outbound(),
            "切到严格本地后闸没关——正在跑的会话会继续外发"
        );
        gate.sync(false);
        assert!(gate.allows_outbound(), "关掉严格本地后闸没重新打开");
    }

    /// 登录了哪台服务器，对话就打哪一台；未登录或严格本地全在本机（ADR-155）。
    ///
    /// 「填了地址但没登录」不再是一个状态：未登录时无论记住的是哪台服务器，都不得派生出
    /// 远端端点——ADR-037 的「未登录时不自发云网络行为」由此成立。
    #[test]
    fn the_chat_endpoint_follows_the_signed_in_server() {
        use crate::cloud::CloudAvailability::{Allowed, BlockedByStrictLocal, SignedOut};
        use crate::cloud::{ComputeEndpoint, ServerRef};

        let org = ServerRef::of("http://10.0.0.62:8090");
        let official = ServerRef::of(luna_cloud::config::DEFAULT_API_BASE);
        assert_eq!(
            compute_endpoint_source(Allowed, &org),
            ComputeEndpoint::LunarcaServer("http://10.0.0.62:8090".into())
        );
        assert_eq!(
            compute_endpoint_source(Allowed, &official),
            ComputeEndpoint::OfficialCloud(luna_cloud::config::DEFAULT_API_BASE.to_owned())
        );
        for availability in [SignedOut, BlockedByStrictLocal] {
            assert_eq!(
                compute_endpoint_source(availability, &org),
                ComputeEndpoint::OnDevice,
                "{availability:?} 下派生出了远端端点——那等于默认联网，或严格本地被绕过"
            );
        }
    }

    /// 转写的默认落点跟着算力位置走，**但仍卡在一次同意后面**。
    ///
    /// 用户连上服务端后期望「AI 就在服务端跑」，不该再去五个槽位逐个挑——这是这次
    /// 重设计的核心。但转写是例外：连服务端可能只为了对话或同步，而麦克风与会议录音
    /// 是另一个量级的东西（ADR-038 ④「默认档不得是云」）。故默认值只在**开了那次同意
    /// 之后**才变，且严格本地下永不变。
    ///
    /// 对话与嵌入不在这里表达——它们的落点由端点解析决定（`compute_endpoint_source`
    /// 非空即走服务端），槽位存的是发给端点的 model 串，值不必变。
    #[test]
    fn transcription_follows_the_server_only_after_its_own_consent() {
        let bare = Settings::with_defaults();
        assert_eq!(
            effective_assignment_target(&bare, "realtime_asr").as_deref(),
            Some(DEVICE_POLICY_ASR),
            "什么都没配时必须是本机兜底——ADR-037 的默认态"
        );

        let mut consented = Settings::with_defaults();
        consented.set("provider.remote_asr_enabled", SettingValue::Bool(true));
        assert_eq!(
            effective_assignment_target(&consented, "realtime_asr").as_deref(),
            Some(REMOTE_ASR_TARGET),
            "开了同意之后就该自动跟随服务端，不必再去槽位里手选"
        );
        // 离线槽位空值时继承实时档 —— 一次同意覆盖两条通道。
        assert_eq!(
            effective_assignment_target(&consented, "batch_asr").as_deref(),
            Some(REMOTE_ASR_TARGET)
        );

        // 严格本地压倒同意：开关值还留着，但它此刻不生效。
        let mut strict = consented.clone();
        strict.set("privacy.strict_local", SettingValue::Bool(true));
        assert_eq!(
            effective_assignment_target(&strict, "realtime_asr").as_deref(),
            Some(DEVICE_POLICY_ASR),
            "严格本地下默认值仍指向服务端 = 隐私档形同虚设"
        );
    }

    /// 中层 crate **不得依赖 `luna-cloud`**（TDD-00 §3 依赖方向）。
    ///
    /// 这条撑着的是 ADR-037 的替代护栏——「未登录时，客户端不自发产生任何
    /// 云网络行为」。它今天之所以能被**单点证明**（`src-tauri` 的命令面钉死测试 + 下载门控），
    /// 前提是云客户端只有装配层这一个入口；某个中层 crate 一旦直接依赖它，云调用就可能从
    /// 转写、导出、索引任何一条路径上长出来，而那两处断言照样全绿。
    #[test]
    fn the_cloud_client_is_reachable_only_from_the_assembly_layer() {
        let middle_layers = [
            (
                "luna-core",
                include_str!("../../../crates/luna-core/Cargo.toml"),
            ),
            (
                "luna-provider",
                include_str!("../../../crates/luna-provider/Cargo.toml"),
            ),
            (
                "luna-jobqueue",
                include_str!("../../../crates/luna-jobqueue/Cargo.toml"),
            ),
            (
                "luna-vault",
                include_str!("../../../crates/luna-vault/Cargo.toml"),
            ),
            (
                "luna-export",
                include_str!("../../../crates/luna-export/Cargo.toml"),
            ),
            (
                "luna-engine",
                include_str!("../../../crates/luna-engine/Cargo.toml"),
            ),
            (
                "luna-audio",
                include_str!("../../../crates/luna-audio/Cargo.toml"),
            ),
            (
                "luna-fetch",
                include_str!("../../../crates/luna-fetch/Cargo.toml"),
            ),
            (
                "luna-config",
                include_str!("../../../crates/luna-config/Cargo.toml"),
            ),
            (
                "luna-metrics",
                include_str!("../../../crates/luna-metrics/Cargo.toml"),
            ),
        ];
        for (name, manifest) in middle_layers {
            assert!(
                !manifest.contains("luna-cloud"),
                "{name} 依赖了 luna-cloud——云调用可以从这条路径上长出来，\
                 而「未登录时零云网络行为」的两处断言不会因此变红"
            );
        }
    }

    /// 面板承载的键必须带着**指向自己那一组、自己那一个 key** 的深链。
    ///
    /// 拼错的表现是"搜到了、点了、跳到别的组"或"跳过去停在页面顶端"——两者都不会让任何
    /// 测试变红，而它正是这条机制存在的全部意义（搜得到 ≠ 去得了）。
    #[test]
    fn panel_hits_carry_a_deep_link_to_their_own_group() {
        let mut seen = 0;
        for spec in luna_config::SETTINGS {
            let SettingSurfaceDto::Panel { route } = SettingSurfaceDto::of(spec) else {
                continue;
            };
            seen += 1;
            assert_eq!(
                route,
                format!(
                    "settings://{}/{}",
                    luna_config::setting_page(spec.key).as_str(),
                    spec.key
                ),
                "{} 的深链指错了地方",
                spec.key
            );
        }
        assert!(seen > 0, "一个面板承载的键都没有——这条断言等于没跑");
    }

    /// 「算力在哪」必须在**普通模式**下可达。
    ///
    /// 此前这一段整个是 `advanced`，于是关掉「显示完整设置」的用户撞上死锁：云相关项
    /// 全部消失 → 服务端转写开关看不见 → 模型分配下拉里的「服务端转写」候选压根不出现
    /// → 他**无法开启**这条能力，甚至不知道它存在。
    ///
    /// 「可达」不等于「摆一行设置出来」：服务器地址是私有化部署方给的实现细节，同类产品
    /// 里根本没有这个概念（用户看到的是「登录」）。故承载点是**算力状态卡片**——转写同意
    /// 归它（`is_panel_owned`），地址留在高级设置供部署方与排障直接改值。
    /// 这条用例守的是这个分工，不是某一项的具体可见性。
    #[test]
    fn choosing_where_compute_runs_is_not_buried_in_advanced() {
        let specs = luna_config::SETTINGS;
        let spec = |key: &str| {
            specs
                .iter()
                .find(|s| s.key == key)
                .unwrap_or_else(|| panic!("设置项 {key} 不见了"))
        };

        // 转写同意（ADR-038 ④「显式、可见、可关」）由卡片承载：它既不渲染成设置行，
        // 也**不得**因此变成只有高级模式才够得着的东西。
        let consent = spec("provider.remote_asr_enabled");
        assert_eq!(
            luna_config::setting_surface(consent.key),
            luna_config::SettingSurface::Panel,
            "转写同意不再由卡片承载 = ADR-038 ④ 的同意点回到了设置第八项"
        );
        assert_eq!(
            consent.visibility(),
            luna_config::SettingVisibility::Standard,
            "转写同意被标成 advanced——卡片在普通模式下也要能开关它"
        );

        // 反过来：成本估算**不该**占普通用户的首屏。
        assert_eq!(
            spec("provider.asr_cost_per_min").visibility(),
            luna_config::SettingVisibility::Advanced,
            "成本估算是实现细节，不该占普通用户的首屏"
        );
    }

    /// ADR-038 ④：严格本地与「交给服务端做」直接矛盾，必须报 `incompatible`，
    /// 且候选表里连这个选项都不给。
    ///
    /// 悄悄按本地跑是更坏的选项：设置页会一直显示「服务端转写」，而设备上从来没走过
    /// 那条路——用户以为自己在用服务端算力，实际在用本机 CPU 慢慢磨。
    #[test]
    fn strict_local_blocks_the_service_target_in_both_transcribe_slots() {
        let temp = tempdir().unwrap();
        let store = ModelStore::for_test_root(temp.path());
        let mut strict = Settings::with_defaults();
        strict.set("provider.remote_asr_enabled", SettingValue::Bool(true));
        strict.set("privacy.strict_local", SettingValue::Bool(true));
        assert!(strict.strict_local(), "测试前提：这份设置是严格本地");

        for slot in ["realtime_asr", "batch_asr"] {
            assert_eq!(
                assignment_readiness(&store, slot, Some(REMOTE_ASR_TARGET), "transcribe", &strict),
                "incompatible",
                "严格本地下槽位 {slot} 仍接受服务端目标"
            );
        }
    }

    /// ADR-038 ④：**默认档不得是云**。
    ///
    /// 麦克风内容是常开的那条路径，默认值一旦是云，绝大多数用户根本不会察觉自己每天
    /// 说的每句话都在离开设备。两道保险各自独立：开关默认关、槽位默认指向本机权重。
    #[test]
    fn out_of_the_box_transcription_never_leaves_the_device() {
        let fresh = Settings::with_defaults();
        assert!(
            !remote_asr_available(&fresh),
            "「服务端转写」开关的默认值是开——默认档成了云"
        );
        for slot in ["realtime_asr", "batch_asr"] {
            assert_ne!(
                effective_assignment_target(&fresh, slot).as_deref(),
                Some(REMOTE_ASR_TARGET),
                "槽位 {slot} 的默认分配指向了服务端"
            );
        }
        assert_eq!(
            effective_assignment_target(&fresh, "realtime_asr").as_deref(),
            Some(DEVICE_POLICY_ASR),
            "实时转写的默认档不是随包本机模型"
        );
    }

    /// **一个端点声明「接管了什么」，必须与它真的接管了什么一致**——两个方向都错过。
    ///
    /// Lunarca 服务端的嵌入确实由 `CloudEmbedProvider` 接管，不算进去的话模型页写着
    /// 「只接管四项」，而 `bge-m3` 其实早已不在执行路径上——「屏幕说的 ≠ 实际」。
    ///
    /// **`transcribe` 任何一档都不在其列**：它由槽位接管，且本机 whisper 恒为断线
    /// 兜底（ADR-038 ⑤）。
    ///
    /// > 历史：这里曾另有一条**逐模型**的隐藏规则（`only_serves`：只服务被接管能力的
    /// > 模型不再列入本地目录）。ADR-048 之后标准投影的粒度变粗了——连上服务端即整块
    /// > 不列本机目录（前端 `viewsFor` 落地），逐模型那条因此再无生产调用方，只剩用例
    /// > 在用。已随本次清理删除；它守的事实（服务端接管嵌入）由下面对 `taken_over_capabilities` 的断言直接守住，而那才是真正驱动界面的东西。
    #[test]
    fn what_an_endpoint_claims_to_take_over_matches_what_it_actually_does() {
        use crate::cloud::ComputeEndpoint;
        let lunarca = ComputeEndpoint::LunarcaServer("http://org:8090".into());
        let settings = Settings::with_defaults();

        let taken = taken_over_capabilities(&settings, &lunarca);
        assert!(
            taken.contains(&"chat"),
            "服务端接管对话，却没这么说：{taken:?}"
        );
        assert!(
            !taken.contains(&"transcribe"),
            "宣称接管转写等于把断线兜底的本机 whisper 说成不在了（ADR-038 ⑤）：{taken:?}"
        );

        assert!(
            taken_over_capabilities(&settings, &lunarca).contains(&"embed"),
            "嵌入已由服务端接管，模型页却写着「只接管四项」"
        );

        assert!(
            taken_over_capabilities(&settings, &ComputeEndpoint::OnDevice).is_empty(),
            "全在本机时什么都没被接管——`remote_compute` 整块不该出现"
        );

        let mut explicit_local = Settings::with_defaults();
        explicit_local.set("ai.assignment.default_llm", "qwen3.5-2b-q8_0-gguf");
        explicit_local.set("ai.assignment.default_embedding", "bge-m3-q8_0-gguf");
        assert!(
            taken_over_capabilities(&explicit_local, &lunarca).is_empty(),
            "用户显式选择的本机 LLM/嵌入被服务端连接覆盖，模型页与真实字节去向都会说谎"
        );
    }

    /// 连接状态不是能力落点。用户显式选择本机 LLM 后，模型推荐、上下文窗和启动提醒
    /// 都必须继续按本机事实工作，不能因为控制面在线而整片切成服务端语义。
    ///
    /// **`service:<model>` 是旧目标，不再被解释成「跑在组织服务端」**（ADR-048：
    /// 原生服务端不再接受客户端逐模型路由）。它在就绪判定里被标为 `incompatible`
    /// （`models.rs` 的 `is_remote_asr_target` 分支：只有两个转写槽位收服务端目标），
    /// 用户清掉之后才回到服务端托管策略。
    ///
    /// > 本条此前断言 `service:chat-default` → `Organization`，即改版**之前**的语义；
    /// > 改版落地后它一直是红的（本次清理时坐实：在 pin 上就红）。留着一条与实现相反的
    /// > 断言比没有断言更坏——它让整条 `cargo test` 变红，于是真正的新失败没人分得出来。
    #[test]
    fn explicit_local_chat_assignment_keeps_chat_compute_location_on_device() {
        let org = crate::cloud::ServerRef::of("http://org:8090");
        let mut settings = Settings::with_defaults();
        settings.set("ai.assignment.default_llm", "qwen3.5-2b-q8_0-gguf");
        assert_eq!(
            chat_compute_location(&settings, crate::cloud::CloudAvailability::Allowed, &org),
            crate::cloud::ComputeLocation::OnDevice
        );

        settings.set("ai.assignment.default_llm", "service:chat-default");
        assert_eq!(
            chat_compute_location(&settings, crate::cloud::CloudAvailability::Allowed, &org),
            crate::cloud::ComputeLocation::OnDevice,
            "旧的 service: 目标不再解释成组织服务端——把它读成 Organization 会让\
             首屏写着「在服务端跑」，而那个分配其实是不兼容的、一用就失败"
        );

        // 清掉显式分配 → 回到 ADR-037 的默认「连上服务端即用服务端算力」。
        settings.set("ai.assignment.default_llm", "");
        assert_eq!(
            chat_compute_location(&settings, crate::cloud::CloudAvailability::Allowed, &org),
            crate::cloud::ComputeLocation::Organization,
            "没有显式分配时必须回到服务端托管，否则登录了也永远显示本机"
        );
    }

    #[test]
    fn job_error_code_dto_serializes_as_stable_screaming_snake_case() {
        assert_eq!(
            serde_json::to_string(&JobErrorCodeDto::ModelMissing).unwrap(),
            "\"MODEL_MISSING\""
        );
        assert_eq!(
            serde_json::from_str::<JobErrorCodeDto>("\"COMMAND_FAILED\"").unwrap(),
            JobErrorCodeDto::CommandFailed
        );
    }

    /// 会话被拒的投影与 `JobError::code()` 逐字一致（TDD-01 §2.4）：前端按 DTO 选修复深链，
    /// 用量账本按 `code()` 记 `error_code`——两处各说各的，账本里就会出现一个界面上从未
    /// 显示过的码。
    #[test]
    fn session_rejection_projects_to_the_same_code_as_the_ledger() {
        let error = luna_core::JobError::Unauthorized {
            provider: luna_core::ProviderId::from("lunarca-cloud"),
        };
        let dto = JobErrorCodeDto::from(&error);
        assert_eq!(dto, JobErrorCodeDto::SignInRequired);
        assert_eq!(
            serde_json::to_string(&dto).unwrap(),
            format!("\"{}\"", error.code())
        );
    }

    /// 跨窗口导航白名单必须覆盖主窗口 `luna:navigate` 处理器支持的全部 scheme
    /// （WorkspaceShell 的 onNavigate 分支）。漏收的后果是静默的：独立窗口里的
    /// 入口（如笔记窗口「在图谱中查看」）点击后被 ROUTE_REJECTED 挡下而界面
    /// 毫无反馈——`graph://`/`notifications://` 曾因此缺席。
    #[test]
    fn workspace_route_whitelist_covers_navigate_handler_schemes() {
        for scheme in [
            "settings://ai_services",
            "queue://current",
            "workspace://current",
            "notifications://",
            "recording://current",
            "transcribe://",
            "chats://Chats/对话.md",
            "notes://Notes/示例.md",
            "creator://",
            "teleprompter://",
            "topics://",
            "tags://%E9%A1%B9%E7%9B%AE",
            "graph://Notes/%E7%A4%BA%E4%BE%8B.md",
        ] {
            assert!(
                workspace_route_allowed(scheme),
                "白名单缺少 scheme：{scheme}"
            );
        }
        // 任意字符串注入仍必须被挡下（白名单存在的理由）。
        assert!(!workspace_route_allowed("https://example.com"));
        assert!(!workspace_route_allowed("file:///etc/passwd"));
        assert!(!workspace_route_allowed(""));
    }

    /// 切库过渡态标记（PRD-06-049 边界⑤）：任何退出路径（`?` / 回滚 / panic 展开）都必须
    /// 复位——漏在真值上，此后每个「未打开 Vault」错误都在说"稍候再试"却永远好不了。
    #[test]
    fn switching_guard_resets_flag_on_every_exit_path() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let flag = AtomicBool::new(false);
        {
            let _guard = VaultSwitchingGuard::arm(&flag);
            assert!(flag.load(Ordering::Acquire), "臂上即置位");
            // 过渡期措辞：说「正在切换」，不说误导性的「尚未选择」。
            // 文案来源缺席时**只发码**（`BackendCopy` 一直以来的回落），故这里断言
            // 的仍然是码本身——本用例守的是「过渡期与未选择是两个码」，不是措辞。
            assert!(vault_absent_message(None, true).starts_with("VAULT_SWITCHING"));
        }
        assert!(!flag.load(Ordering::Acquire), "析构必须复位");
        assert!(vault_absent_message(None, false).starts_with("VAULT_NOT_OPEN"));
    }

    /// 切库换源：新库**没写**的 Vault 级键必须回到默认，绝不残留上一个库的取值。
    ///
    /// 这条泄漏是静默的——切库后文件树仍按旧库的归档目录分区、蒸馏三档还是旧库的设置，
    /// 界面上只是"配置怎么不对"，没有任何报错（ADR-025 / TDD-06 §12）。
    #[test]
    fn switching_vaults_resets_unset_vault_keys_instead_of_inheriting() {
        let old_vault = tempdir().unwrap();
        let new_vault = tempdir().unwrap();

        // 旧库把转写归档目录改到了别处，并落进旧库自己的 `.luna/settings.toml`。
        let mut settings = Settings::with_defaults();
        settings.set("vault.transcripts_dir", "旧库转写");
        let default_dir = Settings::with_defaults()
            .get("vault.transcripts_dir")
            .unwrap();
        assert_ne!(settings.get("vault.transcripts_dir").unwrap(), default_dir);

        // 新库连 `.luna/settings.toml` 都没有（刚建的库）——最容易整片残留的一种。
        let switched = settings_with_vault_scope_from_root(settings.clone(), new_vault.path());
        assert_eq!(
            switched.get("vault.transcripts_dir").unwrap(),
            default_dir,
            "新库未设的键残留了旧库取值"
        );

        // 新库写了自己的取值时以新库为准。
        let path = vault_settings_path(new_vault.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut theirs = Settings::with_defaults();
        theirs.set("vault.transcripts_dir", "新库转写");
        std::fs::write(
            &path,
            theirs
                .to_toml_for_scope(SettingScope::Vault)
                .expect("序列化 Vault 级配置"),
        )
        .unwrap();
        let switched = settings_with_vault_scope_from_root(settings, new_vault.path());
        assert_eq!(
            switched.get("vault.transcripts_dir").unwrap(),
            SettingValue::Str("新库转写".to_owned())
        );
        drop(old_vault);
    }

    fn layout(panes: Vec<WorkspacePaneDto>, active_pane: u32) -> WorkspaceLayoutDto {
        WorkspaceLayoutDto {
            panes,
            active_pane,
            sidebar_width: 0,
        }
    }

    fn pane(paths: &[&str], active_tab: u32) -> WorkspacePaneDto {
        WorkspacePaneDto {
            tabs: paths
                .iter()
                .map(|p| WorkspaceTabDto {
                    path: (*p).to_owned(),
                    mode: EditorViewModeDto::Live,
                    pinned: false,
                })
                .collect(),
            active_tab,
            history: Vec::new(),
            history_cursor: 0,
        }
    }

    #[test]
    fn desktop_lens_close_cancels_ocr_without_touching_a_new_session() {
        let session = DesktopLensSession::default();
        let first = CancelToken::new();
        assert!(session.attach_ocr(first.clone()).is_none());

        session.activate();
        let first_generation = session
            .attach_ocr(first.clone())
            .expect("active lens accepts OCR");
        session.end();
        assert!(first.is_cancelled());

        session.activate();
        let second = CancelToken::new();
        assert!(session.attach_ocr(second.clone()).is_some());
        // 旧回调晚到也不得清走新会话的取消令牌。
        session.finish_ocr(first_generation);
        session.end();
        assert!(second.is_cancelled());
    }

    #[test]
    fn unique_template_note_uses_the_final_file_stem_for_title_variable() {
        let dir = tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        let rel = "Notes/笔记_20260718_093000.md";
        let create = |vault: &Vault| {
            create_unique_note_with(None, vault, rel, NoteType::Note, |stem| {
                format!("# {stem}\n\n{{{{unknown}}}}")
            })
            .unwrap()
        };
        let first = create(&vault);
        let second = create(&vault);
        assert_eq!(first.path, rel);
        assert_eq!(second.path, "Notes/笔记_20260718_093000-1.md");
        assert!(second.content.starts_with("# 笔记_20260718_093000-1"));
        assert!(second.content.ends_with("{{unknown}}"));
    }

    /// 布局剪枝：已删除路径的标签被丢弃，活跃标签按身份跟随（下标漂移后仍指向原标签）。
    #[test]
    fn prune_workspace_layout_drops_missing_and_follows_active_tab() {
        let got = prune_workspace_layout(
            layout(vec![pane(&["gone.md", "a.md", "b.md"], 2)], 0),
            |rel| rel != "gone.md",
        )
        .unwrap();
        assert_eq!(got.panes[0].tabs.len(), 2);
        assert_eq!(got.panes[0].tabs[1].path, "b.md");
        assert_eq!(got.panes[0].active_tab, 1);
    }

    /// **导航历史同受剪枝**（PRD-07-037）：一条指向已删除笔记的后退记录，按下去只会
    /// 打开一个空白标签，而用户读到的是「后退坏了」。游标按**对象身份**跟随，与活跃标签
    /// 那一步同一手法——按下标夹回会在剪掉前面几条之后指向另一篇。
    #[test]
    fn prune_workspace_layout_drops_missing_history_and_follows_the_cursor() {
        let mut pane = pane(&["a.md"], 0);
        pane.history = ["gone.md", "a.md", "also-gone.md", "b.md"]
            .iter()
            .map(|p| WorkspaceTabDto {
                path: (*p).to_owned(),
                mode: EditorViewModeDto::Live,
                pinned: false,
            })
            .collect();
        pane.history_cursor = 3; // 正停在 b.md 上

        let got = prune_workspace_layout(layout(vec![pane], 0), |path| {
            matches!(path, "a.md" | "b.md")
        })
        .expect("还有活着的标签");
        let history: Vec<&str> = got.panes[0]
            .history
            .iter()
            .map(|entry| entry.path.as_str())
            .collect();
        assert_eq!(history, vec!["a.md", "b.md"]);
        assert_eq!(
            got.panes[0].history_cursor, 1,
            "游标该跟着 b.md 走，而不是停在下标 3 被夹回到末位这种巧合上"
        );
    }

    /// 越界路径不进历史：与标签同一条判据（路径穿越拒绝）。
    #[test]
    fn prune_workspace_layout_rejects_escaping_history_paths() {
        let mut pane = pane(&["a.md"], 0);
        pane.history = ["../外面.md", "/绝对.md", "a.md"]
            .iter()
            .map(|p| WorkspaceTabDto {
                path: (*p).to_owned(),
                mode: EditorViewModeDto::Live,
                pinned: false,
            })
            .collect();
        pane.history_cursor = 2;

        let got = prune_workspace_layout(layout(vec![pane], 0), |_| true).expect("标签仍在");
        let history: Vec<&str> = got.panes[0]
            .history
            .iter()
            .map(|entry| entry.path.as_str())
            .collect();
        assert_eq!(history, vec!["a.md"]);
        assert_eq!(got.panes[0].history_cursor, 0);
    }

    /// 布局剪枝：整个 pane 失效即移除并夹回活跃 pane；全失效返回 None。
    #[test]
    fn prune_workspace_layout_removes_empty_panes_and_clamps() {
        let got = prune_workspace_layout(
            layout(vec![pane(&["gone.md"], 0), pane(&["a.md"], 0)], 1),
            |rel| rel == "a.md",
        )
        .unwrap();
        assert_eq!(got.panes.len(), 1);
        assert_eq!(got.active_pane, 0);

        assert!(
            prune_workspace_layout(layout(vec![pane(&["gone.md"], 0)], 0), |_| false).is_none()
        );
    }

    #[test]
    fn prune_workspace_layout_clamps_sidebar_width() {
        // PRD-07-057 可拖拽主侧栏宽：加载端为合法区间的权威口径；0 = 未定制保留。
        let mut wide = layout(vec![pane(&["a.md"], 0)], 0);
        wide.sidebar_width = 9999;
        assert_eq!(
            prune_workspace_layout(wide, |_| true)
                .unwrap()
                .sidebar_width,
            SIDEBAR_WIDTH_MAX
        );
        let mut narrow = layout(vec![pane(&["a.md"], 0)], 0);
        narrow.sidebar_width = 10;
        assert_eq!(
            prune_workspace_layout(narrow, |_| true)
                .unwrap()
                .sidebar_width,
            SIDEBAR_WIDTH_MIN
        );
        let unset = prune_workspace_layout(layout(vec![pane(&["a.md"], 0)], 0), |_| true).unwrap();
        assert_eq!(unset.sidebar_width, 0);
    }

    /// 布局剪枝：路径穿越（`..` / 绝对路径）一律不予恢复（防越出 Vault 根）。
    #[test]
    fn prune_workspace_layout_rejects_traversal_paths() {
        assert!(prune_workspace_layout(
            layout(vec![pane(&["../evil.md", "/abs.md"], 0)], 0),
            |_| true,
        )
        .is_none());
    }

    #[test]
    fn note_stem_strips_dir_and_extension() {
        assert_eq!(note_title_of("Notes/foo.md"), "foo");
        assert_eq!(note_title_of("bar.md"), "bar");
        assert_eq!(note_title_of("a/b/c.md"), "c");
    }

    /// 多分片排序 + 区间推导：显式区间（多 part）与缺省区间（单 part）各走各路。
    #[test]
    fn playback_parts_sort_and_derive_intervals() {
        let p1 = MediaRef {
            path: "Recordings/s-p01.wav".into(),
            part_start_ms: Some(5_000),
            part_end_ms: Some(8_000),
            ..Default::default()
        };
        let p0 = MediaRef {
            path: "Recordings/s-p00.wav".into(),
            part_start_ms: Some(0),
            part_end_ms: Some(5_000),
            ..Default::default()
        };
        let image = MediaRef {
            path: "assets/cover.png".into(),
            ..Default::default()
        };
        let media = vec![p1, image, p0];

        let sorted = sorted_media_parts(&media);
        assert_eq!(sorted.len(), 2, "非可播放媒体应被过滤");
        let parts = derive_playback_parts(&sorted);
        assert_eq!(parts[0].media_rel, "Recordings/s-p00.wav");
        assert_eq!(parts[1].start_ms, 5_000.0);
        assert_eq!(parts[1].end_ms, 8_000.0);
    }

    /// 单分片省略区间字段时由 duration（秒）推导 `[0, duration*1000)`。
    #[test]
    fn playback_single_part_defaults_interval_from_duration() {
        let single = MediaRef {
            path: "Recordings/s-p00.wav".into(),
            duration: Some(12.5),
            ..Default::default()
        };
        let media = vec![single];
        let parts = derive_playback_parts(&sorted_media_parts(&media));
        assert_eq!(parts[0].start_ms, 0.0);
        assert_eq!(parts[0].end_ms, 12_500.0);
    }

    #[test]
    fn onboarding_completed_steps_parse_trims_and_drops_empty() {
        assert_eq!(
            parse_completed_steps(" welcome , ,vault, "),
            vec!["welcome".to_owned(), "vault".to_owned()]
        );
        assert!(parse_completed_steps("").is_empty());
    }

    #[test]
    fn whisper_model_path_uses_realtime_asr_assignment() {
        let mut settings = Settings::new();
        settings.set(
            "ai.assignment.realtime_asr",
            crate::model_store::WHISPER_BASE_REMOTE_ID,
        );
        let root = temp_settings_path("whisper_model_assignment")
            .parent()
            .unwrap()
            .join("models");
        let weights = root
            .join(crate::model_store::WHISPER_BASE_REMOTE_ID)
            .join("weights.bin");
        std::fs::create_dir_all(weights.parent().unwrap()).unwrap();
        std::fs::write(&weights, b"fixture").unwrap();

        assert_eq!(
            whisper_model_path_for_assignment(&settings, &root, "realtime_asr"),
            Some(weights)
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn whisper_model_path_without_installed_assignment_skips_registration() {
        let settings = Settings::with_defaults();
        let root = temp_settings_path("whisper_model_empty")
            .parent()
            .unwrap()
            .join("models");
        assert_eq!(
            whisper_model_path_from_settings_and_root(&settings, &root),
            None
        );
    }

    #[test]
    fn model_assignments_filter_capabilities_and_preserve_inheritance() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = ModelStore::for_test_root(temp.path());
        store.add_downloaded_fixture("whisper-test").unwrap();
        store.add_engine_fixture("bge-m3-q8_0-gguf");
        store.add_engine_fixture("qwen3-embedding-0.6b-q8_0-gguf");
        let mut settings = Settings::with_defaults();
        settings.set("ai.assignment.realtime_asr", "whisper-test");

        let assignments = model_assignment_dtos(
            &settings,
            &store,
            &[],
            false,
            crate::ipc::models::ServerManagedFacts::all_clear(),
        );
        let realtime = assignments
            .iter()
            .find(|assignment| assignment.slot == "realtime_asr")
            .unwrap();
        let batch = assignments
            .iter()
            .find(|assignment| assignment.slot == "batch_asr")
            .unwrap();
        assert_eq!(realtime.effective_target.as_deref(), Some("whisper-test"));
        assert_eq!(batch.effective_target.as_deref(), Some("whisper-test"));
        assert_eq!(batch.source, "inherited");
        assert!(realtime
            .candidates
            .iter()
            .all(|candidate| candidate.target_id != "bge-m3-q8_0-gguf"));

        let embedding = assignments
            .iter()
            .find(|assignment| assignment.slot == "default_embedding")
            .unwrap();
        assert!(embedding
            .candidates
            .iter()
            .any(|candidate| candidate.target_id == "bge-m3-q8_0-gguf"));
        // 分块基准未过的 Qwen3-Embedding 装着也不进候选。
        assert!(!embedding
            .candidates
            .iter()
            .any(|candidate| candidate.target_id == "qwen3-embedding-0.6b-q8_0-gguf"));
    }

    #[test]
    fn native_cloud_projects_managed_text_and_embedding_without_server_model_candidates() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = ModelStore::for_test_root(temp.path());
        store.add_engine_fixture("qwen3.5-2b-q8_0-gguf");
        let assignments = model_assignment_dtos(
            &Settings::with_defaults(),
            &store,
            &[],
            true,
            crate::ipc::models::ServerManagedFacts::all_clear(),
        );
        let default_llm = assignments
            .iter()
            .find(|assignment| assignment.slot == "default_llm")
            .unwrap();
        assert_eq!(default_llm.selection_mode, ModelSelectionMode::Managed);
        assert_eq!(default_llm.readiness, "ready");
        assert_eq!(default_llm.effective_target, None);
        assert!(default_llm
            .candidates
            .iter()
            .all(|candidate| candidate.location == ModelExecutionLocation::Device));
        let embedding = assignments
            .iter()
            .find(|assignment| assignment.slot == "default_embedding")
            .unwrap();
        assert_eq!(embedding.selection_mode, ModelSelectionMode::Profile);
        assert_eq!(embedding.readiness, "ready");
    }

    /// 托管嵌入档的就绪**问注册表事实，不是无条件 `ready`**（真机第八轮）：装配没装上
    /// 云端嵌入时任务必然失败——就绪必须与执行同源，否则设置页写「可用」而每个任务都以
    /// `unresolved` 收场。
    #[test]
    fn server_managed_embedding_is_not_ready_when_assembly_left_no_provider() {
        let temp = tempfile::tempdir().unwrap();
        let store = ModelStore::for_test_root(temp.path());
        let assignments = model_assignment_dtos(
            &Settings::with_defaults(),
            &store,
            &[],
            true,
            crate::ipc::models::ServerManagedFacts {
                cloud_embed_registered: false,
            },
        );
        let readiness = |slot: &str| {
            assignments
                .iter()
                .find(|a| a.slot == slot)
                .unwrap()
                .readiness
                .clone()
        };
        assert_eq!(readiness("default_embedding"), "incompatible");
        assert_eq!(readiness("default_llm"), "ready");
    }

    /// 实时 LLM 的「服务端托管」判的是**整条继承链**：默认 LLM 显式指向本机时，
    /// 未配置的实时槽继承的是那个本机模型——把它标成托管，就是屏幕说服务端、字节走本机。
    #[test]
    fn realtime_slot_inheriting_explicit_device_override_is_not_reported_as_managed() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = ModelStore::for_test_root(temp.path());
        store.add_engine_fixture("qwen3.5-2b-q8_0-gguf");
        let mut settings = Settings::with_defaults();
        settings.set("ai.assignment.default_llm", "qwen3.5-2b-q8_0-gguf");

        let assignments = model_assignment_dtos(
            &settings,
            &store,
            &[],
            true,
            crate::ipc::models::ServerManagedFacts::all_clear(),
        );
        let realtime = assignments
            .iter()
            .find(|assignment| assignment.slot == "realtime_llm")
            .unwrap();
        assert_eq!(realtime.selection_mode, ModelSelectionMode::Model);
        assert_eq!(realtime.source, "inherited");
        assert_eq!(
            realtime.effective_target.as_deref(),
            Some("qwen3.5-2b-q8_0-gguf")
        );

        // 默认 LLM 自己也如实回到显式本机档，而不是托管。
        let default_llm = assignments
            .iter()
            .find(|assignment| assignment.slot == "default_llm")
            .unwrap();
        assert_eq!(default_llm.selection_mode, ModelSelectionMode::Model);
    }

    /// **显式钉了本机模型后，用户仍要能把对话/嵌入切回「服务端托管」**（ADR-037 连上
    /// 服务端即用它、ADR-048 服务端托管）。真机现象：连着组织服务端、服务端能跑 chat/embed，
    /// 而对话/嵌入卡显示「这台设备」，且下拉里**没有**对应转写那档「服务端转写」的
    /// 「服务端托管」——唯一的切回路径（清掉分配 = 下拉空选项「跟随」）在有本机 pin 时
    /// 被泛化地标成「跟随设备策略」，没人看得出它其实是「交回服务端」。
    ///
    /// `follow_is_server_managed` 是那个空选项该不该标成「服务端托管」的**唯一判据**：
    /// = 端点原生托管 且 清掉本槽后继承链不再有本机显式立场。前端据此如实标注。
    /// 反向验证：把这个字段恒置 false（或去掉 native 判定），本用例当场变红。
    #[test]
    fn pinned_device_chat_and_embed_still_expose_server_managed_switch_back() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = ModelStore::for_test_root(temp.path());
        store.add_engine_fixture("qwen3.5-2b-q8_0-gguf");
        let mut settings = Settings::with_defaults();
        settings.set("ai.assignment.default_llm", "qwen3.5-2b-q8_0-gguf");
        settings.set("ai.assignment.default_embedding", "bge-m3-q8_0-gguf");

        let managed = model_assignment_dtos(
            &settings,
            &store,
            &[],
            true,
            crate::ipc::models::ServerManagedFacts::all_clear(),
        );
        let slot = |name: &str| managed.iter().find(|a| a.slot == name).unwrap();

        // 对话/嵌入：显式本机 pin 被尊重（selection_mode = Model），但清掉它就落到服务端——
        // 那个空选项因此必须标成「服务端托管」，这正是缺失的那一档。
        assert!(
            slot("default_llm").follow_is_server_managed,
            "对话槽钉了本机后，切回服务端的路径在界面上没了名字"
        );
        assert!(
            slot("default_embedding").follow_is_server_managed,
            "嵌入槽同理——用户找不到把语义索引交回服务端的入口"
        );
        // 实时 LLM 继承的是本机的默认档：清掉它落到本机默认，而不是服务端——不得误标托管。
        assert!(
            !slot("realtime_llm").follow_is_server_managed,
            "实时槽清掉后继承的是本机默认档，标成服务端托管就是屏幕与字节去向相反"
        );
        // 转写还没同意交给服务端：清掉落到本机兜底，不得标成服务端托管。
        assert!(!slot("realtime_asr").follow_is_server_managed);
        assert!(!slot("batch_asr").follow_is_server_managed);

        // 未连服务端（非原生托管）时，「跟随」落到本机默认档，绝不能标成服务端托管。
        let offline = model_assignment_dtos(
            &settings,
            &store,
            &[],
            false,
            crate::ipc::models::ServerManagedFacts::all_clear(),
        );
        assert!(
            !offline
                .iter()
                .find(|a| a.slot == "default_llm")
                .unwrap()
                .follow_is_server_managed,
            "没连服务端却把「跟随」标成服务端托管，是承诺一个不存在的落点"
        );
    }

    /// 同意把转写交给服务端之后，转写两槽的空选项也叫「服务端托管」（ADR-059 ④）：此前它仍写
    /// 「自动选择」，而同一行的「当前生效」已是「服务端托管」。判的是清掉本槽后**真正**落到
    /// 哪里——实时档钉了本机，离线档清掉继承的就是那台本机模型，不得标托管。
    #[test]
    fn transcription_follow_is_named_server_managed_only_when_it_lands_on_the_server_channel() {
        let temp = tempfile::tempdir().unwrap();
        let store = ModelStore::for_test_root(temp.path());
        let mut settings = Settings::with_defaults();
        settings.set("provider.remote_asr_enabled", true);
        let follow = |settings: &Settings, name: &str| {
            model_assignment_dtos(
                settings,
                &store,
                &[],
                true,
                crate::ipc::models::ServerManagedFacts::all_clear(),
            )
            .into_iter()
            .find(|a| a.slot == name)
            .unwrap()
            .follow_is_server_managed
        };
        assert!(
            follow(&settings, "realtime_asr"),
            "同意后实时转写自动走服务端，空选项却没叫服务端托管"
        );
        assert!(
            follow(&settings, "batch_asr"),
            "离线转写继承实时档的服务端通道，空选项同理"
        );

        settings.set("ai.assignment.realtime_asr", DEVICE_POLICY_ASR);
        assert!(
            follow(&settings, "realtime_asr"),
            "实时档自己清掉后仍落到服务端"
        );
        assert!(
            !follow(&settings, "batch_asr"),
            "实时档钉了本机，离线档清掉继承的是本机模型，标成服务端托管就是屏幕与字节去向相反"
        );

        settings.set("ai.route_strategy", "local_only");
        assert!(!follow(&settings, "realtime_asr"), "仅本机时清掉也落在本机");
    }

    /// 转写槽位下拉为什么没有「服务端托管」：与算力卡同一份阻断原因，另加「还没同意」。
    /// 没有服务端可言时不说话。
    #[test]
    fn asr_server_gate_reports_the_first_closed_gate() {
        use crate::ipc::models::{asr_server_gate, AsrServerGate, TranscriptionServerBlocker};
        let mut settings = Settings::with_defaults();
        assert_eq!(
            asr_server_gate(None, &settings),
            Some(AsrServerGate::ConsentOff),
            "连着能转写的服务端、只差同意时，要说出差的是同意"
        );
        assert_eq!(
            asr_server_gate(Some(TranscriptionServerBlocker::NoServer), &settings),
            None
        );
        assert_eq!(
            asr_server_gate(
                Some(TranscriptionServerBlocker::LocalOnlyStrategy),
                &settings
            ),
            Some(AsrServerGate::Blocked {
                blocker: TranscriptionServerBlocker::LocalOnlyStrategy
            }),
            "同意项勾不动时照搬算力卡的原因，而不是叫用户去勾一个勾不动的开关"
        );
        settings.set("provider.remote_asr_enabled", true);
        assert_eq!(asr_server_gate(None, &settings), None);
    }

    /// 模型分配 DTO 里**一个字的界面文案都不许由 Rust 拼**（ADR-059 ④、TDD-15 i18n）。
    ///
    /// 此前 `service:asr` 那一档的候选名与有效名是写死的中文「服务端转写」：英文界面照样
    /// 显示中文，而且它是「服务端在替你跑」这同一个状态的**第四个名字**——同屏还有
    /// 「服务端托管（推荐）」「服务端托管（自动选择）」「服务端稳定向量档」。名字归 i18n
    /// 之后，术语才可能只有一个。
    ///
    /// 判据是「非 ASCII 字符」而不是"别写这四个字"：换一句中文照样红。本机模型的名字
    /// 不受影响——那是**数据**（目录 / 导入条目名），本来就该原样透出。
    #[test]
    fn assignment_dto_ships_no_ui_copy_for_server_hosted_targets() {
        let mut settings = Settings::with_defaults();
        settings.set("provider.remote_asr_enabled", true);
        let store = ModelStore::for_test();
        for dto in model_assignment_dtos(
            &settings,
            &store,
            &[],
            true,
            crate::ipc::models::ServerManagedFacts::all_clear(),
        ) {
            for candidate in &dto.candidates {
                if candidate.location == models::ModelExecutionLocation::Server {
                    assert!(
                        candidate.name.is_empty(),
                        "槽位 {} 的服务端候选带了展示名 {:?}——服务端托管的目标没有\
                         客户端知道的模型名（ADR-048），名字必须由前端按术语取 i18n",
                        dto.slot,
                        candidate.name
                    );
                }
            }
            if let Some(name) = &dto.effective_name {
                assert!(
                    name.is_ascii(),
                    "槽位 {} 的 effective_name 里出现了非 ASCII 文案 {name:?}——\
                     Rust 不产界面文案",
                    dto.slot
                );
            }
        }
    }

    /// 设备策略档此刻会挑中的本机模型（ADR-059 之后它不再是常量，测试必须自带清单，
    /// 否则断言会随跑测机器的内存档位漂移）。
    fn device_policy_fixture() -> (Vec<crate::local_engine::EngineModelRef>, String) {
        let models = vec![crate::local_engine::EngineModelRef {
            id: "qwen3.5-0.8b-q8_0-gguf".to_owned(),
            kind: crate::local_engine::EngineModelKind::Chat,
            digest: None,
            translation_only: false,
            size_bytes: 800,
        }];
        (models, "qwen3.5-0.8b-q8_0-gguf".to_owned())
    }

    /// 「翻译与实时档」：没显式选时，装了专用翻译模型就落它，没装就继承
    /// 默认对话档；专用翻译模型只在这一档就绪，在默认对话档是「不兼容」——它不会聊天。
    /// 前提是本平台带随包推理引擎：没有引擎的平台上 GGUF 条目不可见（`model_store::visible`）。
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn the_translation_tier_prefers_an_installed_translation_model() {
        use crate::local_engine::{EngineModelKind, EngineModelRef};
        let model = |id: &str, translation_only, size_bytes| EngineModelRef {
            id: id.to_owned(),
            kind: EngineModelKind::Chat,
            digest: None,
            translation_only,
            size_bytes,
        };
        let chat_only = vec![model("qwen3.5-2b-q8_0-gguf", false, 2_000)];
        let with_translator = vec![
            model("qwen3.5-2b-q8_0-gguf", false, 2_000),
            model("hy-mt2-1.8b-q8_0-gguf", true, 1_900),
        ];
        let settings = Settings::with_defaults();
        let target = |installed: &[EngineModelRef], slot| {
            effective_assignment_target_with(&settings, installed, slot)
        };
        assert_eq!(
            target(&chat_only, "realtime_llm").as_deref(),
            Some("qwen3.5-2b-q8_0-gguf")
        );
        assert_eq!(
            target(&with_translator, "realtime_llm").as_deref(),
            Some("hy-mt2-1.8b-q8_0-gguf")
        );
        // 默认对话档不因为专用翻译模型更小就选它。
        assert_eq!(
            target(&with_translator, "default_llm").as_deref(),
            Some("qwen3.5-2b-q8_0-gguf")
        );

        // 本机形态下翻译档单独定向；与默认档相同（继承）时不另注册。
        let availability = crate::cloud::CloudAvailability::SignedOut;
        assert_eq!(
            realtime_local_llm(
                &settings,
                &with_translator,
                availability,
                &crate::cloud::ServerRef::of(luna_cloud::config::DEFAULT_API_BASE)
            ),
            Some(LocalLlm("hy-mt2-1.8b-q8_0-gguf".into()))
        );
        assert_eq!(
            realtime_local_llm(
                &settings,
                &chat_only,
                availability,
                &crate::cloud::ServerRef::of(luna_cloud::config::DEFAULT_API_BASE)
            ),
            None
        );

        // 就绪：翻译档要 translate，默认档要 chat。
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("hy-mt2-1.8b-q8_0-gguf");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(crate::model_store::LUNA_MANAGED_GGUF_FILE), b"x").unwrap();
        let store = ModelStore::load(root.path().to_path_buf());
        let readiness = |slot, capability| {
            models::assignment_readiness(
                &store,
                slot,
                Some("hy-mt2-1.8b-q8_0-gguf"),
                capability,
                &settings,
            )
        };
        assert_eq!(readiness("realtime_llm", "translate"), "ready");
        assert_eq!(readiness("default_llm", "chat"), "incompatible");
    }

    /// 显式本机实时档与设备策略默认名撞名时，托管默认档不得把它送上服务端
    /// （ADR-048 之后名字不再决定执行位置，「同名即走默认档」只对本机默认档成立）。
    #[test]
    fn explicit_device_realtime_stays_targeted_when_default_is_server_managed() {
        let (installed, policy_target) = device_policy_fixture();
        let mut settings = Settings::with_defaults();
        settings.set("ai.assignment.realtime_llm", policy_target.as_str());
        assert_eq!(
            realtime_local_llm(
                &settings,
                &installed,
                crate::cloud::CloudAvailability::Allowed,
                &crate::cloud::ServerRef::of("http://10.0.0.62:8090"),
            )
            .map(|llm| llm.realtime_provider_id()),
            Some(luna_core::ProviderId::from(
                luna_provider::LOCAL_LLM_REALTIME_PROVIDER_ID
            )),
            "显式本机实时档必须保持定向注册，否则实时任务随默认档上服务端"
        );
    }

    /// 本机默认档下，与默认档同名的实时档仍合并进默认档（无谓多注册一份）。
    #[test]
    fn same_name_realtime_still_merges_into_on_device_default() {
        let (installed, policy_target) = device_policy_fixture();
        let mut settings = Settings::with_defaults();
        settings.set("ai.assignment.realtime_llm", policy_target.as_str());
        assert_eq!(
            realtime_local_llm(
                &settings,
                &installed,
                crate::cloud::CloudAvailability::SignedOut,
                &crate::cloud::ServerRef::of(luna_cloud::config::DEFAULT_API_BASE),
            ),
            None
        );
    }

    /// 实时链上没有任何显式选择 → 恒跟随默认档；托管落点下也不得注册本机实时档。
    #[test]
    fn unconfigured_realtime_chain_follows_managed_default() {
        let settings = Settings::with_defaults();
        assert_eq!(
            realtime_local_llm(
                &settings,
                &[],
                crate::cloud::CloudAvailability::Allowed,
                &crate::cloud::ServerRef::of("http://10.0.0.62:8090"),
            ),
            None
        );
    }

    #[test]
    fn legacy_service_model_target_is_rejected_without_migration_or_fallback() {
        let temp = tempfile::tempdir().unwrap();
        let store = ModelStore::for_test_root(temp.path());
        let settings = Settings::with_defaults();
        assert_eq!(
            assignment_readiness(
                &store,
                "default_llm",
                Some("service:qwen3:32b"),
                "chat",
                &settings,
            ),
            "incompatible"
        );
    }

    /// 展示名必须随 DTO 下发，且**未就绪的目标同样要有名字**。
    ///
    /// 前端曾从 `candidates` 反查它，而候选表只含已就绪的模型 → "有效目标尚未就绪"这一档
    /// 必然落空，界面把 `whisper-tiny-bundled` / `bge-m3-q8_0-gguf` 这类机器串直接摆给用户
    /// （真机坐实）。目录里根本没有该目标时才是 `None`（前端呈现"未解析"）。
    #[test]
    fn assignment_carries_display_name_even_when_target_is_not_ready() {
        let temp = tempfile::tempdir().unwrap();
        let store = ModelStore::for_test_root(temp.path());
        let settings = Settings::with_defaults();

        let assignments = model_assignment_dtos(
            &settings,
            &store,
            &[],
            false,
            crate::ipc::models::ServerManagedFacts::all_clear(),
        );
        let realtime = assignments
            .iter()
            .find(|assignment| assignment.slot == "realtime_asr")
            .unwrap();
        // 设备策略指向随包 whisper：此刻**未安装**，故它不在候选里。
        assert_eq!(realtime.readiness, "missing");
        assert!(realtime.candidates.is_empty());
        assert_eq!(
            realtime.effective_target.as_deref(),
            Some(crate::model_store::BUNDLED_WHISPER_ID)
        );
        let name = realtime
            .effective_name
            .as_deref()
            .expect("未就绪也要有展示名");
        assert_ne!(
            name,
            crate::model_store::BUNDLED_WHISPER_ID,
            "展示名不能是 target_id 本身"
        );
        // 展示名取自目录项本身，用例里不抄第二份——抄了的话换一档随包模型就会红，
        // 而红的原因与这条用例要证明的事（未就绪也得有展示名）毫无关系。
        assert_eq!(
            name,
            crate::model_store::remote_model_spec(crate::model_store::BUNDLED_WHISPER_ID)
                .expect("随包档必有目录项")
                .name
        );
    }

    #[test]
    fn delete_plan_blocks_direct_and_inherited_assignments() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = ModelStore::for_test_root(temp.path());
        store.add_downloaded_fixture("whisper-test").unwrap();
        let mut settings = Settings::with_defaults();
        settings.set("ai.assignment.realtime_asr", "whisper-test");

        let plan = delete_plan_for(&store, &settings, &[], "whisper-test").unwrap();
        assert!(!plan.can_delete);
        assert_eq!(
            plan.blocking_reason.as_deref(),
            Some("MODEL_DELETE_BLOCKED_ASSIGNMENT")
        );
        assert_eq!(
            plan.blocking_slots,
            vec!["realtime_asr".to_owned(), "batch_asr".to_owned()]
        );
    }

    #[test]
    fn install_plan_enforces_strict_local_and_free_space_reserve() {
        let store = ModelStore::for_test();
        let entry = store.entry(crate::model_store::BUNDLED_WHISPER_ID).unwrap();

        let strict = install_plan_from_entry(entry.clone(), true, Some(u64::MAX));
        assert!(!strict.can_start);
        assert_eq!(
            strict.blocking_reason.as_deref(),
            Some("STRICT_LOCAL_BLOCKED")
        );

        let insufficient = install_plan_from_entry(entry, false, Some(1));
        assert!(!insufficient.can_start);
        assert_eq!(
            insufficient.blocking_reason.as_deref(),
            Some("MODEL_DISK_SPACE_INSUFFICIENT")
        );
    }

    #[test]
    fn install_plan_counts_only_what_a_resume_still_needs() {
        let store = ModelStore::for_test();
        let mut entry = store.entry(crate::model_store::BUNDLED_WHISPER_ID).unwrap();
        entry.status = "available".to_owned();
        entry.size_bytes = Some(1000.0);
        entry.est_on_disk_bytes = Some(1000.0);

        let fresh = install_plan_from_entry(entry.clone(), false, Some(u64::MAX));
        assert_eq!(fresh.download_bytes, Some(1000.0));
        assert_eq!(fresh.resumed_bytes, None);

        entry.partial_bytes = Some(400.0);
        let resumed = install_plan_from_entry(entry, false, Some(10_000));
        assert_eq!(resumed.download_bytes, Some(600.0));
        assert_eq!(resumed.resumed_bytes, Some(400.0));
        // 断点已在盘上：完成后剩余 = 现有空闲 - 还需写入的部分。
        assert_eq!(resumed.remaining_after_bytes, Some(9_400.0));
    }

    #[test]
    fn vault_root_env_overrides_registry_setting() {
        let mut settings = Settings::new();
        settings.set("vault.root", "/from/settings");
        std::env::set_var("LUNA_VAULT_ROOT", "/from/env");

        assert_eq!(
            explicit_vault_root(&settings),
            Some(PathBuf::from("/from/env"))
        );

        std::env::remove_var("LUNA_VAULT_ROOT");
    }

    #[test]
    fn explicit_vault_root_is_none_without_config() {
        // 无 env、无 vault.root 设置 → 不自动选 Vault（MVP-0 门控，PRD-14）。
        std::env::remove_var("LUNA_VAULT_ROOT");
        let settings = Settings::new();
        assert_eq!(explicit_vault_root(&settings), None);
    }

    #[test]
    fn app_info_exposes_luna_product_name() {
        let info = app_info();
        assert_eq!(info.name, "Lunarca");
        assert_eq!(info.protocol_version, luna_cloud::protocol::CLIENT_PROTOCOL);
        assert_eq!(
            info.git_sha.len(),
            40,
            "build must identify an exact Git revision"
        );
        assert!(
            info.git_sha.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "Git revision must be hexadecimal: {}",
            info.git_sha
        );
        assert!(
            chrono::DateTime::parse_from_rfc3339(&info.build_time).is_ok(),
            "build time must be an RFC 3339 timestamp: {}",
            info.build_time
        );
    }

    #[test]
    fn vault_delete_mode_uses_registry_default() {
        let settings = Settings::with_defaults();
        assert_eq!(
            vault_delete_mode_from_settings(&settings),
            DeleteMode::Trash
        );

        let mut settings = Settings::new();
        settings.set("vault.delete_mode", "vault_trash");
        assert_eq!(
            vault_delete_mode_from_settings(&settings),
            DeleteMode::VaultTrash
        );
    }

    #[test]
    fn floating_menu_job_maps_actions_to_job_backbone() {
        let mut settings = Settings::with_defaults();
        settings.set("popup_translate.native_lang", "zh");
        settings.set("popup_translate.default_foreign", "en");
        let detector = LinguaDetector::new();

        let summarize = floating_menu_job(
            None,
            FloatingMenuRequest {
                text: "selected".into(),
                action: FloatingMenuAction::Summarize,
                scope: FloatingMenuScope::Selection,
                instruction: None,
                context: FloatingMenuContext::default(),
                options: FloatingMenuOptionsDto::default(),
                strict_local: true,
                meeting_minutes: false,
                glossary: &[],
                speaking_rate: None,
            },
            &settings,
            &detector,
        )
        .expect("summarize job");
        assert_eq!(summarize.origin, JobOrigin::FloatingMenu);
        assert!(summarize.routing.local_only);
        assert!(matches!(summarize.kind, JobKind::Summarize { .. }));

        let optimize = floating_menu_job(
            None,
            FloatingMenuRequest {
                text: "selected".into(),
                action: FloatingMenuAction::Optimize,
                scope: FloatingMenuScope::Selection,
                instruction: None,
                context: FloatingMenuContext::default(),
                options: FloatingMenuOptionsDto::default(),
                strict_local: true,
                meeting_minutes: false,
                glossary: &[],
                speaking_rate: None,
            },
            &settings,
            &detector,
        )
        .expect("optimize job");
        assert_eq!(optimize.origin, JobOrigin::FloatingMenu);
        assert!(optimize.routing.local_only);
        match optimize.kind {
            JobKind::Refine { instruction, .. } => {
                let instruction = instruction.expect("optimize instruction");
                // 无论检测结果如何，优化指令都显式禁止翻译（根治优化退化成翻译）。
                assert!(instruction.contains("never translate"));
                assert!(instruction.contains("Output only"));
            }
            _ => panic!("expected refine job"),
        }

        let translate = floating_menu_job(
            None,
            FloatingMenuRequest {
                text: "你好，世界。".into(),
                action: FloatingMenuAction::Translate,
                scope: FloatingMenuScope::Selection,
                instruction: None,
                context: FloatingMenuContext::default(),
                options: FloatingMenuOptionsDto::default(),
                strict_local: true,
                meeting_minutes: false,
                glossary: &[],
                speaking_rate: None,
            },
            &settings,
            &detector,
        )
        .expect("translate job");
        assert_eq!(translate.origin, JobOrigin::FloatingMenu);
        assert!(translate.routing.local_only);
        match translate.kind {
            JobKind::Translate { target, .. } => assert_eq!(target, Lang::from("en")),
            _ => panic!("expected translate job"),
        }
    }

    /// 建议落地方式按「动作 × 作用域」定（PRD-06-021）：改这一段的替换、要留原文的插入
    /// 下方、整篇总结插顶部、光标处一律插此处。
    #[test]
    fn floating_menu_suggested_landing_follows_action_and_scope() {
        use FloatingMenuAction as A;
        use FloatingMenuLanding as L;
        use FloatingMenuScope as S;
        assert_eq!(suggested_landing(A::Optimize, S::Selection), L::Replace);
        assert_eq!(suggested_landing(A::Ask, S::Selection), L::Replace);
        assert_eq!(
            suggested_landing(A::Translate, S::Selection),
            L::InsertBelow
        );
        assert_eq!(
            suggested_landing(A::Summarize, S::Selection),
            L::InsertBelow
        );
        assert_eq!(suggested_landing(A::Summarize, S::Doc), L::InsertTop);
        assert_eq!(suggested_landing(A::Ask, S::Doc), L::InsertEnd);
        for action in [A::Continue, A::Ask, A::Summarize] {
            assert_eq!(suggested_landing(action, S::Cursor), L::InsertCursor);
        }
    }

    /// 上下文每侧封顶：前文取尾部、后文取头部；空白只算空。
    #[test]
    fn floating_menu_context_is_clamped_per_side() {
        let long: String = "a".repeat(FLOATING_MENU_CONTEXT_MAX_CHARS + 50);
        let before = format!("HEAD{long}");
        let after = format!("{long}TAIL");
        let context = FloatingMenuContext::clamped(Some(&before), Some(&after));
        assert_eq!(
            context.before.chars().count(),
            FLOATING_MENU_CONTEXT_MAX_CHARS
        );
        assert!(
            !context.before.starts_with("HEAD"),
            "前文应保留离选区最近的尾部"
        );
        assert_eq!(
            context.after.chars().count(),
            FLOATING_MENU_CONTEXT_MAX_CHARS
        );
        assert!(
            !context.after.ends_with("TAIL"),
            "后文应保留离选区最近的头部"
        );
        assert_eq!(context.chars(), FLOATING_MENU_CONTEXT_MAX_CHARS * 2);
        assert!(FloatingMenuContext::clamped(Some("  "), None).is_empty());
    }

    /// 带上下文的优化 / 自定义指令把前后文以「仅供参考」条款拼进指令，且明确不许输出它。
    #[test]
    fn floating_menu_context_is_appended_as_reference_only() {
        let settings = Settings::with_defaults();
        let detector = LinguaDetector::new();
        let job = floating_menu_job(
            None,
            FloatingMenuRequest {
                text: "middle sentence".into(),
                action: FloatingMenuAction::Optimize,
                scope: FloatingMenuScope::Selection,
                instruction: None,
                context: FloatingMenuContext {
                    before: "Opening line.".into(),
                    after: "Closing line.".into(),
                },
                options: FloatingMenuOptionsDto::default(),
                strict_local: true,
                meeting_minutes: false,
                glossary: &[],
                speaking_rate: None,
            },
            &settings,
            &detector,
        )
        .expect("optimize job");
        match job.kind {
            JobKind::Refine { instruction, .. } => {
                let instruction = instruction.expect("instruction");
                assert!(instruction.contains("Opening line."));
                assert!(instruction.contains("Closing line."));
                assert!(instruction.contains("do not rewrite, repeat, or include it"));
            }
            _ => panic!("expected refine job"),
        }
    }

    /// 长文策略（PRD-06-025）：未超上限直出；整篇总结分段；改写类截断；光标处取尾。
    #[test]
    fn floating_menu_strategy_by_length_scope_and_action() {
        use FloatingMenuStrategy as S;
        let limit = FLOATING_MENU_INPUT_MAX_CHARS;
        let over = limit * 3 + 1;
        assert_eq!(
            floating_menu_strategy(10, FloatingMenuScope::Doc, None, limit),
            (S::Direct, 0)
        );
        assert_eq!(
            floating_menu_strategy(over, FloatingMenuScope::Doc, None, limit),
            (S::Chunked, 4)
        );
        assert_eq!(
            floating_menu_strategy(
                over,
                FloatingMenuScope::Doc,
                Some(FloatingMenuAction::Summarize),
                limit
            ),
            (S::Chunked, 4)
        );
        assert_eq!(
            floating_menu_strategy(
                over,
                FloatingMenuScope::Doc,
                Some(FloatingMenuAction::Ask),
                limit
            ),
            (S::Truncated, 0)
        );
        assert_eq!(
            floating_menu_strategy(
                over,
                FloatingMenuScope::Selection,
                Some(FloatingMenuAction::Optimize),
                limit
            ),
            (S::Truncated, 0)
        );
        assert_eq!(
            floating_menu_strategy(over, FloatingMenuScope::Cursor, None, limit),
            (S::Tail, 0)
        );
        let huge = limit * 40;
        assert_eq!(
            floating_menu_strategy(huge, FloatingMenuScope::Doc, None, limit).1 as usize,
            super::distill::DISTILL_MAX_CHUNKS
        );
    }

    /// 有效上限跟着上下文窗走（PRD-06-025）：未探测取封顶；探测到就按窗收窄并留出输出；
    /// 窗再大也不越过封顶。上限若不跟窗走，超窗那一段要么被引擎拒收、要么（外部端点）被
    /// 静默截断，面板上却没有任何截断提示。
    #[test]
    fn floating_input_limit_follows_the_probed_context_window() {
        assert_eq!(floating_input_limit(0), FLOATING_MENU_INPUT_MAX_CHARS);
        assert_eq!(floating_input_limit(4_096), 3_072);
        assert_eq!(floating_input_limit(8_192), 6_144);
        assert_eq!(floating_input_limit(16_384), FLOATING_MENU_INPUT_MAX_CHARS);
        assert_eq!(
            floating_input_limit(1_000_000),
            FLOATING_MENU_INPUT_MAX_CHARS
        );
        // 预检与提交同一判据：按收窄后的上限，8k 字符的整篇在 4k 窗上必须分段。
        assert!(matches!(
            floating_menu_strategy(
                8_000,
                FloatingMenuScope::Doc,
                None,
                floating_input_limit(4_096)
            ),
            (FloatingMenuStrategy::Chunked, 3)
        ));
    }

    /// 变更解释指令：点名改动形态（改 / 增 / 删）、只要一句、用文本语言。
    #[test]
    fn explain_change_instruction_names_the_edit_and_language() {
        let detection = Detection::confident(Lang::from("en"));
        let changed = explain_change_instruction("recieved", "received", &detection);
        assert!(changed.contains("changing \"recieved\" to \"received\""));
        assert!(changed.contains("in English"));
        assert!(changed.contains("one short sentence"));
        let inserted = explain_change_instruction("", "了", &Detection::unknown());
        assert!(inserted.contains("inserting \"了\""));
        assert!(inserted.contains("same language as the sentence"));
        let removed = explain_change_instruction("合", "", &Detection::unknown());
        assert!(removed.contains("removing \"合\""));
    }

    /// 光标处定稿整理：输出开头重复了输入尾行（标题 / 上一行）时剥掉；不重复则原样。
    #[test]
    fn strip_leading_overlap_drops_echoed_tail_lines_only() {
        let input = "前文。\n\n## 高级技巧与扩展";
        assert_eq!(
            strip_leading_overlap(input, "## 高级技巧与扩展\n\n- 动态斜率切换"),
            "- 动态斜率切换"
        );
        assert_eq!(
            strip_leading_overlap(input, "前文。\n## 高级技巧与扩展\n正文"),
            "正文"
        );
        assert_eq!(strip_leading_overlap(input, "全新的一段。"), "全新的一段。");
        assert_eq!(strip_leading_overlap("", "任何输出"), "任何输出");
    }

    /// 校对 → Refine：只改错不改写、语言锁定、只输出改正稿；改语气三档同样锁语言；
    /// 翻译子菜单点名目标语言时走固定目标；总结风格按子菜单或设置。
    #[test]
    fn floating_menu_batch2_actions_map_to_backbone() {
        let mut settings = Settings::with_defaults();
        settings.set("popup_translate.native_lang", "zh");
        settings.set("popup_translate.default_foreign", "en");
        let detector = LinguaDetector::new();
        let request = |action, options| FloatingMenuRequest {
            text: "Please review the attached document and reply by friday.".into(),
            action,
            scope: FloatingMenuScope::Selection,
            instruction: None,
            context: FloatingMenuContext::default(),
            options,
            strict_local: true,
            meeting_minutes: false,
            glossary: &[],
            speaking_rate: None,
        };
        let proofread = floating_menu_job(
            None,
            request(
                FloatingMenuAction::Proofread,
                FloatingMenuOptionsDto::default(),
            ),
            &settings,
            &detector,
        )
        .expect("proofread job");
        match proofread.kind {
            JobKind::Refine { instruction, .. } => {
                let instruction = instruction.expect("instruction");
                assert!(instruction.contains("Fix only spelling, grammar, and punctuation"));
                assert!(instruction.contains("must stay in English"));
                assert!(instruction.contains("Output only"));
            }
            _ => panic!("expected refine job"),
        }
        let concise = floating_menu_job(
            None,
            request(
                FloatingMenuAction::ToneConcise,
                FloatingMenuOptionsDto::default(),
            ),
            &settings,
            &detector,
        )
        .expect("tone job");
        match concise.kind {
            JobKind::Refine { instruction, .. } => {
                assert!(instruction.expect("instruction").contains("concise"));
            }
            _ => panic!("expected refine job"),
        }
        // 视频脚本的四个改稿动作（PRD-19-013/014）：都是就地改写，指令各带自己的约束与语言锁。
        for (action, needle) in [
            (FloatingMenuAction::ScriptTighten, "fewer words"),
            (FloatingMenuAction::ScriptHook, "never invent"),
            (
                FloatingMenuAction::ScriptSpoken,
                "sounds natural when spoken",
            ),
            (
                FloatingMenuAction::ScriptDictation,
                "keep only the corrected version",
            ),
        ] {
            assert!(action.rewrites_in_place(), "{action:?}");
            assert_eq!(FloatingMenuAction::from_tag(action.as_str()), Some(action));
            let job = floating_menu_job(
                None,
                request(action, FloatingMenuOptionsDto::default()),
                &settings,
                &detector,
            )
            .expect("script job");
            match job.kind {
                JobKind::Refine { instruction, .. } => {
                    let instruction = instruction.expect("instruction");
                    assert!(instruction.contains(needle), "{action:?}");
                    assert!(instruction.contains("`broll` code blocks"), "{action:?}");
                }
                _ => panic!("expected refine job for {action:?}"),
            }
        }
        let translate_ja = floating_menu_job(
            None,
            request(
                FloatingMenuAction::Translate,
                FloatingMenuOptionsDto {
                    target_lang: Some("ja".into()),
                    summary_style: None,
                    table_column: None,
                    target_seconds: None,
                },
            ),
            &settings,
            &detector,
        )
        .expect("translate job");
        match translate_ja.kind {
            JobKind::Translate { target, .. } => assert_eq!(target, Lang::from("ja")),
            _ => panic!("expected translate job"),
        }
        let one_line = floating_menu_job(
            None,
            request(
                FloatingMenuAction::Summarize,
                FloatingMenuOptionsDto {
                    target_lang: None,
                    summary_style: Some(FloatingSummaryStyle::OneLine),
                    table_column: None,
                    target_seconds: None,
                },
            ),
            &settings,
            &detector,
        )
        .expect("summary job");
        match one_line.kind {
            JobKind::Summarize { instruction, .. } => {
                assert!(instruction
                    .expect("instruction")
                    .contains("single sentence"));
            }
            _ => panic!("expected summarize job"),
        }
        // 表格 AI（对标方案 T6）：整理 / 解释各是一趟改写，提示词来自注册表；按列填充不是单个 Job。
        for (action, key) in [
            (
                FloatingMenuAction::TableFromText,
                "header row, a delimiter row",
            ),
            (FloatingMenuAction::TableExplain, "Explain what it shows"),
        ] {
            let job = floating_menu_job(
                None,
                request(action, FloatingMenuOptionsDto::default()),
                &settings,
                &detector,
            )
            .expect("table job");
            match job.kind {
                JobKind::Refine { instruction, .. } => {
                    assert!(instruction.expect("instruction").contains(key))
                }
                _ => panic!("expected refine job"),
            }
        }
        assert!(floating_menu_job(
            None,
            request(
                FloatingMenuAction::TableFillColumn,
                FloatingMenuOptionsDto::default()
            ),
            &settings,
            &detector
        )
        .is_err());
        assert!(FloatingMenuAction::TableFromText.rewrites_in_place());
        assert!(!FloatingMenuAction::TableExplain.rewrites_in_place());
        assert_eq!(
            FloatingMenuAction::from_tag("table_fill_column"),
            Some(FloatingMenuAction::TableFillColumn)
        );
        assert_eq!(
            suggested_landing(FloatingMenuAction::Proofread, FloatingMenuScope::Selection),
            FloatingMenuLanding::Replace
        );
        assert_eq!(
            suggested_landing(FloatingMenuAction::ToneFormal, FloatingMenuScope::Selection),
            FloatingMenuLanding::Replace
        );
    }

    /// 续写 → Refine，指令只要接续内容、不重复给定文本，且点名语言。
    #[test]
    fn floating_menu_continue_maps_to_refine_with_continuation_rule() {
        let settings = Settings::with_defaults();
        let detector = LinguaDetector::new();
        let job = floating_menu_job(
            None,
            FloatingMenuRequest {
                text: "Please review the attached document and".into(),
                action: FloatingMenuAction::Continue,
                scope: FloatingMenuScope::Cursor,
                instruction: None,
                context: FloatingMenuContext::default(),
                options: FloatingMenuOptionsDto::default(),
                strict_local: true,
                meeting_minutes: false,
                glossary: &[],
                speaking_rate: None,
            },
            &settings,
            &detector,
        )
        .expect("continue job");
        match job.kind {
            JobKind::Refine { instruction, .. } => {
                let instruction = instruction.expect("instruction");
                assert!(instruction.contains("Continue writing"));
                assert!(instruction.contains("do not repeat any of the given text"));
                assert!(instruction.contains("in English"));
            }
            _ => panic!("expected refine job"),
        }
    }

    #[test]
    fn floating_menu_ask_requires_instruction_and_carries_it_into_refine() {
        let settings = Settings::with_defaults();
        let detector = LinguaDetector::new();

        let missing = floating_menu_job(
            None,
            FloatingMenuRequest {
                text: "selected".into(),
                action: FloatingMenuAction::Ask,
                scope: FloatingMenuScope::Selection,
                instruction: None,
                context: FloatingMenuContext::default(),
                options: FloatingMenuOptionsDto::default(),
                strict_local: true,
                meeting_minutes: false,
                glossary: &[],
                speaking_rate: None,
            },
            &settings,
            &detector,
        );
        assert!(missing.is_err(), "空指令必须拒绝");
        let blank = floating_menu_job(
            None,
            FloatingMenuRequest {
                text: "selected".into(),
                action: FloatingMenuAction::Ask,
                scope: FloatingMenuScope::Selection,
                instruction: Some("   "),
                context: FloatingMenuContext::default(),
                options: FloatingMenuOptionsDto::default(),
                strict_local: true,
                meeting_minutes: false,
                glossary: &[],
                speaking_rate: None,
            },
            &settings,
            &detector,
        );
        assert!(blank.is_err(), "全空白指令必须拒绝");

        let ask = floating_menu_job(
            None,
            FloatingMenuRequest {
                text: "selected".into(),
                action: FloatingMenuAction::Ask,
                scope: FloatingMenuScope::Selection,
                instruction: Some("改写成正式邮件"),
                context: FloatingMenuContext::default(),
                options: FloatingMenuOptionsDto::default(),
                strict_local: true,
                meeting_minutes: false,
                glossary: &[],
                speaking_rate: None,
            },
            &settings,
            &detector,
        )
        .expect("ask job");
        assert_eq!(ask.origin, JobOrigin::FloatingMenu);
        match ask.kind {
            JobKind::Refine { instruction, .. } => {
                let instruction = instruction.expect("ask instruction");
                assert!(instruction.contains("改写成正式邮件"));
                assert!(instruction.contains("Output only"));
            }
            _ => panic!("expected refine job"),
        }
    }

    #[test]
    fn optimize_instruction_pins_detected_language_across_scripts_and_forbids_translation() {
        // 通用适配：任一检测语言都点名其英文全称（非只中英，避免硬编码），并保持字形变体。
        for (code, name) in [
            ("zh", "Chinese"),
            ("en", "English"),
            ("fr", "French"),
            ("ja", "Japanese"),
            ("ko", "Korean"),
            ("de", "German"),
            ("ru", "Russian"),
        ] {
            let instruction = optimize_instruction(
                &Detection::confident(Lang::from(code)),
                &FloatingMenuContext::default(),
            );
            assert!(
                instruction.contains(&format!("must stay in {name}")),
                "{code} 应点名 {name}"
            );
            // 每种语言都强制字形变体保持（覆盖繁简中文等）+ 禁翻译。
            assert!(instruction.contains("same script and character variant"));
            assert!(instruction.contains("never translate"));
            assert!(instruction.contains("Output only"));
        }

        // 检测不可靠 → 回退「与输入同语言」通用措辞，仍保字形 + 禁翻译。
        let unknown = optimize_instruction(&Detection::unknown(), &FloatingMenuContext::default());
        assert!(unknown.contains("must stay in the same language as the input"));
        assert!(unknown.contains("same script and character variant"));
        assert!(unknown.contains("never translate"));
    }

    #[test]
    fn floating_menu_optimize_pins_language_via_detector() {
        let settings = Settings::with_defaults();
        let detector = LinguaDetector::new();

        // 端到端：各语言输入经真实检测器 → 优化 Job 指令点名对应语言全称。
        for (text, name) in [
            ("今天天气很好，我们一起去公园散步吧。", "Chinese"),
            (
                "Please review the attached document and reply by Friday.",
                "English",
            ),
            (
                "Veuillez relire le document ci-joint et me répondre.",
                "French",
            ),
        ] {
            let optimize = floating_menu_job(
                None,
                FloatingMenuRequest {
                    text: text.into(),
                    action: FloatingMenuAction::Optimize,
                    scope: FloatingMenuScope::Selection,
                    instruction: None,
                    context: FloatingMenuContext::default(),
                    options: FloatingMenuOptionsDto::default(),
                    strict_local: true,
                    meeting_minutes: false,
                    glossary: &[],
                    speaking_rate: None,
                },
                &settings,
                &detector,
            )
            .expect("optimize job");
            match optimize.kind {
                JobKind::Refine { instruction, .. } => {
                    let instruction = instruction.expect("optimize instruction");
                    assert!(
                        instruction.contains(&format!("must stay in {name}")),
                        "输入应被检测并点名 {name}"
                    );
                    assert!(instruction.contains("never translate"));
                }
                _ => panic!("expected refine job"),
            }
        }
    }

    #[test]
    fn optimize_instruction_preserves_traditional_chinese_script() {
        // 繁简同为 ISO 639-1 `zh`，检测层无从区分；靠 prompt 的字形保持约束（举繁简为例）
        // 交 LLM 保持繁体，不在 Rust 端引入繁简字符表（避免硬编码/新依赖）。
        let zh = optimize_instruction(
            &Detection::confident(Lang::from("zh")),
            &FloatingMenuContext::default(),
        );
        assert!(zh.contains("Traditional Chinese"));
        assert!(zh.contains("same script and character variant"));
    }

    #[test]
    fn floating_input_truncates_over_limit_at_char_boundary() {
        let short = "短文本".to_owned();
        assert_eq!(
            truncate_floating_input(short.clone(), FLOATING_MENU_INPUT_MAX_CHARS),
            (short, None)
        );

        let long: String = "语".repeat(FLOATING_MENU_INPUT_MAX_CHARS + 5);
        let (kept, notice) = truncate_floating_input(long, FLOATING_MENU_INPUT_MAX_CHARS);
        assert_eq!(kept.chars().count(), FLOATING_MENU_INPUT_MAX_CHARS);
        assert_eq!(notice, Some(FLOATING_MENU_INPUT_MAX_CHARS as u32));
    }

    #[test]
    fn floating_menu_summary_language_defaults_to_native_and_can_be_configured() {
        let mut settings = Settings::with_defaults();
        settings.set("popup_translate.native_lang", "zh");
        settings.set("editor.floating_summary_lang", "native");
        let detector = LinguaDetector::new();

        let native_summary = floating_menu_job(
            None,
            FloatingMenuRequest {
                text: "selected".into(),
                action: FloatingMenuAction::Summarize,
                scope: FloatingMenuScope::Selection,
                instruction: None,
                context: FloatingMenuContext::default(),
                options: FloatingMenuOptionsDto::default(),
                strict_local: false,
                meeting_minutes: false,
                glossary: &[],
                speaking_rate: None,
            },
            &settings,
            &detector,
        )
        .expect("summary job");
        match native_summary.kind {
            JobKind::Summarize { instruction, .. } => {
                let instruction = instruction.expect("summary instruction");
                assert!(
                    instruction.contains("bullet points"),
                    "要点档必须真的要求要点——此前只写「总结」，要点从未被要求过"
                );
                assert!(
                    instruction.contains("in Chinese"),
                    "语言用全称点名（TDD-03 §3.3）：`zh` 对小模型几乎不起作用"
                );
            }
            _ => panic!("expected summarize job"),
        }

        settings.set("editor.floating_summary_lang", "en");
        let english_summary = floating_menu_job(
            None,
            FloatingMenuRequest {
                text: "selected".into(),
                action: FloatingMenuAction::Summarize,
                scope: FloatingMenuScope::Selection,
                instruction: None,
                context: FloatingMenuContext::default(),
                options: FloatingMenuOptionsDto::default(),
                strict_local: false,
                meeting_minutes: false,
                glossary: &[],
                speaking_rate: None,
            },
            &settings,
            &detector,
        )
        .expect("summary job");
        match english_summary.kind {
            JobKind::Summarize { instruction, .. } => {
                let instruction = instruction.expect("summary instruction");
                assert!(instruction.contains("bullet points"));
                assert!(instruction.contains("in English"));
            }
            _ => panic!("expected summarize job"),
        }
    }

    /// 会议纪要模式（PRD-04-054）：总结指令转「决议/行动项归属/待决问题」结构，
    /// 语言用全称点名（小模型敏感度，TDD-03 §3.3 同策略）；普通模式保持原指令不变。
    #[test]
    fn floating_menu_meeting_minutes_instruction_carries_owners_structure() {
        let mut settings = Settings::with_defaults();
        settings.set("popup_translate.native_lang", "zh");
        let detector = LinguaDetector::new();

        let minutes = floating_menu_job(
            None,
            FloatingMenuRequest {
                text: "张三: 我们下周发布。\n李四: 我来准备发布说明。".into(),
                action: FloatingMenuAction::Summarize,
                scope: FloatingMenuScope::Selection,
                instruction: None,
                context: FloatingMenuContext::default(),
                options: FloatingMenuOptionsDto::default(),
                strict_local: true,
                meeting_minutes: true,
                glossary: &[],
                speaking_rate: None,
            },
            &settings,
            &detector,
        )
        .expect("minutes job");
        match minutes.kind {
            JobKind::Summarize { instruction, .. } => {
                let instruction = instruction.expect("minutes instruction");
                assert!(instruction.contains("meeting transcript"));
                assert!(instruction.contains("key decisions"));
                assert!(instruction.contains("action items"));
                assert!(instruction.contains("speaker names"));
                assert!(instruction.contains("Chinese"), "纪要语言点名全称");
            }
            _ => panic!("expected summarize job"),
        }
    }

    #[test]
    fn ai_artifact_path_uses_configured_dir_without_changing_file_stem() {
        let mut settings = Settings::new();
        settings.set("vault.ai_artifacts_dir", "Artifacts");

        assert_eq!(
            ai_artifact_rel_path(&settings, "chat-123"),
            "Artifacts/chat-123.md"
        );
    }

    #[test]
    fn ai_artifact_path_falls_back_when_configured_dir_is_not_relative_safe() {
        let mut settings = Settings::new();
        settings.set("vault.ai_artifacts_dir", "/tmp/leak");
        assert_eq!(
            ai_artifact_rel_path(&settings, "chat-123"),
            "AI/chat-123.md"
        );

        settings.set("vault.ai_artifacts_dir", "../leak");
        assert_eq!(
            ai_artifact_rel_path(&settings, "chat-123"),
            "AI/chat-123.md"
        );

        settings.set("vault.ai_artifacts_dir", "");
        assert_eq!(
            ai_artifact_rel_path(&settings, "chat-123"),
            "AI/chat-123.md"
        );

        settings.set("vault.ai_artifacts_dir", "./Artifacts");
        assert_eq!(
            ai_artifact_rel_path(&settings, "chat-123"),
            "Artifacts/chat-123.md"
        );
    }

    #[test]
    fn translate_space_note_body_preserves_both_user_visible_texts() {
        assert_eq!(
            translate_space_note_body("Source paragraph", "译文段落"),
            "## 原文\n\nSource paragraph\n\n## 翻译\n\n译文段落\n"
        );
    }

    #[test]
    fn translate_space_directory_is_user_configurable_and_stays_in_the_vault() {
        let mut settings = Settings::new();
        settings.set("vault.translate_space_dir", "知识/译境");
        assert_eq!(
            safe_vault_relative_dir(&settings, "vault.translate_space_dir", "TranslateSpace"),
            "知识/译境"
        );

        settings.set("vault.translate_space_dir", "../../escape");
        assert_eq!(
            safe_vault_relative_dir(&settings, "vault.translate_space_dir", "TranslateSpace"),
            "TranslateSpace"
        );
    }

    #[test]
    fn note_type_dir_maps_each_type_to_its_configured_dir() {
        let mut settings = Settings::new();
        // 默认值（未配置）。
        assert_eq!(note_type_dir(&settings, NoteType::Note), "Notes");
        assert_eq!(note_type_dir(&settings, NoteType::QuickNote), "QuickNotes");
        assert_eq!(note_type_dir(&settings, NoteType::Recording), "Recordings");
        assert_eq!(note_type_dir(&settings, NoteType::Chat), "Chats");
        assert_eq!(
            note_type_dir(&settings, NoteType::Transcript),
            "Transcripts"
        );
        assert_eq!(note_type_dir(&settings, NoteType::VideoProject), "Creator");
        assert_eq!(note_type_dir(&settings, NoteType::Script), "Creator");
        assert_eq!(note_type_dir(&settings, NoteType::Swipe), "Creator/Swipe");
        settings.set("vault.creator_dir", "视频");
        assert_eq!(note_type_dir(&settings, NoteType::Swipe), "视频/Swipe");
        settings.set("vault.creator_dir", "Creator");

        // 用户自定义目录生效；越界值回退默认。
        settings.set("vault.recordings_dir", "会议录音");
        assert_eq!(note_type_dir(&settings, NoteType::Recording), "会议录音");
        settings.set("vault.notes_dir", "../escape");
        assert_eq!(note_type_dir(&settings, NoteType::Note), "Notes");
    }

    /// 资料区目录清单的判据是 `is_source_material`，而不是任何一份手抄的类型清单。
    ///
    /// 反向验证：把判据换成写死的三个 key，本用例的第一条断言（穷尽性）不会红，
    /// 但把某一类移出资料层时它会——这正是前端那份 `MATERIAL_TYPES` 抓不到的那一类漂移。
    #[test]
    fn material_dirs_follow_the_layering_predicate_and_dedupe() {
        let mut settings = Settings::new();
        let expected: Vec<String> = NoteType::ALL
            .into_iter()
            .filter(|t| t.is_source_material())
            .map(|t| note_type_dir(&settings, t))
            .collect();
        assert_eq!(material_archive_dirs(&settings), expected);
        assert_eq!(
            material_archive_dirs(&settings),
            vec!["Recordings", "Transcripts", "Chats", "Documents"]
        );

        // 几个目录被配置到同一处时只出现一次（区段里一个目录不该画两遍）。
        settings.set("vault.recordings_dir", "存档");
        settings.set("vault.transcripts_dir", "存档");
        settings.set("vault.chats_dir", "存档");
        settings.set("vault.documents_dir", "存档");
        assert_eq!(material_archive_dirs(&settings), vec!["存档"]);

        // 知识层的两个目录永不入列（否则整棵知识树会被折进资料区段）。
        settings.set("vault.notes_dir", "笔记");
        settings.set("vault.quick_notes_dir", "速记");
        let dirs = material_archive_dirs(&settings);
        assert!(!dirs.contains(&"笔记".to_owned()));
        assert!(!dirs.contains(&"速记".to_owned()));
    }

    #[test]
    fn empty_note_path_uses_stable_backend_generated_stem() {
        let res = empty_note_rel_path(NoteType::Note, "zh-CN").unwrap();
        assert!(res.starts_with("笔记_"));
        assert!(res.ends_with(".md"));
        let res_en = empty_note_rel_path(NoteType::Note, "en-US").unwrap();
        assert!(res_en.starts_with("Note_"));
        assert!(res_en.ends_with(".md"));
    }

    #[test]
    fn whisper_model_path_uses_device_policy_weight_when_assignment_is_empty() {
        let root = temp_settings_path("whisper_model_path")
            .parent()
            .unwrap()
            .join("models");
        let weights = root
            .join(crate::model_store::BUNDLED_WHISPER_ID)
            .join("weights.bin");
        std::fs::create_dir_all(weights.parent().unwrap()).unwrap();
        std::fs::write(&weights, b"fixture").unwrap();
        let settings = Settings::new();

        assert_eq!(
            whisper_model_path_from_settings_and_root(&settings, &root),
            Some(weights)
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn draft_note_path_uses_content_heading_before_fallback_id() {
        assert_eq!(
            draft_note_rel_path("# 周会纪要\n\n正文", NoteType::Note, "zh-CN").unwrap(),
            "周会纪要.md"
        );
        assert_eq!(
            draft_note_rel_path("hello from luna", NoteType::Note, "zh-CN").unwrap(),
            "hello-from-luna.md"
        );
        let res = draft_note_rel_path("   ", NoteType::Note, "zh-CN").unwrap();
        assert!(res.starts_with("笔记_"));
    }

    #[test]
    fn voice_quick_note_stem_carries_time_and_excerpt_for_archiving() {
        use chrono::TimeZone as _;
        let now = chrono::Local
            .with_ymd_and_hms(2026, 7, 12, 14, 30, 0)
            .unwrap();

        // PRD-02-020：默认名 = 本地化前缀 + 日期时间 + 最终输出摘要（时间无 `:`/`.`）。
        let draft = crate::voice_input::VoiceQuickNoteDraft {
            raw: "原始转录".into(),
            final_text: "明天上午十点和 Lena 对一下发布计划，记得带上审批单".into(),
            output_kind: crate::voice_input::VoiceInputOutputKind::Refined,
            audio: None,
        };
        assert_eq!(
            voice_quick_note_stem(&draft, "zh-CN", now),
            "语音速记 2026-07-12 1430 明天上午十点和-Lena-对一下"
        );

        // 英文界面用英文前缀；摘要清洗文件系统保留字符。
        let draft_en = crate::voice_input::VoiceQuickNoteDraft {
            raw: String::new(),
            final_text: "Ship v2: draft/spec to design".into(),
            output_kind: crate::voice_input::VoiceInputOutputKind::Raw,
            audio: None,
        };
        assert_eq!(
            voice_quick_note_stem(&draft_en, "en-US", now),
            "Voice note 2026-07-12 1430 Ship-v2-draft-s"
        );

        // 无可用文本时仅前缀 + 时间（仍可定位归档）。
        let draft_empty = crate::voice_input::VoiceQuickNoteDraft {
            raw: "   ".into(),
            final_text: String::new(),
            output_kind: crate::voice_input::VoiceInputOutputKind::Raw,
            audio: None,
        };
        assert_eq!(
            voice_quick_note_stem(&draft_empty, "zh-CN", now),
            "语音速记 2026-07-12 1430"
        );
    }

    #[test]
    fn text_capture_shares_the_quick_note_naming_convention_with_its_own_prefix() {
        use chrono::TimeZone as _;
        let now = chrono::Local
            .with_ymd_and_hms(2026, 7, 12, 14, 30, 0)
            .unwrap();

        // PRD-06-058 B：文字捕获**沿用**速记的命名惯例（`<前缀> 日期 时间 摘要`），
        // 只换前缀——第二套命名会让同一个收件箱里"按名排序 = 按时间排序"当场失效。
        assert_eq!(
            super::voice::quick_note_stem(
                super::voice::text_capture_prefix("zh-CN"),
                "明天上午十点和 Lena 对一下发布计划",
                now,
            ),
            "速记 2026-07-12 1430 明天上午十点和-Lena-对一下"
        );
        assert_eq!(
            super::voice::quick_note_stem(
                super::voice::text_capture_prefix("en-US"),
                "Ship v2: draft/spec to design",
                now,
            ),
            "Quick note 2026-07-12 1430 Ship-v2-draft-s"
        );
        // 摘要取的是标题行而不是首行原文：以 Markdown 标题 / 引用开头的正文不把 `##`、`>`
        // 带进文件名（真机 2026-10-02：便签「另存为新速记」得到「速记 … ##-便签冲突验证」）。
        // 与知识笔记命名同一个判据（`luna_sync_core::naming::title_line`）。
        assert_eq!(
            super::voice::quick_note_stem(
                super::voice::text_capture_prefix("zh-CN"),
                "## 便签冲突验证\n\n- 第一条",
                now,
            ),
            "速记 2026-07-12 1430 便签冲突验证"
        );
        assert_eq!(
            super::voice::quick_note_stem(
                super::voice::text_capture_prefix("zh-CN"),
                "##\n> 引用的一句话",
                now,
            ),
            "速记 2026-07-12 1430 引用的一句话"
        );
        // 前缀老实说出这条是敲进来的还是说出来的——两者不得混用同一个词。
        assert_ne!(
            super::voice::text_capture_prefix("zh-CN"),
            super::voice::voice_capture_prefix("zh-CN")
        );
        assert_ne!(
            super::voice::text_capture_prefix("en-US"),
            super::voice::voice_capture_prefix("en-US")
        );
    }

    /// PRD-06-058 P0-1：空白画布的落盘形态**跟随目的地**，且判定只在 Rust 一处。
    ///
    /// 这条守的是真机验收撞上的那个缺陷：用户站在「记录」里，最大最显眼的输入区创建出来的
    /// 却是知识笔记——与目的地语义正好相反。反向条款（`assert_ne!`）同样重要：两个目的地
    /// 若判成同一种形态，这条心智就整个塌了，而那正是把判定写在调用点时最容易漂成的样子。
    #[test]
    fn blank_canvas_lands_where_the_destination_says_it_should() {
        use chrono::TimeZone as _;
        let now = chrono::Local
            .with_ymd_and_hms(2026, 7, 12, 14, 30, 0)
            .unwrap();
        let settings = Settings::new();
        let plan = |destination, content: &str| {
            destination_note_plan(None, &settings, "zh-CN", destination, content, now).unwrap()
        };

        // 「记录」目的地：与侧栏捕获框**同一条**路径——速记目录、速记命名惯例、天生记录态。
        let jotted = plan(
            CaptureDestinationDto::Drafts,
            "  明天和 Lena 对一下发布计划  ",
        );
        assert_eq!(
            jotted.rel_path,
            "QuickNotes/速记 2026-07-12 1430 明天和-Lena-对一下发布计划.md"
        );
        assert_eq!(jotted.note_type, NoteType::QuickNote);
        // 随手记的两端空白不是内容。
        assert_eq!(jotted.body, "明天和 Lena 对一下发布计划");

        // 「笔记库」目的地：知识笔记，沿用既有命名惯例（首行/标题派生），正文逐字节原样。
        let written = plan(CaptureDestinationDto::Notes, "# 周会纪要\n\n正文\n");
        assert_eq!(written.rel_path, "Notes/周会纪要.md");
        assert_eq!(written.note_type, NoteType::Note);
        assert_eq!(written.body, "# 周会纪要\n\n正文\n");

        // 反向条款：两个目的地绝不能落成同一种形态。
        let same_text = "同一段字";
        let a = plan(CaptureDestinationDto::Drafts, same_text);
        let b = plan(CaptureDestinationDto::Notes, same_text);
        assert_ne!(a.note_type, b.note_type);
        assert_ne!(a.rel_path, b.rel_path);

        // 空正文两支都拒绝（空文件不是"记了一条"）。
        for destination in [CaptureDestinationDto::Drafts, CaptureDestinationDto::Notes] {
            let err = destination_note_plan(None, &settings, "zh-CN", destination, "  \n ", now)
                .expect_err("空正文必须被拒");
            assert!(err.starts_with("EMPTY_NOTE:"), "{err}");
        }
    }

    /// 目的地判定读的是**用户配置的目录**，不是写死的字符串。
    #[test]
    fn destination_plan_follows_configured_vault_dirs() {
        use chrono::TimeZone as _;
        let now = chrono::Local
            .with_ymd_and_hms(2026, 7, 12, 14, 30, 0)
            .unwrap();
        let mut settings = Settings::new();
        settings.set("vault.quick_notes_dir", "收件箱");
        settings.set("vault.notes_dir", "知识");

        assert!(destination_note_plan(
            None,
            &settings,
            "zh-CN",
            CaptureDestinationDto::Drafts,
            "abc",
            now
        )
        .unwrap()
        .rel_path
        .starts_with("收件箱/"));
        assert!(destination_note_plan(
            None,
            &settings,
            "zh-CN",
            CaptureDestinationDto::Notes,
            "abc",
            now
        )
        .unwrap()
        .rel_path
        .starts_with("知识/"));
    }

    #[test]
    fn voice_quick_note_body_keeps_raw_transcript_when_refined() {
        let body = voice_quick_note_body(
            &crate::voice_input::VoiceQuickNoteDraft {
                raw: "我 这个 今天 要 记录".into(),
                final_text: "我今天要记录。".into(),
                output_kind: crate::voice_input::VoiceInputOutputKind::Refined,
                audio: None,
            },
            "zh-CN",
        );

        assert!(body.contains("## 优化结果"));
        assert!(body.contains("我今天要记录。"));
        assert!(body.contains("## 原始转录"));
        assert!(body.contains("我 这个 今天 要 记录"));
    }

    #[test]
    fn voice_quick_note_body_does_not_duplicate_raw_output() {
        let body = voice_quick_note_body(
            &crate::voice_input::VoiceQuickNoteDraft {
                raw: "plain transcript".into(),
                final_text: "plain transcript".into(),
                output_kind: crate::voice_input::VoiceInputOutputKind::Raw,
                audio: None,
            },
            "en-US",
        );

        assert_eq!(body, "plain transcript");
    }

    #[test]
    fn voice_level_payload_uses_raw_ipc_body() {
        use tauri::ipc::IpcResponse as _;
        match crate::voice_input::voice_level_payload(vec![1, 2, 3])
            .body()
            .unwrap()
        {
            tauri::ipc::InvokeResponseBody::Raw(bytes) => assert_eq!(bytes, vec![1, 2, 3]),
            tauri::ipc::InvokeResponseBody::Json(json) => {
                panic!("voice levels must use raw IPC body, got JSON {json}")
            }
        }
    }

    #[test]
    fn replace_popup_visibility_depends_on_replace_outcome() {
        assert_eq!(
            replace_panel_action_for_kind("injected"),
            ReplacePanelAction::Close
        );
        assert_eq!(
            replace_panel_action_for_kind("copied_fallback"),
            ReplacePanelAction::KeepOpen
        );
    }

    #[test]
    fn settings_value_dto_converts_config_values() {
        assert_eq!(
            SettingValue::from(SettingValueDto::Bool(true)),
            SettingValue::Bool(true)
        );
        assert_eq!(
            SettingValueDto::try_from(SettingValue::Int(7)).unwrap(),
            SettingValueDto::Int(7)
        );
        assert_eq!(
            SettingValueDto::try_from(SettingValue::Str("system".into())).unwrap(),
            SettingValueDto::Str("system".into())
        );
    }

    #[test]
    fn settings_value_dto_rejects_int_overflow() {
        assert!(SettingValueDto::try_from(SettingValue::Int(i64::from(i32::MAX) + 1)).is_err());
        assert!(SettingValueDto::try_from(SettingValue::Int(i64::from(i32::MIN) - 1)).is_err());
    }

    #[test]
    fn settings_spec_dto_includes_current_value_and_stable_names() {
        let mut settings = Settings::new();
        settings.set("appearance.theme", "dark");

        let dto = settings_spec_dtos(&settings)
            .unwrap()
            .into_iter()
            .find(|dto| dto.key == "appearance.theme")
            .unwrap();

        assert_eq!(dto.group, "appearance");
        assert_eq!(dto.scope, "device");
        assert_eq!(dto.default_value, SettingValueDto::Str("system".into()));
        assert_eq!(dto.current_value, SettingValueDto::Str("dark".into()));
    }

    /// `MemoryOnly`（配置目录都拿不到）下写入**照常成功**，只是重启不留存。
    ///
    /// 曾经这里是 `Disabled` + 断言写入失败：那让配置一旦不可写，用户就再也改不了任何
    /// 设置，首次引导连 Vault 都绑不上（2026-07-26 真机）。"存不住"不该升级成"不让改"。
    #[test]
    fn settings_mutation_in_memory_only_still_applies() {
        let settings = Mutex::new(Settings::new());
        let persistence = SettingsPersistence::MemoryOnly;

        assert!(set_setting_value(
            None,
            &settings,
            &persistence,
            "appearance.theme",
            SettingValueDto::Str("dark".into())
        )
        .is_ok());
        assert_eq!(
            settings
                .lock()
                .unwrap()
                .get_str("appearance.theme")
                .as_deref(),
            Some("dark")
        );
    }

    #[test]
    fn settings_mutation_rejects_unknown_and_secret_keys() {
        let settings = Mutex::new(Settings::new());
        let persistence = SettingsPersistence::Writable(temp_settings_path("settings_mutation"));
        assert!(set_setting_value(
            None,
            &settings,
            &persistence,
            "appearance.theme",
            SettingValueDto::Str("dark".into())
        )
        .is_ok());
        assert_eq!(
            settings
                .lock()
                .unwrap()
                .get_str("appearance.theme")
                .as_deref(),
            Some("dark")
        );

        assert!(set_setting_value(
            None,
            &settings,
            &persistence,
            "unknown.key",
            SettingValueDto::Bool(true)
        )
        .is_err());
        assert!(set_setting_value(
            None,
            &settings,
            &persistence,
            "provider.openai_api_key",
            SettingValueDto::Str("sk-leak".into())
        )
        .is_err());
    }

    /// 控件契约校验落在写入路径上：越界/非选项/非法快捷键一律**拒绝写入**，
    /// 且错误串以稳定 code 开头（前端据此取本地化文案，不解析人话）。
    #[test]
    fn settings_mutation_rejects_values_outside_control_contract() {
        let settings = Mutex::new(Settings::new());
        let persistence = SettingsPersistence::Writable(temp_settings_path("settings_contract"));

        let err = set_setting_value(
            None,
            &settings,
            &persistence,
            "caption.max_lines",
            SettingValueDto::Int(99),
        )
        .unwrap_err();
        assert!(err.starts_with("SETTING_RANGE"), "{err}");

        let err = set_setting_value(
            None,
            &settings,
            &persistence,
            "appearance.theme",
            SettingValueDto::Str("banana".into()),
        )
        .unwrap_err();
        assert!(err.starts_with("SETTING_OPTION"), "{err}");

        let err = set_setting_value(
            None,
            &settings,
            &persistence,
            "vault.notes_dir",
            SettingValueDto::Str("/tmp/outside".into()),
        )
        .unwrap_err();
        assert!(err.starts_with("SETTING_PATH"), "{err}");

        // 值全部保持默认（被拒的写入不得留下半个状态）。
        let guard = settings.lock().unwrap();
        assert_eq!(guard.get_int("caption.max_lines"), Some(3));
        assert_eq!(guard.get_str("appearance.theme").as_deref(), Some("system"));
        assert_eq!(guard.get_str("vault.notes_dir").as_deref(), Some("Notes"));
    }

    /// 快捷键内部冲突（PRD-09-003 ①）：撞上另一项已有绑定即拒绝，并告知被谁占用。
    #[test]
    fn settings_mutation_rejects_conflicting_shortcut() {
        let settings = Mutex::new(Settings::with_defaults());
        let persistence = SettingsPersistence::Writable(temp_settings_path("settings_conflict"));

        // 默认 `shortcuts.voice_input` = cmd_or_ctrl+shift+v；把对话键设成同一组合应被拒。
        let err = set_setting_value(
            None,
            &settings,
            &persistence,
            "shortcuts.quick_chat",
            SettingValueDto::Str("cmd_or_ctrl+shift+v".into()),
        )
        .unwrap_err();
        assert_eq!(err, "SETTING_SHORTCUT_CONFLICT: shortcuts.voice_input");

        // 未占用的组合正常写入。
        set_setting_value(
            None,
            &settings,
            &persistence,
            "shortcuts.quick_chat",
            SettingValueDto::Str("cmd_or_ctrl+shift+j".into()),
        )
        .unwrap();
        assert_eq!(
            settings
                .lock()
                .unwrap()
                .get_str("shortcuts.quick_chat")
                .as_deref(),
            Some("cmd_or_ctrl+shift+j")
        );
    }

    /// 整组恢复：一次落盘、只动目标项（PRD-09-052）。
    #[test]
    fn settings_group_reset_restores_only_targeted_keys() {
        let settings = Mutex::new(Settings::new());
        let path = temp_settings_path("settings_group_reset");
        let persistence = SettingsPersistence::Writable(path.clone());

        for (key, value) in [
            ("caption.font_size", SettingValueDto::Str("small".into())),
            ("appearance.theme", SettingValueDto::Str("dark".into())),
        ] {
            set_setting_value(None, &settings, &persistence, key, value).unwrap();
        }
        set_setting_value(
            None,
            &settings,
            &persistence,
            "caption.max_lines",
            SettingValueDto::Int(6),
        )
        .unwrap();

        reset_setting_values(
            &settings,
            &persistence,
            &["caption.font_size", "caption.max_lines"],
        )
        .unwrap();

        let guard = settings.lock().unwrap();
        assert_eq!(guard.get_str("caption.font_size").as_deref(), Some("large"));
        assert_eq!(guard.get_int("caption.max_lines"), Some(3));
        // 别组的改动不受影响。
        assert_eq!(guard.get_str("appearance.theme").as_deref(), Some("dark"));
        drop(guard);

        let toml = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(toml.contains("appearance.theme"));
        assert!(!toml.contains("caption.font_size"));
    }

    #[test]
    fn settings_mutation_applies_vault_scope_without_device_toml_persistence() {
        let settings = Mutex::new(Settings::new());
        let path = temp_settings_path("settings_vault_scope");
        let persistence = SettingsPersistence::Writable(path.clone());

        set_setting_value(
            None,
            &settings,
            &persistence,
            "vault.delete_mode",
            SettingValueDto::Str("permanent".into()),
        )
        .unwrap();
        assert_eq!(
            settings
                .lock()
                .unwrap()
                .get_str("vault.delete_mode")
                .as_deref(),
            Some("permanent")
        );
        let toml = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(!toml.contains("vault.delete_mode"));

        reset_setting_value(None, &settings, &persistence, "vault.delete_mode").unwrap();
        assert_eq!(
            settings
                .lock()
                .unwrap()
                .get_str("vault.delete_mode")
                .as_deref(),
            Some("trash")
        );
    }

    #[test]
    fn settings_mutation_persists_device_settings() {
        let settings = Mutex::new(Settings::new());
        let path = temp_settings_path("settings_persist");
        let persistence = SettingsPersistence::Writable(path.clone());

        set_setting_value(
            None,
            &settings,
            &persistence,
            "appearance.theme",
            SettingValueDto::Str("dark".into()),
        )
        .unwrap();

        let (loaded, outcome) = Settings::load_path(&path);
        assert_eq!(outcome, luna_config::LoadOutcome::Loaded);
        assert_eq!(loaded.get_str("appearance.theme").as_deref(), Some("dark"));
    }

    #[test]
    fn settings_reset_setting_value_restores_registry_default() {
        let settings = Mutex::new(Settings::new());
        let persistence = SettingsPersistence::Writable(temp_settings_path("settings_reset"));
        set_setting_value(
            None,
            &settings,
            &persistence,
            "appearance.theme",
            SettingValueDto::Str("dark".into()),
        )
        .unwrap();

        reset_setting_value(None, &settings, &persistence, "appearance.theme").unwrap();

        assert_eq!(
            settings
                .lock()
                .unwrap()
                .get_str("appearance.theme")
                .as_deref(),
            Some("system")
        );
    }

    #[test]
    fn renamed_entry_path_keeps_parent_and_preserves_extension() {
        assert_eq!(
            renamed_entry_path(None, "projects/old.md", "weekly", false).unwrap(),
            "projects/weekly.md"
        );
        // 用户自己打了后缀就不再重复追加。
        assert_eq!(
            renamed_entry_path(None, "old.md", "weekly.md", false).unwrap(),
            "weekly.md"
        );
    }

    /// 改名不得改变文件类型：资料附件保留自己的扩展名，而不是被补成 `.md`。
    #[test]
    fn renamed_entry_path_never_retypes_assets_or_folders() {
        assert_eq!(
            renamed_entry_path(None, "Recordings/assets/a.wav", "会议录音", false).unwrap(),
            "Recordings/assets/会议录音.wav"
        );
        // 目录没有扩展名——`v1.0` 这种名字不该被当成扩展名为 `0` 的文件。
        assert_eq!(
            renamed_entry_path(None, "Notes/v1.0", "v2.0", true).unwrap(),
            "Notes/v2.0"
        );
    }

    #[test]
    fn renamed_entry_path_rejects_path_separators() {
        assert!(renamed_entry_path(None, "old.md", "../escape", false).is_err());
        assert!(renamed_entry_path(None, "old.md", "nested/name", false).is_err());
        assert!(renamed_entry_path(None, "old.md", "", false).is_err());
    }

    /// 一次批量的**广播必须在锁外、派生刷新只做一次**——这条纪律只有源码看得出来
    /// （运行期不会报错，只会在真机上表现为"拖完之后整个应用不动了"，2026-09-02）。
    ///
    /// 判据：两条批量命令的函数体里，`emit_vault_changes`（整批版）各出现一次，
    /// 且**一次都不出现** `emit_vault_changed(`（单条版）——后者每调一次就唤起一轮
    /// 知识地图刷新与度量基线重算，而它们都要抢 Vault 锁做全库遍历。
    /// 要等 Vault 锁的命令不得留在主线程（TDD-00 §5「命令跑在哪个线程」）：同步一轮、
    /// 知识地图刷新会长时间占锁，主线程上的同步命令一等锁，整个界面就冻住
    /// （2026-10-13 真机：开同步后改名、删除很慢）。
    #[test]
    fn vault_lock_commands_leave_the_main_thread() {
        for (source, command) in [
            (include_str!("vault_files.rs"), "vault_rename"),
            (include_str!("vault_files.rs"), "vault_delete"),
            (include_str!("vault_files.rs"), "list_trash"),
            (include_str!("notes.rs"), "vault_tree"),
        ] {
            let expected =
                format!("#[tauri::command(async)]\n#[specta::specta]\npub(crate) fn {command}(");
            assert!(
                source.contains(&expected),
                "{command} 必须是 #[tauri::command(async)]：主线程等 Vault 锁 = 界面冻住"
            );
        }
    }

    #[test]
    fn batch_commands_broadcast_once_and_outside_the_lock() {
        let code: String = source_without_tests(include_str!("vault_files.rs"))
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .map(|line| line.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
        for command in [
            "pub(crate) fn vault_move_into(",
            "pub(crate) fn vault_delete(",
        ] {
            let body = code
                .split(command)
                .nth(1)
                .unwrap_or_else(|| panic!("找不到 {command}"));
            // 注释已被剥掉，故函数体的终点取下一个函数/结构体的起点。
            let body = body.split("\npub(crate) ").next().unwrap_or(body);
            let body = body.split("\n#[derive").next().unwrap_or(body);
            assert_eq!(
                body.matches("emit_vault_changes(").count(),
                1,
                "{command} 应当整批广播一次（见 emit_vault_changes 的说明）"
            );
            assert!(
                !body.contains("emit_vault_changed("),
                "{command} 又回到逐条广播了：每一条都会唤起一轮全库遍历，并全部排在这次批量的锁后面"
            );
            assert!(
                body.contains("let (root, ") || body.contains("let root = {"),
                "{command} 的锁作用域被摊平了——广播与度量落库必须在锁外"
            );
        }
    }

    /// 批量移动的四条规则（纯函数，故这里逐条钉住）。第四条是 2026-09-02 真机故障的
    /// 直接原因：不同目录下的两篇同名笔记一起拖进同一个文件夹，磁盘预检全部通过，
    /// 而第二项在真正搬的时候撞上"目标已存在"——用户看到的是"四个成功三个"。
    #[test]
    fn plan_moves_rejects_what_the_selection_itself_already_makes_impossible() {
        let never = |_: &str| false;
        // ① 已经在落点目录里的项跳过（不是错误，只是没有可做的事）。
        assert_eq!(
            plan_moves(&["A/a.md".to_owned(), "b.md".to_owned()], "A", never).unwrap(),
            vec![("b.md".to_owned(), "A/b.md".to_owned())]
        );
        // ② 目录被拖进它自己（或子孙）里：整批拒绝。
        assert_eq!(
            plan_moves(&["A".to_owned()], "A/子", never).unwrap_err(),
            MovePlanRejection::IntoItself
        );
        // ③ 落点被磁盘上的同名对象占着：整批拒绝，绝不覆盖。
        assert_eq!(
            plan_moves(&["b.md".to_owned()], "A", |path| path == "A/b.md").unwrap_err(),
            MovePlanRejection::TargetOccupied
        );
        // ④ **批内重名**：两项落到同一个名字上，第二项在动手前就该被拦住。
        assert_eq!(
            plan_moves(
                &["X/会议.md".to_owned(), "Y/会议.md".to_owned()],
                "A",
                never
            )
            .unwrap_err(),
            MovePlanRejection::TargetOccupied
        );
        // ⑤ 子孙先被剪掉，故"目录 + 它里面那篇"只产生一条落点（第二条会落在一个搬走
        // 之后已不存在的旧路径上）。
        assert_eq!(
            plan_moves(&["A/子".to_owned(), "A/子/b.md".to_owned()], "", never).unwrap(),
            vec![("A/子".to_owned(), "子".to_owned())]
        );
    }

    /// 一次多选里同时选中目录与其子孙是常态：子孙必须被剪掉，否则目录搬走/删掉之后
    /// 第二项落在一个已不存在的旧路径上。
    #[test]
    fn prune_selection_drops_descendants_and_duplicates() {
        let picked = vec![
            "A".to_owned(),
            "A/b.md".to_owned(),
            "A".to_owned(),
            "B/c.md".to_owned(),
        ];
        assert_eq!(
            prune_selection(&picked),
            vec!["A".to_owned(), "B/c.md".to_owned()]
        );
        // 前缀判据认的是路径分段，不是字符串前缀：`AB/` 不是 `A` 的子孙。
        let siblings = vec!["A".to_owned(), "AB/c.md".to_owned()];
        assert_eq!(prune_selection(&siblings), siblings);
    }

    #[test]
    fn secondary_vault_changes_never_mutate_active_vault_metrics() {
        assert!(VaultChangeScope::for_window("main").affects_active_vault());
        assert!(VaultChangeScope::for_window("note-editor-3").affects_active_vault());
        assert!(!VaultChangeScope::for_window("vault-window-0").affects_active_vault());
    }

    /// 「在系统文件管理器中显示」要能作用于**目录**（文件夹节点 / Vault 根），
    /// 但越界路径必须照旧拒绝——放宽的只有"必须是文件"这一条。
    #[test]
    fn resolve_vault_entry_allows_dirs_but_still_rejects_escapes() {
        let root = std::env::temp_dir().join(format!("luna_reveal_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/note.md"), "x").unwrap();

        assert!(resolve_vault_entry(&root, "").is_ok(), "Vault 根本身可定位");
        assert!(resolve_vault_entry(&root, "sub").is_ok(), "目录可定位");
        assert!(resolve_vault_entry(&root, "sub/note.md").is_ok());
        assert!(resolve_vault_entry(&root, "../outside").is_err());
        assert!(resolve_vault_entry(&root, "sub/missing.md").is_err());

        let _ = std::fs::remove_dir_all(&root);
    }

    // ── 工作区信息架构重组（ADR-087）的落点不变式 ────────────────────────────
    //
    // **它守的是"产物落到哪里"，而不是页面长什么样**。ADR-087 是**入口与投影重组**，
    // 明说不改 `luna.type` / `luna.stage` / 目录 / 索引协议——而"重组页面时顺手把某类产物
    // 的落点改了"是静默的：用户下一次录音归档到别的目录里，没有任何东西会红。
    //
    // 前端侧的期望清单（一级空间 / 二级分区 / `CREATE_ACTIONS` 唯一注册表 / `activity://`
    // 明确拒绝）在 `features/workspace/workspace-ia.test.ts`、`workspace-deep-links.test.ts`
    // 与 `features/capture/create-actions.test.ts`。此前这里指着一个
    // `workspace-ia-targets.test.ts`——那是迁移期的"目标形态清单"，重组落地后随迁移脚手架
    // 一起退役了，这行指针却留成了断链。

    /// 五类创建动作的产物目的地（PRD-07-069 / TDD-07 §4.2c）——一条用例把五个落点摆在一起。
    ///
    /// 它们此前散落在各自的用例里，于是"五个里少了一个"这件事没有一处看得见。
    #[test]
    fn the_five_create_actions_land_in_their_documented_destinations() {
        use chrono::TimeZone as _;
        let now = chrono::Local
            .with_ymd_and_hms(2026, 8, 25, 9, 0, 0)
            .unwrap();
        let settings = Settings::new();

        // ① 文字速记 / ② 语音速记 → 「速记」（`type: quick-note`，落速记目录）。
        let text_quick_note = destination_note_plan(
            None,
            &settings,
            "zh-CN",
            CaptureDestinationDto::Drafts,
            "随手记一句",
            now,
        )
        .expect("文字速记应能落盘");
        assert_eq!(text_quick_note.note_type, NoteType::QuickNote);
        assert!(
            text_quick_note.rel_path.starts_with(&format!(
                "{}/",
                note_type_dir(&settings, NoteType::QuickNote)
            )),
            "文字速记落点变了：{}",
            text_quick_note.rel_path
        );
        // 语音速记的落盘闭包要真 `AppHandle` + 真 Vault，故这一条钉在**落点判定**那一行上。
        let voice_src = source_without_tests(include_str!("voice.rs"));
        let voice = voice_src
            .split("fn voice_quick_note_saver")
            .nth(1)
            .expect("找不到语音速记落盘闭包")
            .split("\n/// 语音速记默认文件名")
            .next()
            .expect("闭包边界变了");
        assert!(
            voice.contains("note_type_dir(&settings, NoteType::QuickNote)")
                // 判据锚在**两个独立记号**上，不锚连续子串：批次 J′ 给这条通路加了一个
                // 文案来源参数，`cargo fmt` 随即把调用拆成多行——写成一整串参数的判据当场
                // 假警，而它要守的那件事（落到 `create_unique_note`、类型是速记）一步没变。
                && voice.contains("create_unique_note(")
                && voice.contains("NoteType::QuickNote"),
            "语音速记不再落「速记」——两个速记入口的产物必须归同一处（PRD-07-069）"
        );

        // ③ 新建正式笔记 → 「笔记」（`type: note`，落笔记目录）。
        let new_note = destination_note_plan(
            None,
            &settings,
            "zh-CN",
            CaptureDestinationDto::Notes,
            "# 正式笔记\n",
            now,
        )
        .expect("新建笔记应能落盘");
        assert_eq!(new_note.note_type, NoteType::Note);
        assert!(new_note
            .rel_path
            .starts_with(&format!("{}/", note_type_dir(&settings, NoteType::Note))));
        // 空白新建（侧栏「新建笔记」/ 快速开始）缺省同为 `NoteType::Note`。
        let create_note_src = source_without_tests(include_str!("notes.rs"));
        let create_note = create_note_src
            .split("pub(crate) fn create_note(")
            .nth(1)
            .expect("找不到 create_note")
            .split("\npub(crate) fn create_unique_note(")
            .next()
            .expect("create_note 边界变了");
        assert!(
            create_note.contains("note_type.map(Into::into).unwrap_or(NoteType::Note)")
                && create_note.contains("note_type_dir(&state.settings_snapshot(), n_type)"),
            "新建笔记的类型缺省或落点目录变了"
        );

        // ④ 开始录音 → 「录音」（`type: recording`，落录音目录）。
        assert_eq!(note_type_dir(&settings, NoteType::Recording), "Recordings");
        let recording_src = source_without_tests(include_str!("recording.rs"));
        let recording = recording_src
            .split("pub(crate) fn recording_start(")
            .nth(1)
            .expect("找不到 recording_start")
            .split("\n/// 停止录音会话")
            .next()
            .expect("recording_start 边界变了");
        assert!(
            // 去掉空白再比：调用写成一行还是多行（参数一多 rustfmt 就会拆）不改变落点。
            recording
                .split_whitespace()
                .collect::<String>()
                .contains("finalize_recording(&note_rel_clone"),
            "录音归档不再落到调用方预建的归档笔记上"
        );

        // ⑤ 导入音视频 → 「转写」（`type: transcript`，落转写目录**而非**录音目录）。
        assert_eq!(
            note_type_dir(&settings, NoteType::Transcript),
            "Transcripts"
        );
        let import_src = source_without_tests(include_str!("processing.rs"));
        let import = import_src
            .split("pub(crate) fn plan_import_note(")
            .nth(1)
            .expect("找不到 plan_import_note")
            .split("\n    Ok(ImportPlan {")
            .next()
            .expect("plan_import_note 边界变了");
        assert!(
            import.contains("format!(\"{transcripts_dir}/{stem}.md\")")
                && import.contains("NoteType::Transcript"),
            "导入产物不再落转写目录——与用户亲自采集的录音混放会让「录音」分不清来源"
        );

        // 五个落点不得塌成同一个：塌了之后每个入口看起来都还在，只是产物全堆在一处。
        let mut dirs: Vec<String> = [
            NoteType::QuickNote,
            NoteType::Note,
            NoteType::Recording,
            NoteType::Transcript,
        ]
        .into_iter()
        .map(|t| note_type_dir(&settings, t))
        .collect();
        dirs.sort();
        dirs.dedup();
        assert_eq!(
            dirs.len(),
            4,
            "默认配置下四类产物应有四个互不相同的归档目录"
        );
    }

    /// 布局快照**不含**一级导航状态（PRD-07-057 / TDD-06 §1）——重组因此不需要导航状态迁移。
    ///
    /// 反向验证：给 `WorkspaceLayoutDto` 加一个 `active_space` 字段，本用例的键集合断言即红。
    #[test]
    fn workspace_layout_snapshot_carries_no_primary_navigation_state() {
        let layout = WorkspaceLayoutDto {
            panes: vec![WorkspacePaneDto {
                tabs: vec![WorkspaceTabDto {
                    path: "Notes/示例.md".to_owned(),
                    mode: EditorViewModeDto::Live,
                    pinned: false,
                }],
                active_tab: 0,
                history: Vec::new(),
                history_cursor: 0,
            }],
            active_pane: 0,
            sidebar_width: 240,
        };
        let json = serde_json::to_value(&layout).expect("布局快照可序列化");
        let mut keys: Vec<&str> = json
            .as_object()
            .expect("快照是对象")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec!["active_pane", "panes", "sidebar_width"],
            "`.luna/workspace.json` 只存编辑器 pane/tab 布局与主侧栏宽度；\
             一级导航状态是随会话的 UI 态，落进 Vault 相对文件会在换导航枚举时读到不存在的目的地"
        );
        let text = serde_json::to_string(&layout).unwrap();
        for forbidden in ["activeTab", "active_tab_id", "space", "section"] {
            assert!(
                !text.contains(forbidden),
                "布局快照里出现了导航状态字段：{forbidden}"
            );
        }
        // 侧栏**折叠态**同样不入快照（随空间的临时态，PRD-07-057 ④）。
        assert!(!text.contains("collapsed"));
    }

    /// `checkup://` **只在主窗口前端**有分支，不在跨窗口白名单里。
    ///
    /// 这不是缺陷：知识体检报告面板是主窗口内的常驻宿主，辅助窗口没有唤起它的入口。
    /// 写下来是为了让下一次改白名单的人不至于"顺手把它也加进去"而无人察觉。
    #[test]
    fn checkup_route_is_a_main_window_only_branch() {
        assert!(!workspace_route_allowed("checkup://"));
    }

    /// **`activity://` 被明确拒绝，不做静默重定向**（ADR-087 ② / ADR-046）。
    ///
    /// 「动态」聚合页退役后，通知与用量各自回到了自己的宿主。留一条"自动改送到用量页"
    /// 的兼容分支会让一个已经不存在的心智继续在代码里活着，而调用方永远不会发现自己
    /// 用的是旧路由——`ROUTE_REJECTED` 才会让它当场暴露。
    #[test]
    fn retired_activity_route_is_rejected_not_redirected() {
        for route in [
            "activity://",
            "activity://usage",
            "activity://notifications",
        ] {
            assert!(
                !workspace_route_allowed(route),
                "「动态」已退役，这条路由必须被拒绝：{route}"
            );
        }
    }

    /// `DraftInboxDto` 上那**两个数**都是权威计数，不是列表长度（PRD-07-066 / PRD-06-058 A′）。
    ///
    /// 判据本身（`COUNT(*)` 与列表分开、且与清单同一个谓词）由 `luna-vault` 的
    /// `draft_count_is_not_truncated_by_the_list_limit` 与
    /// `draft_filters_narrow_the_list_and_the_matched_count_together` 锁住；这一条锁的是
    /// **IPC 投影**——把任一个改成 `notes.len()` 不会让那两条红，而角标从此在满页时谎报
    /// 一个上限值。
    #[test]
    fn draft_inbox_dto_total_is_the_authoritative_count_not_the_list_length() {
        let projection_src = source_without_tests(include_str!("notes.rs"));
        let projection = projection_src
            .split("pub(crate) fn list_draft_notes(")
            .nth(1)
            .expect("找不到 list_draft_notes")
            .split("\n/// 一次落盘的完整形态")
            .next()
            .expect("list_draft_notes 边界变了");
        assert!(
            projection.contains("let (entries, next_cursor, matched, total) = vault"),
            "两个数不再与列表同一次调用取齐——徽标与列表会因中间的写盘插队而错位"
        );
        assert!(
            projection.contains("total,")
                && projection.contains("matched,")
                && !projection.contains("notes.len()"),
            "`total`（全库）与 `matched`（本次筛选）都必须直接来自 Rust 的权威计数，\
             绝不由被上限截断的列表长度推导"
        );
    }

    /// `ChatHistoryDto.total` 是权威计数，不是这一页的长度（PRD-03-013 / PRD-07-071 ⑦）。
    ///
    /// 与上一条同形：判据本身由 `luna-vault` 的
    /// `chat_history_count_is_not_truncated_and_the_cursor_reaches_the_tail` 锁住，这一条
    /// 锁的是 **IPC 投影**。把它改回 `entries.len()` 不会让那一条红，而侧栏徽标从此在满页
    /// 时停在页大小上——那正是这条命令此前的原形（写死 100 的 top-N）。
    #[test]
    fn chat_history_dto_total_is_the_authoritative_count_not_the_page_length() {
        let projection_src = source_without_tests(include_str!("chat.rs"));
        let projection = projection_src
            .split("pub(crate) fn chat_history_list(")
            .nth(1)
            .expect("找不到 chat_history_list")
            .split("\n/// 载入一条历史会话")
            .next()
            .expect("chat_history_list 边界变了");
        assert!(
            projection.contains("count_notes_of_type(NoteType::Chat)"),
            "总数必须是独立的 COUNT(*)，不能由这一页的长度推导"
        );
        assert!(
            !projection.contains("entries.len()") && !projection.contains(".len() as u32"),
            "`total` 绝不由列表长度推导"
        );
        assert!(
            projection.contains("cursor.as_deref()") && projection.contains("next_cursor"),
            "列表语义是「全部会话」：游标必须收发，否则又回到写死上限的 top-N"
        );
    }

    fn temp_settings_path(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "luna_ipc_settings_test_{}_{}",
            std::process::id(),
            tag
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("config.toml")
    }
}
