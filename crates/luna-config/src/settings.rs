//! 设置存储：TOML 持久化（带 `schema_version` + 迁移链占位）+ 类型化读写。
//! 事实源：TDD-05 §7.3。

use crate::registry::{spec, SettingScope, SettingSpec, CURRENT_SCHEMA_VERSION, SETTINGS};
use crate::value::SettingValue;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// 设备级配置文件名（设备级存储，TDD-05 §7.2 / TDD-12 §11；落 OS 应用配置目录下）。
pub const DEVICE_CONFIG_FILENAME: &str = "config.toml";

/// 配置错误。
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// TOML 解析失败。
    #[error("toml parse failed: {0}")]
    Parse(String),
    /// 文件版本高于当前（降级安装）——不覆盖、加载默认（TDD-05 §7.3）。
    #[error("config version {found} is newer than {current}; refusing to overwrite")]
    FutureVersion {
        /// 文件里的版本。
        found: u32,
        /// 当前版本。
        current: u32,
    },
    /// 注册表内已知设置的持久化值不符合当前 schema。
    #[error("invalid value for setting {key}: {reason}")]
    InvalidValue {
        /// 设置 key。
        key: String,
        /// 失败原因。
        reason: String,
    },
    /// 序列化失败。
    #[error("serialize failed: {0}")]
    Serialize(String),
    /// 导入文件不是当前 Lunarca 导出格式（缺 `app` 标记 / 旧品牌 / 结构不符）。
    #[error("not a lunarca settings export: {0}")]
    NotAnExport(String),
    /// 导入文件的 schema 与当前**不完全相等**（导入只接受同版本，PRD-09-050）。
    #[error("settings export schema {found} does not match current {current}")]
    SchemaMismatch {
        /// 文件里的版本。
        found: u32,
        /// 当前版本。
        current: u32,
    },
    /// 导入文件里有注册表不认识、或不可导入（密钥 / 分配 / 内部状态）的键。
    #[error("setting {key} cannot be imported: {reason}")]
    UnknownKey {
        /// 设置 key。
        key: String,
        /// 原因。
        reason: String,
    },
    /// 文件读写失败。
    #[error("config file io failed: {0}")]
    Io(String),
}

/// 单项被丢弃的记录（逐项容错的产物，供调用方如实告知用户）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DroppedValue {
    /// 设置键，如 `appearance.theme_family`。
    pub key: String,
    /// 人类可读的原因（类型不符 / 不在选项集内 / 越界……）。
    pub reason: String,
}

/// 配置加载结果（让**调用方**决定日志/通知/是否回写，配置层不做 IO 副作用之外的策略，TDD-05 §7.3）。
#[derive(Debug, PartialEq, Eq)]
pub enum LoadOutcome {
    /// 从现有文件成功加载，每一项都合法。
    Loaded,
    /// 文件**结构**完好，但个别项不合法（取值不在选项集内 / 类型不符 / 越界）——
    /// 这些项各自回退默认，**其余项照常生效**。
    ///
    /// 单项过期是产品演进的常态（枚举改名、用户降级、手改 TOML 打错一个字），
    /// 让它作废整份配置等于因为一颗螺丝丢掉整台机器：用户会莫名其妙地发现语言、
    /// 主题、路径、快捷键**全部**回到默认。2026-07-26 真机即栽在此——
    /// `appearance.theme_family` 的取值随主题族改名而过期，用户整份配置被判损坏，
    /// 应用连引导都走不完。
    LoadedWithDropped(Vec<DroppedValue>),
    /// 每一项都合法，但文件里还留着**这一版已不认识的键**（退役的设置项）。运行态不受影响；
    /// 调用方据此决定是否回写——设备文件该回写（不留历史遗留，ADR-046），同步出去的 Vault
    /// 文件不该（另一台装着新版本的设备写下的键，这一台不认识不等于它该消失）。
    LoadedWithRetired(Vec<String>),
    /// 文件不存在 → 返回默认；调用方可 [`Settings::save_path`] 落盘以首次创建。
    CreatedDefault,
    /// **结构性**损坏：TOML 语法错 / schema_version 高于当前 / 读失败。
    /// 此时无法逐项抢救，回退安全默认（带原因，TDD-05 §7.3）。
    Recovered(String),
}

/// 落盘形态（写）。
#[derive(Serialize)]
struct SettingsFile {
    schema_version: u32,
    values: BTreeMap<String, SettingValue>,
}

/// 读盘形态：值先停在 **`toml::Value`** 这一层，逐键再转成 [`SettingValue`]。
///
/// **不能直接反序列化成 `BTreeMap<String, SettingValue>`**：`SettingValue` 是
/// `#[serde(untagged)]`，任何一个值匹配不上全部变体（`quick_start_actions = [1, 2]`、
/// `= [true]`、一张内联表……）都会让 `toml::from_str` **整份失败**——而 TDD-05 §7.3a 的
/// 表格第一行明写着"当前 schema 内的已知项类型不符 → **只丢该项**"。整份失败的代价是
/// `LoadOutcome::Recovered`：用户的语言、主题、路径、快捷键**全部**回到默认，只因为某个
/// 数组里躺着一个数字。2026-07-26 真机就栽在同一类事情上（那次是取值过期）。
///
/// 这一层同时把整份 `Recovered` 收窄回它本来的语义：**TOML 语法级损坏**（文件根本不是
/// 一份 TOML）、schema 版本高于当前、读失败。那三种才是"无法证明它符合当前契约"。
#[derive(Deserialize)]
struct SettingsFileIn {
    schema_version: u32,
    #[serde(default)]
    values: BTreeMap<String, toml::Value>,
}

