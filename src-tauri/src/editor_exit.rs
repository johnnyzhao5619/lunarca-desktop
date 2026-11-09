//! 退出或重启前收齐编辑窗口的保存回执；失败、缺失或陈旧回执均不能放行。
//!
//! 退出与重启共用一个状态机，拦截点不同：
//! - 退出：在 `ExitRequested` 里 `prevent_exit`，回执收齐后再 `exit`。
//! - 重启（安装更新、重置应用，TDD-13 §5）：`ExitRequestApi::prevent_exit` 遇到 `RESTART_EXIT_CODE` 什么都不做，
//!   `ExitRequested` 里已经拦不住，所以回执必须在调用 `restart` **之前**收齐（[`flush_before_restart`]）。
//!   收齐后输入保持冻结，直到进程换掉；重启前的动作失败才解冻（[`release_restart_hold`]）。
use std::collections::BTreeSet;
use std::sync::{LazyLock, Mutex, MutexGuard, PoisonError};
use std::time::Duration;
use tauri::Manager;
use tauri_specta::Event;
use tokio::sync::oneshot;

/// 等一轮保存回执的上限。到点没收齐按失败处理：留在当前应用，正文还在编辑器里。
const FLUSH_TIMEOUT: Duration = Duration::from_secs(30);

enum Purpose {
    Exit,
    /// 重启这条路由调用方等结果（退出是回执里直接 `exit`，没有人在等）。
    Restart(oneshot::Sender<bool>),
}

#[derive(Default)]
enum Phase {
    #[default]
    Idle,
    Pending {
        id: u32,
        windows: BTreeSet<String>,
        purpose: Purpose,
    },
    ExitApproved,
    /// 重启的保存全部成功，正在安装或重置：输入继续冻结，退出请求一律拦下。
    RestartHeld {
        id: u32,
    },
}

#[derive(Debug, PartialEq, Eq)]
enum Reply {
    Ignored,
    Waiting,
    ExitApproved,
    RestartApproved,
    Refused,
}

#[derive(Default)]
struct Coordinator {
    phase: Phase,
    sequence: u32,
}
impl Coordinator {
    fn next_id(&mut self) -> u32 {
        self.sequence += 1;
        self.sequence
    }
    fn start(&mut self, windows: BTreeSet<String>) -> Option<u32> {
        if !matches!(self.phase, Phase::Idle) || windows.is_empty() {
            return None;
        }
        let id = self.next_id();
        self.phase = Phase::Pending {
            id,
            windows,
            purpose: Purpose::Exit,
        };
        Some(id)
    }
    /// 没有编辑窗口时直接进入 `RestartHeld`：没有东西要等，但安装期间同样不许退出。
    fn start_restart(
        &mut self,
        windows: BTreeSet<String>,
    ) -> Option<(u32, oneshot::Receiver<bool>)> {
        if !matches!(self.phase, Phase::Idle) {
            return None;
        }
        let id = self.next_id();
        let (saved, outcome) = oneshot::channel();
        if windows.is_empty() {
            let _ = saved.send(true);
            self.phase = Phase::RestartHeld { id };
        } else {
            self.phase = Phase::Pending {
                id,
                windows,
                purpose: Purpose::Restart(saved),
            };
        }
        Some((id, outcome))
    }
    fn reply(&mut self, id: u32, window: &str, saved: bool) -> Reply {
        let Phase::Pending {
            id: current,
            windows,
            ..
        } = &mut self.phase
        else {
            return Reply::Ignored;
        };
        if id != *current || !windows.remove(window) {
            return Reply::Ignored;
        }
        if saved && !windows.is_empty() {
            return Reply::Waiting;
        }
        let Phase::Pending { purpose, .. } = std::mem::take(&mut self.phase) else {
            return Reply::Ignored;
        };
        match (saved, purpose) {
            (false, Purpose::Exit) => Reply::Refused,
            (false, Purpose::Restart(outcome)) => {
                let _ = outcome.send(false);
                Reply::Refused
            }
            (true, Purpose::Exit) => {
                self.phase = Phase::ExitApproved;
                Reply::ExitApproved
            }
            (true, Purpose::Restart(outcome)) => {
                self.phase = Phase::RestartHeld { id };
                let _ = outcome.send(true);
                Reply::RestartApproved
            }
        }
    }
    fn cancel(&mut self, id: u32) -> bool {
        if matches!(self.phase, Phase::Pending { id: current, .. } if current == id) {
            self.phase = Phase::Idle;
            return true;
        }
        false
    }
    fn take_approval(&mut self) -> bool {
        if matches!(self.phase, Phase::ExitApproved) {
            self.phase = Phase::Idle;
            return true;
        }
        false
    }
    fn release_restart(&mut self, id: u32) -> bool {
        if matches!(self.phase, Phase::RestartHeld { id: current } if current == id) {
            self.phase = Phase::Idle;
            return true;
        }
        false
    }
    fn restart_in_progress(&self) -> bool {
        matches!(
            self.phase,
            Phase::RestartHeld { .. }
                | Phase::Pending {
                    purpose: Purpose::Restart(_),
                    ..
                }
        )
    }
    fn is_idle(&self) -> bool {
        matches!(self.phase, Phase::Idle)
    }
}

