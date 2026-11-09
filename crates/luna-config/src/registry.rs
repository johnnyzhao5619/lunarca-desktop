//! 设置注册表（**单一来源**，TDD-05 §7.1）。
//!
//! 所有设置项在此编译期登记于一处；§2.6 总表、设置 UI、导入导出、恢复默认、CI 一致性
//! 校验都消费同一份 `SETTINGS`，杜绝定义散落（DRY，呼应 ADR-001）。

use crate::value::SettingValue;

/// 配置归属层：决定写哪套存储 / 是否随同步走（TDD-05 §7.2）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingScope {
    /// 设备级（逐机本地，不随 Vault/同步）。
    Device,
    /// Vault 级（随 Vault / 未来 E2E 同步）。
    Vault,
}

impl SettingScope {
    /// 稳定序列化名（供 IPC / 导出消费，避免 `Debug` 格式漂移）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Device => "device",
            Self::Vault => "vault",
        }
    }
}

/// 设置中心的**一级页面**（§2.6 IA，ADR-120）。
///
/// 一层平铺：每个变体就是侧栏上的一项，主区一次呈现一页。**没有"大类"这一层**——
/// 118 行内容配两层骨架时，简易投影下每页只剩两三行，组头比内容还多。页内的次级切分
/// 由 `section` 承担（PRD-09-070 ⑦），它是同一页内的分段，不是第二级导航。
///
/// 注册表里没有键的页（账户 / 个人词典）不在此枚举内：它们整页由专属面板承载，
/// 登记在前端 `SETTINGS_PANEL_PAGES`。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SettingGroup {
    /// 快捷键。**同质页**：整页都是快捷键行，故不分节，也不做简易/高级切分——
    /// 它是一张查找表，藏掉一半只会让人更难找，「找」这件事归搜索（ADR-120）。
    Shortcuts,
    /// 通用：界面、辅助功能、启动与通知、工作区。
    General,
    /// 语音输入与文本朗读。朗读并入此页而不自成一页：两者都是「声音与文字互转」的
    /// 用户侧设置，各自两三行时分成两页只是多一次导航。
    VoiceInput,
    /// 录音与转写：实时录制、录制中转写、回放，以及导入媒体的离线转写。
    ///
    /// 实时与离线合并成**一页**但**分属不同节**——`realtime.*` 与 `transcribe.*`
    /// 的配置零交集，混用会让「为省电关掉实时转译」静默导致导入不翻译（PRD-05-031）；
    /// 但用户带来的任务是同一个（「把这段音频变成文字」），分成两页只会让他先猜自己
    /// 属于哪一路。
    Recording,
    /// 翻译与字幕：弹窗翻译的语言与预设，实时字幕的声音来源与外观。
    Translate,
    /// 笔记与写作：编辑器、AI 对话、知识复利。
    ///
    /// AI 对话归此页而不与弹窗翻译同页：它的键自己给出了归属——`chat.capture_selection`
    /// 带入选区、`chat.distill_mode` 落成**知识笔记**，围着笔记转；弹窗翻译则是系统级
    /// 划词工具，两者的用户心智不搭。
    Notes,
    /// AI 算力与模型：算力落点、连接的服务端、模型分配与本机模型。
    ProviderModel,
    /// 存储与隐私：Vault、库内目录、附件与本地隐私边界。
    Storage,
    /// 系统：外部工具（yt-dlp / ffmpeg / JS 运行时）、实验室与诊断。
    ///
    /// 独立成页而不是并入任何功能页——它们服务多个功能，归到任一功能下都会让另一半的
    /// 用户找不到。**不叫「高级」**：高级是每页底部那一节的名字（就地展开，ADR-120），
    /// 一个叫「高级」的页面会把分类法抄第二遍。
    System,
}

/// 设置中心的一级页面（ADR-130）。
///
/// `SettingGroup` 仍是配置注册时的历史归属，不能用它推断用户在设置中心应到哪里：一个
/// Vault 设置、隐私同意和模型存储曾被塞进同一页，正是这轮整改要消除的学习成本。页面
/// 投影因此独立声明在 Rust；它不改变 key、默认值、持久化位置或运行行为。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SettingPage {
    General,
    Appearance,
    Shortcuts,
    AiServices,
    LocalModels,
    Translate,
    VoiceInput,
    Recording,
    Notes,
    Glossary,
    Privacy,
    Account,
}

/// 用户任务页的导航顺序，导出为前端只读清单。
pub const SETTINGS_PAGES: &[SettingPage] = &[
    SettingPage::General,
    SettingPage::Appearance,
    SettingPage::Shortcuts,
    SettingPage::AiServices,
    SettingPage::LocalModels,
    SettingPage::Translate,
    SettingPage::VoiceInput,
    SettingPage::Recording,
    SettingPage::Notes,
    SettingPage::Glossary,
    SettingPage::Privacy,
    SettingPage::Account,
];

impl SettingPage {
    /// 常态页头仅保留必要的范围说明，其他介绍继续参与页面搜索。
    #[must_use]
    pub const fn description_inline(self) -> bool {
        matches!(self, Self::Privacy | Self::Account)
    }
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Appearance => "appearance",
            Self::Shortcuts => "shortcuts",
            Self::AiServices => "ai_services",
            Self::LocalModels => "local_models",
            Self::Translate => "translate",
            Self::VoiceInput => "voice_input",
            Self::Recording => "recording",
            Self::Notes => "notes",
            Self::Glossary => "glossary",
            Self::Privacy => "privacy",
            Self::Account => "account",
        }
    }
}

impl SettingGroup {
    /// 稳定序列化名（供 IPC / 设置 UI 消费，避免 `Debug` 格式成为契约）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Shortcuts => "shortcuts",
            Self::General => "general",
            Self::VoiceInput => "voice_input",
            Self::Recording => "recording",
            Self::Translate => "translate",
            Self::Notes => "notes",
            Self::ProviderModel => "provider_model",
            Self::Storage => "storage",
            Self::System => "system",
        }
    }
}

/// 默认值（const 友好，供 `static SETTINGS`）。
#[derive(Clone, Copy, Debug)]
pub enum SettingDefault {
    /// 布尔默认。
    Bool(bool),
    /// 整数默认。
    Int(i64),
    /// 字符串默认。
    Str(&'static str),
    /// 有序字符串列表默认（TDD-05 §7.5a）。
    StringList(&'static [&'static str]),
}

impl SettingDefault {
    /// 转为运行时取值。
    #[must_use]
    pub fn to_value(self) -> SettingValue {
        match self {
            Self::Bool(b) => SettingValue::Bool(b),
            Self::Int(i) => SettingValue::Int(i),
            Self::Str(s) => SettingValue::Str(s.to_owned()),
            Self::StringList(items) => {
                SettingValue::StringList(items.iter().map(|s| (*s).to_owned()).collect())
            }
        }
    }
}

/// 数值控件形态：连续调节走滑块，精确输入走步进器。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumberStyle {
    /// 滑块（连续感知的量：语速、不透明度、帧率……）。
    Slider,
    /// 步进器（精确的计数：份数、行数、容量……）。
    Stepper,
}

impl NumberStyle {
    /// 稳定序列化名（供 IPC / 设置 UI 消费）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Slider => "slider",
            Self::Stepper => "stepper",
        }
    }
}

/// 枚举单选的控件形态：少量短选项并排成分段控件，一眼比对；其余走下拉。
///
/// **形态由注册表声明，不由前端按文案长度猜**：同一项的中英文标签长度不同，按字数
/// 猜会让同一个设置在两种语言下长成两种控件；而"这几个选项值不值得并排"是产品判断，
/// 与 [`NumberStyle`] 选滑块还是步进器同一层级，理应与选项集合写在同一处。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SelectStyle {
    /// 下拉（默认）：选项多、标签长、或只有在展开时才需要读全文。
    #[default]
    Dropdown,
    /// 分段控件：≤ 4 个短标签并排常显，当前档一眼可见（主题 / 录入方式 / 档位）。
    ///
    /// **判据认标签长度，不认选项个数**：四个标签里只要有一个是短语，分段控件就会换行
    /// 成两排——真机 2026-09-10 的「配色主题」（选项名里塞了「（暖调 + 月光金）」）与
    /// 英文字幕字号（"Extra large"）都是这么裂开的。长标签一律走 [`Dropdown`]。
    /// 上限由 `segmented_option_labels_stay_short` 按**中英两份真实文案**钉住。
    ///
    /// [`Dropdown`]: SelectStyle::Dropdown
    Segmented,
}

impl SelectStyle {
    /// 稳定序列化名（供 IPC / 设置 UI 消费）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dropdown => "dropdown",
            Self::Segmented => "segmented",
        }
    }
}

/// 数值单位：**只声明语义**，显示格式（“%”/“ms”/“GiB”）由前端 i18n 决定（禁硬编码文案）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingUnit {
    /// 无单位（纯计数由 `Count` 表达，此项用于无量纲值）。
    None,
    /// 百分比（含 ×100 存储的比率，如 RTF 60 = 0.60×）。
    Percent,
    /// 毫秒。
    Millisecond,
    /// 帧每秒。
    Fps,
    /// GiB（磁盘容量类阈值以 GiB 存储——字节数输入框对人不可读）。
    Gibibyte,
    /// 笔记历史版本数。
    Version,
    /// 同屏字幕行数。
    Line,
    /// 美元每分钟（参考单价）。
    UsdPerMinute,
    /// 美元每百万 token（参考单价）。
    UsdPerMillionTokens,
}

impl SettingUnit {
    /// 稳定序列化名（供 IPC / 设置 UI 消费）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Percent => "percent",
            Self::Millisecond => "millisecond",
            Self::Fps => "fps",
            Self::Gibibyte => "gibibyte",
            Self::Version => "version",
            Self::Line => "line",
            Self::UsdPerMinute => "usd_per_minute",
            Self::UsdPerMillionTokens => "usd_per_million_tokens",
        }
    }

    /// 显示比例由注册表声明；存储、范围与滑块步长仍使用原始单位。
    #[must_use]
    pub const fn display_divisor(self) -> i32 {
        match self {
            Self::Millisecond => 1000,
            _ => 1,
        }
    }

    /// 毫秒时长在人类界面显示秒，其余沿用各自领域单位。
    #[must_use]
    pub const fn display_unit(self) -> &'static str {
        match self {
            Self::Millisecond => "second",
            _ => self.as_str(),
        }
    }
}

/// 路径类设置的语义：决定校验规则与是否给系统目录选择器。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathKind {
    /// 本机绝对目录（如模型存储根）——给「浏览…」原生选择器。
    AbsoluteDir,
    /// 本机绝对**文件**（如外部工具可执行文件路径）——「浏览…」须给文件选择器而非目录选择器。
    /// 与 [`Self::AbsoluteDir`] 分立是因为二者的原生对话框不同类：让用户在目录选择器里
    /// 指一个可执行文件是选不中的。
    AbsoluteFile,
    /// Vault 相对目录（归档/附件/模板目录）——拒绝绝对路径与 `..` 穿越。
    VaultRelativeDir,
    /// **Vault 根**：换库不是写一个字符串，而是重新绑定（建目录/挂媒体服务/装监听/重扫索引）。
    /// 前端据此走 `select_vault` 而非 `set_setting`，避免出现「路径改了但库没换」的半吊子状态。
    VaultRoot,
}

impl PathKind {
    /// 稳定序列化名。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AbsoluteDir => "absolute_dir",
            Self::AbsoluteFile => "absolute_file",
            Self::VaultRelativeDir => "vault_relative_dir",
            Self::VaultRoot => "vault_root",
        }
    }
}

/// 模型槽位：定义在 [`luna_core::ModelSlot`]，此处原样再导出。
///
/// **它搬去 `luna-core` 是 ADR-083 ⑥ 的要求**：
/// 「路由泳道 → 槽位」是一张总映射（`luna_core::Job::model_slot`），设置层按槽位存分配、
/// 路由层按槽位解析 Provider，两处必须读同一个枚举。留在本 crate 的话路由层够不着它，
/// 于是那张表会被抄第二份——而抄出来的分叉表现是「设置页里的实时档在连上服务端之后
/// 悄悄失效」。设置层的调用点因这条再导出一个字节不改。
pub use luna_core::ModelSlot;

/// 文本设置的**取值格式**：写入前先归一化（[`normalize`]），再按格式校验（[`validate`]）。
/// 补得出确定结果的补而不拒；补不出来的当场拒，绝不静默存下一个用起来必错的值。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextFormat {
    /// 自由文本，不校验也不归一化。
    Free,
    /// 导出文件命名模板（PRD-11-007）：`{name}` / `{date}` / `{time}` / `{format}` / `{content}`
    /// 五个占位，语法与渲染都在 `luna_core::filename_template`（设置写入与导出用同一份）。
    ExportFilenameTemplate,
    /// 一段话（可以换行）：首尾空白去掉、换行统一成 `\n`，最多 `max_chars` 个字符。这是取值
    /// 契约，也决定前端给多行文本框（IPC 的 `multiline`）——一段话塞进单行框是看不全的。
    Paragraph {
        /// 最多几个字符（按 Unicode 字符计）。
        max_chars: usize,
    },
    /// 一个内容 id（[`luna_content_schema::ContentId`] 的字符白名单）：去首尾空白。取值是否
    /// 对应一份**此刻存在**的内容由用到它的地方判定（如主题注册表找不到就回落内置），这里只裁决
    /// 写法——id 的集合随用户导入而变，不能写成静态选项。
    ContentId,
}

/// 值归一化 —— **与 [`validate`] 同一批调用点，且必须先于它**。
///
/// 两个调用点：用户写入（`set_setting`）与配置文件加载。**两侧必须同一口径**——配置文件
/// 不是只有本应用写（用户手改、部署方 MDM 下发、排障直接编辑），那些入口写进来的
/// `10.0.0.62:8090` 与输入框里敲的完全一样，凭什么一个被补全、另一个被判非法丢弃？
/// 丢弃的后果还特别隐蔽：服务端地址无声变空，用户只看到"AI 又跑回这台设备上了"。
#[must_use]
pub fn normalize(spec: &SettingSpec, value: SettingValue) -> SettingValue {
    match spec.control {
        SettingControl::Text { format } => {
            let Some(text) = value.as_str() else {
                return value;
            };
            match format {
                TextFormat::Free => value,
                TextFormat::ExportFilenameTemplate => SettingValue::Str(text.trim().to_owned()),
                TextFormat::Paragraph { .. } => {
                    SettingValue::Str(text.replace("\r\n", "\n").trim().to_owned())
                }
                TextFormat::ContentId => SettingValue::Str(text.trim().to_owned()),
            }
        }
        // 有序开关列表写入前归一化到完整全集（ADR-087 ⑥「与注册表对齐」）：缺失的项按默认
        // 顺序补回、显隐取该选项的默认可见性（ADR-162 ①），重复只留首次、未知 id 丢弃、锁定项强制显示。落盘的值因此恒为完整
        // 全集；读取端（活动栏）对文件里的值再跑同一个函数，两边看到的是同一份。
        //
        // 这不是"把非法值洗成合法值"：这个控件的每一项都是**有限全集里的一位开关**，
        // 不存在"用户想写一个不在全集里的 id"这种合法意图——多出来的 id 只可能来自
        // 已退役的空间，少掉的只可能是新加的空间；两者都该由封闭定义域给答案（ADR-046：
        // 归一化不是兼容层）。
        SettingControl::OrderedToggleList {
            options,
            hidden_prefix,
            min_visible,
        } => {
            let Some(items) = value.as_string_list() else {
                return value;
            };
            SettingValue::StringList(
                resolve_ordered_toggle_list(options, hidden_prefix, min_visible, items)
                    .into_iter()
                    .map(|(id, visible)| toggle_token(hidden_prefix, id, visible))
                    .collect(),
            )
        }
        _ => value,
    }
}

/// 有序开关列表里的一个选项（PRD-07-003 / TDD-05 §7.5a）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToggleOption {
    /// 稳定 id（设置值 token 的主体）。
    pub id: &'static str,
    /// 名字键（`ns:key`），设置控件直接 `t()`——指向该对象**已有的**那一个名字，不另起标签。
    pub label_key: &'static str,
    /// 锁定：可排序、不可关（锚点）。
    pub locked: bool,
    /// 默认是否显示：从未配置、或已存的值里缺了这一项（新加的选项）时取它。
    ///
    /// 新选项缺失时一律补成「显示」的话，默认隐藏的选项（如创作空间，ADR-162 ①）会在
    /// 已配置过布局的用户升级后自己冒出来——默认值只对新装生效，等于没有默认值。
    pub default_visible: bool,
}

/// 有序开关列表的 token：`<id>` = 显示，`<hidden_prefix><id>` = 隐藏。
#[must_use]
pub fn toggle_token(hidden_prefix: &str, id: &str, visible: bool) -> String {
    if visible {
        id.to_owned()
    } else {
        format!("{hidden_prefix}{id}")
    }
}

/// 有序开关列表的归一化（[`normalize`] 与读取端共用的唯一实现）：
/// 未知 id 丢弃、重复只留首次、锁定项强制显示、缺失的选项按 `options` 的顺序补回，显隐取
/// [`ToggleOption::default_visible`]（锁定项恒显示）——补在「默认顺序里排在它前面、且已在
/// 结果里」的最后一项之后，一个都没有就放最前。最后，显示项少于 `min_visible` 时按顺序把
/// 最前面的隐藏项改回显示，补足为止。
#[must_use]
pub fn resolve_ordered_toggle_list(
    options: &'static [ToggleOption],
    hidden_prefix: &str,
    min_visible: usize,
    tokens: &[String],
) -> Vec<(&'static str, bool)> {
    let mut entries: Vec<(&'static str, bool)> = Vec::with_capacity(options.len());
    for token in tokens {
        let (id, hidden) = match token.strip_prefix(hidden_prefix) {
            Some(id) => (id, true),
            None => (token.as_str(), false),
        };
        let Some(option) = options.iter().find(|o| o.id == id) else {
            continue;
        };
        if entries.iter().any(|(known, _)| *known == option.id) {
            continue;
        }
        entries.push((option.id, !hidden || option.locked));
    }
    for (index, option) in options.iter().enumerate() {
        if entries.iter().any(|(known, _)| *known == option.id) {
            continue;
        }
        let insert_at = options[..index]
            .iter()
            .rev()
            .find_map(|prev| {
                entries
                    .iter()
                    .position(|(known, _)| *known == prev.id)
                    .map(|position| position + 1)
            })
            .unwrap_or(0);
        entries.insert(
            insert_at,
            (option.id, option.default_visible || option.locked),
        );
    }
    let mut shortfall = min_visible.saturating_sub(entries.iter().filter(|(_, v)| *v).count());
    for entry in &mut entries {
        if shortfall == 0 {
            break;
        }
        if !entry.1 {
            entry.1 = true;
            shortfall -= 1;
        }
    }
    entries
}

/// **控件契约**：一个设置项「长什么样、能取什么值」由注册表声明，前端按 `kind` 渲染。
///
/// 这是 ADR-003（前端零业务）在设置域的落点——选项集合、取值范围、单位、路径语义全部
/// 只有 Rust 一份；前端此前用 `settingOptions(key)` 复制了一份选项表，与后端校验各说各话
/// （后端当时**根本不校验**，非法值可直接写进配置文件）。校验与渲染自此同源（[`validate`]）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SettingControl {
    /// 开关（`bool`）。
    Toggle,
    /// 枚举单选（`str`）；选项标签 i18n key = `option.<setting key>.<value>`。
    Select {
        /// 合法取值集合（**唯一事实源**，校验与下拉同源）。
        options: &'static [&'static str],
        /// 控件形态（下拉 / 分段），见 [`SelectStyle`]。
        style: SelectStyle,
    },
    /// 整数（`int`）：范围/步长/单位/形态由注册表给出。
    Number {
        /// 下界（含）。
        min: i64,
        /// 上界（含）。
        max: i64,
        /// 步长。
        step: i64,
        /// 单位语义。
        unit: SettingUnit,
        /// 滑块 or 步进器。
        style: NumberStyle,
    },
    /// 小数（以 `str` 存储——[`SettingValue`] 无浮点变体）：范围与步长同样声明。
    Decimal {
        /// 下界（含）。
        min: f64,
        /// 上界（含）。
        max: f64,
        /// 步长。
        step: f64,
        /// 单位语义。
        unit: SettingUnit,
    },
    /// 文本（`str`）。
    ///
    /// `format` 是**取值契约**而不是控件形态：控件仍是一个文本框（IPC 的
    /// `SettingControlDto::Text` 不变、前端渲染不变），变的只是「什么值合法」以及
    /// 「保存前先归一化成什么」。做成 `SettingControl` 的新变体是错的——那会让
    /// 前端以为多了一种控件要渲染，而它要渲染的东西一点没变。
    Text {
        /// 取值格式（[`TextFormat`]）。
        format: TextFormat,
    },
    /// 路径（`str`）。
    Path {
        /// 绝对目录 or Vault 相对目录。
        kind: PathKind,
    },
    /// 快捷键（`str`，值语法见 TDD-05 §1.2：普通组合 / `double_tap:<组合>`）。
    ///
    /// `scope` 声明这枚键由谁响应：全局键经系统热键 / 事件 tap 注册，应用内键由前端命令
    /// 注册表在 webview 内判定。两者都支持双击触发；前端不按 key 维护第二份名单（ADR-003）。
    Shortcut {
        /// 响应方（全局 / 应用内）。
        scope: ShortcutScope,
    },
    /// 模型分配（`str`，值为不透明 `ModelTargetId`；空值由槽位继承或设备策略解析）。
    ///
    /// 它是领域状态而不是通用下拉选项：前端不得自行解析路径、模型名或能力，只能经
    /// `assign_model` / `clear_model_assignment` 由 Rust 状态机写入。
    ModelAssignment {
        /// 该项对应的模型分配槽位。
        slot: ModelSlot,
    },
    /// 音频输入设备（`str`，空 = 跟随系统默认；候选由系统枚举）。
    AudioInputDevice,
    /// 有序、无重复的有限字符串选项（`string_list`，TDD-05 §7.5a）。
    ///
    /// 与 [`Self::Select`] 的差别不只是"能多选"：**顺序有意义**（快速开始快捷项的排列
    /// 即屏幕顺序），故它的值是一个 `Vec` 而不是集合，控件也必须给出上移/下移。
    ///
    /// `require_each_category` 是**领域约束**而不是控件形态：控件长什么样与"每类至少
    /// 一项"无关，但校验必须知道它——把这条规则写在消费者那边，用户就会先保存成功、再
    /// 在概览上发现自己没有任何写作入口（PRD-07-068）。
    OrderedMultiSelect {
        /// 合法选项全集（**唯一事实源**，与分类查表同源）。
        options: &'static [&'static str],
        /// 至少几项。
        min: usize,
        /// 至多几项。
        max: usize,
        /// 是否要求写作与资料两类各至少一项（PRD-07-068）。
        require_each_category: bool,
    },
    /// 有序开关列表（`string_list`，PRD-07-003 / TDD-05 §7.5a）：**全集**有序排列、每项可开关、
    /// 锁定项不可关。值是 token 列表：`<id>` = 显示，`<hidden_prefix><id>` = 隐藏；顺序即屏幕顺序。
    ///
    /// 与 [`Self::OrderedMultiSelect`] 的差别：那边"在不在列表里"就是选没选，这边全集恒在场、
    /// 显隐是每项自己的一位——隐藏项也保住它的位置，重新显示时回到原处（PRD-07-003）。
    OrderedToggleList {
        /// 全部选项（id / 名字键 / 是否锁定），**唯一事实源**。
        options: &'static [ToggleOption],
        /// 隐藏项前缀。
        hidden_prefix: &'static str,
        /// 至少显示几项（锁定项计入）；`0` = 不设下限。归一化后显示项不足时，按顺序把排在
        /// 最前的隐藏项改回显示，补足为止——看板总得剩一列（创作空间，ADR-162）。
        min_visible: usize,
    },
}

impl SettingControl {
    /// 稳定 kind 名（供 IPC / 设置 UI 分发渲染）。
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Toggle => "toggle",
            Self::Select { .. } => "select",
            Self::Number { .. } => "number",
            Self::Decimal { .. } => "decimal",
            Self::Text { .. } => "text",
            Self::Path { .. } => "path",
            Self::Shortcut { .. } => "shortcut",
            Self::ModelAssignment { .. } => "model_assignment",
            Self::AudioInputDevice => "audio_input_device",
            Self::OrderedMultiSelect { .. } => "ordered_multi_select",
            Self::OrderedToggleList { .. } => "ordered_toggle_list",
        }
    }
}

/// 快捷键的响应方（[`SettingControl::Shortcut`]）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShortcutScope {
    /// 全局：任意应用前台都响应（热键插件 / 事件 tap 双击手势，由 `luna-app` 注册）。
    Global,
    /// 应用内：只在 Lunarca 窗口有焦点时响应（前端命令注册表判定）。
    InApp,
}

impl ShortcutScope {
    /// 稳定名（IPC 投影）。
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::InApp => "in_app",
        }
    }
}

/// 翻译目标语言全集（BCP-47 主语种码）——**语言清单的唯一事实源**。
///
/// 设置注册表（语音输入/实时转译/弹窗翻译的语言项）与前端各语言下拉共用此集合：
/// 前端从设置规格的 `Select.options` 读取，不再各自维护一份数组（否则前端列出的语言
/// 会被后端校验拒绝，是必然的漂移事故）。标签走 i18n `common:lang.*`。
pub const TRANSLATE_TARGET_LANGS: &[&str] =
    &["zh", "en", "ja", "ko", "fr", "de", "es", "pt", "ru", "it"];

