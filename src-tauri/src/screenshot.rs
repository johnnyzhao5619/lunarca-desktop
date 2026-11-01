//! 截屏翻译编排（PRD-01-007 / TDD-05 §1.5）：Snipaste 式自绘框选层 → 定点截屏 →
//! Vision OCR → 弹窗翻译会话 → 自动生成译文。
//!
//! **第一性**：截屏只是弹窗翻译的又一**输入源**（与选区/手动/语音同级）——识别出文字后写入
//! 同一 `PopupSession` 展示模式并复用既有「显示即自动生成」路径（ADR-003 业务在 Rust，
//! ADR-001 生成走 JobQueue 单一主干），不另起翻译栈。
//!
//! **框选层（对标 Snipaste）**：预热的全屏透明非激活面板池（**每个显示器一面板**，标签
//! `screenshot-overlay-<n>`，不夺前台焦点）自绘暗化蒙层/十字线/选区/尺寸徽标/提示条；拖选
//! 限定单屏（macOS ⇧⌘4 惯例），松开鼠标即确认（`screenshot_overlay_confirm`，坐标按发起
//! 面板所在屏换算，混合 DPI 正确），右键/Esc 取消（`screenshot_overlay_cancel` /
//! CGEventTap Escape）。确认后隐藏全部面板再用 `screencapture -R` 定点捕获，
//! **框选 UI 自身绝不入图**。
//!
//! **隐私**：截图落在进程私有临时目录、OCR 完成后**立即删除**，识别全程本地 Vision，不外发、
//! 不入日志（TDD-12）。
//!
//! ⚠ 真机验收项：框选层交互（拖选/取消/多屏就位）、屏幕录制 TCC 弹框/引导、OCR→弹窗端到端。

use std::path::PathBuf;
use std::sync::Mutex;

/// 截屏后的编排决策（纯逻辑，可单测）：OCR 文本 → 下一步动作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OcrNextStep {
    /// 识别出文字 → 写入弹窗会话展示模式并显示（自动生成第一槽）。
    ShowPopup(String),
    /// 未识别到文字 → 通知用户（不开空弹窗，避免"点了没反应/开了个空窗"两难）。
    NotifyEmpty,
}

/// 由 OCR 结果决定下一步（空白/纯空白 → 通知；否则进弹窗）。
#[must_use]
pub fn decide_ocr_next_step(text: String) -> OcrNextStep {
    if text.trim().is_empty() {
        OcrNextStep::NotifyEmpty
    } else {
        OcrNextStep::ShowPopup(text)
    }
}

/// 前端上报的框选区域（框选层窗口内 CSS 逻辑像素坐标）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SelectionRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// 低于该边长（逻辑像素）的"选区"按误触忽略（单击/极小抖动不产出 1×1 无效截图）。
const MIN_SELECTION_SIZE: f64 = 4.0;

/// 选区是否小到应视为误触（纯逻辑，可单测）。
#[must_use]
pub fn selection_too_small(rect: &SelectionRect) -> bool {
    rect.width < MIN_SELECTION_SIZE || rect.height < MIN_SELECTION_SIZE
}

/// 采集矩形与显示器边界的容差（全局逻辑点）。物理像素 ÷ 该屏 scale 在非整数缩放下会留下
/// 亚点级误差，1 点容差让"恰好铺满一屏"的取景框不被误判为跨屏，又远小于任何真实跨屏溢出。
const DISPLAY_BOUNDS_TOLERANCE_POINTS: f64 = 1.0;

/// 采集矩形是否完整落在某一块显示器内（纯逻辑，可单测）。
///
/// `screencapture -R` 只接受**单一**全局点矩形，而跨屏矩形的两半来自不同 scale 的后备缓冲，
/// 无法用一个矩形如实表达。故越界即拒绝并请用户把取景框移回单屏，绝不产出错位或半屏截图。
#[must_use]
pub fn rect_within_single_display(rect: &SelectionRect, displays: &[SelectionRect]) -> bool {
    displays.iter().any(|display| {
        rect.x + DISPLAY_BOUNDS_TOLERANCE_POINTS >= display.x
            && rect.y + DISPLAY_BOUNDS_TOLERANCE_POINTS >= display.y
            && rect.x + rect.width <= display.x + display.width + DISPLAY_BOUNDS_TOLERANCE_POINTS
            && rect.y + rect.height <= display.y + display.height + DISPLAY_BOUNDS_TOLERANCE_POINTS
    })
}

