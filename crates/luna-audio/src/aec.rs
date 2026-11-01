//! AEC3 回声消除（TDD-02 §3.1-I 层③，PRD-04-052 外放场景音频层根除）。
//!
//! **场景**：Both 双轨 + 外放——扬声器播放的对方声音被麦克风拾取，mic 轨（「我」）混入
//! 对方内容 → 实时字幕误标「我」+ 重复转写。层①（耳机提示）/层②（跨轨转写去重）只能
//! 兜底文本层；本模块在**音频层**消除：以 system 轨归一化输出为参考信号（far-end），
//! 在 mic 轨消费线程、feed_writer/VAD **之前**过 AEC——处理后信号三路同源（落盘/波形/ASR
//! 全部受益）。
//!
//! **选型与形态（S13 spike 实测，S13 AEC3 spike，内部记录）**：
//! `webrtc-audio-processing` 2.1（AEC3，BSD-3-Clause，bundled）。关键实测：该 crate 在
//! **16k 输入失效**（ERLE≈3dB，全配置矩阵）而 32k/48k 正常（≈19dB）——故 AEC 在 **32k 域**
//! 运行，本模块内部经 rubato `FftFixedInOut`（精确整数比 2×，带内零混叠）16k↔32k 往返，
//! 管线 16k mono 契约（§3.1-B）不动。封装路径终验 ERLE=26.8dB；far-end 断流（副轨死亡）
//! 时 capture-only 处理近似直通（-0.5dB），无需额外分支即满足「优雅直通」。
//!
//! **线程模型**：`Processor` 为 `Send+Sync`（官方推荐 Arc 跨采集/渲染线程共享）——
//! [`FarEndFeeder`] 在 system 消费线程灌参考帧、[`EchoCanceller`] 在 mic 消费线程处理，
//! 无自建跨线程桥。两者只在**消费线程**运行（绝不进 cpal 回调 / `FrameInjector`，ADR-009）。
//!
//! **平台**：macOS（bundled meson 构建链未在 Windows 验证，批次 6 评估）；非 macOS
//! [`create_echo_cancel_pair`] 诚实返回 `None`（上层直通 + 层②兜底）。
//!
//! **生命周期**：AEC 状态随会话 part 重建（编排层每 part 构造新 pair），不跨 part 复用。

/// AEC 内部处理采样率：16k 在 webrtc-audio-processing 2.1 实测失效、32k 收敛正常
/// （S13 spike 矩阵），故封装内以 32k 域运行。
#[cfg(target_os = "macos")]
const AEC_PROCESS_RATE: u32 = 32_000;

/// 16k 侧处理块（10ms @16k——AEC3 帧长契约 `rate/100`）。
#[cfg(target_os = "macos")]
const AEC_BLOCK_16K: usize = 160;

/// 采集会话的 AEC 接线角色（编排层构造、经采集启动函数注入消费管线）。
///
/// Box 装箱：两角色内部各持重采样器/处理器句柄（数百字节且尺寸悬殊），装箱平衡变体
/// 尺寸（clippy large_enum_variant），每会话一次构造无热路径成本。
pub enum AecRole {
    /// mic 轨：在 feed_writer/VAD 之前对归一化 16k mono 过 AEC（接收端）。
    Capture(Box<EchoCanceller>),
    /// system 轨：归一化 16k mono 输出分叉为参考信号（发送端）。
    Render(Box<FarEndFeeder>),
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{AEC_BLOCK_16K, AEC_PROCESS_RATE};
    use rubato::{FftFixedInOut, Resampler};
    use std::sync::Arc;
    use webrtc_audio_processing::config::{Config, EchoCanceller as EcConfig};
    use webrtc_audio_processing::Processor;

    /// 32k 侧处理块（10ms @32k）。
    const AEC_BLOCK_32K: usize = 320;

    /// 16k→32k 定长上采样 + 按 10ms 块回调的公共攒块器（far-end 与 capture 两侧同构）。
    struct BlockUpsampler {
        up: FftFixedInOut<f32>,
        in16: Vec<f32>,
    }

    impl BlockUpsampler {
        fn new() -> Result<Self, rubato::ResamplerConstructionError> {
            Ok(Self {
                up: FftFixedInOut::new(16_000, AEC_PROCESS_RATE as usize, AEC_BLOCK_16K, 1)?,
                in16: Vec::new(),
            })
        }

        /// 攒入 16k 样本；每满 10ms 块上采样为 32k 块并回调 `f`。重采样失败的块跳过
        /// （极端异常；样本账目由调用方的输出侧守恒逻辑兜底）。
        fn feed(&mut self, mono16k: &[f32], mut f: impl FnMut(Vec<f32>)) {
            self.in16.extend_from_slice(mono16k);
            while self.in16.len() >= AEC_BLOCK_16K {
                let chunk: Vec<f32> = self.in16.drain(..AEC_BLOCK_16K).collect();
                if let Ok(mut out) = self.up.process(&[chunk], None) {
                    if let Some(block32) = out.pop() {
                        f(block32);
                    }
                }
            }
        }