/// 转写语种提示：`auto`（自动检测）+ [`TRANSLATE_TARGET_LANGS`] 全集（单测锁定同步）。
/// 蒸馏触发档（PRD-06-044 / ADR-024）：资料转写或会话终局后，要不要整理成知识笔记。
///
/// **默认 `prompt`**——`off` 会让这条能力实质不存在（没人会去翻设置发现它），`auto` 则
/// 在用户还不知道产出长什么样时就替他跑 LLM 并往知识库里放东西。`prompt` 骑在既有完成
/// 通知上，零新增打扰面，而动作始终由用户点。
pub const DISTILL_MODES: &[&str] = &["off", "prompt", "auto"];

/// 应用更新策略（PRD-09-051，TDD-13 §5）：启动时检查、由用户确认安装（默认）/ 自动下载安装 /
/// 只在用户点「检查更新」时联网。严格本地开着时三者都不联网。
pub const UPDATE_POLICIES: &[&str] = &["check_and_confirm", "automatic", "manual"];

/// 归入触发档（PRD-16-024）：一篇知识笔记诞生后，要不要把它的要点归入既有主题页。
///
/// 与 [`DISTILL_MODES`] **三档结构同构**（关 / 中档 / 自动，同一「只挂边界事件」纪律、
/// 脏值一律取最保守档），但**中档的呈现刻意不同**：蒸馏的 `prompt` 骑在完成通知上，
/// 归入的 `suggest` 用**就地建议条且不发任何通知**（PRD-16-004）。故键名也不同——
/// 若两边都叫 `prompt`，读代码的人会以为它们的呈现也一样。
///
/// **默认 `suggest`**：human-in-the-loop 在「写手是随机过程」时是质量控制而非落后
/// （PRD-16 §5.1/§5.4），也是信任阶梯的正确起点。
pub const INGEST_MODES: &[&str] = &["off", "suggest", "auto"];

pub const TRANSCRIBE_LANG_HINTS: &[&str] = &[
    "auto", "zh", "en", "ja", "ko", "fr", "de", "es", "pt", "ru", "it",
];

/// 一个设置项的声明。
pub struct SettingSpec {
    /// 稳定 key（如 `"privacy.strict_local"`）。
    pub key: &'static str,
    /// 一级分组。
    pub group: SettingGroup,
    /// 组内分节（`PRD-09-070` ⑦）：同一分组里跨越多个任务阶段的行，按节贴弱化小节标题。
    ///
    /// `None` = 该组不分节（同质小组贴标题只是噪声——标签的职责是区分，没有可区分对象
    /// 就不贴，与文件树段头同一裁决）。声明了分节的分组必须**全员归节且同节连续**
    /// （注册顺序 = 屏幕顺序，读注册表即读界面；由 `sectioned_groups_are_fully_and_contiguously_sectioned`
    /// 钉住）。节标题文案走 `settings:section.<group>.<id>`，中英在场由
    /// `check-settings-registry.py` 校验。
    pub section: Option<&'static str>,
    /// 归属层。
    pub scope: SettingScope,
    /// 「恢复默认」的唯一取值来源。
    pub default: SettingDefault,
    /// 控件契约（怎么编辑 / 合法取值范围，[`SettingControl`]）。
    pub control: SettingControl,
    /// 即时生效 vs 需重启。
    pub requires_restart: bool,
    /// i18n 标签 key（禁硬编码文案）。
    pub i18n_key: &'static str,
}

impl SettingSpec {
    /// 通用恢复不能绕过领域命令或清除凭据，显示投影自身也不属于恢复范围。
    #[must_use]
    pub fn resettable(&self) -> bool {
        !is_internal(self.key)
            && !matches!(
                self.control,
                SettingControl::Path {
                    kind: PathKind::VaultRoot
                }
            )
            && self.key != "storage.models_root"
            && !self.key.starts_with("ai.assignment.")
            && !is_settings_projection_control(self.key)
    }

    /// 实验室依赖与一方应用的启用状态都属于能力事实，不由 UI 根据 key 重建：归属某个一方应用
    /// 的键在那个应用停用时不可用（设置页不显示，ADR-165 ④）。
    #[must_use]
    pub fn available(&self, settings: &crate::Settings) -> bool {
        let lab_ready = !self.key.starts_with("shortcuts.desktop_lens")
            || settings
                .get_bool("labs.translate_space_enabled")
                .unwrap_or(false);
        lab_ready && crate::apps::setting_available(settings, self.key)
    }
    /// 设置中心的显示层级。它只影响同一份配置快照的投影，绝不表示功能是否运行。
    #[must_use]
    pub fn visibility(&self) -> SettingVisibility {
        if is_simple_setting(self.key) {
            SettingVisibility::Standard
        } else {
            SettingVisibility::Advanced
        }
    }
    /// 说明文案的 i18n key（**约定派生**：标签 key + `_desc`）。
    ///
    /// 每个用户可见设置都必须有一句「这是什么 / 改了会怎样」——设置页此前对所有行渲染
    /// 同一句占位说明，等于没有说明。约定派生而非再加一个字段：一处定义、零样板，
    /// CI（`check-settings-registry.py`）校验 zh/en 两份文案都在场。
    #[must_use]
    pub fn desc_i18n_key(&self) -> String {
        format!("{}_desc", self.i18n_key)
    }

    /// 说明的呈现方式（见 [`desc_presentation`]）。
    #[must_use]
    pub fn desc_presentation(&self) -> SettingDescPresentation {
        desc_presentation(self.key)
    }
}

/// 设置页归属是 Rust 注册表的投影，不由前端按 key 前缀或文案猜测。
///
/// 这是一个有意窄的映射：配置的原始 `group` 继续用于存储与旧领域内聚；用户任务页可
/// 随产品分类调整而不重命名或迁移 142 个配置键。
#[must_use]
pub fn setting_page(key: &str) -> SettingPage {
    if key.starts_with("privacy.") {
        SettingPage::Privacy
    } else if key == "storage.models_root" || key.starts_with("models.") {
        SettingPage::LocalModels
    } else if key.starts_with("ai.") || key.starts_with("provider.") {
        SettingPage::AiServices
    } else if key.starts_with("appearance.")
        && key != "appearance.language"
        && key != "appearance.launch_overview"
    {
        SettingPage::Appearance
    } else if key.starts_with("shortcuts.") {
        SettingPage::Shortcuts
    } else if key.starts_with("voice_input.") || key.starts_with("speech.") {
        SettingPage::VoiceInput
    } else if key.starts_with("recording.")
        || key.starts_with("playback.")
        || key.starts_with("export.")
        || key.starts_with("realtime.")
        || key.starts_with("transcribe.")
    {
        SettingPage::Recording
    } else if key.starts_with("popup_translate.") || key.starts_with("caption.") {
        SettingPage::Translate
    } else if key.starts_with("sync.") {
        SettingPage::Account
    } else if key.starts_with("storage.")
        || key.starts_with("vault.")
        || key.starts_with("editor.")
        || key.starts_with("chat.")
        || key.starts_with("knowledge.")
    {
        SettingPage::Notes
    } else {
        SettingPage::General
    }
}

/// 页内分节沿用注册表声明；由专属面板承载的 AI 服务和本地模型页不渲染设置行分节。
#[must_use]
pub fn setting_page_section(spec: &SettingSpec) -> Option<&'static str> {
    match setting_page(spec.key) {
        SettingPage::AiServices | SettingPage::LocalModels => None,
        _ => spec.section,
    }
}

/// 简易层预算：恰好 30 个可持续修改的用户偏好（ADR-130）。
///
/// 内部状态不靠这张表变得可见；其呈现面仍由 `setting_surface` 单独裁决。新增简易项必须
/// 在本表替换一项或连同 ADR/预算守护一起重审，避免把复杂度藏到面板或子控件里。
#[must_use]
pub fn is_simple_setting(key: &str) -> bool {
    matches!(
        key,
        "appearance.language"
            | "appearance.launch_overview"
            | "notifications.system_enabled"
            | "appearance.theme"
            | "appearance.theme_family"
            | "appearance.font_scale"
            | "appearance.reduce_motion"
            | "appearance.high_contrast"
            | "shortcuts.voice_input"
            | "shortcuts.popup_translate"
            | "shortcuts.recording_toggle"
            | "ai.route_strategy"
            | "provider.remote_asr_enabled"
            | "voice_input.input_mode"
            | "voice_input.lang_hint"
            | "voice_input.keep_audio"
            | "speech.highlight"
            | "recording.input_device"
            | "recording.capture_source"
            | "realtime.transcribe_enabled"
            | "popup_translate.native_lang"
            | "popup_translate.default_foreign"
            | "caption.font_size"
            | "storage.vault_root"
            | "vault.delete_mode"
            | "vault.trash_retention_days"
            | "privacy.strict_local"
            | "privacy.local_knowledge_metrics"
            | "privacy.local_workspace_metrics"
    )
}

/// 设置说明（`<key>_desc`）的**呈现方式**（PRD-09-084、TDD-07 §4.1b，2026-10-08 D1）。
///
/// 说明资源一律保留——它同时是设置搜索的语料与注册表校验的对象；这里只声明它**默认怎么出现**。
/// 判据由本注册表逐键声明，前端穷尽消费：不按文案长度、不按 key 前缀、不在前端另抄名单。
/// 只改变呈现，从不改变任何设置值或运行状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingDescPresentation {
    /// 常显一句：影响、风险、作用范围或决策后果（缺省档——没被逐键声明的一律常显）。
    Inline,
    /// 按需展开：说明是机制或语法，标签与控件已足够完成任务；行内给一个可聚焦的帮助入口。
    Disclosure,
    /// 不显示常态说明：标签与控件本身已说清（语言、字号、主题这一类）；说明仍参与搜索。
    LabelOnly,
}

impl SettingDescPresentation {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Inline => "inline",
            Self::Disclosure => "disclosure",
            Self::LabelOnly => "label_only",
        }
    }
}

/// 逐键声明说明的呈现方式（缺省 [`SettingDescPresentation::Inline`]）。
///
/// 两张名单都是**用户能从界面上验证的判断**：`LabelOnly` 的标签 + 控件（选项名、百分比、
/// 实时生效的外观）本身就是说明；`Disclosure` 的说明讲的是机制或语法，决策不依赖它。
/// 隐私、删除、费用、联网、上传类的键**不得**出现在这两张名单里（守护测试
/// `risk_settings_keep_inline_descriptions`）。
#[must_use]
pub fn desc_presentation(key: &str) -> SettingDescPresentation {
    match key {
        "appearance.language"
        | "appearance.font_scale"
        | "appearance.theme"
        | "appearance.density"
        | "appearance.high_contrast"
        | "caption.font_size"
        | "caption.text_color"
        | "playback.subtitle_size"
        | "speech.rate"
        | "editor.show_format_toolbar"
        | "voice_input.translate_target_lang"
        // 两个选项名本身就是手势（「按住说话」「按一下开始 / 结束」，F18）。
        | "voice_input.input_mode"
        | "shortcuts.quick_chat"
        | "shortcuts.open_settings"
        | "shortcuts.toggle_sidebar"
        | "popup_translate.default_foreign"
        | "popup_translate.native_lang"
        | "shortcuts.live_caption" => SettingDescPresentation::LabelOnly,
        "appearance.reduce_motion"
        | "appearance.theme_family"
        | "shortcuts.quick_switcher"
        | "shortcuts.command_palette"
        | "shortcuts.vault_search"
        | "caption.text_opacity"
        | "recording.input_device"
        | "export.filename_template"
        | "recording.waveform_fps"
        | "shortcuts.double_tap_window_ms"
        | "degradation.green_max_rtf"
        | "degradation.yellow_max_rtf"
        | "diarize.cluster_threshold" => SettingDescPresentation::Disclosure,
        _ => SettingDescPresentation::Inline,
    }
}

/// 设置中心的编译期可见性分类（TDD-05 §7.6）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingVisibility {
    /// 常见任务必需的设置与所有隐私、权限、无障碍、错误修复入口。
    Standard,
    /// 仅在用户主动切到完整设置投影后常规显示的系统维护项。
    Advanced,
}

impl SettingVisibility {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Advanced => "advanced",
        }
    }
}

/// 当前设备级配置 schema 版本。
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

/// 某设置键是否为**内部状态**（应用自身读写的进度/标记，非用户偏好）。
///
/// 内部状态不在设置 UI 渲染为可编辑行（有专属交互面——如首启引导经向导/「重新运行引导」
/// 管理；语音 HUD 形态/固定位置经 HUD 自身按钮管理，PRD-02-019），但仍走同一注册表/
/// 持久化/导出通道（单一来源，不另起存储）。
#[must_use]
pub fn is_internal(key: &str) -> bool {
    key.starts_with("onboarding.")
        || key.starts_with("voice_input.hud_")
        || key.starts_with("desktop_lens.")
        // 「这台服务端的算力交接提示已经问过了」——应用自己写的一次性标记，
        // 由算力卡上那条提示的两个按钮管理（ADR-059 ④），不该在设置列表里露面。
        || key.starts_with("compute.handover_")
        // 字幕窗记下的位置与尺寸（收起时由应用写回，下次打开回到原处，PRD-04-040）。
        || key.starts_with("caption.window_")
        // 提词器的显示偏好与窗口几何（面板上的按钮直接调，ADR-162 ⑤ C4a），不进设置页。
        || key.starts_with("teleprompter.")
        // 一方应用的启用状态：由「应用」页的启用 / 停用管理（ADR-165 ③）。
        || key.starts_with("apps.")
        // yt-dlp 的路径覆盖：由「应用 ▸ 从链接导入」的组件面板承载（ADR-167 ⑤），设置中心不再
        // 有「外部工具」页；工具的状态、版本、路径与安装只在那一处说。
        || key == "transcribe.fetch_tool_path"
        // 「从链接导入」的权利声明确认过没有：第一次用时那张确认卡写它（ADR-167 ④）。
        || key.starts_with("link_import.")
        // 「启动时检查更新」问过没有：首启通知与「关于与更新」提问卡写它（PRD-09-051）。
        || key == "update.startup_check_prompt"
}

/// 由**分组专属面板**承载的键：用户仍可改，只是入口在面板里而非设置列表。
///
/// 对外的单一入口是 [`setting_surface`]；本函数是它的判据之一，单独留着是因为下面这段
/// 论证只属于「面板承载」这一档。
///
/// 与 [`is_internal`] 分开的理由：内部键是"用户根本不该在设置页看到"，而这些是
/// "有更合适的呈现处"。真机反馈的正是后者——`transcribe.fetch_tool_path` 作为设置行显示
/// 「未设置」，而同屏的「外部工具」面板显示「可用 · 由 Lunarca 管理」：两个说法互相矛盾
/// （一个说的是"覆盖项没填"，一个说的是"工具的实际状态"），用户只会问"到底装了没有"。
/// 工具的**一切**（状态 / 版本 / 路径 / 安装 / 卸载）归它自己那一行，才没有第二个说法。
///
/// **判据只认「有没有别的面能承载它」，不是「它属于哪个组」**：参考单价（`provider.*_cost_*`）
/// 与模型分配同处「AI 引擎 / 模型」组，但模型面板讲的是分配与运行时，没有任何一格在讲它们；
/// 把它们一并标成面板承载的下场是**两项设置在界面上彻底消失**（既不在列表里，也没有面板
/// 渲染它们）——这正是本组整体从设置页蒸发那一轮的另一半。
#[must_use]
pub fn is_panel_owned(key: &str) -> bool {
    // 同步偏好由账户页「云同步」一节承载（ADR-148 ⑩、ADR-156）：它们是那一节里每个库同步方式
    // 的细节，拆成独立设置行就成了与那一节并列的第二个同步入口。
    if key.starts_with("sync.") {
        return true;
    }
    // 转写同意由「算力位置」卡片承载（ADR-038 ④ 要的是**显式、可见**，而不是
    // 「藏在设置第八项」）。同意点搬到用户**正在决定连不连服务端**的那一刻，
    // 比留在列表里更符合那条裁决；留在列表里则会与卡片上的开关重复成两个入口。
    if key == "provider.remote_asr_enabled" {
        return true;
    }
    // 模型存储根只由搬移状态机写（PRD-10-014）：设置行不渲染，入口在模型页。
    if key == "storage.models_root" {
        return true;
    }
    // 配色主题由主题库卡片承载：内置主题、当前安装包与安装动作必须在一个地方，避免
    // “选主题”与“管理主题”拆成两套入口。
    if key == "appearance.theme_family" {
        return true;
    }
    // 优先落点由算力卡承载：它决定的正是那张卡上逐条能力的去向，而它此前是这一页上
    // **唯一**一行设置，夹在两张卡与一个面板之间（ADR-122 ①）。
    if key == "ai.route_strategy" {
        return true;
    }
    // 两个参考单价只喂「执行记录」那一面的估算，别处没有任何一格在讲它们（ADR-122 ⑥）。
    if matches!(
        key,
        "provider.asr_cost_per_min" | "provider.llm_cost_per_million_tokens"
    ) {
        return true;
    }
    // 模型分配槽由「AI 引擎 / 模型」面板独占写入口（唯一写入命令 `assign_model`）。
    // 判据取**控件形态**而不是再抄一份 key 清单：抄一份就会在下一个槽位加进来时静默漏掉，
    // 而漏掉的那一项会以一个写不进去的下拉出现在设置列表里。
    spec(key).is_some_and(|s| matches!(s.control, SettingControl::ModelAssignment { .. }))
}

/// 设置中心侧栏底部的全局投影控件；它有注册表值，但不属于任何分组的内容范围。
#[must_use]
pub fn is_settings_projection_control(key: &str) -> bool {
    key == "settings.advanced_mode"
}

/// 一个设置键在设置页上**呈现在哪**。
///
/// 取代此前的 `hidden_in_list: bool`。那个布尔把两件语义相反的事压成了一个值：
/// 内部键是「用户界面里根本没有它」，而面板承载的键是「用户看得见、改得了，只是入口
/// 在别处」。压平的代价是**面板承载的键连搜索都命中不到**——转写同意（一个隐私开关）
/// 此前是设置行、能被搜到，归给算力卡片之后就搜不到了；模型分配与外部工具路径一直如此。
/// 用户搜「上传」「转写」找不到那个开关时，得到的结论是"这个功能不存在"。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingSurface {
    /// 普通设置行。
    Row,
    /// 由分组专属面板 / 卡片承载：不渲染成行，但**搜索照常命中**，命中后深链过去。
    Panel,
    /// 内部状态与全局投影控件：界面上没有它的位置，搜索也不该命中。
    Internal,
}

/// 该键呈现在哪（设置页的单一判据）。
#[must_use]
pub fn setting_surface(key: &str) -> SettingSurface {
    if is_internal(key) || is_settings_projection_control(key) {
        return SettingSurface::Internal;
    }
    if is_panel_owned(key) {
        return SettingSurface::Panel;
    }
    SettingSurface::Row
}

