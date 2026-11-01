//! 双轨时钟对齐：主机 monotonic 锚点 +（后置）漂移估计 + rate-aware 校正决策。
//! 事实源：TDD-02 §3.1-C（rate-aware 重采样）/§3.1-I（时钟对齐）/§6（漂移指标）。
//!
//! **为什么需要主机锚点**：麦克风 ADC 与系统输出 DAC/虚拟时钟属**不同时钟域**，长会必漂移；
//! 且 Both 双轨两会话先后启动有毫秒级偏差。采集侧为帧流打**主机 monotonic 时间戳锚点**——
//! 消费侧据此以主机时钟为主轴估计漂移并微调重采样（§3.1-C 已预留位置），混流时按锚点差对
//! 晚启动轨头部补零对齐（§3.1-I）。
//!
//! **ADR-009 铁律**：[`CaptureClock::mark_captured`] 在 cpal 实时回调 / `FrameInjector` 内调用，
//! 只做 `Instant::now` + 无锁原子写（禁分配/锁/IO/日志）。`Instant::now` 是单调时钟读取，
//! 无分配、无锁，是回调内**唯一**许可的重操作（本批已知约束）。
//!
//! **pacer 补零不计入**：Windows loopback 静音期由 [`crate::pacer::LoopbackPacer`] 在消费线程
//! 补零——那是主机时钟侧产物，绝不经 `mark_captured` 计入采集锚点（计入即自我抵消谬误，§3.1-I）。

use crate::TARGET_SAMPLE_RATE;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// `first_nanos` 的未设置哨兵（首个真实采集批次到达前）。
const UNSET_NANOS: u64 = u64::MAX;

/// 漂移估计所需的最小主机时间跨度（秒）：过短跨度 ppm 估计噪声大，不出评估。
pub const DRIFT_MIN_SPAN_SECS: f64 = 5.0;

/// 漂移校正触发阈值（ms）：16k 输出相对主机时钟的累计残余偏移超此值才微调 ratio。
/// 依据：>2ms 已达单帧（20ms）十分之一，长会累积成可闻错位/AEC 失效（TDD-02 §3.1-I）。
pub const DRIFT_CORRECT_THRESHOLD_MS: f64 = 2.0;

/// 单次校正的相对 ratio 步长（ppm）——克制防音染（§3.1-C「步长克制」）。
/// 50ppm ≈ 每秒 0.05ms 追赶，对语音不可闻，多步收敛。
pub const DRIFT_STEP_PPM: f64 = 50.0;

/// 累计相对 ratio 校正上限（ppm）：大偏差如实上报指标而非猛拉 ratio（本批已知约束）。
pub const DRIFT_MAX_CORRECTION_PPM: f64 = 500.0;

/// 超过这个量就**不是漂移**，是「声明的采集格式」与「设备实际交付」对不上（ADR-082）。
///
/// 真实设备时钟漂移是 ppm 量级（百万分之几十）；[`DRIFT_MAX_CORRECTION_PPM`] 的校正上限
/// 500ppm 已比任何真实晶振宽裕两个数量级。取 100_000ppm（= 10%）作分界：远在真实漂移之上、
/// 又远在「整数倍率对不上」之下（真机实测出现过 +2,002,000ppm = 3.00×，根因是 6 声道的
/// VPIO 回声参考流被当成 2 声道 tap 计量）。落在这一档时校正**永远追不上**，必须与常规
/// 漂移分开报，且**不得只打日志继续**——见 [`crate::CaptureDiagnostic::SourceRateMismatch`]。
pub const RATE_MISMATCH_PPM: f64 = 100_000.0;

/// 采集时钟锚点（每采集会话一个，`Arc` 共享给采集侧与消费侧）。
///
/// 采集侧（cpal 回调 / [`crate::capture::FrameInjector`]）经 [`Self::mark_captured`] 无锁写入；
/// 消费侧经 [`Self::snapshot`] 读取。Both 双轨令两会话共享**同一 `epoch`**（由编排层传入），
/// 使两轨 `first_nanos` 可跨会话相减得启动偏差（§3.1-I 任务 4）。
#[derive(Debug)]
pub struct CaptureClock {
    /// 会话主机时钟原点（Both 双轨共享 → 跨会话锚点可比）。
    epoch: Instant,
    /// 首个真实采集批次的主机纳秒（相对 `epoch`）；`UNSET_NANOS` = 尚无数据。
    first_nanos: AtomicU64,
    /// 最近一次真实采集批次的主机纳秒（相对 `epoch`）。
    last_nanos: AtomicU64,
    /// 真实采集的**交错**样本累计（不含 pacer 补零）。
    produced_samples: AtomicU64,
}

