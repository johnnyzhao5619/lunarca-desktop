//! `luna-app` 库入口（Tauri 2 装配）。
//!
//! 物理布局：本 crate 承担标准 `src-tauri` 角色（TDD-00 §3）。Phase 0 仅打通一条
//! tauri-specta 往返；业务逻辑全在 Rust（ADR-003），前端零业务。

mod account_scope;
pub mod arbiter;
/// 工件下载：模型安装「取字节到本地文件」层的 trait 与直连默认实现。
pub mod artifact_fetcher;
/// 应用装配（ADR-037 后：单一构建的默认装配；`run_with` 仍供测试与嵌入方定制）。
pub mod assembly;
/// 云客户端装配与门控（ADR-037：未登录时零云网络行为）。
pub mod cloud;
/// 制品面工件下载适配器（云门控允许时由 [`cloud`] 委派）。
pub mod cloud_artifact;
mod cloud_bearer;
pub mod cloud_sync;
mod components;
mod device_state;
/// 说话人细分 Provider（sherpa-onnx，跨平台，ADR-020）。
pub mod diarize_provider;
pub mod fetch_tool;
mod hardware;
pub mod i18n;
mod ipc;
mod silero_vad;
pub mod sync_service;
// 门禁模块：只在测试期编译。放在 src/ 而非 src/ipc/，故它自己的违规样本
// 结构性地落在扫描面之外——守护不该扫到自己。
mod boot_appearance;
mod editor_exit;
#[cfg(test)]
mod ipc_error_copy;
mod knowledge_shadow;
mod knowledge_source_grant;
mod local_engine;
mod media_server;
mod metrics;
mod metrics_authorization;
pub mod model_import;
pub mod model_store;
mod model_target;
pub mod notifications;
/// macOS Vision OCR Provider（截屏翻译 OCR 环节走单一 AI 主干，ADR-001）。仅 macOS 编译。
#[cfg(target_os = "macos")]
pub mod ocr_provider;
pub mod onboarding_funnel;
/// 转写 / 翻译 / 说话人细分的**执行核心**（导入首转 + 二次转写 + 二次翻译 + 细分四路共用）。
pub mod pipeline;
pub mod platform;
pub mod popup;
pub mod processing_queue;
pub mod recording;
pub mod resource_monitor;
pub mod screenshot;
mod secrets;
pub mod startup;
pub mod tts;
pub mod usage;
mod vault_lists;
mod vault_location;
pub mod voice_input;
pub mod whisper_backend;
mod workspace_metrics;

pub use assembly::AppAssembly;
pub use ipc::builder;

/// 生成的 TS 绑定落位（提交入库 + CI `git diff` 校验无漂移，TDD-13 §2）。
///
/// 用 `CARGO_MANIFEST_DIR` 构造绝对路径——与 `cargo run` 的 CWD 无关（CWD 可能是
/// workspace 根而非 `src-tauri`），CI/本地导出落点一致。
pub const BINDINGS_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../apps/desktop/src/ipc/bindings.ts"
);

/// 随包的内置主题样式表（ADR-169 ③）：由主题包数据生成、提交入库，与生成器不一致时
/// `ipc::themes` 的测试判红；随 `export-bindings` 一起重新生成。
pub const THEMES_CSS_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../apps/desktop/src/styles/tokens/themes.generated.css"
);

// 非激活 HUD 的首帧/最小几何集中定义。首帧须在前端异步布局前完整容纳默认界面；
// 最小值只保证可操作下限。显示后根视图填满用户可缩放窗口，内容只在放不下时向下扩容，
// 不再由前端回写固定宽度。
#[derive(Debug, Clone, Copy, PartialEq)]
struct HudWindowGeometry {
    initial_width: f64,
    initial_height: f64,
    min_width: f64,
    min_height: f64,
}

const POPUP_TRANSLATE_HUD_GEOMETRY: HudWindowGeometry = HudWindowGeometry {
    // 输入态首帧只容输入卡（ADR-112 ④：结果区首 token 才出现），随结果向下扩容。
    initial_width: 500.0,
    initial_height: 200.0,
    min_width: 360.0,
    min_height: 160.0,
};

/// 每窗每档的首帧高度（ADR-112 ③：档位几何归 Rust；跨档跃迁由 `hud_set_tier` 执行 `set_size`）。
///
/// 只给高度：宽度沿用窗口当前宽（用户拉宽过的尺寸在换档时仍被尊重）。每档高度须
/// **不小于该档内容的最小高**——真机 spike（2026-09-08）实测把语音窗设到 52 高时前端自动扩容
/// 立刻把它撑回 80，档位几何小于内容等于白设一次。档内仍只向下扩容、不缩。
pub(crate) fn hud_tier_height(label: &str, tier: &str) -> Option<f64> {
    Some(match (label, tier) {
        // 语音（ADR-137 / PRD-02-018）：pill = 一行（空闲 / 说话 / 处理，首帧同
        // VOICE_INPUT_HUD_GEOMETRY）；card = 一行 + 结果编辑区或出错卡的**下限**，更高的内容由档内
        // 扩容撑开；panel = 一行 + 配置（收起是跃迁回 pill/card，档内只扩不缩——ADR-112 ③）。
        ("voice-input-hud", "pill") => VOICE_INPUT_HUD_GEOMETRY.initial_height,
        ("voice-input-hud", "card") => 150.0,
        ("voice-input-hud", "panel") => 320.0,
        // 文字速记：pill = 一行输入（首帧）；card = 写了多行 / 不可用 / 没存上时的下限。
        // 两档都只是**下限**，档内随内容向下扩容（ADR-112 ③）。存下的回执在 pill 行内，
        // 不为一句「已存」跃迁档位——此前敲一个字窗口就从 148 跳到 320。
        ("text-quick-note", "pill") => TEXT_QUICK_NOTE_HUD_GEOMETRY.initial_height,
        ("text-quick-note", "card") => 120.0,
        // 弹窗翻译：input = 只有输入卡（首帧）；result = 结果区出现后的基准，随内容再扩。
        ("popup-translate", "input") => POPUP_TRANSLATE_HUD_GEOMETRY.initial_height,
        // result 只是下限：译文框按内容高（ADR-137 V7），长段落由档内扩容撑开——此前 440 起步，
        // 翻一个词也是一大块。
        ("popup-translate", "result") => 260.0,
        // 录音：bar = 纯录音控制条；card = 带字幕画布（首帧）；panel = 设置展开。
        ("recording-hud", "bar") => 56.0,
        ("recording-hud", "card") => RECORDING_HUD_GEOMETRY.initial_height,
        // panel（设置展开 / 菜单开着）：选项面板的高度随内容扩容，收起时缩回 card / bar。
        ("recording-hud", "panel") => 300.0,
        _ => return None,
    })
}

/// HUD 窗口标题（系统窗口列表 / Mission Control / 辅助功能树可见，随界面语言）。
/// 无边框窗自己不画标题，但读屏会念它——此前七扇窗写死中文。
fn hud_window_title<M: tauri::Manager<tauri::Wry>>(
    manager: &M,
    key: &str,
    fallback: &str,
) -> String {
    ipc::backend_text(manager.app_handle(), key, fallback)
}

/// 界面语言切换后给已存在的 HUD 窗口换标题（`set_backend_language` 调用）。
/// 截屏框选层是按屏临时建的，不在此列——它下次创建时自然取新语言。
pub(crate) fn retitle_hud_windows(app: &tauri::AppHandle) {
    use tauri::Manager as _;
    const TITLES: &[(&str, &str, &str)] = &[
        (
            "popup-translate",
            "window-title-popup-translate",
            "Lunarca 弹窗翻译",
        ),
        (
            "voice-input-hud",
            "window-title-voice-input",
            "Lunarca 语音输入",
        ),
        ("quick-chat", "window-title-quick-chat", "Lunarca 快捷对话"),
        (
            TEXT_QUICK_NOTE_WINDOW,
            "window-title-text-quick-note",
            "Lunarca 速记",
        ),
        ("recording-hud", "window-title-recording", "Lunarca 录音"),
        (
            ipc::TELEPROMPTER_LABEL,
            "window-title-teleprompter",
            "Lunarca 提词器",
        ),
        (
            DESKTOP_LENS_WINDOW,
            "window-title-desktop-lens",
            "Lunarca 译境",
        ),
        (
            DESKTOP_LENS_TRIGGER,
            "window-title-desktop-lens-trigger",
            "Lunarca 译境控制",
        ),
    ];
    for (label, key, fallback) in TITLES {
        if let Some(win) = app.get_webview_window(label) {
            let _ = win.set_title(&hud_window_title(app, key, fallback));
        }
    }
}
const VOICE_INPUT_HUD_GEOMETRY: HudWindowGeometry = HudWindowGeometry {
    // 胶囊一行（ADR-137 V7）：圆钮 32 + 上下内距 10 + 细边 = 54；宽只够提示语与落点胶囊并排。
    // 结果 / 出错到达再跃迁 card 并随内容向下扩容。
    initial_width: 380.0,
    initial_height: 56.0,
    min_width: 320.0,
    min_height: 52.0,
};
// 快捷对话 HUD 只在 macOS 建窗；几何常量的用例全平台跑。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const QUICK_CHAT_HUD_GEOMETRY: HudWindowGeometry = HudWindowGeometry {
    initial_width: 560.0,
    initial_height: 520.0,
    min_width: 440.0,
    min_height: 400.0,
};
/// 文字速记面板的窗口标签（前端 `App.tsx` 按同一字面量路由；改这里必须同改那边）。
pub(crate) const TEXT_QUICK_NOTE_WINDOW: &str = "text-quick-note";

/// 文字速记面板（PRD-06-058 B″）。首帧只有窗口栏 + 一行输入 + 脚注（pill 档）；
/// 多行输入由档内扩容撑开，`card` 档留给"存下之后还想接着写"的那个高度。
const TEXT_QUICK_NOTE_HUD_GEOMETRY: HudWindowGeometry = HudWindowGeometry {
    // 启动器式一行（ADR-137 / PRD-06-058 B″）：身份图标 · 输入框 · 存下回执 · 关闭；
    // 键盘提示只给读屏，脚注退役。多行与出错跃迁 card，随内容向下扩容。
    initial_width: 560.0,
    initial_height: 56.0,
    min_width: 420.0,
    min_height: 52.0,
};
const RECORDING_HUD_GEOMETRY: HudWindowGeometry = HudWindowGeometry {
    // 控制条一行（ADR-137 H5，Loom 式）：「● 计时 · 内嵌波形 · 暂停 · 书签 · 停止 · ⋯ · ✕」；
    // 首帧是带最近两条字幕的卡片档（实时转写默认开），纯录音缩到一行。
    initial_width: 420.0,
    initial_height: 150.0,
    min_width: 380.0,
    min_height: 52.0,
};
/// 字幕窗：首帧 = 工具药丸 + 两行双语字幕（真机 2026-10-02：「默认窗口太高」——420 高时
/// 字幕只占底下一小条，上面大半是空的，分两扇窗时还放不下第二扇）。外观面板在矮窗里自己滚动。
const LIVE_CAPTION_HUD_GEOMETRY: HudWindowGeometry = HudWindowGeometry {
    initial_width: 820.0,
    initial_height: 280.0,
    min_width: 480.0,
    min_height: 200.0,
};

/// 提词器（ADR-162 ⑤）：竖长的阅读面，放在镜头附近；最小尺寸容纳控制条 + 三行字。
// 初始宽要放得下整条工具栏（跟读、滚动、标记、三组步进、镜像、回到开头、收起）不折行：
// 720 时中文界面折成两行、收起钮掉到第二行（真机 2026-10-17）；英文文案更长，取 880。
const TELEPROMPTER_GEOMETRY: HudWindowGeometry = HudWindowGeometry {
    initial_width: 880.0,
    initial_height: 520.0,
    min_width: 420.0,
    min_height: 260.0,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GlobalShortcutAction {
    VoiceInput,
    TextQuickNote,
    PopupTranslate,
    ScreenshotTranslate,
    DesktopLensToggle,
    DesktopLensCapture,
    QuickChat,
    RecordingToggle,
    RecordingBookmark,
    LiveCaption,
}

/// 全局快捷键「设置键 → 动作」映射。键值取自 `luna-config` 注册表与用户设置（默认见
/// TDD-05 §3），改键经 `reregister_global_shortcuts` 重注册，此处不写键位。
///
/// 每个键接受两种值语法：普通组合（`cmd_or_ctrl+shift+v`）走全局热键插件；
/// `double_tap:<组合>` 进事件 tap 的双击手势表（`apply_double_tap_settings`，下标即绑定 id）。
/// 注册表里声明为全局的快捷键必须与本表一一对应，由用例钉住。
const GLOBAL_SHORTCUT_SETTINGS: &[(&str, GlobalShortcutAction)] = &[
    (
        "shortcuts.popup_translate",
        GlobalShortcutAction::PopupTranslate,
    ),
    (
        "shortcuts.screenshot_translate",
        GlobalShortcutAction::ScreenshotTranslate,
    ),
    (
        "shortcuts.desktop_lens",
        GlobalShortcutAction::DesktopLensToggle,
    ),
    (
        "shortcuts.desktop_lens_capture",
        GlobalShortcutAction::DesktopLensCapture,
    ),
    ("shortcuts.voice_input", GlobalShortcutAction::VoiceInput),
    (
        "shortcuts.text_quick_note",
        GlobalShortcutAction::TextQuickNote,
    ),
    ("shortcuts.quick_chat", GlobalShortcutAction::QuickChat),
    (
        "shortcuts.recording_toggle",
        GlobalShortcutAction::RecordingToggle,
    ),
    (
        "shortcuts.recording_bookmark",
        GlobalShortcutAction::RecordingBookmark,
    ),
    ("shortcuts.live_caption", GlobalShortcutAction::LiveCaption),
];

/// 某设置键是否为**全局**快捷键（改它需在 Rust 侧重注册；应用内快捷键由前端命令注册表管，不在此列）。
#[must_use]
pub fn is_global_shortcut_setting(key: &str) -> bool {
    GLOBAL_SHORTCUT_SETTINGS.iter().any(|(k, _)| *k == key)
}

/// 双击手势附加设置键（时间窗全局共用，PRD-09 §2.6）。绑定键本身
/// （`GLOBAL_SHORTCUT_SETTINGS`）改动也会重建手势表（见 ipc set/reset 钩子）。
const DOUBLE_TAP_EXTRA_SETTINGS: &[&str] = &["shortcuts.double_tap_window_ms"];

/// 某设置键是否影响双击手势表（改它需热应用到 [`platform::DoubleTapConfig`]）。
#[must_use]
pub fn is_double_tap_setting(key: &str) -> bool {
    DOUBLE_TAP_EXTRA_SETTINGS.contains(&key)
}

/// **保留编辑组合**（`Cmd/Ctrl+{C,V,X,A,Z}`）判定——转出配置层的单一实现。
///
/// 普通模式注册这些键会劫持系统复制/粘贴，故只允许双击模式；校验（`set_setting`）、
/// 注册路径与设置 UI 共用 `luna_config::shortcut` 的同一份判定，不再各写一遍。
pub use luna_config::shortcut::is_reserved_editing_combo;

/// 双击手势共享配置（`manage` 进 Tauri state；tap 回调与设置热更新两端共用）。
struct DoubleTapState(std::sync::Arc<platform::DoubleTapConfig>);

/// 从当前设置快照重建双击手势绑定表（启动时 + `set_setting`/`reset_setting` 改相关键后调用）。
///
/// 遍历 `GLOBAL_SHORTCUT_SETTINGS`：值为 `double_tap:<组合>` 语法的进表（下标 = 绑定 id），
/// 普通组合归全局热键插件（`register_global_shortcuts`）。解析失败告警并跳过，绝不猜测键位。
pub fn apply_double_tap_settings(app: &tauri::AppHandle) {
    use tauri::Manager as _;
    let (Some(state), Some(double_tap)) = (
        app.try_state::<ipc::AppState>(),
        app.try_state::<DoubleTapState>(),
    ) else {
        return;
    };
    let settings = state.settings_snapshot();
    let config = &double_tap.0;

    let mut bindings = Vec::new();
    for (idx, &(key, _)) in GLOBAL_SHORTCUT_SETTINGS.iter().enumerate() {
        if matches!(
            GLOBAL_SHORTCUT_SETTINGS[idx].1,
            GlobalShortcutAction::DesktopLensToggle | GlobalShortcutAction::DesktopLensCapture
        ) && !state.desktop_lens_enabled()
        {
            continue;
        }
        let Some(spec) = settings.get_str(key).filter(|s| !s.trim().is_empty()) else {
            continue;
        };
        if !spec.trim().starts_with(platform::DOUBLE_TAP_PREFIX) {
            continue; // 普通组合 → 插件注册路径
        }
        // 词表内的双击值必然有键码（校验与键码表由用例钉在一起）；解析不出只可能是手改
        // 配置写了非法值，那一项在加载时已回落默认。
        match platform::DoubleTapGesture::for_setting_value(&spec) {
            Some(gesture) => bindings.push(platform::GestureBinding {
                gesture,
                id: u32::try_from(idx).unwrap_or(u32::MAX),
            }),
            None => eprintln!("[shortcut] 无法解析 {key}={spec}，该双击手势已停用"),
        }
    }
    config.set_bindings(bindings);
    config.set_window_ms(
        settings
            .get_int("shortcuts.double_tap_window_ms")
            .unwrap_or(0),
    );
}

/// 当前生效的全局快捷键绑定（设置驱动，运行期可重建）。与 `AppState` 同级 `manage`。
#[derive(Default)]
struct GlobalShortcutBindings(
    std::sync::Mutex<Vec<(tauri_plugin_global_shortcut::Shortcut, GlobalShortcutAction)>>,
);

/// 全局快捷键注册结果（PRD-09-003 ③）：`设置 key → 失败 code`。
///
/// 他应用占用的热键无法预检（操作系统不提供枚举接口），只能在注册时探测失败；结果记在
/// 这里供设置页在该行就地提示。
#[derive(Default)]
struct ShortcutRegistrationStatus(
    std::sync::Mutex<std::collections::BTreeMap<String, &'static str>>,
);

/// 读取当前注册失败表（设置页诊断命令消费；无状态时视为全部正常）。
pub(crate) fn shortcut_registration_failures(
    app: &tauri::AppHandle,
) -> std::collections::BTreeMap<String, &'static str> {
    use tauri::Manager as _;
    app.try_state::<ShortcutRegistrationStatus>()
        .and_then(|state| state.0.lock().ok().map(|guard| guard.clone()))
        .unwrap_or_default()
}

/// 把设置中的归一化键串（如 `cmd_or_ctrl+shift+r`）转为插件可解析的加速器串（`CmdOrCtrl+Shift+R`）。
///
/// 纯函数、可单测；大小写与常见别名容错。键位事实源为 [TDD-05 §3]，本函数只做格式换算、不内置键位。
fn normalize_to_accelerator(spec: &str) -> String {
    spec.split('+')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|token| match token.to_ascii_lowercase().as_str() {
            "cmd_or_ctrl" | "cmdorctrl" => "CmdOrCtrl".to_string(),
            "cmd" | "command" | "super" | "meta" | "win" => "Super".to_string(),
            "ctrl" | "control" => "Ctrl".to_string(),
            "shift" => "Shift".to_string(),
            "alt" | "option" | "opt" => "Alt".to_string(),
            other => other.to_uppercase(),
        })
        .collect::<Vec<_>>()
        .join("+")
}

/// 解析设置键串为插件 `Shortcut`；无法解析（空/非法）返回 `None`（调用方跳过该项注册）。
fn parse_shortcut(spec: &str) -> Option<tauri_plugin_global_shortcut::Shortcut> {
    let accel = normalize_to_accelerator(spec);
    if accel.is_empty() {
        return None;
    }
    tauri_plugin_global_shortcut::Shortcut::try_from(accel.as_str()).ok()
}

/// 从当前设置（含注册表默认回退）注册全部全局快捷键，并刷新 [`GlobalShortcutBindings`]。
/// 先 `unregister_all` 再按设置注册——**改键即调本函数重注册**（设置事实源单一，无残留旧键）。
fn register_global_shortcuts(app: &tauri::AppHandle) {
    use tauri::Manager as _;
    use tauri_plugin_global_shortcut::GlobalShortcutExt as _;

    let Some(state) = app.try_state::<ipc::AppState>() else {
        return;
    };
    let settings = state.settings_snapshot();
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();

    let mut bindings = Vec::new();
    // 注册失败表（每次重注册整体重建，绝不残留上一轮的旧诊断）。
    let mut failures: std::collections::BTreeMap<String, &'static str> =
        std::collections::BTreeMap::new();
    for &(key, action) in GLOBAL_SHORTCUT_SETTINGS {
        if matches!(
            action,
            GlobalShortcutAction::DesktopLensToggle | GlobalShortcutAction::DesktopLensCapture
        ) && !state.desktop_lens_enabled()
        {
            continue;
        }
        // 设置快照已含注册表默认（未改键即默认 TDD-05 §3）；空串视为未绑定，跳过。
        let Some(spec) = settings.get_str(key).filter(|s| !s.trim().is_empty()) else {
            continue;
        };
        // 双击语法归 CGEventTap 手势表（apply_double_tap_settings），不走插件注册。
        if spec.trim().starts_with(platform::DOUBLE_TAP_PREFIX) {
            continue;
        }
        // 保留编辑组合（Cmd+C/V/X/A/Z）作普通全局热键会拦截系统编辑键 → 拒绝注册并告警
        // （前端已锁定为仅双击、后端 set_setting 亦拒绝写入，此处为最后一道防线，TDD-05 §1.2）。
        if is_reserved_editing_combo(&spec) {
            eprintln!(
                "[shortcut] 拒绝把保留编辑键注册为普通全局热键 {key}={spec}（仅允许双击模式）"
            );
            failures.insert(key.to_owned(), "invalid");
            continue;
        }
        let Some(shortcut) = parse_shortcut(&spec) else {
            eprintln!("[shortcut] 无法解析 {key}={spec}，跳过注册");
            failures.insert(key.to_owned(), "invalid");
            continue;
        };
        match gs.register(shortcut) {
            Ok(()) => {
                eprintln!("[shortcut] 已注册 {key}={spec}");
                bindings.push((shortcut, action));
            }
            Err(e) => {
                eprintln!("[shortcut] 注册 {key}={spec} 失败（可能与他应用冲突）：{e}");
                failures.insert(key.to_owned(), "unavailable");
            }
        }
    }

    if let Some(slot) = app.try_state::<ShortcutRegistrationStatus>() {
        if let Ok(mut guard) = slot.0.lock() {
            *guard = failures;
        }
    }

    if let Some(slot) = app.try_state::<GlobalShortcutBindings>() {
        if let Ok(mut guard) = slot.0.lock() {
            *guard = bindings;
        }
    }
}

/// 改键后重注册全局快捷键（`set_setting`/`reset_setting` 命令在改 `shortcuts.*` 全局键后调用）。
pub fn reregister_global_shortcuts(app: &tauri::AppHandle) {
    register_global_shortcuts(app);
}

/// 录快捷键期间把全局键整体收起来（PRD-09-003）。
///
/// 网页里的 `preventDefault()` 拦不住系统级热键：普通组合由系统抢在应用之前吃掉，双击组合
/// 由事件 tap 观测，两条路都不经过 webview。不收起的话，按到已被占用的组合会当场唤起占用者，
/// 录制器却收不到那一下。macOS 系统设置与 Raycast 同样在录制期间解除全部绑定。
pub fn suspend_global_shortcuts(app: &tauri::AppHandle) {
    use tauri::Manager as _;
    use tauri_plugin_global_shortcut::GlobalShortcutExt as _;
    let _ = app.global_shortcut().unregister_all();
    if let Some(double_tap) = app.try_state::<DoubleTapState>() {
        double_tap.0.set_bindings(Vec::new());
    }
    if let Some(slot) = app.try_state::<GlobalShortcutBindings>() {
        if let Ok(mut guard) = slot.0.lock() {
            guard.clear();
        }
    }
}