/// 全量设置注册（新增设置 = 在此加一行）。Phase 2 先登记主干所需的少量项。
pub static SETTINGS: &[SettingSpec] = &[
    // ── 云同步（设备级，ADR-156）────────────────────────────────────────────────
    //
    // 录音（音视频）的同步方式：按需下载（默认，别的设备的录音只登记为仅云端、打开时才下）/
    // 全部下载 / 不同步（本机新录的不上传）。笔记与边车不受它影响，恒同步。
    SettingSpec {
        key: "sync.recordings_mode",
        group: SettingGroup::Storage,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Str("on_demand"),
        control: SettingControl::Select {
            options: &["on_demand", "full", "off"],
            style: SelectStyle::Dropdown,
        },
        requires_restart: false,
        i18n_key: "settings:sync.recordings_mode",
    },
    // 按流量计费网络（手机热点）上暂停大文件的上传下载，笔记照常（ADR-156 ③）。
    SettingSpec {
        key: "sync.pause_large_on_metered",
        group: SettingGroup::Storage,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Bool(true),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:sync.pause_large_on_metered",
    },
    // 低电量模式或电量低于 20% 时暂停大文件的上传下载，笔记照常（ADR-156 ③）。
    SettingSpec {
        key: "sync.pause_large_on_battery",
        group: SettingGroup::Storage,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Bool(true),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:sync.pause_large_on_battery",
    },
    SettingSpec {
        key: "privacy.strict_local",
        group: SettingGroup::Storage,
        section: Some("privacy"),
        scope: SettingScope::Device,
        default: SettingDefault::Bool(false),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:privacy.strict_local",
    },
    SettingSpec {
        // 网页剪藏（ADR-164 ⑦，PRD-09-087）：用户点了才发生的联网动作，开关让它可见可关；
        // 严格本地模式开着时无论此项如何都拦下。
        key: "privacy.web_clip",
        group: SettingGroup::Storage,
        section: Some("privacy"),
        scope: SettingScope::Device,
        default: SettingDefault::Bool(true),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:privacy.web_clip",
    },
    SettingSpec {
        // Phase 11 local-only product evidence. A Vault may live in sync storage; this data may not.
        key: "privacy.local_knowledge_metrics",
        group: SettingGroup::Storage,
        section: Some("privacy"),
        scope: SettingScope::Device,
        default: SettingDefault::Bool(true),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:privacy.local_knowledge_metrics",
    },
    // 工作区任务观测（PRD-09-076 / PRD-07-070 / TDD-12 §2.1）。
    //
    // **独立开关、默认关闭**，绝不复用或扩大 `privacy.local_knowledge_metrics` 的授权
    // （PRD-09-076 / TDD-12 §2.1）：两者记录的是不同的东西，用一个开关启停两类记录，等于用户为
    // A 打开的同意被拿去做了 B。
    //
    // **默认关而知识指标默认开**，不是不一致：知识指标回答的是"AI 建议到底有没有帮上
    // 忙"（产品的核心不确定性，且只落聚合），工作区观测回答的是"这次改版有没有让人更快
    // 上手"——后者是**我们**想知道的事，不是用户使用产品必需的，故由他明确打开。
    //
    // `Standard` 而不是 `Advanced`：隐私开关必须在标准模式可达（TDD-05 §7.6）。
    SettingSpec {
        key: "privacy.local_workspace_metrics",
        group: SettingGroup::Storage,
        section: Some("privacy"),
        scope: SettingScope::Device,
        default: SettingDefault::Bool(false),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:privacy.local_workspace_metrics",
    },
    // ── 逐槽模型分配（advanced）────────────────────────────────────────────────
    //
    // 模型分配是独立领域对象。每个槽位持久化不透明 `ModelTargetId`，空值由 Rust 解析为
    // 继承或设备策略；通用设置命令不能直接写入，见 ModelAssignmentService。
    //
    // **整段 advanced**。留空即由设备策略解析，这是绝大多数人应该保持的状态：普通用户
    // 要回答的是「AI 在哪跑」（首屏卡片），而不是「实时对话该用哪个 checkpoint、批式转写
    // 又该用哪个」——后者要求他先知道自己有哪些模型、各自多大、跑不跑得动。
    //
    // 这五项**由模型面板承载**（`SettingSurface::Panel`）：不渲染成设置行，闸在面板的
    // 「分配」页签上。`visibility` 与它**正交**且照样生效——它决定搜索命中这五项时算不算
    // 「另有 N 个高级结果」，以及深链跳过去要不要顺带揭示完整设置。两个字段各答一个问题：
    // surface 说「呈现在哪」，visibility 说「属不属于进阶」。
    SettingSpec {
        key: "ai.assignment.default_llm",
        group: SettingGroup::ProviderModel,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Str(""),
        control: SettingControl::ModelAssignment {
            slot: ModelSlot::DefaultLlm,
        },
        requires_restart: false,
        i18n_key: "settings:ai.assignment.default_llm",
    },
    SettingSpec {
        key: "ai.assignment.realtime_llm",
        group: SettingGroup::ProviderModel,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Str(""),
        control: SettingControl::ModelAssignment {
            slot: ModelSlot::RealtimeLlm,
        },
        requires_restart: false,
        i18n_key: "settings:ai.assignment.realtime_llm",
    },
    SettingSpec {
        key: "ai.assignment.realtime_asr",
        group: SettingGroup::ProviderModel,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Str(""),
        control: SettingControl::ModelAssignment {
            slot: ModelSlot::RealtimeAsr,
        },
        requires_restart: false,
        i18n_key: "settings:ai.assignment.realtime_asr",
    },
    SettingSpec {
        key: "ai.assignment.batch_asr",
        group: SettingGroup::ProviderModel,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Str(""),
        control: SettingControl::ModelAssignment {
            slot: ModelSlot::BatchAsr,
        },
        requires_restart: false,
        i18n_key: "settings:ai.assignment.batch_asr",
    },
    SettingSpec {
        key: "ai.assignment.default_embedding",
        group: SettingGroup::ProviderModel,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Str(""),
        control: SettingControl::ModelAssignment {
            slot: ModelSlot::DefaultEmbed,
        },
        requires_restart: false,
        i18n_key: "settings:ai.assignment.default_embedding",
    },
    // 路由策略预设（PRD-10-008 P1 落地形态，TDD-01 §4.1）：只改候选顺序与是否允许外发；
    // 故障处置仍由类型化处置唯一裁决。三档而非规格里的四档：成本优先在定价表落地前与
    // 本机优先无法区分（本机 = 0 费用是唯一可信的成本事实），并入一档不造假。
    SettingSpec {
        key: "ai.route_strategy",
        group: SettingGroup::ProviderModel,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Str("cloud_first"),
        // 下拉而不是分段：四档里有一档是整句话（「本机优先，兜底服务端」/ "Local first,
        // server as fallback"），四个标签英文合计 63 个字符——分段控件按 `SelectStyle` 的
        // 判据只收「≤ 4 个**短**标签」，撑不下就会换行成两排（真机 2026-09-10 的字幕字号
        // 正是这么裂开的）。判据认标签长度，不认选项个数。
        control: SettingControl::Select {
            options: &[
                "cloud_first",
                "local_first",
                "local_first_fallback",
                "local_only",
            ],
            style: SelectStyle::Dropdown,
        },
        requires_restart: false,
        i18n_key: "settings:ai.route_strategy",
    },
    // ── 算力落点（standard）────────────────────────────────────────────────────
    //
    // 这一段**必须是 standard**：整段 advanced 时，关掉「显示完整设置」的用户看不见
    // 服务端转写开关，也就无法开启那条能力，甚至不知道它存在。
    //
    // 本页**不分节**（ADR-122 ⑦）：键全部由面板承载，`section` 声明只会生成几个永不
    // 渲染的空段标题。服务器地址与验签公钥不是设置项——它们是账户与信任锚的事实（ADR-155）。
    // 服务端转写开关（ADR-036 批式通道 / ADR-038 实时通道）。
    //
    // **默认关，且刻意不与「连接服务端」合并**。连服务端可能只为了对话或同步，而
    // 麦克风与会议录音是**另一个量级的同意**——ADR-038 ④ 写死「默认档不得是云」。
    // 把它并进地址那一项 = 连上服务端就默认开始上传录音；简化到这一步就过头了。
    //
    // 为什么不靠「注册顺序 = 本机优先」兜住：本机 whisper 是**懒加载**的（首个转写任务
    // 才注册），启动后的一段时间里注册表里根本没有本地候选，云会成为唯一候选。
    // 靠时序保证隐私默认值，是一条随时会被重构掉的隐性依赖。
    SettingSpec {
        key: "provider.remote_asr_enabled",
        group: SettingGroup::ProviderModel,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Bool(false),
        control: SettingControl::Toggle,
        // **热生效**：`apply_setting_side_effects` 立刻重装 ASR 两条通道并对齐外发闸
        // （真机验过：翻开关后模型分配页的槽位状态当场改变，无需重启）。
        // 标成需重启是骗用户白白重启一次——而正在录长会议的人会因此丢掉这一场。
        requires_restart: false,
        i18n_key: "settings:provider.remote_asr_enabled",
    },
    // 两个参考单价：只喂「执行记录」那一面的估算，**不产生任何联网行为、与计费无关**
    // （ADR-122 ⑥），故由该面板承载。
    SettingSpec {
        key: "provider.asr_cost_per_min",
        group: SettingGroup::ProviderModel,
        section: None,
        scope: SettingScope::Device,
        // 字符串存储（SettingValue 无浮点）：值 "0.006" = 0.006 USD/min。
        // **默认留空 = 未配置**（PRD-07-051）：这里此前默认写着某厂商的公开报价，
        // 于是概览会拿一个用户从没设过、也看不到出处的单价算出一个金额给他看。
        default: SettingDefault::Str(""),
        control: SettingControl::Decimal {
            min: 0.0,
            max: 10.0,
            step: 0.001,
            unit: SettingUnit::UsdPerMinute,
        },
        requires_restart: false,
        i18n_key: "settings:provider.asr_cost_per_min",
    },
    SettingSpec {
        key: "provider.llm_cost_per_million_tokens",
        group: SettingGroup::ProviderModel,
        section: None,
        scope: SettingScope::Device,
        // 字符串存储：值 "1.0" = 1.0 USD/百万 token。默认留空 = 未配置（同上一条）。
        default: SettingDefault::Str(""),
        control: SettingControl::Decimal {
            min: 0.0,
            max: 1000.0,
            step: 0.1,
            unit: SettingUnit::UsdPerMillionTokens,
        },
        requires_restart: false,
        i18n_key: "settings:provider.llm_cost_per_million_tokens",
    },
    // 启动指针（Device 级，PRD-14）：记录用户在首启引导/设置中选择/创建的 Vault 根，下次启动据此
    // 直接打开。与 Vault 级 `vault.root`（vault 内容配置）分离——位置是“本机打开哪个 Vault”的设备态。
    SettingSpec {
        key: "storage.vault_root",
        group: SettingGroup::Storage,
        section: Some("vault"),
        scope: SettingScope::Device,
        default: SettingDefault::Str(""),
        control: SettingControl::Path {
            kind: PathKind::VaultRoot,
        },
        requires_restart: false,
        i18n_key: "settings:storage.vault_root",
    },
    // 模型存储根（PRD-10-014 / TDD-04 §2.3）：空 = `<app_data>/models`。**不是可直接写字符串的
    // 通用路径设置**——只有搬移状态机在「拷贝 → 校验 → 切换 → 删旧」全部通过后才写它
    //（`is_panel_owned`：设置行不渲染，入口在模型页「搬移到…」）。设备级：路径指向这台机器的磁盘。
    SettingSpec {
        key: "storage.models_root",
        group: SettingGroup::ProviderModel,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Str(""),
        control: SettingControl::Path {
            kind: PathKind::AbsoluteDir,
        },
        requires_restart: false,
        i18n_key: "settings:storage.models_root",
    },
    // 本机模型的首选下载来源（ADR-144 ①）：只改尝试顺序，首选那家失败仍回落其余来源。
    // 设备级：选哪家取决于这台机器所在的网络。连着服务端时制品面镜像始终在最前，不受它影响。
    SettingSpec {
        key: "models.download_source",
        group: SettingGroup::ProviderModel,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Str("auto"),
        control: SettingControl::Select {
            options: &["auto", "huggingface", "modelscope"],
            // 下拉：两个品牌名不能缩写，英文三枚合计排不进分段控件的定宽列。
            style: SelectStyle::Dropdown,
        },
        requires_restart: false,
        i18n_key: "settings:models.download_source",
    },
    SettingSpec {
        key: "voice_input.auto_refine",
        group: SettingGroup::VoiceInput,
        section: Some("input"),
        scope: SettingScope::Device,
        default: SettingDefault::Bool(true),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:voice_input.auto_refine",
    },
    SettingSpec {
        key: "voice_input.quick_note_refine",
        group: SettingGroup::VoiceInput,
        section: Some("input"),
        scope: SettingScope::Device,
        // 默认关（默认离线、显式可见，PRD-06-058 C′）：开了才在语音速记落盘后跑一次 AI 提炼。
        default: SettingDefault::Bool(false),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:voice_input.quick_note_refine",
    },
    // 保存原始语音文件（PRD-02-030）：语音速记落盘时把这段音频作为附件留在笔记旁，
    // 于是速记也能回放、能用更好的模型二次转写（PRD-02-032）。默认开：速记只有几十秒，
    // 而「说过什么」的原声是文字对不上时唯一的凭据；不落速记（注入 / 剪贴板）的会话
    // 结束即删。设备级：是否留音频是这台机器的磁盘偏好。
    SettingSpec {
        key: "voice_input.keep_audio",
        group: SettingGroup::VoiceInput,
        section: Some("input"),
        scope: SettingScope::Device,
        default: SettingDefault::Bool(true),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:voice_input.keep_audio",
    },
    SettingSpec {
        key: "voice_input.translate_enabled",
        group: SettingGroup::VoiceInput,
        section: Some("input"),
        scope: SettingScope::Device,
        // 语音输入默认只按原语种润色；翻译必须由用户显式开启（PRD-02-013）。
        default: SettingDefault::Bool(false),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:voice_input.translate_enabled",
    },
    SettingSpec {
        key: "voice_input.translate_target_lang",
        group: SettingGroup::VoiceInput,
        section: Some("input"),
        scope: SettingScope::Device,
        // 语音输入翻译目标语言；中文用户高频是口述中文→英文输出，默认 en。
        default: SettingDefault::Str("en"),
        control: SettingControl::Select {
            options: TRANSLATE_TARGET_LANGS,
            style: SelectStyle::Dropdown,
        },
        requires_restart: false,
        i18n_key: "settings:voice_input.translate_target_lang",
    },
    SettingSpec {
        key: "voice_input.landing_mode",
        group: SettingGroup::VoiceInput,
        section: Some("input"),
        scope: SettingScope::Device,
        default: SettingDefault::Str("inject"),
        control: SettingControl::Select {
            options: &["inject", "clipboard", "quick_note"],
            style: SelectStyle::Dropdown,
        },
        requires_restart: false,
        i18n_key: "settings:voice_input.landing_mode",
    },
    SettingSpec {
        key: "voice_input.input_mode",
        group: SettingGroup::VoiceInput,
        section: Some("input"),
        scope: SettingScope::Device,
        default: SettingDefault::Str("hold"),
        control: SettingControl::Select {
            options: &["hold", "toggle"],
            style: SelectStyle::Segmented,
        },
        requires_restart: false,
        i18n_key: "settings:voice_input.input_mode",
    },
    SettingSpec {
        key: "voice_input.lang_hint",
        group: SettingGroup::VoiceInput,
        section: Some("input"),
        scope: SettingScope::Device,
        default: SettingDefault::Str("auto"),
        control: SettingControl::Select {
            options: TRANSCRIBE_LANG_HINTS,
            style: SelectStyle::Dropdown,
        },
        requires_restart: false,
        i18n_key: "settings:voice_input.lang_hint",
    },
    // ── 算力交接提示的一次性标记（内部键，`is_internal` 前缀 `compute.handover_`）──
    //
    // 存的是**已经问过的那台服务端**（`server_bucket` 分桶），不是一个布尔。布尔的话，
    // 用户换一台组织服务端之后再也不会被问——而「换了服务端」恰恰是最该重新问一次的时刻。
    // 空串 = 从没问过。专属交互面是算力卡上那条提示的两个按钮（ADR-059 ④）。
    SettingSpec {
        key: "compute.handover_prompted_for",
        group: SettingGroup::ProviderModel,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Str(""),
        control: SettingControl::Text {
            format: TextFormat::Free,
        },
        requires_restart: false,
        i18n_key: "settings:compute.handover_prompted_for",
    },
    // ── 语音 HUD 窗口状态（内部键，`is_internal` 前缀 `voice_input.hud_`，PRD-02-018/019）──
    // 专属交互面在 HUD 自身（「⋯」里的固定位置），设置页不渲染为可编辑行。
    SettingSpec {
        // 固定位置开关：开 = 显示时用记忆坐标（不跟随光标），由 HUD 图钉按钮切换。
        key: "voice_input.hud_pinned",
        group: SettingGroup::VoiceInput,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Bool(false),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:voice_input.hud_pinned",
    },
    SettingSpec {
        // 固定位置记忆坐标（窗口左上角，物理像素；仅 hud_pinned=true 时有意义）。
        key: "voice_input.hud_pin_x",
        group: SettingGroup::VoiceInput,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Int(0),
        control: SettingControl::Number {
            min: -10000,
            max: 10000,
            step: 1,
            unit: SettingUnit::None,
            style: NumberStyle::Stepper,
        },
        requires_restart: false,
        i18n_key: "settings:voice_input.hud_pin_x",
    },
    SettingSpec {
        key: "voice_input.hud_pin_y",
        group: SettingGroup::VoiceInput,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Int(0),
        control: SettingControl::Number {
            min: -10000,
            max: 10000,
            step: 1,
            unit: SettingUnit::None,
            style: NumberStyle::Stepper,
        },
        requires_restart: false,
        i18n_key: "settings:voice_input.hud_pin_y",
    },
    // 弹窗翻译语言基准（设备级，TDD-05 §7.2）：智能双向模式据此定方向。
    // > 假设：默认 native=zh / foreign=en（中文优先产品 + 核心画像为中英双语，
    // > PRD-00）；首启引导（Phase 6）会按用户实际设定覆盖。
    SettingSpec {
        key: "popup_translate.native_lang",
        group: SettingGroup::Translate,
        section: Some("popup"),
        scope: SettingScope::Device,
        default: SettingDefault::Str("zh"),
        control: SettingControl::Select {
            options: TRANSLATE_TARGET_LANGS,
            style: SelectStyle::Dropdown,
        },
        requires_restart: false,
        i18n_key: "settings:popup_translate.native_lang",
    },
    SettingSpec {
        key: "popup_translate.default_foreign",
        group: SettingGroup::Translate,
        section: Some("popup"),
        scope: SettingScope::Device,
        default: SettingDefault::Str("en"),
        control: SettingControl::Select {
            options: TRANSLATE_TARGET_LANGS,
            style: SelectStyle::Dropdown,
        },
        requires_restart: false,
        i18n_key: "settings:popup_translate.default_foreign",
    },
    // 默认启用模板 id（PRD-01-013 默认 1 槽的默认模板）；MVP 内置 general/slack/email/polish_en。
    SettingSpec {
        key: "popup_translate.default_preset",
        group: SettingGroup::Translate,
        section: Some("popup"),
        scope: SettingScope::Device,
        default: SettingDefault::Str("general"),
        control: SettingControl::Select {
            options: &["general", "slack", "email", "polish_en"],
            style: SelectStyle::Dropdown,
        },
        requires_restart: false,
        i18n_key: "settings:popup_translate.default_preset",
    },
    // 生成完成自动复制译文到剪贴板（PRD-01-032：对标 Bob/Easydict "翻完即得"零点击取用；
    // 默认关——自动改写剪贴板对不知情用户有破坏性，显式开启）。
    SettingSpec {
        key: "popup_translate.auto_copy",
        group: SettingGroup::Translate,
        section: Some("popup"),
        scope: SettingScope::Device,
        default: SettingDefault::Bool(false),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:popup_translate.auto_copy",
    },
    // 设置中心的唯一全局显示投影开关（TDD-05 §7.6）。它不属于任何设置分组，也绝不
    // 参与运行态、副作用或其它设置的取值判断；页头紧凑控件是唯一承载面。
    SettingSpec {
        key: "settings.advanced_mode",
        group: SettingGroup::System,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Bool(false),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:advanced_mode",
    },
    SettingSpec {
        key: "onboarding.completed",
        group: SettingGroup::System,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Bool(false),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:onboarding.completed",
    },
    SettingSpec {
        key: "onboarding.completed_steps",
        group: SettingGroup::System,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Str(""),
        control: SettingControl::Text {
            format: TextFormat::Free,
        },
        requires_restart: false,
        i18n_key: "settings:onboarding.completed_steps",
    },
    SettingSpec {
        // 首个 AI 任务在这台设备上成功过一次（语音输入落地 / 弹窗翻译出结果）。
        // 由 Rust 在产生结果的路径上写，前端只读（PRD-14-050 成功检测的事实源）。
        key: "onboarding.first_task_done",
        group: SettingGroup::System,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Bool(false),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:onboarding.first_task_done",
    },
    SettingSpec {
        // 用户点过「完成设置」横幅上的「以后再说」。横幅不再出现，启动概览也不再因为
        // "设置还没做完"而弹（那两件事是同一个提醒的两个面，只关一半等于没关）。
        // 引导本身照常可从「重新运行引导」进入——关掉的是提醒，不是能力。
        key: "onboarding.setup_reminder_dismissed",
        group: SettingGroup::System,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Bool(false),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:onboarding.setup_reminder_dismissed",
    },
    SettingSpec {
        key: "appearance.theme",
        group: SettingGroup::General,
        section: Some("interface"),
        scope: SettingScope::Device,
        default: SettingDefault::Str("system"),
        control: SettingControl::Select {
            options: &["system", "light", "dark"],
            style: SelectStyle::Segmented,
        },
        requires_restart: false,
        i18n_key: "settings:appearance.theme",
    },
    // 信息密度（PRD-12-021）：紧凑 / 舒适两档，与明暗、配色族正交、即时生效。前端只投影
    // 到根节点 `data-density`，尺寸由 density Token 在 CSS 层重映射（PRD-12-022，组件零改动）。
    // 默认紧凑（PRD-12-020 对标 Obsidian）。设备级：显示偏好逐机（TDD-05 §7.2）。
    SettingSpec {
        key: "appearance.density",
        group: SettingGroup::General,
        section: Some("interface"),
        scope: SettingScope::Device,
        default: SettingDefault::Str("compact"),
        control: SettingControl::Select {
            options: &["compact", "comfortable"],
            style: SelectStyle::Segmented,
        },
        requires_restart: false,
        i18n_key: "settings:appearance.density",
    },
    SettingSpec {
        key: "appearance.theme_family",
        group: SettingGroup::General,
        section: Some("interface"),
        scope: SettingScope::Device,
        default: SettingDefault::Str("luna"),
        // 取值是主题 id（内置或内容层里导入的，ADR-169 ③ ④）：集合随导入而变，故不是静态选项；
        // 找不到对应主题时由主题注册表回落内置的 `luna`。
        control: SettingControl::Text {
            format: TextFormat::ContentId,
        },
        requires_restart: false,
        i18n_key: "settings:appearance.theme_family",
    },
    SettingSpec {
        key: "appearance.language",
        group: SettingGroup::General,
        section: Some("interface"),
        scope: SettingScope::Device,
        default: SettingDefault::Str("system"),
        control: SettingControl::Select {
            options: &["system", "zh-CN", "en-US"],
            // 下拉：界面语言会随本地化继续加（今天三档，多一种语言就撑破分段）。
            style: SelectStyle::Dropdown,
        },
        requires_restart: false,
        i18n_key: "settings:appearance.language",
    },
    // 字号缩放：以百分比整数存储（100 = 1.0×），SettingValue 无浮点；前端除以 100 设 --font-scale。
    SettingSpec {
        key: "appearance.font_scale",
        group: SettingGroup::General,
        section: Some("interface"),
        scope: SettingScope::Device,
        default: SettingDefault::Int(100),
        control: SettingControl::Number {
            min: 80,
            max: 150,
            step: 5,
            unit: SettingUnit::Percent,
            style: NumberStyle::Slider,
        },
        requires_restart: false,
        i18n_key: "settings:appearance.font_scale",
    },
    // 无障碍开关（TDD-11 §a11y）：减少动效 / 高对比 即时生效；前端 theme.ts 据此设根属性。
    SettingSpec {
        key: "appearance.reduce_motion",
        group: SettingGroup::General,
        section: Some("a11y"),
        scope: SettingScope::Device,
        default: SettingDefault::Bool(false),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:appearance.reduce_motion",
    },
    SettingSpec {
        key: "appearance.high_contrast",
        group: SettingGroup::General,
        section: Some("a11y"),
        scope: SettingScope::Device,
        default: SettingDefault::Bool(false),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:appearance.high_contrast",
    },
    SettingSpec {
        key: "appearance.launch_overview",
        group: SettingGroup::General,
        section: Some("launch_notify"),
        scope: SettingScope::Device,
        default: SettingDefault::Bool(true),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:appearance.launch_overview",
    },
    SettingSpec {
        key: "notifications.completed_enabled",
        group: SettingGroup::General,
        section: Some("launch_notify"),
        scope: SettingScope::Device,
        default: SettingDefault::Bool(true),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:notifications.completed_enabled",
    },
    SettingSpec {
        // 系统通知总闸（PRD-15-070 / PRD-15-012，2026-09-06 随系统通道接线登记）：关着时
        // 不在场也按在场路由，完成 / 失败 / 高危一律只走应用内通道 + 历史。
        key: "notifications.system_enabled",
        group: SettingGroup::General,
        section: Some("launch_notify"),
        scope: SettingScope::Device,
        default: SettingDefault::Bool(true),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:notifications.system_enabled",
    },
    // 录音 / 实时转写进行中抑制非高危通知（PRD-15-032）：开会时弹横幅体验极差。被扣下的
    // 通知历史照记、录音结束后按原顺序补发；系统健康（高危）类不扣。macOS 专注模式由系统
    // 通知通道自行尊重（OS 层裁决投递），应用内通道无公开 API 可查，不另做判断。
    SettingSpec {
        key: "notifications.suppress_while_recording",
        group: SettingGroup::General,
        section: Some("launch_notify"),
        scope: SettingScope::Device,
        default: SettingDefault::Bool(true),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:notifications.suppress_while_recording",
    },
    // 应用更新（PRD-09-051，客户端分发 D2）：检查更新是 opt-in 的下行联网（TDD-12 §1），**缺省仅手动**——
    // 用户没点过「检查更新」、也没答应过启动时检查，就一次都不联网。首次启动发一条本地通知问一次
    // （`update.startup_check_prompt`），答应了才改成「启动时检查、由你确认」。
    SettingSpec {
        key: "update.policy",
        group: SettingGroup::General,
        section: Some("launch_notify"),
        scope: SettingScope::Device,
        default: SettingDefault::Str("manual"),
        control: SettingControl::Select {
            options: UPDATE_POLICIES,
            style: SelectStyle::Dropdown,
        },
        requires_restart: false,
        i18n_key: "settings:update.policy",
    },
    // ── 「启动时检查更新」问过没有（内部键，`is_internal`）──
    //
    // 空串 = 还没问；`notified` = 首启那条通知已发（不再每次启动都发）；`answered` = 用户在
    // 「关于与更新」的提问卡上答过了。专属交互面是那张卡的两个按钮（`app_release_answer_startup_check`）。
    SettingSpec {
        key: "update.startup_check_prompt",
        group: SettingGroup::General,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Str(""),
        control: SettingControl::Text {
            format: TextFormat::Free,
        },
        requires_restart: false,
        i18n_key: "settings:update.startup_check_prompt",
    },
    // 主题「族」与明暗是**正交**的两维（同 density 与 theme 正交）：族决定配色性格，
    // `appearance.theme` 仍然决定明/暗/跟随系统——否则把 nebula-dark 塞进 theme 的枚举里，
    // 选了非默认族就再也跟随不了系统明暗。前端只投影到根节点 `data-theme-family`，
    // 配色由 CSS 在语义 Token 层重映射（PRD-12-017，组件零改动）。
    // 启动概览弹窗（PRD-07-057 二次修订）：每次运行首次进入工作区时呈现一次的仪表盘
    // 弹窗。真机反馈「可选择下次不弹」——这是真实的用户偏好而非伪开关：弹窗内可勾掉，
    // 此处保证设置页能找回（只给"不再显示"入口而没有找回入口是陷阱）。
    // 「快速开始」快捷项（PRD-07-068 / PRD-09-070 / TDD-05 §7.5a）：启动概览上那一排入口
    // 显示哪几个、按什么顺序。
    //
    // **它只改呈现，不改动作身份、目的地或权限**：隐藏一个入口不会让对应能力消失
    // （完整目录恒在「添加内容」里），故它是安全的个人偏好而不是功能开关。
    //
    // **`Standard` 而不是 `Advanced`**：调整自己最常用的入口是最普通不过的偏好，
    // 而不是需要先理解内部机制才能碰的参数。
    SettingSpec {
        key: "workspace.quick_start_actions",
        group: SettingGroup::General,
        section: Some("workspace"),
        scope: SettingScope::Device,
        default: SettingDefault::StringList(crate::QUICK_START_DEFAULT),
        control: SettingControl::OrderedMultiSelect {
            options: crate::CREATE_ACTION_IDS,
            // 下限 2 而不是 1：只剩一个入口的"快速开始"已经不是一排入口了，而两类各至少
            // 一项本身就要求至少两项——写成 1 会造出一条永远无法满足的组合约束。
            min: 2,
            max: 4,
            require_each_category: true,
        },
        requires_restart: false,
        i18n_key: "settings:workspace.quick_start_actions",
    },
    // 活动栏顺序与显隐（PRD-07-003 / ADR-087 ⑥）：一个有序开关列表说清两件事——顺序即
    // 屏幕顺序，每项可开关；锚点（笔记 / 知识库）锁定不可关。隐藏 ≠ 停用：快捷键、添加
    // 内容、深链、命令面板与文件本身全部照常，故它与「快速开始」同为安全的个人偏好
    //（Standard、Device）。值语法见 TDD-05 §7.5a，词表与归一化归 `workspace_space`。
    SettingSpec {
        key: "workspace.rail_layout",
        group: SettingGroup::General,
        section: Some("workspace"),
        scope: SettingScope::Device,
        default: SettingDefault::StringList(crate::workspace_space::RAIL_LAYOUT_DEFAULT),
        control: SettingControl::OrderedToggleList {
            options: crate::workspace_space::RAIL_SPACE_OPTIONS,
            hidden_prefix: crate::workspace_space::RAIL_HIDDEN_PREFIX,
            // 锚点恒显示，已经保证活动栏不会空。
            min_visible: 0,
        },
        requires_restart: false,
        i18n_key: "settings:workspace.rail_layout",
    },
    // 创作空间看板的列（PRD-19-003 / ADR-162）：七个阶段按需显隐、可排序，至少留一列。
    // 与活动栏布局同一种控件、同一种值语法：名字直接用看板上的阶段名（每个对象只有一个
    // 名字键）。隐藏某列不改任何项目文件——停在那个阶段的项目暂时显示在它前面最近的可见列里。
    SettingSpec {
        key: "creator.board_stages",
        group: SettingGroup::General,
        section: Some("workspace"),
        scope: SettingScope::Device,
        default: SettingDefault::StringList(crate::creator_board::CREATOR_BOARD_DEFAULT),
        control: SettingControl::OrderedToggleList {
            options: crate::creator_board::CREATOR_STAGE_OPTIONS,
            hidden_prefix: crate::creator_board::CREATOR_STAGE_HIDDEN_PREFIX,
            min_visible: 1,
        },
        requires_restart: false,
        i18n_key: "settings:creator.board_stages",
    },
    // 创作看板的视图（同一份项目数据的四种看法，Notion / Linear / Trello 同一做法）：看板按阶段
    // 分列拖拽、列表按阶段分组成紧凑清单、表格一行一个项目并可就地改阶段、日历按计划发布日期
    // 排（PRD-19-040，拖到某一天即改日期）。
    SettingSpec {
        key: "creator.board_view",
        group: SettingGroup::General,
        section: Some("workspace"),
        scope: SettingScope::Device,
        default: SettingDefault::Str("board"),
        control: SettingControl::Select {
            options: crate::creator_board::CREATOR_BOARD_VIEWS,
            style: SelectStyle::Segmented,
        },
        requires_restart: false,
        i18n_key: "settings:creator.board_view",
    },
    // 我的频道风格（ADR-166 ⑤，PRD-19-042）：一段话，生成章节、渠道包装、分发与脚本改稿时作为
    // 内容带给模型（不改提示词结构）。描述的是这个创作者 / 这个库，随库（Vault）；空 = 不带。
    SettingSpec {
        key: "creator.channel_voice",
        group: SettingGroup::General,
        section: Some("workspace"),
        scope: SettingScope::Vault,
        default: SettingDefault::Str(""),
        control: SettingControl::Text {
            format: TextFormat::Paragraph { max_chars: 1000 },
        },
        requires_restart: false,
        i18n_key: "settings:creator.channel_voice",
    },
    // 弹窗翻译触发（与其余全局快捷键同构，PRD-09 §2.6 / TDD-05 §1.2）：值支持普通组合或
    // `double_tap:<组合>` 语法，改值热生效。默认双击复制键：双击有选区即捕获翻译、无选区
    // 降级空输入弹窗，故无需单设备用键。
    SettingSpec {
        key: "shortcuts.popup_translate",
        group: SettingGroup::Shortcuts,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Str("double_tap:cmd_or_ctrl+c"),
        control: SettingControl::Shortcut {
            scope: ShortcutScope::Global,
        },
        requires_restart: false,
        i18n_key: "settings:shortcuts.popup_translate",
    },
    // 截屏翻译触发键（PRD-01-007：交互框选 → Vision OCR → 弹窗翻译；键值格式同其他 shortcuts.*，
    // 默认 Cmd/Ctrl+Shift+2——毗邻系统截屏键位 3/4/5 便于记忆，避开 Cmd+Shift+S「存储为」等常用应用键）。
    SettingSpec {
        key: "shortcuts.screenshot_translate",
        group: SettingGroup::Shortcuts,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Str("cmd_or_ctrl+shift+2"),
        control: SettingControl::Shortcut {
            scope: ShortcutScope::Global,
        },
        requires_restart: false,
        i18n_key: "settings:shortcuts.screenshot_translate",
    },
    // 译境的会话开关：与其它全局快捷键共用注册、冲突校验与热更新主干。
    SettingSpec {
        key: "shortcuts.desktop_lens",
        group: SettingGroup::Shortcuts,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Str("cmd_or_ctrl+shift+1"),
        control: SettingControl::Shortcut {
            scope: ShortcutScope::Global,
        },
        requires_restart: false,
        i18n_key: "settings:shortcuts.desktop_lens",
    },
    // 译境的按需捕捉：会话中唯一会反复做的动作，故值得一个不用把手挪开键盘的入口。默认键
    // 必须与截屏翻译键不同，避免实验室启用后两个全局动作争用同一组合。仅在会话运行时有效，
    // 绝不用“捕捉”隐式开启会话。
    SettingSpec {
        key: "shortcuts.desktop_lens_capture",
        group: SettingGroup::Shortcuts,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Str("cmd_or_ctrl+shift+9"),
        control: SettingControl::Shortcut {
            scope: ShortcutScope::Global,
        },
        requires_restart: false,
        i18n_key: "settings:shortcuts.desktop_lens_capture",
    },
    // 译境的窗口几何属于设备级内部状态：由取景器本身拖动/缩放写入，不在设置表格重复
    // 造一套难以理解的坐标输入。单独的 placed 标志区分“尚未定位”与用户有意放在左上角
    // （x=0, y=0），不让坐标值兼任状态位。
    SettingSpec {
        key: "desktop_lens.placed",
        group: SettingGroup::Translate,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Bool(false),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:popup_translate.desktop_lens",
    },
    SettingSpec {
        key: "desktop_lens.x",
        group: SettingGroup::Translate,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Int(0),
        control: SettingControl::Number {
            min: -10000,
            max: 10000,
            step: 1,
            unit: SettingUnit::None,
            style: NumberStyle::Stepper,
        },
        requires_restart: false,
        i18n_key: "settings:popup_translate.desktop_lens",
    },
    SettingSpec {
        key: "desktop_lens.y",
        group: SettingGroup::Translate,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Int(0),
        control: SettingControl::Number {
            min: -10000,
            max: 10000,
            step: 1,
            unit: SettingUnit::None,
            style: NumberStyle::Stepper,
        },
        requires_restart: false,
        i18n_key: "settings:popup_translate.desktop_lens",
    },
    SettingSpec {
        key: "desktop_lens.width",
        group: SettingGroup::Translate,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Int(560),
        control: SettingControl::Number {
            min: 200,
            max: 4000,
            step: 10,
            unit: SettingUnit::None,
            style: NumberStyle::Stepper,
        },
        requires_restart: false,
        i18n_key: "settings:popup_translate.desktop_lens",
    },
    SettingSpec {
        key: "desktop_lens.height",
        group: SettingGroup::Translate,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Int(320),
        control: SettingControl::Number {
            min: 160,
            max: 4000,
            step: 10,
            unit: SettingUnit::None,
            style: NumberStyle::Stepper,
        },
        requires_restart: false,
        i18n_key: "settings:popup_translate.desktop_lens",
    },
    // 双击判定时间窗（毫秒，全部双击手势共用；TDD-05 §1.2 T_double 可配，非正值运行期回退默认 400）。
    SettingSpec {
        key: "shortcuts.double_tap_window_ms",
        group: SettingGroup::Shortcuts,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Int(400),
        control: SettingControl::Number {
            min: 200,
            max: 1000,
            step: 50,
            unit: SettingUnit::Millisecond,
            style: NumberStyle::Slider,
        },
        requires_restart: false,
        i18n_key: "settings:shortcuts.double_tap_window_ms",
    },
    SettingSpec {
        key: "shortcuts.voice_input",
        group: SettingGroup::Shortcuts,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Str("cmd_or_ctrl+shift+v"),
        control: SettingControl::Shortcut {
            scope: ShortcutScope::Global,
        },
        requires_restart: false,
        i18n_key: "settings:shortcuts.voice_input",
    },
    SettingSpec {
        // 文字速记的全局面板（PRD-06-058 B″）。默认 `Cmd/Ctrl+Shift+Space`：
        // 刻意不用 `Cmd/Ctrl+Shift+N`：它在 macOS Finder 与 Windows 资源管理器都是
        // 「新建文件夹」，注册成全局热键等于把那个键从用户手里抢走。`shift+space` 不在
        // `shortcut::RESERVED_SYSTEM_COMBOS` 里，且贴合「空格即捕获」的既有惯例；
        // `alt+space` 同样出局——Windows 上那是窗口系统菜单。
        key: "shortcuts.text_quick_note",
        group: SettingGroup::Shortcuts,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Str("cmd_or_ctrl+shift+space"),
        control: SettingControl::Shortcut {
            scope: ShortcutScope::Global,
        },
        requires_restart: false,
        i18n_key: "settings:shortcuts.text_quick_note",
    },
    SettingSpec {
        key: "shortcuts.quick_chat",
        group: SettingGroup::Shortcuts,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Str("cmd_or_ctrl+shift+a"),
        control: SettingControl::Shortcut {
            scope: ShortcutScope::Global,
        },
        requires_restart: false,
        i18n_key: "settings:shortcuts.quick_chat",
    },
    SettingSpec {
        key: "shortcuts.quick_switcher",
        group: SettingGroup::Shortcuts,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Str("cmd_or_ctrl+o"),
        control: SettingControl::Shortcut {
            scope: ShortcutScope::InApp,
        },
        requires_restart: false,
        i18n_key: "settings:shortcuts.quick_switcher",
    },
    // 命令面板（PRD-07-041，Obsidian / VS Code 同键）。应用内快捷键全部可改键（W3），与全局热键同一套冲突检测。
    SettingSpec {
        key: "shortcuts.command_palette",
        group: SettingGroup::Shortcuts,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Str("cmd_or_ctrl+p"),
        control: SettingControl::Shortcut {
            scope: ShortcutScope::InApp,
        },
        requires_restart: false,
        i18n_key: "settings:shortcuts.command_palette",
    },
    // 全库搜索（PRD-06-031；避开 ⌘F——那是编辑器的单篇查找）。应用内快捷键全部可改键（W3），与全局热键同一套冲突检测。
    SettingSpec {
        key: "shortcuts.vault_search",
        group: SettingGroup::Shortcuts,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Str("cmd_or_ctrl+shift+f"),
        control: SettingControl::Shortcut {
            scope: ShortcutScope::InApp,
        },
        requires_restart: false,
        i18n_key: "settings:shortcuts.vault_search",
    },
    // 主侧栏收起 / 展开（PRD-07-057）。应用内快捷键全部可改键（W3），与全局热键同一套冲突检测。
    SettingSpec {
        key: "shortcuts.toggle_sidebar",
        group: SettingGroup::Shortcuts,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Str("cmd_or_ctrl+backslash"),
        control: SettingControl::Shortcut {
            scope: ShortcutScope::InApp,
        },
        requires_restart: false,
        i18n_key: "settings:shortcuts.toggle_sidebar",
    },
    // 前往设置（macOS 惯例 ⌘,）。应用内快捷键全部可改键（W3），与全局热键同一套冲突检测。
    SettingSpec {
        key: "shortcuts.open_settings",
        group: SettingGroup::Shortcuts,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Str("cmd_or_ctrl+comma"),
        control: SettingControl::Shortcut {
            scope: ShortcutScope::InApp,
        },
        requires_restart: false,
        i18n_key: "settings:shortcuts.open_settings",
    },
    // 重开刚关闭的标签（PRD-07-035 W2b，浏览器同键）。应用内快捷键全部可改键（W3），与全局热键同一套冲突检测。
    SettingSpec {
        key: "shortcuts.reopen_closed_tab",
        group: SettingGroup::Shortcuts,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Str("cmd_or_ctrl+shift+t"),
        control: SettingControl::Shortcut {
            scope: ShortcutScope::InApp,
        },
        requires_restart: false,
        i18n_key: "settings:shortcuts.reopen_closed_tab",
    },
    // 编辑 ⇄ 阅读切换（PRD-06-017，Obsidian 同键）——编辑器的整篇动作。可改键（W3b），由活跃编辑器判定，与工作区命令同一套冲突检测。
    SettingSpec {
        key: "shortcuts.toggle_reading_view",
        group: SettingGroup::Shortcuts,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Str("cmd_or_ctrl+e"),
        control: SettingControl::Shortcut {
            scope: ShortcutScope::InApp,
        },
        requires_restart: false,
        i18n_key: "settings:shortcuts.toggle_reading_view",
    },
    // AI 操作召唤（PRD-06-020，Notion 同键）——编辑器的整篇动作。可改键（W3b），由活跃编辑器判定，与工作区命令同一套冲突检测。
    SettingSpec {
        key: "shortcuts.ai_composer",
        group: SettingGroup::Shortcuts,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Str("cmd_or_ctrl+j"),
        control: SettingControl::Shortcut {
            scope: ShortcutScope::InApp,
        },
        requires_restart: false,
        i18n_key: "settings:shortcuts.ai_composer",
    },
    SettingSpec {
        key: "editor.autosave_enabled",
        group: SettingGroup::Notes,
        section: Some("editor"),
        scope: SettingScope::Vault,
        default: SettingDefault::Bool(true),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:editor.autosave_enabled",
    },
    // 预览标签（PRD-07-035 2026-10-05 修订，用户裁决 Q3）：浏览打开的标签是「预览」、会被下一次
    // 浏览复用，编辑后转常驻。改变用户习惯，故给开关；它是个人习惯而不是库的属性（Device），
    // 也不是新手需要碰的东西（不进简易层，`is_simple_setting` 不列）。
    SettingSpec {
        key: "editor.preview_tabs",
        group: SettingGroup::Notes,
        section: Some("editor"),
        scope: SettingScope::Device,
        default: SettingDefault::Bool(true),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:editor.preview_tabs",
    },
    SettingSpec {
        key: "editor.autosave_debounce_ms",
        group: SettingGroup::Notes,
        section: Some("editor"),
        scope: SettingScope::Vault,
        default: SettingDefault::Int(1500),
        control: SettingControl::Number {
            min: 200,
            max: 5000,
            step: 100,
            unit: SettingUnit::Millisecond,
            style: NumberStyle::Slider,
        },
        requires_restart: false,
        i18n_key: "settings:editor.autosave_debounce_ms",
    },
    // 粘贴远程图片链接时自动下载为本地附件（PRD-06-012 扩展）。用户显式动作触发的联网，
    // 受 privacy.strict_local 硬开关约束；关闭则远程图片链接保持原样（不下载、不渲染）。
    SettingSpec {
        key: "editor.localize_remote_images",
        group: SettingGroup::Notes,
        section: Some("editor"),
        scope: SettingScope::Vault,
        default: SettingDefault::Bool(true),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:editor.localize_remote_images",
    },
    // 写作视图模式（PRD-06-014，Typora 对标）：打字机模式（当前行垂直居中）与
    // 专注模式（淡化非当前段落）。设备级偏好（写作习惯随设备而非随 Vault）。
    SettingSpec {
        key: "editor.typewriter_mode",
        group: SettingGroup::Notes,
        section: Some("editor"),
        scope: SettingScope::Device,
        default: SettingDefault::Bool(false),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:editor.typewriter_mode",
    },
    SettingSpec {
        key: "editor.focus_mode",
        group: SettingGroup::Notes,
        section: Some("editor"),
        scope: SettingScope::Device,
        default: SettingDefault::Bool(false),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:editor.focus_mode",
    },
    // 格式工具栏显隐（PRD-06-014 页头收口 P1）：熟悉 Markdown 的用户不需要它常驻占一行；
    // 默认开（不熟悉 Markdown 的用户先看到它）。页头「更多」菜单与设置页双入口，设备级偏好。
    SettingSpec {
        key: "editor.show_format_toolbar",
        group: SettingGroup::Notes,
        section: Some("editor"),
        scope: SettingScope::Device,
        default: SettingDefault::Bool(true),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:editor.show_format_toolbar",
    },
    // 西文拼写检查（笔记库对标轮 W6a）：正文与输入框里拼错的西文词画波浪线，编辑器右键给建议。
    // 默认关：中文笔记里夹着的产品名、代码标识符会被大片标红，要的人自己打开。设备级（拼写
    // 词典本就是这台设备的）。中日韩文字不检查（系统拼写检查器本身不管）。
    SettingSpec {
        key: "editor.spellcheck",
        group: SettingGroup::Notes,
        section: Some("editor"),
        scope: SettingScope::Device,
        default: SettingDefault::Bool(false),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:editor.spellcheck",
    },
    // 正文宽度（对标方案 E；Obsidian「可读行宽」/ Typora 版心）：窄 / 中 / 宽三档，默认「中」
    // = 原版心。编辑、阅读与知识库阅读面同一宽度；设备级（屏幕大小随设备）。
    SettingSpec {
        key: "editor.body_width",
        group: SettingGroup::Notes,
        section: Some("editor"),
        scope: SettingScope::Device,
        default: SettingDefault::Str("medium"),
        control: SettingControl::Select {
            options: &["narrow", "medium", "wide"],
            style: SelectStyle::Segmented,
        },
        requires_restart: false,
        i18n_key: "settings:editor.body_width",
    },
    SettingSpec {
        key: "editor.floating_summary_lang",
        group: SettingGroup::Notes,
        section: Some("editor"),
        scope: SettingScope::Device,
        default: SettingDefault::Str("native"),
        control: SettingControl::Select {
            options: &["native", "default_foreign", "zh", "en"],
            style: SelectStyle::Dropdown,
        },
        requires_restart: false,
        i18n_key: "settings:editor.floating_summary_lang",
    },
    // 悬浮菜单「总结」默认风格（PRD-06-020）：要点 / 段落 / 一句话；子菜单可按次覆盖。
    SettingSpec {
        key: "editor.floating_summary_style",
        group: SettingGroup::Notes,
        section: Some("editor"),
        scope: SettingScope::Device,
        default: SettingDefault::Str("bullets"),
        control: SettingControl::Select {
            options: &["bullets", "paragraph", "one_line"],
            style: SelectStyle::Dropdown,
        },
        requires_restart: false,
        i18n_key: "settings:editor.floating_summary_style",
    },
    // 悬浮菜单改写时是否附带选区前后段落（PRD-06-020 上下文策略）：只改选区、前后文仅供
    // 参考，让短句润色不脱离语境；上限与拼装全在 Rust。云路径下它决定出域字数，故用户
    // 可关（AGENTS 铁律：联网行为显式可见可关闭）。
    SettingSpec {
        key: "editor.floating_context",
        group: SettingGroup::Notes,
        section: Some("editor"),
        scope: SettingScope::Device,
        default: SettingDefault::Bool(true),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:editor.floating_context",
    },
    // AI 关系标签（PRD-06-035）：保存/二次转写完成后自动经本地 LLM 提炼主题标签写入
    // `luna.ai_tags`（用户顶层 tags 只读不写），驱动关系图谱连通。编辑器手动入口不受
    // 本开关约束（显式动作恒可用）。
    SettingSpec {
        key: "vault.graph_ai_tags",
        group: SettingGroup::Notes,
        section: Some("editor"),
        scope: SettingScope::Vault,
        default: SettingDefault::Bool(true),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:vault.graph_ai_tags",
    },
    // 对话唤起时自动带入前台应用选区（PRD-03-008：AX 直读优先 / 受保护剪贴板捕获降级；
    // 默认开——US-03-5 核心价值即"免复制粘贴"；带入内容以可移除 chip 呈现，发送前可一键去除）。
    SettingSpec {
        key: "chat.capture_selection",
        group: SettingGroup::Notes,
        section: Some("chat"),
        scope: SettingScope::Device,
        default: SettingDefault::Bool(true),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:chat.capture_selection",
    },
    // 默认把新对话保存到历史（PRD-03-013 长期会话列表；默认开——数据只落本地 Vault 不外发）。
    // 这是**新会话的默认持久化取向**，单次可经对话页「临时对话」开关按会话覆盖（PRD-03-012）；
    // 关闭则新对话默认临时（不落盘、不进历史），仍可单独「保存此对话」。
    SettingSpec {
        key: "chat.save_history",
        group: SettingGroup::Notes,
        section: Some("chat"),
        scope: SettingScope::Device,
        default: SettingDefault::Bool(true),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:chat.save_history",
    },
    // 新对话**默认**要不要检索知识库作答（PRD-16-005 / PRD-03-015）。
    // 这只是默认档、不是主控件：此刻要不要查库由输入区的「知识库」芯片按会话决定，
    // 单次范围还可经透明行就地纠正（PRD-16 D1 的三层范围控制）。想永久关掉的人改这里。
    SettingSpec {
        key: "chat.grounding",
        group: SettingGroup::Notes,
        section: Some("chat"),
        scope: SettingScope::Device,
        default: SettingDefault::Bool(true),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:chat.grounding",
    },
    // 对话终局（开新会话顶替旧的）后要不要把它整理成一篇知识笔记（PRD-03-014 / ADR-024）。
    // 触发点刻意**不是每轮归档**——持久会话每轮都写盘，逐轮整理一段还没有结论的对话毫无意义。
    SettingSpec {
        key: "chat.distill_mode",
        group: SettingGroup::Notes,
        section: Some("chat"),
        scope: SettingScope::Device,
        default: SettingDefault::Str("prompt"),
        control: SettingControl::Select {
            options: DISTILL_MODES,
            style: SelectStyle::Dropdown,
        },
        requires_restart: false,
        i18n_key: "settings:chat.distill_mode",
    },
    SettingSpec {
        key: "vault.delete_mode",
        group: SettingGroup::Storage,
        section: Some("vault"),
        scope: SettingScope::Vault,
        default: SettingDefault::Str("trash"),
        control: SettingControl::Select {
            options: &["trash", "vault_trash", "permanent"],
            style: SelectStyle::Dropdown,
        },
        requires_restart: false,
        i18n_key: "settings:vault.delete_mode",
    },
    SettingSpec {
        key: "vault.trash_retention_days",
        group: SettingGroup::Storage,
        section: Some("vault"),
        scope: SettingScope::Vault,
        default: SettingDefault::Str("30"),
        control: SettingControl::Select {
            // 取值是"天数或永不"，不是一个连续区间——故走枚举而不是 Number：那会要求
            // 为一个设置新造一个 `Day` 单位，且允许 137 天这种没人想要的值。
            options: &["7", "30", "90", "never"],
            style: SelectStyle::Dropdown,
        },
        requires_restart: false,
        i18n_key: "settings:vault.trash_retention_days",
    },
    SettingSpec {
        key: "vault.snapshot_max_versions",
        group: SettingGroup::Storage,
        section: Some("vault"),
        scope: SettingScope::Vault,
        default: SettingDefault::Int(20),
        control: SettingControl::Number {
            min: 1,
            max: 200,
            step: 1,
            unit: SettingUnit::Version,
            style: NumberStyle::Stepper,
        },
        requires_restart: false,
        i18n_key: "settings:vault.snapshot_max_versions",
    },
    // 显示隐藏文件（V4b，PRD-06-063）：只让点文件出现在文件树里，它们仍不进索引与搜索；
    // 是看法偏好而非库的属性，故设备级；新手用不着，不进简易层。
    SettingSpec {
        key: "vault.show_hidden_files",
        group: SettingGroup::Storage,
        section: Some("vault"),
        scope: SettingScope::Device,
        default: SettingDefault::Bool(false),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:vault.show_hidden_files",
    },
    // ── 按笔记类型的归档目录（Vault 相对路径；PRD-09 §2.6 存储/隐私 行）──────────
    // 新建/归档时由 Rust 决定落盘目录（ADR-003）；非法值（绝对路径/越界）回退默认。
    SettingSpec {
        key: "vault.notes_dir",
        group: SettingGroup::Storage,
        section: Some("dirs"),
        scope: SettingScope::Vault,
        default: SettingDefault::Str("Notes"),
        control: SettingControl::Path {
            kind: PathKind::VaultRelativeDir,
        },
        requires_restart: false,
        i18n_key: "settings:vault.notes_dir",
    },
    SettingSpec {
        key: "vault.quick_notes_dir",
        group: SettingGroup::Storage,
        section: Some("dirs"),
        scope: SettingScope::Vault,
        default: SettingDefault::Str("QuickNotes"),
        control: SettingControl::Path {
            kind: PathKind::VaultRelativeDir,
        },
        requires_restart: false,
        i18n_key: "settings:vault.quick_notes_dir",
    },
    SettingSpec {
        key: "vault.recordings_dir",
        group: SettingGroup::Storage,
        section: Some("dirs"),
        scope: SettingScope::Vault,
        default: SettingDefault::Str("Recordings"),
        control: SettingControl::Path {
            kind: PathKind::VaultRelativeDir,
        },
        requires_restart: false,
        i18n_key: "settings:vault.recordings_dir",
    },
    SettingSpec {
        // 媒体转写归档目录（导入音/视频的转写笔记，PRD-05-030）。与录音目录分开——
        // 「我采集的」和「我导入的」是两类产物，混放会让 Vault 在外部编辑器里语义错乱。
        key: "vault.transcripts_dir",
        group: SettingGroup::Storage,
        section: Some("dirs"),
        scope: SettingScope::Vault,
        default: SettingDefault::Str("Transcripts"),
        control: SettingControl::Path {
            kind: PathKind::VaultRelativeDir,
        },
        requires_restart: false,
        i18n_key: "settings:vault.transcripts_dir",
    },
    SettingSpec {
        // 导入文档的归档目录（ADR-164，PRD-06-065）：Word、文本、表格转成的资料笔记与原文件。
        key: "vault.documents_dir",
        group: SettingGroup::Storage,
        section: Some("dirs"),
        scope: SettingScope::Vault,
        default: SettingDefault::Str("Documents"),
        control: SettingControl::Path {
            kind: PathKind::VaultRelativeDir,
        },
        requires_restart: false,
        i18n_key: "settings:vault.documents_dir",
    },
    SettingSpec {
        key: "vault.chats_dir",
        group: SettingGroup::Storage,
        section: Some("dirs"),
        scope: SettingScope::Vault,
        default: SettingDefault::Str("Chats"),
        control: SettingControl::Path {
            kind: PathKind::VaultRelativeDir,
        },
        requires_restart: false,
        i18n_key: "settings:vault.chats_dir",
    },
    SettingSpec {
        // 创作空间的落盘目录（ADR-162 ②）：视频项目文件夹与灵感条目都在它下面。
        // 项目内的脚本跟着项目文件夹走，不另设目录。
        key: "vault.creator_dir",
        group: SettingGroup::Storage,
        section: Some("dirs"),
        scope: SettingScope::Vault,
        default: SettingDefault::Str("Creator"),
        control: SettingControl::Path {
            kind: PathKind::VaultRelativeDir,
        },
        requires_restart: false,
        i18n_key: "settings:vault.creator_dir",
    },
    // 提词稿目录（ADR-176 ②，PRD-20-004）：「新建提词稿」「从剪贴板新建」落在这里；归提词器应用。
    SettingSpec {
        key: "vault.teleprompter_dir",
        group: SettingGroup::Storage,
        section: Some("dirs"),
        scope: SettingScope::Vault,
        default: SettingDefault::Str("Teleprompter"),
        control: SettingControl::Path {
            kind: PathKind::VaultRelativeDir,
        },
        requires_restart: false,
        i18n_key: "settings:vault.teleprompter_dir",
    },
    SettingSpec {
        key: "vault.translate_space_dir",
        group: SettingGroup::Storage,
        section: Some("dirs"),
        scope: SettingScope::Vault,
        default: SettingDefault::Str("TranslateSpace"),
        control: SettingControl::Path {
            kind: PathKind::VaultRelativeDir,
        },
        requires_restart: false,
        i18n_key: "settings:vault.translate_space_dir",
    },
    SettingSpec {
        // 知识产物目录（ADR-096 ②）：新生的主题页/知识地图默认落这里，让「资料→笔记→产物」
        // 的分层在外部编辑器里也看得见。与 `vault.ai_artifacts_dir` 不是一回事——后者是
        // 「单次 AI 结果另存为普通笔记」的落点（生下来就归用户）；产物页由系统持续维护。
        // 存量产物不迁移（身份在 frontmatter，与路径无关）。
        key: "vault.knowledge_dir",
        group: SettingGroup::Storage,
        section: Some("dirs"),
        scope: SettingScope::Vault,
        default: SettingDefault::Str("Knowledge"),
        control: SettingControl::Path {
            kind: PathKind::VaultRelativeDir,
        },
        requires_restart: false,
        i18n_key: "settings:vault.knowledge_dir",
    },
    SettingSpec {
        key: "vault.ai_artifacts_dir",
        group: SettingGroup::Storage,
        section: Some("dirs"),
        scope: SettingScope::Vault,
        default: SettingDefault::Str("AI"),
        control: SettingControl::Path {
            kind: PathKind::VaultRelativeDir,
        },
        requires_restart: false,
        i18n_key: "settings:vault.ai_artifacts_dir",
    },
    // Vault 位置的唯一事实源是设备级启动指针 `storage.vault_root`（`explicit_vault_root` 只读它）；
    // 曾并存的 Vault 级 `vault.root` 无任何生产消费者，两个同名概念摆在同一页上只会让人不知道
    // 该改哪个 —— 已删除（PRD-09-020 归位到 storage.vault_root 一项）。
    SettingSpec {
        key: "vault.templates_dir",
        group: SettingGroup::Storage,
        section: Some("dirs"),
        scope: SettingScope::Vault,
        default: SettingDefault::Str(".luna/templates"),
        control: SettingControl::Path {
            kind: PathKind::VaultRelativeDir,
        },
        // **热生效**。它的每一个消费者（`user_templates_dir`）都从**当次设置快照**读，
        // 从来没有在启动时捕获过——`requires_restart: true` 是界面上的一句假话，
        // 而"标成需重启"的代价是骗用户白重启一次。
        requires_restart: false,
        i18n_key: "settings:vault.templates_dir",
    },
    // ── 附件（图片/粘贴文件）落盘策略（Obsidian「附件默认存放位置」对标，PRD-06-012）──
    // 位置模式：vault_root（仓库根）/ note_folder（笔记同级）/ note_subfolder（笔记同级子目录）
    // / specified_folder（指定库内目录）。落盘目录与回填的**笔记相对链接**由 Rust 单一推导
    // （`attachment_target`，ADR-003），前端仅渲染下拉。
    SettingSpec {
        key: "vault.attachment_location",
        group: SettingGroup::Storage,
        section: Some("attachments"),
        scope: SettingScope::Vault,
        default: SettingDefault::Str("note_subfolder"),
        control: SettingControl::Select {
            options: &[
                "vault_root",
                "note_folder",
                "note_subfolder",
                "specified_folder",
            ],
            style: SelectStyle::Dropdown,
        },
        requires_restart: false,
        i18n_key: "settings:vault.attachment_location",
    },
    // 附件目录名/路径：note_subfolder 模式=子目录名（默认 assets）；specified_folder 模式=
    // 库内相对路径（如 Attachments）。vault_root / note_folder 模式忽略此值。
    SettingSpec {
        key: "vault.attachment_folder",
        group: SettingGroup::Storage,
        section: Some("attachments"),
        scope: SettingScope::Vault,
        default: SettingDefault::Str("assets"),
        control: SettingControl::Path {
            kind: PathKind::VaultRelativeDir,
        },
        requires_restart: false,
        i18n_key: "settings:vault.attachment_folder",
    },
    // ── 录音 / 实时转录（P1，PRD-09 §2.5 / §2.6 录音/实时 行）──────────────────
    // **不登记只有一个合法取值的「设置」**：录音格式（FLAC 编码器待 spike，当前恒 WAV）与
    // 采样率（管线恒 16k，whisper 最优输入）曾各占一行，前者的读取函数忽略取值直接返回 WAV、
    // 后者全仓无消费者——摆出一个改不动的选择器只会让用户以为自己没找对地方。FLAC / 可变
    // 采样率落地时再登记（TDD-02 §3.5）。
    SettingSpec {
        key: "recording.input_device",
        group: SettingGroup::Recording,
        section: Some("capture"),
        scope: SettingScope::Device,
        // 空字符串 = 跟随系统默认输入设备（PRD-09-040）。
        default: SettingDefault::Str(""),
        control: SettingControl::AudioInputDevice,
        requires_restart: false,
        i18n_key: "settings:recording.input_device",
    },
    SettingSpec {
        key: "recording.capture_source",
        group: SettingGroup::Recording,
        section: Some("capture"),
        scope: SettingScope::Device,
        // "input_device" = 麦克风（默认）；"system_loopback" = 系统音频（macOS 14.4+ CATap /
        // Windows WASAPI loopback，PRD-04-052）；"both" = 双轨（mic+system，立体声落盘）。
        default: SettingDefault::Str("input_device"),
        control: SettingControl::Select {
            options: &["input_device", "system_loopback", "both"],
            style: SelectStyle::Dropdown,
        },
        requires_restart: false,
        i18n_key: "settings:recording.capture_source",
    },
    SettingSpec {
        // 剩余磁盘空间预警阈值，**以 GiB 计**（默认 1）。此前以字节存储且从未被读取——
        // 一个要用户在输入框里数 1073741824 的旋钮，既没人会调也没接线（PRD-04 §5）。
        // 现随录音会话注入 `WriterConfig.disk_soft_threshold_bytes`（× 1 GiB）。
        key: "recording.disk_warn_gib",
        group: SettingGroup::Recording,
        section: Some("capture"),
        scope: SettingScope::Device,
        default: SettingDefault::Int(1),
        control: SettingControl::Number {
            min: 1,
            max: 200,
            step: 1,
            unit: SettingUnit::Gibibyte,
            style: NumberStyle::Stepper,
        },
        requires_restart: false,
        i18n_key: "settings:recording.disk_warn_gib",
    },
    SettingSpec {
        // 长录音自动切分的单文件上限，**以 GiB 计**（默认 2，PRD-04 §5）。同上：此前以字节
        // 存储且未接线，writer 一直只用自己的编译期默认。
        key: "recording.rolling_split_gib",
        group: SettingGroup::Recording,
        section: Some("capture"),
        scope: SettingScope::Device,
        default: SettingDefault::Int(2),
        control: SettingControl::Number {
            min: 1,
            max: 16,
            step: 1,
            unit: SettingUnit::Gibibyte,
            style: NumberStyle::Stepper,
        },
        requires_restart: false,
        i18n_key: "settings:recording.rolling_split_gib",
    },
    SettingSpec {
        key: "recording.waveform_fps",
        group: SettingGroup::Recording,
        // 归「采集与录制」节：它是录音 HUD 波形绘制的帧率——录制过程的视觉反馈，与转写无关。
        section: Some("capture"),
        scope: SettingScope::Device,
        // 录音 HUD 波形刷新帧率（fps）；默认 18，ADR-011 / rAF 合并模式。
        default: SettingDefault::Int(18),
        control: SettingControl::Number {
            min: 8,
            max: 30,
            step: 1,
            unit: SettingUnit::Fps,
            style: NumberStyle::Slider,
        },
        requires_restart: false,
        i18n_key: "settings:recording.waveform_fps",
    },
    SettingSpec {
        key: "recording.vad_sensitivity",
        group: SettingGroup::Recording,
        // 归**采集**分节而非 realtime：它描述"麦克风进来的声音怎么判成语音"这件采集行为，
        // 三条采集入口（语音输入 / 录音 / 实时字幕）共用同一颗旋钮，与"实时转写这项任务
        // 怎么配"无关（ADR-077 ⑤）。注册顺序 = 屏幕顺序，故它也必须**物理位于**采集节内。
        section: Some("capture"),
        scope: SettingScope::Device,
        // 语音检测灵敏度：low / normal / high（默认 normal）。值域语义 = **门限高于本轨
        // 环境底噪的 SNR 余量**，不是绝对音量门限——算法与三档余量的事实源是
        // PRD-09 §2.6 / TDD-02 §3.2 / ADR-077，此处不复制。
        default: SettingDefault::Str("normal"),
        control: SettingControl::Select {
            options: &["low", "normal", "high"],
            style: SelectStyle::Segmented,
        },
        requires_restart: false,
        i18n_key: "settings:recording.vad_sensitivity",
    },
    // 语音检测引擎（ADR-012 / ADR-077）：能量 VAD（零模型，默认）或 Silero（神经，需装权重）。
    // 同在采集节、紧挨灵敏度：两者是同一件事的两个旋钮。选了 Silero 而权重不在场时
    // 回落能量并记日志，下一次会话生效。
    SettingSpec {
        key: "recording.vad_engine",
        group: SettingGroup::Recording,
        section: Some("capture"),
        scope: SettingScope::Device,
        default: SettingDefault::Str("energy"),
        control: SettingControl::Select {
            options: &["energy", "silero"],
            style: SelectStyle::Segmented,
        },
        requires_restart: false,
        i18n_key: "settings:recording.vad_engine",
    },
    SettingSpec {
        key: "realtime.transcribe_enabled",
        group: SettingGroup::Recording,
        section: Some("realtime"),
        scope: SettingScope::Device,
        // 录音中实时转写总开关（默认开，PRD-09-012）。
        default: SettingDefault::Bool(true),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:realtime.transcribe_enabled",
    },
    SettingSpec {
        key: "realtime.translate_enabled",
        group: SettingGroup::Recording,
        section: Some("realtime"),
        scope: SettingScope::Device,
        // 实时转译总开关（转写→翻译，默认关；组件已落地，TDD-08 / PRD-04-011）。
        default: SettingDefault::Bool(false),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:realtime.translate_enabled",
    },
    SettingSpec {
        key: "realtime.transcribe_lang",
        group: SettingGroup::Recording,
        section: Some("realtime"),
        scope: SettingScope::Device,
        // "auto" = Whisper 自动语种检测（默认）；可设为 BCP-47 如 "zh"/"en"（PRD-09-040）。
        default: SettingDefault::Str("auto"),
        control: SettingControl::Select {
            options: TRANSCRIBE_LANG_HINTS,
            style: SelectStyle::Dropdown,
        },
        requires_restart: false,
        i18n_key: "settings:realtime.transcribe_lang",
    },
    SettingSpec {
        key: "realtime.translate_target_lang",
        group: SettingGroup::Recording,
        section: Some("realtime"),
        scope: SettingScope::Device,
        // 实时转译**默认外语**（BCP-47）：与 `popup_translate.native_lang`（母语）组成智能双向语言
        // 基准——源==母语→译成本外语，否则→母语。默认 "en"（母语默认 zh，故中文语音→英文，
        // 根治「开转译却仍输出源语言」，PRD-04-011/TDD-08）。录音/字幕面板可现场切换。
        default: SettingDefault::Str("en"),
        control: SettingControl::Select {
            options: TRANSLATE_TARGET_LANGS,
            style: SelectStyle::Dropdown,
        },
        requires_restart: false,
        i18n_key: "settings:realtime.translate_target_lang",
    },
    // 回放高亮粒度（段/词）同理**暂不登记**：词级高亮是 P1 未实现项，只留「段级」一个选项
    // 的下拉是伪开关（PRD-09 §2.5，词级落地时补登记）。
    SettingSpec {
        key: "recording.separate_translation",
        group: SettingGroup::Recording,
        section: Some("realtime"),
        scope: SettingScope::Device,
        // 录音 HUD 的实时译文是否用**独立窗口**显示（复用 live-caption-translation 镜像窗，
        // 与字幕面板的 `caption.separate_translation` 偏好各自独立，PRD-04-044 录音扩展）。
        default: SettingDefault::Bool(false),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:recording.separate_translation",
    },
    // `recording.vad_sensitivity` 不在此处：它是采集行为（判"有没有人在说话"），已随
    // section 一起上移到「采集与录制」节内——注册顺序即屏幕顺序（ADR-077 ⑤）。
    // ── 视频同步回放（PRD-04-058 / PRD-05-023，内嵌视频 + 字幕叠加）──────────────────
    SettingSpec {
        key: "playback.video_layout",
        group: SettingGroup::Recording,
        section: Some("playback"),
        scope: SettingScope::Device,
        // 视频画面在同步回放视图中的呈现方式："split"（默认）= 与转写并排分屏；
        // "theater" = 画面占满内容区（转写暂隐）；"float" = 可拖动悬浮小窗；
        // "audio_only" = 只听声音（画面隐藏，播放不中断）。
        default: SettingDefault::Str("split"),
        control: SettingControl::Select {
            options: &["split", "theater", "float", "audio_only"],
            style: SelectStyle::Dropdown,
        },
        requires_restart: false,
        i18n_key: "settings:playback.video_layout",
    },
    SettingSpec {
        key: "playback.video_subtitles",
        group: SettingGroup::Recording,
        section: Some("playback"),
        scope: SettingScope::Device,
        // 视频画面上是否叠加当前 cue 字幕（内容跟随回放的原文/译文/双语三态，默认开）。
        default: SettingDefault::Bool(true),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:playback.video_subtitles",
    },
    SettingSpec {
        key: "playback.background_video",
        group: SettingGroup::Recording,
        section: Some("playback"),
        scope: SettingScope::Device,
        // 离开同步回放视图后视频画面的去向（PRD-04-067，ADR-118）：
        // "audio_only"（默认）= 只留声音，底部「正在播放」条上可随时把画面要回来；
        // "float" = 画面收成可拖动的悬浮小窗跟着走。
        // 默认落在"只留声音"这一侧：这是一台工作站而不是视频站，用户切走多半是去写
        // 东西，一块自动弹出来的画面会盖住他刚要写的地方。
        default: SettingDefault::Str("audio_only"),
        control: SettingControl::Select {
            options: &["audio_only", "float"],
            style: SelectStyle::Segmented,
        },
        requires_restart: false,
        i18n_key: "settings:playback.background_video",
    },
    SettingSpec {
        key: "playback.subtitle_size",
        group: SettingGroup::Recording,
        section: Some("playback"),
        scope: SettingScope::Device,
        // 视频字幕字号档（与实时字幕面板同一档位集合：small/medium/large/xlarge）。
        default: SettingDefault::Str("medium"),
        control: SettingControl::Select {
            options: &["small", "medium", "large", "xlarge"],
            // 下拉：与字幕字号同一个契约（四档字号），同契约必须同长相——一个分段一个下拉，
            // 用户会以为它们是两种不同的东西。
            style: SelectStyle::Dropdown,
        },
        requires_restart: false,
        i18n_key: "settings:playback.subtitle_size",
    },
    // 停录后台**权威重转写完成**后要不要整理成知识笔记（PRD-04-059 / ADR-024）。
    // 挂在权威文本落定之后——早于此蒸馏等于整理了一份实时预览的草稿。
    SettingSpec {
        key: "recording.distill_mode",
        group: SettingGroup::Recording,
        section: Some("after"),
        scope: SettingScope::Device,
        default: SettingDefault::Str("prompt"),
        control: SettingControl::Select {
            options: DISTILL_MODES,
            style: SelectStyle::Dropdown,
        },
        requires_restart: false,
        i18n_key: "settings:recording.distill_mode",
    },
    // ── 媒体转写（离线，PRD-05）───────────────────────────────────────────────────
    // 与上方 `realtime.*`（录音会话/实时字幕）**严格分离**：离线批量无延迟压力、可用大模型
    // 求准，且用户为省电关掉实时转译时不应静默影响导入结果（PRD-05-031）。
    // **不设「自动翻译」开关**：翻译是显式动作（回放视图按需点「翻译」），不由一个全局
    // 开关替用户决定整批是否翻译；智能双向的语言基准复用 `popup_translate.*`（用户的
    // 母语与默认外语是单一概念，不为离线域再造一份）。
    SettingSpec {
        key: "transcribe.lang",
        group: SettingGroup::Recording,
        section: Some("media"),
        scope: SettingScope::Device,
        // "auto" = Whisper 自动语种检测（默认）；可设为 BCP-47 如 "zh"/"en"。
        // 导入首转 / 二次转写 / 说话人细分嵌入选档共用此语种（同源单一口径）。
        default: SettingDefault::Str("auto"),
        control: SettingControl::Select {
            options: TRANSCRIBE_LANG_HINTS,
            style: SelectStyle::Dropdown,
        },
        requires_restart: false,
        i18n_key: "settings:transcribe.lang",
    },
    SettingSpec {
        key: "transcribe.diarize_auto",
        group: SettingGroup::Recording,
        section: Some("media"),
        scope: SettingScope::Device,
        // 导入转写完成后自动入队说话人细分（默认关：细分要额外跑一遍模型，且不是每种
        // 素材都需要区分说话人）。模型未装时静默跳过，不让导入报错（PRD-05-032）。
        default: SettingDefault::Bool(false),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:transcribe.diarize_auto",
    },
    // 导入/抓取转写完成后要不要整理成知识笔记（PRD-05-034 / ADR-024）。
    SettingSpec {
        key: "transcribe.distill_mode",
        group: SettingGroup::Recording,
        section: Some("media"),
        scope: SettingScope::Device,
        default: SettingDefault::Str("prompt"),
        control: SettingControl::Select {
            options: DISTILL_MODES,
            style: SelectStyle::Dropdown,
        },
        requires_restart: false,
        i18n_key: "settings:transcribe.distill_mode",
    },
    // ── 从链接导入（PRD-05-006/007，ADR-023）──────────────────────────────────────
    // 抓取是**显式动作**（粘链接 → 确认卡 → 开始），故这里只有"下次的初始值"与"环境参数"，
    // 没有任何能替用户静默决定是否联网的开关（PRD-05-031 的教训）。
    SettingSpec {
        key: "transcribe.fetch_media",
        group: SettingGroup::Recording,
        section: Some("media"),
        scope: SettingScope::Device,
        // 默认仅音频：体积常为视频的 1/10~1/50 而转写效果等价；要对着画面校对字幕的人
        // 在确认卡上一键切「视频」，选择随任务走。
        //
        // `link_only`（转写完即删，只留文稿与来源地址）是最省磁盘的一档，但**不作默认**：
        // 源视频下架后连素材都没了，而"知识库自包含、可迁移"是 Vault 的价值主张
        // （PRD-05-004①）。要这个权衡是用户的选择。
        default: SettingDefault::Str("audio"),
        control: SettingControl::Select {
            options: &["audio", "video", "link_only"],
            style: SelectStyle::Dropdown,
        },
        requires_restart: false,
        i18n_key: "settings:transcribe.fetch_media",
    },
    SettingSpec {
        key: "transcribe.fetch_proxy",
        group: SettingGroup::Recording,
        section: Some("media"),
        scope: SettingScope::Device,
        // 空 = 不显式指定代理。GUI 应用继承的是 launchd 的最小环境，`http_proxy` 等
        // 环境变量在此不可用，故必须给一个显式入口，否则需要代理的站点在真机上恒失败。
        default: SettingDefault::Str(""),
        control: SettingControl::Text {
            format: TextFormat::Free,
        },
        requires_restart: false,
        i18n_key: "settings:transcribe.fetch_proxy",
    },
    SettingSpec {
        // 字幕导出默认带 BOM（PRD-11-007）：部分老旧播放器要它才认中文。作为「导出选项」
        // 对话框里 BOM 勾选的默认值，改一次记住。
        key: "export.utf8_bom",
        group: SettingGroup::Recording,
        section: Some("export"),
        scope: SettingScope::Device,
        default: SettingDefault::Bool(false),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:export.utf8_bom",
    },
    // 导出文件命名模板（PRD-11-007）：默认只有笔记名 = 既有行为；占位表与渲染归
    // `luna_core::filename_template`，批量导出仍在此基础上追加序号绝不覆盖。
    SettingSpec {
        key: "export.filename_template",
        group: SettingGroup::Recording,
        section: Some("export"),
        scope: SettingScope::Device,
        default: SettingDefault::Str(luna_core::filename_template::DEFAULT_TEMPLATE),
        control: SettingControl::Text {
            format: TextFormat::ExportFilenameTemplate,
        },
        requires_restart: false,
        i18n_key: "settings:export.filename_template",
    },
    SettingSpec {
        key: "link_import.rights_confirmed",
        group: SettingGroup::System,
        section: Some("tools"),
        scope: SettingScope::Device,
        default: SettingDefault::Bool(false),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:link_import.rights_confirmed",
    },
    SettingSpec {
        key: "transcribe.fetch_tool_path",
        group: SettingGroup::System,
        section: Some("tools"),
        scope: SettingScope::Device,
        // 空 = 自动发现（Homebrew / pipx / MacPorts / PATH，见 `luna_fetch::tool`）。
        // 装在别处的用户在此显式指定——这是唯一需要用户"自己说清楚"的场景。
        default: SettingDefault::Str(""),
        control: SettingControl::Path {
            kind: PathKind::AbsoluteFile,
        },
        requires_restart: false,
        i18n_key: "settings:transcribe.fetch_tool_path",
    },
    SettingSpec {
        key: "labs.translate_space_enabled",
        group: SettingGroup::System,
        section: Some("labs"),
        scope: SettingScope::Device,
        default: SettingDefault::Bool(false),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:labs.translate_space_enabled",
    },
    // ── 知识复利（PRD-16 §3.5）────────────────────────────────────────────────────
    // 触发点是**蒸馏完成这一边界事件**（一篇知识笔记刚诞生），绝不挂 `vault://changed`
    // 之类的高频写盘信号——那会让每敲一个字都排一次分钟级 LLM 任务。
    SettingSpec {
        key: "knowledge.ingest_mode",
        group: SettingGroup::Notes,
        section: Some("knowledge"),
        scope: SettingScope::Device,
        default: SettingDefault::Str("suggest"),
        control: SettingControl::Select {
            options: INGEST_MODES,
            style: SelectStyle::Dropdown,
        },
        requires_restart: false,
        i18n_key: "settings:knowledge.ingest_mode",
    },
    // 语义检索意图属于 Vault；嵌入目标则是设备级模型分配领域，知识页不拥有第二个选择器。
    SettingSpec {
        key: "knowledge.semantic",
        group: SettingGroup::Notes,
        section: Some("knowledge"),
        scope: SettingScope::Vault,
        default: SettingDefault::Bool(true),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:knowledge.semantic",
    },
    // 知识体检的周期提醒（PRD-16-032）。
    // **默认关**，且它只发一条提醒——绝不静默替用户跑深度体检：本地引擎是共享资源，
    // 抢占它去做用户没要求的事是越界（同「批量整理绝不自动开跑」的既有裁决）。
    SettingSpec {
        key: "knowledge.checkup_reminder",
        group: SettingGroup::Notes,
        section: Some("knowledge"),
        scope: SettingScope::Device,
        default: SettingDefault::Bool(false),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:knowledge.checkup_reminder",
    },
    // ── 文本朗读（TTS，PRD-01-042 / PRD-06-042，TDD-05 §9）────────────────────────
    SettingSpec {
        key: "speech.rate",
        group: SettingGroup::VoiceInput,
        section: Some("speech"),
        scope: SettingScope::Device,
        // 朗读语速百分比：50–200（100 = 常速）。控制器映射为 AVSpeechUtterance.rate（0–1）。
        default: SettingDefault::Int(100),
        control: SettingControl::Number {
            min: 50,
            max: 200,
            step: 5,
            unit: SettingUnit::Percent,
            style: NumberStyle::Slider,
        },
        requires_restart: false,
        i18n_key: "settings:speech.rate",
    },
    SettingSpec {
        key: "speech.highlight",
        group: SettingGroup::VoiceInput,
        section: Some("speech"),
        scope: SettingScope::Device,
        // 全文朗读时是否逐句高亮源文并跟随滚动（Live Preview / 源码模式，默认开，PRD-06-042）。
        default: SettingDefault::Bool(true),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:speech.highlight",
    },
    // ── 降级阈值（Advanced，RTF × 100 存储；PRD-04 §4 / TDD-07 §3.1）───────────
    SettingSpec {
        key: "degradation.green_max_rtf",
        group: SettingGroup::System,
        section: Some("diagnostics"),
        scope: SettingScope::Device,
        // RTF 0.60 → 整数 60（乘 100 存储，避免 SettingValue 无 float 限制）。
        default: SettingDefault::Int(60),
        control: SettingControl::Number {
            min: 10,
            max: 300,
            step: 5,
            unit: SettingUnit::Percent,
            style: NumberStyle::Slider,
        },
        requires_restart: false,
        i18n_key: "settings:degradation.green_max_rtf",
    },
    SettingSpec {
        key: "degradation.yellow_max_rtf",
        group: SettingGroup::System,
        section: Some("diagnostics"),
        scope: SettingScope::Device,
        // RTF 0.80 → 整数 80。
        default: SettingDefault::Int(80),
        control: SettingControl::Number {
            min: 10,
            max: 300,
            step: 5,
            unit: SettingUnit::Percent,
            style: NumberStyle::Slider,
        },
        requires_restart: false,
        i18n_key: "settings:degradation.yellow_max_rtf",
    },
    SettingSpec {
        key: "degradation.orange_max_rtf",
        group: SettingGroup::System,
        section: Some("diagnostics"),
        scope: SettingScope::Device,
        // RTF 1.00 → 整数 100（超过此值 → red tier，录完后转写）。
        default: SettingDefault::Int(100),
        control: SettingControl::Number {
            min: 10,
            max: 300,
            step: 5,
            unit: SettingUnit::Percent,
            style: NumberStyle::Slider,
        },
        requires_restart: false,
        i18n_key: "settings:degradation.orange_max_rtf",
    },
    // ── 说话人细分（Advanced，PRD-04-054 / TDD-03 §10.2 / S12 结论 #5）────────────
    SettingSpec {
        key: "diarize.cluster_threshold",
        group: SettingGroup::System,
        section: Some("diagnostics"),
        scope: SettingScope::Device,
        // 聚类阈值 0.70 → 整数 70（×100 存储，同降级阈值先例）；改动即重建细分
        // Provider 热生效。偏低易把同一人裂成多人、偏高易把多人并成一人（阈值敏感，
        // 随真实录音基准校准）。
        default: SettingDefault::Int(70),
        control: SettingControl::Number {
            min: 10,
            max: 95,
            step: 5,
            unit: SettingUnit::Percent,
            style: NumberStyle::Slider,
        },
        requires_restart: false,
        i18n_key: "settings:diarize.cluster_threshold",
    },
    // ── 录音快捷键（Shortcuts，Device，PRD-09-001 / TDD-05 §3）──────────────────
    SettingSpec {
        key: "shortcuts.recording_toggle",
        group: SettingGroup::Shortcuts,
        section: None,
        scope: SettingScope::Device,
        // P1 功能，UI 未启用前禁用/隐藏；键值格式同其他 shortcuts.*。
        default: SettingDefault::Str("cmd_or_ctrl+shift+r"),
        control: SettingControl::Shortcut {
            scope: ShortcutScope::Global,
        },
        requires_restart: false,
        i18n_key: "settings:shortcuts.recording_toggle",
    },
    SettingSpec {
        key: "shortcuts.recording_bookmark",
        group: SettingGroup::Shortcuts,
        section: None,
        scope: SettingScope::Device,
        // 录音中打点快捷键（无默认，空字符串 = 未绑定）。
        default: SettingDefault::Str(""),
        control: SettingControl::Shortcut {
            scope: ShortcutScope::Global,
        },
        requires_restart: false,
        i18n_key: "settings:shortcuts.recording_bookmark",
    },
    // 立即隐藏 / 显示提词器（ADR-176 ⑤，PRD-20-007）：归「提词器」应用，应用没生效时不注册。
    SettingSpec {
        key: "shortcuts.teleprompter_toggle",
        group: SettingGroup::Shortcuts,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Str("ctrl+alt+h"),
        control: SettingControl::Shortcut {
            scope: ShortcutScope::Global,
        },
        requires_restart: false,
        i18n_key: "settings:shortcuts.teleprompter_toggle",
    },
    // 送进提词器（ADR-176 ⑨，PRD-20-021）：选中的文字，没有就剪贴板里的；归「提词器」应用。
    SettingSpec {
        key: "shortcuts.teleprompter_send",
        group: SettingGroup::Shortcuts,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Str("ctrl+alt+p"),
        control: SettingControl::Shortcut {
            scope: ShortcutScope::Global,
        },
        requires_restart: false,
        i18n_key: "settings:shortcuts.teleprompter_send",
    },
    // 提词器浮窗的按键（ADR-176 ⑥，PRD-20-011）：只在浮窗显示时注册，收起即解绑；带修饰，
    // 避开系统的输入法切换（⌃Space / ⌃⌥Space）与切换桌面（⌃← / ⌃→）。
    teleprompter_shortcut(
        "shortcuts.teleprompter_play",
        "settings:shortcuts.teleprompter_play",
        "ctrl+alt+s",
    ),
    teleprompter_shortcut(
        "shortcuts.teleprompter_line_up",
        "settings:shortcuts.teleprompter_line_up",
        "ctrl+alt+up",
    ),
    teleprompter_shortcut(
        "shortcuts.teleprompter_line_down",
        "settings:shortcuts.teleprompter_line_down",
        "ctrl+alt+down",
    ),
    teleprompter_shortcut(
        "shortcuts.teleprompter_block_prev",
        "settings:shortcuts.teleprompter_block_prev",
        "ctrl+alt+pageup",
    ),
    teleprompter_shortcut(
        "shortcuts.teleprompter_block_next",
        "settings:shortcuts.teleprompter_block_next",
        "ctrl+alt+pagedown",
    ),
    teleprompter_shortcut(
        "shortcuts.teleprompter_faster",
        "settings:shortcuts.teleprompter_faster",
        "ctrl+alt+equal",
    ),
    teleprompter_shortcut(
        "shortcuts.teleprompter_slower",
        "settings:shortcuts.teleprompter_slower",
        "ctrl+alt+minus",
    ),
    teleprompter_shortcut(
        "shortcuts.teleprompter_current",
        "settings:shortcuts.teleprompter_current",
        "ctrl+alt+c",
    ),
    teleprompter_shortcut(
        "shortcuts.teleprompter_top",
        "settings:shortcuts.teleprompter_top",
        "ctrl+alt+0",
    ),
    teleprompter_shortcut(
        "shortcuts.teleprompter_close",
        "settings:shortcuts.teleprompter_close",
        "ctrl+alt+escape",
    ),
    SettingSpec {
        key: "shortcuts.live_caption",
        group: SettingGroup::Shortcuts,
        section: None,
        scope: SettingScope::Device,
        // 实时字幕面板全局快捷键（PRD-04-040；键值格式同其他 shortcuts.*）。
        default: SettingDefault::Str("cmd_or_ctrl+shift+k"),
        control: SettingControl::Shortcut {
            scope: ShortcutScope::Global,
        },
        requires_restart: false,
        i18n_key: "settings:shortcuts.live_caption",
    },
    // ── 实时字幕（Caption，Device，PRD-04-040）──────────────────────────────────
    SettingSpec {
        key: "caption.capture_source",
        group: SettingGroup::Translate,
        section: Some("caption_source"),
        scope: SettingScope::Device,
        // 实时字幕默认同时听取麦克风与系统声音，覆盖面对面交流、会议和视频听译三类主场景。
        // 可显式收窄为单麦克风 / 单系统音频；与录音采集源分键，两个任务的偏好互不污染。
        default: SettingDefault::Str("both"),
        control: SettingControl::Select {
            options: &["input_device", "system_loopback", "both"],
            style: SelectStyle::Dropdown,
        },
        requires_restart: false,
        i18n_key: "settings:caption.capture_source",
    },
    SettingSpec {
        key: "caption.translation_enabled",
        group: SettingGroup::Translate,
        section: Some("caption_source"),
        scope: SettingScope::Device,
        // 字幕本身是会话不变量；实时翻译是否启用由设置统一拥有，HUD 不再提供临时复选项。
        default: SettingDefault::Bool(true),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:caption.translation_enabled",
    },
    SettingSpec {
        key: "caption.separate_translation",
        group: SettingGroup::Translate,
        section: Some("caption_source"),
        scope: SettingScope::Device,
        // 译文是否用**独立窗口**显示（true=原文窗仅原文、另开译文窗；false=同窗双语，默认）。
        default: SettingDefault::Bool(false),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:caption.separate_translation",
    },
    SettingSpec {
        key: "caption.font_size",
        group: SettingGroup::Translate,
        section: Some("caption_appearance"),
        scope: SettingScope::Device,
        // 字幕字号档：small/medium/large/xlarge（默认 large，影片字幕偏大）。
        default: SettingDefault::Str("large"),
        control: SettingControl::Select {
            options: &["small", "medium", "large", "xlarge"],
            // 下拉：四档字号在英文下已经贴着分段控件的宽度上限（Small/Medium/Large/Largest），
            // 再加一档或换一种语言就会裂成两排；档位数还会随可读性反馈调整。
            style: SelectStyle::Dropdown,
        },
        requires_restart: false,
        i18n_key: "settings:caption.font_size",
    },
    SettingSpec {
        key: "caption.text_color",
        group: SettingGroup::Translate,
        section: Some("caption_appearance"),
        scope: SettingScope::Device,
        // 字幕文字颜色档（对标播放器字幕：white/yellow/cyan/green）：仅在覆盖视频的沉浸/透明
        // 场景生效（框体模式随主题文本色）。默认 white（影院字幕惯例）。
        default: SettingDefault::Str("white"),
        control: SettingControl::Select {
            options: &["white", "yellow", "cyan", "green"],
            style: SelectStyle::Dropdown,
        },
        requires_restart: false,
        i18n_key: "settings:caption.text_color",
    },
    SettingSpec {
        key: "caption.show_source",
        group: SettingGroup::Translate,
        section: Some("caption_appearance"),
        scope: SettingScope::Device,
        // 翻译开着时原文窗是否也显示原文（ADR-137 H6 补）：false = 只看译文（看外语片的电影
        // 字幕式）。翻译关着时恒显示原文，本项不起作用。HUD 工具药丸的「原文 / 双语 / 译文」
        // 三段与设置页是同一个值。
        default: SettingDefault::Bool(true),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:caption.show_source",
    },
    SettingSpec {
        key: "caption.translation_first",
        group: SettingGroup::Translate,
        section: Some("caption_appearance"),
        scope: SettingScope::Device,
        // 双语时译文在上、原文在下且小一号（电影双语字幕的排法：母语一眼读到，原文作参照）。
        // 默认 false：原文在上（会议场景里原文是主体）。
        default: SettingDefault::Bool(false),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "settings:caption.translation_first",
    },
    SettingSpec {
        key: "caption.max_lines",
        group: SettingGroup::Translate,
        section: Some("caption_appearance"),
        scope: SettingScope::Device,
        // 同屏最多呈现的字幕行数（影片字幕惯例少而大；1–6，默认 3）。
        default: SettingDefault::Int(3),
        control: SettingControl::Number {
            min: 1,
            max: 6,
            step: 1,
            unit: SettingUnit::Line,
            style: NumberStyle::Stepper,
        },
        requires_restart: false,
        i18n_key: "settings:caption.max_lines",
    },
    SettingSpec {
        key: "caption.text_opacity",
        group: SettingGroup::Translate,
        section: Some("caption_appearance"),
        scope: SettingScope::Device,
        // 沉浸模式**文字衬底**不透明度（0–100 百分比，逐行暗底紧贴文字）：0=纯描边无底、
        // 越大暗底越实（默认 55，兼顾亮色视频背景可读与不遮挡画面）。
        default: SettingDefault::Int(55),
        control: SettingControl::Number {
            min: 0,
            max: 100,
            step: 5,
            unit: SettingUnit::Percent,
            style: NumberStyle::Slider,
        },
        requires_restart: false,
        i18n_key: "settings:caption.text_opacity",
    },
    SettingSpec {
        key: "caption.frame_opacity",
        group: SettingGroup::Translate,
        section: Some("caption_appearance"),
        scope: SettingScope::Device,
        // **框体背景**不透明度（0–100 百分比，整块面板暗底衬，字幕条效果）：0=全透明（沉浸，
        // 纯悬浮文字）、越大整条越实。与文字衬底可独立叠加。默认半透明暗底（真机 2026-10-02：
        // 默认全透明时第一次打开看不出窗在哪、浅色内容上字也难认）；要沉浸自己拖到 0。
        default: SettingDefault::Int(60),
        control: SettingControl::Number {
            min: 0,
            max: 100,
            step: 5,
            unit: SettingUnit::Percent,
            style: NumberStyle::Slider,
        },
        requires_restart: false,
        i18n_key: "settings:caption.frame_opacity",
    },
    // ── 字幕窗记下的位置与尺寸（内部键，`is_internal` 前缀 `caption.window_`）──
    // 物理像素；宽 / 高为 0 = 还没记过，按默认定位。
    caption_window_geometry("caption.window_source_x"),
    caption_window_geometry("caption.window_source_y"),
    caption_window_geometry("caption.window_source_width"),
    caption_window_geometry("caption.window_source_height"),
    caption_window_geometry("caption.window_translation_x"),
    caption_window_geometry("caption.window_translation_y"),
    caption_window_geometry("caption.window_translation_width"),
    caption_window_geometry("caption.window_translation_height"),
    // ── 提词器（内部键，`is_internal` 前缀 `teleprompter.`；ADR-162 ⑤ C4a）──
    // 面板上的按钮直接改这几个值；几何与字幕窗同一套记法。
    teleprompter_number("teleprompter.font_size", 40, 20, 120, 2),
    teleprompter_number("teleprompter.line_width", 70, 40, 100, 5),
    teleprompter_number("teleprompter.speed", 40, 5, 200, 5),
    SettingSpec {
        key: "teleprompter.mirror",
        group: SettingGroup::General,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Bool(false),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "teleprompter.mirror",
    },
    caption_window_geometry("teleprompter.window_x"),
    caption_window_geometry("teleprompter.window_y"),
    caption_window_geometry("teleprompter.window_width"),
    caption_window_geometry("teleprompter.window_height"),
    // 尽量不被录屏 / 屏幕共享捕获（ADR-176 ⑤，PRD-20-007）：默认开；只在「提词器」应用生效时起作用。
    SettingSpec {
        key: "teleprompter.capture_hidden",
        group: SettingGroup::General,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Bool(true),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "teleprompter.capture_hidden",
    },
    // 开始前倒计时秒数（ADR-176 ⑤，PRD-20-009）：倒计时结束才开始滚动与跟读。
    SettingSpec {
        key: "teleprompter.countdown_secs",
        group: SettingGroup::General,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Str("3"),
        control: SettingControl::Select {
            options: &["0", "3", "5"],
            style: SelectStyle::Segmented,
        },
        requires_restart: false,
        i18n_key: "teleprompter.countdown_secs",
    },
    // 遥控模式（ADR-176 ⑥，PRD-20-012）：浮窗显示时另注册一组不带修饰的键，给翻页器与脚踏板；
    // 默认关，开着时浮窗上常亮「遥控中」。
    SettingSpec {
        key: "teleprompter.remote_enabled",
        group: SettingGroup::General,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Bool(false),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "teleprompter.remote_enabled",
    },
    // 跟读的总开关（ADR-176 ④，PRD-20-005）：用户开、默认关；关着时面板没有跟读控件、不碰麦克风。
    SettingSpec {
        key: "teleprompter.follow_enabled",
        group: SettingGroup::General,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Bool(false),
        control: SettingControl::Toggle,
        requires_restart: false,
        i18n_key: "teleprompter.follow_enabled",
    },
    // 一方应用的启用状态（ADR-165 ③）：内部键（`is_internal` 前缀 `apps.`），只由「应用」页的
    // 启用 / 停用改；与活动栏布局同一归属层（设备）、同一种值（每个应用一位开关，默认全关）。
    SettingSpec {
        key: crate::apps::APPS_ENABLED_KEY,
        group: SettingGroup::General,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::StringList(crate::apps::APPS_ENABLED_DEFAULT),
        control: SettingControl::OrderedToggleList {
            options: crate::apps::APP_OPTIONS,
            hidden_prefix: crate::apps::APPS_OFF_PREFIX,
            min_visible: 0,
        },
        requires_restart: false,
        i18n_key: "apps.enabled",
    },
    // 一方应用的安装状态（ADR-168 ②）：与 `apps.enabled` 同形；应用生效 = 已安装 ∧ 已启用，
    // 只由「应用」页的安装 / 卸载改。
    SettingSpec {
        key: crate::apps::APPS_INSTALLED_KEY,
        group: SettingGroup::General,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::StringList(crate::apps::APPS_INSTALLED_DEFAULT),
        control: SettingControl::OrderedToggleList {
            options: crate::apps::APP_OPTIONS,
            hidden_prefix: crate::apps::APPS_OFF_PREFIX,
            min_visible: 0,
        },
        requires_restart: false,
        i18n_key: "apps.installed",
    },
];

