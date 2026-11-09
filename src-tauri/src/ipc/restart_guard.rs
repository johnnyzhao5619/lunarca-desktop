//! 应用主动重启前的共同闸门（安装更新、重置应用，TDD-13 §5）：录音进行中不重启，全部编辑窗口保存成功
//! 才放行。放行之后输入保持冻结、新的采集开始不了，直到进程换掉；调用方做不成了就 [`RestartClearance::abandon`]。

use std::sync::{Arc, Mutex, PoisonError};

use super::AppState;
use crate::arbiter::{MicArbiter, MicHolder};
use crate::editor_exit::{self, RestartBlocked, RestartHold};
use crate::recording::RecordingState;

pub(super) const CODE_BLOCKED_CAPTURE: &str = "restart_blocked_capture";
const CODE_SAVE_FAILED: &str = "restart_save_failed";
const CODE_SAVE_TIMEOUT: &str = "restart_save_timeout";

fn blocked_code(blocked: RestartBlocked) -> &'static str {
    match blocked {
        RestartBlocked::Busy | RestartBlocked::SaveFailed => CODE_SAVE_FAILED,
        RestartBlocked::TimedOut => CODE_SAVE_TIMEOUT,
    }
}

/// **录音进行中**的判据：录音 / 实时字幕 / 提词器跟读会话未结束（录音中、暂停、停止收尾，暂停时麦克风
/// 已释放，只看占用会漏），或麦克风被语音输入占着。`MicBusy` / `Error` 只是上一次的结果，不算进行中。
fn capture_in_progress(recording: &RecordingState, mic_idle: bool) -> bool {
    recording.session_open() || !mic_idle
}

pub(super) fn capture_blocks_restart(state: &AppState) -> bool {
    let mic_idle = state
        .mic_handle()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .is_idle();
    capture_in_progress(&state.recording.state(), mic_idle)
}

/// 占住麦克风，让判据通过之后到重启之前开始不了新的采集。放手靠 `Drop`；成功时进程直接换掉。
struct MicLease(Arc<Mutex<MicArbiter>>);

impl MicLease {
    fn acquire(mic: Arc<Mutex<MicArbiter>>) -> Option<Self> {
        let acquired = mic
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .try_acquire(MicHolder::AppRestart)
            .is_ok();
        acquired.then_some(Self(mic))
    }
}

impl Drop for MicLease {
    fn drop(&mut self) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .release(MicHolder::AppRestart);
    }
}

/// 重启已获放行。持有期间输入冻结、退出被拦、采集开始不了。
#[must_use]
pub(super) struct RestartClearance {
    _lease: MicLease,
    hold: RestartHold,
}

impl RestartClearance {
    /// 重启不成了：解冻，用户接着用。
    pub(super) fn abandon(self, app: &tauri::AppHandle) {
        editor_exit::release_restart_hold(app, self.hold);
    }
}

/// 占麦克风 → 复查录音会话 → 冻结输入并收齐全部编辑窗口的保存回执。
///
/// # Errors
/// 稳定错误码（`restart_blocked_capture` / `restart_save_failed` / `restart_save_timeout`）。
pub(super) async fn clear_for_restart(
    app: &tauri::AppHandle,
    state: &AppState,
) -> Result<RestartClearance, &'static str> {
    let lease = MicLease::acquire(state.mic_handle()).ok_or(CODE_BLOCKED_CAPTURE)?;
    if state.recording.state().session_open() {
        return Err(CODE_BLOCKED_CAPTURE);
    }
    let hold = editor_exit::flush_before_restart(app)
        .await
        .map_err(blocked_code)?;
    Ok(RestartClearance {
        _lease: lease,
        hold,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::{RecordingStatus, SessionPurpose};

    fn recording(status: RecordingStatus, purpose: SessionPurpose) -> RecordingState {
        RecordingState {
            status,
            purpose,
            session_id: None,
            note_rel: None,
            elapsed_ms: 0.0,
            message_key: None,
            message_detail: None,
        }
    }

    #[test]
    fn an_open_session_or_a_held_mic_counts_as_capturing() {
        use RecordingStatus as S;
        for status in [S::Recording, S::Paused, S::Stopping] {
            for purpose in [
                SessionPurpose::Record,
                SessionPurpose::Caption,
                SessionPurpose::Teleprompter,
            ] {
                assert!(capture_in_progress(&recording(status, purpose), true));
            }
        }
        for status in [S::Idle, S::MicBusy, S::Error] {
            assert!(!capture_in_progress(
                &recording(status, SessionPurpose::Record),
                true
            ));
            assert!(capture_in_progress(
                &recording(status, SessionPurpose::Record),
                false
            ));
        }
    }

    #[test]
    fn the_lease_refuses_a_busy_mic_blocks_new_capture_and_lets_go_on_drop() {
        let mic = Arc::new(Mutex::new(MicArbiter::new()));
        mic.lock()
            .unwrap()
            .try_acquire(MicHolder::VoiceInput)
            .unwrap();
        assert!(MicLease::acquire(Arc::clone(&mic)).is_none());
        mic.lock().unwrap().release(MicHolder::VoiceInput);

        let lease = MicLease::acquire(Arc::clone(&mic)).expect("空闲时占得到");
        assert!(mic
            .lock()
            .unwrap()
            .try_acquire(MicHolder::Recording)
            .is_err());
        drop(lease);
        assert!(mic.lock().unwrap().is_idle());
    }

    #[test]
    fn every_refusal_has_a_stable_code_the_panels_know() {
        assert_eq!(blocked_code(RestartBlocked::Busy), "restart_save_failed");
        assert_eq!(
            blocked_code(RestartBlocked::SaveFailed),
            "restart_save_failed"
        );
        assert_eq!(
            blocked_code(RestartBlocked::TimedOut),
            "restart_save_timeout"
        );
    }
}
