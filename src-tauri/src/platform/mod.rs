//! 平台系统集成抽象（TDD-05）。`luna-app` 承担平台 trait + Tauri 接线。
//!
//! 本模块的**纯逻辑**（双击时序判定、自发事件指纹）零平台依赖、headless 可单测；具体的
//! `CGEventTap`/AX/NSPanel 实现按平台分入子模块（[`macos`]，`cfg(target_os = "macos")`；`windows`，
//! `cfg(target_os = "windows")`，其纯判定在测试构建里全平台编译）。
//!
//! 运行时行为（双击观测不影响复制、超时自动重启用、事件源过滤、非夺焦）只能在真机 +
//! 辅助功能授权下验证；headless 只保证纯逻辑。

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(any(target_os = "windows", test))]
pub mod windows;

/// 权限编排（TDD-05 §4）：纯逻辑（种类/状态/降级/撤销检测）+ 跨平台 trait，零平台依赖、可单测。
pub mod permissions;

use std::time::Duration;

/// 当前 GPU 利用率百分比（0–100）。macOS 经 IOKit `IOAccelerator` 的
/// `PerformanceStatistics` → `Device Utilization %`（无需 root）；其它平台或读取失败返回
/// `None`（状态栏据此仅显示 CPU）。事实源：状态栏资源监控（PRD-04-033）。
#[must_use]
pub fn gpu_utilization_percent() -> Option<f32> {
    #[cfg(target_os = "macos")]
    {
        macos::gpu::utilization_percent()
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// 同步大文件的暂停条件（ADR-156 ③）：此刻的网络与电量。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LargeTransferConditions {
    /// 按流量计费的网络（手机热点、蜂窝、低数据模式）。
    pub metered: bool,
    /// 电量低：低电量模式，或用电池供电且低于 [`LOW_BATTERY_PERCENT`]。
    pub low_battery: bool,
}

/// 电量低的门槛（用电池供电时）。
pub const LOW_BATTERY_PERCENT: u8 = 20;

/// 电量低的判定（纯逻辑）：低电量模式开着，或用电池供电且低于门槛；插着电不算。
#[must_use]
pub fn battery_is_low(battery: Option<(bool, u8)>, low_power_mode: bool) -> bool {
    low_power_mode
        || battery.is_some_and(|(on_battery, percent)| on_battery && percent < LOW_BATTERY_PERCENT)
}

/// 起网络路径监听（应用装配时调一次；幂等）。
pub fn start_large_transfer_watch() {
    #[cfg(target_os = "macos")]
    macos::power_network::start_network_monitor();
    #[cfg(target_os = "windows")]
    self::windows::power_network::start_network_monitor();
}

/// 此刻的暂停条件。macOS 读网络路径监听与电源信息，Windows 读 WinRT 网络费用与
/// `GetSystemPowerStatus`；其它平台恒为不暂停（不会误停同步，只是那两项开关在那里不起作用）。
#[must_use]
pub fn large_transfer_conditions() -> LargeTransferConditions {
    #[cfg(target_os = "macos")]
    {
        use macos::power_network as pn;
        LargeTransferConditions {
            metered: pn::network_is_metered(),
            low_battery: battery_is_low(pn::battery(), pn::low_power_mode()),
        }
    }
    #[cfg(target_os = "windows")]
    {
        use self::windows::power_network as pn;
        LargeTransferConditions {
            metered: pn::network_is_metered(),
            low_battery: battery_is_low(pn::battery(), pn::low_power_mode()),
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        LargeTransferConditions::default()
    }
}

/// 「注入光标」的目标（ADR-163 ③）：Windows 是此刻的前台窗口，注入前核对它仍在前台；
/// macOS 的注入写的是 AX 焦点元素，不按窗口核对，恒为 `None`。
#[must_use]
pub fn inject_target_now() -> Option<isize> {
    #[cfg(target_os = "windows")]
    {
        self::windows::foreground::foreground_window()
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

/// **自发事件指纹（单一来源，TDD-05 §1.4 硬约束）**。
///
/// Lunarca 合成的 `Cmd+C`（选区捕获）/`Cmd+V`（注入）用专属 `CGEventSource` 的
/// `kCGEventSourceUserData` 打此魔数；`CGEventTap` 回调首行据此**过滤本进程自发事件**，
/// 杜绝"合成复制被自己的双击检测误判 → 误弹窗甚至递归"。`KeyTap` 观测端（本模块）与
/// `TextInjector` 合成端**共享此常量**，不得各写一份。
///
/// 取值 = ASCII `"LUNA"`（`0x41 55 52 41`），仅需进程内自识别、非安全边界。
pub const LUNA_SYNTHETIC_EVENT_MAGIC: i64 = 0x4155_5241;

/// 双击手势的默认时间窗（`T_double`，TDD-05 §1.2，可配：`shortcuts.double_tap_window_ms`）。
pub const DEFAULT_DOUBLE_TAP_WINDOW: Duration = Duration::from_millis(400);

/// 双击型快捷键值的前缀（值语法 `double_tap:<组合>`）。
///
/// **唯一事实源在配置层**（`luna_config::shortcut::DOUBLE_TAP_PREFIX`）——值语法属于设置的
/// 契约，校验（`set_setting`）、平台手势解析与设置 UI 三处必须同一份，故此处只做转出。
pub use luna_config::shortcut::DOUBLE_TAP_PREFIX;

/// 解析后的双击手势（纯数据；修饰键要求**精确匹配**，防 `Cmd+Shift+C` 误触 `Cmd+C` 手势）。
///
/// 只从配置层的 [`luna_config::shortcut::Combo`] 派生，不另起一套词法：值语法、主键词表与
/// 归一化都归配置层，这里只做「词表名 → 虚拟键码」的映射。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DoubleTapGesture {
    /// 本平台键码（[`platform_keycode_for_key`]：macOS `kVK_*`，Windows 虚拟键码）。
    pub keycode: u16,
    /// 需要 Command（`cmd_or_ctrl`：macOS 是 Command，Windows 是 Ctrl）。
    pub cmd: bool,
    /// 需要 Shift。
    pub shift: bool,
    /// 需要 Option/Alt。
    pub alt: bool,
}

impl DoubleTapGesture {
    /// 由归一化组合得到手势；主键在本平台没有虚拟键码时为 `None`。
    ///
    /// 词表（[`luna_config::shortcut::MAIN_KEYS`]）里的每个名字都必须映射得上——由用例
    /// `every_main_key_has_a_macos_keycode` / `every_main_key_has_a_windows_vk` 钉住，故对合法
    /// 设置值本函数不会返回 `None`。
    #[must_use]
    pub fn for_combo(combo: &luna_config::shortcut::Combo) -> Option<Self> {
        Some(Self {
            keycode: platform_keycode_for_key(&combo.key)?,
            cmd: combo.cmd_or_ctrl,
            shift: combo.shift,
            alt: combo.alt,
        })
    }

    /// 由设置值（`double_tap:<组合>` 语法）得到手势；普通组合或非法值为 `None`。
    #[must_use]
    pub fn for_setting_value(value: &str) -> Option<Self> {
        let binding = luna_config::shortcut::parse_binding(value).ok()?;
        binding
            .double_tap
            .then(|| Self::for_combo(&binding.combo))
            .flatten()
    }
}

/// 主键词表名 → 本平台键码：Windows 是虚拟键码（[`windows::keys::windows_vk_for_key`]，按键事件由
/// 低层键盘钩子报来），其余平台是 macOS `kVK_*`（`CGEventTap` 报来）。
#[must_use]
pub fn platform_keycode_for_key(token: &str) -> Option<u16> {
    #[cfg(target_os = "windows")]
    {
        self::windows::keys::windows_vk_for_key(token)
    }
    #[cfg(not(target_os = "windows"))]
    {
        macos_keycode_for_key(token)
    }
}

/// 主键词表名 → macOS 虚拟键码（`kVK_*`，US 物理位）。覆盖 [`luna_config::shortcut::MAIN_KEYS`] 全集。
#[must_use]
pub fn macos_keycode_for_key(token: &str) -> Option<u16> {
    let code = match token {
        "a" => 0,
        "s" => 1,
        "d" => 2,
        "f" => 3,
        "h" => 4,
        "g" => 5,
        "z" => 6,
        "x" => 7,
        "c" => 8,
        "v" => 9,
        "b" => 11,
        "q" => 12,
        "w" => 13,
        "e" => 14,
        "r" => 15,
        "y" => 16,
        "t" => 17,
        "1" => 18,
        "2" => 19,
        "3" => 20,
        "4" => 21,
        "6" => 22,
        "5" => 23,
        "equal" => 24,
        "9" => 25,
        "7" => 26,
        "minus" => 27,
        "8" => 28,
        "0" => 29,
        "bracketright" => 30,
        "o" => 31,
        "u" => 32,
        "bracketleft" => 33,
        "i" => 34,
        "p" => 35,
        "enter" => 36,
        "l" => 37,
        "j" => 38,
        "quote" => 39,
        "k" => 40,
        "semicolon" => 41,
        "backslash" => 42,
        "comma" => 43,
        "slash" => 44,
        "n" => 45,
        "m" => 46,
        "period" => 47,
        "tab" => 48,
        "space" => 49,
        "backquote" => 50,
        "backspace" => 51,
        "escape" => 53,
        "f17" => 64,
        "f18" => 79,
        "f19" => 80,
        "f20" => 90,
        "f5" => 96,
        "f6" => 97,
        "f7" => 98,
        "f3" => 99,
        "f8" => 100,
        "f9" => 101,
        "f11" => 103,
        "f13" => 105,
        "f16" => 106,
        "f14" => 107,
        "f10" => 109,
        "f12" => 111,
        "f15" => 113,
        // PC 键盘的 Insert 在 macOS 上是 Help 键位。
        "insert" => 114,
        "home" => 115,
        "pageup" => 116,
        "delete" => 117,
        "f4" => 118,
        "end" => 119,
        "f2" => 120,
        "pagedown" => 121,
        "f1" => 122,
        "left" => 123,
        "right" => 124,
        "down" => 125,
        "up" => 126,
        _ => return None,
    };
    Some(code)
}

/// 一条双击手势绑定：手势 + 稳定 id（调用方把 id 映射回动作，平台层不知语义）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GestureBinding {
    /// 触发手势。
    pub gesture: DoubleTapGesture,
    /// 绑定 id（= 调用方绑定表下标，随 [`TapEvent::DoubleTap`] 回传）。
    pub id: u32,
}

/// 双击手势运行期配置（设置驱动，热更新；TDD-05 §1.2「时间窗与触发总开关可配」，
/// 泛化后**任意全局快捷键**都可绑双击手势——绑定表由设置层重建整表替换）。
///
/// tap 回调在专用 `CFRunLoop` 线程逐键查表：`try_read` 拿不到锁（设置写入瞬间）即放过
/// 该键事件，绝不阻塞 tap（超时停用是头号失灵根因）；时间窗走原子。
#[derive(Debug)]
pub struct DoubleTapConfig {
    bindings: std::sync::RwLock<Vec<GestureBinding>>,
    window_ms: std::sync::atomic::AtomicU64,
}

impl Default for DoubleTapConfig {
    fn default() -> Self {
        Self {
            bindings: std::sync::RwLock::new(Vec::new()),
            window_ms: std::sync::atomic::AtomicU64::new(
                DEFAULT_DOUBLE_TAP_WINDOW.as_millis() as u64
            ),
        }
    }
}

impl DoubleTapConfig {
    /// 整表替换绑定（设置变更时由设置层重建；写锁只在此瞬间持有）。
    pub fn set_bindings(&self, bindings: Vec<GestureBinding>) {
        if let Ok(mut guard) = self.bindings.write() {
            *guard = bindings;
        }
    }

    /// 按键事件查绑定：修饰键**精确匹配**（防 `Cmd+Shift+C` 误触 `Cmd+C` 手势）。
    /// 值语法只有 `cmd_or_ctrl`（macOS = Command），故按住 Control 的组合一律不匹配。
    ///
    /// 锁竞争（设置写入瞬间）时返回 `None`——放过单次键事件优于阻塞 tap 线程。
    #[must_use]
    pub fn match_key(
        &self,
        keycode: i64,
        cmd: bool,
        shift: bool,
        alt: bool,
        ctrl: bool,
    ) -> Option<u32> {
        if ctrl {
            return None;
        }
        let guard = self.bindings.try_read().ok()?;
        guard
            .iter()
            .find(|binding| {
                i64::from(binding.gesture.keycode) == keycode
                    && binding.gesture.cmd == cmd
                    && binding.gesture.shift == shift
                    && binding.gesture.alt == alt
            })
            .map(|binding| binding.id)
    }

    /// 更新时间窗（非正值回退默认，防手滑填 0 把双击永久打死）。
    pub fn set_window_ms(&self, window_ms: i64) {
        let value = if window_ms > 0 {
            window_ms as u64
        } else {
            DEFAULT_DOUBLE_TAP_WINDOW.as_millis() as u64
        };
        self.window_ms
            .store(value, std::sync::atomic::Ordering::Relaxed);
    }

    /// 当前时间窗（毫秒）。
    #[must_use]
    pub fn window_ms(&self) -> u64 {
        self.window_ms.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// 剪贴板多格式备份（类型 UTI 字符串 → 原始字节）。选区捕获（§1.4-B）与合成粘贴注入
/// （§2.1-B）共用：备份→写→还原全格式无损（TDD-05）。平台无关，具体读写在 [`macos::clipboard`]。
pub type ClipboardBackup = Vec<(String, Vec<u8>)>;

/// **全局剪贴板互斥锁（单一来源，TDD-05 §1.4/§2.1 硬约束）**。
///
/// 选区主动捕获（§1.4-B）与合成粘贴注入（§2.1-B）都"备份→写→还原"系统剪贴板，必须串行，
/// 防止并发写互相覆盖（备份灾难）。**`tokio::sync::Mutex` 不可重入**：同一交互流程**每段只取
/// 一次锁**（"捕获取一次→释放→…→注入再取一次"），叶子函数（[`macos::clipboard`] 的备份/写/还原）
/// 不在持锁态互调。
pub static CLIPBOARD_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 文本注入错误（TDD-05 §2）。错误文本不得包含待注入正文。
#[derive(Debug, thiserror::Error)]
pub enum TextInjectionError {
    /// AX 直插不可用。
    #[error("direct AX insertion is unavailable")]
    AccessibilityUnavailable,
    /// 捕获后的源选区已经变化，不能自动替换。
    #[error("the source selection snapshot is stale")]
    SelectionSnapshotInvalid,
    /// 合成粘贴事件发送失败。
    #[error("synthetic paste failed: {0}")]
    SyntheticPasteFailed(String),
    /// 注入目标窗口已不在前台（Windows 注入前核对，ADR-163 ③）。
    #[error("the target window is no longer in front")]
    TargetNotInFront,
    /// 剪贴板被别的程序占着，写不进去。
    #[error("the clipboard is unavailable")]
    ClipboardUnavailable,
}

/// 跨平台文本注入抽象（TDD-05 §2）。弹窗替换原文、语音输入写回光标共用。
pub trait TextInjector: Send + Sync {
    /// 把文本注入当前光标/选区。
    ///
    /// 有明确选区时可用 AX 直插优先；无明确选区的光标注入场景应走 IME 更安全的合成粘贴优先。
    fn inject_at_cursor(
        &self,
        text: String,
    ) -> impl std::future::Future<Output = Result<(), TextInjectionError>> + Send;

    /// 放入剪贴板（用户手动粘贴兜底）。
    ///
    /// # Errors
    /// 平台剪贴板写入失败时返回错误；当前 macOS 实现为同步 API，正常总是成功。
    fn place_in_clipboard(&self, text: &str) -> Result<(), TextInjectionError>;
}

/// 键盘观测上报的语义事件（只听不拦，TDD-05 §1.2）。
///
/// 弹窗翻译复用**同一个** `CGEventTap` 观测两类键：双击复制（唤起）与 `Escape`（关闭面板）。
/// `Escape` 走 tap 而非全局热键——非激活面板不取键焦点、webview 收不到 ESC keydown，而全局
/// 热键动态注册会阻塞主线程 / 与插件锁重入死锁（已实测）。tap 是 ListenOnly：ESC 既关面板又
/// 照常传给前台 App，不吞系统 ESC。
pub enum TapEvent {
    /// 某条双击手势绑定在时间窗内命中两次（不拦截原键；`u32` = [`GestureBinding::id`]）。
    DoubleTap(u32),
    /// `Escape` 按下（用于关闭已显示的弹窗面板）。
    Escape,
}

/// 键盘观测回调：命中关注的键时以语义事件调用。
pub type TapCallback = Box<dyn Fn(TapEvent) + Send + 'static>;

/// 跨平台键盘观测（TDD-05 §1.2）。macOS = `CGEventTap`（只听不拦）；Windows = `WH_KEYBOARD_LL`
/// （[`windows::keytap`]）。
pub trait KeyTap: Send + Sync {
    /// 启动观测；命中 [`TapEvent`] 时调 `on_event`。返回的句柄存活期间持续观测。
    ///
    /// # Errors
    /// 无法创建底层事件钩子（如未授予辅助功能权限）时返回错误。
    fn start(&self, on_event: TapCallback) -> Result<TapHandle, KeyTapError>;
}

/// 观测句柄：drop 即停止观测（平台实现把"停掉自己的 run loop"交进来）。
///
/// 可停止是**重建**的前提：辅助功能权限在运行期被授予或撤销后，旧 tap 不再收事件，必须
/// 停掉再建一个；不可停的句柄会让两条 tap 线程同时活着、同一次按键回调两遍。
pub struct TapHandle {
    stop: Option<Box<dyn FnOnce() + Send>>,
}

impl TapHandle {
    // macOS 的 CGEventTap 与 Windows 的低层键盘钩子建它。
    #[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
    pub(crate) fn new(stop: impl FnOnce() + Send + 'static) -> Self {
        Self {
            stop: Some(Box::new(stop)),
        }
    }
}

impl Drop for TapHandle {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            stop();
        }
    }
}

/// 键盘观测错误。
#[derive(Debug, thiserror::Error)]
pub enum KeyTapError {
    /// 创建底层事件钩子失败（多因未授予辅助功能权限）。
    #[error("could not create the keyboard event tap (accessibility permission may be missing)")]
    TapCreationFailed,
}

/// **双击判定状态机（纯逻辑，零平台依赖，可单测，TDD-05 §8）**。
///
/// 输入为单调递增的毫秒时间戳（由平台层从底层事件时间或本地时钟提供）。算法：相邻两次
/// `Cmd+C` 间隔 ≤ 窗口 → 判定双击并**复位**（避免第三次快速按键与第二次再次成对）。
#[derive(Debug)]
pub struct DoubleTapDetector {
    last_ms: Option<u64>,
}

impl DoubleTapDetector {
    /// 构造（时间窗改为逐次传入，支持设置热更新而无需重建状态机）。
    #[must_use]
    pub fn new() -> Self {
        Self { last_ms: None }
    }

    /// 记录一次触发键按下；返回 `true` 表示本次**完成了一次双击**。
    ///
    /// `window_ms` 为本次判定采用的时间窗（来自 [`PopupGestureConfig`]，可运行期变化）。
    /// 命中后复位，使"第三次快速按键"成为新一轮的"第一次"，不与上一次结果重复成对。
    pub fn record(&mut self, now_ms: u64, window_ms: u64) -> bool {
        match self.last_ms {
            Some(prev) if now_ms.saturating_sub(prev) <= window_ms => {
                self.last_ms = None;
                true
            }
            _ => {
                self.last_ms = Some(now_ms);
                false
            }
        }
    }
}

impl Default for DoubleTapDetector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 电量低（ADR-156 ③）：低电量模式，或用电池且低于 20%；插着电、读不到电池都不算。
    #[test]
    fn battery_is_low_only_on_battery_below_the_threshold_or_in_low_power_mode() {
        assert!(battery_is_low(Some((true, 19)), false));
        assert!(!battery_is_low(Some((true, 20)), false));
        assert!(!battery_is_low(Some((false, 5)), false), "插着电不算");
        assert!(!battery_is_low(None, false), "台式机没有电池");
        assert!(battery_is_low(None, true), "低电量模式");
    }

    const WIN: u64 = 400;

    #[test]
    fn two_taps_within_window_is_double() {
        let mut d = DoubleTapDetector::new();
        assert!(!d.record(0, WIN), "首次不算双击");
        assert!(d.record(300, WIN), "300ms 内第二次 → 双击");
    }

    #[test]
    fn two_taps_outside_window_is_not_double() {
        let mut d = DoubleTapDetector::new();
        assert!(!d.record(0, WIN));
        assert!(!d.record(500, WIN), "超窗 → 不算双击，仅记为新的首次");
    }

    #[test]
    fn single_tap_never_doubles() {
        let mut d = DoubleTapDetector::new();
        assert!(!d.record(1000, WIN));
    }

    #[test]
    fn boundary_exactly_window_counts() {
        let mut d = DoubleTapDetector::new();
        assert!(!d.record(0, WIN));
        assert!(d.record(400, WIN), "恰好等于窗口边界 → 计为双击（≤）");
    }

    #[test]
    fn third_rapid_tap_starts_new_pair_not_triple() {
        let mut d = DoubleTapDetector::new();
        assert!(!d.record(0, WIN));
        assert!(d.record(100, WIN), "1+2 成双");
        assert!(
            !d.record(150, WIN),
            "第三次复位后为新首次，不与第二次再成对"
        );
        assert!(d.record(200, WIN), "3+4 再成双");
    }

    #[test]
    fn slow_then_fast() {
        let mut d = DoubleTapDetector::new();
        assert!(!d.record(0, WIN));
        assert!(!d.record(1000, WIN), "慢 → 新首次");
        assert!(d.record(1100, WIN), "紧接的一次 → 双击");
    }

    #[test]
    fn window_change_applies_immediately() {
        let mut d = DoubleTapDetector::new();
        assert!(!d.record(0, WIN));
        assert!(!d.record(500, WIN), "400ms 窗下 500ms 不成双");
        assert!(
            d.record(900, 600),
            "改 600ms 窗后 400ms 间隔成双（热更新语义）"
        );
    }

    // —— 双击手势解析（PRD-09 §2.6 双击键）——

    #[test]
    fn gesture_for_default_spec() {
        let g = DoubleTapGesture::for_setting_value("double_tap:cmd_or_ctrl+c").unwrap();
        assert_eq!(
            g,
            DoubleTapGesture {
                keycode: platform_keycode_for_key("c").unwrap(), // kVK_ANSI_C / 'C'
                cmd: true,
                shift: false,
                alt: false,
            }
        );
    }

    #[test]
    fn gesture_supports_multi_modifiers_and_named_keys() {
        let g = DoubleTapGesture::for_setting_value("double_tap:cmd_or_ctrl+shift+1").unwrap();
        assert!(g.cmd && g.shift && !g.alt);
        assert_eq!(g.keycode, platform_keycode_for_key("1").unwrap());
        // 具名键（文字速记默认键切成双击模式）。
        let space =
            DoubleTapGesture::for_setting_value("double_tap:cmd_or_ctrl+shift+space").unwrap();
        assert_eq!(space.keycode, platform_keycode_for_key("space").unwrap());
    }

    #[test]
    fn gesture_rejects_plain_and_invalid_values() {
        assert!(
            DoubleTapGesture::for_setting_value("cmd_or_ctrl+c").is_none(),
            "普通组合归热键插件"
        );
        assert!(DoubleTapGesture::for_setting_value("double_tap:cmd_or_ctrl").is_none());
        assert!(DoubleTapGesture::for_setting_value("double_tap:c+v").is_none());
        assert!(DoubleTapGesture::for_setting_value("").is_none());
    }

    /// 词表里的每个名字都有键码：校验放行的双击值，手势表一定建得出来。
    #[test]
    fn every_main_key_has_a_macos_keycode() {
        let missing: Vec<&str> = luna_config::shortcut::MAIN_KEYS
            .iter()
            .copied()
            .filter(|name| macos_keycode_for_key(name).is_none())
            .collect();
        assert_eq!(missing, Vec::<&str>::new(), "这些主键无法作双击手势");
        let mut codes: Vec<u16> = luna_config::shortcut::MAIN_KEYS
            .iter()
            .filter_map(|name| macos_keycode_for_key(name))
            .collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(
            codes.len(),
            luna_config::shortcut::MAIN_KEYS.len(),
            "键码有重复"
        );
    }

    #[test]
    fn double_tap_config_matches_exact_modifiers_only() {
        let c = i64::from(platform_keycode_for_key("c").unwrap());
        let v = i64::from(platform_keycode_for_key("v").unwrap());
        let config = DoubleTapConfig::default();
        assert_eq!(config.window_ms(), 400);
        assert_eq!(
            config.match_key(c, true, false, false, false),
            None,
            "空表无匹配"
        );

        let popup = DoubleTapGesture::for_setting_value("double_tap:cmd_or_ctrl+c").unwrap();
        let voice = DoubleTapGesture::for_setting_value("double_tap:cmd_or_ctrl+shift+v").unwrap();
        config.set_bindings(vec![
            GestureBinding {
                gesture: popup,
                id: 0,
            },
            GestureBinding {
                gesture: voice,
                id: 2,
            },
        ]);

        assert_eq!(
            config.match_key(c, true, false, false, false),
            Some(0),
            "⌘C → popup"
        );
        assert_eq!(
            config.match_key(v, true, true, false, false),
            Some(2),
            "⌘⇧V → voice"
        );
        assert_eq!(
            config.match_key(c, true, true, false, false),
            None,
            "⌘⇧C ≠ ⌘C（修饰键精确匹配）"
        );
        assert_eq!(
            config.match_key(v, true, false, false, false),
            None,
            "⌘V 无绑定"
        );
        assert_eq!(
            config.match_key(c, true, false, false, true),
            None,
            "按住 Control 的 ⌃⌘C 不是 ⌘C"
        );

        config.set_bindings(Vec::new());
        assert_eq!(
            config.match_key(c, true, false, false, false),
            None,
            "整表替换生效"
        );

        config.set_window_ms(250);
        assert_eq!(config.window_ms(), 250);
        config.set_window_ms(0);
        assert_eq!(config.window_ms(), 400, "非正值回退默认");
    }
}
