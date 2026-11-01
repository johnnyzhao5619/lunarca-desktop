//! 多浮层并发仲裁 · 麦克风硬互斥（UI 层互斥）。事实源：TDD-05 §5.3、PRD-00 §7。
//!
//! 全局入口（P0：弹窗翻译 / 语音输入 HUD / 快捷对话；P1：录音）都是按热键随时唤起的浮层，
//! **必然会被同时触发**——这是物理冲突，须在 UI 层显式仲裁。区别于 [TDD-01 §5.2] 的**引擎资源**
//! 抢占（那解决"谁先用 whisper / 推理引擎"），本模块解决"**谁持有麦克风**"。
//!
//! **麦克风独占（硬互斥）**：语音输入与录音都走 [`luna_audio`] 的采集，**同一时刻只允许一路
//! 采集会话**：录音进行中触发语音输入 → 默认拒绝并提示；语音输入进行中触发录音 → 提示先结束；
//! **弹窗翻译输入模式的语音按钮复用语音输入的"录入+ASR"子能力（PRD-01-005），故同受此互斥**。
//!
//! 归 `luna-app` 的浮层状态机（单一来源）；纯逻辑、零 IO，headless 可测。

use std::fmt;

/// 麦克风占用者（用于互斥裁决 + 拒绝提示 + 确保"谁取谁还"）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MicHolder {
    /// AI 语音输入（Cmd+Shift+V）。
    VoiceInput,
    /// 弹窗翻译输入模式的语音按钮（复用语音输入子能力，PRD-01-005）。
    PopupVoiceButton,
    /// 快捷录音（P1，Cmd+Shift+R）。
    Recording,
}

impl MicHolder {
    /// 面向用户的占用者名（i18n key 基；UI 经 key 取本地化文案）。
    #[must_use]
    pub fn i18n_key(self) -> &'static str {
        match self {
            Self::VoiceInput => "mic.holder.voice_input",
            Self::PopupVoiceButton => "mic.holder.popup_voice",
            Self::Recording => "mic.holder.recording",
        }
    }
}

impl fmt::Display for MicHolder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::VoiceInput => "语音输入",
            Self::PopupVoiceButton => "弹窗语音输入",
            Self::Recording => "录音",
        })
    }
}

/// 申请麦克风失败：已被他者占用。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MicBusy {
    /// 当前持有者。
    pub held_by: MicHolder,
    /// 申请者。
    pub requested_by: MicHolder,
}

impl MicBusy {
    /// 中文拒绝引导（脱敏：不含任何转写文本/音频，隐私铁律）。
    #[must_use]
    pub fn guidance_text(self) -> String {
        match (self.held_by, self.requested_by) {
            (MicHolder::Recording, _) => {
                "录音进行中，已暂不开启语音输入。请先结束录音，或将这段话直接录入当前录音。"
                    .to_string()
            }
            (held, MicHolder::Recording) => {
                format!("{held}进行中，请先结束后再开始录音。")
            }
            (held, _) => format!("{held}正在使用麦克风，请先结束后再试。"),
        }
    }

    /// i18n key——**UI 只认这个**。`guidance_text` 只服务 CLI 与日志。
    #[must_use]
    pub fn i18n_key(self) -> &'static str {
        match self.held_by {
            MicHolder::Recording => "mic.busy.recording_active",
            _ => "mic.busy.capture_active",
        }
    }
}

/// 麦克风硬互斥仲裁器（同一时刻只允许一路采集会话）。
///
/// 用法：唤起语音输入/录音前 [`try_acquire`]；采集会话结束（正常/取消/失败/手动停）后
/// **务必** [`release`]（采集句柄 drop 的兜底也应触发 release，防麦克风悬挂，PRD-02 §5 边界②）。
///
/// [`try_acquire`]: MicArbiter::try_acquire
/// [`release`]: MicArbiter::release
#[derive(Clone, Copy, Debug, Default)]
pub struct MicArbiter {
    held_by: Option<MicHolder>,
}

