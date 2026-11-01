//! S15 ScreenCaptureKit 实机宿主窗口排除探针。
//!
//! 仅在调试构建且显式 `LUNA_S15_WINDOW_EXCLUSION_PROBE=1` 时由装配层调用。探针只枚举
//! 窗口 ID，验证可把真实 Lunarca `NSWindow` 映射为 `SCWindow`、构造排除过滤器并完成一次流
//! 启停；不读取像素、不访问窗口标题，也不触发 OCR 或任何持久化。

use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

use block2::RcBlock;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{class, define_class, msg_send, AnyThread, DefinedClass};
use objc2_app_kit::NSWindow;
use objc2_core_graphics::CGImage;
use objc2_core_media::CMSampleBuffer;
use objc2_foundation::{NSArray, NSError};
use objc2_screen_capture_kit::{
    SCContentFilter, SCScreenshotManager, SCShareableContent, SCStream, SCStreamConfiguration,
    SCStreamOutput, SCStreamOutputType,
};

/// 只递增帧计数的 ScreenCaptureKit 输出对象。
///
/// `CMSampleBuffer` 只作为 Objective-C 协议签名接收；实现绝不访问其像素缓冲、附件或时间戳。
struct FrameCounterIvars {
    frames: Arc<AtomicUsize>,
    stopping: AtomicBool,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[ivars = FrameCounterIvars]
    struct S15FrameCounter;

    unsafe impl NSObjectProtocol for S15FrameCounter {}

    unsafe impl SCStreamOutput for S15FrameCounter {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        unsafe fn did_output_sample_buffer(
            &self,
            stream: &SCStream,
            _sample_buffer: &CMSampleBuffer,
            _output_type: SCStreamOutputType,
        ) {
            let previous = self.ivars().frames.fetch_add(1, Ordering::Relaxed);
            if previous == 0
                && self
                    .ivars()
                    .stopping
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
            {
                let frames = self.ivars().frames.clone();
                let stop = RcBlock::new(move |stop_error: *mut NSError| {
                    eprintln!(
                        "[S15] exclusion_filtered_stream_stopped={} error_present={} frame_count={}",
                        stop_error.is_null(),
                        !stop_error.is_null(),
                        frames.load(Ordering::Relaxed),
                    );
                });
                // SAFETY: `stream` 是 ScreenCaptureKit 正在派发给此输出对象的有效引用；本探针
                // 在第一个回调即请求停止，且 completion 仅读取原子计数。
                unsafe {
                    stream.stopCaptureWithCompletionHandler(Some(&stop));
                }
            }
        }
    }
);

impl S15FrameCounter {
    fn new(frames: Arc<AtomicUsize>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(FrameCounterIvars {
            frames,
            stopping: AtomicBool::new(false),
        });
        // SAFETY: `S15FrameCounter` 的 NSObject 超类无额外初始化前置条件。
        unsafe { msg_send![super(this), init] }
    }
}