/// 提词器浮窗的一个按键（ADR-176 ⑥）：全局作用域，但只在浮窗显示时由 Rust 注册。
const fn teleprompter_shortcut(
    key: &'static str,
    i18n_key: &'static str,
    default: &'static str,
) -> SettingSpec {
    SettingSpec {
        key,
        group: SettingGroup::Shortcuts,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Str(default),
        control: SettingControl::Shortcut {
            scope: ShortcutScope::Global,
        },
        requires_restart: false,
        i18n_key,
    }
}

/// 提词器的数值偏好（字号 px、行宽 %、滚动速度 px/s）：内部键，由面板按钮步进。
const fn teleprompter_number(
    key: &'static str,
    default: i64,
    min: i64,
    max: i64,
    step: i64,
) -> SettingSpec {
    SettingSpec {
        key,
        group: SettingGroup::General,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Int(default),
        control: SettingControl::Number {
            min,
            max,
            step,
            unit: SettingUnit::None,
            style: NumberStyle::Stepper,
        },
        requires_restart: false,
        i18n_key: key,
    }
}

/// 字幕窗几何的内部键规格（四个数一组、两扇窗各一组，形态完全相同）。范围放宽到多屏
/// 物理像素坐标的量级（两块 5K 并排，x 就过万）。
const fn caption_window_geometry(key: &'static str) -> SettingSpec {
    SettingSpec {
        key,
        group: SettingGroup::Translate,
        section: None,
        scope: SettingScope::Device,
        default: SettingDefault::Int(0),
        control: SettingControl::Number {
            min: -100_000,
            max: 100_000,
            step: 1,
            unit: SettingUnit::None,
            style: NumberStyle::Stepper,
        },
        requires_restart: false,
        i18n_key: key,
    }
}

