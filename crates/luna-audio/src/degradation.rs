//! 降级状态机与滞后区间控制器。事实源：TDD-02 §5.2。
//!
//! `DegradationState`/`DegradationController` 是 `luna-audio` 提供的可复用算子；
//! **实例与档位驱动由 `RealtimeSession`（编排层）持有**——`luna-audio` 只产指标 + 暴露旋钮，
//! 不持有决策（TDD-02 §5.2 决策归属，避免下层反向驱动上层）。
//!
//! ⚠ `#[repr(u8)]` 钉死判别值顺序：`update()` 用 `as u8` 比较档位高低；
//! 后人重排变体会导致升降档逻辑静默错乱——禁止重排。

/// 降级档位（TDD-02 §5.2）。
///
/// ⚠ 禁止重排变体——判别值顺序与 `as u8` 升降档比较强绑定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DegradationState {
    /// RTF < 0.6：全功能正常（波形维持基线 ~18fps、实时转译、默认 ASR 引擎）。
    Green = 0,
    /// 0.6 ≤ RTF < 0.8：便宜杠杆——波形临时降至 ~5fps + 挂起实时转译译文。
    Yellow = 1,
    /// 0.8 ≤ RTF < 1.0：进一步调宽 commit 间隔/缩短窗；持续不缓解则在 utterance 边界换小模型。
    Orange = 2,
    /// RTF ≥ 1.0：全面挂起 ASR 推理，进入 Spill-to-Disk 纯录音模式（结束后超实时补转写）。
    Red = 3,
}

/// **本机 ASR 负载**的一次采样（`DegradationController` 的输入，TDD-02 §5.2 / ADR-078 ②）。
///
/// # 为什么输入不再是「单条流的 RTF」
///
/// RTF 只答得上「实时腿跟不跟得上」，答不上「是谁让它跟不上的」。而卸载负载的第一个
/// 问题恰恰是后者：同一个 RTF=0.9，可能是这台机器本来就慢（那就该少解几个快照），
/// 也可能是一个批式二次转写正在同一个 whisper 池里全速跑（那就该先把批式停下来——
/// 它的音频已经全量落盘，等三十秒解出来的字一模一样）。
///
/// 此前控制器只看得见 RTF，于是**唯一**能做的动作就是跳实时快照窗，结果是被批式推高的
/// 负载由实时字幕买单：用户点了「二次转写」，代价是同一时刻的实时字幕停更。方向反了。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AsrLoad {
    /// 实时腿的滑窗 RTF（`T_inference / T_audio`）。
    pub realtime_rtf: f32,
    /// 此刻在本机 ASR 引擎上跑着的**批式**转写 Job 数（音频已全量在手、无截止期的那一类）。
    pub batch_inflight: usize,
}

impl AsrLoad {
    /// 只有实时腿、没有批式在跑的负载（单测与无批式路径的便捷构造）。
    #[must_use]
    pub const fn realtime_only(rtf: f32) -> Self {
        Self {
            realtime_rtf: rtf,
            batch_inflight: 0,
        }
    }
}

/// 高负载时**先卸谁**（TDD-02 §5.2 的「谁被节流」，ADR-078 ②）。
///
/// 判据只有一条：**错过了还能不能补回来**。批式转写的音频已经全量落盘，晚三十秒解出来
/// 的字一模一样；实时窗错过就是永久丢失（停录后的权威重转写能补齐文本，但补不回
/// 「当时看得见字幕」这件事本身）。所以顺序是固定的：先批式，再实时。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SheddingTarget {
    /// 负载正常，谁都不卸。
    None,
    /// 先停批式转写的准入——它没有截止期。
    BatchAsr,
    /// 批式已经卸干净了负载还是下不来 → 才轮到实时窗（`should_decode_window` 的档位语义）。
    RealtimeWindows,
}

/// 一次负载采样的裁决：档位 + 该卸谁。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DegradationDecision {
    /// 当前有效档位（滞后区间控制器的输出，语义与此前完全一致）。
    pub tier: DegradationState,
    /// 本档位下**先卸哪一侧**。
    pub shed: SheddingTarget,
}

impl DegradationDecision {
    /// 喂给 `should_decode_window` 的**实时档位**。
    ///
    /// 档位语义本身不变（Yellow/Orange 跳快照、Red 只放探测窗）——变的是「什么时候
    /// 轮到实时被跳」。只要还有批式可卸，实时这一侧就按 Green 走：先把没有截止期的那份
    /// 活停下来，是免费的；先跳实时快照窗，是拿用户此刻看得见的东西去补一份晚点解也一样
    /// 的活。
    #[must_use]
    pub const fn realtime_tier(&self) -> DegradationState {
        match self.shed {
            SheddingTarget::None | SheddingTarget::BatchAsr => DegradationState::Green,
            SheddingTarget::RealtimeWindows => self.tier,
        }
    }
}