/// 录制结束：按当前设置把两条路都装回去（与启动、改键走同一对函数，不另写一份）。
pub fn resume_global_shortcuts(app: &tauri::AppHandle) {
    register_global_shortcuts(app);
    apply_double_tap_settings(app);
}

/// 执行一个全局快捷键动作（插件热键与双击手势共用的单一分发点）。
///
/// 语音输入按住说话需要 `pressed/released` 两相；其余动作仅在按下触发。
// 译境入口在非 macOS 上提前返回即闭包末尾。
#[cfg_attr(not(target_os = "macos"), allow(clippy::needless_return))]
fn dispatch_shortcut_action(app: &tauri::AppHandle, action: GlobalShortcutAction, pressed: bool) {
    use tauri::Manager as _;
    match action {
        // 语音输入按住说话：按下/松开都要（hold-to-talk）。
        GlobalShortcutAction::VoiceInput => {
            let phase = if pressed { "pressed" } else { "released" };
            let app = app.clone();
            let _ = app.clone().run_on_main_thread(move || {
                show_voice_input_hud(&app, phase);
            });
        }
        // 其余动作仅在**按下**触发（松开忽略）。
        _ if !pressed => {}
        GlobalShortcutAction::PopupTranslate => {
            // Windows（ADR-163 ②）：热键带上选区，没有选区才开输入模式空窗。
            #[cfg(target_os = "windows")]
            tauri::async_runtime::spawn(run_hotkey_selection_capture(app.clone()));
            // 热键路径（PRD-01-002/006）→ 输入模式空窗。ESC 走 CGEventTap，非全局热键。
            #[cfg(not(target_os = "windows"))]
            {
                let app = app.clone();
                let _ = app.clone().run_on_main_thread(move || {
                    if let Some(state) = app.try_state::<ipc::AppState>() {
                        if let Ok(mut session) = state.popup().lock() {
                            session.set_empty_input();
                        }
                    }
                    show_popup_panel(&app);
                });
            }
        }
        GlobalShortcutAction::ScreenshotTranslate => {
            // 截屏翻译（PRD-01-007）：框选/OCR/会话/显示全流程在 screenshot 模块编排（tokio 任务，
            // 不阻塞热键回调；内部窗口操作自行派主线程）。
            tauri::async_runtime::spawn(screenshot::run_screenshot_translate(app.clone()));
        }
        GlobalShortcutAction::DesktopLensToggle => {
            let app = app.clone();
            let _ = app.clone().run_on_main_thread(move || {
                if !app
                    .try_state::<ipc::AppState>()
                    .is_some_and(|state| state.desktop_lens_enabled())
                {
                    return;
                }
                if app
                    .try_state::<ipc::AppState>()
                    .is_some_and(|state| state.desktop_lens_active())
                {
                    hide_desktop_lens(&app);
                    return;
                }
                #[cfg(target_os = "macos")]
                if platform::macos::permissions::has_screen_capture_access() {
                    show_desktop_lens(&app);
                } else {
                    let _ = platform::macos::permissions::request_screen_capture_access();
                }
            });
        }
        GlobalShortcutAction::DesktopLensCapture => {
            // 会话中唯一会反复做的动作，给一条不用把手从键盘挪开的路径：按当前取景范围直接
            // 拍一张，与控制条相机键**共用同一编排**（`run_desktop_lens_capture`）。
            // 会话未运行时是无动作——本键的语义严格限定在会话内，绝不用一个"捕捉"键去
            // 隐式开启会话（那会在用户尚未取景时拍下一片无意义区域）。
            let app = app.clone();
            if app
                .try_state::<ipc::AppState>()
                .is_some_and(|state| state.desktop_lens_enabled() && state.desktop_lens_active())
            {
                tauri::async_runtime::spawn(async move {
                    if let Err(error) = ipc::run_desktop_lens_capture(app).await {
                        eprintln!("[desktop-lens] 快捷键捕捉失败：{error}");
                    }
                });
            }
        }
        GlobalShortcutAction::TextQuickNote => {
            // 只在**按下**触发（上面那条 `_ if !pressed` 已经挡掉松开）。不带任何上下文
            // 捕获：这扇窗要做的只是"给你一个能立刻打字的地方"，读前台选区是另一个功能
            // （快捷对话）的语义，混进来会让同一次按键有两种结果。
            let app = app.clone();
            let _ = app.clone().run_on_main_thread(move || {
                show_text_quick_note_panel(&app, "shortcut");
            });
        }
        GlobalShortcutAction::QuickChat => {
            // 先显示面板（响应性优先），选区捕获并发进行（PRD-03-008：AX 直读优先，
            // 无 AX 暴露时降级受保护剪贴板捕获），就绪后经 `chat://prefill` 通知面板拉取。
            let app = app.clone();
            #[cfg(target_os = "macos")]
            {
                let capture_enabled = app
                    .try_state::<ipc::AppState>()
                    .map(|state| {
                        state
                            .settings_snapshot()
                            .get_bool("chat.capture_selection")
                            .unwrap_or(true)
                    })
                    .unwrap_or(false);
                if capture_enabled {
                    tauri::async_runtime::spawn(run_quick_chat_selection_capture(app.clone()));
                }
            }
            let _ = app.clone().run_on_main_thread(move || {
                show_quick_chat_panel(&app);
            });
        }
        GlobalShortcutAction::RecordingToggle => {
            let app = app.clone();
            let _ = app.clone().run_on_main_thread(move || {
                toggle_recording_hud(&app);
            });
        }
        // 录音打点（PRD-04-005）：只在会话进行中有意义，空闲时无动作，绝不用一个打点键去
        // 隐式开始录音。时刻由 HUD 的计时决定，打点与反馈走 HUD 那一份实现。提词器跟读
        // 会话里同一个键打在提词器上（ADR-162 ⑤ C4c），不弹录音 HUD。
        GlobalShortcutAction::RecordingBookmark => {
            let Some(session) = app
                .try_state::<ipc::AppState>()
                .map(|state| state.recording().state())
            else {
                return;
            };
            if !matches!(
                session.status,
                recording::RecordingStatus::Recording | recording::RecordingStatus::Paused
            ) {
                return;
            }
            if session.purpose == recording::SessionPurpose::Teleprompter {
                ipc::mark_from_hotkey(app);
                return;
            }
            let app = app.clone();
            let _ = app.clone().run_on_main_thread(move || {
                run_recording_hud_hotkey(&app, "bookmark");
            });
        }
        GlobalShortcutAction::LiveCaption => {
            let app = app.clone();
            let _ = app.clone().run_on_main_thread(move || {
                toggle_live_caption(&app);
            });
        }
    }
}

/// 在当前绑定中查找按下的快捷键对应的动作（设置驱动；O(n=4) 线性查足够）。
fn lookup_shortcut_action(
    app: &tauri::AppHandle,
    shortcut: &tauri_plugin_global_shortcut::Shortcut,
) -> Option<GlobalShortcutAction> {
    use tauri::Manager as _;
    let bindings = app.try_state::<GlobalShortcutBindings>()?;
    let guard = bindings.0.lock().ok()?;
    guard
        .iter()
        .find(|(sc, _)| sc == shortcut)
        .map(|(_, action)| *action)
}

/// Headless 导出 TypeScript 绑定（供 `export-bindings` 二进制 / CI 调用，不启动窗口）。
///
/// # Errors
/// 导出失败（路径不可写等）时返回错误。
pub fn export_bindings() -> Result<(), Box<dyn std::error::Error>> {
    ipc::builder().export(specta_typescript::Typescript::default(), BINDINGS_PATH)?;
    let mut source = std::fs::read_to_string(BINDINGS_PATH)?;
    let pages: Vec<_> = luna_config::registry::SETTINGS_PAGES
        .iter()
        .map(|page| page.as_str())
        .collect();
    source.push_str(&format!(
        "\nexport const SETTINGS_PAGES = {} as const;\n",
        serde_json::to_string(&pages)?
    ));
    let descriptions: std::collections::BTreeMap<_, _> = luna_config::registry::SETTINGS_PAGES
        .iter()
        .map(|page| (page.as_str(), page.description_inline()))
        .collect();
    source.push_str(&format!(
        "\nexport const SETTINGS_PAGE_DESCRIPTION_INLINE = {} as const;\n",
        serde_json::to_string(&descriptions)?
    ));
    // 一方应用的 id（ADR-169 ⑧）：前端的应用注册表以它为键（`Record<AppId, …>`），少一个、多一个
    // 都是编译错误；散在各处的应用 id 字面量也都标成 `AppId`。
    let app_ids: Vec<_> = luna_config::apps::FIRST_PARTY_APPS
        .iter()
        .map(|app| app.id)
        .collect();
    source.push_str(&format!(
        "\nexport const FIRST_PARTY_APP_IDS = {} as const;\nexport type AppId = (typeof FIRST_PARTY_APP_IDS)[number];\n",
        serde_json::to_string(&app_ids)?
    ));
    std::fs::write(BINDINGS_PATH, source)?;
    normalize_bindings(BINDINGS_PATH)?;
    std::fs::write(THEMES_CSS_PATH, luna_content_schema::builtin_stylesheet())?;
    Ok(())
}

/// 规范化生成产物：去每行行尾空白 + 统一 LF + 单一末尾换行。
///
/// `specta_typescript` 对 union 类型每支输出形如 `"trash" | `（**行尾带空格**）。CI 以
/// `git diff --exit-code` 校验"重新生成后无漂移"（[TDD-13 §2]），而编辑器/format-on-save
/// 普遍自动去行尾空白——两者口径不一致会造成**反复漂移**（提交态被去空白、重新生成又带回）。
/// 在唯一导出工厂里统一规范化，使生成产物确定、与提交态稳定一致——根因消除，非补丁。
fn normalize_bindings(path: &str) -> std::io::Result<()> {
    let raw = std::fs::read_to_string(path)?;
    let mut out = String::with_capacity(raw.len());
    for line in raw.lines() {
        out.push_str(line.trim_end());
        out.push('\n');
    }
    std::fs::write(path, out)
}

/// 运行桌面应用（OSS 默认装配薄壳，TDD-14 §3.2 ①）。
///
/// # Panics
/// Tauri 装配/运行失败时 panic（Phase 0 走 expect；后续接结构化错误与诊断）。
pub fn run() {
    // 探针子进程必须在构造 Context **之前**短路退出（保持与历史单体 run() 完全相同的
    // 执行顺序）；`run_with` 内部还会再查一次以覆盖外部调用方——探针命中即 exit，二查幂等。
    whisper_backend::run_probe_if_requested();
    run_with(tauri::generate_context!(), default_assembly());
}

/// 默认装配（ADR-037：单一构建）—— 挂载云客户端插件与云感知的工件下载。
///
/// **挂载 ≠ 联网**：插件在场只意味着「用户可以去登录」；未登录时
/// 一切云路径由 [`cloud::CloudGate`] 关死（该不变量有机器断言，见 `cloud` 模块测试）。
/// 这正是 ADR-037 用运行期护栏替换构建期「零云符号」断言的落点。
#[must_use]
pub fn default_assembly() -> AppAssembly {
    use std::sync::{Arc, OnceLock};

    // AppHandle 到 setup 才存在；门控闭包需要提前构造，故经 OnceLock 延迟注入。
    let handle_cell: Arc<OnceLock<tauri::AppHandle>> = Arc::new(OnceLock::new());
    let gate = cloud::production_gate(Arc::clone(&handle_cell));
    let plugin_gate = gate.clone();

    // 制品面 fetcher 与云客户端指向同一台服务器：格子在这里出生、交给 fetcher，并经插件构建
    // 时 manage 到 app 上；每次换齐（`cloud::reconfigure_cloud_stack`）把云客户端此刻的地址写进去。
    // 两条路径若各自解析，就会出现「账户连对了服务器、制品下载却打向别处」这种半连状态。
    let api_base_cell = cloud_artifact::new_address_cell();

    // 账户变化 → 重算算力落点并广播（选定服务器 / 登录 / 退出 / 注销 / 等浏览器）。
    //
    // 「登录了哪台」是算力落点的判据：没有这条接线时，用户登录成功、卡片写「AI 由 Lunarca 云
    // 提供」，而对话仍在本机模型上跑。只是展示变了（等浏览器、收到链接）时不重装，只广播。
    let observer_cell = Arc::clone(&handle_cell);
    // 上一次观察到的登录态（0 = 未知、1 = 未登录、2 = 已登录）：只在**翻转**时做前台动作，
    // 换钥 / 换服务器触发的重装不会反复弹 toast。
    let last_logged_in = Arc::new(std::sync::atomic::AtomicU8::new(0));
    let last_for_changed = Arc::clone(&last_logged_in);
    let last_for_setup = Arc::clone(&last_logged_in);
    let on_account_changed: luna_cloud::AccountObserver = Arc::new(move |change| {
        use std::sync::atomic::Ordering;
        use tauri::Manager as _;
        let Some(app) = observer_cell.get() else {
            return;
        };
        // 付款处境变了（纵切 3 C3）：只发 / 收通知，不重装、不广播——账户页自己会重取。
        if change == luna_cloud::account::AccountChange::BillingStanding {
            cloud::notice_billing_standing(app);
            return;
        }
        // 用量预警轮次变了（纵切 3 C4）：只按落盘标记发阈值通知，不重装、不广播。
        if change == luna_cloud::account::AccountChange::UsageAlerts {
            cloud::notice_usage_alerts(app);
            return;
        }
        let Some(state) = app.try_state::<ipc::AppState>() else {
            return;
        };
        if change == luna_cloud::account::AccountChange::Session {
            cloud::reconfigure_cloud_stack(app, &state);
            // 同步绑定归账户（ADR-151 修订 2026-10-15）：换了账户当场按新账户重排、重发总览。
            sync_service::SyncService::account_changed(app);
            // 库列表跟着账户走（同上修订「续二」）：换了账户就换列表、打开那个账户上次用的库；
            // 退出不是切换，列表照旧。
            vault_lists::account_changed(app);
        }
        // 重装完再广播：界面重读到的落点与转写可用性才是装配后的事实。
        ipc::account::emit_account_changed(app);
        let logged_in = app
            .try_state::<luna_cloud::commands::CloudState>()
            .is_some_and(|cloud| cloud.0.logged_in());
        let previous = last_for_changed.swap(if logged_in { 2 } else { 1 }, Ordering::SeqCst);
        if logged_in {
            // 在新版本登录之后，旧版本留在钥匙串里的条目静默删掉一次（只删不读，失败不阻断）。
            secrets::spawn_legacy_cleanup(app);
        }
        // 刚登录（未登录 / 未知 → 已登录）：无条件拉一遍全部目录，不按新鲜度判（ADR-170 ①）。
        // 换钥、换服务器等已登录态里的重装不在此列，由 `reconfigure_cloud_stack` 按 6 小时判。
        if logged_in && previous != 2 {
            cloud::spawn_catalog_refresh_on_sign_in(app);
        }
        // 刚在浏览器里批准完：把应用亮回前台并确认一次（Slack / GitHub Desktop 同一做法），
        // 否则用户停在浏览器那句「可以关闭本页了」上，不知道客户端已经继续了。
        if logged_in && previous == 1 {
            show_main_window(app);
            if let Some(center) = app.try_state::<notifications::NotificationCenter>() {
                center.publish(notifications::NotificationEvent::signed_in(), app);
            }
        }
    });
    // 会话被动失效（refresh 被拒 / 设备被撤销）：与主动登出走同一条重装，再多一条
    // 「请重新登录」通知——没有它，卡片继续写「AI 发往服务端」而每次对话都在 401 上失败。
    let lost_cell = Arc::clone(&handle_cell);
    let changed_for_lost = Arc::clone(&on_account_changed);
    let on_session_lost: luna_cloud::SessionObserver = Arc::new(move || {
        use tauri::Manager as _;
        changed_for_lost(luna_cloud::account::AccountChange::Session);
        let Some(app) = lost_cell.get() else {
            return;
        };
        if let Some(center) = app.try_state::<notifications::NotificationCenter>() {
            center.publish(notifications::NotificationEvent::session_expired(), app);
        }
    });

    // 提示词目录告警 → 通知中心（ADR-050 修订 2026-09-04）：整份被拒 / 按旧清单部分生效
    // 此前只有一行 stderr，运营发了一版、客户端没用上，界面上没有任何信号。
    let alert_cell = Arc::clone(&handle_cell);
    let on_prompt_overlay: luna_cloud::PromptOverlayObserver = Arc::new(move |alert| {
        use tauri::Manager as _;
        let Some(app) = alert_cell.get() else {
            return;
        };
        let Some(center) = app.try_state::<notifications::NotificationCenter>() else {
            return;
        };
        let event = match alert {
            luna_cloud::PromptOverlayAlert::Rejected { version, key } => {
                notifications::NotificationEvent::prompt_catalog_rejected(*version, key)
            }
            luna_cloud::PromptOverlayAlert::Degraded {
                version,
                manifest_version,
                missing,
            } => notifications::NotificationEvent::prompt_catalog_degraded(
                *version,
                *manifest_version,
                missing.len(),
            ),
        };
        center.publish(event, app);
    });

    // 主题目录交付变了 → 广播「主题库变了」：封禁 / 解除封禁不改设置，外观要靠它在运行中重读（纵切 7a T3）。
    let themes_cell = Arc::clone(&handle_cell);
    let on_themes_changed: luna_cloud::CatalogChangedObserver = Arc::new(move || {
        if let Some(app) = themes_cell.get() {
            let _ = tauri_specta::Event::emit(&ipc::themes::ThemeLibraryChanged, app);
        }
    });

    // 模板目录交付变了 → 广播「模板库变了」：同主题目录，封禁要在运行中生效（纵切 7b）。
    let templates_cell = Arc::clone(&handle_cell);
    let on_templates_changed: luna_cloud::CatalogChangedObserver = Arc::new(move || {
        if let Some(app) = templates_cell.get() {
            let _ = tauri_specta::Event::emit(&ipc::template_library::TemplateLibraryChanged, app);
        }
    });

    // 「已连接」探针给 luna-cloud 的后台目录同步：只有已登录（且不在严格本地）时才自发拉
    // 目录——ADR-037 的零云网络不变量在未登录态原样成立。
    let connected_gate = gate.clone();
    let connected: luna_cloud::ConnectedProbe =
        Arc::new(move || connected_gate.evaluate().allowed());

    // 灰度受众（ADR-050 修订 ④）：应用版本 + 界面语言 + 按设备稳定的桶（主机名 + 用户目录
    // 派生，luna-cloud 负责；这里只给盐）。
    let rollout_bucket =
        luna_cloud::catalog::device_rollout_bucket(&std::env::var("HOME").unwrap_or_default());
    let audience: luna_cloud::AudienceProvider = Arc::new(move || luna_cloud::AudienceContext {
        client_version: env!("CARGO_PKG_VERSION").to_owned(),
        locale: luna_core::prompt::registry().locale(),
        bucket: rollout_bucket,
    });

    // 严格本地探针给 luna-cloud：目录刷新/制品下载的联网动作据此硬拦截。
    let strict_local_probe: luna_cloud::StrictLocalProbe = Arc::new(move || {
        // 探针只回答「是否严格本地」这一个问题——未配置云**不等于**严格本地：
        // 未登录用户仍可显式点「刷新目录」（`cloud` 模块头注释的例外清单）。
        matches!(
            plugin_gate.evaluate(),
            cloud::CloudAvailability::BlockedByStrictLocal
        )
    });
    let fetcher = Arc::new(cloud::CloudAwareArtifactFetcher::new(
        Some(Arc::new(cloud_artifact::ArtifactPlaneFetcher::new(
            Arc::clone(&api_base_cell),
            Arc::clone(&strict_local_probe),
        ))),
        gate,
    ));

    // 账户数据导出的保存对话框（纵切 3 C6）：对话框插件在宿主，与别处导出同一份实现。
    // 对话框没给出路径（取消，或回调通道断了）一律按取消处理——什么都不发。
    let picker_cell = Arc::clone(&handle_cell);
    let pick_export_destination: luna_cloud::ExportDestinationPicker = Arc::new(move |file_name| {
        let app = picker_cell.get().cloned();
        Box::pin(async move {
            let app = app?;
            ipc::pick_export_destination(&app, &file_name, "json")
                .await
                .ok()
                .flatten()
        })
    });

    AppAssembly::default()
        .with_plugin(move |builder| {
            builder
                // 会话被动失效的钩子要在 setup 里挂到云客户端上，而它在这个闭包里出生：
                // 经 state 递过去，setup 那侧按类型取。
                .manage(SessionLostHook(on_session_lost))
                .manage(cloud_artifact::CloudAddress(Arc::clone(&api_base_cell)))
                .plugin(luna_cloud::init(luna_cloud::CloudHooks {
                    probe: strict_local_probe,
                    on_account_changed,
                    on_prompt_overlay,
                    on_themes_changed,
                    on_templates_changed,
                    connected,
                    audience,
                    pick_export_destination,
                    credentials: secrets::credential_factory(),
                }))
        })
        .with_artifact_fetcher(fetcher)
        .with_setup(move |app| {
            use tauri::Manager as _;
            let _ = handle_cell.set(app.clone());
            // 启动时按已恢复的会话定初值：启动后第一次账户广播（如权益刷新）不是「刚登录」，
            // 否则每次启动都无条件拉一遍目录（ADR-170 ①「启动时会话恢复不算登录」，2026-10-23 真机）。
            // 只在观察者还没见过登录态时写，不盖掉已经发生的翻转。
            // 云插件没装上（无云装配）就不定：误写成「未登录」会让启动变成一次登录。
            if let Some(cloud) = app.try_state::<luna_cloud::commands::CloudState>() {
                let _ = last_for_setup.compare_exchange(
                    0,
                    if cloud.0.logged_in() { 2 } else { 1 },
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                );
            }
            Ok(())
        })
}

