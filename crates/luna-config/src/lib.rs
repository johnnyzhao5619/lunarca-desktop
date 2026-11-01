//! `luna-config` — 配置与设置注册表（单一来源）+ 本机密钥保险箱。
//!
//! 事实源：TDD-05 §7（设置注册表 / 分层 / 迁移 / 导入导出）、TDD-12 §2（钥匙串里只有一条主密钥，
//! 其余密钥逐条加密落盘）。
//! Phase 2 落地：[`registry`] 注册表 + [`Settings`] TOML 持久化（带 `schema_version`）+
//! [`SecretStore`] 密钥抽象与 [`SecretVault`] 保险箱。完整的 Vault 级存储 / 导入导出 / CI 一致性校验为后续增量。
#![forbid(unsafe_code)]

pub mod app_lock;
pub mod apps;
pub mod create_action;
pub mod creator_board;
pub mod identity;
pub mod registry;
pub mod secret;
pub mod settings;
pub mod shortcut;
pub mod value;
pub mod vault;
pub mod workspace_space;

pub use create_action::{
    create_action_category, ordered_create_action_ids, CreateAction, CreateActionCategory,
    CREATE_ACTIONS, CREATE_ACTION_IDS, QUICK_START_DEFAULT,
};
pub use registry::{
    desc_presentation, is_simple_setting, normalize, setting_page, setting_page_section,
    setting_surface, spec, validate, ModelSlot, NumberStyle, PathKind, Rejection, SelectStyle,
    SettingControl, SettingDefault, SettingDescPresentation, SettingGroup, SettingPage,
    SettingScope, SettingSpec, SettingSurface, SettingUnit, SettingVisibility, ShortcutScope,
    TextFormat, CURRENT_SCHEMA_VERSION, SETTINGS, TRANSCRIBE_LANG_HINTS, TRANSLATE_TARGET_LANGS,
};
pub use secret::{MemorySecretStore, SecretError, SecretStore, VaultSecretStore};
pub use settings::{
    parse_export_toml, ConfigError, DroppedValue, LoadOutcome, Settings, SettingsImport,
    DEVICE_CONFIG_FILENAME, EXPORT_APP_TAG,
};
pub use value::SettingValue;
pub use vault::{
    KeyringBackend, KeyringCall, MemoryKeyring, SecretVault, SystemKeyring, VaultError,
    MASTER_KEY_ACCOUNT, MASTER_KEY_SERVICE, VAULT_FILE,
};