/// 按 key 查规格。
#[must_use]
pub fn spec(key: &str) -> Option<&'static SettingSpec> {
    SETTINGS.iter().find(|s| s.key == key)
}

/// 取值被拒绝的原因（**稳定 code**：IPC 原样透出，前端按 code 取本地化文案，不解析人话）。
#[derive(Clone, Debug, PartialEq)]
pub enum Rejection {
    /// 基础类型不符（如给 bool 项写字符串）。
    Type {
        /// 期望的类型名。
        expected: &'static str,
    },
    /// 不在声明的选项集合内。
    NotAnOption,
    /// 越界（含 IPC 可表达范围）。
    OutOfRange {
        /// 下界。
        min: i64,
        /// 上界。
        max: i64,
    },
    /// 不是合法小数或越界。
    NotDecimal {
        /// 下界。
        min: f64,
        /// 上界。
        max: f64,
    },
    /// 路径必须是 Vault 内相对目录（拒绝绝对路径与 `..` 穿越）。
    NotVaultRelative,
    /// 导出命名模板不合法（空 / 花括号不成对 / 未知占位 / 非法字符）。
    BadFilenameTemplate {
        /// 原因（英文机器话，前端只按码取文案）。
        reason: String,
    },
    /// 快捷键语法非法（空组合 / 多主键 / 裸键）。
    ShortcutSyntax,
    /// 保留编辑组合只允许双击模式。
    ShortcutReservedEditing,
    /// 主键不在词表内（录制器认不出的键 / 手改配置写了个注册不上的名字）。
    ShortcutUnknownKey,
    /// 应用内快捷键绑到了编辑器固定占用的组合（[`crate::shortcut::is_editor_reserved_combo`]）：
    /// 编辑器与工作区会同时响应同一次按键。
    ShortcutEditorKey,
    /// 有序多选里出现了重复项（顺序有意义，重复既没有语义也会让条数虚高）。
    DuplicateOption,
    /// 有序多选缺少某一必需类别（PRD-07-068：写作与资料各至少一项）。
    RequiredCategory,
    /// 一段话超过了字数上限。
    TextTooLong {
        /// 上限（字符）。
        max: usize,
    },
}