/// 以给定装配运行桌面应用。
///
/// 生产二进制恒经 [`run`] 走 [`default_assembly`]（含云插件与云感知下载）。本函数保留
/// 显式装配入口是为**测试与嵌入方**：例如以空装配 `AppAssembly::default()` 起一个完全
/// 无云的实例。ADR-037 后它不再是「风味接缝」——单一构建没有第二种风味可接。
///
/// # Panics
/// Tauri 装配/运行失败时 panic（同 [`run`]）。
pub fn run_with(context: tauri::Context, assembly: AppAssembly) {
    use tauri::Manager as _; // for app.manage()

    // 冷启动时间线的原点：进程能记到的最早一刻（TDD-13 §6 指标从这里起算）。
    let startup_origin = std::time::Instant::now();

    // 进程外 Metal 探针子进程入口：若本进程由 `resolve_backend` spawn 而来，则在此试跑
    // Metal 并 `exit`，绝不进入下方 GUI 初始化（TDD-03 §4）。
    whisper_backend::run_probe_if_requested();

    // 应用标识迁移（ADR-030，TDD-13 §5）：任何插件或窗口碰到数据目录之前，把旧标识下的目录改名到
    // 新标识下。逐个父目录独立判断；失败只记日志，旧数据原样留着（新目录为空等于全新安装）。
    for (base, result) in
        luna_config::identity::migrate_legacy_dirs(&luna_config::identity::app_dir_bases())
    {
        match result {
            Ok(luna_config::identity::DirMigration::Migrated) => {
                eprintln!("[identity] app directory migrated: {}", base.display());
            }
            Ok(_) => {}
            Err(err) => {
                eprintln!(
                    "[identity] app directory migration failed at {}: {err}",
                    base.display()
                );
            }
        }
    }

    let builder = ipc::builder();
    let AppAssembly {
        plugins,
        extra_setup,
        artifact_fetcher,
    } = assembly;

    let tauri_builder = tauri::Builder::default();
    // 单实例必须排在插件链第一个（插件文档要求：它要在其它插件初始化前拦下第二个进程）。
    // 只在 Windows / Linux：那里点 `lunarca://` 会另起进程；macOS 由系统交给已在跑的实例，
    // 行为不变。第二个进程的链接经 `deep-link` feature 走既有 `on_open_url`，这里只亮主窗。
    #[cfg(not(target_os = "macos"))]
    let tauri_builder =
        tauri_builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            show_main_window(app);
        }));
    let mut tauri_builder = tauri_builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        // `lunarca://connect?…` 连接链接（ADR-116）。scheme 登记在 tauri.conf.json；
        // 送达后只**存起来并通知界面**，写设置 / 钉住一律等用户在连接面板上确认。
        .plugin(tauri_plugin_deep_link::init())
        // 系统通知通道（PRD-15-012）：由前端在收到带 `system` 通道的事件时经本插件发出——
        // 文案只在前端一份（notifications.json），Rust 只按在场态决定走不走这条通道。
        .plugin(tauri_plugin_notification::init())
        // 备用全局热键插件（TDD-05 §3）。处理器在按下时显示已预热的弹窗——给 ADR-015 不夺焦
        // 测试一个不依赖辅助功能的入口（标准热键不走 CGEventTap）。
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    // 逐事件诊断仅在显式开启时打印（LUNA_DEBUG_SHORTCUTS），避免正常使用日志刷屏。
                    if std::env::var_os("LUNA_DEBUG_SHORTCUTS").is_some() {
                        eprintln!(
                            "[global-shortcut] event shortcut={} id={} state={:?}",
                            shortcut,
                            event.id,
                            event.state()
                        );
                    }
                    let Some(action) = lookup_shortcut_action(app, shortcut) else {
                        return;
                    };
                    let pressed =
                        event.state() == tauri_plugin_global_shortcut::ShortcutState::Pressed;
                    dispatch_shortcut_action(app, action, pressed);
                })
                .build(),
        );
    // 装配接缝：额外插件在内建插件之后逐个应用（OSS 默认装配为空，零行为变化）。
    for plugin in plugins {
        tauri_builder = plugin(tauri_builder);
    }

    tauri_builder
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            // 冷启动时间线（ADR-114 ①）：先于一切装配托管，后续里程碑都往这里打。
            let timeline =
                startup::StartupTimeline::new(startup_origin, app.path().app_data_dir().ok());
            timeline.mark(startup::StartupMilestone::SetupBegin);
            app.manage(timeline);
            // 首启引导的本机记录（PRD-14 §1 的度量落点）：与启动时间线同一个设备级目录、
            // 同一条纪律——只记步骤与时刻，默认离线、不上报。跨启动续写，因为引导本来就
            // 可能分几次走完。
            app.manage(onboarding_funnel::OnboardingFunnel::new(
                app.path().app_data_dir().ok(),
            ));
            // 「应用在跑」的标记（ADR-141 ④）：托管到退出，CLI 据此拒绝 `luna vault index`。
            // 拿不到（另一个实例或正在跑的 `luna vault index` 占着）照常启动。
            if let Some(lock) = app
                .path()
                .app_config_dir()
                .ok()
                .and_then(|dir| luna_config::app_lock::hold(&dir))
            {
                app.manage(lock);
            }
            // 命令 + 事件集合挂载到本 app 实例。
            builder.mount_events(app);
            app.manage(notifications::NotificationCenter::new());
            // 刚完成标识迁移：系统权限（麦克风、辅助功能、屏幕与系统声音）按应用标识记，换标识后要重新授权
            // ——预告这是升级的一次性代价，不让用户以为应用坏了（TDD-13 §5 ③）。只提示一次。
            if app
                .path()
                .app_data_dir()
                .is_ok_and(|dir| luna_config::identity::take_migration_marker(&dir))
            {
                if let Some(center) = app.try_state::<notifications::NotificationCenter>() {
                    center.publish(
                        notifications::NotificationEvent::info_at(
                            "identifier_migrated",
                            notifications::NotificationTarget::Settings("privacy".into()),
                        ),
                        app.handle(),
                    );
                }
            }
            // 加载设备级配置（损坏回退默认，不覆盖用户文件，TDD-05 §7.3）。
            let (settings, settings_persistence) = load_device_settings(app);
            let device_usage = usage::open_device_usage_store(
                &app.path()
                    .app_data_dir()
                    .unwrap_or_else(|_| std::env::temp_dir().join("luna-app-data")),
            )?;
            // 后端 i18n（TDD-11）：按 appearance.language 构造（MVP-0 仅 zh-CN），供后端侧文案查询。
            // 经 `resolve_language_setting` 而不是原值直传：该项的默认值是 `system`，
            // 而设置读取会回落到注册表默认值——原值直传时 `"system"` 会被当成语言标签解析
            // 成 en-US，默认安装的后端于是「启动即英文」（连带让启动窗口内的自动打标打出
            // 英文标签）。理由与代价逐条见该函数。
            let backend_language = i18n::resolve_language_setting(
                &settings.get_str("appearance.language").unwrap_or_default(),
            );
            // 能力提示词的 locale 变体取用同一个语言（ADR-050）：目录可以只覆盖某个语言的
            // 某几条模板，而「当前是哪个语言」在客户端只有这一个事实源。不接这一句的话，
            // 运营发出去的 locale 变体永远不会生效，而两侧都不会报错。
            luna_core::prompt::registry().set_locale(Some(backend_language.clone()));
            // **一个对象、两处持有**（ADR-107）：Tauri state 服务 `coded_error(app, …)`，
            // `AppState` 服务只持 `&AppState` 的那一大片（`vault_not_open()` 被上百处调用）。
            // 各建一份的话，`set_language` 之后两边说的会是两种语言。
            let backend_copy = std::sync::Arc::new(i18n::BackendI18n::new(backend_language));
            app.manage(std::sync::Arc::clone(&backend_copy));
            // 托盘图标 + 右键菜单（快捷启动各功能 + 退出）：在 BackendI18n 托管后建，菜单文案走后端 i18n。
            setup_tray_icon(app)?;
            // 构造并托管全局 JobQueue（在 tokio 运行时内构造——JobQueue::new 内部 spawn）。
            // `artifact_fetcher` = 装配注入的工件下载实现；`None`（空装配）→ 构造侧
            // 落直连实现。生产装配注入的是云感知实现（未配置云时其行为即直连）。
            let state = tauri::async_runtime::block_on(async {
                ipc::build_state_with_settings_persistence(
                    settings,
                    settings_persistence,
                    device_usage,
                    artifact_fetcher,
                )
            });
            if let Some(center) = app.try_state::<notifications::NotificationCenter>() {
                center.configure_history(state.device_usage_store(), state.vault_arc());
            }
            // **唯一装配点**（`the_backend_copy_is_attached_exactly_once` 判定）：漏了它，
            // 那一大片只持 `&AppState` 的地方会静默退回「只发码」，界面上是一串 SCREAMING。
            state.attach_backend_copy(backend_copy);
            state.install_vault_watcher(app.handle().clone());
            // 说话人细分 Provider（L1，ADR-020）：模型齐备才注册（缺失=诚实无此能力）。
            state.register_diarize_provider_if_available(app.handle());
            if let Some(vault_root) = state.vault_root() {
                app.asset_protocol_scope()
                    .allow_directory(&vault_root, true)?;
            }
            let resume_queue = state.has_pending_processing();
            // Phase 11 is independent of Vault storage. It is intentionally managed alongside the
            // app state before IPC becomes reachable, so a missing Keychain produces zero capture.
            let metrics_dir = app.path().app_data_dir()?;
            let metrics_enabled = state
                .settings_snapshot()
                .get_bool("privacy.local_knowledge_metrics")
                .unwrap_or(false);
            let app_for_metrics_status = app.handle().clone();
            // 工作区观测：**默认关闭**，且关着时连库文件都不建（TDD-12 §2.1）。
            // 它不需要钥匙串——库里只有计数，没有任何可关联值要保护。
            app.manage(workspace_metrics::LocalWorkspaceMetrics::new(
                metrics_dir.clone(),
                state
                    .settings_snapshot()
                    .get_bool("privacy.local_workspace_metrics")
                    .unwrap_or(false),
            ));
            // 本机保险箱（TDD-12 §2）：云插件 setup 已先以同一目录装上，这里取回同一份。
            let secret_vault = secrets::attach(&app.path().app_config_dir()?);
            app.manage(metrics::LocalKnowledgeMetrics::deferred(
                metrics_dir,
                secret_vault,
                metrics_enabled,
                std::sync::Arc::new(move |_| {
                    ipc::emit_local_knowledge_metrics_availability_changed(&app_for_metrics_status);
                }),
            ));
            app.manage(state);
            // 随包推理引擎（ADR-143）：状态变化转发给前端；按已装 GGUF 编排一次——
            // 一个都没装时什么也不起（不为「也许会用」常驻一个进程）。两者都经
            // `app.state::<AppState>()` 取监管器，**必须在 `app.manage(state)` 之后**——
            // 放在前面会在 setup 里 panic，而 setup 不能展开，整个应用启动即 abort（真机踩过）。
            local_engine::forward_status_changes(app.handle());
            // 随包推理引擎（ADR-143）：迁移遗留槽位 → 编排一次 → 记「引擎就绪」→
            // 核查默认对话模型（缺失即通知引导，PRD-01 §异常：不等首次生成才失败）。
            local_engine::spawn_startup(app.handle());
            // 官方同步（ADR-148）：托管服务并起调度循环。循环每一轮自己核对此刻打开的库，不靠
            // 任何一条打开路径通知；严格本地、未登录、没开同步时一个请求都不发。
            // 旧版写下的没有归属账户的绑定与不分账户的提醒标记：启动时清掉（ADR-151 修订
            // 2026-10-15，未发布即硬切），已开同步的库重新开启一次。
            if let Ok(config_dir) = app.path().app_config_dir() {
                sync_service::remove_legacy_state(&config_dir);
                account_scope::remove_legacy_markers(&config_dir);
            }
            app.manage(std::sync::Arc::new(sync_service::SyncService::default()));
            // 库列表跟着账户走（ADR-151 修订 2026-10-15「续二」）：注册表每改写一次发一次
            // `vaults://changed`；启动时把旧清单里的同步库列进绑定账户、按此刻登录的账户对齐（后台：
            // 读钥匙串可能等系统授权，不占 setup）。启动装载不经 `bind_vault`，账户那一笔在这里补。
            vault_lists::install(app.handle());
            vault_lists::startup(app.handle());
            // 同步大文件的暂停条件（ADR-156 ③）：网络路径监听随进程常驻。
            crate::platform::start_large_transfer_watch();
            sync_service::SyncService::start(app.handle().clone());
            // macOS「服务」菜单「用 Lunarca 存为速记」（W5b）：落盘要 AppState，故在托管之后挂。
            ipc::install_quick_note_service(app.handle());
            // 录音状态转移 → 全窗事件（B2 状态栏「录音中」/ H2 面板以控制器为准）。
            ipc::recording::install_recording_state_broadcast(app.handle());
            // 路由策略预设（PRD-10-008）：启动按设置投影一次，之后随设置副作用热切换。
            if let Some(state) = app.try_state::<ipc::AppState>() {
                ipc::apply_route_strategy(&state);
            }
            // **启动装配 = 「从无到有换一次服务端」**，走与热切完全同一个入口
            // （`cloud::reconfigure_cloud_stack`：外发闸 → ASR 两条通道 → 对话两档 →
            // 上下文窗，顺序即在那里）。
            //
            // 此前启动另有一条路径：`build_state…` 在构造期无条件把默认档 LLM 按本机
            // 档装上。两条路径里只有热切那条解析端点，于是配了组织服务端或登录了
            // Lunarca 云的用户，**每次重启后对话与翻译都打回本机**，而设置页同时写着
            // 「由服务端执行」；改任意一个云设置才会「莫名其妙好了」——这类 bug 用户几乎
            // 报告不出来。合成一条之后，那个入口既有的守护测试同时覆盖了启动。
            //
            // 必须在 `app.manage(state)` **之后**：门控要经 `try_state::<AppState>` 读隐私档。
            // 仍在任何后台任务 spawn 之前，故「启动瞬间外发闸是放行的默认值」这个窗口
            // 与此前一样不存在。
            //
            // 门控会读一次保险箱里的会话（`logged_in`）。没登录过的用户没有那一条（甚至没有
            // 保险箱文件）——不碰钥匙串；登录过的用户要取一次保险箱主密钥（钥匙串里唯一的
            // 那一条，TDD-12 §2），本应用自己写的、ACL 相符时不弹框。
            //
            // **但这次读绝不能占住 setup（真机白屏坐实，2026-08-24）**：重签名后的构建
            // （每次 `codesign` 的 ad-hoc 签名都不同）读到 ACL 失配的条目时，macOS 弹出
            // 钥匙串授权框并**阻塞调用线程直到用户作答**——此前这段跑在 setup 主线程上，
            // 事件循环卡死在 `did_finish_launching`：窗口已建但 IPC 永不处理，`onboardingState`
            // 永不返回，首帧永远是白屏；用户没注意到（或挪开了）那个系统框，看到的就是
            // 「启动只剩一个白屏」。主密钥读失败的静默期防的是**失败之后**的弹窗风暴，
            // 防不了第一次读的阻塞——此前此处的注释把两件事混为一谈。
            //
            // 修法与下面 metrics 激活同一条纪律：可能等待系统授权的阻塞 I/O 一律不占 setup。
            // 装配挪到阻塞线程池上后台执行；完成前各 Provider 停留在构造期默认（本机档），
            // 与「会话变更时经 SessionObserver 重装配」走的本就是同一条随时可重入的路径。
            {
                let app = app.handle().clone();
                tauri::async_runtime::spawn_blocking(move || {
                    use tauri::Manager as _;
                    let state = app.state::<ipc::AppState>();
                    cloud::reconfigure_cloud_stack(&app, &state);
                    // 已在新版本登录过（上次清理没来得及做完）：把旧钥匙串条目清掉。
                    secrets::spawn_legacy_cleanup(&app);
                });
            }
            // Keychain 访问可能等待系统授权或解锁，SQLite 打开也是阻塞 I/O；二者都不能
            // 占住 Tauri setup。后台激活完成后再建立 baseline，并用专用事件通知已打开的
            // 设置页刷新。生命周期锁保证它不会与“重试/清除/禁用”交错。
            ipc::activate_local_knowledge_metrics(app.handle(), metrics_enabled);
            // 任何会通过 AppHandle 取 AppState 的后台任务，都必须在 manage 之后启动。
            // Tokio 调度没有「当前闭包先执行完」的保证；提前 spawn 会在冷启动或退出竞态中
            // 触发 `state() called before manage()`，直接破坏跨启动度量与处理队列续跑。
            // 随包转写档物化（PRD-14 §6）：包内那份是母版，启动时铺一份到模型目录，
            // 此后权重与 .app 再无关系。它是「零下载即可在引导内完成首个任务」的全部
            // 依据——不铺，引导第一屏承诺的那件事在断网新装上就是空话。
            //
            // 放在阻塞线程上：一次约 31 MB 的复制不该占住 setup 的首帧预算（同上面
            // 那条「可能阻塞的 I/O 一律不占 setup」的纪律）。铺完发一条 `setup://changed`，
            // 模型页与「完成设置」横幅据此自己刷新，不必等窗口重新聚焦。
            {
                let app = app.handle().clone();
                tauri::async_runtime::spawn_blocking(move || {
                    use tauri::Manager as _;
                    let Ok(resource_dir) = app.path().resource_dir() else {
                        return;
                    };
                    let models_root = app.state::<ipc::AppState>().models_root(&app);
                    match model_store::materialize_bundled_asr(&resource_dir, &models_root) {
                        Ok(true) => {
                            use tauri_specta::Event as _;
                            let _ = ipc::SetupStatusChanged {}.emit(&app);
                        }
                        Ok(false) => {}
                        Err(error) => eprintln!("[models] 随包转写档物化失败：{error}"),
                    }
                });
            }
            // 抓取临时区清理（PRD-05-006）：上次崩溃/强退遗留的半个视频不该静默占着磁盘。
            // 必须在续跑之前——否则刚拉起的任务会与待删目录赛跑。
            ipc::purge_fetch_temp(app.handle());
            // 索引增量扫描（PRD-06-004）：启动装载不经 `bind_vault`，必须在这里也起一次——
            // 漏掉它，绝大多数只开同一个库的用户永远不会扫描（两条开库路径的老坑）。知识体检的
            // 周期提醒挂在这一轮扫描做完之后（`spawn_background_scan`），两条路径都经过那里。
            ipc::start_index_scan(app.handle());
            // 上次会话的失败 / 取消项回填（PRD-04-062）：与续跑档一样随库，但来源是用量库
            // 而非续跑档（终态不进续跑档）。必须挂在启动这一处——启动装载不经 `bind_vault`。
            ipc::restore_queue_history(app.handle());
            // 断点续跑（PRD-05-012）：上次会话未完成的转写任务已从续跑档恢复为排队态，
            // 此处拉起 worker 继续。模型按需加载在任务内部，故不阻塞启动。
            if resume_queue {
                ipc::resume_processing_queue(app.handle().clone());
            }
            // 主窗：由 Rust 隐藏创建（`tauri.conf.json` 里 `create: false`），注入首帧外观
            // 快照并按系统明暗铺底色；前端画出第一面后经 `startup_mark(first_frame)` 亮出
            // （PRD-14-066）。
            setup_main_window(app)?;
            #[cfg(target_os = "macos")]
            editor_exit::install_menu(app)?;
            // F1 弹窗翻译（PRD-01 / ADR-015）：
            //   ① 预热 popup-translate 窗口并在 show 前转为非激活 NSPanel；
            //   ② 启动双击 Cmd+C 观测，命中即在前台显示该非激活弹窗；
            //   ③ 注册 Cmd+Shift+T 备用入口，便于无辅助功能授权时验证 NSPanel。
            setup_voice_input_panel(app)?;
            #[cfg(target_os = "macos")]
            setup_screenshot_overlay_panels(app)?;
            #[cfg(target_os = "macos")]
            setup_desktop_lens_panels(app)?;
            #[cfg(target_os = "macos")]
            setup_quick_chat_panel(app)?;
            #[cfg(target_os = "macos")]
            setup_text_quick_note_panel(app)?;
            // 桌面便签按 frontmatter 恢复（ADR-119 ③）。它依赖 Vault 已装载，故排在
            // 面板预热之后；库还没打开时是空操作。
            restore_desktop_notes(app.handle());
            install_connect_link_listener(app.handle());
            if let (Some(cloud), Some(hook)) = (
                app.try_state::<luna_cloud::commands::CloudState>(),
                app.try_state::<SessionLostHook>(),
            ) {
                cloud.0.on_session_lost(std::sync::Arc::clone(&hook.0));
            }
            // 轮换验过之后由谁把它落地（ADR-124 ③）。验链是密码学判断、归 `luna-cloud`；
            // 写地址两钥 + 重钉 + 三重重置只有这一层做得到——它同时看得见设置与云客户端。
            if let Some(cloud) = app.try_state::<luna_cloud::commands::CloudState>() {
                let handle = app.handle().clone();
                cloud
                    .0
                    .set_rotation_applier(std::sync::Arc::new(move |offered| {
                        ipc::trust::apply_rotation(&handle, offered)
                    }));
            }
            setup_recording_hud_panel(app)?;
            #[cfg(target_os = "macos")]
            setup_live_caption_panel(app)?;
            setup_teleprompter_panel(app)?;
            // 全局快捷键：**设置驱动**注册（语音输入 / 快捷对话 / 录音切换 / 弹窗备用键），
            // 键位事实源 = `luna-config` 注册表/用户设置（默认 TDD-05 §3）；改键即经
            // `set_setting`→`reregister_global_shortcuts` 重注册（PRD-09-001），不再硬编码。
            app.manage(GlobalShortcutBindings::default());
            app.manage(ShortcutRegistrationStatus::default());
            // 截屏框选会话状态（PRD-01-007：记录唤起时弹窗可见性，取消/无结果原样恢复）。
            app.manage(screenshot::OverlayState::default());
            register_global_shortcuts(app.handle());
            app.state::<startup::StartupTimeline>()
                .mark(startup::StartupMilestone::ShortcutsRegistered);
            // 双击手势绑定表：设置驱动 + 热更新（任意全局快捷键可双击触发，PRD-09 §2.6）。
            app.manage(DoubleTapState(std::sync::Arc::new(
                platform::DoubleTapConfig::default(),
            )));
            apply_double_tap_settings(app.handle());

            #[cfg(target_os = "macos")]
            {
                setup_popup_panel(app)?;
                #[cfg(debug_assertions)]
                start_s15_window_exclusion_probe_if_requested(app.handle().clone());
                app.manage(KeyTapService::default());
                ensure_key_tap(app.handle());
                // 运行期权限变化：授予 → 建 tap；撤销 → 停 tap + 上报健康事件（§4）。
                start_permission_watch(app.handle().clone());
            }
            // Windows（ADR-163）：弹窗翻译窗 + 低层键盘钩子（双击手势、Esc 关弹窗）。低层钩子
            // 不需要任何授权，故没有权限轮询。
            #[cfg(target_os = "windows")]
            {
                setup_popup_panel(app)?;
                app.manage(KeyTapService::default());
                ensure_key_tap(app.handle());
            }
            // 状态栏系统资源监控（CPU/GPU 占用，PRD-04-033）：周期采样低频 typed event。
            resource_monitor::start(app.handle().clone());

            // 启动即广播一条低频 typed event（演示 event 往返；GUI 真机可见）。
            use tauri_specta::Event as _;
            ipc::AppReady {
                version: env!("CARGO_PKG_VERSION").to_string(),
            }
            .emit(app.handle())?;
            // 装配接缝：额外 setup 钩子在既有 setup 末尾逐个执行（OSS 默认装配为空）；
            // 错误按既有 setup 错误路径传播（`?`），与内建步骤同一失败语义。
            for hook in extra_setup {
                hook(app.handle())?;
            }
            // 应用更新（PRD-09-051）：待装版本的暂存，与启动后的那一次检查（按 `update.policy`、严格本地）。
            app.manage(ipc::PendingRelease::default());
            ipc::spawn_release_startup_check(app.handle().clone());
            app.state::<startup::StartupTimeline>()
                .mark(startup::StartupMilestone::SetupEnd);
            Ok(())
        })
        .build(context)
        .expect("构建 Tauri 应用出错")
        .run(|app, event| {
            // macOS 点击 Dock 图标 = `Reopen`（applicationShouldHandleReopen）。主窗口
            // 红叉是「隐藏不退出」（见 setup 内 CloseRequested 拦截），不接这条事件，
            // Dock 图标点击就毫无反应——用户唯一的找回路径只剩托盘，而 Dock 是 macOS
            // 最符合直觉的那条。语义与托盘左键完全一致：唤起主窗口（幂等）。
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                show_main_window(app);
            }
            // 真正退出时收掉随包推理引擎：SIGTERM → 3 s → SIGKILL（ADR-143 ⑤）。放在 `Exit`
            // 而不是 `ExitRequested`——后者可能被未保存的编辑器拦下，那时引擎还要继续服务。
            if matches!(event, tauri::RunEvent::Exit) {
                use tauri::Manager as _;
                let engine = app.state::<ipc::AppState>().local_engine().clone();
                tauri::async_runtime::block_on(engine.stop());
            }
            // 字幕窗的位置与尺寸平时在收起时记下；直接退出不经过收起，这里补记一次。
            if let tauri::RunEvent::ExitRequested { api, .. } = event {
                editor_exit::request(app, &api);
                remember_open_caption_windows(app);
                if let Some(win) = app.get_webview_window(ipc::TELEPROMPTER_LABEL) {
                    remember_caption_window(app, &win);
                }
            }
        });
}