impl CaptureClock {
    /// 以给定主机时钟原点构造。Both 双轨传入**同一 `epoch`**；单轨可用 `Instant::now()`。
    #[must_use]
    pub fn new(epoch: Instant) -> Arc<Self> {
        Arc::new(Self {
            epoch,
            first_nanos: AtomicU64::new(UNSET_NANOS),
            last_nanos: AtomicU64::new(0),
            produced_samples: AtomicU64::new(0),
        })
    }

    /// **实时采集侧**为一批交错样本打主机锚点（**wait-free**，ADR-009）。
    ///
    /// 仅 `Instant::now` + 无锁原子写：首批次经一次 CAS 记 `first_nanos`（后续 no-op），
    /// 累加样本数、更新 `last_nanos`。禁分配/锁/IO/日志。空批次直接返回。
    #[inline]
    pub fn mark_captured(&self, interleaved_len: usize) {
        if interleaved_len == 0 {
            return;
        }
        let t = self.epoch.elapsed().as_nanos() as u64;
        // 首批次记锚点（CAS 从哨兵切换；已设置则失败即忽略——单生产者，无竞态语义损失）。
        let _ =
            self.first_nanos
                .compare_exchange(UNSET_NANOS, t, Ordering::AcqRel, Ordering::Relaxed);
        self.produced_samples
            .fetch_add(interleaved_len as u64, Ordering::Relaxed);
        self.last_nanos.store(t, Ordering::Release);
    }

    /// 消费侧读取当前锚点快照；首个真实批次到达前返回 `None`。
    #[must_use]
    pub fn snapshot(&self) -> Option<ClockSnapshot> {
        let first = self.first_nanos.load(Ordering::Acquire);
        if first == UNSET_NANOS {
            return None;
        }
        let produced = self.produced_samples.load(Ordering::Acquire);
        let last = self.last_nanos.load(Ordering::Acquire);
        Some(ClockSnapshot {
            first_nanos: first,
            last_nanos: last,
            produced_samples: produced,
        })
    }
}

/// 采集锚点快照（消费侧读取）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockSnapshot {
    /// 首个真实采集批次的主机纳秒（相对会话 `epoch`）。
    pub first_nanos: u64,
    /// 最近真实采集批次的主机纳秒（相对会话 `epoch`）。
    pub last_nanos: u64,
    /// 真实采集的交错样本累计（不含 pacer 补零）。
    pub produced_samples: u64,
}

impl ClockSnapshot {
    /// 首批次到最近批次的主机时间跨度（纳秒）。
    #[must_use]
    pub fn host_span_nanos(&self) -> u64 {
        self.last_nanos.saturating_sub(self.first_nanos)
    }
}

/// 一次漂移评估结果（消费线程据此上报指标 + 施加校正）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DriftReport {
    /// 设备时钟相对主机的漂移（ppm；正 = 设备比主机快，采集帧多于主机时钟期望）。
    pub drift_ppm: f64,
    /// 16k 输出相对主机时钟的**累计残余偏移**（ms；正 = 输出超前主机）。校正后趋近 0。
    pub offset_ms: f64,
    /// 本次是否需施加新相对 ratio 校正（`Some(applied_rel)`；`None` = 无需/已达上限）。
    pub apply_rel: Option<f64>,
    /// 会话累计校正次数（§6 指标）。
    pub corrections: u32,
    /// `drift_ppm` 超 [`RATE_MISMATCH_PPM`]：**不是漂移**，是声明与实际交付对不上
    /// （校正追不上，不会自愈）。消费线程据此一次性上报
    /// [`crate::CaptureDiagnostic::SourceRateMismatch`]。
    pub rate_mismatch: bool,
}

/// 双轨时钟漂移监视器（每采集会话一个，消费线程持有；纯逻辑，headless 可单测）。
///
/// **闭环控制变量 = 16k 输出相对主机时钟的残余偏移**：以采集锚点主机跨度 `T` 为主轴，
/// 输出样本应等于 `TARGET_SAMPLE_RATE·T`。偏移超阈值时以 `set_resample_ratio_relative`
/// 微步（±[`DRIFT_STEP_PPM`]）朝抵消方向调 ratio——超前降 ratio、落后升 ratio；步长克制、
/// 幅度封顶 [`DRIFT_MAX_CORRECTION_PPM`]。积分作用把累计偏移拉回 ~0（含追平启动期已积欠）。
///
/// `drift_ppm`（设备时钟漂移）由采集帧数与主机跨度直接算得（**与校正无关**，物理量，作
/// §6「双轨时钟漂移量」指标）。**pacer 补零源不喂本监视器**（补零已是主机时钟对齐产物）。
#[derive(Debug)]
pub struct DriftMonitor {
    /// 设备标称采样率（Hz），算设备漂移用。
    device_rate: f64,
    /// 当前已施加的相对 ratio（相对基准 ratio；1.0 = 未校正）。
    applied_rel: f64,
    /// 累计校正次数。
    corrections: u32,
}