/// 把窗口内 CSS 选区换算为 `screencapture -R` 的**全局逻辑点**矩形（纯逻辑，可单测）。
///
/// macOS 把窗口按"点"排布：窗口物理原点 / 该屏 scale = 窗口点原点；webview 内 CSS 像素
/// 即逻辑点，直接相加。混合 DPI 多屏下各屏各自换算（框选层恰铺满单屏，故用其所在屏 scale）。
#[must_use]
pub fn selection_to_global_points(
    rect: &SelectionRect,
    window_physical_origin: (f64, f64),
    scale: f64,
) -> SelectionRect {
    let scale = if scale > 0.0 { scale } else { 1.0 };
    SelectionRect {
        x: window_physical_origin.0 / scale + rect.x,
        y: window_physical_origin.1 / scale + rect.y,
        width: rect.width,
        height: rect.height,
    }
}

/// 框选会话状态（`manage` 进 Tauri state）：记录"唤起时弹窗是否可见"，取消/无结果时原样恢复。
/// `None` = 当前无进行中的框选会话（confirm/cancel 命令据此幂等）。
#[derive(Debug, Default)]
// 截屏框选只在 macOS 实现；状态与用例全平台在。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub struct OverlayState(Mutex<Option<OverlaySession>>);

#[derive(Debug, Clone, Copy)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
struct OverlaySession {
    restore_popup: bool,
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
impl OverlayState {
    fn begin(&self, restore_popup: bool) {
        if let Ok(mut guard) = self.0.lock() {
            *guard = Some(OverlaySession { restore_popup });
        }
    }

    /// 结束会话并取出"是否恢复弹窗"；无进行中会话返回 `None`（重复 confirm/cancel 幂等）。
    fn take(&self) -> Option<bool> {
        self.0
            .lock()
            .ok()
            .and_then(|mut guard| guard.take())
            .map(|session| session.restore_popup)
    }
}

/// 本次截屏的私有临时文件路径（进程 id + 计数防碰撞；macOS `TMPDIR` 为每用户 0700 私有目录）。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn capture_file_path() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "luna-screenshot-{}-{}.png",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ))
}

/// 框选层隐藏 → 定点截屏之间的等待：给合成器一帧余量把覆盖层撤下屏幕，避免蒙层入图。
#[cfg(target_os = "macos")]
const OVERLAY_TEARDOWN_MS: u64 = 120;

/// 定点截屏（`screencapture -R`，全局逻辑点矩形；无快门声、无交互）。
///
/// # Errors
/// `screencapture` 启动失败或未产出文件时返回错误（不含用户内容）。
#[cfg(target_os = "macos")]
async fn capture_region(rect: &SelectionRect) -> Result<PathBuf, String> {
    let path = capture_file_path();
    let region = format!("{},{},{},{}", rect.x, rect.y, rect.width, rect.height);
    let status = tokio::process::Command::new("/usr/sbin/screencapture")
        .arg("-x") // 无快门声
        .arg(format!("-R{region}"))
        .arg("-t")
        .arg("png")
        .arg(&path)
        .status()
        .await
        .map_err(|e| format!("无法启动 screencapture：{e}"))?;
    let produced = tokio::fs::metadata(&path)
        .await
        .map(|m| m.len() > 0)
        .unwrap_or(false);
    if produced {
        Ok(path)
    } else {
        Err(format!("screencapture 未产出文件（status={status}）"))
    }
}

/// 对截图文件跑 OCR 并**确保删除临时文件**（成功/失败都删，隐私优先）。
///
/// OCR 经 **JobQueue 单一 AI 主干**（ADR-001）：提交 `JobKind::Ocr`，按 `Capability::Ocr`
/// 路由到 `VisionOcrProvider`（`crate::ocr_provider`）。临时文件生命周期归本函数（截屏方持有），
/// Provider 只按路径识别、不负责删除。
#[cfg(target_os = "macos")]
async fn ocr_and_cleanup(
    app: &tauri::AppHandle,
    path: &std::path::Path,
    desktop_lens_session: bool,
) -> Result<String, String> {
    let result = run_ocr_job(app, path, desktop_lens_session).await;
    let _ = tokio::fs::remove_file(path).await;
    result
}