static EXIT: LazyLock<Mutex<Coordinator>> = LazyLock::new(|| Mutex::new(Coordinator::default()));

/// 中毒也继续用：`panic` 不能换成「拦不住退出」或闪退。
fn lock(coordinator: &Mutex<Coordinator>) -> MutexGuard<'_, Coordinator> {
    coordinator.lock().unwrap_or_else(PoisonError::into_inner)
}

fn is_editor_window(label: &str) -> bool {
    label == "main" || label.starts_with("note-editor-") || label.starts_with("vault-window-")
}

fn editor_windows(app: &tauri::AppHandle) -> BTreeSet<String> {
    app.webview_windows()
        .into_keys()
        .filter(|label| is_editor_window(label))
        .collect()
}

/// 一轮保存进行中不开新的编辑窗口：新窗口不在这一轮的回执名单里，也不会被冻结。
pub(crate) fn accepts_new_editor_windows() -> bool {
    lock(&EXIT).is_idle()
}

#[cfg(target_os = "macos")]
pub(crate) fn install_menu(app: &tauri::App) -> tauri::Result<()> {
    use tauri::menu::{AboutMetadata, Menu, MenuItem, PredefinedMenuItem, Submenu};
    let handle = app.handle();
    let menu = Menu::default(handle)?;
    let quit = MenuItem::with_id(
        handle,
        "editor_quit",
        app.state::<std::sync::Arc<crate::i18n::BackendI18n>>()
            .translate("tray:quit")
            .unwrap_or_else(|| "Quit Lunarca".into()),
        true,
        Some("CmdOrCtrl+Q"),
    )?;
    let application = Submenu::with_items(
        handle,
        handle.package_info().name.clone(),
        true,
        &[
            &PredefinedMenuItem::about(
                handle,
                None,
                Some(AboutMetadata {
                    name: Some(handle.package_info().name.clone()),
                    version: Some(handle.package_info().version.to_string()),
                    copyright: handle.config().bundle.copyright.clone(),
                    authors: handle.config().bundle.publisher.clone().map(|p| vec![p]),
                    ..Default::default()
                }),
            )?,
            &PredefinedMenuItem::separator(handle)?,
            &PredefinedMenuItem::services(handle, None)?,
            &PredefinedMenuItem::separator(handle)?,
            &PredefinedMenuItem::hide(handle, None)?,
            &PredefinedMenuItem::hide_others(handle, None)?,
            &PredefinedMenuItem::separator(handle)?,
            &quit,
        ],
    )?;
    // Tauri 默认应用菜单位于首项；原生 quit 直接调用 NSApp terminate，绕过退出回执。
    menu.remove_at(0)?;
    menu.insert(&application, 0)?;
    app.set_menu(menu)?;
    app.on_menu_event(|app, event| {
        if event.id.as_ref() == "editor_quit" {
            app.exit(0);
        }
    });
    Ok(())
}