impl Rejection {
    /// 稳定 code（前端映射本地化文案）。
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Type { .. } => "SETTING_TYPE",
            Self::NotAnOption => "SETTING_OPTION",
            Self::OutOfRange { .. } => "SETTING_RANGE",
            Self::NotDecimal { .. } => "SETTING_DECIMAL",
            Self::NotVaultRelative => "SETTING_PATH",
            Self::BadFilenameTemplate { .. } => "SETTING_FILENAME_TEMPLATE",
            Self::ShortcutSyntax => "SETTING_SHORTCUT",
            Self::ShortcutReservedEditing => "SETTING_SHORTCUT_RESERVED",
            Self::ShortcutUnknownKey => "SETTING_SHORTCUT_KEY",
            Self::ShortcutEditorKey => "SETTING_SHORTCUT_EDITOR",
            // **未知选项与越界复用既有码**（`SETTING_OPTION` / `SETTING_RANGE`）：
            // 它们与既有拒绝理由逐字相同，另起 `SETTING_UNKNOWN_OPTION` /
            // `SETTING_OUT_OF_RANGE` 只是给同一件事第二个名字，而每个调用方都要为此
            // 多写一条一模一样的分支。新码只给真正的新失败模式。
            Self::DuplicateOption => "SETTING_DUPLICATE_OPTION",
            Self::RequiredCategory => "SETTING_REQUIRED_CATEGORY",
            Self::TextTooLong { .. } => "SETTING_TEXT_TOO_LONG",
        }
    }

    /// 供日志/持久化错误的可读原因（**不含用户内容**，只含约束本身）。
    #[must_use]
    pub fn reason(&self) -> String {
        match self {
            Self::Type { expected } => format!("expected {expected}"),
            Self::NotAnOption => "not an allowed option".to_owned(),
            Self::OutOfRange { min, max } => format!("out of range [{min}, {max}]"),
            Self::NotDecimal { min, max } => format!("not a decimal in [{min}, {max}]"),
            Self::NotVaultRelative => "must be a vault-relative directory".to_owned(),
            Self::BadFilenameTemplate { reason } => format!("export filename template: {reason}"),
            Self::ShortcutSyntax => "invalid shortcut syntax".to_owned(),
            Self::ShortcutReservedEditing => {
                "reserved editing combo requires double-tap".to_owned()
            }
            Self::ShortcutUnknownKey => "shortcut main key is not in the vocabulary".to_owned(),
            Self::ShortcutEditorKey => "combo is taken by the editor".to_owned(),
            Self::DuplicateOption => "ordered list must not repeat an option".to_owned(),
            Self::RequiredCategory => {
                "ordered list must keep at least one option of each category".to_owned()
            }
            Self::TextTooLong { max } => format!("text longer than {max} characters"),
        }
    }
}

/// 校验一个值是否满足该设置的控件契约（类型 + 选项 + 范围 + 语法）。
///
/// **单一校验点**：`set_setting`（用户改值）与配置文件加载（`validate_persisted_value`）
/// 共用；此前后端只比对基础类型，`caption.max_lines = -3`、`appearance.theme = "banana"`
/// 之类的值可以直接落进配置文件，再在各消费点以不可预期的方式回退。
///
/// # Errors
/// 值不满足契约时返回具体 [`Rejection`]。
pub fn validate(spec: &SettingSpec, value: &SettingValue) -> Result<(), Rejection> {
    let expect_str = |value: &SettingValue| -> Result<String, Rejection> {
        value
            .as_str()
            .map(str::to_owned)
            .ok_or(Rejection::Type { expected: "str" })
    };
    match spec.control {
        SettingControl::Toggle => {
            value
                .as_bool()
                .ok_or(Rejection::Type { expected: "bool" })?;
        }
        SettingControl::Select { options, .. } => {
            let text = expect_str(value)?;
            if !options.contains(&text.as_str()) {
                return Err(Rejection::NotAnOption);
            }
        }
        SettingControl::Number { min, max, .. } => {
            let n = value.as_int().ok_or(Rejection::Type { expected: "int" })?;
            if n < min || n > max {
                return Err(Rejection::OutOfRange { min, max });
            }
            if !value.fits_i32() {
                return Err(Rejection::OutOfRange { min, max });
            }
        }
        // **留空 = 未配置**，是合法值而不是错误：参考单价一类的项没有一个诚实的
        // 默认数字可填（PRD-07-051 明令不写入会随市场变化的厂商价格），只有"用户还没说"。
        // 消费方据此决定不展示，而不是拿一个编出来的数去算。
        SettingControl::Decimal { min, max, .. } => {
            let text = expect_str(value)?;
            if text.trim().is_empty() {
                return Ok(());
            }
            let parsed: f64 = text
                .trim()
                .parse()
                .map_err(|_| Rejection::NotDecimal { min, max })?;
            if !parsed.is_finite() || parsed < min || parsed > max {
                return Err(Rejection::NotDecimal { min, max });
            }
        }
        SettingControl::ModelAssignment { .. } | SettingControl::AudioInputDevice => {
            expect_str(value)?;
        }
        // 校验的输入**是归一化之后的值**（`normalize` 先跑，见其文档）：这里不再补
        // scheme，只裁决"补完还是不合法"。两件事分开，是为了让"补得出来的"永远不打扰
        // 用户，"补不出来的"永远不静默通过。
        SettingControl::Text { format } => {
            let text = expect_str(value)?;
            match format {
                TextFormat::Free => {}
                TextFormat::Paragraph { max_chars } => {
                    if text.chars().count() > max_chars {
                        return Err(Rejection::TextTooLong { max: max_chars });
                    }
                }
                TextFormat::ContentId => {
                    luna_content_schema::ContentId::parse(&text)
                        .map_err(|_| Rejection::NotAnOption)?;
                }
                TextFormat::ExportFilenameTemplate => {
                    luna_core::filename_template::validate(&text).map_err(|e| {
                        Rejection::BadFilenameTemplate {
                            reason: e.to_string(),
                        }
                    })?;
                }
            }
        }
        SettingControl::Path { kind } => {
            let text = expect_str(value)?;
            if kind == PathKind::VaultRelativeDir && !is_vault_relative_dir(&text) {
                return Err(Rejection::NotVaultRelative);
            }
        }
        SettingControl::OrderedMultiSelect {
            options,
            min,
            max,
            require_each_category,
        } => {
            let items = value.as_string_list().ok_or(Rejection::Type {
                expected: "string_list",
            })?;
            // 四条判据的**顺序**就是用户该先知道哪一件事：id 不认识 → 重复 → 条数
            // → 类别。先报"少了一个写作入口"而其实那个 id 根本拼错了，只会让人绕远路。
            for item in items {
                if !options.contains(&item.as_str()) {
                    return Err(Rejection::NotAnOption);
                }
            }
            let unique: std::collections::BTreeSet<&str> =
                items.iter().map(String::as_str).collect();
            if unique.len() != items.len() {
                return Err(Rejection::DuplicateOption);
            }
            if items.len() < min || items.len() > max {
                return Err(Rejection::OutOfRange {
                    min: min as i64,
                    max: max as i64,
                });
            }
            if require_each_category {
                let has = |category| {
                    items
                        .iter()
                        .any(|id| crate::create_action_category(id) == Some(category))
                };
                if !has(crate::CreateActionCategory::Writing)
                    || !has(crate::CreateActionCategory::Material)
                {
                    return Err(Rejection::RequiredCategory);
                }
            }
        }
        // 有序开关列表只校验形状：内容层面的一切偏差（未知 id / 重复 / 锁定项被隐藏 / 缺项）
        // 都在 [`normalize`] 里按封闭定义域归一——它们不是用户能"写错"的东西（见那里的说明）。
        SettingControl::OrderedToggleList { .. } => {
            value.as_string_list().ok_or(Rejection::Type {
                expected: "string_list",
            })?;
        }
        SettingControl::Shortcut { scope } => {
            let text = expect_str(value)?;
            if text.trim().is_empty() {
                return Ok(()); // 空 = 未绑定，是合法状态
            }
            let binding = crate::shortcut::parse_binding(&text).map_err(|e| match e {
                crate::shortcut::ParseError::Syntax => Rejection::ShortcutSyntax,
                crate::shortcut::ParseError::UnknownKey => Rejection::ShortcutUnknownKey,
            })?;
            if binding.combo.is_reserved_editing() && !binding.double_tap {
                return Err(Rejection::ShortcutReservedEditing);
            }
            // 应用内快捷键由窗口级监听判定，编辑器的键由编辑器自己处理——两者绑同一个组合，
            // 一次按键两边都响（W3）。全局热键在系统层截走按键，不在此列。
            if scope == ShortcutScope::InApp && binding.combo.is_editor_reserved() {
                return Err(Rejection::ShortcutEditorKey);
            }
        }
    }
    Ok(())
}