/// 提交一次 OCR Job 到 JobQueue 并 drain 出识别文本（成功 → `Ok(text)`；失败/取消 → `Err`）。
#[cfg(target_os = "macos")]
async fn run_ocr_job(
    app: &tauri::AppHandle,
    path: &std::path::Path,
    desktop_lens_session: bool,
) -> Result<String, String> {
    use luna_core::{ImageRef, Job, JobEvent, JobKind, JobOrigin, ResultPayload};
    use luna_jobqueue::JobSink;
    use tauri::Manager as _;

    let state = app
        .try_state::<crate::ipc::AppState>()
        .ok_or_else(|| "AppState 不可用".to_string())?;
    let job = Job::new(
        JobKind::Ocr {
            image: ImageRef::Path(path.to_path_buf()),
        },
        JobOrigin::Screenshot,
    );
    let mut handle = state.queue_handle().submit(job);
    let lens_ocr_generation = desktop_lens_session
        .then(|| state.desktop_lens_attach_ocr(handle.cancel_token()))
        .flatten();
    if desktop_lens_session && lens_ocr_generation.is_none() {
        return Err("译境已关闭".into());
    }
    let result = loop {
        let Some(ev) = handle.recv().await else {
            break Err("OCR Job 通道提前关闭".into());
        };
        match ev {
            JobEvent::Finished(result) => {
                break match result.payload {
                    ResultPayload::Text(text) => Ok(text),
                    _ => Err("OCR 结果非文本".into()),
                };
            }
            JobEvent::Failed(e) => break Err(e.to_string()),
            JobEvent::Cancelled => break Err("OCR 已取消".into()),
            _ => {}
        }
    };
    if let Some(generation) = lens_ocr_generation {
        state.desktop_lens_finish_ocr(generation);
    }
    result
}

#[cfg(target_os = "macos")]
fn notify(app: &tauri::AppHandle, event: crate::notifications::NotificationEvent) {
    use tauri::Manager as _;
    if let Some(center) = app.try_state::<crate::notifications::NotificationCenter>() {
        center.publish(event, app);
    }
}

/// 恢复此前可见的弹窗面板（主线程）。
#[cfg(target_os = "macos")]
fn restore_popup_if_needed(app: &tauri::AppHandle, restore: bool) {
    if !restore {
        return;
    }
    let app_show = app.clone();
    let _ = app.run_on_main_thread(move || crate::show_popup_panel(&app_show));
}

/// 截屏翻译入口（全局快捷键 / 托盘 / 弹窗按钮共用）：权限检查 → 收起弹窗 → 显示框选层。
///
/// 后续由框选层回调收口：确认走 [`confirm_region`]、右键/Esc 走 [`cancel_overlay`]。
#[cfg(target_os = "macos")]
pub async fn run_screenshot_translate(app: tauri::AppHandle) {
    use crate::notifications::NotificationEvent;
    use tauri::Manager as _;

    if !crate::platform::macos::permissions::has_screen_capture_access() {
        // 触发系统 TCC 弹框（每版本仅一次）+ 应用内引导（含降级说明），本次中止。
        let _ = crate::platform::macos::permissions::request_screen_capture_access();
        notify(&app, NotificationEvent::screenshot_permission_missing());
        return;
    }

    // 收起弹窗（若可见）：不清会话，取消后原样回来。
    let popup_was_visible = app
        .get_webview_window("popup-translate")
        .and_then(|win| win.is_visible().ok())
        .unwrap_or(false);

    if let Some(state) = app.try_state::<OverlayState>() {
        state.begin(popup_was_visible);
    }

    let _ = app.clone().run_on_main_thread(move || {
        if popup_was_visible {
            crate::hide_popup_panel(&app);
        }
        if !crate::show_screenshot_overlay(&app) {
            // 框选层不可用：结束会话并恢复弹窗（不留半开状态）。
            use tauri::Manager as _;
            if let Some(state) = app.try_state::<OverlayState>() {
                if let Some(restore) = state.take() {
                    restore_popup_if_needed(&app, restore);
                }
            }
        }
    });
}