/// `toml::Value` → [`SettingValue`]（**只认当前 schema 的四种形态**，不做任何转换）。
///
/// **不是宽松解析**：字符串 `"3"` 不会变成整数 3，`1` 不会变成 `true`。认不出来就是
/// 认不出来，交由调用方按"类型不符"丢掉该项——容错保护的是**启动可用性**，不是给旧值
/// 开一条静默的语义转换通路（同 §7.3a"'忽略'绝不等于兼容"）。
fn setting_value_from_toml(value: &toml::Value) -> Option<SettingValue> {
    match value {
        toml::Value::Boolean(b) => Some(SettingValue::Bool(*b)),
        toml::Value::Integer(i) => Some(SettingValue::Int(*i)),
        toml::Value::String(s) => Some(SettingValue::Str(s.clone())),
        toml::Value::Array(items) => items
            .iter()
            .map(|item| item.as_str().map(str::to_owned))
            .collect::<Option<Vec<String>>>()
            .map(SettingValue::StringList),
        _ => None,
    }
}

/// 一套设置（非密钥）。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Settings {
    values: BTreeMap<String, SettingValue>,
}

impl Settings {
    /// 空设置（所有 `get` 回落到注册表默认）。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 用注册表默认填充全部项。
    #[must_use]
    pub fn with_defaults() -> Self {
        let values = SETTINGS
            .iter()
            .map(|s| (s.key.to_owned(), s.default.to_value()))
            .collect();
        Self { values }
    }

    /// 用注册表默认填充设备级项。设备配置文件只允许持久化这些值。
    #[must_use]
    pub fn device_defaults() -> Self {
        let values = SETTINGS
            .iter()
            .filter(|s| s.scope == SettingScope::Device)
            .map(|s| (s.key.to_owned(), s.default.to_value()))
            .collect();
        Self { values }
    }

    /// 取值（缺省回落到注册表默认）。
    #[must_use]
    pub fn get(&self, key: &str) -> Option<SettingValue> {
        self.values
            .get(key)
            .cloned()
            .or_else(|| crate::registry::spec(key).map(|s| s.default.to_value()))
    }