/// 降级状态机（滞后区间控制器，TDD-02 §5.2）。
///
/// - **升档（加压）即时生效**：确保算力自愈不被延迟。
/// - **降档（恢复）需连续 3 tick**：防止 RTF 在临界点频繁来回跳变（ping-pong effect）。
pub struct DegradationController {
    current_state: DegradationState,
    /// 连续低负载 tick 计数（用于延迟降档）。
    low_load_ticks: usize,
}

impl DegradationController {
    /// 构造初始状态为 Green 的控制器。
    #[must_use]
    pub fn new() -> Self {
        Self {
            current_state: DegradationState::Green,
            low_load_ticks: 0,
        }
    }

    /// 当前档位。
    #[must_use]
    pub fn state(&self) -> DegradationState {
        self.current_state
    }

    /// 以一次**本机 ASR 负载**采样更新状态机，返回档位 + 该卸谁。
    ///
    /// 升档（Green→…→Red）立刻生效；降档需连续 3 tick 处于低负载。
    pub fn update(&mut self, load: AsrLoad) -> DegradationDecision {
        let tier = self.update_tier(load.realtime_rtf);
        let shed = match (tier, load.batch_inflight) {
            (DegradationState::Green, _) => SheddingTarget::None,
            // 还有批式在跑 → 它先让路。释放的是同一个 whisper 池里的算力，
            // 而它一个字都不会因此丢（音频全量在手，晚点解结果相同）。
            (_, 1..) => SheddingTarget::BatchAsr,
            // 没有批式可卸了，负载是这台机器自己的 → 才轮到实时窗按档位跳。
            (_, 0) => SheddingTarget::RealtimeWindows,
        };
        DegradationDecision { tier, shed }
    }

    /// 滞后区间档位推进（本函数的行为与本次改动前逐字一致——变的只是它的调用者
    /// 现在还要回答「该卸谁」）。
    fn update_tier(&mut self, rtf: f32) -> DegradationState {
        let target = if rtf >= 1.0 {
            DegradationState::Red
        } else if rtf >= 0.8 {
            DegradationState::Orange
        } else if rtf >= 0.6 {
            DegradationState::Yellow
        } else {
            DegradationState::Green
        };

        let target_u8 = target as u8;
        let current_u8 = self.current_state as u8;
        if target_u8 > current_u8 {
            // 升档（加压）：立刻生效，重置降档计数。
            self.current_state = target;
            self.low_load_ticks = 0;
        } else if target_u8 < current_u8 {
            // 降档（恢复）：需连续 3 tick 才切换。
            self.low_load_ticks += 1;
            if self.low_load_ticks >= 3 {
                self.current_state = target;
                self.low_load_ticks = 0;
            }
        }
        // 维持档位时：允许 low_load_ticks 继续累积（连续 tick 恢复路径）。

        self.current_state
    }
}

impl Default for DegradationController {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 只有实时腿在跑时的档位（本控制器此前的全部行为）。
    fn tier(ctrl: &mut DegradationController, rtf: f32) -> DegradationState {
        ctrl.update(AsrLoad::realtime_only(rtf)).tier
    }

    /// `#[repr(u8)]` 判别值顺序回归（禁止重排变体）。
    #[test]
    fn repr_u8_order_is_correct() {
        assert_eq!(DegradationState::Green as u8, 0);
        assert_eq!(DegradationState::Yellow as u8, 1);
        assert_eq!(DegradationState::Orange as u8, 2);
        assert_eq!(DegradationState::Red as u8, 3);
    }

    /// 升档立刻生效（Green → Red，单次 RTF≥1.0）。
    #[test]
    fn escalation_is_immediate() {
        let mut ctrl = DegradationController::new();
        assert_eq!(
            tier(&mut ctrl, 1.5),
            DegradationState::Red,
            "RTF≥1.0 应立刻升到 Red"
        );
        assert_eq!(ctrl.low_load_ticks, 0);
    }

    /// 降档需连续 3 tick（Red → Green 需 3 次 RTF<0.6）。
    #[test]
    fn recovery_requires_3_consecutive_ticks() {
        let mut ctrl = DegradationController::new();
        tier(&mut ctrl, 1.5); // Red
        assert_eq!(ctrl.state(), DegradationState::Red);

        tier(&mut ctrl, 0.3);
        assert_eq!(ctrl.state(), DegradationState::Red, "1 tick 不足，应仍 Red");

        tier(&mut ctrl, 0.3);
        assert_eq!(ctrl.state(), DegradationState::Red, "2 tick 不足，应仍 Red");

        assert_eq!(
            tier(&mut ctrl, 0.3),
            DegradationState::Green,
            "3 tick 后应降到 Green"
        );
    }