impl MicArbiter {
    /// 新建空闲仲裁器。
    #[must_use]
    pub fn new() -> Self {
        Self { held_by: None }
    }

    /// 申请麦克风。空闲则授予并记录持有者；已被占用则拒绝（含提示信息）。
    ///
    /// **重入约定**：同一持有者重复申请视为成功（幂等，便于"按住说话"期间的状态查询）。
    ///
    /// # Errors
    /// 已被**他者**占用时返回 [`MicBusy`]。
    pub fn try_acquire(&mut self, who: MicHolder) -> Result<(), MicBusy> {
        match self.held_by {
            None => {
                self.held_by = Some(who);
                Ok(())
            }
            Some(h) if h == who => Ok(()), // 幂等重入。
            Some(h) => Err(MicBusy {
                held_by: h,
                requested_by: who,
            }),
        }
    }

    /// 释放麦克风——**仅当持有者为 `who`** 才释放（防误释他者会话）。返回是否释放成功。
    pub fn release(&mut self, who: MicHolder) -> bool {
        if self.held_by == Some(who) {
            self.held_by = None;
            true
        } else {
            false
        }
    }

    /// 当前持有者（`None` = 空闲）。
    #[must_use]
    pub fn holder(&self) -> Option<MicHolder> {
        self.held_by
    }

    /// 是否空闲。
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.held_by.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_when_idle_succeeds() {
        let mut a = MicArbiter::new();
        assert!(a.is_idle());
        assert!(a.try_acquire(MicHolder::VoiceInput).is_ok());
        assert_eq!(a.holder(), Some(MicHolder::VoiceInput));
    }

    #[test]
    fn recording_blocks_voice_input_with_guidance() {
        let mut a = MicArbiter::new();
        a.try_acquire(MicHolder::Recording).unwrap();
        let err = a.try_acquire(MicHolder::VoiceInput).unwrap_err();
        assert_eq!(err.held_by, MicHolder::Recording);
        assert_eq!(err.i18n_key(), "mic.busy.recording_active");
        assert!(err.guidance_text().contains("录音进行中"));
    }

    #[test]
    fn voice_input_blocks_recording() {
        let mut a = MicArbiter::new();
        a.try_acquire(MicHolder::VoiceInput).unwrap();
        let err = a.try_acquire(MicHolder::Recording).unwrap_err();
        assert_eq!(err.held_by, MicHolder::VoiceInput);
        assert!(err.guidance_text().contains("请先结束"));
    }

    #[test]
    fn popup_voice_button_shares_the_mutex() {
        // PRD-01-005：弹窗语音按钮复用语音输入子能力，录音进行中同样被拒。
        let mut a = MicArbiter::new();
        a.try_acquire(MicHolder::Recording).unwrap();
        assert!(a.try_acquire(MicHolder::PopupVoiceButton).is_err());
    }

    #[test]
    fn same_holder_reacquire_is_idempotent() {
        let mut a = MicArbiter::new();
        a.try_acquire(MicHolder::VoiceInput).unwrap();
        assert!(a.try_acquire(MicHolder::VoiceInput).is_ok());
    }

    #[test]
    fn release_only_by_owner() {
        let mut a = MicArbiter::new();
        a.try_acquire(MicHolder::VoiceInput).unwrap();
        // 他者释放无效（防误释）。
        assert!(!a.release(MicHolder::Recording));
        assert_eq!(a.holder(), Some(MicHolder::VoiceInput));
        // 持有者释放成功 → 回空闲。
        assert!(a.release(MicHolder::VoiceInput));
        assert!(a.is_idle());
        // 重复释放无效。
        assert!(!a.release(MicHolder::VoiceInput));
    }

    #[test]
    fn idle_after_release_allows_new_holder() {
        let mut a = MicArbiter::new();
        a.try_acquire(MicHolder::VoiceInput).unwrap();
        a.release(MicHolder::VoiceInput);
        assert!(a.try_acquire(MicHolder::Recording).is_ok());
    }
}
