//! macOS 原生文本朗读（TTS）后端：`AVSpeechSynthesizer`（TDD-05 §9）。
//!
//! 选型与风格（对齐 `system_audio_tap.rs` / `voice_process_input.rs` 的手写最小 FFI 面）：
//! - `AVSpeechSynthesizer` 是 macOS 10.14+ **稳定 ObjC 类**，经 `objc2::class!` + `msg_send`
//!   直连，**零新增依赖 crate**（不引 `objc2-avf-audio` typed binding）。
//! - `#[link(name = "AVFoundation")]` 空 `extern` 块强制加载 AVFoundation 伞框架（含 AVFAudio 的
//!   语音合成），使运行时能按类名解析 `AVSpeechSynthesizer` / `AVSpeechUtterance`。
//! - delegate 用 `objc2::define_class!` 定义（与 `screen_capture_probe.rs` 同范式），把
//!   `didStart/didFinish/didCancel/didPause/didContinue` 回调转发给注入的 Rust 闭包。
//!
//! **线程契约（关键）**：`AVSpeechSynthesizer` 及其回放对象**非 `Send`、主线程亲和**——本模块所有
//! 函数**必须在主线程调用**（由 [`crate::tts`] 控制器经 Tauri `run_on_main_thread` 编排）。合成器
//! 常驻主线程 `thread_local`，跨表面/跨会话复用同一个（单一发声者，天然互斥抢占）。

#![allow(non_snake_case)]

use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol};
use objc2::{class, define_class, msg_send, AnyThread, DefinedClass};
use objc2_foundation::NSString;

/// **当前期望回调的发声单元指针**（只做相等比较，**绝不解引用**）。
///
/// 这是区分「本次单元读完」与「上一会话遗留回调」的**唯一可靠令牌**：`speakUtterance:` 是
/// 入队不抢占，而 `stop` / 新朗读替换旧朗读时，旧单元的 `didFinish` 可能**已经投递进主队列**
/// ——仅靠控制器侧的会话标志挡不住它（`run_on_main_thread` 在主线程是内联执行，闭包会先于
/// 队列中的陈旧回调跑完）。不丢弃它就会推进错会话：句索引与实际发声错位一句、提前收尾。
/// 用 `AtomicUsize` 而非 `thread_local`：delegate 回调线程未由 Apple 文档保证。
static EXPECTED_UTTERANCE: AtomicUsize = AtomicUsize::new(0);

// 强制链接 AVFoundation 伞框架（含语音合成），使 `class!(AVSpeechSynthesizer)` 运行时可解析。
#[link(name = "AVFoundation", kind = "framework")]
extern "C" {}

/// `AVSpeechBoundaryImmediate`（立即在当前词边界暂停/停止，不等整句读完）。
const BOUNDARY_IMMEDIATE: i64 = 0;

/// delegate 转发给控制器的语音会话事件（不含任何正文，脱敏）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeechDelegateEvent {
    /// 当前发声单元开始播报。
    DidStart,
    /// 当前发声单元自然读完（控制器据此推进到下一句或收尾）。
    DidFinish,
    /// 当前发声单元被取消（`stop` 触发；控制器忽略，停止态已由 `stop` 落定）。
    DidCancel,
    /// 已暂停。
    DidPause,
    /// 已继续。
    DidContinue,
}

/// delegate 回调闭包（主线程调用；捕获 `Arc<TtsShared>` 等，故要求 `Send`）。
type DelegateCallback = Box<dyn Fn(SpeechDelegateEvent) + Send + 'static>;

struct DelegateIvars {
    callback: DelegateCallback,
}

define_class!(
    // AVSpeechSynthesizer.delegate 是 `weak`：合成器不持有，故由 `MacSpeaker` 强引用保活。
    #[unsafe(super(NSObject))]
    #[ivars = DelegateIvars]
    struct LunaSpeechDelegate;

    unsafe impl NSObjectProtocol for LunaSpeechDelegate {}

    // `AVSpeechSynthesizerDelegate` 是**正式协议但方法全为 optional**：ObjC 运行时按
    // `respondsToSelector:` 派发，故注册同名方法即可被合成器回调，无需在 Rust 侧形式化 adopt。
    impl LunaSpeechDelegate {
        #[unsafe(method(speechSynthesizer:didStartSpeechUtterance:))]
        fn did_start(&self, _synth: *mut AnyObject, _utterance: *mut AnyObject) {
            (self.ivars().callback)(SpeechDelegateEvent::DidStart);
        }

        #[unsafe(method(speechSynthesizer:didFinishSpeechUtterance:))]
        fn did_finish(&self, _synth: *mut AnyObject, utterance: *mut AnyObject) {
            // 身份校验：只有「当前期望的那个发声单元」读完才推进会话。被 stop / 新朗读替换掉的
            // 旧单元其回调可能已在队列中，放行即错位一句（详见 EXPECTED_UTTERANCE 注释）。
            let expected = EXPECTED_UTTERANCE.load(Ordering::Acquire);
            if expected == 0 || utterance as usize != expected {
                return;
            }
            (self.ivars().callback)(SpeechDelegateEvent::DidFinish);
        }

        #[unsafe(method(speechSynthesizer:didCancelSpeechUtterance:))]
        fn did_cancel(&self, _synth: *mut AnyObject, _utterance: *mut AnyObject) {
            (self.ivars().callback)(SpeechDelegateEvent::DidCancel);
        }

        #[unsafe(method(speechSynthesizer:didPauseSpeechUtterance:))]
        fn did_pause(&self, _synth: *mut AnyObject, _utterance: *mut AnyObject) {
            (self.ivars().callback)(SpeechDelegateEvent::DidPause);
        }

        #[unsafe(method(speechSynthesizer:didContinueSpeechUtterance:))]
        fn did_continue(&self, _synth: *mut AnyObject, _utterance: *mut AnyObject) {
            (self.ivars().callback)(SpeechDelegateEvent::DidContinue);
        }
    }
);