/// 启动一次真实 Lunarca 窗口的 ScreenCaptureKit 排除过滤器验证。
///
/// `ns_window` 必须来自 Tauri 的 `WebviewWindow::ns_window()`，且调用发生在主线程。
/// ScreenCaptureKit 异步回调只输出窗口 ID/数量和过滤器构造结果，绝不输出用户内容或窗口标题。
pub fn start_host_window_exclusion_probe(ns_window: *mut std::ffi::c_void) -> Result<(), String> {
    if ns_window.is_null() {
        return Err("S15 宿主 NSWindow 指针为空".to_string());
    }

    // SAFETY: 调用方保证指针来自仍存活的 Tauri `NSWindow`，且在主线程调用；`windowNumber`
    // 是 AppKit 公开的只读窗口标识，正是 ScreenCaptureKit `SCWindow.windowID` 的匹配基线。
    let target_window_id = unsafe {
        let window: &NSWindow = &*(ns_window.cast());
        window.windowNumber()
    };
    if target_window_id <= 0 {
        return Err("S15 宿主窗口尚未获得有效 windowNumber".to_string());
    }
    let target_window_id = u32::try_from(target_window_id)
        .map_err(|_| "S15 宿主窗口 ID 超出 CGWindowID 范围".to_string())?;

    let completion = RcBlock::new(
        move |content: *mut SCShareableContent, error: *mut NSError| {
            let outcome: Result<usize, String> = if content.is_null() {
                // 不打印 NSError 的描述，避免未来系统错误信息意外拼入外部窗口数据。
                let has_error = !error.is_null();
                Err(format!(
                    "callback_content_null=true error_present={has_error}"
                ))
            } else {
                // SAFETY: ScreenCaptureKit 在回调期间保证 `content` 和其数组对象有效；本回调只复制
                // `SCWindow`/`SCDisplay` 的 Objective-C 引用用于同步构造过滤器，随后立即释放。
                unsafe {
                    let shareable = &*content;
                    let windows = shareable.windows().to_vec();
                    let window_count = windows.len();
                    let matched = windows
                        .into_iter()
                        .find(|window| window.windowID() == target_window_id);
                    let displays = shareable.displays().to_vec();
                    let display = displays.into_iter().next();
                    match (matched, display) {
                        (Some(matched), Some(display)) => {
                            let excluded =
                                NSArray::from_retained_slice(std::slice::from_ref(&matched));
                            let filter_alloc: Allocated<SCContentFilter> =
                                msg_send![class!(SCContentFilter), alloc];
                            let filter = SCContentFilter::initWithDisplay_excludingWindows(
                                filter_alloc,
                                &display,
                                &excluded,
                            );
                            start_filtered_image_probe(&filter);
                            start_filtered_stream_lifecycle_probe(&filter);
                            Ok(window_count)
                        }
                        (None, _) => Err(format!(
                            "host_window_found=false shareable_window_count={window_count}"
                        )),
                        (_, None) => Err(format!(
                            "host_window_found=true shareable_window_count={window_count} display_count=0"
                        )),
                    }
                }
            };

            match outcome {
                Ok(window_count) => eprintln!(
                    "[S15] host_window_found=true exclusion_filter_constructed=true host_window_id={target_window_id} shareable_window_count={window_count}"
                ),
                Err(summary) => eprintln!(
                    "[S15] host_window_found=false exclusion_filter_constructed=false host_window_id={target_window_id} {summary}"
                ),
            }
        },
    );

    // SAFETY: `RcBlock` 是堆分配且 ScreenCaptureKit 按 Objective-C block 约定保留到异步回调完成；
    // 该 API 只请求可共享窗口元数据，不会开始屏幕流或取图。
    unsafe {
        SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(
            true,
            true,
            &completion,
        );
    }
    Ok(())
}

/// 验证排除过滤器能被实际图像采集 API 接受。
///
/// 只请求 64×64 的图像对象并测试指针是否存在；绝不读取、渲染、打印或落盘任何像素。
unsafe fn start_filtered_image_probe(filter: &SCContentFilter) {
    let configuration = SCStreamConfiguration::new();
    configuration.setWidth(64);
    configuration.setHeight(64);
    configuration.setShowsCursor(false);
    let completion = RcBlock::new(move |image: *mut CGImage, error: *mut NSError| {
        eprintln!(
            "[S15] exclusion_filtered_image_capture_succeeded={} error_present={}",
            !image.is_null(),
            !error.is_null()
        );
    });
    SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(
        filter,
        &configuration,
        Some(&completion),
    );
}

/// 验证排除过滤器可驱动一次 `SCStream` 的完整生命周期。
///
/// 输出对象只原子计数、不访问 `CMSampleBuffer`；在第一个帧回调立即请求停止。这样可验证真实
/// 帧回调与过滤器兼容，但不读取、保存或显示任何用户像素。
unsafe fn start_filtered_stream_lifecycle_probe(filter: &SCContentFilter) {
    let configuration = SCStreamConfiguration::new();
    configuration.setWidth(64);
    configuration.setHeight(64);
    configuration.setShowsCursor(false);
    configuration.setQueueDepth(1);

    let stream_alloc: Allocated<SCStream> = msg_send![class!(SCStream), alloc];
    let stream =
        SCStream::initWithFilter_configuration_delegate(stream_alloc, filter, &configuration, None);
    let frames = Arc::new(AtomicUsize::new(0));
    let output = S15FrameCounter::new(frames.clone());
    let output_protocol: &ProtocolObject<dyn SCStreamOutput> = ProtocolObject::from_ref(&*output);
    if stream
        .addStreamOutput_type_sampleHandlerQueue_error(
            output_protocol,
            SCStreamOutputType::Screen,
            None,
        )
        .is_err()
    {
        eprintln!("[S15] exclusion_filtered_stream_output_added=false error_present=true");
        return;
    }
    let stream_for_start = stream.clone();
    let start = RcBlock::new(move |error: *mut NSError| {
        let _keep_stream_alive = &stream_for_start;
        if !error.is_null() {
            eprintln!("[S15] exclusion_filtered_stream_started=false error_present=true");
            return;
        }
        eprintln!("[S15] exclusion_filtered_stream_started=true");
    });

    // SAFETY: output 已在启动前注册，且 ScreenCaptureKit 按 API 约定保留输出与 start completion；
    // 一旦收到首帧，输出对象用回调中的有效 stream 引用请求停止。
    unsafe {
        stream.startCaptureWithCompletionHandler(Some(&start));
    }
}