/// S15 真实 Lunarca 宿主窗口排除验证。
///
/// 仅在显式设置 `LUNA_S15_WINDOW_EXCLUSION_PROBE=1` 的开发会话触发：短暂展示已预热的
/// 非激活弹窗，使它成为 ScreenCaptureKit 的 on-screen `SCWindow`，再验证其 `windowNumber`
/// 可被映射并用于 `SCContentFilter` 排除。默认路径零行为变化；不创建流、不读取像素或标题。
#[cfg(all(target_os = "macos", debug_assertions))]
fn start_s15_window_exclusion_probe_if_requested(app: tauri::AppHandle) {
    use std::time::Duration;
    use tauri::Manager as _;

    if std::env::var_os("LUNA_S15_WINDOW_EXCLUSION_PROBE").is_none() {
        return;
    }
    let Some(popup) = app.get_webview_window("popup-translate") else {
        eprintln!("[S15] probe 未启动：popup-translate 尚未预热");
        return;
    };
    if let Err(error) = popup.show() {
        eprintln!("[S15] probe 未启动：无法显示宿主面板：{error}");
        return;
    }
    eprintln!("[S15] 已显示宿主面板，等待其进入 ScreenCaptureKit 可枚举集合");

    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(700));
        let probe_app = app.clone();
        let _ = app.run_on_main_thread(move || {
            let Some(popup) = probe_app.get_webview_window("popup-translate") else {
                eprintln!("[S15] probe 失败：宿主面板已不存在");
                return;
            };
            match popup.ns_window() {
                Ok(ns_window) => {
                    if let Err(error) =
                        platform::macos::screen_capture_probe::start_host_window_exclusion_probe(
                            ns_window,
                        )
                    {
                        eprintln!("[S15] probe 启动失败：{error}");
                    }
                }
                Err(error) => eprintln!("[S15] probe 失败：无法取得宿主 NSWindow：{error}"),
            }
        });

        // 只为让枚举回调有机会执行而保持可见；随后恢复预热隐藏态。
        std::thread::sleep(Duration::from_millis(1500));
        let hide_app = app.clone();
        let _ = app.run_on_main_thread(move || {
            hide_popup_panel(&hide_app);
        });
    });
}

/// 托盘与应用包共用由 `luna-icon.svg` 生成的标准图标，避免维护第二套像素图形。
fn luna_tray_icon_as_template() -> bool {
    false
}

fn luna_tray_icon() -> tauri::Result<tauri::image::Image<'static>> {
    tauri::image::Image::from_bytes(include_bytes!("../icons/32x32.png"))
}

fn setup_tray_icon(app: &mut tauri::App) -> tauri::Result<()> {
    use tauri::tray::TrayIconBuilder;
    use tauri::Manager as _;

    // 托盘右键菜单：各功能快捷启动 + 退出（PRD-04-043 常驻入口；native 菜单文案走后端 i18n）。
    let tr = |key: &str, fallback: &str| -> String {
        app.state::<std::sync::Arc<i18n::BackendI18n>>()
            .translate(key)
            .unwrap_or_else(|| fallback.to_owned())
    };
    let menu = tray_menu(app, &tr)?;

    let tray = TrayIconBuilder::with_id("main")
        .icon(luna_tray_icon()?)
        .icon_as_template(luna_tray_icon_as_template())
        .tooltip("Lunarca")
        .menu(&menu)
        // 左键单击不弹菜单（macOS 惯例：左键唤起主窗口，右键弹菜单）。
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| handle_tray_menu(app, event.id.as_ref()))
        // 托盘左键单击 → 唤起主窗口（关闭红叉隐藏后的找回路径，配合主窗口 CloseRequested→hide）。
        .on_tray_icon_event(|tray, event| {
            use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;
    app.manage(tray);
    Ok(())
}

/// 构造托盘菜单（快捷启动各功能 + 退出）。id 与 [`handle_tray_menu`] 分派一一对应。
fn tray_menu(
    app: &tauri::App,
    tr: &dyn Fn(&str, &str) -> String,
) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
    let item = |id: &str, key: &str, fallback: &str| -> tauri::Result<MenuItem<tauri::Wry>> {
        MenuItem::with_id(app, id, tr(key, fallback), true, None::<&str>)
    };
    Menu::with_items(
        app,
        &[
            &item("tray_show_main", "tray:show_main", "打开 Lunarca 主窗口")?,
            &PredefinedMenuItem::separator(app)?,
            &item("tray_text_quick_note", "tray:text_quick_note", "文字速记")?,
            &item("tray_voice_input", "tray:voice_input", "语音输入")?,
            &item("tray_popup_translate", "tray:popup_translate", "弹窗翻译")?,
            &item(
                "tray_screenshot_translate",
                "tray:screenshot_translate",
                "截屏翻译",
            )?,
            &item("tray_quick_chat", "tray:quick_chat", "AI 对话")?,
            &item("tray_recording", "tray:recording", "开始录音")?,
            &item("tray_live_caption", "tray:live_caption", "实时字幕")?,
            &PredefinedMenuItem::separator(app)?,
            &item("tray_quit", "tray:quit", "退出 Lunarca")?,
        ],
    )
}

/// 分派托盘菜单点击（各功能唤起走与全局热键相同的常驻面板；退出走 `AppHandle::exit`）。
fn handle_tray_menu(app: &tauri::AppHandle, id: &str) {
    use tauri::Manager as _;
    match id {
        "tray_show_main" => show_main_window(app),
        "tray_voice_input" => {
            // 托盘唤起是"普通入口"：按用户的持久默认落点，且同样经会话准备函数
            // （TDD-07 §2.3：内部快捷键/菜单不得绕过它直接改运行态）。
            show_voice_hud_from_rust(app, ipc::VoiceInputHudPresetDto::Current);
        }
        "tray_text_quick_note" => show_text_quick_note_panel(app, "tray"),
        "tray_popup_translate" => {
            if let Some(state) = app.try_state::<ipc::AppState>() {
                if let Ok(mut session) = state.popup().lock() {
                    session.set_empty_input();
                }
            }
            show_popup_panel(app);
        }
        "tray_screenshot_translate" => {
            tauri::async_runtime::spawn(screenshot::run_screenshot_translate(app.clone()));
        }
        "tray_quick_chat" => show_quick_chat_panel(app),
        "tray_recording" => {
            show_recording_hud(app);
        }
        "tray_live_caption" => {
            show_live_caption(app);
        }
        // 退出：唯一的显式退出路径（红叉被拦截为 hide；托盘常驻后台）。exit 触发正常清理。
        "tray_quit" => app.exit(0),
        _ => {}
    }
}

/// 主窗创建（ADR-114 ④ ⑵）：隐藏建窗 → 注入外观快照 → 按明暗铺底色 → 关闭即隐藏 →
/// 75% 屏幕尺寸居中 → 兜底计时器（前端若迟迟不报首帧，也绝不让用户面对一个没有窗口的应用）。
fn setup_main_window(app: &mut tauri::App) -> tauri::Result<()> {
    use tauri::Manager as _;
    use tauri::{WebviewWindow, WebviewWindowBuilder};

    let config = app
        .config()
        .app
        .windows
        .iter()
        .find(|window| window.label == "main")
        .cloned()
        .ok_or_else(|| tauri::Error::WindowNotFound)?;
    let boot = {
        let state = app.state::<ipc::AppState>();
        let registry = ipc::themes::load_theme_registry(app.handle());
        boot_appearance::BootAppearance::from_settings_and_registry(
            &state.settings_snapshot(),
            &registry,
        )
    };
    let main: WebviewWindow = WebviewWindowBuilder::from_config(app, &config)?
        .visible(false)
        .initialization_script(boot.script())
        .build()?;

    // 底色：显式选了明暗就听它的；跟随系统时问窗口此刻的系统主题。
    let dark = boot
        .explicit_dark()
        .unwrap_or_else(|| matches!(main.theme(), Ok(tauri::Theme::Dark)));
    let _ = main.set_background_color(Some(boot_appearance::canvas_color(&boot, dark)));

    // 关闭语义：**隐藏到后台，不退出**（应用常驻后台 + 托盘/全局热键持续可用）。
    // 拦截红叉 CloseRequested → prevent_close + hide；经托盘左键单击或 Dock 图标
    // 点击（RunEvent::Reopen，见 run 尾部）再唤起。
    let main_for_close = main.clone();
    main.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = main_for_close.hide();
        }
    });

    // ── 动态窗口尺寸：取主屏参数，75% 逻辑尺寸 + clamp 安全边界 ──
    if let Ok(Some(monitor)) = main.primary_monitor() {
        let size = monitor.size();
        let scale = monitor.scale_factor();
        let logical_w = size.width as f64 / scale;
        let logical_h = size.height as f64 / scale;
        let target_w = (logical_w * 0.75).clamp(960.0, 1600.0);
        let target_h = (logical_h * 0.75).clamp(680.0, 1000.0);
        let _ = main.set_size(tauri::Size::Logical(tauri::LogicalSize::new(
            target_w, target_h,
        )));
        let _ = main.center();
    }

    // 兜底：前端 4 秒内没报首帧（脚本异常、devtools 断点、极慢的盘）也要亮窗——
    // 隐藏创建是为了不让用户看见空白，不是为了让他看不见应用。
    let app_for_fallback = app.handle().clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(4));
        if let Some(main) = app_for_fallback.get_webview_window("main") {
            if !main.is_visible().unwrap_or(true) {
                eprintln!("[startup] 前端 4s 内未报首帧，按兜底显示主窗");
                show_main_window(&app_for_fallback);
            }
        }
    });
    Ok(())
}

/// 唤起主窗口（托盘左键 / 菜单「打开主窗口」共用；首帧到达时亦经此亮出）。
pub(crate) fn show_main_window(app: &tauri::AppHandle) {
    use tauri::Manager as _;
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.show();
        let _ = main.unminimize();
        let _ = main.set_focus();
    }
}

/// 加载设备级配置（`<app_config_dir>/config.toml`，TDD-05 §7.2 / TDD-12 §11）。
///
/// **永不 panic、永远可用**（TDD-05 §7.3）：文件缺失 → 默认并首次回写创建；损坏 / 未知版本 →
/// 回退安全默认且**不覆盖原文件**（提示用户而非静默丢配置）。配置目录不可解析时退回纯默认。
fn load_device_settings(app: &tauri::App) -> (luna_config::Settings, ipc::SettingsPersistence) {
    use luna_config::{LoadOutcome, Settings, DEVICE_CONFIG_FILENAME};
    use tauri::Manager as _;

    let Ok(dir) = app.path().app_config_dir() else {
        eprintln!("[config] 无法解析应用配置目录，使用注册表默认设置。");
        return (
            Settings::with_defaults(),
            ipc::SettingsPersistence::MemoryOnly,
        );
    };
    let path = dir.join(DEVICE_CONFIG_FILENAME);
    let (settings, outcome) = Settings::load_path(&path);
    match outcome {
        LoadOutcome::Loaded => {
            eprintln!("[config] 已加载设备级配置：{}", path.display());
            (settings, ipc::SettingsPersistence::Writable(path))
        }
        LoadOutcome::CreatedDefault => {
            // 首次启动：把默认落盘，便于用户编辑（损坏回退路径绝不回写）。
            match settings.save_path(&path) {
                Ok(()) => eprintln!("[config] 未见配置文件，已写入默认：{}", path.display()),
                Err(e) => eprintln!("[config] 写默认配置失败（仅内存生效）：{e}"),
            }
            (settings, ipc::SettingsPersistence::Writable(path))
        }
        LoadOutcome::LoadedWithRetired(keys) => {
            // 退役键不影响运行态，也不值得打扰用户；但不能留在文件里（ADR-046 不留遗留）。
            eprintln!(
                "[config] 已加载设备级配置，移除 {} 个已退役的键：{}",
                keys.len(),
                keys.join(", ")
            );
            if let Err(e) = settings.save_path(&path) {
                eprintln!("[config] 回写规范化配置失败（仅内存生效）：{e}");
            }
            (settings, ipc::SettingsPersistence::Writable(path))
        }
        LoadOutcome::LoadedWithDropped(dropped) => {
            // 结构完好、个别项过期：其余项**照常生效**，只有这几项回到默认。
            let keys: Vec<&str> = dropped.iter().map(|d| d.key.as_str()).collect();
            eprintln!(
                "[config] 已加载设备级配置（{} 项取值已过期，回退默认）：{} — {}",
                dropped.len(),
                path.display(),
                keys.join(", ")
            );
            // 立刻把回退后的值写回，让文件与运行态一致；下次启动就不再报同样的项。
            if let Err(e) = settings.save_path(&path) {
                eprintln!("[config] 回写规范化配置失败（仅内存生效）：{e}");
            }
            if let Some(center) = app.try_state::<notifications::NotificationCenter>() {
                center.publish(
                    notifications::NotificationEvent::config_values_reset(&keys),
                    app.handle(),
                );
            }
            (settings, ipc::SettingsPersistence::Writable(path))
        }
        LoadOutcome::Recovered(reason) => {
            // 结构性损坏（TOML 语法错 / 未来版本）。原文件**留档**而非原地保留：
            // 曾经的做法是"不覆盖原文件 + 禁止一切写入"，结果整个应用变成不可配置——
            // 首次引导连 Vault 都写不进去，新用户第一次打开就卡死在引导里（2026-07-26 真机）。
            // 改为：把损坏文件改名留档（用户仍可取回内容），配置照常可写。
            let archived = archive_corrupt_config(&path);
            eprintln!(
                "[config] ⚠ 配置结构损坏，已回退安全默认：{reason}（原文件留档：{}）",
                archived
                    .as_deref()
                    .map_or_else(|| "留档失败".to_string(), |p| p.display().to_string())
            );
            if let Some(center) = app.try_state::<notifications::NotificationCenter>() {
                center.publish(
                    notifications::NotificationEvent::config_recovered(),
                    app.handle(),
                );
            }
            (settings, ipc::SettingsPersistence::Writable(path))
        }
    }
}

/// 把损坏的配置文件改名留档，返回留档路径。
///
/// 留档而非删除：用户的原始配置可能有他手写的内容，出了问题要能取回。
/// 留档而非"原地不动 + 禁写"：后者会让应用彻底不可配置（见调用点说明）。
fn archive_corrupt_config(path: &std::path::Path) -> Option<std::path::PathBuf> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let archived = path.with_extension(format!("corrupt-{stamp}.toml"));
    std::fs::rename(path, &archived).ok().map(|()| archived)
}

/// 显示已预热的非激活弹窗面板，并**可靠刷新其会话视图**（纯窗口动作）。
///
/// **刷新桥 = `win.eval`**（不用 tauri typed event）：预热隐藏窗口为长驻、按 show 复用，其 webview
/// 只挂载一次；fire-and-forget 事件对它不可靠（emit 早于监听就绪即丢失、不重放）。故每次 show 后
/// 直接注入 JS 调用前端 `__lunaRefreshPopup()`，由前端 `popup_session_state()` 拉取会话真值渲染——
/// 每次 show 必触发、无事件投递时序竞态（决策 A）。
///
/// **ESC 关闭不走全局热键**（已证其动态 register 阻塞主线程 / 与插件锁重入死锁）——改由已在跑的
/// `CGEventTap` 观测 `Escape`（[`TapEvent::Escape`]，ListenOnly 不吞键），见 `start_key_tap`。
pub(crate) fn show_popup_panel(app: &tauri::AppHandle) {
    use tauri::Manager as _;
    if let Some(win) = app.get_webview_window("popup-translate") {
        let (mode, has_source) = app
            .try_state::<ipc::AppState>()
            .and_then(|state| {
                state
                    .popup()
                    .lock()
                    .ok()
                    .map(|session| (session.mode(), session.source_text().is_some()))
            })
            .unwrap_or_default();
        if let Some(state) = app.try_state::<ipc::AppState>() {
            if let Ok(mut blur) = state.popup_blur().lock() {
                blur.arm_for_show(mode);
            }
        }
        // 输入态回到紧凑首帧（ADR-112 ④：结果区首 token 才出现）；有结果 / 展示模式沿用当前
        // 尺寸。跨状态的尺寸重置是显式状态变化，不受档内「不缩小」守卫约束（ADR-112 ③）。
        if mode == popup::PopupMode::Input && !has_source {
            if let (Ok(size), Ok(scale)) = (win.inner_size(), win.scale_factor()) {
                let width = f64::from(size.width) / scale;
                let _ = win.set_size(tauri::Size::Logical(tauri::LogicalSize::new(
                    width,
                    POPUP_TRANSLATE_HUD_GEOMETRY.initial_height,
                )));
            }
        }
        position_popup(&win, mode, has_source); // show 前就位，避免首帧闪到错误屏幕

        // Windows（ADR-163 ①）：展示模式不可激活——源窗口保住焦点与选区，「替换原文」才有目标；
        // 输入模式要收键盘，改为可激活并在显示后激活（没有源选区要保）。macOS 由 NSPanel 的
        // `becomesKeyOnlyIfNeeded` 处理（TDD-05 §5.1.1）。
        #[cfg(target_os = "windows")]
        let _ = win.set_focusable(mode == popup::PopupMode::Input);
        let _ = win.show();
        #[cfg(target_os = "windows")]
        if mode == popup::PopupMode::Input {
            let _ = win.set_focus();
        }
        // 会话源文本须在调用本函数前已写入（set_display/clear），故此处拉取即得最新真值。
        let _ = win.eval("window.__lunaRefreshPopup && window.__lunaRefreshPopup()");
    }
}

/// 把弹窗定位到选区 bounds 或鼠标光标附近（TDD-05 §5.2 跟随光标/选区）。
///
/// 展示模式优先用 AX `AXBoundsForRange` 跟随源选区；读取失败或输入模式则退回 Tauri
/// `cursor_position()`。多屏 clamp 走 `popup::place_popup` 纯逻辑，避免 `primary_monitor()` 把副屏
/// 触发的弹窗夹回主屏。**手动拖动非激活面板在 macOS 不可靠**，故 HUD 走自动定位。
fn position_popup(win: &tauri::WebviewWindow, mode: popup::PopupMode, has_source: bool) {
    // **坐标系铁律**：cursor_position()/available_monitors()/set_position() 全为**物理像素**；而 AX
    // `AXBoundsForRange` 返回**点坐标**（top-left 原点）。Retina（scale>1）下两者差一个 scale_factor，
    // 故 AX 选区 bounds 必须 ×scale 转物理像素后再与 monitors/cursor 同空间 clamp，否则面板偏左上。
    let scale = win.scale_factor().unwrap_or(1.0);
    let ax_bounds = if mode == popup::PopupMode::Display && has_source {
        #[cfg(target_os = "macos")]
        {
            platform::macos::selection::read_selected_text_bounds().map(|b| {
                popup::ScreenRect::new(b.x * scale, b.y * scale, b.width * scale, b.height * scale)
            })
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = scale;
            None
        }
    } else {
        None
    };

    let anchor = if let Some(bounds) = ax_bounds {
        bounds
    } else {
        // cursor_position() 已是物理像素，无需缩放。
        let Ok(cursor) = win.cursor_position() else {
            return;
        };
        popup::ScreenRect::from_point(popup::ScreenPoint::new(cursor.x, cursor.y))
    };

    let Ok(size) = win.outer_size() else {
        return;
    };
    let pos = popup::place_popup(
        anchor,
        popup::ScreenSize::new(size.width as f64, size.height as f64),
        &collect_monitors(win),
    );
    let _ = win.set_position(tauri::PhysicalPosition::new(pos.x as i32, pos.y as i32));
}

/// 收集全部显示器为纯逻辑屏幕矩形（物理像素），供 `popup::place_*` clamp 使用。
pub(crate) fn collect_monitors(win: &tauri::WebviewWindow) -> Vec<popup::ScreenRect> {
    win.available_monitors().map_or_else(
        |_| Vec::new(),
        |monitors| {
            // 取**工作区**而不是整屏：整屏把程序坞与菜单栏也算作可用，贴着屏幕右缘唤起的速记窗
            // 一半压在程序坞底下（真机 2026-10-01）。「整窗完整可见」（PRD-02-019）说的是看得见。
            monitors
                .into_iter()
                .map(|m| {
                    let area = m.work_area();
                    popup::ScreenRect::new(
                        f64::from(area.position.x),
                        f64::from(area.position.y),
                        f64::from(area.size.width),
                        f64::from(area.size.height),
                    )
                })
                .collect()
        },
    )
}

/// HUD 面板与光标的垂直间距（物理像素；语音输入 / 快捷对话 / 录音 HUD 统一）。
const HUD_CURSOR_GAP: f64 = 24.0;

/// 把 HUD 面板定位到光标下方居中，并 clamp 到光标所在屏内**完整可见**（多屏；
/// 纯逻辑在 [`popup::place_panel_at_cursor`]，有单测）。取不到光标/尺寸则保持原位。
fn position_panel_near_cursor(win: &tauri::WebviewWindow) {
    let Ok(cursor) = win.cursor_position() else {
        return;
    };
    let Ok(size) = win.outer_size() else {
        return;
    };
    let pos = popup::place_panel_at_cursor(
        popup::ScreenPoint::new(cursor.x, cursor.y),
        popup::ScreenSize::new(size.width as f64, size.height as f64),
        HUD_CURSOR_GAP,
        &collect_monitors(win),
    );
    let _ = win.set_position(tauri::PhysicalPosition::new(pos.x as i32, pos.y as i32));
}

/// 隐藏弹窗面板（纯窗口动作，不动会话）。
pub(crate) fn hide_popup_panel(app: &tauri::AppHandle) {
    use tauri::Manager as _;
    if let Some(win) = app.get_webview_window("popup-translate") {
        let _ = win.hide();
    }
}

/// 关闭弹窗：清空会话（关窗即清、不落盘，PRD-01-016）+ 隐藏面板。
///
/// 失焦自动收起 / ESC（经 tap）/ 关闭按钮共用此单一路径（TDD-05 §5.2）。
pub(crate) fn close_popup(app: &tauri::AppHandle) {
    use tauri::Manager as _;
    if let Some(state) = app.try_state::<ipc::AppState>() {
        let _ = state.cancel_voice_holder(crate::arbiter::MicHolder::PopupVoiceButton);
        // 关窗即停：在途的生成随会话一起作废（否则它继续跑完并计费，而结果无处可去）。
        state.cancel_popup_runs(None);
        if let Ok(mut session) = state.popup().lock() {
            session.clear();
        }
    }
    hide_popup_panel(app);
}

/// 显示语音输入 HUD，并把热键动作转发给前端桥接函数。
///
/// 前端拿到 `pressed/released` 后调用 `voice_input_start/stop`，以便为每次会话提供
/// `ipc::Channel<VoiceInputEvent>`。Rust 热键回调不直接持有前端 channel。
pub(crate) fn show_voice_input_hud(app: &tauri::AppHandle, action: &str) {
    // 全局热键同样是"普通入口"，且同样经会话准备函数（TDD-07 §2.3）。
    show_voice_hud_from_rust(app, ipc::VoiceInputHudPresetDto::Current);
    use tauri::Manager as _;
    let Some(win) = app.get_webview_window("voice-input-hud") else {
        eprintln!("[F2] voice-input-hud 窗口不存在，无法响应热键：action={action}");
        return;
    };
    let script = format!("window.__lunaVoiceHotkey && window.__lunaVoiceHotkey({action:?})");
    match win.eval(&script) {
        Ok(()) => eprintln!("[F2] voice-input HUD 桥已调用：action={action}"),
        Err(e) => eprintln!("[F2] voice-input HUD 桥调用失败：{e}"),
    }
}

/// Rust 侧唤起 HUD 的统一入口（托盘 / 全局热键 / 双击手势）。
///
/// **不直接调 `show_voice_input_hud_panel`**：那条路会绕过会话准备函数，于是"这一次
/// 该落到哪儿"从来没被回答过，HUD 显示的是上一次会话遗留的运行态（TDD-07 §2.3）。
/// 会话忙时如实忽略错误——HUD 已经显示出来了，用户要的正是回到那个会话。
pub(crate) fn show_voice_hud_from_rust(
    app: &tauri::AppHandle,
    preset: ipc::VoiceInputHudPresetDto,
) {
    use tauri::Manager as _;
    let Some(state) = app.try_state::<ipc::AppState>() else {
        return;
    };
    let _ = ipc::show_voice_hud_with_preset(app, &state, preset);
}