impl DriftMonitor {
    /// 以设备标称采样率构造。
    #[must_use]
    pub fn new(device_rate: u32) -> Self {
        Self {
            device_rate: f64::from(device_rate),
            applied_rel: 1.0,
            corrections: 0,
        }
    }

    /// 喂入采集锚点主机跨度、真实采集帧数、16k 累计输出样本 → 漂移评估（或 `None`：跨度不足）。
    ///
    /// `device_frames` = 采集交错样本 / 声道数（真实帧，不含 pacer 补零）。
    /// `output_samples` = 归一化 16k mono 累计输出样本。
    pub fn observe(
        &mut self,
        host_span_nanos: u64,
        device_frames: u64,
        output_samples: u64,
    ) -> Option<DriftReport> {
        let span_s = host_span_nanos as f64 / 1e9;
        if span_s < DRIFT_MIN_SPAN_SECS {
            return None;
        }
        // 设备时钟漂移（物理量，与校正无关）。
        let expected_device = self.device_rate * span_s;
        if expected_device <= 0.0 {
            return None;
        }
        let drift_ppm = (device_frames as f64 - expected_device) / expected_device * 1e6;

        // 闭环控制变量：16k 输出相对主机时钟的残余偏移。
        let expected_out = f64::from(TARGET_SAMPLE_RATE) * span_s;
        let offset_ms =
            (output_samples as f64 - expected_out) / f64::from(TARGET_SAMPLE_RATE) * 1e3;

        let apply_rel = if offset_ms.abs() > DRIFT_CORRECT_THRESHOLD_MS {
            let step = DRIFT_STEP_PPM / 1e6;
            // 输出超前（offset>0）→ 降 ratio 出得慢些；落后 → 升 ratio。
            let dir = if offset_ms > 0.0 { -1.0 } else { 1.0 };
            let bound = DRIFT_MAX_CORRECTION_PPM / 1e6;
            let new_rel = (self.applied_rel + dir * step).clamp(1.0 - bound, 1.0 + bound);
            if (new_rel - self.applied_rel).abs() > f64::EPSILON {
                self.applied_rel = new_rel;
                self.corrections += 1;
                Some(new_rel)
            } else {
                None // 已达校正上限，如实上报指标不再猛拉
            }
        } else {
            None
        };

        Some(DriftReport {
            drift_ppm,
            offset_ms,
            apply_rel,
            corrections: self.corrections,
            rate_mismatch: drift_ppm.abs() > RATE_MISMATCH_PPM,
        })
    }