pub(crate) fn request(app: &tauri::AppHandle, code: Option<i32>, api: &tauri::ExitRequestApi) {
    // 走到这里的重启已经收齐回执（`prevent_exit` 对它无效，见模块注释），不再发起一轮。
    if code == Some(tauri::RESTART_EXIT_CODE) {
        return;
    }
    let mut coordinator = lock(&EXIT);
    if coordinator.take_approval() {
        return;
    }
    if coordinator.restart_in_progress() {
        api.prevent_exit();
        return;
    }
    let windows = editor_windows(app);
    if windows.is_empty() {
        return;
    }
    api.prevent_exit();
    let Some(id) = coordinator.start(windows.clone()) else {
        return;
    };
    drop(coordinator);
    for window in windows {
        if (crate::ipc::EditorFlushRequested { request_id: id })
            .emit_to(app, window)
            .is_err()
        {
            lock(&EXIT).cancel(id);
            let _ = crate::ipc::EditorFlushCancelled.emit(app);
            crate::show_main_window(app);
            return;
        }
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FLUSH_TIMEOUT).await;
        if lock(&EXIT).cancel(id) {
            let _ = crate::ipc::EditorFlushCancelled.emit(&app);
            crate::show_main_window(&app);
        }
    });
}

pub(crate) fn reply(app: &tauri::AppHandle, window: &str, id: u32, saved: bool) {
    let outcome = lock(&EXIT).reply(id, window, saved);
    match outcome {
        Reply::ExitApproved => app.exit(0),
        Reply::Refused => {
            let _ = crate::ipc::EditorFlushCancelled.emit(app);
            if let Some(window) = app.get_webview_window(window) {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }
        Reply::Ignored | Reply::Waiting | Reply::RestartApproved => {}
    }
}

/// 重启为什么没被放行（都留在当前应用，正文还在编辑器里）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RestartBlocked {
    /// 已有一轮保存在进行（比如用户刚按了退出）。
    Busy,
    /// 有窗口回报写盘失败或外部冲突，或者保存请求发不出去。
    SaveFailed,
    TimedOut,
}

/// 全部保存成功之后的占用：不交还就一直冻结输入、拦住退出。
#[must_use]
pub(crate) struct RestartHold(u32);

/// 发起一轮保存所需的窗口面（测试里换成替身，真实实现是 [`TauriFlush`]）。
trait FlushChannel {
    fn editor_windows(&self) -> BTreeSet<String>;
    fn request_flush(&self, window: &str, id: u32) -> bool;
    fn cancelled(&self);
}

struct TauriFlush<'a>(&'a tauri::AppHandle);

impl FlushChannel for TauriFlush<'_> {
    fn editor_windows(&self) -> BTreeSet<String> {
        editor_windows(self.0)
    }
    fn request_flush(&self, window: &str, id: u32) -> bool {
        crate::ipc::EditorFlushRequested { request_id: id }
            .emit_to(self.0, window.to_owned())
            .is_ok()
    }
    fn cancelled(&self) {
        let _ = crate::ipc::EditorFlushCancelled.emit(self.0);
    }
}

async fn flush_for_restart(
    coordinator: &Mutex<Coordinator>,
    channel: &impl FlushChannel,
    timeout: Duration,
) -> Result<u32, RestartBlocked> {
    let windows = channel.editor_windows();
    let (id, mut outcome) = lock(coordinator)
        .start_restart(windows.clone())
        .ok_or(RestartBlocked::Busy)?;
    for window in &windows {
        if !channel.request_flush(window, id) {
            lock(coordinator).cancel(id);
            channel.cancelled();
            return Err(RestartBlocked::SaveFailed);
        }
    }
    match tokio::time::timeout(timeout, &mut outcome).await {
        Ok(Ok(true)) => Ok(id),
        // 拒绝的那份回执已经广播过解冻（[`reply`]）。
        Ok(Ok(false) | Err(_)) => Err(RestartBlocked::SaveFailed),
        Err(_) => {
            if lock(coordinator).cancel(id) {
                channel.cancelled();
                return Err(RestartBlocked::TimedOut);
            }
            // 到点的同时最后一份回执也到了：以回执为准。
            match outcome.try_recv() {
                Ok(true) => Ok(id),
                _ => Err(RestartBlocked::SaveFailed),
            }
        }
    }
}