/// 仅显示语音输入 HUD 面板（不触发热键桥）——**只给 `show_voice_hud_with_preset` 用**。
///
/// 返回窗口句柄以便调用方继续操作；窗口未预热时返回 `None`。
pub(crate) fn show_voice_input_hud_panel(app: &tauri::AppHandle) -> Option<tauri::WebviewWindow> {
    use tauri::Manager as _;
    let win = app.get_webview_window("voice-input-hud")?;
    // 仅在隐藏→显示时定位（可见期间连按热键不跳位）：固定模式用记忆坐标，否则跟随光标。
    if !win.is_visible().unwrap_or(false) {
        position_voice_input_hud(app, &win);
    }
    if let Err(e) = win.show() {
        eprintln!("[F2] voice-input HUD show 失败：{e}");
    }
    Some(win)
}

/// 语音 HUD 定位（PRD-02-019 可固定位置）：`voice_input.hud_pinned` 开 → 用记忆坐标
/// （经 [`popup::clamp_saved_position`] 校正屏幕布局变化后的屏外坐标）；关 → 跟随光标。
fn position_voice_input_hud(app: &tauri::AppHandle, win: &tauri::WebviewWindow) {
    use tauri::Manager as _;
    let pinned = app.try_state::<ipc::AppState>().and_then(|state| {
        let settings = state.settings_snapshot();
        if !settings.get_bool("voice_input.hud_pinned").unwrap_or(false) {
            return None;
        }
        let x = settings.get_int("voice_input.hud_pin_x")?;
        let y = settings.get_int("voice_input.hud_pin_y")?;
        Some(popup::ScreenPoint::new(x as f64, y as f64))
    });
    match pinned {
        Some(saved) => {
            let Ok(size) = win.outer_size() else {
                return;
            };
            let pos = popup::clamp_saved_position(
                saved,
                popup::ScreenSize::new(size.width as f64, size.height as f64),
                &collect_monitors(win),
            );
            let _ = win.set_position(tauri::PhysicalPosition::new(pos.x as i32, pos.y as i32));
        }
        None => position_panel_near_cursor(win),
    }
}

/// 显示快捷对话非激活面板，并把输入焦点交给前端输入框。
///
/// 不读取前台选区、不自动带上下文；只打开手动输入/粘贴入口（Phase 5 Q4）。
pub(crate) fn show_quick_chat_panel(app: &tauri::AppHandle) {
    use tauri::Manager as _;
    if let Some(win) = app.get_webview_window("quick-chat") {
        if !win.is_visible().unwrap_or(false) {
            position_panel_near_cursor(&win);
        }
        if let Err(e) = win.show() {
            eprintln!("[C9] quick-chat 面板 show 失败：{e}");
            return;
        }
        if let Err(e) = win.eval("window.__lunaFocusChatInput && window.__lunaFocusChatInput()") {
            eprintln!("[C9] quick-chat 输入焦点桥调用失败：{e}");
        }
    } else {
        eprintln!("[C9] quick-chat 窗口不存在，无法响应热键。");
    }
}

/// 预热 `popup-translate` 窗口并转为非激活面板（ADR-015；Windows 见 ADR-163 ①）。
///
/// **预热即转换（TDD-05 §5.2）**：隐藏态创建 → 立即 Rust 侧转面板（在主线程、首次 `show()` 前），
/// 之后只 `show()/hide()`。否则预热出的仍是会夺焦的普通窗。
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn setup_popup_panel(app: &mut tauri::App) -> tauri::Result<()> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};

    let popup =
        WebviewWindowBuilder::new(app, "popup-translate", WebviewUrl::App("index.html".into()))
            .title(hud_window_title(
                &*app,
                "window-title-popup-translate",
                "Lunarca 弹窗翻译",
            ))
            .inner_size(
                POPUP_TRANSLATE_HUD_GEOMETRY.initial_width,
                POPUP_TRANSLATE_HUD_GEOMETRY.initial_height,
            )
            .min_inner_size(
                POPUP_TRANSLATE_HUD_GEOMETRY.min_width,
                POPUP_TRANSLATE_HUD_GEOMETRY.min_height,
            )
            .resizable(true)
            .hud_chrome()
            .non_activating()
            .visible(false) // 隐藏态预热；转换后由 spike 触发 show()
            .build()?;

    #[cfg(target_os = "windows")]
    install_windows_panel(&popup);
    #[cfg(target_os = "macos")]
    match popup.ns_window() {
        Ok(ns_window) => {
            // SAFETY: setup 在主线程；ns_window 为有效 NSWindow；尚未 show（预热即转换）。
            unsafe {
                platform::macos::convert_to_nonactivating_panel(ns_window);
                platform::macos::enable_input_mode_focus(ns_window);
            }
            eprintln!("[ADR-015] popup-translate 已预热并转为非激活 NSPanel（隐藏待显）。");
        }
        Err(e) => eprintln!("[ADR-015] 取 ns_window 失败：{e}（无法转面板）"),
    }
    let app_handle = app.handle().clone();
    popup.on_window_event(move |event| {
        if let tauri::WindowEvent::Focused(focused) = event {
            use tauri::Manager as _;
            let should_close = app_handle
                .try_state::<ipc::AppState>()
                .and_then(|state| {
                    state
                        .popup_blur()
                        .lock()
                        .ok()
                        .map(|mut blur| blur.on_focus_changed(*focused))
                })
                .unwrap_or(false);
            if should_close {
                close_popup(&app_handle);
            }
        }
    });
    Ok(())
}

/// 预热 `voice-input-hud` 窗口并转为非激活面板（ADR-015；Windows 见 ADR-163 ①，
/// [`HudWindowChrome::non_activating`]）。
fn setup_voice_input_panel(app: &mut tauri::App) -> tauri::Result<()> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};

    let hud =
        WebviewWindowBuilder::new(app, "voice-input-hud", WebviewUrl::App("index.html".into()))
            .title(hud_window_title(
                &*app,
                "window-title-voice-input",
                "Lunarca 语音输入",
            ))
            .inner_size(
                VOICE_INPUT_HUD_GEOMETRY.initial_width,
                VOICE_INPUT_HUD_GEOMETRY.initial_height,
            )
            .min_inner_size(
                VOICE_INPUT_HUD_GEOMETRY.min_width,
                VOICE_INPUT_HUD_GEOMETRY.min_height,
            )
            .resizable(true)
            .hud_chrome()
            .non_activating()
            .visible(false)
            .build()?;

    #[cfg(target_os = "windows")]
    install_windows_panel(&hud);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let _ = hud;
    #[cfg(target_os = "macos")]
    match hud.ns_window() {
        // 语音 HUD **不调** enable_input_mode_focus：它没有文本输入，必须严格非激活、永不夺取
        // 键焦点——否则会把焦点从源 App 抢走，导致注入目标（源 App 光标）丢失（TextInjector
        // 写 kAXFocusedUIElement）。仅转为非激活面板即可（可接收鼠标点击但不成为 key window）。
        Ok(ns_window) => unsafe {
            platform::macos::convert_to_nonactivating_panel(ns_window);
        },
        Err(e) => eprintln!("[F2] 取 voice-input-hud ns_window 失败：{e}（无法转面板）"),
    }
    Ok(())
}

/// 随包引擎首轮编排之后的默认对话模型核查：槽位指向的模型没装 → 发通知引导去模型页
/// 切换 / 下载（PRD-01 §异常「主动引导，不等首次生成才失败」）。判定纯逻辑在
/// [`model_store::assess_llm_readiness`]；已装清单取引擎编排时记下的那份（专用翻译模型不算
/// 对话模型）。
///
/// 算力在服务端时**不核查、不发通知**：对话在服务端跑，本机装没装与它无关——此前无条件
/// 核查让「连了服务端、本机一个模型都没装」这个完全正常的形态每次启动都弹一条去下载。
pub(crate) async fn check_llm_readiness(app: &tauri::AppHandle) {
    use tauri::Manager as _;
    let state = app.state::<ipc::AppState>();
    if ipc::current_chat_compute_location(app, &state).is_server() {
        return;
    }
    let settings = state.settings_snapshot();
    let configured = state
        .effective_assignment(&settings, "default_llm")
        .unwrap_or_default();
    let installed = state
        .engine_models_snapshot()
        .into_iter()
        .filter(|m| m.kind == local_engine::EngineModelKind::Chat && !m.translation_only)
        .map(|m| m.id)
        .collect::<Vec<_>>();
    // 通知里给人看的是模型名，不是条目 id。
    let store = model_store::ModelStore::load(state.models_root(app));
    let label = |id: &str| store.entry(id).map_or_else(|| id.to_owned(), |e| e.name);
    let event = match model_store::assess_llm_readiness(&configured, &installed) {
        model_store::LlmReadiness::Ready => return,
        model_store::LlmReadiness::MissingWithAlternatives { configured } => {
            // 条目 id 非用户内容，可入日志（TDD-12 脱敏纪律不受影响）。
            eprintln!(
                "[engine] 默认对话模型 {configured} 未安装（本机有其他可用模型），已通知引导切换"
            );
            notifications::NotificationEvent::llm_default_model_missing(&label(&configured))
        }
        model_store::LlmReadiness::NothingInstalled { configured } => {
            eprintln!("[engine] 本机没有已安装的对话模型（配置 {configured}），已通知引导下载");
            notifications::NotificationEvent::llm_no_models_installed()
        }
    };
    if let Some(center) = app.try_state::<notifications::NotificationCenter>() {
        center.publish(event, app);
    }
}

/// 截屏框选层窗口标签前缀：**每个显示器一面板**，标签 = 前缀 + 显示器序号
/// （capability 侧以 `screenshot-overlay-*` glob 授权）。
#[cfg(target_os = "macos")]
pub(crate) const SCREENSHOT_OVERLAY_PREFIX: &str = "screenshot-overlay-";

/// 常驻译境取景器与其独立触发器。镜框本体静默时鼠标穿透；触发器是唯一接收 hover/click
/// 的小面板，避免透明大窗截获用户对其它 App 的操作（PRD-01-036）。
pub(crate) const DESKTOP_LENS_WINDOW: &str = "desktop-lens";
pub(crate) const DESKTOP_LENS_TRIGGER: &str = "desktop-lens-trigger";

/// 标签是否属于截屏框选层面板池。
#[cfg(target_os = "macos")]
pub(crate) fn is_screenshot_overlay_label(label: &str) -> bool {
    label.starts_with(SCREENSHOT_OVERLAY_PREFIX)
}

/// 光标（全局物理坐标）是否落在该显示器内。
#[cfg(target_os = "macos")]
fn monitor_contains_cursor(
    monitor: &tauri::Monitor,
    cursor: &tauri::PhysicalPosition<f64>,
) -> bool {
    let pos = monitor.position();
    let size = monitor.size();
    cursor.x >= pos.x as f64
        && cursor.x < (pos.x + size.width as i32) as f64
        && cursor.y >= pos.y as f64
        && cursor.y < (pos.y + size.height as i32) as f64
}

/// 取或建第 `index` 块屏的框选层面板（截屏翻译 Snipaste 式框选层，TDD-05 §1.5 / PRD-01-007）。
///
/// 全屏透明覆盖层：暗化蒙层 + 拖拽选区 + 尺寸徽标全在前端自绘；面板仅接收鼠标、
/// 不夺键焦点（框选期间前台 App 上下文不变）。窗口惰性建池、跨次复用（显示器增减
/// 由 [`show_screenshot_overlay`] 每次按当前排列同步）。
#[cfg(target_os = "macos")]
fn ensure_screenshot_overlay_panel<M>(app: &M, index: usize) -> tauri::Result<tauri::WebviewWindow>
where
    M: tauri::Manager<tauri::Wry>,
{
    use tauri::{WebviewUrl, WebviewWindowBuilder};

    let label = format!("{SCREENSHOT_OVERLAY_PREFIX}{index}");
    if let Some(win) = app.get_webview_window(&label) {
        return Ok(win);
    }
    let overlay = WebviewWindowBuilder::new(app, &label, WebviewUrl::App("index.html".into()))
        .title(hud_window_title(
            app,
            "window-title-screenshot",
            "Lunarca 截屏翻译",
        ))
        .inner_size(800.0, 600.0) // 兜底初始值；显示时按对应屏全屏就位
        .decorations(false)
        .skip_taskbar(true)
        .transparent(true)
        .visible(false)
        .build()?;

    #[cfg(target_os = "macos")]
    match overlay.ns_window() {
        // 框选层只接收鼠标（拖选/右键取消），严格非激活；Esc 经 CGEventTap（同弹窗关闭路径）。
        Ok(ns_window) => unsafe {
            platform::macos::convert_to_nonactivating_panel(ns_window);
        },
        Err(e) => eprintln!("[screenshot] 取 {label} ns_window 失败：{e}（无法转面板）"),
    }
    Ok(overlay)
}

/// 启动预热截屏框选层面板池（按当前显示器数量各建一面板，show 前完成 NSPanel 转换，
/// TDD-05 §5.2 时序铁律）。运行期插拔显示器由 [`show_screenshot_overlay`] 增量补建。
#[cfg(target_os = "macos")]
fn setup_screenshot_overlay_panels(app: &mut tauri::App) -> tauri::Result<()> {
    let count = app
        .available_monitors()
        .map(|m| m.len())
        .unwrap_or(1)
        .max(1);
    for index in 0..count {
        ensure_screenshot_overlay_panel(app, index)?;
    }
    Ok(())
}

/// 预热译境与其贴边控制条；两者都在首次展示前转换为非激活面板（ADR-015）。
#[cfg(target_os = "macos")]
fn setup_desktop_lens_panels(app: &mut tauri::App) -> tauri::Result<()> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};

    let lens = WebviewWindowBuilder::new(
        app,
        DESKTOP_LENS_WINDOW,
        WebviewUrl::App("index.html".into()),
    )
    .title(hud_window_title(
        app,
        "window-title-desktop-lens",
        "Lunarca 译境",
    ))
    .inner_size(560.0, 320.0)
    .decorations(false)
    .resizable(false)
    .skip_taskbar(true)
    .transparent(true)
    .visible(false)
    .build()?;
    if let Ok(ns_window) = lens.ns_window() {
        // SAFETY: setup 在主线程；窗口尚未 show。静默态必须穿透鼠标，编辑态才反转。
        unsafe {
            platform::macos::convert_to_nonactivating_panel(ns_window);
            platform::macos::set_ignores_mouse_events(ns_window, true);
        }
    }

    let trigger = WebviewWindowBuilder::new(
        app,
        DESKTOP_LENS_TRIGGER,
        WebviewUrl::App("index.html".into()),
    )
    .title(hud_window_title(
        app,
        "window-title-desktop-lens-trigger",
        "Lunarca 译境控制",
    ))
    // 逻辑点，须恰好容下 `.luna-desktop-lens-trigger` 的 border-box（CSS 侧同口径注释）：
    //   宽 = 2×border(1) + 2×padding(4) + 3×键(28) + 3×gap(4) + 分隔(1 + 2×2) = 111 → 112
    //   高 = 2×border(1) + 2×padding(4) + 键(28) = 38
    // 窗口小于内容不会滚动、只会裁切，故键数或尺寸 Token 变动时必须同步重算这两个数。
    .inner_size(112.0, 38.0)
    .decorations(false)
    .resizable(false)
    .skip_taskbar(true)
    .transparent(true)
    .visible(false)
    .build()?;
    if let Ok(ns_window) = trigger.ns_window() {
        // SAFETY: 同上；触发器必须接收 hover，但仍不能激活前台 App。
        unsafe { platform::macos::convert_to_nonactivating_panel(ns_window) };
    }
    Ok(())
}

/// 取景框的最小尺寸（**逻辑点**）。Rust 是权威夹紧点（加载与保存都过），前端拖拽缩放同口径
/// 夹紧仅为交互手感，最终以此处为准（同 `sidebar_width` 的权威 clamp 先例）。
pub(crate) const DESKTOP_LENS_MIN_WIDTH_POINTS: i64 = 160;
pub(crate) const DESKTOP_LENS_MIN_HEIGHT_POINTS: i64 = 100;

/// 取景框在当前显示器上居中（逻辑点）。
#[cfg(target_os = "macos")]
fn center_lens_on_current_monitor(lens: &tauri::WebviewWindow, size: tauri::LogicalSize<f64>) {
    let Ok(Some(monitor)) = lens.current_monitor() else {
        return;
    };
    let scale = monitor.scale_factor();
    let origin = monitor.position().to_logical::<f64>(scale);
    let bounds = monitor.size().to_logical::<f64>(scale);
    let _ = lens.set_position(tauri::LogicalPosition::new(
        origin.x + (bounds.width - size.width).max(0.0) / 2.0,
        origin.y + (bounds.height - size.height).max(0.0) / 2.0,
    ));
}

/// 显示静默的固定取景器并把唯一触发器贴到其右上角。
///
/// **单位铁律**：译境的窗口几何全程走**逻辑点**——窗口按逻辑点创建、按逻辑点定位与缩放、
/// 按逻辑点持久化，`screencapture -R` 要的也正是全局逻辑点。这样混合 DPI 下不做任何二次
/// 换算，同一份几何在 Retina 与非 Retina 屏上呈现同样大小，也不会出现读物理、写逻辑（或
/// 反之）导致的 2× 错位。
#[cfg(target_os = "macos")]
pub(crate) fn show_desktop_lens(app: &tauri::AppHandle) {
    use tauri::Manager as _;
    let Some(lens) = app.get_webview_window(DESKTOP_LENS_WINDOW) else {
        return;
    };
    // 注册表保证这些键存在且有默认值（560×320 点），故此处不复写第二份默认常量。
    let settings = app
        .try_state::<ipc::AppState>()
        .map(|state| state.settings_snapshot())
        .unwrap_or_default();
    let width = settings
        .get_int("desktop_lens.width")
        .unwrap_or_default()
        .max(DESKTOP_LENS_MIN_WIDTH_POINTS) as f64;
    let height = settings
        .get_int("desktop_lens.height")
        .unwrap_or_default()
        .max(DESKTOP_LENS_MIN_HEIGHT_POINTS) as f64;
    let x = settings.get_int("desktop_lens.x").unwrap_or_default() as f64;
    let y = settings.get_int("desktop_lens.y").unwrap_or_default() as f64;
    let size = tauri::LogicalSize::new(width, height);
    let _ = lens.set_size(size);

    // 已保存几何必须完整落在**当前**某块显示器内才复用（判据与采集侧同一函数）：显示器拔出
    // 或分辨率变化后的陈旧坐标会让取景框停在不可见处，用户既找不到它、也点不到控制条关闭。
    // 不可见即视同"尚未放置"——居中并请用户重新放一次，而不是留一个幽灵窗口。
    let saved_visible = settings.get_bool("desktop_lens.placed").unwrap_or_default()
        && screenshot::rect_within_single_display(
            &screenshot::SelectionRect {
                x,
                y,
                width,
                height,
            },
            &screenshot::display_bounds_in_points(app),
        );
    if saved_visible {
        let _ = lens.set_position(tauri::LogicalPosition::new(x, y));
    } else {
        center_lens_on_current_monitor(&lens, size);
    }
    let placed = saved_visible;
    #[cfg(target_os = "macos")]
    if let Ok(ns_window) = lens.ns_window() {
        // SAFETY: 已在 setup 转为面板；本调用在主线程的 Tauri command/启动路径执行。
        unsafe { platform::macos::set_ignores_mouse_events(ns_window, true) };
    }
    let _ = lens
        .eval("window.__lunaDesktopLensSetEditing && window.__lunaDesktopLensSetEditing(false)");
    let _ = lens.show();
    if let Some(state) = app.try_state::<ipc::AppState>() {
        state.desktop_lens_activate();
    }
    ipc::emit_desktop_lens_changed(app, true);
    if placed {
        sync_desktop_lens_trigger(app);
    } else {
        // 第一次没有可辨识的小触发器可供用户寻找：直接进入可调整态，完成后才回到穿透的
        // 静默镜框。这一初次设置不改变后续“悬停触发器才实体化”的低干扰契约。
        set_desktop_lens_editing(app, true);
    }
}

/// 会话运行期把取景器重新显示出来（单次拍摄结束后恢复可见）。
///
/// 与 [`show_desktop_lens`] 分工明确：那是**会话启动**（读几何、定位、置运行态、首次进调整
/// 态、广播状态）；本函数只做"恢复可见"——不重跑首次定位分支、不改运行态、不重发状态事件。
/// 因此一次拍摄既不会把用户放好的取景框弹回屏幕中央（设备设置不可写时 `placed` 仍为 false
/// 也不误判为"尚未定位"），也不会向前端重复广播一遍"已开启"。
#[cfg(target_os = "macos")]
pub(crate) fn reveal_desktop_lens(app: &tauri::AppHandle) {
    use tauri::Manager as _;
    let Some(lens) = app.get_webview_window(DESKTOP_LENS_WINDOW) else {
        return;
    };
    let _ = lens.show();
    sync_desktop_lens_trigger(app);
}

/// 控制条与取景框之间的外挂间距（逻辑点）。几何全程走逻辑点，故这里就是最终观感间距，
/// 不再按 scale 二次换算。
const LENS_TRIGGER_GAP_POINTS: f64 = 6.0;

/// 控制条相对取景框的贴边位置（**逻辑点**，纯逻辑，可单测）。
///
/// 默认悬挂在取景框**外侧上缘**：控制条常驻于用户正在阅读的内容之上，压在取景范围里会
/// 恰好挡住该区域右上角的正文。上方放不下时（取景框贴着屏幕顶边）翻回框内上缘，保证控制条
/// 始终留在当前屏可见范围内、绝不飞出屏幕（翻边原则同 `PRD-01-035`）。
///
/// 横向恒与取景框右缘对齐；`display_top` 为 `None` 表示显示器信息不可读，此时不施加上边界
/// 约束（宁可外挂也不无谓地压回内容上）。
fn place_lens_trigger(
    lens_position: tauri::LogicalPosition<f64>,
    lens_size: tauri::LogicalSize<f64>,
    trigger_size: tauri::LogicalSize<f64>,
    display_top: Option<f64>,
) -> tauri::LogicalPosition<f64> {
    let x = lens_position.x + lens_size.width - trigger_size.width;
    let above = lens_position.y - trigger_size.height - LENS_TRIGGER_GAP_POINTS;
    let y = if above >= display_top.unwrap_or(f64::NEG_INFINITY) {
        above
    } else {
        lens_position.y
    };
    tauri::LogicalPosition::new(x, y)
}

/// 窗口当前的逻辑点几何（位置 + 外框尺寸）。读侧唯一换算点：物理 ÷ 本窗口 scale。
pub(crate) fn window_logical_geometry(
    window: &tauri::WebviewWindow,
) -> Option<(tauri::LogicalPosition<f64>, tauri::LogicalSize<f64>)> {
    let scale = window.scale_factor().ok().filter(|s| *s > 0.0)?;
    let position = window.outer_position().ok()?.to_logical::<f64>(scale);
    let size = window.outer_size().ok()?.to_logical::<f64>(scale);
    Some((position, size))
}

fn sync_desktop_lens_trigger(app: &tauri::AppHandle) {
    use tauri::Manager as _;
    let (Some(lens), Some(trigger)) = (
        app.get_webview_window(DESKTOP_LENS_WINDOW),
        app.get_webview_window(DESKTOP_LENS_TRIGGER),
    ) else {
        return;
    };
    let Some((position, size)) = window_logical_geometry(&lens) else {
        return;
    };
    let Some((_, trigger_size)) = window_logical_geometry(&trigger) else {
        return;
    };
    // 取景框所在屏的上边界（逻辑点）：控制条外挂后不得越出该屏顶边。
    let display_top = lens.current_monitor().ok().flatten().map(|monitor| {
        monitor
            .position()
            .to_logical::<f64>(monitor.scale_factor())
            .y
    });
    let _ = trigger.set_position(place_lens_trigger(
        position,
        size,
        trigger_size,
        display_top,
    ));
    let _ = trigger.show();
}