    /// 取布尔。
    #[must_use]
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.get(key).and_then(|v| v.as_bool())
    }

    /// 取整数。
    #[must_use]
    pub fn get_int(&self, key: &str) -> Option<i64> {
        self.get(key).and_then(|v| v.as_int())
    }

    /// 取字符串。
    #[must_use]
    pub fn get_str(&self, key: &str) -> Option<String> {
        self.get(key).and_then(|v| v.as_str().map(str::to_owned))
    }

    /// 写入一个设置。
    pub fn set(&mut self, key: impl Into<String>, value: impl Into<SettingValue>) {
        self.values.insert(key.into(), value.into());
    }

    /// 恢复单项默认：移除显式覆盖，后续 [`Self::get`] 回落注册表默认。
    pub fn reset(&mut self, key: &str) {
        self.values.remove(key);
    }

    /// 应用更新策略（`update.policy`，取值见 [`crate::registry::UPDATE_POLICIES`]）。缺省 = 仅手动
    /// （检查更新是 opt-in 联网，TDD-12 §1）。
    #[must_use]
    pub fn update_policy(&self) -> String {
        self.get_str("update.policy")
            .unwrap_or_else(|| "manual".to_owned())
    }

    /// 「启动时检查更新」问到哪一步了（`update.startup_check_prompt`）：空 = 没问、`notified` / `answered`。
    #[must_use]
    pub fn update_startup_check_prompt(&self) -> String {
        self.get_str("update.startup_check_prompt")
            .unwrap_or_default()
    }

    /// 严格本地模式（隐私护栏开关，TDD-12 §1）。
    #[must_use]
    pub fn strict_local(&self) -> bool {
        self.get_bool("privacy.strict_local").unwrap_or(false)
    }

    /// 序列化为设备级 TOML（**仅 Device scope**，带当前 `schema_version`）。
    ///
    /// # Errors
    /// 序列化失败时返回 [`ConfigError::Serialize`]。
    pub fn to_toml(&self) -> Result<String, ConfigError> {
        self.to_toml_for_scope(SettingScope::Device)
    }

    /// 序列化指定 scope 的 TOML（带当前 `schema_version`）。
    ///
    /// # Errors
    /// 序列化失败时返回 [`ConfigError::Serialize`]。
    pub fn to_toml_for_scope(&self, scope: SettingScope) -> Result<String, ConfigError> {
        let values = self
            .values
            .iter()
            .filter_map(|(k, v)| {
                let spec = spec(k)?;
                if spec.scope != scope || validate_persisted_value(spec, v).is_err() {
                    return None;
                }
                Some((k.clone(), v.clone()))
            })
            .collect();
        let file = SettingsFile {
            schema_version: CURRENT_SCHEMA_VERSION,
            values,
        };
        toml::to_string_pretty(&file).map_err(|e| ConfigError::Serialize(e.to_string()))
    }

    /// 将另一个 Settings 中指定 scope 的值合并进当前设置。
    ///
    /// **只覆盖 `other` 里写过的键**——`other` 由配置文件解析而来故是稀疏的。要表达「换源」
    /// （切库：新库的配置**取代**上一个库的）须先调 [`Self::reset_scope_to_defaults`]。
    pub fn merge_scope_from(&mut self, other: &Self, scope: SettingScope) {
        for (key, value) in &other.values {
            if spec(key).is_some_and(|setting| setting.scope == scope) {
                self.values.insert(key.clone(), value.clone());
            }
        }
    }

    /// 把某作用域的全部键**重置为注册表默认**。
    ///
    /// 切库换源的前半步（ADR-025）：配置文件是稀疏的，只 merge 会让新库没写的键残留上一个
    /// 库的取值——归档目录、蒸馏三档指着另一个库的配置，是静默的跨库泄漏。
    pub fn reset_scope_to_defaults(&mut self, scope: SettingScope) {
        for setting in SETTINGS.iter() {
            if setting.scope == scope {
                self.values
                    .insert(setting.key.to_owned(), setting.default.to_value());
            }
        }
    }

    /// 从 TOML 解析（带版本检查 + 迁移占位）。
    ///
    /// # Errors
    /// 解析失败 / 版本高于当前时返回错误（调用方据此回落默认，不覆盖用户文件）。
    pub fn from_toml(s: &str) -> Result<Self, ConfigError> {
        Self::from_toml_with_scope(s, None)
    }

    /// **严格**解析：任何单项不合法即整体 `Err`。
    ///
    /// 与 [`Self::load_path`] 的逐项容错刻意分家——两条路径服务两种场景：
    /// 严格版用于"这份内容必须完全合法"（校验工具、单测）；加载已落盘文件走容错版，
    /// 因为那里的坏项来自版本演进而非用户此刻的输入，不该让它作废整份配置。
    fn from_toml_with_scope(s: &str, scope: Option<SettingScope>) -> Result<Self, ConfigError> {
        let (settings, dropped, _retired) = Self::parse_with_scope(s, scope)?;
        if let Some(first) = dropped.into_iter().next() {
            return Err(ConfigError::InvalidValue {
                key: first.key,
                reason: first.reason,
            });
        }
        Ok(settings)
    }

    /// 解析 + **逐项容错**：结构性问题才整份失败，单项不合法只丢该项并如实报告。
    ///
    /// 未知的 key 本就是跳过的（旧版本残留 / 未来版本新增，见下面的 `continue`）；
    /// 既然未知键能容错，已知键的**过期取值**没有理由更严厉——两者都是"这一项我不认识"。
    fn parse_with_scope(
        s: &str,
        scope: Option<SettingScope>,
    ) -> Result<(Self, Vec<DroppedValue>, Vec<String>), ConfigError> {
        let file: SettingsFileIn =
            toml::from_str(s).map_err(|e| ConfigError::Parse(e.to_string()))?;
        if file.schema_version > CURRENT_SCHEMA_VERSION {
            return Err(ConfigError::FutureVersion {
                found: file.schema_version,
                current: CURRENT_SCHEMA_VERSION,
            });
        }
        // 旧版本迁移：Phase 2 无结构变更，原样接受（迁移链占位，TDD-05 §7.3）。
        let mut values = BTreeMap::new();
        let mut dropped = Vec::new();
        let mut retired = Vec::new();
        for (key, raw) in file.values {
            let Some(spec) = spec(&key) else {
                retired.push(key);
                continue;
            };
            if scope.is_some_and(|scope| spec.scope != scope) {
                continue;
            }
            // 逐键转形：这一步认不出来就是"类型不符"，与下面 `validate` 的越界/非法取值
            // 走**同一条丢弃通路**——两者对用户是同一件事（这一项我不认识，回默认）。
            let Some(value) = setting_value_from_toml(&raw) else {
                dropped.push(DroppedValue {
                    key: key.clone(),
                    reason: format!(
                        "expected {}, got {}",
                        spec.default.to_value().kind(),
                        raw.type_str()
                    ),
                });
                continue;
            };
            // **归一化先于校验，且写入侧与加载侧同一口径**。
            //
            // 配置文件不是只有本应用写：用户会手改、部署方会用 MDM 下发、排障时会直接
            // 编辑。这些入口写进来的 `10.0.0.62:8090` 与用户在输入框里敲的完全一样，
            // 凭什么一个被补全、另一个被判非法丢弃？丢弃的后果还特别隐蔽——组织服务端
            // 地址无声变空，用户只看到"AI 又跑回这台设备上了"。
            // 两侧同一口径不是兼容层，是**校验点只有一个**这条纪律的必然结果。
            let value = crate::registry::normalize(spec, value);
            if let Err(e) = validate_persisted_value(spec, &value) {
                dropped.push(DroppedValue {
                    key: key.clone(),
                    reason: e.to_string(),
                });
                continue; // 该项取注册表默认，其余项照常
            }
            values.insert(key, value);
        }
        Ok((Self { values }, dropped, retired))
    }

    /// 从设备级配置文件加载，带**损坏回退**（TDD-05 §7.3，**永不 panic、永远可用**）。
    ///
    /// 语义（[`LoadOutcome`]）：文件缺失 → 默认 + `CreatedDefault`（调用方可回写创建）；
    /// 解析失败 / 未知版本 / 读失败 → 默认 + `Recovered`（**调用方不得回写**，避免覆盖
    /// 损坏但或可人工挽救的用户文件）。绝不静默丢配置。
    #[must_use]
    pub fn load_path(path: &Path) -> (Self, LoadOutcome) {
        Self::load_path_for_scope(path, SettingScope::Device)
    }

    /// 从指定 scope 的配置文件加载，带**损坏回退**。
    #[must_use]
    pub fn load_path_for_scope(path: &Path, scope: SettingScope) -> (Self, LoadOutcome) {
        match std::fs::read_to_string(path) {
            Ok(content) => match Self::parse_with_scope(&content, Some(scope)) {
                Ok((s, dropped, retired)) if dropped.is_empty() && retired.is_empty() => {
                    (s, LoadOutcome::Loaded)
                }
                // 过期取值优先报：那一档本就回写，退役键随之一并消失。
                Ok((s, dropped, _)) if !dropped.is_empty() => {
                    (s, LoadOutcome::LoadedWithDropped(dropped))
                }
                Ok((s, _, retired)) => (s, LoadOutcome::LoadedWithRetired(retired)),
                Err(e) => (
                    Self::device_defaults(),
                    LoadOutcome::Recovered(e.to_string()),
                ),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                (Self::device_defaults(), LoadOutcome::CreatedDefault)
            }
            Err(e) => (
                Self::device_defaults(),
                LoadOutcome::Recovered(e.to_string()),
            ),
        }
    }

    /// 写入设备级配置文件（自动创建父目录）。**密钥项天然排除**（[`Self::to_toml`]）。
    ///
    /// ⚠ 调用方在 [`LoadOutcome::Recovered`] 时**不应**调用本方法（不覆盖损坏原文件，TDD-05 §7.3）。
    ///
    /// # Errors
    /// 序列化失败 / 建目录或写文件失败时返回 [`ConfigError`]。
    pub fn save_path(&self, path: &Path) -> Result<(), ConfigError> {
        self.save_path_for_scope(path, SettingScope::Device)
    }

    /// 写入指定 scope 的配置文件（自动创建父目录）。
    ///
    /// # Errors
    /// 序列化失败 / 建目录或写文件失败时返回 [`ConfigError`]。
    pub fn save_path_for_scope(&self, path: &Path, scope: SettingScope) -> Result<(), ConfigError> {
        let toml = self.to_toml_for_scope(scope)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ConfigError::Io(e.to_string()))?;
        }
        std::fs::write(path, toml).map_err(|e| ConfigError::Io(e.to_string()))
    }
}

