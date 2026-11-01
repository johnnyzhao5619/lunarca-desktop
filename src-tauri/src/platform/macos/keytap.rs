//! macOS `CGEventTap` 双击手势被动观测（TDD-05 §1.2，**只听不拦**）。
//!
//! 四条 macOS 必备处理（"线上偶发失灵"的根因都在此）：
//! 1. **超时自动重启用**：tap 处理过慢被系统发 `TapDisabledByTimeout`/`...ByUserInput` 停用，
//!    回调内即 `CGEventTapEnable(true)` 重启（即便 `ListenOnly` 也会发生）。
//! 2. **专用 `CFRunLoop` 线程**：tap 必须 attach 到运行 `CFRunLoopRun()` 的线程，绝不放主线程/
//!    tokio executor；回调只做轻量双击判定 + 投递。停止 = 停掉该 run loop（[`TapHandle`] drop）。
//! 3. **过滤本进程自发事件**：选区捕获/注入合成的 `Cmd+C`/`Cmd+V` 带
//!    [`LUNA_SYNTHETIC_EVENT_MAGIC`] 源 userData，回调首行据此忽略（自污染防护，§1.4）。
//! 4. **忽略键盘自动重复**：按住 ⌘C 不放，系统按 ~30ms 连发 KeyDown，两下之间的间隔远在
//!    时间窗内，不过滤就是"按住复制键会弹出翻译窗"。
//!
//! 运行时行为只能真机验：本文件 headless 仅保证编译。

use crate::platform::{
    DoubleTapConfig, DoubleTapDetector, KeyTap, KeyTapError, TapCallback, TapEvent, TapHandle,
    LUNA_SYNTHETIC_EVENT_MAGIC,
};
use core_foundation::base::TCFType;
use core_foundation::mach_port::CFMachPortRef;
use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop};
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventTapProxy, CGEventType, EventField,
};
use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Instant;

// CoreGraphics：core-graphics 的 `CGEventTap::enable()` 借用 self，无法在其自身回调内调用；
// 故超时重启用走 raw FFI（同框架，已由 core-graphics 链接）。
extern "C" {
    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
}

// 辅助功能信任检查（is_process_trusted/prompt_accessibility_trust）已收口至
// [`crate::platform::macos::permissions`]（单一来源，DRY）——CGEventTap 与权限管理共用。

/// `kVK_Escape`（Escape 键虚拟键码）。
const KEYCODE_ESCAPE: i64 = 53;

/// macOS `CGEventTap` 实现。双击手势绑定表/时间窗来自共享的 [`DoubleTapConfig`]
///（设置驱动，热更新——改设置即生效，无需重启 tap 线程）。
pub struct CGEventTapKeyTap {
    config: Arc<DoubleTapConfig>,
}

impl CGEventTapKeyTap {
    /// 用共享手势配置构造。
    #[must_use]
    pub fn new(config: Arc<DoubleTapConfig>) -> Self {
        Self { config }
    }
}

impl KeyTap for CGEventTapKeyTap {
    fn start(&self, on_event: TapCallback) -> Result<TapHandle, KeyTapError> {
        let config = Arc::clone(&self.config);
        // tap 必须在它将要 run 的线程上创建；用 oneshot 把"创建成功（附该线程的 run loop，
        // 供停止用）/ 失败"回传给调用方。
        let (ready_tx, ready_rx) = mpsc::channel::<Option<CFRunLoop>>();

        thread::Builder::new()
            .name("luna-keytap".into())
            .spawn(move || run_tap_loop(&config, on_event, &ready_tx))
            .map_err(|_| KeyTapError::TapCreationFailed)?;

        match ready_rx.recv() {
            Ok(Some(run_loop)) => Ok(TapHandle::new(move || run_loop.stop())),
            _ => Err(KeyTapError::TapCreationFailed),
        }
    }
}