impl LunaSpeechDelegate {
    fn new(callback: DelegateCallback) -> Retained<Self> {
        let this = Self::alloc().set_ivars(DelegateIvars { callback });
        // SAFETY: NSObject 超类无额外初始化前置条件。
        unsafe { msg_send![super(this), init] }
    }
}

/// 主线程常驻的合成器 + delegate（非 `Send`，仅经 `thread_local` 在主线程访问）。
struct MacSpeaker {
    synth: Retained<AnyObject>,
    // delegate 由本结构强引用保活（合成器 `weak` 引用）；字段虽不再直接读取，但不可丢弃。
    _delegate: Retained<LunaSpeechDelegate>,
}

impl MacSpeaker {
    fn new(callback: DelegateCallback) -> Self {
        let delegate = LunaSpeechDelegate::new(callback);
        // SAFETY: `AVSpeechSynthesizer` 为稳定 ObjC 类；alloc+init 无前置条件；setDelegate 存 weak 引用。
        let synth: Retained<AnyObject> = unsafe {
            let alloc: Allocated<AnyObject> = msg_send![class!(AVSpeechSynthesizer), alloc];
            let synth: Retained<AnyObject> = msg_send![alloc, init];
            let _: () = msg_send![&*synth, setDelegate: &*delegate];
            synth
        };
        Self {
            synth,
            _delegate: delegate,
        }
    }

    fn speak(&self, text: &str, lang: Option<&str>, rate: f32) {
        let ns_text = NSString::from_str(text);
        // SAFETY: 全为稳定 AVFoundation ObjC 接口，参数非空，均在主线程调用。
        unsafe {
            let utterance: Retained<AnyObject> =
                msg_send![class!(AVSpeechUtterance), speechUtteranceWithString: &*ns_text];
            let _: () = msg_send![&*utterance, setRate: rate];
            if let Some(code) = lang {
                let ns_lang = NSString::from_str(code);
                let voice: Option<Retained<AnyObject>> =
                    msg_send![class!(AVSpeechSynthesisVoice), voiceWithLanguage: &*ns_lang];
                if let Some(voice) = voice {
                    let _: () = msg_send![&*utterance, setVoice: &*voice];
                }
            }
            // 先登记期望单元再入队：确保回调到达时比对基准已就位。
            EXPECTED_UTTERANCE.store(Retained::as_ptr(&utterance) as usize, Ordering::Release);
            // ⚠ `- (void)speakUtterance:` **返回 void**（不同于 pause/stop/continue 的 BOOL）。
            // 写成 `bool` 会被 objc2 的 debug 期方法签名校验判定不匹配并 **panic**——表现正是
            // 「能开启朗读、但从不发声」（会话已建、状态已广播，真正开播的那步炸在主线程闭包里）。
            let _: () = msg_send![&*self.synth, speakUtterance: &*utterance];
        }
    }

    fn pause(&self) {
        // SAFETY: 稳定接口，主线程调用。
        unsafe {
            let _: bool = msg_send![&*self.synth, pauseSpeakingAtBoundary: BOUNDARY_IMMEDIATE];
        }
    }

    fn resume(&self) {
        // SAFETY: 稳定接口，主线程调用。
        unsafe {
            let _: bool = msg_send![&*self.synth, continueSpeaking];
        }
    }

    fn stop(&self) {
        // 先作废期望单元：此后到达的任何 didFinish（含正在取消的那个）都不再推进会话。
        EXPECTED_UTTERANCE.store(0, Ordering::Release);
        // SAFETY: 稳定接口，主线程调用。
        unsafe {
            let _: bool = msg_send![&*self.synth, stopSpeakingAtBoundary: BOUNDARY_IMMEDIATE];
            // 清可能残留的**暂停闩锁**：合成器处于 paused 时 `speakUtterance:` 只入队不发声，
            // 而 `stopSpeaking` 是否顺带清该状态并无文档保证——若不清，「暂停 A → 去 B 处点朗读」
            // 会得到一个静默的「朗读中」。此刻队列已空，续播是无害幂等操作。
            let _: bool = msg_send![&*self.synth, continueSpeaking];
        }
    }
}