/// 持久化值校验：类型 + 控件契约（选项/范围/语法）**同一口径**（[`crate::registry::validate`]）。
///
/// 配置文件里的越界值与 UI 写入的越界值不该有两套标准——文件里的 `caption.max_lines = -3`
/// 同样会一路流到消费点。不合法即**只丢该项**（`LoadOutcome::LoadedWithDropped`，回退
/// 注册表默认，其余合法项照常生效），与 TDD-05 §7.3a 的表格逐行一致；整份回退
/// （`LoadOutcome::Recovered`）只留给 TOML 语法级损坏、schema 版本过高与读失败。
fn validate_persisted_value(spec: &SettingSpec, value: &SettingValue) -> Result<(), ConfigError> {
    let default = spec.default.to_value();
    if !value.same_type_as(&default) {
        return Err(ConfigError::InvalidValue {
            key: spec.key.to_string(),
            reason: format!("expected {}, got {}", default.kind(), value.kind()),
        });
    }
    if !value.fits_i32() {
        return Err(ConfigError::InvalidValue {
            key: spec.key.to_string(),
            reason: "integer outside supported settings IPC range".to_string(),
        });
    }
    crate::registry::validate(spec, value).map_err(|rejection| ConfigError::InvalidValue {
        key: spec.key.to_string(),
        reason: rejection.reason(),
    })
}

// ─── 导入 / 导出（PRD-09-050，TDD-05 §7.4）──────────────────────────────────────

/// 导出文件里的应用标记：旧品牌 / 他人格式的文件一律不认（ADR-046 不留兼容读取）。
pub const EXPORT_APP_TAG: &str = "lunarca";

/// 导出文件（写）：`schema_version` + `app` + 按 scope 分节的值。
#[derive(Serialize)]
struct ExportFile {
    schema_version: u32,
    app: &'static str,
    device: BTreeMap<String, SettingValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    vault: Option<BTreeMap<String, SettingValue>>,
}

/// 导出文件（读）：值先停在 `toml::Value`，逐键严格转形。
#[derive(Deserialize)]
struct ExportFileIn {
    schema_version: u32,
    app: String,
    #[serde(default)]
    device: BTreeMap<String, toml::Value>,
    #[serde(default)]
    vault: Option<BTreeMap<String, toml::Value>>,
}

/// 通过全量校验的导入内容：按 scope 分好、已归一化、逐项合法。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SettingsImport {
    /// 设备级键值。
    pub device: Vec<(String, SettingValue)>,
    /// Vault 级键值（文件里带 `[vault]` 节才有）。
    pub vault: Option<Vec<(String, SettingValue)>>,
}

/// 某键能不能进导出文件：模型分配（有自己的校验与热切换写入口）、
/// 功能界面自管的内部状态、设置页显示投影（TDD-05 §7.4「显示过滤不参与导入/导出」）
/// 都不进。导出与导入用**同一个**判据，两边永远对得上。
fn export_rejection(spec: &SettingSpec) -> Option<&'static str> {
    if spec.key.starts_with("ai.assignment.") {
        Some("model assignments are validated against installed models and are not portable")
    } else if crate::registry::is_internal(spec.key) {
        Some("internal state is not portable")
    } else if crate::registry::is_settings_projection_control(spec.key) {
        Some("display projection is not part of settings data")
    } else {
        None
    }
}

impl Settings {
    /// 某 scope 里可导出的键值（同 [`Self::to_toml_for_scope`] 的过滤 + 导出专属排除）。
    fn exportable_values(&self, scope: SettingScope) -> BTreeMap<String, SettingValue> {
        self.values
            .iter()
            .filter_map(|(k, v)| {
                let spec = spec(k)?;
                if spec.scope != scope
                    || export_rejection(spec).is_some()
                    || validate_persisted_value(spec, v).is_err()
                {
                    return None;
                }
                Some((k.clone(), v.clone()))
            })
            .collect()
    }