/// 冻结全部编辑窗口的输入并等保存回执。`Ok` 之后输入保持冻结，调用方要么重启，要么
/// [`release_restart_hold`]。
///
/// # Errors
/// 见 [`RestartBlocked`]。
pub(crate) async fn flush_before_restart(
    app: &tauri::AppHandle,
) -> Result<RestartHold, RestartBlocked> {
    let result = flush_for_restart(&EXIT, &TauriFlush(app), FLUSH_TIMEOUT).await;
    // 回执失败时 [`reply`] 已经把出错的窗口叫到前面；超时不知道是哪一扇。
    if matches!(result, Err(RestartBlocked::TimedOut)) {
        crate::show_main_window(app);
    }
    result.map(RestartHold)
}

/// 重启不成了（安装或重置失败）：解冻，用户接着编辑。
pub(crate) fn release_restart_hold(app: &tauri::AppHandle, hold: RestartHold) {
    if lock(&EXIT).release_restart(hold.0) {
        let _ = crate::ipc::EditorFlushCancelled.emit(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    fn windows() -> BTreeSet<String> {
        ["main".into(), "note-editor-1".into()].into()
    }
    #[test]
    fn all_windows_must_ack_before_exit() {
        let mut c = Coordinator::default();
        let id = c.start(windows()).unwrap();
        assert_eq!(c.reply(id, "main", true), Reply::Waiting);
        assert!(!c.take_approval());
        assert_eq!(c.reply(id, "main", true), Reply::Ignored);
        assert_eq!(c.reply(id, "wrong-window", true), Reply::Ignored);
        assert_eq!(c.reply(id, "note-editor-1", true), Reply::ExitApproved);
        assert!(c.take_approval());
        assert!(!c.take_approval());
    }
    #[test]
    fn failure_timeout_and_stale_reply_never_approve() {
        let mut c = Coordinator::default();
        let id = c.start(windows()).unwrap();
        assert_eq!(c.reply(id, "main", false), Reply::Refused);
        assert!(!c.take_approval());
        let next = c.start(windows()).unwrap();
        assert!(next > id);
        assert_eq!(c.reply(id, "note-editor-1", true), Reply::Ignored);
        assert!(c.cancel(next));
        assert_eq!(c.reply(next, "main", true), Reply::Ignored);
        assert!(!c.take_approval());
    }

    #[test]
    fn editor_windows_are_main_notes_and_side_vaults_only() {
        for label in ["main", "note-editor-3", "vault-window-0"] {
            assert!(is_editor_window(label), "{label}");
        }
        for label in ["voice-hud", "popup", "caption"] {
            assert!(!is_editor_window(label), "{label}");
        }
    }

    /// 窗口替身：记下收到保存请求的窗口与解冻次数；`replies` 是窗口收到请求后怎么答。
    struct FakeWindows {
        coordinator: Arc<Mutex<Coordinator>>,
        windows: BTreeSet<String>,
        /// `None` = 这扇窗不答（保存卡死）。
        replies: Vec<(&'static str, Option<bool>)>,
        send_fails: bool,
        requested: Mutex<Vec<String>>,
        cancelled: AtomicU32,
    }

    impl FakeWindows {
        fn new(replies: Vec<(&'static str, Option<bool>)>) -> Self {
            Self {
                coordinator: Arc::default(),
                windows: replies.iter().map(|(w, _)| (*w).to_owned()).collect(),
                replies,
                send_fails: false,
                requested: Mutex::default(),
                cancelled: AtomicU32::new(0),
            }
        }
        fn cancel_count(&self) -> u32 {
            self.cancelled.load(Ordering::SeqCst)
        }
        async fn flush(&self) -> Result<u32, RestartBlocked> {
            flush_for_restart(&self.coordinator, self, Duration::from_secs(30)).await
        }
    }

    impl FlushChannel for FakeWindows {
        fn editor_windows(&self) -> BTreeSet<String> {
            self.windows.clone()
        }
        fn request_flush(&self, window: &str, id: u32) -> bool {
            if self.send_fails {
                return false;
            }
            self.requested.lock().unwrap().push(window.to_owned());
            let answer = self.replies.iter().find(|(w, _)| *w == window);
            if let Some((_, Some(saved))) = answer {
                // 与 [`reply`] 一样：拒绝的回执由回执入口广播解冻。
                if lock(&self.coordinator).reply(id, window, *saved) == Reply::Refused {
                    self.cancelled();
                }
            }
            true
        }
        fn cancelled(&self) {
            self.cancelled.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn restart_waits_for_every_dirty_window() {
        let fake = FakeWindows::new(vec![
            ("main", Some(true)),
            ("note-editor-1", Some(true)),
            ("vault-window-0", Some(true)),
        ]);
        let id = fake.flush().await.expect("全部保存成功才放行");
        assert_eq!(fake.requested.lock().unwrap().len(), 3);
        assert_eq!(fake.cancel_count(), 0, "保存成功后输入保持冻结");
        assert!(lock(&fake.coordinator).restart_in_progress());
        assert!(lock(&fake.coordinator).release_restart(id));
    }

    #[tokio::test]
    async fn no_editor_windows_still_holds_exit_during_install() {
        let fake = FakeWindows::new(vec![]);
        let id = fake.flush().await.expect("没有窗口要等");
        assert!(lock(&fake.coordinator).restart_in_progress());
        assert!(lock(&fake.coordinator).release_restart(id));
    }

    /// 写盘失败与外部冲突在窗口里都是 `saved = false`（前端的判定见 editor-restart-flush.test.ts）。
    #[tokio::test]
    async fn a_window_that_could_not_save_keeps_the_app_running() {
        let fake = FakeWindows::new(vec![("main", Some(true)), ("note-editor-1", Some(false))]);
        assert_eq!(fake.flush().await, Err(RestartBlocked::SaveFailed));
        assert_eq!(fake.cancel_count(), 1, "拒绝后全部窗口解冻");
        assert!(lock(&fake.coordinator).is_idle(), "可以再来一轮");
        assert!(fake.flush().await.is_err(), "下一轮同样按回执判定");
    }

    #[tokio::test(start_paused = true)]
    async fn a_window_that_never_replies_times_out_and_stays() {
        let fake = FakeWindows::new(vec![("main", Some(true)), ("note-editor-1", None)]);
        assert_eq!(fake.flush().await, Err(RestartBlocked::TimedOut));
        assert_eq!(fake.cancel_count(), 1, "超时后解冻");
        assert!(lock(&fake.coordinator).is_idle());
        // 卡住的保存迟到的回执不能放行任何东西。
        let stale = lock(&fake.coordinator).sequence;
        assert_eq!(
            lock(&fake.coordinator).reply(stale, "note-editor-1", true),
            Reply::Ignored
        );
    }

    #[tokio::test]
    async fn a_flush_request_that_cannot_be_sent_blocks_the_restart() {
        let mut fake = FakeWindows::new(vec![("main", Some(true))]);
        fake.send_fails = true;
        assert_eq!(fake.flush().await, Err(RestartBlocked::SaveFailed));
        assert_eq!(fake.cancel_count(), 1);
        assert!(lock(&fake.coordinator).is_idle());
    }

    #[tokio::test]
    async fn an_exit_already_flushing_refuses_the_restart() {
        let fake = FakeWindows::new(vec![("main", Some(true))]);
        let exit = lock(&fake.coordinator).start(windows()).unwrap();
        assert_eq!(fake.flush().await, Err(RestartBlocked::Busy));
        assert!(fake.requested.lock().unwrap().is_empty());
        assert!(lock(&fake.coordinator).cancel(exit));
    }

    #[test]
    fn restart_in_progress_blocks_exit_and_new_rounds_until_released() {
        let mut c = Coordinator::default();
        let (id, _outcome) = c.start_restart(windows()).unwrap();
        assert!(c.restart_in_progress());
        assert!(!c.is_idle(), "保存进行中不开新的编辑窗口");
        assert!(c.start(windows()).is_none());
        assert_eq!(c.reply(id, "main", true), Reply::Waiting);
        assert_eq!(c.reply(id, "note-editor-1", true), Reply::RestartApproved);
        assert!(c.restart_in_progress(), "安装期间继续拦退出");
        assert!(!c.take_approval(), "重启的批准不能被退出请求领走");
        assert!(!c.release_restart(id + 1), "陈旧的占用不能解冻");
        assert!(c.release_restart(id));
        assert!(c.is_idle());
    }
}