/// 在用户明确悬停触发器后进入调整态；否则镜框始终保持鼠标穿透。
pub(crate) fn set_desktop_lens_editing(app: &tauri::AppHandle, editing: bool) {
    use tauri::Manager as _;
    let Some(lens) = app.get_webview_window(DESKTOP_LENS_WINDOW) else {
        return;
    };
    #[cfg(target_os = "macos")]
    if let Ok(ns_window) = lens.ns_window() {
        // SAFETY: 同 [`show_desktop_lens`]，只切换 AppKit 的鼠标命中策略。
        unsafe { platform::macos::set_ignores_mouse_events(ns_window, !editing) };
    }
    let _ = lens.eval(format!(
        "window.__lunaDesktopLensSetEditing && window.__lunaDesktopLensSetEditing({editing})"
    ));
    if editing {
        if let Some(trigger) = app.get_webview_window(DESKTOP_LENS_TRIGGER) {
            let _ = trigger.hide();
        }
    } else {
        sync_desktop_lens_trigger(app);
    }
}

pub(crate) fn hide_desktop_lens(app: &tauri::AppHandle) {
    use tauri::Manager as _;
    for label in [DESKTOP_LENS_WINDOW, DESKTOP_LENS_TRIGGER] {
        if let Some(window) = app.get_webview_window(label) {
            let _ = window.hide();
        }
    }
    if let Some(state) = app.try_state::<ipc::AppState>() {
        state.desktop_lens_end();
    }
    ipc::emit_desktop_lens_changed(app, false);
}

/// 显示截屏框选层：**所有显示器同时铺满各自面板**（多屏截屏惯例，Snipaste/CleanShot 同型），
/// 并经 eval 桥重置前端框选状态（与弹窗刷新桥同模式）；光标所在屏立即显示提示条。
/// 混合 DPI 下按各屏 scale 换算为**逻辑坐标**就位（物理坐标经窗口旧 scale 反算会错位）。
/// 返回是否至少成功显示一块屏的面板。
#[cfg(target_os = "macos")]
pub(crate) fn show_screenshot_overlay(app: &tauri::AppHandle) -> bool {
    let monitors = match app.available_monitors() {
        Ok(monitors) if !monitors.is_empty() => monitors,
        _ => {
            eprintln!("[screenshot] 无可用显示器信息，框选层无法就位");
            return false;
        }
    };
    let cursor = app.cursor_position().ok();
    let mut shown = 0usize;
    for (index, monitor) in monitors.iter().enumerate() {
        let win = match ensure_screenshot_overlay_panel(app, index) {
            Ok(win) => win,
            Err(e) => {
                eprintln!("[screenshot] 创建框选层面板 {index} 失败：{e}");
                continue;
            }
        };
        let scale = monitor.scale_factor();
        let _ = win.set_position(monitor.position().to_logical::<f64>(scale));
        let _ = win.set_size(monitor.size().to_logical::<f64>(scale));
        let has_cursor = cursor
            .as_ref()
            .map_or(index == 0, |c| monitor_contains_cursor(monitor, c));
        let _ = win.eval(format!(
            "window.__lunaScreenshotReset && window.__lunaScreenshotReset({has_cursor})"
        ));
        if let Err(e) = win.show() {
            eprintln!("[screenshot] 框选层面板 {index} show 失败：{e}");
            continue;
        }
        shown += 1;
    }
    // 显示器减少后池中多余面板本就隐藏，无需处理；增多由 ensure 补建。
    shown > 0
}

/// 隐藏全部截屏框选层面板（纯窗口动作；确认/取消编排在 `screenshot` 模块）。
#[cfg(target_os = "macos")]
pub(crate) fn hide_screenshot_overlay(app: &tauri::AppHandle) {
    use tauri::Manager as _;
    for (label, win) in app.webview_windows() {
        if is_screenshot_overlay_label(&label) {
            let _ = win.hide();
        }
    }
}

/// 任一截屏框选层面板当前是否可见（Esc 分发用）。
#[cfg(target_os = "macos")]
pub(crate) fn is_screenshot_overlay_visible(app: &tauri::AppHandle) -> bool {
    use tauri::Manager as _;
    app.webview_windows()
        .iter()
        .any(|(label, win)| is_screenshot_overlay_label(label) && win.is_visible().unwrap_or(false))
}

/// 预热 `quick-chat` 窗口并转为可输入的非激活 NSPanel（ADR-015）。
#[cfg(target_os = "macos")]
fn setup_quick_chat_panel(app: &mut tauri::App) -> tauri::Result<()> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};

    // 固定初始尺寸 + 用户可拖拽调整（对话内容随会话增长，自适应尺寸会导致窗口跳动——
    // 与实时字幕窗同一教训）；最小尺寸保证头部 + 至少一屏消息 + Composer 完整可用。
    let chat = WebviewWindowBuilder::new(app, "quick-chat", WebviewUrl::App("index.html".into()))
        .title(hud_window_title(
            &*app,
            "window-title-quick-chat",
            "Lunarca 快捷对话",
        ))
        .inner_size(
            QUICK_CHAT_HUD_GEOMETRY.initial_width,
            QUICK_CHAT_HUD_GEOMETRY.initial_height,
        )
        .min_inner_size(
            QUICK_CHAT_HUD_GEOMETRY.min_width,
            QUICK_CHAT_HUD_GEOMETRY.min_height,
        )
        .resizable(true)
        .hud_chrome()
        .visible(false)
        .build()?;

    match chat.ns_window() {
        Ok(ns_window) => unsafe {
            platform::macos::convert_to_nonactivating_panel(ns_window);
            platform::macos::enable_input_mode_focus(ns_window);
        },
        Err(e) => eprintln!("[C9] 取 quick-chat ns_window 失败：{e}（无法转面板）"),
    }
    Ok(())
}

/// 预热 `text-quick-note` 窗口并转为**可输入的**非激活 NSPanel（ADR-015 / PRD-06-058 B″）。
///
/// 与语音 HUD 的关键区别是必须调 `enable_input_mode_focus`：这扇窗的全部内容就是一个
/// 文本框，拿不到键焦点等于一个字也打不进去。语音 HUD 那条「刻意不调」说的是相反的场景
/// （它要把注入目标留在源 App 的光标上），别照着抄。
///
/// 另一处区别是不注册失焦收起（与快捷对话、录音 HUD 同）。理由不同于录音（那是会话
/// 常驻）——这里护的是用户已经敲下、尚未存下的那段字：点一下别的应用就把它连窗带字
/// 收走，是这扇窗唯一不可接受的失败。
#[cfg(target_os = "macos")]
fn setup_text_quick_note_panel(app: &mut tauri::App) -> tauri::Result<()> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};

    let panel = WebviewWindowBuilder::new(
        app,
        TEXT_QUICK_NOTE_WINDOW,
        WebviewUrl::App("index.html".into()),
    )
    .title(hud_window_title(
        &*app,
        "window-title-text-quick-note",
        "Lunarca 速记",
    ))
    .inner_size(
        TEXT_QUICK_NOTE_HUD_GEOMETRY.initial_width,
        TEXT_QUICK_NOTE_HUD_GEOMETRY.initial_height,
    )
    .min_inner_size(
        TEXT_QUICK_NOTE_HUD_GEOMETRY.min_width,
        TEXT_QUICK_NOTE_HUD_GEOMETRY.min_height,
    )
    .resizable(true)
    .hud_chrome()
    .visible(false)
    .build()?;

    match panel.ns_window() {
        Ok(ns_window) => unsafe {
            platform::macos::convert_to_nonactivating_panel(ns_window);
            platform::macos::enable_input_mode_focus(ns_window);
        },
        Err(e) => eprintln!("[quick-note] 取 text-quick-note ns_window 失败：{e}（无法转面板）"),
    }
    Ok(())
}

/// 显示文字速记面板并把输入焦点交给前端的捕获框（PRD-06-058 B″）。
///
/// `origin` 是入口面（`shortcut` / `tray`），随焦点桥一次带过去——指标要分得清这两条
/// 路径各自的到达率，混记成同一个数就等于没测（PRD-07-070）。不另起事件通道：一次
/// `eval` 已经在做「叫醒它」这件事，第二条通道只会多一处可能失步的状态。
pub(crate) fn show_text_quick_note_panel(app: &tauri::AppHandle, origin: &str) {
    use tauri::Manager as _;
    let Some(win) = app.get_webview_window(TEXT_QUICK_NOTE_WINDOW) else {
        eprintln!("[quick-note] text-quick-note 窗口不存在，无法响应入口。");
        return;
    };
    // 仅在隐藏→显示时定位：可见期间连按热键不跳位（与快捷对话同口径）。
    if !win.is_visible().unwrap_or(false) {
        position_panel_near_cursor(&win);
    }
    if let Err(e) = win.show() {
        eprintln!("[quick-note] 面板 show 失败：{e}");
        return;
    }
    // origin 只有两个字面量、且都由本文件给出，故直接内插；它不是用户输入。
    if let Err(e) = win.eval(format!(
        "window.__lunaShowQuickNote && window.__lunaShowQuickNote('{origin}')"
    )) {
        eprintln!("[quick-note] 焦点桥调用失败：{e}");
    }
}

/// 桌面便签的默认尺寸（逻辑像素）。一张便签是「瞥一眼、补一句」的东西，不是编辑器。
const DESKTOP_NOTE_SIZE: (f64, f64) = (260.0, 220.0);

/// 建一扇桌面便签窗（ADR-119 ③）。
///
/// 非激活 NSPanel + 输入焦点模式：它有文本框（与快捷对话同类），但不该把前台应用的焦点
/// 抢走——用户在别的应用里工作时，便签只是贴在那儿。
///
/// 恢复坐标前必须过 `clamp_saved_position`：显示器拔掉、分辨率改了、Vault 从另一台机器
/// 同步过来，坐标都会落到屏幕外，而一扇看不见的窗等于一张丢了的便签。
pub(crate) fn open_desktop_note_panel(
    app: &tauri::AppHandle,
    label: &str,
    desktop: &luna_vault::DesktopNote,
) -> tauri::Result<()> {
    use tauri::{Manager as _, WebviewUrl, WebviewWindowBuilder};

    let (w, h) = (
        desktop.w.map_or(DESKTOP_NOTE_SIZE.0, f64::from),
        desktop.h.map_or(DESKTOP_NOTE_SIZE.1, f64::from),
    );
    let window = WebviewWindowBuilder::new(app, label, WebviewUrl::App("index.html".into()))
        .title(hud_window_title(
            app,
            "window-title-desktop-note",
            "Lunarca 便签",
        ))
        .inner_size(w, h)
        .min_inner_size(180.0, 120.0)
        .resizable(true)
        .hud_chrome()
        .visible(false)
        .build()?;

    #[cfg(target_os = "macos")]
    match window.ns_window() {
        Ok(ns_window) => unsafe {
            platform::macos::convert_to_nonactivating_panel(ns_window);
            platform::macos::enable_input_mode_focus(ns_window);
        },
        Err(e) => eprintln!("[sticky] 取 {label} ns_window 失败：{e}（无法转面板）"),
    }

    place_desktop_note_window(&window, desktop);
    let _ = window.show();

    let handle = app.clone();
    let owned = label.to_owned();
    window.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed) {
            if let Some(state) = handle.try_state::<ipc::AppState>() {
                state.forget_desktop_note_window(&owned);
            }
        }
    });
    Ok(())
}

/// 把一扇便签窗摆到它该在的位置与大小（新建与复用共用这一份）。
///
/// 坐标显示前必须 clamp 进屏：显示器拔掉、分辨率改了、Vault 从另一台机器同步过来，
/// 坐标都会落到屏幕外，而一扇看不见的窗等于一张丢了的便签。
pub(crate) fn place_desktop_note_window(
    window: &tauri::WebviewWindow,
    desktop: &luna_vault::DesktopNote,
) {
    let (w, h) = (
        desktop.w.map_or(DESKTOP_NOTE_SIZE.0, f64::from),
        desktop.h.map_or(DESKTOP_NOTE_SIZE.1, f64::from),
    );
    let _ = window.set_size(tauri::Size::Logical(tauri::LogicalSize::new(w, h)));
    match (desktop.x, desktop.y) {
        (Some(x), Some(y)) => {
            let scale = window.scale_factor().unwrap_or(1.0);
            let clamped = popup::clamp_saved_position(
                popup::ScreenPoint::new(f64::from(x), f64::from(y)),
                popup::ScreenSize::new(w * scale, h * scale),
                &collect_monitors(window),
            );
            let _ = window.set_position(tauri::PhysicalPosition::new(
                clamped.x as i32,
                clamped.y as i32,
            ));
        }
        // 还没摆过的新便签落在光标附近——用户刚点的「钉到桌面」就在那儿。
        _ => position_panel_near_cursor(window),
    }
}

/// 启动时恢复全部钉住的便签（ADR-119 ③）。
///
/// 读不出来或建窗失败的那一张跳过而不是整体放弃：一篇坏笔记不该让所有便签都恢复不了。
pub(crate) fn restore_desktop_notes(app: &tauri::AppHandle) {
    use tauri::Manager as _;
    let Some(state) = app.try_state::<ipc::AppState>() else {
        return;
    };
    let pinned = match state.with_vault("main", |vault| {
        vault.desktop_notes().map_err(|e| e.to_string())
    }) {
        Ok(list) => list,
        // 库还没打开（首启引导中）就没有便签可恢复，这不是错误。
        Err(_) => return,
    };
    for (path, desktop) in pinned {
        if let Err(e) = ipc::open_desktop_note_window(app, &state, &path, &desktop) {
            eprintln!("[sticky] 恢复便签失败（跳过这一张）：{e}");
        }
    }
}

/// 预热 `recording-hud` 窗口并转为常驻非激活面板（ADR-015 / PRD-04；Windows 见 ADR-163 ①，
/// [`HudWindowChrome::non_activating`]）。
///
/// 与弹窗/语音 HUD 的关键区别：**不注册 on_window_event 失焦收起回调**——录音 HUD
/// 是持久面板，用户手动停止录音后才隐藏（`recording_stop` 命令收尾时隐藏）。
fn setup_recording_hud_panel(app: &mut tauri::App) -> tauri::Result<()> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};

    // 首帧为舒适基线；空闲/录音态共用稳定内容画布，用户尺寸由窗口拥有。
    let hud = WebviewWindowBuilder::new(app, "recording-hud", WebviewUrl::App("index.html".into()))
        .title(hud_window_title(
            &*app,
            "window-title-recording",
            "Lunarca 录音",
        ))
        .inner_size(
            RECORDING_HUD_GEOMETRY.initial_width,
            RECORDING_HUD_GEOMETRY.initial_height,
        )
        .min_inner_size(
            RECORDING_HUD_GEOMETRY.min_width,
            RECORDING_HUD_GEOMETRY.min_height,
        )
        .resizable(true)
        .hud_chrome()
        .non_activating()
        .visible(false)
        .build()?;

    #[cfg(target_os = "windows")]
    install_windows_panel(&hud);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let _ = hud;
    #[cfg(target_os = "macos")]
    match hud.ns_window() {
        // 录音 HUD 无需输入焦点（仅显示状态/控制按钮），严格非激活面板。
        Ok(ns_window) => unsafe {
            platform::macos::convert_to_nonactivating_panel(ns_window);
        },
        Err(e) => eprintln!("[F4] 取 recording-hud ns_window 失败：{e}（无法转面板）"),
    }
    // 不注册 on_window_event 失焦收起：录音 HUD 是常驻面板（区别于弹窗/语音 HUD）。
    Ok(())
}

/// 提词器面板（ADR-162 ⑤ C4a）：预热即转换、常驻、只隐藏不关闭。严格非激活面板（录屏 / 录像
/// 软件保持在前台，按钮第一下就生效；Windows 见 ADR-163 ①）。
fn setup_teleprompter_panel(app: &mut tauri::App) -> tauri::Result<()> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};

    let panel = WebviewWindowBuilder::new(
        app,
        ipc::TELEPROMPTER_LABEL,
        WebviewUrl::App("index.html".into()),
    )
    .title(hud_window_title(
        &*app,
        "window-title-teleprompter",
        "Lunarca 提词器",
    ))
    .inner_size(
        TELEPROMPTER_GEOMETRY.initial_width,
        TELEPROMPTER_GEOMETRY.initial_height,
    )
    .min_inner_size(
        TELEPROMPTER_GEOMETRY.min_width,
        TELEPROMPTER_GEOMETRY.min_height,
    )
    .resizable(true)
    .hud_chrome()
    .non_activating()
    .visible(false)
    .build()?;

    #[cfg(target_os = "windows")]
    install_windows_panel(&panel);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let _ = panel;
    #[cfg(target_os = "macos")]
    match panel.ns_window() {
        Ok(ns_window) => unsafe {
            platform::macos::convert_to_nonactivating_panel(ns_window);
        },
        Err(e) => eprintln!("[teleprompter] 取 ns_window 失败：{e}（无法转面板）"),
    }
    Ok(())
}

/// 显示提词器：没显示时先回到记下的位置（没记过就放在屏幕上方居中），再让面板重读脚本。
pub(crate) fn show_teleprompter(app: &tauri::AppHandle) -> bool {
    use tauri::Manager as _;
    let Some(win) = app.get_webview_window(ipc::TELEPROMPTER_LABEL) else {
        return false;
    };
    if !win.is_visible().unwrap_or(false) {
        let remembered = app
            .try_state::<ipc::AppState>()
            .and_then(|state| {
                ipc::remembered_caption_window_geometry(&state.settings_snapshot(), win.label())
            })
            .map(|saved| {
                popup::clamp_saved_rect(
                    popup::ScreenRect::new(
                        f64::from(saved.x),
                        f64::from(saved.y),
                        f64::from(saved.width),
                        f64::from(saved.height),
                    ),
                    &collect_monitors(&win),
                )
            });
        match remembered {
            Some(rect) => apply_caption_rect(&win, rect),
            None => position_teleprompter_window(&win),
        }
    }
    let shown = win.show().is_ok();
    // 常驻面板只挂载一次：换脚本时请它重读（与桌面便签的 `__lunaReload*` 同一做法）。
    let _ = win.eval("window.__lunaReloadTeleprompter?.()");
    shown
}

/// 提词器没记过位置时的落位：镜头多在屏幕上沿，所以水平居中、离屏顶约十分之一屏高。
/// 不借字幕窗那种「从屏底往上算」的落位——矮屏上会算出负的 y。
fn position_teleprompter_window(win: &tauri::WebviewWindow) {
    let Ok(Some(monitor)) = win.current_monitor().or_else(|_| win.primary_monitor()) else {
        return;
    };
    let Ok(size) = win.outer_size() else {
        return;
    };
    let (m_pos, m_size) = (monitor.position(), monitor.size());
    let width = i32::try_from(m_size.width).unwrap_or(i32::MAX);
    let height = i32::try_from(m_size.height).unwrap_or(i32::MAX);
    let own_width = i32::try_from(size.width).unwrap_or(0);
    let x = m_pos.x + (width - own_width).max(0) / 2;
    let y = m_pos.y + height / 10;
    let _ = win.set_position(tauri::PhysicalPosition::new(x, y));
}

/// 收起提词器：先记下位置与尺寸，再走 Rust 侧 hide（面板绝不 close，见 `desktop_note.rs`）。
pub(crate) fn hide_teleprompter(app: &tauri::AppHandle) {
    use tauri::Manager as _;
    if let Some(win) = app.get_webview_window(ipc::TELEPROMPTER_LABEL) {
        remember_caption_window(app, &win);
        let _ = win.hide();
    }
}

/// 切换录音 HUD 显示/隐藏（Cmd+Shift+R 热键响应）。
///
/// 若 HUD 当前可见则隐藏（不停止录音，仅收起面板）；否则显示并居中定位。
pub(crate) fn toggle_recording_hud(app: &tauri::AppHandle) {
    run_recording_hud_hotkey(app, "toggle");
}

/// 把一条全局热键命令交给录音 HUD 执行（`toggle` / `bookmark`）。
///
/// HUD 是常驻的非激活面板：热键先确保它可见、重播一次权威快照，再 eval 前端钩子
/// `__lunaRecordingHotkey`。开始/停止、打点及其反馈都由 HUD 那一份实现承担，Rust 不另写一遍。
/// 录音中绝不用热键隐藏 HUD（TDD-05 §5.3），停止归档后由前端自隐藏。
fn run_recording_hud_hotkey(app: &tauri::AppHandle, command: &str) {
    use tauri::Manager as _;
    let Some(win) = app.get_webview_window("recording-hud") else {
        eprintln!("[F4] recording-hud 窗口不存在，无法响应热键。");
        return;
    };
    if !win.is_visible().unwrap_or(false) {
        position_panel_near_cursor(&win);
        if let Err(e) = win.show() {
            eprintln!("[F4] recording-hud show 失败：{e}");
        }
    }
    ipc::recording::broadcast_recording_state(app);
    let script =
        format!("window.__lunaRecordingHotkey && window.__lunaRecordingHotkey('{command}')");
    if let Err(e) = win.eval(&script) {
        eprintln!("[F4] recording-hud {command} 桥调用失败：{e}");
    }
}

/// 显示录音 HUD 常驻面板（纯窗口动作，不切换录音状态）。
///
/// 应用内「开始录音」入口用：与热键路径共用同一非激活 NSPanel（ADR-015），
/// 只 show 不 eval toggle——开始/停止由用户在 HUD 内决定。
pub(crate) fn show_recording_hud(app: &tauri::AppHandle) -> bool {
    use tauri::Manager as _;
    if let Some(win) = app.get_webview_window("recording-hud") {
        if !win.is_visible().unwrap_or(false) {
            position_panel_near_cursor(&win);
        }
        let shown = win.show().is_ok();
        // 显示即重播权威快照（H2，同 `toggle_recording_hud`）。
        ipc::recording::broadcast_recording_state(app);
        shown
    } else {
        false
    }
}

/// 隐藏录音 HUD 面板（纯窗口动作）。
///
/// **必须走 Rust 侧 `win.hide()`**：录音 HUD 是非激活 NSPanel，前端 JS `window.hide()`
/// 受 Tauri v2 capability 约束（`core:window:default` 不含 `allow-hide`），且 recording-hud
/// 此前未列入 capabilities → JS 隐藏被 ACL 拒绝。与 popup/voice/quick-chat 三个 HUD 一致，
/// 一律从 Rust 收起（停录归档自隐藏 + 关闭按钮 dismiss 共用此单一路径）。
pub(crate) fn hide_recording_hud(app: &tauri::AppHandle) {
    use tauri::Manager as _;
    if let Some(win) = app.get_webview_window("recording-hud") {
        let _ = win.hide();
    }
}

// ─── 实时字幕面板（live-caption，PRD-04-040）──────────────────────────────────

/// 预热实时字幕非激活面板（与录音 HUD 同为常驻 NSPanel，ADR-015）。
///
/// 字幕面板更宽（影片字幕/歌词式呈现），首帧完整容纳框体模式；正文在稳定窗口内滚动。
/// 悬浮窗（HUD）与桌面便签的窗口外观（ADR-137 V1）：无边框、不进任务栏、**透明窗 + 系统阴影**。
///
/// 此前这几扇窗都是不透明的直角矩形——看上去就是一扇普通窗口，而它们是浮在别的应用
/// 之上的浮层。透明之后圆角与细边由 CSS 的 `.luna-hud-surface` 画，阴影交给系统（按内容的
/// alpha 投影，圆角外不带阴影）。字幕类窗口自己声明透明（沉浸态要整窗无底），不走这里。
trait HudWindowChrome {
    fn hud_chrome(self) -> Self;
    /// 非激活浮层的窗口标志。macOS 上为恒等，由调用方在建窗后转 NSPanel（ADR-015）。
    /// Windows（ADR-163 ①）：`focusable(false)` 给 `WS_EX_NOACTIVATE`、`focused(false)` 让每次
    /// `show()` 都走 `SW_SHOWNOACTIVATE`、`always_on_top(true)` 给 `WS_EX_TOPMOST`——三者都必须
    /// 走 tao 的窗口标志（tao 每次改标志都会重写扩展样式）；`WS_EX_TOOLWINDOW` 由建窗后的
    /// `install_windows_panel` 守住。
    fn non_activating(self) -> Self;
}