/// 在专用 `CFRunLoop` 线程内：建 tap → 接 run loop → 启用 → 阻塞 run，直到句柄把 run loop 停掉。
fn run_tap_loop(
    config: &Arc<DoubleTapConfig>,
    on_event: TapCallback,
    ready_tx: &mpsc::Sender<Option<CFRunLoop>>,
) {
    // 每条绑定各自独立的双击状态机（id → 检测器；表小，HashMap 足够）。
    let detectors = RefCell::new(std::collections::HashMap::<u32, DoubleTapDetector>::new());
    let config = Arc::clone(config);
    let base = Instant::now();
    // 回调内需要 tap 的 mach port 以重启用，但 tap 此刻尚未创建 → 先放共享格，建好后填入。
    let port_cell = Arc::new(AtomicUsize::new(0));
    let cb_port = Arc::clone(&port_cell);
    // 诊断开关：打印每个收到的键事件（区分"tap 没收到事件"vs"收到但双击没判定"）。
    let verbose = std::env::var_os("LUNA_KEYTAP_VERBOSE").is_some();

    // C 回调里的 panic 只能 abort（`panic_cannot_unwind`）：进程退出期一次 panic 会把整个
    // 进程杀成 SIGABRT（「重置应用」的重启因此走不完）。任何一条路径（借用 RefCell、往已
    // 关闭的通道发、状态已卸载）都不许把 panic 漏出这层。
    let callback = move |_proxy: CGEventTapProxy, etype: CGEventType, event: &CGEvent| {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            tap_event_body(
                etype, event, &cb_port, verbose, &on_event, &config, &detectors, &base,
            )
        }));
        match outcome {
            Ok(result) => result,
            Err(_) => {
                eprintln!("[keytap] 回调内 panic 已拦下（C 回调不能展开，进程退出期常见）");
                None
            }
        }
    };

    // events_of_interest 只放 KeyDown：core-graphics 0.24 用 `1 << (event_type as u64)` 建
    // mask，而 TapDisabledByTimeout/ByUserInput 的枚举值是 0xFFFFFFFE/0xFFFFFFFF → 放进去会
    // `1 << 巨值` 移位溢出 panic（event.rs:488）。这两个停用事件**无论 mask 是否包含都会自动
    // 投递给回调**（Apple 行为），故回调里仍能收到并重启用，无需（也不能）放进 mask。
    let tap = match CGEventTap::new(
        CGEventTapLocation::HID,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::ListenOnly,
        vec![CGEventType::KeyDown],
        callback,
    ) {
        Ok(t) => t,
        Err(()) => {
            let _ = ready_tx.send(None); // 多因未授予辅助功能权限
            return;
        }
    };

    let port = tap.mach_port.as_concrete_TypeRef();
    port_cell.store(port as usize, Ordering::SeqCst);

    let source = match tap.mach_port.create_runloop_source(0) {
        Ok(s) => s,
        Err(()) => {
            let _ = ready_tx.send(None);
            return;
        }
    };

    let run_loop = CFRunLoop::get_current();
    // SAFETY: 在当前线程的 run loop 上挂入本 tap 的事件源；source 与 tap 同线程存活。
    unsafe { run_loop.add_source(&source, kCFRunLoopCommonModes) };
    tap.enable();

    let _ = ready_tx.send(Some(run_loop));
    // 阻塞本线程跑 run loop；tap 在此线程上持续观测，直到句柄停掉 run loop。
    CFRunLoop::run_current();
    // SAFETY: port 来自本线程创建、此刻仍存活的 tap；停用后再随 `tap` 一起释放。
    unsafe { CGEventTapEnable(port, false) };
}

/// 回调正文（被 `catch_unwind` 包住的那一层）。返回值语义同 tap 回调：`None` = 放行。
#[allow(clippy::too_many_arguments)]
fn tap_event_body(
    etype: CGEventType,
    event: &CGEvent,
    cb_port: &Arc<AtomicUsize>,
    verbose: bool,
    on_event: &TapCallback,
    config: &Arc<DoubleTapConfig>,
    detectors: &RefCell<std::collections::HashMap<u32, DoubleTapDetector>>,
    base: &Instant,
) -> Option<CGEvent> {
    match etype {
        // 1. 超时/被打断停用 → 立即重启用（头号失灵根因）。
        CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput => {
            let port = cb_port.load(Ordering::SeqCst);
            if port != 0 {
                // SAFETY: port 来自本进程创建的 tap 的 CFMachPort，且 tap 生命周期 = 本线程。
                unsafe { CGEventTapEnable(port as CFMachPortRef, true) };
            }
        }
        CGEventType::KeyDown => {
            // 3. 自污染防护：本进程合成的带魔数事件直接忽略（绝不喂双击检测，§1.4）。
            let user_data = event.get_integer_value_field(EventField::EVENT_SOURCE_USER_DATA);
            if user_data == LUNA_SYNTHETIC_EVENT_MAGIC {
                return None;
            }
            // 4. 自动重复不是"再按一次"：既不喂双击检测，也不打断已开始的那一对。
            if event.get_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT) != 0 {
                return None;
            }
            let keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
            let flags = event.get_flags();
            if verbose {
                eprintln!("[keytap] KeyDown keycode={keycode} flags={flags:?}");
            }
            if keycode == KEYCODE_ESCAPE {
                // Escape：关闭已显示的弹窗面板（只听不拦，ESC 照常传给前台 App）。
                on_event(TapEvent::Escape);
            }
            let matched = config.match_key(
                // 设置驱动的双击手势绑定表（任意全局快捷键可绑双击；修饰键精确匹配在表内做）。
                keycode,
                flags.contains(CGEventFlags::CGEventFlagCommand),
                flags.contains(CGEventFlags::CGEventFlagShift),
                flags.contains(CGEventFlags::CGEventFlagAlternate),
                flags.contains(CGEventFlags::CGEventFlagControl),
            );
            let mut detectors = detectors.borrow_mut();
            match matched {
                Some(id) => {
                    // 其它手势的半程一律作废：⌘C、⌘V、⌘C 不是一次双击 ⌘C。
                    detectors.retain(|other, _| *other == id);
                    let now_ms = base.elapsed().as_millis() as u64;
                    if detectors
                        .entry(id)
                        .or_default()
                        .record(now_ms, config.window_ms())
                    {
                        on_event(TapEvent::DoubleTap(id));
                    }
                }
                // 中间夹了任何别的键，已开始的那一对不再成立。
                None => detectors.clear(),
            }
        }
        _ => {}
    }
    // ListenOnly：永远放行，不消费事件（正常复制完全不受影响）。
    None
}