/// 框选确认（前端松开鼠标）：隐藏框选层 → 定点截屏 → OCR → 进弹窗 / 通知。
///
/// `window` = 发起确认的那块屏的框选层面板（多屏各有一面板，坐标换算取**它自己**的
/// 几何与 scale，混合 DPI 天然正确）。
#[cfg(target_os = "macos")]
pub async fn confirm_region(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    rect: SelectionRect,
) {
    use crate::notifications::NotificationEvent;
    use tauri::Manager as _;

    if !crate::is_screenshot_overlay_label(window.label()) {
        return; // 只接受框选层面板发起的确认（防其他窗口误调）。
    }

    let Some(restore_popup) = app
        .try_state::<OverlayState>()
        .and_then(|state| state.take())
    else {
        return; // 无进行中会话（重复确认/竞态），幂等忽略。
    };

    // 换算须在隐藏前读取窗口几何（隐藏后 outer_position 仍有效，但就位状态更可靠）。
    let geometry = window
        .outer_position()
        .ok()
        .zip(window.scale_factor().ok())
        .map(|(origin, scale)| ((origin.x as f64, origin.y as f64), scale));

    let app_hide = app.clone();
    let _ = app.run_on_main_thread(move || crate::hide_screenshot_overlay(&app_hide));

    if selection_too_small(&rect) {
        // 误触（前端已过滤，此为后端防线）：视为取消。
        restore_popup_if_needed(&app, restore_popup);
        return;
    }
    let Some((origin, scale)) = geometry else {
        eprintln!("[screenshot] 框选层几何不可读，本次取消");
        restore_popup_if_needed(&app, restore_popup);
        return;
    };
    let global = selection_to_global_points(&rect, origin, scale);

    // 给合成器一帧余量撤下覆盖层，确保蒙层/徽标绝不入图。
    tokio::time::sleep(std::time::Duration::from_millis(OVERLAY_TEARDOWN_MS)).await;

    let text = match capture_region(&global).await {
        Ok(path) => match ocr_and_cleanup(&app, &path, false).await {
            Ok(text) => text,
            Err(e) => {
                eprintln!("[screenshot] {e}");
                notify(&app, NotificationEvent::screenshot_no_text());
                restore_popup_if_needed(&app, restore_popup);
                return;
            }
        },
        Err(e) => {
            eprintln!("[screenshot] 截屏失败：{e}");
            notify(&app, NotificationEvent::screenshot_no_text());
            restore_popup_if_needed(&app, restore_popup);
            return;
        }
    };

    match decide_ocr_next_step(text) {
        OcrNextStep::NotifyEmpty => {
            notify(&app, NotificationEvent::screenshot_no_text());
            restore_popup_if_needed(&app, restore_popup);
        }
        OcrNextStep::ShowPopup(text) => {
            // 脱敏日志：只记长度，绝不记识别正文（TDD-12 §1）。
            eprintln!("[screenshot] OCR 完成 len={}", text.len());
            if let Some(state) = app.try_state::<crate::ipc::AppState>() {
                if let Ok(mut session) = state.popup().lock() {
                    // 展示模式（无选区快照 → "替换原文"自然降级复制），显示即自动生成。
                    session.set_display(text);
                }
            }
            let app_show = app.clone();
            let _ = app.run_on_main_thread(move || crate::show_popup_panel(&app_show));
        }
    }
}

/// 当前各显示器在全局逻辑点坐标系中的边界。换算口径与窗口几何完全一致（物理原点 ÷ 该屏
/// scale），故混合 DPI 下与取景框几何、[`capture_fixed_region`] 收到的矩形处在同一坐标系。
///
/// 采集前的跨屏校验与译境恢复几何的可见性校验共用它 + [`rect_within_single_display`]，
/// 保证"取景框必须完整落在一块显示器内"这条不变量只有一处定义。
pub fn display_bounds_in_points(app: &tauri::AppHandle) -> Vec<SelectionRect> {
    app.available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|monitor| {
            let scale = monitor.scale_factor();
            let scale = if scale > 0.0 { scale } else { 1.0 };
            let position = monitor.position();
            let size = monitor.size();
            SelectionRect {
                x: f64::from(position.x) / scale,
                y: f64::from(position.y) / scale,
                width: f64::from(size.width) / scale,
                height: f64::from(size.height) / scale,
            }
        })
        .collect()
}