/// Vault 相对目录判定：空（= 库根）合法；绝对路径、盘符、`..` 穿越一律拒绝。
fn is_vault_relative_dir(path: &str) -> bool {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return true;
    }
    if trimmed.starts_with('/') || trimmed.starts_with('\\') || trimmed.starts_with('~') {
        return false;
    }
    if trimmed.chars().nth(1) == Some(':') {
        return false; // Windows 盘符
    }
    !trimmed
        .split(['/', '\\'])
        .any(|segment| segment == ".." || segment.trim() != segment)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 风险与决策类说明必须常显（2026-10-08 D1）：隐私、删除、联网、上传、计费、库位置、
    /// 需要重启的项，说明里写的正是后果——收进帮助等于把后果藏起来。
    #[test]
    fn risk_settings_keep_inline_descriptions() {
        for spec in SETTINGS {
            let risky = spec.key.starts_with("privacy.")
                || matches!(
                    spec.key,
                    "vault.delete_mode"
                        | "recording.vad_sensitivity"
                        | "recording.vad_engine"
                        | "degradation.orange_max_rtf"
                        | "vault.trash_retention_days"
                        | "storage.vault_root"
                        | "chat.save_history"
                        | "chat.capture_selection"
                        | "editor.floating_context"
                        | "editor.localize_remote_images"
                        | "popup_translate.auto_copy"
                        | "transcribe.fetch_proxy"
                        | "models.download_source"
                        | "knowledge.ingest_mode"
                        | "voice_input.keep_audio"
                        | "voice_input.landing_mode"
                )
                || spec.requires_restart;
            if risky {
                assert_eq!(
                    spec.desc_presentation(),
                    SettingDescPresentation::Inline,
                    "{} 的说明是决策后果，必须常显",
                    spec.key
                );
            }
        }
    }

    /// 名单里的每个键都真实存在且是用户可见的设置行——改名或删键后残留在名单里，
    /// 那一行就悄悄回到常显，而没有任何东西会红。
    #[test]
    fn desc_presentation_lists_name_real_visible_settings() {
        let declared: Vec<&SettingSpec> = SETTINGS
            .iter()
            .filter(|spec| spec.desc_presentation() != SettingDescPresentation::Inline)
            .collect();
        assert!(declared.len() >= 10, "名单不该是空跑的");
        for spec in declared {
            assert!(
                !is_internal(spec.key),
                "{} 是内部键，没有说明可呈现",
                spec.key
            );
        }
        for key in [
            "appearance.language",
            "appearance.font_scale",
            "export.filename_template",
            "degradation.orange_max_rtf",
        ] {
            assert!(
                SETTINGS.iter().any(|spec| spec.key == key),
                "{key} 不在注册表里"
            );
        }
    }

    /// 三种呈现面互斥且穷尽，且**内部键与面板承载不得混淆**。
    ///
    /// 混淆的代价是方向相反的两种：把面板承载的键当成内部键 → 它连搜索都命中不到，用户
    /// 搜「上传」找不到那个隐私开关，结论是"没这功能"；把内部键当成面板承载 → 一堆
    /// `onboarding.*` 进度标记出现在搜索结果里，还带着一个跳不到任何地方的按钮。
    #[test]
    fn every_key_has_exactly_one_surface() {
        for spec in SETTINGS {
            let surface = setting_surface(spec.key);
            assert_eq!(
                surface == SettingSurface::Internal,
                is_internal(spec.key) || is_settings_projection_control(spec.key),
                "{} 的内部性判定与呈现面不一致",
                spec.key
            );
            assert_eq!(
                surface == SettingSurface::Panel,
                is_panel_owned(spec.key)
                    && !is_internal(spec.key)
                    && !is_settings_projection_control(spec.key),
                "{} 的面板承载判定与呈现面不一致",
                spec.key
            );
            assert_eq!(
                matches!(surface, SettingSurface::Row),
                matches!(setting_surface(spec.key), SettingSurface::Row),
                "{} 既不渲染成行、也不属于任何一种隐藏——它会从界面上彻底消失",
                spec.key
            );
        }

        // 面板承载的键确实存在（判据不是恒假），且都落在**有面板的分组**里。
        let panel_keys: Vec<&str> = SETTINGS
            .iter()
            .filter(|s| setting_surface(s.key) == SettingSurface::Panel)
            .map(|s| s.key)
            .collect();
        assert!(
            !panel_keys.is_empty(),
            "一个面板承载的键都没有——判据恒假等于这条机制不存在"
        );
        for key in panel_keys {
            let group = setting_page(key).as_str();
            assert!(
                matches!(
                    group,
                    "appearance" | "ai_services" | "local_models" | "account"
                ),
                "{key} 归给了 {group}，而那个组没有承载它的面板——该项会在界面上彻底消失"
            );
        }
    }

    /// 每个设置的**默认值必须自洽**：默认值本身要能通过自己的控件契约校验。
    /// 否则「恢复默认」会写进一个随即被校验拒绝的值，或加载即判损坏。
    #[test]
    fn every_default_satisfies_its_own_control() {
        for spec in SETTINGS {
            let value = spec.default.to_value();
            assert!(
                validate(spec, &value).is_ok(),
                "{} 的默认值不满足自身控件契约：{:?}",
                spec.key,
                validate(spec, &value)
            );
        }
    }

    /// 控件与基础类型必须匹配（Toggle↔bool / Number↔int / 有序多选↔string_list / 其余↔str）。
    #[test]
    fn control_matches_value_type() {
        for spec in SETTINGS {
            let expected = match spec.control {
                SettingControl::Toggle => "bool",
                SettingControl::Number { .. } => "int",
                SettingControl::OrderedMultiSelect { .. }
                | SettingControl::OrderedToggleList { .. } => "string_list",
                _ => "str",
            };
            assert_eq!(
                spec.default.to_value().kind(),
                expected,
                "{} 的控件与取值类型不符",
                spec.key
            );
        }
    }

    /// 数值控件的区间必须有意义（min < max、step 为正、默认落在区间内）。
    #[test]
    fn number_controls_declare_sane_ranges() {
        for spec in SETTINGS {
            if let SettingControl::Number { min, max, step, .. } = spec.control {
                assert!(min < max, "{} 的区间为空", spec.key);
                assert!(step > 0, "{} 的步长必须为正", spec.key);
            }
            if let SettingControl::Decimal { min, max, step, .. } = spec.control {
                assert!(min < max, "{} 的小数区间为空", spec.key);
                assert!(step > 0.0, "{} 的小数步长必须为正", spec.key);
            }
        }
    }

    #[test]
    fn display_units_preserve_storage_contracts() {
        for spec in SETTINGS {
            if let SettingControl::Number { unit, style, .. } = spec.control {
                assert!(unit.display_divisor() > 0);
                if unit.display_divisor() != 1 {
                    assert_eq!(
                        style,
                        NumberStyle::Slider,
                        "{} scaled editing needs a contract",
                        spec.key
                    );
                }
            }
        }
        let autosave = spec("editor.autosave_debounce_ms").unwrap();
        assert!(matches!(autosave.default, SettingDefault::Int(1500)));
        assert!(validate(autosave, &SettingValue::Int(1500)).is_ok());
        assert!(validate(autosave, &SettingValue::Int(1)).is_err());
        assert_eq!(SettingUnit::Millisecond.display_unit(), "second");
        assert_eq!(SettingUnit::Millisecond.display_divisor(), 1000);
    }

    /// 选项集合不得为空，且不得有重复项（重复项会渲染出两条一样的下拉项）。
    #[test]
    fn select_controls_declare_non_empty_unique_options() {
        for spec in SETTINGS {
            if let SettingControl::Select { options, .. } = spec.control {
                assert!(!options.is_empty(), "{} 的选项集合为空", spec.key);
                let mut sorted = options.to_vec();
                sorted.sort_unstable();
                let len = sorted.len();
                sorted.dedup();
                assert_eq!(sorted.len(), len, "{} 的选项有重复", spec.key);
            }
        }
    }

    /// 语种提示 = `auto` + 翻译目标语言全集（两份常量不许各自漂移）。
    #[test]
    fn lang_hints_extend_translate_targets() {
        assert_eq!(TRANSCRIBE_LANG_HINTS[0], "auto");
        assert_eq!(&TRANSCRIBE_LANG_HINTS[1..], TRANSLATE_TARGET_LANGS);
    }

    /// 说明文案 key 由标签 key 约定派生（一处定义，CI 校验两语种在场）。
    #[test]
    fn desc_key_is_derived_from_label_key() {
        let spec = spec("privacy.strict_local").unwrap();
        assert_eq!(spec.desc_i18n_key(), "settings:privacy.strict_local_desc");
    }

    #[test]
    fn validate_rejects_out_of_contract_values() {
        let lines = spec("caption.max_lines").unwrap();
        assert!(validate(lines, &SettingValue::Int(3)).is_ok());
        assert_eq!(
            validate(lines, &SettingValue::Int(-3)),
            Err(Rejection::OutOfRange { min: 1, max: 6 })
        );
        assert_eq!(
            validate(lines, &SettingValue::Str("3".into())),
            Err(Rejection::Type { expected: "int" })
        );

        let theme = spec("appearance.theme").unwrap();
        assert!(validate(theme, &SettingValue::Str("dark".into())).is_ok());
        assert_eq!(
            validate(theme, &SettingValue::Str("banana".into())),
            Err(Rejection::NotAnOption)
        );

        let cost = spec("provider.asr_cost_per_min").unwrap();
        assert!(validate(cost, &SettingValue::Str("0.006".into())).is_ok());
        assert!(matches!(
            validate(cost, &SettingValue::Str("免费".into())),
            Err(Rejection::NotDecimal { .. })
        ));

        let dir = spec("vault.notes_dir").unwrap();
        assert!(validate(dir, &SettingValue::Str("Notes/2026".into())).is_ok());
        assert_eq!(
            validate(dir, &SettingValue::Str("/etc".into())),
            Err(Rejection::NotVaultRelative)
        );
        assert_eq!(
            validate(dir, &SettingValue::Str("../外面".into())),
            Err(Rejection::NotVaultRelative)
        );

        let hotkey = spec("shortcuts.voice_input").unwrap();
        assert!(validate(hotkey, &SettingValue::Str(String::new())).is_ok());
        assert!(validate(hotkey, &SettingValue::Str("cmd_or_ctrl+shift+v".into())).is_ok());
        assert_eq!(
            validate(hotkey, &SettingValue::Str("v".into())),
            Err(Rejection::ShortcutSyntax)
        );
        assert_eq!(
            validate(hotkey, &SettingValue::Str("cmd_or_ctrl+c".into())),
            Err(Rejection::ShortcutReservedEditing)
        );
        // 词表之外的主键在写入时就被拒（此前一路存进去，到注册那一步才静默失败）。
        assert_eq!(
            validate(hotkey, &SettingValue::Str("cmd_or_ctrl+shift+dead".into())),
            Err(Rejection::ShortcutUnknownKey)
        );
        assert!(validate(
            hotkey,
            &SettingValue::Str("double_tap:cmd_or_ctrl+c".into())
        )
        .is_ok());
    }

    /// 伪开关（只有一个合法取值 / 全仓无消费者）不得回到注册表。
    #[test]
    fn retired_placeholder_keys_stay_unregistered() {
        for key in [
            "recording.format",
            "recording.sample_rate",
            "recording.playback_highlight",
            "notifications.frontmost_suppression",
            "vault.root",
            "provider.default_llm",
            "provider.default_llm_model",
            "provider.realtime_llm_model",
            "provider.default_asr_model_path",
            "provider.batch_asr_model_path",
            "knowledge.embed_model",
            "provider.openai_api_key",
        ] {
            assert!(spec(key).is_none(), "{key} 是已下线的伪开关，不应重新登记");
        }
    }

    #[test]
    fn keys_are_unique() {
        let mut keys: Vec<&str> = SETTINGS.iter().map(|s| s.key).collect();
        keys.sort_unstable();
        let len = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), len, "设置 key 必须唯一");
    }

    #[test]
    fn spec_lookup_and_retired_keys() {
        assert!(spec("privacy.strict_local").is_some());
        assert!(spec("nonexistent").is_none());
        assert!(spec("provider.openai_api_key").is_none());
    }

    #[test]
    fn default_strict_local_is_false() {
        let spec = spec("privacy.strict_local").unwrap();
        assert_eq!(spec.default.to_value(), SettingValue::Bool(false));
    }

    /// 归入档与蒸馏档**结构同构但键名不同**（PRD-16-024）：三档、默认落在中档。
    ///
    /// 中档键名刻意不是 `prompt`：蒸馏的中档骑完成通知，归入的中档用就地建议条且
    /// **不发通知**——两者若同名，读代码的人会理所当然地以为呈现也一样，而那正是
    /// PRD-16-004 明令区分的一点。取值集合被这条断言钉死，改名即红。
    #[test]
    fn ingest_mode_mirrors_distill_modes_but_never_reuses_prompt() {
        let setting = spec("knowledge.ingest_mode").unwrap();
        assert_eq!(setting.group, SettingGroup::Notes);
        assert_eq!(setting.scope, SettingScope::Device);
        assert_eq!(
            setting.default.to_value(),
            SettingValue::Str("suggest".into())
        );
        assert_eq!(INGEST_MODES, &["off", "suggest", "auto"]);
        assert_eq!(INGEST_MODES.len(), DISTILL_MODES.len(), "三档结构同构");
        assert!(!INGEST_MODES.contains(&"prompt"), "归入的中档不叫 prompt");
        match setting.control {
            SettingControl::Select { options, .. } => assert_eq!(options, INGEST_MODES),
            ref other => panic!("归入档必须是枚举控件：{other:?}"),
        }
    }

    /// 嵌入分配只存在于 AI 模型领域；知识页只保留语义意图，绝不复制选择器。
    #[test]
    fn embedding_assignment_is_device_owned_and_semantic_intent_is_vault_owned() {
        let assignment = spec("ai.assignment.default_embedding").unwrap();
        assert_eq!(assignment.group, SettingGroup::ProviderModel);
        assert_eq!(assignment.scope, SettingScope::Device);
        assert!(matches!(
            assignment.control,
            SettingControl::ModelAssignment {
                slot: ModelSlot::DefaultEmbed
            }
        ));
        let toggle = spec("knowledge.semantic").unwrap();
        assert_eq!(toggle.default.to_value(), SettingValue::Bool(true));
        assert_eq!(toggle.group, SettingGroup::Notes);
        assert_eq!(toggle.scope, SettingScope::Vault);
    }

    /// 面板承载 = 「另有一个面在讲同一件事」，不是「和面板同组」。
    ///
    /// 这条曾被违反成：模型分配槽与两项参考单价一起被标成面板承载，而当时没有任何面板
    /// 在画那两项单价——它们既不在列表里也不在任何一面上，等于被删掉了。
    ///
    /// 判据是「有没有面在讲它」，不是「这一组还剩不剩行」（ADR-122 改判）。此前还断言过
    /// 「模型组不应零设置行」，理由是零行的页只能靠面板型豁免存在、漏登记就整页消失——
    /// 那条风险是真的，但 Rust 看不见前端的 `SETTINGS_PANEL_PAGES`，双向强制在
    /// `check-settings-registry.py` §8/§9。在这里再写一遍的代价，是它会拦住
    /// 「把这一页收成零设置行 + 四个面板」这次对的改动。
    #[test]
    fn panel_ownership_means_some_surface_shows_it() {
        for spec in SETTINGS {
            let is_assignment = matches!(spec.control, SettingControl::ModelAssignment { .. });
            if is_assignment {
                assert!(
                    is_panel_owned(spec.key),
                    "{} 是模型分配槽，必须由模型面板独占写入口",
                    spec.key
                );
            }
        }
        // 逐键钉住「哪一面在讲它」——清单本身就是那条判据的可读形式；新增一个面板承载键
        // 而想不出它长在哪一面上，就是这条断言要拦下的那种改动。
        for (key, surface) in [
            ("ai.route_strategy", "算力卡"),
            ("provider.remote_asr_enabled", "算力卡"),
            ("provider.asr_cost_per_min", "执行记录面板"),
            ("provider.llm_cost_per_million_tokens", "执行记录面板"),
            ("storage.models_root", "模型库"),
        ] {
            assert!(spec(key).is_some(), "{key} 应仍在注册表中");
            assert!(
                matches!(setting_surface(key), SettingSurface::Panel),
                "{key} 由「{surface}」承载，应判为面板承载"
            );
        }
    }

    #[test]
    fn translate_space_is_an_opt_in_lab_feature() {
        let setting = spec("labs.translate_space_enabled").unwrap();
        assert_eq!(setting.group, SettingGroup::System);
        assert_eq!(setting.scope, SettingScope::Device);
        assert_eq!(setting.default.to_value(), SettingValue::Bool(false));
        assert_eq!(
            spec("shortcuts.desktop_lens_capture")
                .unwrap()
                .default
                .to_value(),
            SettingValue::Str("cmd_or_ctrl+shift+9".into())
        );
    }

    #[test]
    fn voice_input_settings_are_registered_with_mvp_defaults() {
        assert_eq!(
            spec("voice_input.auto_refine").unwrap().default.to_value(),
            SettingValue::Bool(true)
        );
        assert_eq!(
            spec("voice_input.landing_mode").unwrap().default.to_value(),
            SettingValue::Str("inject".into())
        );
        assert_eq!(
            spec("voice_input.input_mode").unwrap().default.to_value(),
            SettingValue::Str("hold".into())
        );
        assert_eq!(
            spec("voice_input.lang_hint").unwrap().default.to_value(),
            SettingValue::Str("auto".into())
        );
        assert_eq!(
            spec("voice_input.translate_enabled")
                .unwrap()
                .default
                .to_value(),
            SettingValue::Bool(false)
        );
        assert_eq!(
            spec("voice_input.translate_target_lang")
                .unwrap()
                .default
                .to_value(),
            SettingValue::Str("en".into())
        );
        assert_eq!(
            spec("ai.assignment.realtime_asr")
                .unwrap()
                .default
                .to_value(),
            SettingValue::Str(String::new())
        );
    }

    #[test]
    fn vault_settings_are_registered_as_vault_scope() {
        for key in [
            "vault.templates_dir",
            "vault.delete_mode",
            "vault.snapshot_max_versions",
            "vault.ai_artifacts_dir",
            "vault.notes_dir",
            "vault.quick_notes_dir",
            "vault.recordings_dir",
            "vault.chats_dir",
            "vault.creator_dir",
            "vault.teleprompter_dir",
            "vault.translate_space_dir",
            "vault.knowledge_dir",
        ] {
            assert_eq!(spec(key).unwrap().scope, SettingScope::Vault, "{key}");
        }
    }

    #[test]
    fn note_type_dir_settings_have_expected_defaults() {
        for (key, dir) in [
            ("vault.notes_dir", "Notes"),
            ("vault.quick_notes_dir", "QuickNotes"),
            ("vault.recordings_dir", "Recordings"),
            ("vault.chats_dir", "Chats"),
            ("vault.creator_dir", "Creator"),
            ("vault.teleprompter_dir", "Teleprompter"),
            ("vault.translate_space_dir", "TranslateSpace"),
            ("vault.knowledge_dir", "Knowledge"),
        ] {
            assert_eq!(
                spec(key).unwrap().default.to_value(),
                SettingValue::Str(dir.into()),
                "{key}"
            );
        }
    }

    #[test]
    fn phase6_mvp0_settings_are_registered() {
        for key in [
            "onboarding.completed",
            "onboarding.completed_steps",
            "notifications.completed_enabled",
            "appearance.theme",
            "appearance.language",
            "shortcuts.popup_translate",
            "shortcuts.voice_input",
            "shortcuts.quick_chat",
            "shortcuts.quick_switcher",
        ] {
            assert!(spec(key).is_some(), "missing setting: {key}");
        }
    }

    #[test]
    fn workspace_commands_are_rebindable_in_app_shortcuts() {
        // W3：命令注册表里原本写死键位的工作区命令，全部改由设置声明（应用内、设备级）。
        for (key, default) in [
            ("shortcuts.command_palette", "cmd_or_ctrl+p"),
            ("shortcuts.vault_search", "cmd_or_ctrl+shift+f"),
            ("shortcuts.toggle_sidebar", "cmd_or_ctrl+backslash"),
            ("shortcuts.open_settings", "cmd_or_ctrl+comma"),
            ("shortcuts.reopen_closed_tab", "cmd_or_ctrl+shift+t"),
            ("shortcuts.toggle_reading_view", "cmd_or_ctrl+e"),
            ("shortcuts.ai_composer", "cmd_or_ctrl+j"),
        ] {
            let spec = spec(key).unwrap_or_else(|| panic!("missing {key}"));
            assert_eq!(spec.scope, SettingScope::Device);
            assert!(matches!(
                spec.control,
                SettingControl::Shortcut {
                    scope: ShortcutScope::InApp
                }
            ));
            assert_eq!(spec.default.to_value(), SettingValue::Str(default.into()));
            assert!(
                validate(spec, &spec.default.to_value()).is_ok(),
                "{key} 的默认值得过校验"
            );
        }
    }

    #[test]
    fn in_app_shortcuts_cannot_take_editor_keys_but_global_ones_are_not_judged_here() {
        let in_app = spec("shortcuts.command_palette").unwrap();
        assert_eq!(
            validate(in_app, &SettingValue::Str("cmd_or_ctrl+b".into())),
            Err(Rejection::ShortcutEditorKey),
            "⌘B 是编辑器的加粗"
        );
        assert!(validate(in_app, &SettingValue::Str("cmd_or_ctrl+shift+p".into())).is_ok());
        let global = spec("shortcuts.quick_chat").unwrap();
        assert!(validate(global, &SettingValue::Str("cmd_or_ctrl+alt+t".into())).is_ok());
    }

    #[test]
    fn quick_switcher_shortcuts_setting_is_device_scope() {
        let spec = spec("shortcuts.quick_switcher").unwrap();
        assert_eq!(spec.scope, SettingScope::Device);
        assert_eq!(
            spec.default.to_value(),
            SettingValue::Str("cmd_or_ctrl+o".into())
        );
        assert_eq!(spec.i18n_key, "settings:shortcuts.quick_switcher");
    }

    #[test]
    fn editor_autosave_settings_are_vault_scope() {
        let enabled = spec("editor.autosave_enabled").unwrap();
        assert_eq!(enabled.scope, SettingScope::Vault);
        assert_eq!(enabled.default.to_value(), SettingValue::Bool(true));
        assert_eq!(enabled.i18n_key, "settings:editor.autosave_enabled");

        let debounce = spec("editor.autosave_debounce_ms").unwrap();
        assert_eq!(debounce.scope, SettingScope::Vault);
        assert_eq!(debounce.default.to_value(), SettingValue::Int(1500));
        assert_eq!(debounce.i18n_key, "settings:editor.autosave_debounce_ms");
    }

    #[test]
    fn writing_view_modes_are_device_scope_bools_defaulting_off() {
        // PRD-06-014（Typora 对标）：打字机/专注模式为设备级写作偏好，默认关。
        for key in ["editor.typewriter_mode", "editor.focus_mode"] {
            let s = spec(key).unwrap();
            assert_eq!(s.group, SettingGroup::Notes);
            assert_eq!(s.scope, SettingScope::Device);
            assert_eq!(s.default.to_value(), SettingValue::Bool(false));
        }
    }

    #[test]
    fn format_toolbar_visibility_is_device_scope_bool_defaulting_on() {
        // PRD-06-014 页头收口 P1：工具栏显隐是设备级写作偏好，默认开（新手先看到它）。
        let s = spec("editor.show_format_toolbar").unwrap();
        assert_eq!(s.group, SettingGroup::Notes);
        assert_eq!(s.scope, SettingScope::Device);
        assert_eq!(s.default.to_value(), SettingValue::Bool(true));
    }

    #[test]
    fn floating_menu_summary_language_is_device_scope_and_defaults_to_native() {
        let spec = spec("editor.floating_summary_lang").unwrap();
        assert_eq!(spec.group, SettingGroup::Notes);
        assert_eq!(spec.scope, SettingScope::Device);
        assert_eq!(spec.default.to_value(), SettingValue::Str("native".into()));
        assert_eq!(spec.i18n_key, "settings:editor.floating_summary_lang");
    }

    #[test]
    fn floating_menu_summary_style_defaults_to_bullets() {
        let spec = spec("editor.floating_summary_style").unwrap();
        assert_eq!(spec.group, SettingGroup::Notes);
        assert_eq!(spec.scope, SettingScope::Device);
        assert_eq!(spec.default.to_value(), SettingValue::Str("bullets".into()));
    }

    #[test]
    fn floating_menu_context_is_device_scope_toggle_defaulting_on() {
        let spec = spec("editor.floating_context").unwrap();
        assert_eq!(spec.group, SettingGroup::Notes);
        assert_eq!(spec.scope, SettingScope::Device);
        // 高级：默认开且开着就对，改它是省 token 的定制而不是完成常见任务的必经项（PRD-09-072）。
        assert_eq!(spec.visibility(), SettingVisibility::Advanced);
        assert_eq!(spec.default.to_value(), SettingValue::Bool(true));
        assert_eq!(spec.i18n_key, "settings:editor.floating_context");
    }

    #[test]
    fn onboarding_settings_are_device_scope() {
        for key in ["onboarding.completed", "onboarding.completed_steps"] {
            assert_eq!(spec(key).unwrap().scope, SettingScope::Device, "{key}");
        }
    }

    #[test]
    fn appearance_settings_are_device_scope() {
        for key in [
            "appearance.theme",
            "appearance.theme_family",
            "appearance.language",
            "appearance.reduce_motion",
            "appearance.font_scale",
            "appearance.high_contrast",
        ] {
            assert_eq!(spec(key).unwrap().scope, SettingScope::Device, "{key}");
        }
    }

    #[test]
    fn development_schema_has_no_legacy_migrations() {
        assert_eq!(CURRENT_SCHEMA_VERSION, 1);
    }

    // ── Task 8：录音/实时/降级/快捷键 设置注册 ───────────────────────────────

    #[test]
    fn recording_core_settings_are_registered_with_correct_defaults() {
        assert_eq!(
            spec("recording.capture_source").unwrap().default.to_value(),
            SettingValue::Str("input_device".into()),
            "默认采集源应为 input_device"
        );
        assert_eq!(
            spec("recording.vad_sensitivity")
                .unwrap()
                .default
                .to_value(),
            SettingValue::Str("normal".into()),
        );
        assert_eq!(
            spec("recording.waveform_fps").unwrap().default.to_value(),
            SettingValue::Int(18),
        );
    }

    #[test]
    fn realtime_settings_are_registered_with_correct_defaults() {
        assert_eq!(
            spec("realtime.transcribe_enabled")
                .unwrap()
                .default
                .to_value(),
            SettingValue::Bool(true),
            "实时转写默认开"
        );
        assert_eq!(
            spec("realtime.translate_enabled")
                .unwrap()
                .default
                .to_value(),
            SettingValue::Bool(false),
            "实时转译默认关（P1/UI 隐藏）"
        );
        assert_eq!(
            spec("realtime.transcribe_lang").unwrap().default.to_value(),
            SettingValue::Str("auto".into()),
            "默认转写语种为 auto"
        );
        assert_eq!(
            spec("realtime.translate_target_lang")
                .unwrap()
                .default
                .to_value(),
            SettingValue::Str("en".into()),
            "实时转译默认外语为 en（母语默认 zh → 中文语音译成英文，智能双向规避同语）"
        );
    }

    #[test]
    fn caption_capture_defaults_to_both_sources_independently_from_recording() {
        assert_eq!(
            spec("caption.capture_source").unwrap().default.to_value(),
            SettingValue::Str("both".into()),
            "实时字幕默认应同时采集麦克风与系统声音"
        );
        assert_eq!(
            spec("caption.capture_source").unwrap().group,
            SettingGroup::Translate,
        );
        assert_eq!(
            spec("caption.translation_enabled")
                .unwrap()
                .default
                .to_value(),
            SettingValue::Bool(true),
            "实时字幕默认启用翻译，开关由字幕设置单点拥有"
        );
        assert_eq!(
            spec("recording.capture_source").unwrap().default.to_value(),
            SettingValue::Str("input_device".into()),
            "字幕默认不得反向改变录音默认"
        );
    }

    #[test]
    fn recording_disk_thresholds_are_declared_in_gib() {
        // 阈值以 **GiB** 存储（此前是字节，既没人会在输入框里数 1073741824，也从未接线）；
        // 会话启动时 × 1 GiB 注入 writer（`WriterConfig`），故这里锁定单位与默认档位。
        let disk_warn = spec("recording.disk_warn_gib").unwrap();
        let rolling = spec("recording.rolling_split_gib").unwrap();
        assert_eq!(disk_warn.default.to_value(), SettingValue::Int(1));
        assert_eq!(rolling.default.to_value(), SettingValue::Int(2));
        for spec in [disk_warn, rolling] {
            assert!(
                matches!(
                    spec.control,
                    SettingControl::Number {
                        unit: SettingUnit::Gibibyte,
                        ..
                    }
                ),
                "{} 必须以 GiB 为单位呈现",
                spec.key
            );
            assert!(spec.default.to_value().fits_i32());
        }
    }

    #[test]
    fn degradation_rtf_thresholds_are_registered_under_advanced() {
        // RTF 阈值以 ×100 整数存储（0.60→60，0.80→80，1.00→100）。
        assert_eq!(
            spec("degradation.green_max_rtf")
                .unwrap()
                .default
                .to_value(),
            SettingValue::Int(60)
        );
        assert_eq!(
            spec("degradation.yellow_max_rtf")
                .unwrap()
                .default
                .to_value(),
            SettingValue::Int(80)
        );
        assert_eq!(
            spec("degradation.orange_max_rtf")
                .unwrap()
                .default
                .to_value(),
            SettingValue::Int(100)
        );
        // 必须在 Advanced 组（PRD-09 §2.6 "高级" 行含"高负载降级阈值"）。
        for key in [
            "degradation.green_max_rtf",
            "degradation.yellow_max_rtf",
            "degradation.orange_max_rtf",
        ] {
            assert_eq!(
                spec(key).unwrap().group,
                SettingGroup::System,
                "{key} 应在 Advanced 组"
            );
            assert_eq!(
                spec(key).unwrap().scope,
                SettingScope::Device,
                "{key} 应为 Device 作用域"
            );
        }
    }

    #[test]
    fn recording_shortcut_settings_are_device_scope() {
        let toggle = spec("shortcuts.recording_toggle").unwrap();
        assert_eq!(toggle.scope, SettingScope::Device);
        assert_eq!(
            toggle.default.to_value(),
            SettingValue::Str("cmd_or_ctrl+shift+r".into()),
            "录音热键默认值"
        );
        assert_eq!(toggle.group, SettingGroup::Shortcuts);

        let bookmark = spec("shortcuts.recording_bookmark").unwrap();
        assert_eq!(bookmark.scope, SettingScope::Device);
        // 打点热键无默认（空字符串 = 未绑定）。
        assert_eq!(
            bookmark.default.to_value(),
            SettingValue::Str("".into()),
            "打点热键默认应为空（未绑定）"
        );
    }

    #[test]
    fn all_recording_settings_are_device_scope_and_have_i18n_keys() {
        let recording_keys = [
            "recording.input_device",
            "recording.capture_source",
            "recording.vad_sensitivity",
            "recording.disk_warn_gib",
            "recording.rolling_split_gib",
            "recording.waveform_fps",
            "realtime.transcribe_enabled",
            "realtime.translate_enabled",
            "realtime.transcribe_lang",
            "realtime.translate_target_lang",
        ];
        for key in recording_keys {
            let s = spec(key).unwrap_or_else(|| panic!("missing setting: {key}"));
            assert_eq!(
                s.scope,
                SettingScope::Device,
                "{key} should be Device scope"
            );
            assert!(!s.i18n_key.is_empty(), "{key} missing i18n_key");
        }
    }

    // ── W1-T7：DashboardView 估算/展示用设置注册 ───────────────────────────────

    /// 参考单价**默认留空 = 未配置**（PRD-07-051）：不预置任何厂商报价。
    ///
    /// 预置一个的后果是概览拿用户从没设过、也看不到出处的单价算出金额给他看；
    /// 而那两个数还会随市场变化，规格明令不写入它们。
    #[test]
    fn provider_cost_settings_for_dashboard_ship_unconfigured() {
        for key in [
            "provider.asr_cost_per_min",
            "provider.llm_cost_per_million_tokens",
        ] {
            let s = spec(key).unwrap();
            assert_eq!(s.group, SettingGroup::ProviderModel);
            assert_eq!(
                s.default.to_value(),
                SettingValue::Str(String::new()),
                "{key} 不得预置厂商报价"
            );
            assert_eq!(s.i18n_key, format!("settings:{key}"));
        }
    }

    /// 小数控件接受留空（= 未配置），但仍拒非数值与越界。
    #[test]
    fn decimal_accepts_blank_but_still_rejects_garbage() {
        let s = spec("provider.asr_cost_per_min").unwrap();
        assert!(validate(s, &SettingValue::Str(String::new())).is_ok());
        assert!(validate(s, &SettingValue::Str("  ".into())).is_ok());
        assert!(validate(s, &SettingValue::Str("0.01".into())).is_ok());
        assert!(validate(s, &SettingValue::Str("nope".into())).is_err());
        assert!(validate(s, &SettingValue::Str("-1".into())).is_err());
    }

    #[test]
    fn popup_translate_aligns_with_other_global_shortcuts() {
        // 对齐后：弹窗翻译 = 单一 `shortcuts.popup_translate` 键（默认双击复制键，与其余全局
        // 快捷键同构，可切普通/双击模式）+ 全局共用双击时间窗。备用键/双击开关债已删除。
        let gesture = spec("shortcuts.popup_translate").unwrap();
        assert_eq!(gesture.scope, SettingScope::Device);
        assert_eq!(gesture.group, SettingGroup::Shortcuts);
        assert_eq!(
            gesture.default.to_value(),
            SettingValue::Str("double_tap:cmd_or_ctrl+c".into())
        );

        let window = spec("shortcuts.double_tap_window_ms").unwrap();
        assert_eq!(window.default.to_value(), SettingValue::Int(400));

        // 冗余/展示标签债已删除：备用键与双击开关并入统一键；显示一律派生自真实绑定。
        for key in [
            "shortcuts.popup_translate_backup",
            "shortcuts.popup_translate_enabled",
            "shortcuts.popup_translate_label",
            "shortcuts.voice_input_label",
            "shortcuts.quick_chat_label",
        ] {
            assert!(spec(key).is_none(), "{key} 应已删除");
        }
    }

    /// 组内分节契约（`PRD-09-070` ⑦）：声明了分节的分组必须**全员归节**，且同一节在
    /// 注册顺序里**连续**——注册表顺序 = 屏幕顺序，读注册表即读界面。
    ///
    /// 半分节的组会渲染出"游离在任何小节之外的行"（读者不知道它属于谁）；同节不连续
    /// 则屏幕上同一个小节标题出现两次。两种坏形态都在这里编译期挡掉。
    #[test]
    fn sectioned_groups_are_fully_and_contiguously_sectioned() {
        use std::collections::{HashMap, HashSet};
        let mut sectioned_groups: HashSet<SettingGroup> = HashSet::new();
        for spec in SETTINGS {
            if spec.section.is_some() {
                sectioned_groups.insert(spec.group);
            }
        }
        for group in &sectioned_groups {
            let rows: Vec<&SettingSpec> = SETTINGS
                .iter()
                .filter(|s| {
                    s.group == *group && matches!(setting_surface(s.key), SettingSurface::Row)
                })
                .collect();
            let mut seen: HashMap<&str, usize> = HashMap::new();
            let mut previous: Option<&str> = None;
            for spec in rows {
                let section = spec
                    .section
                    .unwrap_or_else(|| panic!("{} 所在组已分节，它却未归入任何一节", spec.key));
                if previous != Some(section) {
                    assert!(
                        !seen.contains_key(section),
                        "{} 的节「{section}」在注册顺序里不连续",
                        spec.key
                    );
                    seen.insert(section, 1);
                }
                previous = Some(section);
            }
        }
        // 反向：本轮刻意分节的三组必须真的分了节（守护自身不空跑）。
        for group in [
            SettingGroup::Storage,
            SettingGroup::Recording,
            SettingGroup::Translate,
        ] {
            assert!(
                sectioned_groups.contains(&group),
                "{group:?} 应已声明组内分节"
            );
        }
    }

    /// 分层判据落地扫描（2026-08-03，`PRD-09-072`）：隐私、数据安全与核心入口必须留在
    /// 标准投影——它们是「不得被高级模式隐藏」的那一类（`PRD-09-073`）。
    #[test]
    fn privacy_and_safety_entries_stay_standard() {
        for key in [
            "privacy.strict_local",
            "privacy.local_knowledge_metrics",
            "storage.vault_root",
            "vault.delete_mode",
            // 判据 C（不可藏）：藏起来有实质风险的两类——无障碍与数据安全。
            // `reduce_motion` / `high_contrast` 是无障碍项；`font_scale` 是看不清时唯一的
            // 补救；`caption.font_size` 同理（字幕看不清时它是唯一的补救，颜色与行数不是）。
            "appearance.reduce_motion",
            "appearance.high_contrast",
            "appearance.font_scale",
            "caption.font_size",
            "speech.highlight",
            // 回收站保留期决定「删掉的东西什么时候彻底没了」——数据安全，不是外观偏好。
            "vault.trash_retention_days",
            "privacy.local_workspace_metrics",
        ] {
            assert_eq!(
                spec(key).unwrap().visibility(),
                SettingVisibility::Standard,
                "{key} 属于隐私/数据安全入口，必须留在标准投影"
            );
        }
    }

    #[test]
    fn simple_projection_has_the_reviewed_budget_and_page_mapping() {
        let simple: Vec<&str> = SETTINGS
            .iter()
            .filter(|spec| !is_internal(spec.key) && !is_settings_projection_control(spec.key))
            .filter(|spec| spec.visibility() == SettingVisibility::Standard)
            .map(|spec| spec.key)
            .collect();
        // 预算上限 30（PRD-09-081）；服务器地址随统一登录移出设置（ADR-155），现为 29。
        assert_eq!(simple.len(), 29, "简易层是经评审的 29 个独立偏好");
        for key in [
            "appearance.theme",
            "appearance.theme_family",
            "storage.vault_root",
            "privacy.strict_local",
            "provider.remote_asr_enabled",
        ] {
            assert!(simple.contains(&key), "{key} 是经评审保留的简易项");
        }
        assert_eq!(setting_page("appearance.theme"), SettingPage::Appearance);
        assert_eq!(setting_page("storage.vault_root"), SettingPage::Notes);
        assert_eq!(setting_page("privacy.strict_local"), SettingPage::Privacy);
        assert_eq!(
            setting_page("transcribe.fetch_proxy"),
            SettingPage::Recording
        );
        assert!(is_internal("transcribe.fetch_tool_path"));
        assert_eq!(
            setting_page("storage.models_root"),
            SettingPage::LocalModels
        );
        let appearance = SETTINGS
            .iter()
            .find(|spec| spec.key == "appearance.theme")
            .unwrap();
        let notes = SETTINGS
            .iter()
            .find(|spec| spec.key == "storage.vault_root")
            .unwrap();
        let service = SETTINGS
            .iter()
            .find(|spec| spec.key == "provider.remote_asr_enabled")
            .unwrap();
        assert_eq!(setting_page_section(appearance), Some("interface"));
        assert_eq!(setting_page_section(notes), Some("vault"));
        assert_eq!(setting_page_section(service), None);
    }

    /// 分层判据落地扫描（2026-08-03，`PRD-09-072`「完成常见任务是否必须理解」）：
    /// 库内布局定制（归档目录 / 模板目录 / 附件放置 / 快照保留上限）、字幕不透明度微调
    /// 与「从链接导入」的下次默认（确认卡上就地可改）都不是常见任务的必经项，归高级投影。
    #[test]
    fn fine_grained_defaults_live_in_advanced_projection() {
        for key in [
            // 2026-09-10 分层重划（PRD-09-072 的 A/B/C/D 判据）：以下均在判据 A
            // 「不改会得到不想要的结果」上不成立——默认值好好的，改与不改都不出错。
            "appearance.density",
            "voice_input.auto_refine",
            "editor.localize_remote_images",
            "editor.typewriter_mode",
            "editor.focus_mode",
            "editor.spellcheck",
            "editor.body_width",
            "editor.floating_summary_lang",
            "editor.floating_summary_style",
            "editor.floating_context",
            "vault.graph_ai_tags",
            "knowledge.checkup_reminder",
            "playback.video_layout",
            "playback.video_subtitles",
            "playback.background_video",
            "playback.subtitle_size",
            "caption.separate_translation",
            "caption.text_color",
            "caption.max_lines",
            "recording.separate_translation",
            "export.utf8_bom",
            "voice_input.quick_note_refine",
            "chat.grounding",
            "chat.distill_mode",
            "notifications.suppress_while_recording",
            // 判据 B 不成立：工作区布局定制不是首月会主动去改的高频偏好。
            "workspace.quick_start_actions",
            "workspace.rail_layout",
            "vault.templates_dir",
            "vault.snapshot_max_versions",
            "vault.ai_artifacts_dir",
            "vault.notes_dir",
            "vault.quick_notes_dir",
            "vault.recordings_dir",
            "vault.transcripts_dir",
            "vault.documents_dir",
            "vault.chats_dir",
            "vault.creator_dir",
            "vault.teleprompter_dir",
            "vault.translate_space_dir",
            "vault.knowledge_dir",
            "vault.attachment_location",
            "vault.attachment_folder",
            "caption.text_opacity",
            "caption.frame_opacity",
            "caption.show_source",
            "caption.translation_first",
            "transcribe.fetch_media",
        ] {
            assert_eq!(
                spec(key).unwrap().visibility(),
                SettingVisibility::Advanced,
                "{key} 是精细化定制项，应归高级投影"
            );
        }
    }
}