    /// 导出为当前格式 TOML：设备级必带；`include_vault` 时附带当前 Vault 级的值。
    ///
    /// # Errors
    /// 序列化失败时返回 [`ConfigError::Serialize`]。
    pub fn export_toml(&self, include_vault: bool) -> Result<String, ConfigError> {
        let file = ExportFile {
            schema_version: CURRENT_SCHEMA_VERSION,
            app: EXPORT_APP_TAG,
            device: self.exportable_values(SettingScope::Device),
            vault: include_vault.then(|| self.exportable_values(SettingScope::Vault)),
        };
        toml::to_string_pretty(&file).map_err(|e| ConfigError::Serialize(e.to_string()))
    }
}

/// 严格解析一份导出文件：schema 必须**完全相等**、`app` 标记必须是本应用、每个键都
/// 必须是注册表已知且可导入、落在正确的 scope 节、取值经归一化后合法。任一条不满足
/// 即整体拒绝——导入是原子的，不存在"导入了一半"（PRD-09-050）。
///
/// # Errors
/// 见 [`ConfigError`] 各变体。
pub fn parse_export_toml(s: &str) -> Result<SettingsImport, ConfigError> {
    let file: ExportFileIn = toml::from_str(s).map_err(|e| ConfigError::Parse(e.to_string()))?;
    if file.app != EXPORT_APP_TAG {
        return Err(ConfigError::NotAnExport(format!(
            "app tag is {:?}, expected {EXPORT_APP_TAG:?}",
            file.app
        )));
    }
    if file.schema_version != CURRENT_SCHEMA_VERSION {
        return Err(ConfigError::SchemaMismatch {
            found: file.schema_version,
            current: CURRENT_SCHEMA_VERSION,
        });
    }
    let device = import_section(file.device, SettingScope::Device)?;
    let vault = file
        .vault
        .map(|section| import_section(section, SettingScope::Vault))
        .transpose()?;
    Ok(SettingsImport { device, vault })
}