/// 译境的一次采集：调用方已先隐藏取景器，故这里仅校验区域、等待合成器撤帧、按当前固定
/// 区域截图，再复用 OCR → 弹窗翻译的既有单一主干。它是**用户明确点击**的单次动作，不是
/// 持续轮询；定时/自动策略只能在 S15 ScreenCaptureKit 否决门通过后接入（TDD-05 §1.6）。
///
/// 任何提前收场都必须让取景器**恢复可见**（会话仍在运行时），否则用户会失去唯一的控制入口；
/// 唯一例外是会话已关闭——那时绝不能把镜框拉回来（`PRD-01-034` 关闭即终态）。
#[cfg(target_os = "macos")]
pub async fn capture_fixed_region(app: tauri::AppHandle, rect: SelectionRect) {
    use crate::notifications::NotificationEvent;
    use tauri::Manager as _;

    let lens_active = || {
        app.try_state::<crate::ipc::AppState>()
            .is_some_and(|state| state.desktop_lens_active())
    };
    // 会话已关闭：取景器保持隐藏，绝不因本次在途采集被重新唤醒。
    if !lens_active() {
        return;
    }
    let abort = |event: Option<NotificationEvent>| {
        if lens_active() {
            if let Some(event) = event {
                notify(&app, event);
            }
            crate::reveal_desktop_lens(&app);
        }
    };
    if selection_too_small(&rect) {
        abort(None);
        return;
    }
    let displays = display_bounds_in_points(&app);
    if !displays.is_empty() && !rect_within_single_display(&rect, &displays) {
        abort(Some(NotificationEvent::desktop_lens_region_offscreen()));
        return;
    }
    tokio::time::sleep(std::time::Duration::from_millis(OVERLAY_TEARDOWN_MS)).await;
    let text = match capture_region(&rect).await {
        Ok(path) => match ocr_and_cleanup(&app, &path, true).await {
            Ok(text) => text,
            Err(error) => {
                eprintln!("[desktop-lens] OCR failed: {error}");
                abort(Some(NotificationEvent::screenshot_no_text()));
                return;
            }
        },
        Err(error) => {
            eprintln!("[desktop-lens] capture failed: {error}");
            abort(Some(NotificationEvent::screenshot_no_text()));
            return;
        }
    };
    if !lens_active() {
        return;
    }
    match decide_ocr_next_step(text) {
        OcrNextStep::NotifyEmpty => notify(&app, NotificationEvent::screenshot_no_text()),
        OcrNextStep::ShowPopup(text) => {
            if let Some(state) = app.try_state::<crate::ipc::AppState>() {
                if let Ok(mut session) = state.popup().lock() {
                    session.set_display_from_translate_space(text);
                }
            }
            let app_show = app.clone();
            let _ = app.run_on_main_thread(move || crate::show_popup_panel(&app_show));
        }
    }
    if lens_active() {
        crate::reveal_desktop_lens(&app);
    }
}

/// 取消框选（右键 / Esc / 框选层不可用）：隐藏框选层，此前弹窗可见则原样恢复。
#[cfg(target_os = "macos")]
pub fn cancel_overlay(app: &tauri::AppHandle) {
    use tauri::Manager as _;
    crate::hide_screenshot_overlay(app);
    if let Some(state) = app.try_state::<OverlayState>() {
        if let Some(restore) = state.take() {
            restore_popup_if_needed(app, restore);
        }
    }
}

/// 非 macOS 平台占位：截屏翻译随 Windows 并行兼容阶段接入（与 `TextInjector` 同节奏）。
#[cfg(not(target_os = "macos"))]
pub async fn run_screenshot_translate(_app: tauri::AppHandle) {
    eprintln!("[screenshot] 当前平台尚未实现截屏翻译");
}

#[cfg(not(target_os = "macos"))]
pub async fn confirm_region(
    _app: tauri::AppHandle,
    _window: tauri::WebviewWindow,
    _rect: SelectionRect,
) {
}

#[cfg(not(target_os = "macos"))]
pub fn cancel_overlay(_app: &tauri::AppHandle) {}