impl<'a, R: tauri::Runtime, M: tauri::Manager<R>> HudWindowChrome
    for tauri::WebviewWindowBuilder<'a, R, M>
{
    fn hud_chrome(self) -> Self {
        self.decorations(false)
            .skip_taskbar(true)
            .transparent(true)
            .shadow(true)
            // 第一下点击即生效（ADR-137 H4/H5，真机 2026-10-01）：面板不是 key 时，WKWebView 默认
            // 把落在它上面的第一下只用来获得焦点、点击本身被吞（语音「⋯」要点两下、翻译窗图钉
            // 取消不了）。走 wry 自带的 `acceptsFirstMouse` 开关——此前在 objc 运行时给 webview
            // 实例换动态子类，真机在鼠标进出时触发 AppKit 断言崩溃（2026-10-02）。
            .accept_first_mouse(true)
    }

    fn non_activating(self) -> Self {
        #[cfg(target_os = "macos")]
        {
            self
        }
        #[cfg(not(target_os = "macos"))]
        {
            self.always_on_top(true).focused(false).focusable(false)
        }
    }
}

/// Windows：给建好的浮层挂样式守卫（`WS_EX_TOOLWINDOW`、点击不激活，ADR-163 ①）。
/// 须在主线程（窗口所属线程）上调用——setup 阶段即是。
#[cfg(target_os = "windows")]
fn install_windows_panel(win: &tauri::WebviewWindow) {
    match win.hwnd() {
        Ok(hwnd) => {
            if !platform::windows::panel::install(hwnd.0 as isize) {
                eprintln!(
                    "[ADR-163] {} 挂样式守卫失败（仍不可激活，但会出现在 Alt+Tab）",
                    win.label()
                );
            }
        }
        Err(e) => eprintln!("[ADR-163] 取 {} 的窗口句柄失败：{e}", win.label()),
    }
}

/// 构造一个字幕类非激活透明面板（原文窗 / 译文窗共用，DRY）。
#[cfg(target_os = "macos")]
fn build_caption_panel(app: &mut tauri::App, label: &str, title: &str) -> tauri::Result<()> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};

    let panel = WebviewWindowBuilder::new(app, label, WebviewUrl::App("index.html".into()))
        .title(title)
        .inner_size(
            LIVE_CAPTION_HUD_GEOMETRY.initial_width,
            LIVE_CAPTION_HUD_GEOMETRY.initial_height,
        )
        // 用户可拖拽调整大小（宽/高皆可，PRD-04-040）+ 最小尺寸兜底；**不自动随内容改尺寸**
        // （字幕框稳定、内容在框内滚动贴底，对标主流播放器字幕），避免窗口自跳。
        // 最小尺寸须容纳框体模式的顶栏 + 控制条 + 至少一行字幕，避免调至最小时控件被挤压/遮挡。
        .resizable(true)
        .min_inner_size(
            LIVE_CAPTION_HUD_GEOMETRY.min_width,
            LIVE_CAPTION_HUD_GEOMETRY.min_height,
        )
        .decorations(false)
        .skip_taskbar(true)
        // 透明窗口：背景浓淡由 CSS 按 `caption.frame_opacity` 画（0 = 沉浸，文字悬浮于任意
        // 应用之上；越大底越实），圆角外区域透明（ADR-137 H6）。
        .transparent(true)
        // 字幕工具药丸第一下点击即生效（同 `hud_chrome()` 的理由）。
        .accept_first_mouse(true)
        .visible(false)
        .build()?;

    #[cfg(not(target_os = "macos"))]
    let _ = (panel, label);
    #[cfg(target_os = "macos")]
    match panel.ns_window() {
        // 字幕面板不夺焦（覆盖在其它应用之上做字幕），严格非激活面板。
        Ok(ns_window) => unsafe {
            platform::macos::convert_to_nonactivating_panel(ns_window);
        },
        Err(e) => eprintln!("[caption] 取 {label} ns_window 失败：{e}（无法转面板）"),
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn setup_live_caption_panel(app: &mut tauri::App) -> tauri::Result<()> {
    build_caption_panel(app, "live-caption", "Lunarca 实时字幕")?;
    // 译文独立窗（PRD-04-044）：与原文窗同为常驻透明面板，默认隐藏，按设置/切换开合。
    build_caption_panel(app, "live-caption-translation", "Lunarca 译文字幕")
}

/// 运行时创建弹出编辑器独立窗口（PRD-07-036；Obsidian「移动到新窗口」对标）。
///
/// 与字幕/HUD 面板的关键区别：这是**常规可激活窗口**（带原生标题栏、可缩放、进任务栏），
/// 编辑器需夺焦并接收键盘输入，故**不转 NSPanel**。窗口销毁时清理 `AppState` 的
/// label→path 映射；关闭回主窗打开的编排在前端（经 `open_workspace_route`）。
pub(crate) fn open_note_editor_window(
    app: &tauri::AppHandle,
    label: &str,
    title: &str,
) -> tauri::Result<()> {
    use tauri::{Manager as _, WebviewUrl, WebviewWindowBuilder};

    let window = WebviewWindowBuilder::new(app, label, WebviewUrl::App("index.html".into()))
        .title(title)
        .inner_size(920.0, 720.0)
        .min_inner_size(480.0, 360.0)
        .resizable(true)
        .build()?;

    let handle = app.clone();
    let owned_label = label.to_owned();
    window.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed) {
            if let Some(state) = handle.try_state::<ipc::AppState>() {
                state.forget_note_window(&owned_label);
            }
        }
    });
    let _ = window.set_focus();
    Ok(())
}

/// 打开副库独立窗口（PRD-06-050）：受限浏览/编辑面，尺寸略大于单笔记窗（带文件树列）。
/// 销毁时摘除其 Vault 实例（`Arc` 最后一份释放时索引连接随之关闭）。
pub(crate) fn open_secondary_vault_window(
    app: &tauri::AppHandle,
    label: &str,
    title: &str,
) -> tauri::Result<()> {
    use tauri::{Manager as _, WebviewUrl, WebviewWindowBuilder};

    let window = WebviewWindowBuilder::new(app, label, WebviewUrl::App("index.html".into()))
        .title(title)
        .inner_size(1100.0, 760.0)
        .min_inner_size(640.0, 420.0)
        .resizable(true)
        .build()?;

    let handle = app.clone();
    let owned_label = label.to_owned();
    window.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed) {
            if let Some(state) = handle.try_state::<ipc::AppState>() {
                state.forget_vault_window(&owned_label);
            }
        }
    });
    let _ = window.set_focus();
    Ok(())
}

/// 切换实时字幕面板显示（全局热键响应）：不可见则按 [`place_caption_window`] 落位后显示；
/// 已可见则仅确保存在（不 hide，字幕面板常驻，停止由用户在面板内决定，同录音 HUD 语义）。
pub(crate) fn toggle_live_caption(app: &tauri::AppHandle) {
    use tauri::Manager as _;
    if let Some(win) = app.get_webview_window("live-caption") {
        if !win.is_visible().unwrap_or(false) {
            place_caption_window(app, &win);
            if let Err(e) = win.show() {
                eprintln!("[caption] live-caption show 失败：{e}");
            }
        }
        if let Err(e) =
            win.eval("window.__lunaCaptionHotkey && window.__lunaCaptionHotkey('toggle')")
        {
            eprintln!("[caption] live-caption toggle 桥调用失败：{e}");
        }
    } else {
        eprintln!("[caption] live-caption 窗口不存在，无法响应热键。");
    }
}

/// 显示某字幕窗（纯窗口动作，不开始采音）；不可见时先落位。
fn show_caption_window(app: &tauri::AppHandle, label: &str) -> bool {
    use tauri::Manager as _;
    if let Some(win) = app.get_webview_window(label) {
        if !win.is_visible().unwrap_or(false) {
            place_caption_window(app, &win);
        }
        win.show().is_ok()
    } else {
        false
    }
}

/// 隐藏某字幕窗：先记下它此刻的位置与尺寸（下次打开回到这里），再走 Rust 侧 hide
/// （同录音 HUD 收起理由）。收起后广播 [`ipc::CaptionWindowHidden`]：那扇窗据此收起自己的
/// 外观面板，下次打开从干净的字幕面开始。本来就隐藏着的窗不广播（没有「被收起」这件事）。
fn hide_caption_window(app: &tauri::AppHandle, label: &str) {
    use tauri::Manager as _;
    use tauri_specta::Event as _;
    if let Some(win) = app.get_webview_window(label) {
        let was_visible = win.is_visible().unwrap_or(false);
        remember_caption_window(app, &win);
        let _ = win.hide();
        if was_visible {
            let _ = ipc::CaptionWindowHidden {
                label: label.to_owned(),
            }
            .emit(app);
        }
    }
}

/// 记下一扇**正显示着**的字幕窗的位置与尺寸。隐藏着的窗不记：它的几何是上一次收起时
/// 的，那时已经记过了。
fn remember_caption_window(app: &tauri::AppHandle, win: &tauri::WebviewWindow) {
    use tauri::Manager as _;
    if !win.is_visible().unwrap_or(false) {
        return;
    }
    let (Ok(pos), Ok(size)) = (win.outer_position(), win.inner_size()) else {
        return;
    };
    let Some(state) = app.try_state::<ipc::AppState>() else {
        return;
    };
    let geometry = ipc::CaptionWindowGeometry {
        x: pos.x,
        y: pos.y,
        width: size.width,
        height: size.height,
    };
    if let Err(e) = ipc::remember_caption_window_geometry(&state, win.label(), geometry) {
        eprintln!("[caption] 记下 {} 的位置失败：{e}", win.label());
    }
}

/// 应用退出时记下仍开着的字幕窗：退出不经过收起，不在这里记，用户摆好的位置就丢了。
fn remember_open_caption_windows(app: &tauri::AppHandle) {
    use tauri::Manager as _;
    for label in ["live-caption", "live-caption-translation"] {
        if let Some(win) = app.get_webview_window(label) {
            remember_caption_window(app, &win);
        }
    }
}

/// 字幕窗落位（PRD-04-040 / 044）：
/// - 记过 → 回到记下的位置与尺寸，夹回当前屏幕可用区（拔了显示器、换了分辨率也整窗可见）；
/// - 没记过 → 原文窗在屏幕底部居中；译文窗贴在开着的原文窗正上方（放不下压矮、再不行放
///   下方），原文窗没开或上下都放不下时退回屏幕中上区。
///
/// 译文窗记下的位置若会压住此刻开着的原文窗，不用它、重新贴：两扇窗重叠正是 2026-10-02
/// 真机修掉的那件事（此前按屏高比例放，译文窗压住原文窗大半、连外观面板一起盖住）。
fn place_caption_window(app: &tauri::AppHandle, win: &tauri::WebviewWindow) {
    use tauri::Manager as _;
    let monitors = collect_monitors(win);
    let remembered = app
        .try_state::<ipc::AppState>()
        .and_then(|state| {
            ipc::remembered_caption_window_geometry(&state.settings_snapshot(), win.label())
        })
        .map(|saved| {
            popup::clamp_saved_rect(
                popup::ScreenRect::new(
                    f64::from(saved.x),
                    f64::from(saved.y),
                    f64::from(saved.width),
                    f64::from(saved.height),
                ),
                &monitors,
            )
        });
    if win.label() == "live-caption" {
        match remembered {
            Some(rect) => apply_caption_rect(win, rect),
            None => position_caption_window(win, 8),
        }
        return;
    }
    let anchor = app
        .get_webview_window("live-caption")
        .filter(|source| source.is_visible().unwrap_or(false))
        .and_then(|source| window_rect(&source));
    if let Some(rect) = remembered.filter(|rect| anchor.is_none_or(|a| !rect.intersects(a))) {
        apply_caption_rect(win, rect);
        return;
    }
    if let (Some(anchor), Ok(size)) = (anchor, win.inner_size()) {
        let scale = win.scale_factor().unwrap_or(1.0);
        let size = popup::ScreenSize::new(f64::from(size.width), f64::from(size.height));
        let slot = popup::stack_beside(
            anchor,
            size,
            &monitors,
            CAPTION_STACK_GAP_POINTS * scale,
            LIVE_CAPTION_HUD_GEOMETRY.min_height * scale,
        );
        if let Some(slot) = slot {
            apply_caption_rect(
                win,
                popup::ScreenRect::new(slot.x, slot.y, size.width, slot.height),
            );
            return;
        }
    }
    position_caption_window(win, 3);
}

/// 两扇字幕窗上下相贴时留的缝（逻辑点）：贴死了两圈虚线边界会连成一条，分不清是两扇窗。
const CAPTION_STACK_GAP_POINTS: f64 = 8.0;

/// 窗口此刻在屏幕上占的矩形（物理像素）。
fn window_rect(win: &tauri::WebviewWindow) -> Option<popup::ScreenRect> {
    let (Ok(pos), Ok(size)) = (win.outer_position(), win.outer_size()) else {
        return None;
    };
    Some(popup::ScreenRect::new(
        f64::from(pos.x),
        f64::from(pos.y),
        f64::from(size.width),
        f64::from(size.height),
    ))
}

/// 把字幕窗放到 `rect`（物理像素）。
fn apply_caption_rect(win: &tauri::WebviewWindow, rect: popup::ScreenRect) {
    let _ = win.set_size(tauri::PhysicalSize::new(
        rect.width.round() as u32,
        rect.height.round() as u32,
    ));
    let _ = win.set_position(tauri::PhysicalPosition::new(
        rect.x.round() as i32,
        rect.y.round() as i32,
    ));
}

/// 显示实时字幕（原文）常驻面板。
pub(crate) fn show_live_caption(app: &tauri::AppHandle) -> bool {
    show_caption_window(app, "live-caption")
}

/// 隐藏实时字幕（原文）面板。
pub(crate) fn hide_live_caption(app: &tauri::AppHandle) {
    hide_caption_window(app, "live-caption");
}

/// 显示译文独立窗（PRD-04-044；落位见 [`place_caption_window`]）。
pub(crate) fn show_live_caption_translation(app: &tauri::AppHandle) -> bool {
    show_caption_window(app, "live-caption-translation")
}

/// 隐藏译文独立窗。
pub(crate) fn hide_live_caption_translation(app: &tauri::AppHandle) {
    hide_caption_window(app, "live-caption-translation");
}

/// 录音会话的译文独立窗收尾（PRD-04-044 录音扩展）：`recording.separate_translation`
/// 开启时镜像窗服务于本次录音会话，随录音 HUD 收起一并隐藏（避免孤窗）；
/// 关闭时不动镜像窗——它可能正被字幕面板的独立窗偏好占位使用。
pub(crate) fn hide_recording_translation_mirror(app: &tauri::AppHandle) {
    use tauri::Manager as _;
    let serving_recording = app
        .try_state::<crate::ipc::AppState>()
        .and_then(|state| {
            state
                .settings_snapshot()
                .get_bool("recording.separate_translation")
        })
        .unwrap_or(false);
    if serving_recording {
        hide_live_caption_translation(app);
    }
}

/// 定位字幕窗到当前屏幕**底部居中**（影片字幕惯例）；`bottom_divisor` 越大越贴底。失败保持原位。
fn position_caption_window(win: &tauri::WebviewWindow, bottom_divisor: i32) {
    let Ok(Some(monitor)) = win.current_monitor().or_else(|_| win.primary_monitor()) else {
        return;
    };
    let Ok(size) = win.outer_size() else {
        return;
    };
    let m_pos = monitor.position();
    let m_size = monitor.size();
    let x = m_pos.x + ((m_size.width as i32 - size.width as i32) / 2);
    let y = m_pos.y + m_size.height as i32
        - size.height as i32
        - (m_size.height as i32 / bottom_divisor.max(1));
    let _ = win.set_position(tauri::PhysicalPosition::new(x, y));
}

/// 启动**运行期权限撤销检测**（TDD-05 §4，仅 macOS）。
///
/// 低频轮询辅助功能 trust（`AXIsProcessTrusted` 廉价、不弹框）；已授予→被撤销时经
/// [`permissions::detect_revocation`] 判定，并把脱敏的 `permission_revoked` 事件直接投递
/// `NotificationCenter` 路由 banner + 系统横幅。把"用户莫名发现双击失灵"提前为"主动告知 +
/// 引导备用热键"（PRD-15-020）。
///
/// ⚠ 实际撤销→降级的端到端行为只能真机验（系统设置里关掉授权后观察日志/降级）。
#[cfg(target_os = "macos")]
fn start_permission_watch(app: tauri::AppHandle) {
    use platform::macos::MacPermissionManager;
    use platform::permissions::{detect_revocation, PermissionKind, PermissionManager as _};

    tauri::async_runtime::spawn(async move {
        let manager = MacPermissionManager;
        let mut previous = manager.check(PermissionKind::Accessibility);
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            let current = manager.check(PermissionKind::Accessibility);
            if current != previous {
                // 授予 → 此刻建 tap（用户不必重启）；撤销 → 停掉已失效的 tap 并标记原因。
                ensure_key_tap(&app);
            }
            if let Some(event) = detect_revocation(previous, current, PermissionKind::Accessibility)
            {
                use tauri::Manager as _;
                if let Some(center) = app.try_state::<notifications::NotificationCenter>() {
                    center.publish(
                        notifications::NotificationEvent::permission_revoked(match event.kind {
                            PermissionKind::Accessibility => "accessibility",
                            PermissionKind::InputMonitoring => "input_monitoring",
                            PermissionKind::Microphone => "microphone",
                            PermissionKind::Notification => "notification",
                            PermissionKind::ScreenRecording => "screen_recording",
                        }),
                        &app,
                    );
                }
            }
            previous = current;
        }
    });
}

/// 双击手势按键观测的生命周期（macOS `CGEventTap`、Windows 低层键盘钩子）。绑定表归
/// [`DoubleTapState`]，这里只管"tap 在不在跑、不跑是为什么"——设置页的快捷键诊断据此就地提示。
#[cfg(any(target_os = "macos", target_os = "windows"))]
#[derive(Default)]
struct KeyTapService(std::sync::Mutex<KeyTapInner>);

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[derive(Default)]
struct KeyTapInner {
    /// 活着的 tap 句柄；drop 即停掉那条 run loop 线程。
    handle: Option<platform::TapHandle>,
    status: KeyTapStatus,
}

/// 按键观测状态（诊断投影）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
// 「缺辅助功能权限」只在 macOS 出现（Windows 低层钩子不需要授权）。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) enum KeyTapStatus {
    /// 本平台没有按键观测（macOS 与 Windows 以外）。
    #[default]
    Unsupported,
    /// 观测中。
    Running,
    /// 未授予辅助功能权限，无法建 tap。
    AccessibilityMissing,
    /// 已授权但 tap 创建失败。
    Failed,
}