    /// 会话累计校正次数（§6 指标）。
    #[must_use]
    pub fn corrections(&self) -> u32 {
        self.corrections
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn snapshot_none_before_first_sample() {
        let clock = CaptureClock::new(Instant::now());
        assert!(clock.snapshot().is_none(), "尚无数据时快照应为 None");
    }

    #[test]
    fn mark_accumulates_samples_and_sets_anchor_once() {
        let epoch = Instant::now();
        let clock = CaptureClock::new(epoch);
        clock.mark_captured(0); // 空批次不改变状态
        assert!(clock.snapshot().is_none());

        clock.mark_captured(160);
        let first = clock.snapshot().expect("首批次后应有快照");
        assert_eq!(first.produced_samples, 160);
        let anchor = first.first_nanos;

        std::thread::sleep(Duration::from_millis(2));
        clock.mark_captured(160);
        let second = clock.snapshot().unwrap();
        assert_eq!(second.produced_samples, 320);
        assert_eq!(second.first_nanos, anchor, "first_nanos 只在首批次设置一次");
        assert!(
            second.last_nanos >= second.first_nanos,
            "last_nanos 单调不早于 first_nanos"
        );
        assert!(second.host_span_nanos() > 0, "两批次间应有正的主机时间跨度");
    }

    #[test]
    fn drift_report_none_before_min_span() {
        let mut m = DriftMonitor::new(48_000);
        // 跨度 1s < 5s 阈值 → 不出评估。
        assert!(m.observe(1_000_000_000, 48_000, 16_000).is_none());
    }

    #[test]
    fn drift_ppm_reflects_device_clock() {
        let mut m = DriftMonitor::new(48_000);
        // 10s 主机跨度，设备多产 +200ppm 帧 → drift_ppm ≈ +200。
        let span_ns = 10 * 1_000_000_000u64;
        let device_frames = (48_000.0 * 10.0 * 1.0002) as u64;
        let rep = m.observe(span_ns, device_frames, 160_000).unwrap();
        assert!(
            (rep.drift_ppm - 200.0).abs() < 5.0,
            "设备漂移应 ≈ +200ppm（实得 {}）",
            rep.drift_ppm
        );
    }

    /// 模拟恒定漂移设备，跑闭环校正，断言 16k 输出相对主机的累计偏移长时有界。
    fn simulate_convergence(drift_ppm: f64) -> (f64, u32) {
        let r = 48_000.0;
        let base_ratio = f64::from(TARGET_SAMPLE_RATE) / r; // 16k/48k
        let actual_rate = r * (1.0 + drift_ppm / 1e6);
        let mut m = DriftMonitor::new(48_000);
        let mut applied_rel = 1.0;
        let mut prev_device = 0.0f64;
        let mut output = 0.0f64;
        let mut last_offset = 0.0;
        // 180s，每秒一次评估。
        for t in 1..=180u64 {
            let device_total = actual_rate * t as f64;
            let delta_device = device_total - prev_device;
            prev_device = device_total;
            // 本区间用当前 applied_rel 产出 16k 样本。
            output += delta_device * base_ratio * applied_rel;
            let span_ns = t * 1_000_000_000;
            if let Some(rep) = m.observe(span_ns, device_total as u64, output as u64) {
                last_offset = rep.offset_ms;
                if let Some(rel) = rep.apply_rel {
                    applied_rel = rel;
                }
            }
        }
        (last_offset, m.corrections())
    }

    #[test]
    fn correction_bounds_offset_for_positive_drift() {
        let (offset, corrections) = simulate_convergence(200.0);
        assert!(
            offset.abs() < 4.0,
            "+200ppm 校正后累计偏移应有界（实得 {offset}ms）"
        );
        assert!(
            (1..500).contains(&corrections),
            "应发生有限次校正（实得 {corrections}）"
        );
    }

    #[test]
    fn correction_bounds_offset_for_negative_drift() {
        let (offset, corrections) = simulate_convergence(-200.0);
        assert!(
            offset.abs() < 4.0,
            "-200ppm 校正后累计偏移应有界（实得 {offset}ms）"
        );
        assert!(corrections >= 1, "负漂移也应触发校正");
    }

    #[test]
    fn correction_caps_at_max_ppm() {
        // 极端漂移（2000ppm）：单轮 rel 步进受 MAX_CORRECTION 封顶，不猛拉。
        let mut m = DriftMonitor::new(48_000);
        let mut applied = 1.0;
        for t in 1..=200u64 {
            // 输出恒定超前（模拟持续正偏移）：output 远超期望。
            let out = f64::from(TARGET_SAMPLE_RATE) * t as f64 * 1.01;
            if let Some(rep) = m.observe(t * 1_000_000_000, 48_000 * t, out as u64) {
                if let Some(rel) = rep.apply_rel {
                    applied = rel;
                }
            }
        }
        let bound = DRIFT_MAX_CORRECTION_PPM / 1e6;
        assert!(
            applied >= 1.0 - bound - f64::EPSILON,
            "校正幅度不得超过封顶（实得 rel {applied}）"
        );
    }

    /// ADR-082：声明与交付差整数倍时，判定必须是「不符」而不是「漂移」——
    /// 反向验证同处：常规 ppm 级漂移不得被误判成不符。
    #[test]
    fn rate_mismatch_separates_from_real_drift() {
        let mut m = DriftMonitor::new(48_000);
        let span_ns = 10 * 1_000_000_000u64;
        // 真机现场：声明 48k/2ch，实际按 6ch 缓冲计量 → 帧数 3.00×。
        let tripled = m.observe(span_ns, 48_000 * 10 * 3, 160_000).unwrap();
        assert!(
            (tripled.drift_ppm - 2_000_000.0).abs() < 1_000.0,
            "3.00× 应算得 ≈ +2,000,000ppm（实得 {}）",
            tripled.drift_ppm
        );
        assert!(tripled.rate_mismatch, "整数倍率对不上必须判为「不符」");

        let mut m = DriftMonitor::new(48_000);
        let real = m
            .observe(span_ns, (48_000.0 * 10.0 * 1.0002) as u64, 160_000)
            .unwrap();
        assert!(!real.rate_mismatch, "+200ppm 是真实漂移，不得误判为不符");
    }

    #[test]
    fn shared_epoch_makes_two_clocks_comparable() {
        // Both 双轨：共享 epoch → 两轨 first_nanos 可相减得启动偏差。
        let epoch = Instant::now();
        let mic = CaptureClock::new(epoch);
        mic.mark_captured(160);
        std::thread::sleep(Duration::from_millis(3));
        let sys = CaptureClock::new(epoch);
        sys.mark_captured(160);
        let m = mic.snapshot().unwrap().first_nanos;
        let s = sys.snapshot().unwrap().first_nanos;
        assert!(s > m, "晚启动轨 first_nanos 应更大（同一 epoch 基准）");
    }
}