#[cfg(not(target_os = "macos"))]
pub async fn capture_fixed_region(_app: tauri::AppHandle, _rect: SelectionRect) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_ocr_text_notifies_instead_of_opening_popup() {
        assert_eq!(
            decide_ocr_next_step(String::new()),
            OcrNextStep::NotifyEmpty
        );
        assert_eq!(
            decide_ocr_next_step("   \n\t ".to_string()),
            OcrNextStep::NotifyEmpty
        );
    }

    #[test]
    fn recognized_text_goes_to_popup_verbatim() {
        assert_eq!(
            decide_ocr_next_step("Hello\n世界".to_string()),
            OcrNextStep::ShowPopup("Hello\n世界".to_string())
        );
    }

    #[test]
    fn capture_paths_are_unique_per_call() {
        assert_ne!(capture_file_path(), capture_file_path());
    }

    #[test]
    fn tiny_selection_is_treated_as_misclick() {
        assert!(selection_too_small(&SelectionRect {
            x: 10.0,
            y: 10.0,
            width: 3.0,
            height: 200.0
        }));
        assert!(!selection_too_small(&SelectionRect {
            x: 10.0,
            y: 10.0,
            width: 40.0,
            height: 18.0
        }));
    }

    #[test]
    fn selection_converts_to_global_points_with_retina_scale() {
        // Retina ×2：窗口物理原点 (2880, 200) → 点原点 (1440, 100)；CSS 选区直接相加。
        let global = selection_to_global_points(
            &SelectionRect {
                x: 100.0,
                y: 50.0,
                width: 320.0,
                height: 120.0,
            },
            (2880.0, 200.0),
            2.0,
        );
        assert_eq!(
            global,
            SelectionRect {
                x: 1540.0,
                y: 150.0,
                width: 320.0,
                height: 120.0
            }
        );
    }

    /// 双屏（左 1512×982 @2x，右 1920×1080 @1x 接在 x=1512）在全局逻辑点下的边界。
    fn two_display_layout() -> Vec<SelectionRect> {
        vec![
            SelectionRect {
                x: 0.0,
                y: 0.0,
                width: 1512.0,
                height: 982.0,
            },
            SelectionRect {
                x: 1512.0,
                y: 0.0,
                width: 1920.0,
                height: 1080.0,
            },
        ]
    }

    #[test]
    fn region_inside_either_display_is_accepted() {
        let displays = two_display_layout();
        // 主屏内。
        assert!(rect_within_single_display(
            &SelectionRect {
                x: 100.0,
                y: 80.0,
                width: 320.0,
                height: 120.0
            },
            &displays
        ));
        // 副屏内（不同 scale，坐标续在主屏右侧）。
        assert!(rect_within_single_display(
            &SelectionRect {
                x: 1600.0,
                y: 200.0,
                width: 640.0,
                height: 240.0
            },
            &displays
        ));
    }

    #[test]
    fn region_straddling_the_seam_is_rejected() {
        // 横跨主副屏接缝：两半来自不同 scale，单一 -R 矩形无法如实表达。
        assert!(!rect_within_single_display(
            &SelectionRect {
                x: 1400.0,
                y: 100.0,
                width: 400.0,
                height: 200.0
            },
            &two_display_layout()
        ));
    }

    #[test]
    fn region_exactly_filling_a_display_survives_subpoint_rounding() {
        // 恰好铺满副屏、且非整数 scale 换算留下亚点误差时不得误判为跨屏。
        assert!(rect_within_single_display(
            &SelectionRect {
                x: 1512.0 - 0.4,
                y: -0.4,
                width: 1920.0,
                height: 1080.0
            },
            &two_display_layout()
        ));
    }

    #[test]
    fn region_is_rejected_when_no_display_contains_it() {
        // 显示器拔出后残留的旧几何：不落在任何一屏内即拒绝，绝不截出错位画面。
        assert!(!rect_within_single_display(
            &SelectionRect {
                x: 4000.0,
                y: 100.0,
                width: 200.0,
                height: 100.0
            },
            &two_display_layout()
        ));
    }

    #[test]
    fn overlay_state_take_is_idempotent() {
        let state = OverlayState::default();
        state.begin(true);
        assert_eq!(state.take(), Some(true));
        assert_eq!(state.take(), None); // 重复 confirm/cancel 幂等
    }

    #[test]
    fn zero_scale_falls_back_to_identity() {
        let global = selection_to_global_points(
            &SelectionRect {
                x: 5.0,
                y: 6.0,
                width: 7.0,
                height: 8.0,
            },
            (100.0, 100.0),
            0.0,
        );
        assert_eq!(global.x, 105.0);
        assert_eq!(global.y, 106.0);
    }
}
