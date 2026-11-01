//! Windows 双击手势被动观测：低层键盘钩子 `WH_KEYBOARD_LL`（TDD-05 §1.2，**只听不拦**）。
//!
//! 与 macOS `CGEventTap` 同一套语义，判定写成纯状态机 [`KeyTapMachine`]，在任何平台上单测：
//! 1. **过滤本进程合成的键**：`SendInput` 时 `dwExtraInfo` 打 [`LUNA_SYNTHETIC_EVENT_MAGIC`]，
//!    钩子据此忽略（自污染防护，TDD-05 §1.4）。
//! 2. **忽略自动重复**：低层钩子不标记自动重复，按住一个键时系统连发按下事件；状态机记住
//!    哪些键已经按下，再来一次按下就是自动重复。
//! 3. **夹键作废**：两次触发键之间夹了别的非修饰键，那一对不成立。
//! 4. **修饰键映射**：设置里的 `cmd_or_ctrl` 在 Windows 上是 Ctrl；macOS 那条「按住 Control 不算」
//!    在这里对应「按住 Win 不算」。
//!
//! 钩子线程有自己的消息循环（低层钩子要求安装线程泵消息）；回调里只做查表与投递。
//! 回调超过系统的 `LowLevelHooksTimeout` 会被跳过、多次超时后被系统静默摘掉——这是 Windows
//! 版「用一阵子就失灵」的头号风险，所以回调里绝不加锁等待、不做 IO。
//! 已知边界：系统不把管理员权限窗口里的按键交给非管理员进程的钩子（UIPI），在那些窗口里
//! 双击不会触发；安全桌面（UAC、锁屏）同理。

use super::keys;
use crate::platform::{DoubleTapConfig, DoubleTapDetector};
use std::collections::HashMap;

/// 一个低层键盘事件（钩子回调里从 `KBDLLHOOKSTRUCT` 取出）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LlKeyEvent {
    pub vk: u16,
    pub down: bool,
    /// `dwExtraInfo` 等于 Lunarca 的合成指纹。
    pub synthetic_from_us: bool,
}

/// 状态机对一个事件的结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TapOutput {
    /// 按下了 Escape（关闭弹窗；照常传给前台应用）。
    pub escape: bool,
    /// 命中的双击手势绑定 id。
    pub double_tap: Option<u32>,
}

/// 低层钩子的纯状态机：按下集合（判自动重复、修饰键）+ 每条绑定一个双击检测器。
#[derive(Debug, Default)]
pub struct KeyTapMachine {
    held: [u64; 4],
    detectors: HashMap<u32, DoubleTapDetector>,
}

impl KeyTapMachine {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn is_held(&self, vk: u16) -> bool {
        let vk = usize::from(vk & 0xFF);
        self.held[vk / 64] & (1 << (vk % 64)) != 0
    }

    fn set_held(&mut self, vk: u16, held: bool) {
        let vk = usize::from(vk & 0xFF);
        if held {
            self.held[vk / 64] |= 1 << (vk % 64);
        } else {
            self.held[vk / 64] &= !(1 << (vk % 64));
        }
    }

    fn any_held(&self, a: u16, b: u16) -> bool {
        self.is_held(a) || self.is_held(b)
    }

    /// 喂一个事件。`now_ms` 单调递增；时间窗与绑定表来自共享的 [`DoubleTapConfig`]。
    pub fn on_event(
        &mut self,
        event: LlKeyEvent,
        now_ms: u64,
        config: &DoubleTapConfig,
    ) -> TapOutput {
        // 1. 自己合成的键：既不计入按下集合，也不喂检测器（物理键盘状态不受它影响）。
        if event.synthetic_from_us {
            return TapOutput::default();
        }
        if !event.down {
            self.set_held(event.vk, false);
            return TapOutput::default();
        }
        // 2. 已经按着又来一次按下 = 自动重复：不是「再按一次」，也不打断已开始的那一对。
        if self.is_held(event.vk) {
            return TapOutput::default();
        }
        self.set_held(event.vk, true);
        // 修饰键本身的按下不参与（与 macOS 只看 KeyDown、不看 FlagsChanged 一致）。
        if keys::is_modifier_vk(event.vk) {
            return TapOutput::default();
        }

        let mut out = TapOutput {
            escape: event.vk == keys::VK_ESCAPE,
            double_tap: None,
        };
        let matched = config.match_key(
            i64::from(event.vk),
            self.any_held(keys::VK_LCONTROL, keys::VK_RCONTROL),
            self.any_held(keys::VK_LSHIFT, keys::VK_RSHIFT),
            self.any_held(keys::VK_LMENU, keys::VK_RMENU),
            // 4. 「按住 Control 不算」在 Windows 上是「按住 Win 不算」。
            self.any_held(keys::VK_LWIN, keys::VK_RWIN),
        );
        match matched {
            Some(id) => {
                // 3. 其它手势的半程作废：Ctrl+C、Ctrl+V、Ctrl+C 不是一次双击 Ctrl+C。
                self.detectors.retain(|other, _| *other == id);
                if self
                    .detectors
                    .entry(id)
                    .or_default()
                    .record(now_ms, config.window_ms())
                {
                    out.double_tap = Some(id);
                }
            }
            None => self.detectors.clear(),
        }
        out
    }
}