    /// 降档途中被高负载打断 → 重置计数并升至新档。
    #[test]
    fn recovery_resets_on_high_load_interrupt() {
        let mut ctrl = DegradationController::new();
        tier(&mut ctrl, 1.5); // Red

        tier(&mut ctrl, 0.3); // tick 1
        tier(&mut ctrl, 0.3); // tick 2
        tier(&mut ctrl, 1.5); // 打断 → 立刻回 Red，计数重置
        assert_eq!(ctrl.state(), DegradationState::Red, "高负载打断后应回 Red");

        tier(&mut ctrl, 0.3);
        tier(&mut ctrl, 0.3);
        tier(&mut ctrl, 0.3);
        assert_eq!(
            ctrl.state(),
            DegradationState::Green,
            "重新 3 tick 后降到 Green"
        );
    }

    /// 完整升降档序列测试。
    #[test]
    fn full_escalation_and_recovery_sequence() {
        let mut ctrl = DegradationController::new();

        assert_eq!(tier(&mut ctrl, 0.3), DegradationState::Green);
        assert_eq!(tier(&mut ctrl, 0.7), DegradationState::Yellow);
        assert_eq!(tier(&mut ctrl, 0.9), DegradationState::Orange);
        assert_eq!(tier(&mut ctrl, 1.1), DegradationState::Red);

        tier(&mut ctrl, 0.1);
        tier(&mut ctrl, 0.1);
        assert_eq!(tier(&mut ctrl, 0.1), DegradationState::Green);
    }

    /// 维持同档位不影响降档 tick 计数（连续低负载可正常累积）。
    #[test]
    fn same_tier_ticks_accumulate() {
        let mut ctrl = DegradationController::new();
        tier(&mut ctrl, 0.7); // Yellow

        tier(&mut ctrl, 0.4);
        tier(&mut ctrl, 0.4);
        assert_eq!(tier(&mut ctrl, 0.4), DegradationState::Green);
    }

    // ── 卸载方向（ADR-078 ②）────────────────────────────────────────────────

    /// **被节流的必须是批式，而不是错过就补不回来的实时窗。**
    ///
    /// 现场那一幕的另一半：批式二次转写把同一个 whisper 池的负载推高 → 实时腿的 RTF
    /// 跟着涨 → 控制器（当时只看得见 RTF）升到 Yellow → `should_decode_window` 跳掉
    /// **全部**快照窗 = 字幕停更。用户点的是「二次转写」，付账的是实时字幕。
    #[test]
    fn a_batch_job_pays_for_the_load_it_created_before_the_live_captions_do() {
        let mut ctrl = DegradationController::new();

        // 高负载 + 有批式在跑 → 先卸批式；实时侧仍按 Green 全量解码。
        let decision = ctrl.update(AsrLoad {
            realtime_rtf: 0.9,
            batch_inflight: 1,
        });
        assert_eq!(decision.tier, DegradationState::Orange, "档位判定不变");
        assert_eq!(decision.shed, SheddingTarget::BatchAsr);
        assert_eq!(
            decision.realtime_tier(),
            DegradationState::Green,
            "还有批式可卸时，实时快照窗必须照常解码"
        );
    }

    /// 批式卸干净了负载还是下不来 → 才轮到实时窗，且档位语义原样沿用。
    #[test]
    fn the_live_leg_is_shed_only_after_the_batch_lane_is_already_empty() {
        let mut ctrl = DegradationController::new();
        let decision = ctrl.update(AsrLoad {
            realtime_rtf: 0.9,
            batch_inflight: 0,
        });
        assert_eq!(decision.shed, SheddingTarget::RealtimeWindows);
        assert_eq!(
            decision.realtime_tier(),
            DegradationState::Orange,
            "没有批式可卸时，实时侧按原档位语义跳窗"
        );
    }

    /// Green 档谁都不卸——批式在跑不构成卸载理由，只有负载才是。
    #[test]
    fn a_healthy_session_sheds_nothing_even_with_batch_work_in_flight() {
        let mut ctrl = DegradationController::new();
        let decision = ctrl.update(AsrLoad {
            realtime_rtf: 0.2,
            batch_inflight: 3,
        });
        assert_eq!(decision.tier, DegradationState::Green);
        assert_eq!(decision.shed, SheddingTarget::None);
        assert_eq!(decision.realtime_tier(), DegradationState::Green);
    }
}