        /// 余量样本数（<10ms，flush 用）。
        fn pending(&self) -> usize {
            self.in16.len()
        }

        /// 余量补零到整块并处理（会话收尾）。
        fn flush(&mut self, f: impl FnMut(Vec<f32>)) {
            if self.in16.is_empty() {
                return;
            }
            let pad = AEC_BLOCK_16K - self.in16.len();
            let tail = vec![0.0f32; pad];
            self.feed(&tail, f);
        }
    }

    /// far-end 参考信号灌入端（system 轨消费线程持有）。
    pub struct FarEndFeeder {
        proc: Arc<Processor>,
        chunker: BlockUpsampler,
    }

    impl FarEndFeeder {
        /// 灌入一批 system 轨归一化 16k mono 样本作为 AEC 参考。
        pub fn push(&mut self, mono16k: &[f32]) {
            let proc = &self.proc;
            self.chunker.feed(mono16k, |block32| {
                // 渲染帧处理失败仅意味着本块参考缺失——AEC3 容忍参考空洞，绝不中断采集。
                let _ = proc.process_render_frame(vec![block32]);
            });
        }
    }

    /// mic 轨回声消除端（mic 轨消费线程持有；在 feed_writer/VAD 之前调用）。
    pub struct EchoCanceller {
        proc: Arc<Processor>,
        chunker: BlockUpsampler,
        down: FftFixedInOut<f32>,
    }

    impl EchoCanceller {
        /// 处理一批归一化 16k mono 样本，返回等量（按 10ms 块粒度滞后 <10ms）的消回声样本。
        pub fn process(&mut self, mono16k: &[f32]) -> Vec<f32> {
            let mut out16: Vec<f32> = Vec::with_capacity(mono16k.len());
            let proc = &self.proc;
            let down = &mut self.down;
            self.chunker.feed(mono16k, |block32| {
                let mut frame = vec![block32];
                // capture 处理失败 → 本块原样直通（诚实降级，绝不丢录音样本）。
                let fell_back = proc.process_capture_frame(&mut frame).is_err();
                let block32 = frame.pop().unwrap_or_default();
                match down.process(std::slice::from_ref(&block32), None) {
                    Ok(mut o) if !fell_back => out16.extend(o.pop().unwrap_or_default()),
                    // 下采样失败或 capture 失败：从 32k 块直接抽偶数样本回 16k
                    //（2× 整数比，内容 ≤8k 带限，抽取无混叠）——样本账目恒守恒。
                    _ => out16.extend(block32.iter().step_by(2)),
                }
            });
            out16
        }

        /// 会话收尾：把不足一块的余量补零处理后返回（截到真实余量长度，样本账目守恒）。
        pub fn flush(&mut self) -> Vec<f32> {
            let pending = self.chunker.pending();
            if pending == 0 {
                return Vec::new();
            }
            let mut out16: Vec<f32> = Vec::with_capacity(AEC_BLOCK_16K);
            let proc = &self.proc;
            let down = &mut self.down;
            self.chunker.flush(|block32| {
                let mut frame = vec![block32];
                let fell_back = proc.process_capture_frame(&mut frame).is_err();
                let block32 = frame.pop().unwrap_or_default();
                match down.process(std::slice::from_ref(&block32), None) {
                    Ok(mut o) if !fell_back => out16.extend(o.pop().unwrap_or_default()),
                    _ => out16.extend(block32.iter().step_by(2)),
                }
            });
            out16.truncate(pending);
            out16
        }
    }