thread_local! {
    /// 主线程常驻合成器。首次 [`ensure`] 时以控制器注入的回调创建，之后复用。
    static SPEAKER: RefCell<Option<MacSpeaker>> = const { RefCell::new(None) };
}

/// 确保合成器已就绪（首次以 `callback` 创建）。**必须在主线程调用。**
///
/// 回调只在首次生效（后续复用同一 delegate/合成器）。控制器的 `Arc<TtsShared>` 是稳定单例，
/// 故一次注入长期有效。
pub fn ensure(callback: DelegateCallback) {
    SPEAKER.with(|cell| {
        if cell.borrow().is_none() {
            *cell.borrow_mut() = Some(MacSpeaker::new(callback));
        }
    });
}

/// 朗读一个发声单元（追加到合成器队列；控制器逐句驱动，不并发）。**必须在主线程调用。**
pub fn speak(text: &str, lang: Option<&str>, rate: f32) {
    SPEAKER.with(|cell| {
        if let Some(speaker) = cell.borrow().as_ref() {
            speaker.speak(text, lang, rate);
        }
    });
}

/// 暂停当前朗读。**必须在主线程调用。**
pub fn pause() {
    SPEAKER.with(|cell| {
        if let Some(speaker) = cell.borrow().as_ref() {
            speaker.pause();
        }
    });
}

/// 继续朗读。**必须在主线程调用。**
pub fn resume() {
    SPEAKER.with(|cell| {
        if let Some(speaker) = cell.borrow().as_ref() {
            speaker.resume();
        }
    });
}

/// 停止朗读并清空队列。**必须在主线程调用。**
pub fn stop() {
    SPEAKER.with(|cell| {
        if let Some(speaker) = cell.borrow().as_ref() {
            speaker.stop();
        }
    });
}

#[cfg(test)]
mod tests {
    use objc2::runtime::AnyClass;
    use objc2::sel;

    /// **ObjC 方法签名一致性**——向真实运行时核对我们声明的返回类型编码。
    ///
    /// 缘由（真机故障）：`speakUtterance:` 返回 **void**，曾被误声明为 `bool`；objc2 在
    /// debug 构建下 `verify_method_signature` 判定不匹配即 **panic**，表现为「朗读能开启、
    /// 却从不发声」——UI 状态已广播，真正开播那步炸在主线程闭包里，日志不显眼极难定位。
    /// 手写 FFI 没有编译期类型保障，故用本测试把签名钉死：改错即红，且指出正确编码。
    #[test]
    fn objc_method_signatures_match_declarations() {
        // 编码：`v` = void，`B` = BOOL，`@` = 对象。
        let cases: &[(&str, &[(&str, &str)])] = &[
            (
                "AVSpeechSynthesizer",
                &[
                    ("speakUtterance:", "v"),
                    ("pauseSpeakingAtBoundary:", "B"),
                    ("stopSpeakingAtBoundary:", "B"),
                    ("continueSpeaking", "B"),
                    ("setDelegate:", "v"),
                ],
            ),
            (
                "AVSpeechUtterance",
                &[("setRate:", "v"), ("setVoice:", "v")],
            ),
        ];

        for (class_name, methods) in cases {
            let name = std::ffi::CString::new(*class_name).expect("class name");
            let Some(class) = AnyClass::get(&name) else {
                // 未链接到 AVFoundation 或系统过老：跳过而非误报（朗读本就整体不可用）。
                eprintln!("[TTS] 测试跳过：运行时无 {class_name} 类");
                continue;
            };
            for (selector, expected) in *methods {
                let sel = match *selector {
                    "speakUtterance:" => sel!(speakUtterance:),
                    "pauseSpeakingAtBoundary:" => sel!(pauseSpeakingAtBoundary:),
                    "stopSpeakingAtBoundary:" => sel!(stopSpeakingAtBoundary:),
                    "continueSpeaking" => sel!(continueSpeaking),
                    "setDelegate:" => sel!(setDelegate:),
                    "setRate:" => sel!(setRate:),
                    "setVoice:" => sel!(setVoice:),
                    other => panic!("测试用例未映射 selector：{other}"),
                };
                let method = class
                    .instance_method(sel)
                    .unwrap_or_else(|| panic!("{class_name} 无方法 {selector}"));
                let actual = method.return_type();
                let actual = actual.to_str().expect("返回类型编码非 UTF-8");
                assert_eq!(
                    actual, *expected,
                    "{class_name}.{selector} 返回类型编码应为 {expected}，运行时实际为 {actual}\
                     ——声明与之不符会被 objc2 debug 校验 panic（表现为「能开启却不发声」）"
                );
            }
        }
    }
}