/// 让按键观测与当前辅助功能授权对齐，返回对齐后的状态。幂等、可反复调用：启动、权限
/// 轮询发现变化、设置页拉诊断时都走这一个入口，用户授权后无需重启。
///
/// 授权在：没在跑就建，已在跑则原样。授权不在：停掉旧 tap 并标记原因。Windows 的低层钩子
/// 不需要授权，只有「在跑 / 建不起来」两种。
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(crate) fn ensure_key_tap(app: &tauri::AppHandle) -> KeyTapStatus {
    use tauri::Manager as _;

    let Some(service) = app.try_state::<KeyTapService>() else {
        return KeyTapStatus::Failed;
    };
    let Ok(mut inner) = service.0.lock() else {
        return KeyTapStatus::Failed;
    };
    #[cfg(target_os = "macos")]
    if !platform::macos::is_process_trusted() {
        inner.handle = None;
        inner.status = KeyTapStatus::AccessibilityMissing;
        return inner.status;
    }
    if inner.status == KeyTapStatus::Running && inner.handle.is_some() {
        return KeyTapStatus::Running;
    }
    inner.handle = None;
    match start_key_tap(app) {
        Ok(handle) => {
            inner.handle = Some(handle);
            inner.status = KeyTapStatus::Running;
        }
        Err(e) => {
            eprintln!("[keytap] 启动失败：{e}");
            inner.status = KeyTapStatus::Failed;
        }
    }
    inner.status
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(crate) fn ensure_key_tap(_app: &tauri::AppHandle) -> KeyTapStatus {
    KeyTapStatus::Unsupported
}

/// 建 tap 并接上手势分发：双击命中 → 按绑定 id 查 [`GLOBAL_SHORTCUT_SETTINGS`] 派发动作；
/// `Escape` → 关闭截屏框选层 / 弹窗面板。回调跑在 tap 的 `CFRunLoop` 线程（Windows 是钩子线程），
/// 只做投递。
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn start_key_tap(app: &tauri::AppHandle) -> Result<platform::TapHandle, platform::KeyTapError> {
    use platform::KeyTap as _;
    use tauri::Manager as _;

    let double_tap = app
        .try_state::<DoubleTapState>()
        .ok_or(platform::KeyTapError::TapCreationFailed)?;
    #[cfg(target_os = "macos")]
    let tap = platform::macos::CGEventTapKeyTap::new(std::sync::Arc::clone(&double_tap.0));
    #[cfg(target_os = "windows")]
    let tap = platform::windows::keytap::LowLevelKeyTap::new(std::sync::Arc::clone(&double_tap.0));
    let app = app.clone();
    tap.start(Box::new(move |ev| match ev {
        platform::TapEvent::DoubleTap(id) => {
            let Some(&(_, action)) = GLOBAL_SHORTCUT_SETTINGS.get(id as usize) else {
                return;
            };
            match action {
                GlobalShortcutAction::PopupTranslate => {
                    // 弹窗翻译走选区捕获编排：**同步抓基线 changeCount**（须在用户那次复制
                    // 落盘前取，否则丢失"是否自增"信号），其余编排派 tokio 后台任务。
                    // Windows 的对应物是剪贴板序号（钩子回调先于目标应用处理这次 Ctrl+C）。
                    #[cfg(target_os = "macos")]
                    let baseline = platform::macos::clipboard::change_count();
                    #[cfg(target_os = "windows")]
                    let baseline = platform::windows::clipboard::sequence_number();
                    tauri::async_runtime::spawn(run_double_copy_capture(app.clone(), baseline));
                }
                GlobalShortcutAction::VoiceInput => {
                    // 双击无按住语义（无独立松开相）→ 仅唤出 HUD 面板，不驱动 hold-to-talk。
                    let app_voice = app.clone();
                    let _ = app.run_on_main_thread(move || {
                        show_voice_hud_from_rust(&app_voice, ipc::VoiceInputHudPresetDto::Current);
                    });
                }
                other => dispatch_shortcut_action(&app, other, true),
            }
        }
        platform::TapEvent::Escape => {
            // 截屏框选层优先（它盖在最上层，PRD-01-007），其次关闭可见的弹窗面板。
            let app_esc = app.clone();
            let _ = app.run_on_main_thread(move || {
                use tauri::Manager as _;
                // 截屏框选层只在 macOS 实现（Windows 截屏归 W3）。
                #[cfg(target_os = "macos")]
                if is_screenshot_overlay_visible(&app_esc) {
                    screenshot::cancel_overlay(&app_esc);
                    return;
                }
                if let Some(win) = app_esc.get_webview_window("popup-translate") {
                    if win.is_visible().unwrap_or(false) {
                        close_popup(&app_esc);
                    }
                }
            });
        }
    }))
}

/// 双击复制命中后的**选区捕获编排**（TDD-05 §1.2/§1.4，决策 A）。
///
/// 流程：轮询 `changeCount` 自增（不固定延迟）→ 自增则取选区文本（方案 A AX 直读优先、否则读
/// 用户那次复制）→ [`classify_capture`] 判定展示/输入 → 写 Rust 会话（源文本+选区快照归 Rust，
/// ADR-003）→ 主线程 `show_popup_panel`（内含 eval 刷新桥让弹窗拉取会话真值渲染）。
///
/// 跑在 tokio 任务（非 CFRunLoop 线程）：轮询的 `sleep` 不阻塞 tap 回调。
#[cfg(target_os = "macos")]
async fn run_double_copy_capture(app: tauri::AppHandle, baseline: i64) {
    use crate::popup::{classify_capture, SelectionSnapshot};
    use platform::macos::{clipboard, selection};

    // 轮询 changeCount 自增（§1.2：用"复制真的落盘"的信号，而非固定延迟）。
    let incremented = poll_changecount_incremented(baseline).await;
    // **仅自增分支取文本**（避免读到旧剪贴板）：方案 A AX 直读优先，无 AX 暴露时读用户复制。
    let captured = if incremented {
        selection::capture_selected_text_snapshot()
            .map(|snapshot| {
                (
                    snapshot.text.clone(),
                    SelectionSnapshot::with_macos_target(snapshot.text, snapshot.target),
                )
            })
            .or_else(|| {
                clipboard::read_text().map(|text| {
                    let snapshot = SelectionSnapshot::new(text.clone());
                    (text, snapshot)
                })
            })
    } else {
        None
    };
    let kind = classify_capture(
        incremented,
        captured.as_ref().map(|(text, _)| text.as_str()),
    );
    deliver_popup_capture(app, "双击捕获", kind, captured, true);
}

/// Windows 的双击复制捕获（ADR-163 ②）：与 macOS 同一判定（[`popup::classify_capture`]），
/// 只是「复制落盘」的信号换成剪贴板序号。读的是用户本人那次复制（只读不写，PRD-01-027），
/// 不合成任何键；「替换原文」的目标是此刻的前台窗口（弹窗非激活，不会改变它）。
#[cfg(target_os = "windows")]
async fn run_double_copy_capture(app: tauri::AppHandle, baseline: u32) {
    use crate::popup::{classify_capture, SelectionSnapshot};
    use platform::windows::{clipboard, foreground, selection};

    let target = foreground::foreground_window();
    let incremented = selection::wait_sequence_changed(baseline, selection::COPY_TIMEOUT).await;
    let text = if incremented {
        tauri::async_runtime::spawn_blocking(clipboard::read_text)
            .await
            .ok()
            .flatten()
    } else {
        None
    };
    let captured = text.map(|text| {
        (
            text.clone(),
            SelectionSnapshot::with_windows_target(text, target),
        )
    });
    let kind = classify_capture(
        incremented,
        captured.as_ref().map(|(text, _)| text.as_str()),
    );
    deliver_popup_capture(app, "双击捕获", kind, captured, true);
}

/// Windows 的弹窗热键带上选区（ADR-163 ②）：UI Automation 优先，读不到回落受保护的剪贴板复制；
/// 有选区即展示模式译选区，没有才开输入模式空窗（与双击无选区同一归宿）。剪贴板没能原样还原时
/// 由弹窗如实提示。
#[cfg(target_os = "windows")]
async fn run_hotkey_selection_capture(app: tauri::AppHandle) {
    use crate::popup::{classify_capture, SelectionSnapshot};

    let captured = platform::windows::selection::capture_selection().await;
    let intact = captured
        .clipboard
        .is_none_or(platform::windows::clipboard::ClipboardRestore::is_intact);
    let target = captured.target;
    let pair = captured.text.map(|text| {
        (
            text.clone(),
            SelectionSnapshot::with_windows_target(text, target),
        )
    });
    let kind = classify_capture(pair.is_some(), pair.as_ref().map(|(text, _)| text.as_str()));
    deliver_popup_capture(app, "热键捕获", kind, pair, intact);
}

/// 把一次捕获写进弹窗会话并显示弹窗（双击 / 热键捕获共用的收尾）。
///
/// 决策 A：源文本与选区归 Rust，前端纯视图；显示在主线程，`show_popup_panel` 内的 eval 刷新桥
/// 会让弹窗拉取刚写入的会话真值并渲染（display→自动生成 / input→空窗）。
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn deliver_popup_capture(
    app: tauri::AppHandle,
    origin: &str,
    kind: popup::CaptureKind,
    captured: Option<(String, popup::SelectionSnapshot)>,
    clipboard_intact: bool,
) {
    use crate::popup::CaptureKind;
    use tauri::Manager as _;

    let len = captured.as_ref().map_or(0, |(text, _)| text.len());
    if let Some(state) = app.try_state::<ipc::AppState>() {
        if let Ok(mut session) = state.popup().lock() {
            match (&kind, captured) {
                (CaptureKind::DisplayWithSelection, Some((text, snapshot))) => {
                    session.set_display_with_snapshot(text, snapshot);
                }
                // 无选区 → 输入模式空窗：源文本留空，待用户键入经 popup_set_input。
                _ => session.set_empty_input(),
            }
            if !clipboard_intact {
                session.mark_clipboard_not_restored();
            }
        }
    }

    let (mode, has_selection) = match kind {
        CaptureKind::DisplayWithSelection => ("display", true),
        CaptureKind::InputNoSelection => ("input", false),
    };
    // 脱敏日志（TDD-12 §1）：只记长度，绝不记选区正文。
    eprintln!("[F1] {origin} → mode={mode} has_selection={has_selection} len={len}");

    let _ = app.clone().run_on_main_thread(move || {
        show_popup_panel(&app);
    });
}

/// 快捷对话唤起时的**选区带入捕获**（PRD-03-008 / TDD-05 §1.4）。
///
/// 与双击复制路径（[`run_double_copy_capture`]）的差别：热键路径没有"用户本人复制"这一步，
/// 所以降级分支用**受保护的剪贴板捕获**——备份全类型 → 合成 `Cmd+C`（带自污染防护魔数，
/// 本进程 tap 忽略）→ 轮询 `changeCount` 落盘 → 读取 → **无损还原**用户剪贴板。
/// AX 直读命中则零剪贴板接触。捕获到非空文本即写入 chat prefill 单槽并广播
/// `chat://prefill`（正文不进事件系统，前端经 `chat_prefill_take` 单次拉取）。
#[cfg(target_os = "macos")]
async fn run_quick_chat_selection_capture(app: tauri::AppHandle) {
    use platform::macos::{clipboard, selection, synthetic};
    use tauri::Manager as _;

    // 方案 A：AX 直读选区（零剪贴板接触、无感）。
    let text = match selection::capture_selected_text_snapshot() {
        Some(snapshot) => Some(snapshot.text),
        None => {
            // 降级：受保护的剪贴板捕获（备份 → 合成复制 → 轮询 → 读取 → 还原）。
            // 全程持 CLIPBOARD_MUTEX 一次（TDD-05 §1.4/§2.1-B：与合成粘贴注入的
            // "备份→写→还原"序列抢同一把锁，防两条路径交叉污染剪贴板；本函数是叶子，
            // 锁内不再调用其它取锁函数）。
            let _lock = platform::CLIPBOARD_MUTEX.lock().await;
            let backup = clipboard::read_all_types();
            let baseline = clipboard::change_count();
            if synthetic::post_synthetic_copy().is_err() {
                None
            } else {
                let incremented = poll_changecount_incremented(baseline).await;
                let captured = if incremented {
                    clipboard::read_text()
                } else {
                    None
                };
                clipboard::restore_all_types(&backup);
                captured
            }
        }
    };
    // 脱敏日志（TDD-12 §1）：只记长度，绝不记选区正文。
    eprintln!(
        "[C9] 快捷对话选区捕获 → len={}",
        text.as_ref().map_or(0, String::len)
    );
    let Some(text) = text else {
        return;
    };
    if let Some(state) = app.try_state::<ipc::AppState>() {
        state.offer_chat_prefill(&app, text);
    }
}

/// 轮询通用剪贴板 `changeCount` 是否自增（带 ~300ms 超时，TDD-05 §1.2）。
#[cfg(target_os = "macos")]
async fn poll_changecount_incremented(baseline: i64) -> bool {
    use platform::macos::clipboard;
    const POLL_INTERVAL_MS: u64 = 10;
    const POLL_TIMEOUT_MS: u64 = 300;

    for _ in 0..(POLL_TIMEOUT_MS / POLL_INTERVAL_MS) {
        if clipboard::change_count() != baseline {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
    clipboard::change_count() != baseline
}

#[cfg(test)]
mod tests {
    use super::{
        is_global_shortcut_setting, is_reserved_editing_combo, luna_tray_icon,
        luna_tray_icon_as_template, normalize_to_accelerator, parse_shortcut, place_lens_trigger,
        HudWindowGeometry, GLOBAL_SHORTCUT_SETTINGS, LIVE_CAPTION_HUD_GEOMETRY,
        POPUP_TRANSLATE_HUD_GEOMETRY, QUICK_CHAT_HUD_GEOMETRY, RECORDING_HUD_GEOMETRY,
        VOICE_INPUT_HUD_GEOMETRY,
    };
    use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut};

    /// 取景框 560×320 点 @ (400, y)，控制条 72×36 点，间距 6 点。全程逻辑点、与显示器
    /// scale 无关——这正是修掉 Retina 上 2× 错位的关键不变量。
    fn lens_trigger_case(lens_y: f64, display_top: Option<f64>) -> tauri::LogicalPosition<f64> {
        place_lens_trigger(
            tauri::LogicalPosition::new(400.0, lens_y),
            tauri::LogicalSize::new(560.0, 320.0),
            tauri::LogicalSize::new(72.0, 36.0),
            display_top,
        )
    }

    #[test]
    fn lens_trigger_hangs_outside_the_top_edge_and_aligns_right() {
        let placed = lens_trigger_case(300.0, Some(0.0));
        // 右缘对齐：400 + 560 - 72。
        assert_eq!(placed.x, 888.0);
        // 外挂在框上方：300 - 36 - 6，不压住取景内容。
        assert_eq!(placed.y, 258.0);
    }

    #[test]
    fn lens_trigger_flips_inside_when_it_would_leave_the_display() {
        // 取景框贴着屏幕顶边：上方放不下 → 翻回框内上缘，绝不飞出屏幕。
        let placed = lens_trigger_case(20.0, Some(0.0));
        assert_eq!(placed.x, 888.0);
        assert_eq!(placed.y, 20.0);
    }

    #[test]
    fn lens_trigger_respects_a_non_zero_display_origin() {
        // 副屏原点非 0（如上方还有一块屏）：以该屏顶边为界，恰好放得下即外挂。
        assert_eq!(lens_trigger_case(1042.0, Some(1000.0)).y, 1000.0);
        // 再低一点点就放不下 → 翻回框内。
        assert_eq!(lens_trigger_case(1041.0, Some(1000.0)).y, 1041.0);
    }

    /// 恢复几何的可见性判据与采集侧共用同一函数：陈旧坐标（显示器拔出/分辨率变化）不得
    /// 被复用，否则取景框停在不可见处，用户既找不到它也点不到控制条关闭。
    #[test]
    fn stale_saved_geometry_is_not_reused_for_restore() {
        use crate::screenshot::{rect_within_single_display, SelectionRect};
        let displays = [SelectionRect {
            x: 0.0,
            y: 0.0,
            width: 1512.0,
            height: 982.0,
        }];
        let on_screen = SelectionRect {
            x: 100.0,
            y: 80.0,
            width: 560.0,
            height: 320.0,
        };
        // 外接屏拔出后残留的坐标：落在已不存在的显示器上 → 判为不可复用，改居中重放。
        let stale = SelectionRect {
            x: 2000.0,
            y: 80.0,
            width: 560.0,
            height: 320.0,
        };
        assert!(rect_within_single_display(&on_screen, &displays));
        assert!(!rect_within_single_display(&stale, &displays));
    }

    #[test]
    fn lens_trigger_without_display_info_still_hangs_outside() {
        // 显示器信息不可读时不施加上边界约束，宁可外挂也不无谓压回内容上。
        assert_eq!(lens_trigger_case(20.0, None).y, -22.0);
    }

    #[test]
    fn hud_tier_heights_stay_within_each_window_bounds() {
        // 每档高度不小于该窗最小高（否则 set_size 会被窗口最小尺寸顶住）；首帧档与首帧几何一致。
        for (label, geometry, tiers) in [
            (
                "voice-input-hud",
                VOICE_INPUT_HUD_GEOMETRY,
                &["pill", "card", "panel"][..],
            ),
            (
                "popup-translate",
                POPUP_TRANSLATE_HUD_GEOMETRY,
                &["input", "result"][..],
            ),
            (
                "recording-hud",
                RECORDING_HUD_GEOMETRY,
                &["bar", "card", "panel"][..],
            ),
        ] {
            for tier in tiers {
                let height = super::hud_tier_height(label, tier).expect("档位已登记");
                assert!(height >= geometry.min_height, "{label}/{tier} 低于最小高");
            }
        }
        assert_eq!(
            super::hud_tier_height("voice-input-hud", "pill"),
            Some(56.0)
        );
        assert_eq!(
            super::hud_tier_height("voice-input-hud", "settings"),
            None,
            "配置是 panel 档本身，不再另设一档"
        );
        assert_eq!(
            super::hud_tier_height("popup-translate", "input"),
            Some(200.0)
        );
        assert_eq!(super::hud_tier_height("recording-hud", "card"), Some(150.0));
        assert_eq!(
            super::hud_tier_height("quick-chat", "panel"),
            None,
            "未登记的窗不猜"
        );
    }

    #[test]
    fn hud_first_frame_geometries_fit_the_current_content_contract() {
        let surfaces: [(&str, HudWindowGeometry); 5] = [
            ("popup-translate", POPUP_TRANSLATE_HUD_GEOMETRY),
            ("voice-input-hud", VOICE_INPUT_HUD_GEOMETRY),
            ("quick-chat", QUICK_CHAT_HUD_GEOMETRY),
            ("recording-hud", RECORDING_HUD_GEOMETRY),
            ("live-caption", LIVE_CAPTION_HUD_GEOMETRY),
        ];

        for (label, geometry) in surfaces {
            assert!(
                geometry.initial_width >= geometry.min_width,
                "{label} 首帧宽度不得小于可用下限"
            );
            assert!(
                geometry.initial_height >= geometry.min_height,
                "{label} 首帧高度不得小于可用下限"
            );
        }

        // 锁定这轮 UI 的完整首帧基线：未来若增添首屏内容，必须同步评估这里而非重新引入裁切。
        // 弹窗翻译输入态首帧只容输入卡（ADR-112 ④，结果区首 token 才出现，随内容向下扩容）。
        assert_eq!(POPUP_TRANSLATE_HUD_GEOMETRY.initial_height, 200.0);
        // 语音胶囊首帧只有一行（ADR-137 V7：圆钮 32 + 内距 + 细边）。
        assert_eq!(VOICE_INPUT_HUD_GEOMETRY.initial_height, 56.0);
        assert_eq!(QUICK_CHAT_HUD_GEOMETRY.initial_height, 520.0);
        // 录音首帧 = 一行控制条 + 最近两条字幕（ADR-137 H5）。
        assert_eq!(RECORDING_HUD_GEOMETRY.initial_height, 150.0);
        // 字幕首帧 = 工具药丸 + 两行双语字幕；外观面板在窗内滚动，不靠窗高撑。
        assert_eq!(LIVE_CAPTION_HUD_GEOMETRY.initial_height, 280.0);
    }

    fn cmd_or_ctrl() -> Modifiers {
        #[cfg(target_os = "macos")]
        {
            Modifiers::SUPER
        }
        #[cfg(not(target_os = "macos"))]
        {
            Modifiers::CONTROL
        }
    }

    #[test]
    fn tray_icon_uses_luna_brand_asset_without_template_monochrome() {
        let icon = luna_tray_icon().expect("Lunarca 托盘图标应可解码");

        assert_eq!((icon.width(), icon.height()), (32, 32));
        assert!(icon
            .rgba()
            .chunks_exact(4)
            .any(|pixel| pixel == [0xf3, 0xd9, 0x98, 0xff]));
        assert!(!luna_tray_icon_as_template());
    }

    #[test]
    fn normalize_settings_format_to_plugin_accelerator() {
        assert_eq!(
            normalize_to_accelerator("cmd_or_ctrl+shift+r"),
            "CmdOrCtrl+Shift+R"
        );
        assert_eq!(
            normalize_to_accelerator("cmd_or_ctrl+shift+v"),
            "CmdOrCtrl+Shift+V"
        );
        // 别名/大小写/空白容错。
        assert_eq!(normalize_to_accelerator(" ctrl + alt + o "), "Ctrl+Alt+O");
        assert_eq!(normalize_to_accelerator("super+shift+a"), "Super+Shift+A");
    }

    #[test]
    fn parse_shortcut_maps_settings_string_to_correct_binding() {
        // 1.4 命门：设置键串解析出的 Shortcut 必须等价于期望的修饰键 + 键码，
        // 否则改键"注册了但匹配不上"→ 改键不生效。
        assert_eq!(
            parse_shortcut("cmd_or_ctrl+shift+r"),
            Some(Shortcut::new(
                Some(cmd_or_ctrl() | Modifiers::SHIFT),
                Code::KeyR
            ))
        );
        assert_eq!(
            parse_shortcut("cmd_or_ctrl+shift+v"),
            Some(Shortcut::new(
                Some(cmd_or_ctrl() | Modifiers::SHIFT),
                Code::KeyV
            ))
        );
        // 用户改键示例：录音切换改成 Cmd/Ctrl+Shift+M。
        assert_eq!(
            parse_shortcut("cmd_or_ctrl+shift+m"),
            Some(Shortcut::new(
                Some(cmd_or_ctrl() | Modifiers::SHIFT),
                Code::KeyM
            ))
        );
    }

    /// 注册表里声明为全局的快捷键与动作表一一对应。
    ///
    /// 漏掉一项就是一个可录、能保存、却什么都不注册也没有诊断的设置。
    #[test]
    fn global_shortcut_table_matches_registry_scope() {
        use luna_config::{SettingControl, ShortcutScope, SETTINGS};
        let registry: std::collections::BTreeSet<&str> = SETTINGS
            .iter()
            .filter(|spec| {
                matches!(
                    spec.control,
                    SettingControl::Shortcut {
                        scope: ShortcutScope::Global
                    }
                )
            })
            .map(|spec| spec.key)
            .collect();
        let table: std::collections::BTreeSet<&str> = super::GLOBAL_SHORTCUT_SETTINGS
            .iter()
            .map(|(key, _)| *key)
            .collect();
        assert_eq!(registry, table, "注册表全局快捷键 ≠ 动作表");
    }

    /// 主键词表（`MAIN_KEYS`）里的每个名字都必须能注册成全局热键：录制器只产出词表内的
    /// 名字，校验只放行词表内的名字，这一条守住第三段。
    #[test]
    fn every_name_the_recorder_can_produce_is_registrable() {
        let unregistrable: Vec<&str> = luna_config::shortcut::MAIN_KEYS
            .iter()
            .copied()
            .filter(|name| parse_shortcut(&format!("cmd_or_ctrl+shift+{name}")).is_none())
            .collect();
        assert_eq!(
            unregistrable,
            Vec::<&str>::new(),
            "录制器能产出、这边却注册不上的名字——用户会得到一个按下去没反应的键"
        );
    }

    #[test]
    fn parse_shortcut_accepts_space_as_main_key() {
        // 文字速记的默认键是 `Cmd/Ctrl+Shift+Space`（PRD-06-058 B″ / TDD-05 §3）。
        // `normalize_to_accelerator` 把主键整体大写（`SPACE`），而插件那侧接不接受这个
        // 写法没有第二处证据：接不了的表现不是报错，是那个键从此一按没反应。键位表里
        // 此前清一色是字母与数字，本条是第一个非字母主键，故单独钉住。
        assert_eq!(
            parse_shortcut("cmd_or_ctrl+shift+space"),
            Some(Shortcut::new(
                Some(cmd_or_ctrl() | Modifiers::SHIFT),
                Code::Space
            ))
        );
    }

    #[test]
    fn text_quick_note_panel_has_registered_tiers() {
        // 档位没登记时 `hud_set_tier` 直接返回 Err，表现是这扇窗永远停在首帧高度
        // （ADR-112 ③：尺寸只在 Rust）。
        assert_eq!(
            crate::hud_tier_height("text-quick-note", "pill"),
            Some(crate::TEXT_QUICK_NOTE_HUD_GEOMETRY.initial_height)
        );
        assert!(crate::hud_tier_height("text-quick-note", "card").is_some());
    }

    #[test]
    fn parse_shortcut_rejects_empty() {
        assert_eq!(parse_shortcut(""), None);
        assert_eq!(parse_shortcut("   "), None);
    }

    #[test]
    fn global_shortcut_settings_keys_are_recognized() {
        assert!(is_global_shortcut_setting("shortcuts.recording_toggle"));
        assert!(is_global_shortcut_setting("shortcuts.voice_input"));
        assert!(is_global_shortcut_setting("shortcuts.quick_chat"));
        assert!(is_global_shortcut_setting("shortcuts.popup_translate"));
        // 应用内快捷键由前端命令注册表管，不在全局列；非快捷键键不误判。
        assert!(!is_global_shortcut_setting("shortcuts.quick_switcher"));
        assert!(!is_global_shortcut_setting("recording.format"));
        for (key, _) in GLOBAL_SHORTCUT_SETTINGS {
            assert!(is_global_shortcut_setting(key));
        }
    }

    #[test]
    fn reserved_editing_combos_are_detected_in_both_syntaxes() {
        // 保留编辑键：Cmd/Ctrl + 单编辑键（无额外修饰键），普通与双击语法均识别。
        for spec in [
            "cmd_or_ctrl+c",
            "double_tap:cmd_or_ctrl+c",
            "cmd+v",
            "ctrl+x",
            "cmd_or_ctrl+a",
            "double_tap:cmd_or_ctrl+z",
        ] {
            assert!(is_reserved_editing_combo(spec), "{spec} 应为保留编辑键");
        }
        // 带额外修饰键或非编辑主键的组合是安全的自定义键，可作普通全局热键。
        for spec in [
            "cmd_or_ctrl+shift+c",
            "cmd_or_ctrl+alt+v",
            "cmd_or_ctrl+shift+t",
            "double_tap:cmd_or_ctrl+shift+v",
            "cmd_or_ctrl+t",
            "",
        ] {
            assert!(!is_reserved_editing_combo(spec), "{spec} 不应为保留编辑键");
        }
    }
}

#[cfg(test)]
mod remote_asr_default_tests {

    /// 服务端转写默认**关**。
    ///
    /// 这条钉的是 ADR-038 ④「默认档不得是云」。麦克风与会议录音是隐私量级最高的
    /// 一类内容，不该因为用户为了别的功能连了服务端就悄悄开始上传。默认值被改成
    /// `true` 时，编译、lint、其余单测全绿，而每一段录音都开始离开设备。
    #[test]
    fn remote_asr_is_off_by_default() {
        let settings = luna_config::Settings::with_defaults();
        assert_eq!(
            settings.get_bool("provider.remote_asr_enabled"),
            Some(false),
            "服务端转写必须默认关闭"
        );
    }
}

/// 会话被动失效的观察者（装配闭包里出生、setup 里挂到云客户端上）。
struct SessionLostHook(luna_cloud::SessionObserver);

/// 连接链接的两条到达路径（ADR-116）：运行中经 `deep-link://new-url`；冷启动即由链接拉起时
/// 经 `get_current`。两条都只把 URL 交给 `ipc::handle_incoming_deep_link`——它存起来、
/// 亮主窗、通知界面；**不在这里解析、更不在这里写任何设置**。
///
/// Windows / Linux 上「运行中」那条靠单实例插件：点链接另起的进程把 argv 交给已在跑的实例
/// 后自行退出，`deep-link` feature 再把它转成同一条 `on_open_url`（见 `run_with` 插件链首）。
fn install_connect_link_listener(app: &tauri::AppHandle) {
    use tauri_plugin_deep_link::DeepLinkExt as _;
    // 开发构建运行时登记 scheme（写当前用户的 HKCU\Software\Classes\lunarca，指向这个 debug
    // 可执行文件；TDD-13）。发行版由安装包登记，不在运行时改注册表。macOS 由 Info.plist 声明。
    #[cfg(all(debug_assertions, not(target_os = "macos")))]
    if let Err(error) = app.deep_link().register_all() {
        eprintln!("[deep-link] 开发构建登记 lunarca:// 失败：{error}");
    }
    let handle = app.clone();
    app.deep_link().on_open_url(move |event| {
        for url in event.urls() {
            ipc::handle_incoming_deep_link(&handle, url.as_str());
        }
    });
    if let Ok(Some(urls)) = app.deep_link().get_current() {
        for url in urls {
            ipc::handle_incoming_deep_link(app, url.as_str());
        }
    }
}