    /// 构造一对共享 AEC3 处理器的（消回声端，参考灌入端）；构造失败诚实返回 `None`
    /// （上层直通，层②转写去重兜底）。
    #[must_use]
    pub fn create_echo_cancel_pair() -> Option<(EchoCanceller, FarEndFeeder)> {
        let proc = Processor::new(AEC_PROCESS_RATE).ok()?;
        proc.set_config(Config {
            // Full AEC3 + 内部延迟估计（delay-agnostic，S13 实测显式 delay 无增益）。
            // 仅开回声消除——NS/AGC 恒关（录音保真；响度对齐在混流收口，TDD-02 §3.1-I）。
            echo_canceller: Some(EcConfig::Full {
                stream_delay_ms: None,
            }),
            ..Config::default()
        });
        let proc = Arc::new(proc);
        let canceller = EchoCanceller {
            proc: Arc::clone(&proc),
            chunker: BlockUpsampler::new().ok()?,
            down: FftFixedInOut::new(AEC_PROCESS_RATE as usize, 16_000, AEC_BLOCK_32K, 1).ok()?,
        };
        let feeder = FarEndFeeder {
            proc,
            chunker: BlockUpsampler::new().ok()?,
        };
        Some((canceller, feeder))
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    /// 编译期空类型：非 macOS 无 AEC 实现，[`create_echo_cancel_pair`] 恒 `None`，
    /// 本类型不可构造——方法体经空 match 编译期证明不可达（无运行时兜底假装）。
    enum Void {}

    /// far-end 参考信号灌入端（非 macOS 不可构造）。
    pub struct FarEndFeeder(Void);

    impl FarEndFeeder {
        /// 灌入参考样本（不可达）。
        pub fn push(&mut self, _mono16k: &[f32]) {
            match self.0 {}
        }
    }

    /// mic 轨回声消除端（非 macOS 不可构造）。
    pub struct EchoCanceller(Void);

    impl EchoCanceller {
        /// 处理采集样本（不可达）。
        pub fn process(&mut self, _mono16k: &[f32]) -> Vec<f32> {
            match self.0 {}
        }

        /// 收尾余量（不可达）。
        pub fn flush(&mut self) -> Vec<f32> {
            match self.0 {}
        }
    }

    /// 非 macOS：AEC 依赖（bundled meson 构建链）未验证，诚实无此能力（批次 6 评估）。
    #[must_use]
    pub fn create_echo_cancel_pair() -> Option<(EchoCanceller, FarEndFeeder)> {
        None
    }
}

pub use imp::{create_echo_cancel_pair, EchoCanceller, FarEndFeeder};

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    fn noise(n: usize, seed: u64) -> Vec<f32> {
        let mut rng = seed;
        (0..n)
            .map(|_| {
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                (rng as f64 / u64::MAX as f64) as f32 - 0.5
            })
            .collect()
    }

    fn energy(s: &[f32]) -> f64 {
        s.iter().map(|&x| (x as f64) * (x as f64)).sum()
    }

    /// 合成「远端信号 + 衰减延迟副本」→ AEC 后残余能量显著低于直通（批次任务 2 验收）。
    #[test]
    fn cancels_synthetic_echo_significantly() {
        let (mut canceller, mut feeder) = create_echo_cancel_pair().expect("macOS 应可构造");
        let n = 16_000 * 20;
        let far = noise(n, 0x243F_6A88_85A3_08D3);
        let delay = 320; // 20ms @16k（外放声学+缓冲延迟量级）
        let echo: Vec<f32> = (0..n)
            .map(|i| {
                if i >= delay {
                    far[i - delay] * 0.4
                } else {
                    0.0
                }
            })
            .collect();

        let (mut in_e, mut out_e) = (0.0f64, 0.0f64);
        let tail = n / 2; // 后半段（已收敛）统计
        for f in 0..n / AEC_BLOCK_16K {
            let lo = f * AEC_BLOCK_16K;
            let hi = lo + AEC_BLOCK_16K;
            feeder.push(&far[lo..hi]);
            let out = canceller.process(&echo[lo..hi]);
            if lo >= tail {
                in_e += energy(&echo[lo..hi]);
                out_e += energy(&out);
            }
        }
        let erle_db = 10.0 * (in_e / out_e.max(1e-12)).log10();
        assert!(
            erle_db > 10.0,
            "AEC 后残余能量应显著低于直通（ERLE>10dB，实得 {erle_db:.1}dB）"
        );
    }

    /// far-end 断流（副轨死亡降级）：capture-only 处理近似直通，不伤近端语音。
    #[test]
    fn render_starved_capture_is_near_passthrough() {
        let (mut canceller, _feeder) = create_echo_cancel_pair().expect("macOS 应可构造");
        let speech = noise(16_000 * 5, 0x1234_5678_9ABC_DEF0);
        let (mut in_e, mut out_e) = (0.0f64, 0.0f64);
        for chunk in speech.chunks(AEC_BLOCK_16K) {
            let out = canceller.process(chunk);
            in_e += energy(chunk);
            out_e += energy(&out);
        }
        let delta_db = 10.0 * (out_e / in_e.max(1e-12)).log10();
        assert!(
            delta_db.abs() < 3.0,
            "无参考时近端语音应近似直通（|Δ|<3dB，实得 {delta_db:.1}dB）"
        );
    }

    /// 样本账目守恒：变长帧输入，process+flush 总输出样本数 == 总输入（回放 seek 命门）。
    #[test]
    fn sample_count_is_conserved_across_variable_frames() {
        let (mut canceller, _feeder) = create_echo_cancel_pair().expect("macOS 应可构造");
        let input = noise(16_000, 0xDEAD_BEEF_CAFE_F00D);
        let mut total_out = 0usize;
        // 变长帧（质数步长，覆盖跨块边界）。
        for chunk in input.chunks(367) {
            total_out += canceller.process(chunk).len();
        }
        total_out += canceller.flush().len();
        assert_eq!(total_out, input.len(), "AEC 不得增减样本");
    }
}