fn import_section(
    section: BTreeMap<String, toml::Value>,
    scope: SettingScope,
) -> Result<Vec<(String, SettingValue)>, ConfigError> {
    let mut out = Vec::with_capacity(section.len());
    for (key, raw) in section {
        let Some(spec) = spec(&key) else {
            return Err(ConfigError::UnknownKey {
                key,
                reason: "not a setting of this version".to_owned(),
            });
        };
        if let Some(reason) = export_rejection(spec) {
            return Err(ConfigError::UnknownKey {
                key,
                reason: reason.to_owned(),
            });
        }
        if spec.scope != scope {
            return Err(ConfigError::UnknownKey {
                key,
                reason: format!(
                    "belongs to the {} section, found under {}",
                    spec.scope.as_str(),
                    scope.as_str()
                ),
            });
        }
        let Some(value) = setting_value_from_toml(&raw) else {
            return Err(ConfigError::InvalidValue {
                key,
                reason: format!(
                    "expected {}, got {}",
                    spec.default.to_value().kind(),
                    raw.type_str()
                ),
            });
        };
        let value = crate::normalize(spec, value);
        crate::validate(spec, &value).map_err(|rejection| ConfigError::InvalidValue {
            key: key.clone(),
            reason: format!("{} {}", rejection.code(), rejection.reason()),
        })?;
        out.push((key, value));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_round_trips_and_excludes_assignments_and_internal_state() {
        let mut s = Settings::with_defaults();
        s.set("appearance.theme", "dark");
        s.set("ai.assignment.default_llm", "qwen3:4b");
        s.set("vault.delete_mode", "permanent");
        let toml = s.export_toml(true).unwrap();
        assert!(toml.contains("app = \"lunarca\""));
        assert!(toml.contains("[device]"));
        assert!(toml.contains("[vault]"));
        assert!(!toml.contains("ai.assignment"));
        let parsed = parse_export_toml(&toml).unwrap();
        assert!(parsed
            .device
            .iter()
            .any(|(k, v)| k == "appearance.theme" && v.as_str() == Some("dark")));
        assert!(parsed
            .vault
            .as_ref()
            .unwrap()
            .iter()
            .any(|(k, v)| k == "vault.delete_mode" && v.as_str() == Some("permanent")));
        // 不带 Vault：文件里没有 `[vault]` 节，解析回来也没有。
        let device_only = Settings::with_defaults().export_toml(false).unwrap();
        assert!(!device_only.contains("[vault]"));
        assert!(parse_export_toml(&device_only).unwrap().vault.is_none());
    }

    #[test]
    fn import_rejects_wrong_app_version_unknown_keys_and_bad_values() {
        let current = CURRENT_SCHEMA_VERSION;
        let wrong_app = format!("schema_version = {current}\napp = \"other\"\n[device]\n");
        assert!(matches!(
            parse_export_toml(&wrong_app),
            Err(ConfigError::NotAnExport(_))
        ));
        let wrong_version = format!(
            "schema_version = {}\napp = \"lunarca\"\n[device]\n",
            current + 1
        );
        assert!(matches!(
            parse_export_toml(&wrong_version),
            Err(ConfigError::SchemaMismatch { .. })
        ));
        let unknown =
            format!("schema_version = {current}\napp = \"lunarca\"\n[device]\n\"nope.key\" = 1\n");
        assert!(matches!(
            parse_export_toml(&unknown),
            Err(ConfigError::UnknownKey { .. })
        ));
        let wrong_scope = format!(
            "schema_version = {current}\napp = \"lunarca\"\n[device]\n\"vault.delete_mode\" = \"permanent\"\n"
        );
        assert!(matches!(
            parse_export_toml(&wrong_scope),
            Err(ConfigError::UnknownKey { .. })
        ));
        let bad_value = format!("schema_version = {current}\napp = \"lunarca\"\n[device]\n\"appearance.theme\" = \"neon\"\n");
        assert!(matches!(
            parse_export_toml(&bad_value),
            Err(ConfigError::InvalidValue { .. })
        ));
        let bad_type = format!(
            "schema_version = {current}\napp = \"lunarca\"\n[device]\n\"appearance.theme\" = 3\n"
        );
        assert!(matches!(
            parse_export_toml(&bad_type),
            Err(ConfigError::InvalidValue { .. })
        ));
    }

    #[test]
    fn defaults_and_fallback() {
        let s = Settings::new();
        // 空 Settings 也能回落到注册表默认。
        assert!(!s.strict_local());
        assert_eq!(s.get_str("ai.assignment.default_llm").as_deref(), Some(""));
    }

    #[test]
    fn with_defaults_excludes_retired_keys() {
        let s = Settings::with_defaults();
        let toml = s.to_toml().unwrap();
        assert!(toml.contains("schema_version = 1"));
        assert!(toml.contains("privacy.strict_local"));
        assert!(!toml.contains("provider.default_llm_model"));
    }

    #[test]
    fn device_toml_excludes_vault_scope_defaults() {
        let s = Settings::with_defaults();
        let toml = s.to_toml().unwrap();

        assert!(toml.contains("appearance.theme"));
        assert!(!toml.contains("vault.templates_dir"));
        assert!(!toml.contains("vault.delete_mode"));
        assert!(!toml.contains("vault.snapshot_max_versions"));
        assert!(!toml.contains("vault.ai_artifacts_dir"));
    }

    #[test]
    fn device_defaults_only_materialize_device_scope_values() {
        let s = Settings::device_defaults();
        let toml = s.to_toml().unwrap();

        assert!(toml.contains("appearance.theme"));
        assert!(!toml.contains("vault.templates_dir"));
        assert!(!toml.contains("vault.delete_mode"));
    }

    #[test]
    fn set_does_not_persist_retired_keys() {
        let mut s = Settings::new();
        s.set("provider.default_llm_model", "legacy-model");
        assert!(!s.to_toml().unwrap().contains("provider.default_llm_model"));
    }

    #[test]
    fn toml_round_trip() {
        let mut s = Settings::new();
        s.set("privacy.strict_local", true);
        s.set("popup_translate.native_lang", "en");
        let toml = s.to_toml().unwrap();
        let back = Settings::from_toml(&toml).unwrap();
        assert!(back.strict_local());
        assert_eq!(
            back.get_str("popup_translate.native_lang").as_deref(),
            Some("en")
        );
    }

    #[test]
    fn from_toml_filters_unknown_and_retired_keys() {
        let toml = r#"
schema_version = 1

[values]
"appearance.theme" = "dark"
"unknown.key" = true
"provider.default_llm_model" = "legacy-model"
"#;

        let settings = Settings::from_toml(toml).unwrap();
        let serialized = settings.to_toml().unwrap();

        assert_eq!(
            settings.get_str("appearance.theme").as_deref(),
            Some("dark")
        );
        assert_eq!(settings.get("unknown.key"), None);
        assert!(!serialized.contains("unknown.key"));
        assert!(!serialized.contains("provider.default_llm_model"));
        assert!(!serialized.contains("legacy-model"));
    }

    #[test]
    fn from_toml_rejects_wrong_type_for_registered_key() {
        let toml = r#"
schema_version = 1

[values]
"appearance.theme" = true
"#;

        assert!(matches!(
            Settings::from_toml(toml),
            Err(ConfigError::InvalidValue { key, .. }) if key == "appearance.theme"
        ));
    }

    #[test]
    fn from_toml_rejects_ipc_unrepresentable_int() {
        let toml = r#"
schema_version = 1

[values]
"vault.snapshot_max_versions" = 2147483648
"#;

        assert!(matches!(
            Settings::from_toml(toml),
            Err(ConfigError::InvalidValue { key, .. }) if key == "vault.snapshot_max_versions"
        ));
    }

    /// 类型不符的单项：只丢该项，且 `load_path` 自身**绝不写文件**。
    ///
    /// 本测试曾断言"整份 Recovered"——那正是 2026-07-26 真机事故的契约。改为逐项容错后，
    /// 它仍守着另一半纪律：加载是纯读操作，是否回写由调用方决定（TDD-05 §7.3）。
    #[test]
    fn invalid_registered_value_drops_only_that_key_and_load_never_writes() {
        let dir = temp_dir("invalid_registered");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let original =
            "schema_version = 1\n[values]\n\"appearance.theme\" = true\n\"appearance.language\" = \"en-US\"\n";
        std::fs::write(&path, original).unwrap();

        let (settings, outcome) = Settings::load_path(&path);

        let LoadOutcome::LoadedWithDropped(dropped) = outcome else {
            panic!("类型不符应只丢该项：{outcome:?}");
        };
        assert_eq!(dropped.len(), 1);
        assert_eq!(dropped[0].key, "appearance.theme");
        assert_eq!(
            settings.get_str("appearance.theme").as_deref(),
            Some("system")
        );
        assert_eq!(
            settings.get_str("appearance.language").as_deref(),
            Some("en-US")
        );
        // 加载是纯读：原文件一字未动。
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reset_removes_override_and_uses_registry_default() {
        let mut s = Settings::new();
        s.set("appearance.theme", "dark");
        assert_eq!(s.get_str("appearance.theme").as_deref(), Some("dark"));

        s.reset("appearance.theme");

        assert_eq!(s.get_str("appearance.theme").as_deref(), Some("system"));
    }

    #[test]
    fn future_version_rejected() {
        let toml = "schema_version = 999\n[values]\n";
        assert!(matches!(
            Settings::from_toml(toml),
            Err(ConfigError::FutureVersion { found: 999, .. })
        ));
    }

    /// 每个测试独享一个临时目录（用 pid + 行号去重，避免并行串扰），结束尽力清理。
    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("luna_cfg_test_{}_{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn save_then_load_round_trip_via_file() {
        let path = temp_dir("rt").join("config.toml");
        let mut s = Settings::new();
        s.set("privacy.strict_local", true);
        s.set("popup_translate.native_lang", "en");
        s.save_path(&path).unwrap(); // 自动建父目录

        let (back, outcome) = Settings::load_path(&path);
        assert_eq!(outcome, LoadOutcome::Loaded);
        assert!(back.strict_local());
        assert_eq!(
            back.get_str("popup_translate.native_lang").as_deref(),
            Some("en")
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn save_path_writes_device_scope_only() {
        let path = temp_dir("device_only").join("config.toml");
        let mut s = Settings::with_defaults();
        s.set("vault.delete_mode", "permanent");
        s.set("vault.notes_dir", "Leaked Notes");

        s.save_path(&path).unwrap();

        let toml = std::fs::read_to_string(&path).unwrap();
        assert!(toml.contains("appearance.theme"));
        assert!(!toml.contains("vault.delete_mode"));
        assert!(!toml.contains("permanent"));
        assert!(!toml.contains("vault.notes_dir"));
        assert!(!toml.contains("Leaked Notes"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn save_path_for_scope_writes_vault_scope_only() {
        let path = temp_dir("vault_only").join("settings.toml");
        let mut s = Settings::with_defaults();
        s.set("vault.delete_mode", "permanent");
        s.set("vault.ai_artifacts_dir", "Artifacts");
        s.set("appearance.theme", "dark");

        s.save_path_for_scope(&path, SettingScope::Vault).unwrap();

        let toml = std::fs::read_to_string(&path).unwrap();
        assert!(toml.contains("vault.delete_mode"));
        assert!(toml.contains("vault.ai_artifacts_dir"));
        assert!(!toml.contains("appearance.theme"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn merge_scope_from_only_applies_requested_scope() {
        let mut base = Settings::new();
        let mut other = Settings::new();
        other.set("vault.delete_mode", "permanent");
        other.set("appearance.theme", "dark");

        base.merge_scope_from(&other, SettingScope::Vault);

        assert_eq!(
            base.get_str("vault.delete_mode").as_deref(),
            Some("permanent")
        );
        assert_eq!(base.get_str("appearance.theme").as_deref(), Some("system"));
    }

    #[test]
    fn load_path_ignores_vault_scope_values_in_device_file() {
        let dir = temp_dir("load_device_scope");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            "schema_version = 1\n[values]\n\"appearance.theme\" = \"dark\"\n\"vault.delete_mode\" = \"permanent\"\n",
        )
        .unwrap();

        let (settings, outcome) = Settings::load_path(&path);

        assert_eq!(outcome, LoadOutcome::Loaded);
        assert_eq!(
            settings.get_str("appearance.theme").as_deref(),
            Some("dark")
        );
        assert_eq!(
            settings.get_str("vault.delete_mode").as_deref(),
            Some("trash")
        );
        assert!(!settings.to_toml().unwrap().contains("vault.delete_mode"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_path_reports_retired_keys_so_the_caller_can_drop_them() {
        // 统一登录（ADR-155）删掉的三个云端键：值本身合法，只是这一版已不认识它们。
        let dir = temp_dir("load_retired");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            "schema_version = 1\n[values]\n\"appearance.theme\" = \"dark\"\n\"cloud.api_base\" = \"\"\n\"cloud.lease_pubkey\" = \"ab\"\n",
        )
        .unwrap();

        let (settings, outcome) = Settings::load_path(&path);

        assert_eq!(
            outcome,
            LoadOutcome::LoadedWithRetired(vec![
                "cloud.api_base".to_owned(),
                "cloud.lease_pubkey".to_owned()
            ])
        );
        assert_eq!(
            settings.get_str("appearance.theme").as_deref(),
            Some("dark")
        );
        settings.save_path(&path).unwrap();
        let rewritten = std::fs::read_to_string(&path).unwrap();
        assert!(!rewritten.contains("cloud."), "{rewritten}");
        assert_eq!(Settings::load_path(&path).1, LoadOutcome::Loaded);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_yields_defaults_and_created_outcome() {
        let path = temp_dir("missing").join("config.toml");
        let (s, outcome) = Settings::load_path(&path);
        assert_eq!(outcome, LoadOutcome::CreatedDefault);
        // 默认仍可用（回落注册表默认）。
        assert!(!s.strict_local());
        assert!(!path.exists(), "load 不应创建文件（回写交调用方决定）");
    }

    #[test]
    fn corrupt_file_recovers_to_defaults_without_overwriting() {
        let dir = temp_dir("corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "this is not valid toml ::: {{{").unwrap();

        let (s, outcome) = Settings::load_path(&path);
        assert!(matches!(outcome, LoadOutcome::Recovered(_)));
        assert!(!s.strict_local()); // 安全默认
                                    // 原（损坏）文件未被触碰——内容仍是垃圾，load 绝不覆盖（§7.3）。
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "this is not valid toml ::: {{{"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 2026-07-26 真机事故的最小复现：一个枚举取值随版本演进而过期，
    /// 曾导致**整份配置**被判损坏、全部设置回默认、应用进入不可配置状态。
    #[test]
    fn stale_enum_value_drops_only_that_key() {
        let dir = temp_dir("stale_enum");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            "schema_version = 1\n[values]\n\
             \"appearance.density\" = \"a-density-that-no-longer-exists\"\n\
             \"appearance.language\" = \"zh-CN\"\n\
             \"appearance.theme\" = \"light\"\n",
        )
        .unwrap();

        let (settings, outcome) = Settings::load_path(&path);
        let LoadOutcome::LoadedWithDropped(dropped) = outcome else {
            panic!("单项过期必须逐项容错，不得整份作废：{outcome:?}");
        };
        assert_eq!(dropped.len(), 1);
        assert_eq!(dropped[0].key, "appearance.density");
        // 过期项回退注册表默认……
        assert_eq!(
            settings.get_str("appearance.density").as_deref(),
            Some("compact")
        );
        // ……其余项**照常生效**（真机上正是这些一起丢了，界面连语言都变了）。
        assert_eq!(
            settings.get_str("appearance.language").as_deref(),
            Some("zh-CN")
        );
        assert_eq!(
            settings.get_str("appearance.theme").as_deref(),
            Some("light")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn out_of_range_number_drops_only_that_key() {
        let dir = temp_dir("stale_range");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            "schema_version = 1\n[values]\n\
             \"caption.max_lines\" = -3\n\
             \"appearance.language\" = \"en-US\"\n",
        )
        .unwrap();

        let (settings, outcome) = Settings::load_path(&path);
        let LoadOutcome::LoadedWithDropped(dropped) = outcome else {
            panic!("越界值应只丢该项：{outcome:?}");
        };
        assert_eq!(dropped.len(), 1);
        assert_eq!(dropped[0].key, "caption.max_lines");
        assert_eq!(
            settings.get_str("appearance.language").as_deref(),
            Some("en-US")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **数组元素类型不符曾让整份配置作废**（N05）。
    ///
    /// `SettingValue` 是 `#[serde(untagged)]`：`["a", 1]` 匹配不上任何一个变体，于是
    /// `toml::from_str` 在**整份文件**上失败，走 `Recovered` —— 用户的语言、主题、路径、
    /// 快捷键全部回默认，只因为某个数组里躺着一个数字。§7.3a 的表格第一行要的是"只丢该项"。
    #[test]
    fn a_wrongly_typed_array_element_drops_only_that_key() {
        for bad in [
            "[1, 2]",
            "[true]",
            r#"["text_quick_note", 7]"#,
            "\"text_quick_note,new_note\"", // 字符串不是数组：同样只丢该项，不做转换
        ] {
            let dir = temp_dir(&format!("bad_array_{}", bad.len()));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("config.toml");
            std::fs::write(
                &path,
                format!(
                    "schema_version = 1\n[values]\n\
                     \"workspace.quick_start_actions\" = {bad}\n\
                     \"appearance.language\" = \"en-US\"\n\
                     \"appearance.theme\" = \"dark\"\n"
                ),
            )
            .unwrap();

            let (settings, outcome) = Settings::load_path(&path);
            let LoadOutcome::LoadedWithDropped(dropped) = outcome else {
                panic!("坏数组 {bad} 必须只丢该项，不得整份作废：{outcome:?}");
            };
            assert_eq!(dropped.len(), 1, "{bad}");
            assert_eq!(dropped[0].key, "workspace.quick_start_actions", "{bad}");
            // 该项回退注册表默认……
            assert_eq!(
                settings
                    .get("workspace.quick_start_actions")
                    .and_then(|v| v.as_string_list().map(<[String]>::to_vec)),
                Some(
                    crate::QUICK_START_DEFAULT
                        .iter()
                        .map(|s| (*s).to_owned())
                        .collect::<Vec<_>>()
                ),
                "{bad}"
            );
            // ……其余项照常生效（这正是整份作废时一起丢掉的那些）。
            assert_eq!(
                settings.get_str("appearance.language").as_deref(),
                Some("en-US"),
                "{bad}"
            );
            assert_eq!(
                settings.get_str("appearance.theme").as_deref(),
                Some("dark"),
                "{bad}"
            );
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// 合法数组照常读进来（上一条不能靠"数组一律丢掉"蒙混过关）。
    #[test]
    fn a_valid_string_array_still_loads() {
        let dir = temp_dir("good_array");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            "schema_version = 1\n[values]\n\
             \"workspace.quick_start_actions\" = [\"new_note\", \"import_material\"]\n",
        )
        .unwrap();
        let (settings, outcome) = Settings::load_path(&path);
        assert_eq!(outcome, LoadOutcome::Loaded);
        assert_eq!(
            settings
                .get("workspace.quick_start_actions")
                .and_then(|v| v.as_string_list().map(<[String]>::to_vec)),
            Some(vec!["new_note".to_owned(), "import_material".to_owned()])
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 反向锁定：**结构性**损坏仍须整份回退（逐项容错不能顺手把这条也放宽了）。
    #[test]
    fn structural_damage_still_recovers_wholesale() {
        let dir = temp_dir("structural");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "schema_version = 1\n[values\n oops").unwrap();
        let (_, outcome) = Settings::load_path(&path);
        assert!(matches!(outcome, LoadOutcome::Recovered(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn future_version_file_recovers() {
        let dir = temp_dir("future");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "schema_version = 999\n[values]\n").unwrap();

        let (_, outcome) = Settings::load_path(&path);
        assert!(matches!(outcome, LoadOutcome::Recovered(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