#[cfg(target_os = "windows")]
pub use imp::LowLevelKeyTap;

#[cfg(target_os = "windows")]
mod imp {
    use super::{KeyTapMachine, LlKeyEvent};
    use crate::platform::{
        DoubleTapConfig, KeyTap, KeyTapError, TapCallback, TapEvent, TapHandle,
        LUNA_SYNTHETIC_EVENT_MAGIC,
    };
    use std::cell::RefCell;
    use std::sync::{mpsc, Arc};
    use std::time::Instant;
    use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, GetMessageW, PostThreadMessageW, SetWindowsHookExW,
        TranslateMessage, UnhookWindowsHookEx, HC_ACTION, KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL,
        WM_KEYDOWN, WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP,
    };

    /// 钩子线程上的状态。低层钩子回调只在安装它的线程上被调用，故用线程局部量、不加锁。
    struct HookState {
        machine: KeyTapMachine,
        config: Arc<DoubleTapConfig>,
        on_event: TapCallback,
        base: Instant,
    }

    thread_local! {
        static HOOK_STATE: RefCell<Option<HookState>> = const { RefCell::new(None) };
    }

    /// Windows 低层键盘钩子实现。双击绑定表 / 时间窗来自共享的 [`DoubleTapConfig`]（热更新）。
    pub struct LowLevelKeyTap {
        config: Arc<DoubleTapConfig>,
    }

    impl LowLevelKeyTap {
        #[must_use]
        pub fn new(config: Arc<DoubleTapConfig>) -> Self {
            Self { config }
        }
    }

    impl KeyTap for LowLevelKeyTap {
        fn start(&self, on_event: TapCallback) -> Result<TapHandle, KeyTapError> {
            let config = Arc::clone(&self.config);
            let (ready_tx, ready_rx) = mpsc::channel::<Option<u32>>();
            std::thread::Builder::new()
                .name("luna-keytap".into())
                .spawn(move || run_hook_thread(config, on_event, &ready_tx))
                .map_err(|_| KeyTapError::TapCreationFailed)?;
            match ready_rx.recv() {
                Ok(Some(thread_id)) => Ok(TapHandle::new(move || {
                    // SAFETY: 向钩子线程投递 WM_QUIT；线程已退出时调用只是失败，无副作用。
                    unsafe {
                        let _ = PostThreadMessageW(thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
                    }
                })),
                _ => Err(KeyTapError::TapCreationFailed),
            }
        }
    }

    fn run_hook_thread(
        config: Arc<DoubleTapConfig>,
        on_event: TapCallback,
        ready_tx: &mpsc::Sender<Option<u32>>,
    ) {
        HOOK_STATE.with(|cell| {
            *cell.borrow_mut() = Some(HookState {
                machine: KeyTapMachine::new(),
                config,
                on_event,
                base: Instant::now(),
            });
        });
        // SAFETY: 标准的低层钩子安装；模块句柄取本进程，回调为本模块的 extern "system" 函数。
        let hook = unsafe {
            let module = GetModuleHandleW(None).ok();
            SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), module.map(|m| m.into()), 0)
        };
        let Ok(hook) = hook else {
            let _ = ready_tx.send(None);
            return;
        };
        // SAFETY: 取当前线程 id，供句柄投递 WM_QUIT。
        let thread_id = unsafe { GetCurrentThreadId() };
        let _ = ready_tx.send(Some(thread_id));

        let mut msg = MSG::default();
        // SAFETY: 本线程的标准消息循环；GetMessageW 收到 WM_QUIT 返回 0、出错返回 -1，都退出。
        unsafe {
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            let _ = UnhookWindowsHookEx(hook);
        }
        HOOK_STATE.with(|cell| cell.borrow_mut().take());
    }

    unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code == HC_ACTION as i32 {
            // SAFETY: HC_ACTION 时 lparam 指向系统提供的 KBDLLHOOKSTRUCT。
            let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
            let down = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            let up = matches!(wparam.0 as u32, WM_KEYUP | WM_SYSKEYUP);
            if down || up {
                let event = LlKeyEvent {
                    vk: (info.vkCode & 0xFFFF) as u16,
                    down,
                    synthetic_from_us: info.dwExtraInfo == LUNA_SYNTHETIC_EVENT_MAGIC as usize,
                };
                // C 回调不能展开 panic：任何一条路径都不许把 panic 漏出这层。
                let _ = std::panic::catch_unwind(|| deliver(event));
            }
        }
        // 只听不拦：永远交给下一个钩子。
        // SAFETY: 原样转交本次回调参数。
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }

    fn deliver(event: LlKeyEvent) {
        HOOK_STATE.with(|cell| {
            let Ok(mut guard) = cell.try_borrow_mut() else {
                return;
            };
            let Some(state) = guard.as_mut() else {
                return;
            };
            let now_ms = state.base.elapsed().as_millis() as u64;
            let out = state.machine.on_event(event, now_ms, &state.config);
            if out.escape {
                (state.on_event)(TapEvent::Escape);
            }
            if let Some(id) = out.double_tap {
                (state.on_event)(TapEvent::DoubleTap(id));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{DoubleTapGesture, GestureBinding};
    use keys::{VK_C, VK_ESCAPE, VK_LCONTROL, VK_LSHIFT, VK_LWIN, VK_V};

    fn config() -> DoubleTapConfig {
        let config = DoubleTapConfig::default();
        let gesture = DoubleTapGesture {
            keycode: VK_C,
            cmd: true,
            shift: false,
            alt: false,
        };
        config.set_bindings(vec![GestureBinding { gesture, id: 0 }]);
        config
    }

    fn down(vk: u16) -> LlKeyEvent {
        LlKeyEvent {
            vk,
            down: true,
            synthetic_from_us: false,
        }
    }

    fn up(vk: u16) -> LlKeyEvent {
        LlKeyEvent {
            vk,
            down: false,
            synthetic_from_us: false,
        }
    }

    /// 按住 Ctrl，C 点两下。
    fn ctrl_c_twice(m: &mut KeyTapMachine, c: &DoubleTapConfig, gap_ms: u64) -> Option<u32> {
        m.on_event(down(VK_LCONTROL), 0, c);
        m.on_event(down(VK_C), 10, c);
        m.on_event(up(VK_C), 40, c);
        let hit = m.on_event(down(VK_C), 10 + gap_ms, c).double_tap;
        m.on_event(up(VK_C), 40 + gap_ms, c);
        m.on_event(up(VK_LCONTROL), 60 + gap_ms, c);
        hit
    }

    #[test]
    fn double_ctrl_c_within_the_window_fires() {
        let c = config();
        let mut m = KeyTapMachine::new();
        assert_eq!(ctrl_c_twice(&mut m, &c, 200), Some(0));
        let mut slow = KeyTapMachine::new();
        assert_eq!(ctrl_c_twice(&mut slow, &c, 900), None, "超窗不算双击");
    }

    #[test]
    fn holding_ctrl_c_down_is_autorepeat_not_a_double_tap() {
        let c = config();
        let mut m = KeyTapMachine::new();
        m.on_event(down(VK_LCONTROL), 0, &c);
        assert_eq!(m.on_event(down(VK_C), 10, &c).double_tap, None);
        // 系统按 ~30ms 连发按下，中间没有松开。
        for t in [40, 70, 100] {
            assert_eq!(
                m.on_event(down(VK_C), t, &c).double_tap,
                None,
                "{t}ms 的连发"
            );
        }
    }

    #[test]
    fn our_own_synthetic_ctrl_c_never_counts() {
        let c = config();
        let mut m = KeyTapMachine::new();
        m.on_event(down(VK_LCONTROL), 0, &c);
        m.on_event(down(VK_C), 10, &c);
        m.on_event(up(VK_C), 20, &c);
        let synthetic = LlKeyEvent {
            vk: VK_C,
            down: true,
            synthetic_from_us: true,
        };
        assert_eq!(
            m.on_event(synthetic, 100, &c).double_tap,
            None,
            "取选区合成的 Ctrl+C 不能把自己判成双击"
        );
    }

    #[test]
    fn an_intervening_key_cancels_the_pair() {
        let c = config();
        let mut m = KeyTapMachine::new();
        m.on_event(down(VK_LCONTROL), 0, &c);
        m.on_event(down(VK_C), 10, &c);
        m.on_event(up(VK_C), 20, &c);
        m.on_event(down(VK_V), 50, &c);
        m.on_event(up(VK_V), 60, &c);
        assert_eq!(
            m.on_event(down(VK_C), 100, &c).double_tap,
            None,
            "Ctrl+C、Ctrl+V、Ctrl+C 不是一次双击 Ctrl+C"
        );
    }

    #[test]
    fn modifiers_must_match_exactly() {
        let c = config();
        let mut m = KeyTapMachine::new();
        m.on_event(down(VK_LCONTROL), 0, &c);
        m.on_event(down(VK_LSHIFT), 0, &c);
        m.on_event(down(VK_C), 10, &c);
        m.on_event(up(VK_C), 20, &c);
        assert_eq!(
            m.on_event(down(VK_C), 100, &c).double_tap,
            None,
            "Ctrl+Shift+C ≠ Ctrl+C"
        );

        let mut win = KeyTapMachine::new();
        win.on_event(down(VK_LWIN), 0, &c);
        win.on_event(down(VK_LCONTROL), 0, &c);
        win.on_event(down(VK_C), 10, &c);
        win.on_event(up(VK_C), 20, &c);
        assert_eq!(
            win.on_event(down(VK_C), 100, &c).double_tap,
            None,
            "按住 Win 的组合不是 Ctrl+C"
        );
    }

    #[test]
    fn escape_is_reported_and_passes_through() {
        let c = config();
        let mut m = KeyTapMachine::new();
        let out = m.on_event(down(VK_ESCAPE), 0, &c);
        assert!(out.escape);
        assert_eq!(out.double_tap, None);
        assert!(!m.on_event(up(VK_ESCAPE), 10, &c).escape);
    }
}