#[cfg(test)]
mod ordered_multi_select_tests {
    use super::*;

    fn spec_of(key: &str) -> &'static SettingSpec {
        spec(key).unwrap_or_else(|| panic!("找不到设置：{key}"))
    }

    fn list(items: &[&str]) -> SettingValue {
        SettingValue::StringList(items.iter().map(|s| (*s).to_owned()).collect())
    }

    #[test]
    fn a_legal_ordered_value_is_saved_in_the_order_it_was_given() {
        // 顺序即屏幕顺序：校验不得排序、不得去重、不得补项——它只裁决合不合法。
        let spec = spec_of("workspace.quick_start_actions");
        let value = list(&["import_material", "new_note"]);
        assert!(validate(spec, &value).is_ok());
        assert_eq!(
            value.as_string_list().map(<[String]>::to_vec),
            Some(vec!["import_material".to_owned(), "new_note".to_owned()]),
            "校验不该改动值本身"
        );
    }

    #[test]
    fn every_rejection_has_its_own_stable_code() {
        let spec = spec_of("workspace.quick_start_actions");
        let code = |items: &[&str]| validate(spec, &list(items)).unwrap_err().code();
        assert_eq!(code(&["new_note", "nope"]), "SETTING_OPTION", "未知 id");
        assert_eq!(
            code(&["new_note", "new_note", "import_material"]),
            "SETTING_DUPLICATE_OPTION"
        );
        assert_eq!(code(&["new_note"]), "SETTING_RANGE", "少于下限");
        assert_eq!(
            code(&[
                "text_quick_note",
                "voice_quick_note",
                "new_note",
                "import_material",
                "start_recording"
            ]),
            "SETTING_RANGE",
            "多于上限"
        );
        assert_eq!(
            code(&["text_quick_note", "new_note"]),
            "SETTING_REQUIRED_CATEGORY",
            "全是写作入口"
        );
        assert_eq!(
            code(&["start_recording", "import_material"]),
            "SETTING_REQUIRED_CATEGORY",
            "全是资料入口"
        );
    }

    #[test]
    fn a_wrong_shaped_value_is_a_type_error_not_a_silent_pass() {
        // 给它写一个字符串（比如有人把它当逗号串用）必须当场拒绝——静默接受的话，
        // 消费面拿到的是一个 `as_string_list() == None` 的值，界面上表现为"快速开始空了"。
        let spec = spec_of("workspace.quick_start_actions");
        let err =
            validate(spec, &SettingValue::Str("new_note,import_material".into())).unwrap_err();
        assert_eq!(err.code(), "SETTING_TYPE");
    }

    #[test]
    fn the_creator_board_always_keeps_at_least_one_column() {
        // 看板列（ADR-162 ⑨）：全部收起写进来，落盘的值仍留一列——列序最前的那一列。
        let spec = spec_of("creator.board_stages");
        let all_hidden: Vec<String> = crate::creator_board::CREATOR_STAGE_IDS
            .iter()
            .map(|id| format!("hidden:{id}"))
            .collect();
        let written = normalize(spec, SettingValue::StringList(all_hidden));
        let tokens = written.as_string_list().unwrap();
        assert_eq!(tokens.len(), crate::creator_board::CREATOR_STAGE_IDS.len());
        assert_eq!(tokens[0], "idea");
        assert!(tokens[1..].iter().all(|t| t.starts_with("hidden:")));
        // 默认：七列全显示。
        assert_eq!(
            spec.default.to_value(),
            SettingValue::StringList(
                crate::creator_board::CREATOR_BOARD_DEFAULT
                    .iter()
                    .map(|s| (*s).to_owned())
                    .collect()
            )
        );
    }

    #[test]
    fn the_rail_layout_normalizes_to_the_full_set_and_only_rejects_the_wrong_shape() {
        // 有序开关列表：形状不对才拒绝；内容偏差一律归一（ADR-087 ⑥）。
        let spec = spec_of("workspace.rail_layout");
        assert_eq!(
            validate(spec, &SettingValue::Str("search,notes".into()))
                .unwrap_err()
                .code(),
            "SETTING_TYPE"
        );
        // 隐藏了锚点、丢了几项、混进一个已退役 id、还重复了一次——写进来后是一份完整全集。
        let written = normalize(
            spec,
            list(&["hidden:notes", "chat", "materials", "hidden:search", "chat"]),
        );
        assert!(validate(spec, &written).is_ok());
        assert_eq!(
            written.as_string_list().map(<[String]>::to_vec),
            Some(
                [
                    "notes",
                    "knowledge",
                    "quick_notes",
                    "recordings",
                    "transcripts",
                    "chat",
                    "creator",
                    "teleprompter",
                    "hidden:search"
                ]
                .iter()
                .map(|s| (*s).to_owned())
                .collect::<Vec<_>>()
            ),
            "锚点强制显示、退役 id 丢弃、重复只留首次、缺项补在其默认前驱之后、按默认可见性补回"
        );
        // 已是完整全集的合法值原样保存（顺序即屏幕顺序，不得被"归一化"成默认顺序）。
        let kept = list(&[
            "chat",
            "transcripts",
            "recordings",
            "quick_notes",
            "knowledge",
            "notes",
            "creator",
            "teleprompter",
            "hidden:search",
        ]);
        assert_eq!(normalize(spec, kept.clone()), kept);
    }

    #[test]
    fn the_ordering_of_the_four_checks_tells_the_user_the_first_real_problem() {
        // 一个既拼错又重复又超限的值，先说"这个 id 不认识"——先报"少了一个写作入口"
        // 而其实 id 根本拼错了，只会让人绕远路。
        let spec = spec_of("workspace.quick_start_actions");
        assert_eq!(
            validate(spec, &list(&["nope", "nope"])).unwrap_err().code(),
            "SETTING_OPTION"
        );
    }

    #[test]
    fn a_paragraph_is_trimmed_keeps_its_line_breaks_and_has_a_length_cap() {
        let voice = spec_of("creator.channel_voice");
        let normalized = normalize(
            voice,
            SettingValue::Str("  面向程序员\r\n说话直接  \n".into()),
        );
        assert_eq!(normalized, SettingValue::Str("面向程序员\n说话直接".into()));
        assert!(validate(voice, &normalized).is_ok());
        let long = SettingValue::Str("字".repeat(1001));
        assert_eq!(
            validate(voice, &long).unwrap_err(),
            Rejection::TextTooLong { max: 1000 }
        );
    }

    #[test]
    fn the_theme_family_takes_any_well_formed_content_id() {
        // 主题 id 随导入而变（ADR-169 ④），故不是静态选项；写法不合规的才拒绝。
        let family = spec_of("appearance.theme_family");
        for ok in ["luna", "nebula", "com.example.bay"] {
            let value = normalize(family, SettingValue::Str(format!(" {ok} ")));
            assert!(validate(family, &value).is_ok(), "{ok}");
        }
        for bad in ["", "Luna", "a\"]{}", "../x"] {
            assert_eq!(
                validate(family, &SettingValue::Str(bad.into())).unwrap_err(),
                Rejection::NotAnOption,
                "{bad}"
            );
        }
    }
}
