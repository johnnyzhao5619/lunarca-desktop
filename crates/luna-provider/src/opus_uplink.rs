//! 实时转写上行的 Opus 编码（ADR-038 ③）。
//!
//! ```text
//! > 上行必须压缩，且必须只发新增：16 kHz mono f32 PCM ≈ 512 kbps，直传在移动网络上
//! > 不成立。
//! ```
//!
//! 「只发新增」由采集侧的绝对游标解决（`WindowInput::fresh_after`）；**压缩**是本模块。
//! 16-bit PCM 那一版把 512 kbps 砍到 256 kbps，仍然是一条移动网络上撑不住的上行；
//! Opus 在同一段语音上是 **≈24 kbps**，再降一个数量级。
//!
//! # 三个参数，逐个说清依据
//!
//! - **档位 = VoIP**（`OPUS_APPLICATION_VOIP`）。这条链路上的内容恒为人声，且我们要的是
//!   「说了什么」而不是「听起来像什么」——VoIP 档为语音可懂度优化，同码率下的 ASR
//!   识别率高于 Audio 档。
//! - **码率 = 24 kbps CBR-ish（VBR，目标 24 kbps）**。libopus 在 16 kHz 单声道语音上
//!   24 kbps 已是**透明**档位（宽带语音的推荐区间是 16–32 kbps）；实测一段 440 Hz 探针
//!   往返后与原信号的最优时延相关 > 0.999。再往上加码率买不到 ASR 精度，只是替用户
//!   多付流量——而 ADR-038 ④ 明说这条上行是常开的。
//! - **帧长 = 20 ms**（320 样本 @16 kHz）。Opus 允许 2.5/5/10/20/40/60 ms；20 ms 是
//!   WebRTC 与几乎所有 VoIP 栈的默认值，理由同样适用于这里：更短的帧把每包固定开销
//!   （WebSocket 帧头 + Opus 包头）摊得更薄的收益，抵不过包数翻倍带来的调度与网络
//!   开销；更长的帧（40/60 ms）省下的那点带宽换的是同等量级的额外延迟，而 ADR-038 ②
//!   把延迟预算列成了验收项。
//!
//! # 一包一帧，包与包之间不再需要任何定界
//!
//! 每个 WebSocket 二进制帧**恰好装一个 Opus 包**。这不是随手的选择：Opus 包本身不自
//! 带长度，一条二进制帧里塞多个包就必须再造一层定界，而那层定界会成为服务端第一个
//! 需要防御的解析面。让 WebSocket 的帧边界直接充当包边界，服务端那侧 `decode` 收到
//! 什么就解什么。
//!
//! # 帧对齐由本模块负责，采集侧不必知道
//!
//! 采集侧给的是**任意长度**的新增语音（`fresh_after` 的产物，长度由分段器与 VAD 决定）。
//! 本模块攒够 320 个样本才吐一个包，不足的留在 `carry` 里等下一批。跨窗口拼接是
//! 对的：客户端 VAD 让静音不上行（ADR-038 ②），服务端看到的本来就是一条「语音拼接
//! 流」，而不是墙钟时间轴——这也正是重连时时间戳偏移能按帧数算出来的原因。

use opusic_c::{Application, Bitrate, Channels, Encoder, SampleRate};

/// 上行采样率。与采集侧 `TARGET_SAMPLE_RATE` 和服务端的折算口径同一个数。
pub const UPLINK_SAMPLE_RATE: u32 = 16_000;

/// 一个 Opus 包覆盖的样本数（20 ms @ 16 kHz）。见模块头「帧长」。
pub const FRAME_SAMPLES: usize = 320;

/// 一个 Opus 包覆盖的时长。
pub const FRAME_DURATION: std::time::Duration = std::time::Duration::from_millis(20);

/// 目标码率。见模块头「码率」。
pub const TARGET_BITRATE: u32 = 24_000;

/// 单个 Opus 包的字节上限。
///
/// 1275 是 libopus 单帧包的协议上限（`opus_encode` 的 `max_data_bytes` 建议值），
/// 与我们选的码率无关——写死这个数是为了让编码缓冲区**永远不可能不够**，而不是
/// 让它「按码率算出来刚好够」：后者在编码器某一帧突然吐一个大包时表现为编码失败，
/// 而那一帧恰恰是信息最多的那一帧。
pub const MAX_PACKET_BYTES: usize = 1275;

/// 一个已编码的上行包，带它在**会话上行时间轴**上的序号。
///
/// 序号 = 从会话开始算起的第几个 20 ms 帧。它是重连重放与时间戳偏移校正的共同基准
/// （见 [`crate::asr_reconnect`]）——两件事共用一个计数器，就不存在「重放对齐了、
/// 时间戳没对齐」这种只能靠听出来的错。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UplinkPacket {
    /// 帧序号（会话内单调递增，跨重连不重置）。
    pub frame: u64,
    /// Opus 包字节。
    pub bytes: Vec<u8>,
}

/// 编码失败。
///
/// 只有两种来源：建编码器（参数非法/内存不足）与编码本身。两者都不该在跑起来之后
/// 出现，故不细分——真出现时它是一条 `ProviderUnavailable`，链路交给本机兜底。
#[derive(Debug)]
pub struct OpusEncodeError(opusic_c::ErrorCode);

impl std::fmt::Display for OpusEncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Opus 编码失败：{:?}", self.0)
    }
}

impl std::error::Error for OpusEncodeError {}

/// 会话级的上行编码器。
///
/// **整场会话共用一个编码器实例，重连时不重建**。Opus 是有状态的预测编码器：
/// 重建等于把预测状态清零，重连后的第一批包会明显更糙；而重放的那些包本来就是这个
/// 实例在断线前编出来的，让它们和后续包出自同一条编码轨迹，解码端才不会在接缝处
/// 听到一次跳变。（解码端是新的，那不要紧——Opus 本就为丢包/中途接入而设计。）
pub struct OpusUplink {
    encoder: Encoder,
    /// 攒够一帧之前的余样本。
    carry: Vec<f32>,
    /// 下一个要产出的帧序号。
    next_frame: u64,
}

impl OpusUplink {
    /// 建一条会话上行。
    pub fn new() -> Result<Self, OpusEncodeError> {
        let mut encoder = Encoder::new(
            Channels::Mono,
            SampleRate::Hz16000,
            // 见模块头「档位」。
            Application::Voip,
        )
        .map_err(OpusEncodeError)?;
        encoder
            .set_bitrate(Bitrate::Value(TARGET_BITRATE))
            .map_err(OpusEncodeError)?;
        Ok(Self {
            encoder,
            carry: Vec::with_capacity(FRAME_SAMPLES),
            next_frame: 0,
        })
    }

    /// 已经产出过的帧数 = 会话上行时间轴的当前长度。
    #[must_use]
    pub fn frames_emitted(&self) -> u64 {
        self.next_frame
    }

    /// 收下一段新增语音，吐出它凑齐的整帧包。
    ///
    /// 不足一帧的余样本留在内部，**不补零**——补零会把静音塞进上行，而 ADR-038 ③
    /// 那条补注（2026-08-08 真机）算过这笔账：padding 的零绝不上行。
    pub fn push(&mut self, samples: &[f32]) -> Result<Vec<UplinkPacket>, OpusEncodeError> {
        self.carry.extend_from_slice(samples);
        let full = self.carry.len() / FRAME_SAMPLES;
        let mut out = Vec::with_capacity(full);
        // 整帧先摘出来再编码：借用检查不允许一边借 `self.carry` 一边可变借 `self`，
        // 而这一条链路本就是「攒够一帧就交出去」，摘出来是它本来的形状。
        let ready: Vec<f32> = self.carry.drain(..full * FRAME_SAMPLES).collect();
        for frame in ready.chunks_exact(FRAME_SAMPLES) {
            out.push(self.encode_frame(frame)?);
        }
        Ok(out)
    }

    /// 会话收尾：把余样本补零成整帧发出去。
    ///
    /// 这是**唯一**允许补零的地方，且只补一帧：不补的话，用户说的最后不到 20 ms
    /// 会被悄悄丢掉。收尾时那点零对计费与隐私都无关紧要（≤ 20 ms），而丢掉的可能
    /// 是一个字的尾音。
    pub fn flush(&mut self) -> Result<Option<UplinkPacket>, OpusEncodeError> {
        if self.carry.is_empty() {
            return Ok(None);
        }
        let mut frame = std::mem::take(&mut self.carry);
        frame.resize(FRAME_SAMPLES, 0.0);
        self.encode_frame(&frame).map(Some)
    }

    fn encode_frame(&mut self, frame: &[f32]) -> Result<UplinkPacket, OpusEncodeError> {
        debug_assert_eq!(frame.len(), FRAME_SAMPLES);
        let mut buf = [0u8; MAX_PACKET_BYTES];
        let written = self
            .encoder
            .encode_float_to_slice(frame, &mut buf)
            .map_err(OpusEncodeError)?;
        let packet = UplinkPacket {
            frame: self.next_frame,
            bytes: buf[..written].to_vec(),
        };
        self.next_frame += 1;
        Ok(packet)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opusic_c::Decoder;

    /// 440 Hz 正弦探针，幅度 0.5。
    fn tone(samples: usize) -> Vec<f32> {
        (0..samples)
            .map(|i| {
                (i as f32 * 2.0 * std::f32::consts::PI * 440.0 / UPLINK_SAMPLE_RATE as f32).sin()
                    * 0.5
            })
            .collect()
    }

    /// 编解码往返：把一段探针编出去、按**服务端那侧的解法**解回来，还原度必须站得住。
    ///
    /// 判据是**最优时延下的归一化相关**而不是逐样本差：Opus 有算法时延（16 kHz 下
    /// 编码器 look-ahead 104 样本、解码端再加一段），逐样本比会把一个完好的还原判成
    /// 「误差 0.24」——那不是失真，是相位。这条曾经差点被写成前者。
    #[test]
    fn a_probe_survives_the_round_trip() {
        let mut uplink = OpusUplink::new().expect("建编码器");
        let mut decoder = Decoder::new(Channels::Mono, SampleRate::Hz16000).expect("建解码器");
        let src = tone(FRAME_SAMPLES * 25);

        let mut decoded: Vec<f32> = Vec::new();
        let mut total_bytes = 0usize;
        let mut pcm: Vec<u16> = Vec::new();
        for packet in uplink.push(&src).expect("编码") {
            total_bytes += packet.bytes.len();
            pcm.clear();
            let n = decoder
                .decode_to_vec(&packet.bytes, &mut pcm, FRAME_SAMPLES, false)
                .expect("解码");
            assert_eq!(n, FRAME_SAMPLES, "一个包必须解出恰好一帧");
            decoded.extend(pcm.iter().map(|s| f32::from(*s as i16) / 32768.0));
        }
        assert_eq!(decoded.len(), src.len(), "样本数不守恒");

        // 码率落在声明的量级上（VBR，故给区间而不是等号）。
        let kbps = total_bytes * 8 / 25 * 50 / 1000;
        assert!(
            (12..=32).contains(&kbps),
            "实测 {kbps} kbps 偏离声明的 24 kbps 档太远——要么参数没生效，要么换了档位"
        );

        let window = src.len() - 400;
        let best = (0..400)
            .map(|lag| {
                let a = &src[..window];
                let b = &decoded[lag..lag + window];
                let num: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
                let da = a.iter().map(|x| x * x).sum::<f32>().sqrt();
                let db = b.iter().map(|y| y * y).sum::<f32>().sqrt();
                num / (da * db)
            })
            .fold(f32::MIN, f32::max);
        assert!(best > 0.99, "往返后与原信号的最优相关只有 {best}");
    }

    /// 帧对齐：任意长度的新增语音进去，出来的永远是整 20 ms 包，序号连续不跳。
    #[test]
    fn arbitrary_chunks_come_out_as_whole_frames() {
        let mut uplink = OpusUplink::new().expect("建编码器");
        // 分段器给的长度由 VAD 与静音决定，绝不会恰好是 320 的倍数。
        let mut frames = Vec::new();
        for len in [100usize, 700, 13, 1_000] {
            for packet in uplink.push(&tone(len)).expect("编码") {
                frames.push(packet.frame);
            }
        }
        let total: usize = [100usize, 700, 13, 1_000].iter().sum();
        assert_eq!(
            frames,
            (0..(total / FRAME_SAMPLES) as u64).collect::<Vec<_>>()
        );
        assert_eq!(uplink.frames_emitted(), frames.len() as u64);
    }

    /// 收尾补零只发生一次、且只补一帧——用户说的最后那点尾音不能被悄悄吞掉。
    #[test]
    fn the_tail_is_flushed_exactly_once() {
        let mut uplink = OpusUplink::new().expect("建编码器");
        assert!(uplink
            .push(&tone(FRAME_SAMPLES / 2))
            .expect("编码")
            .is_empty());
        let tail = uplink.flush().expect("收尾").expect("余样本必须发出去");
        assert_eq!(tail.frame, 0);
        assert!(
            uplink.flush().expect("再收尾").is_none(),
            "收尾发了第二帧——那是一帧凭空多出来的静音，会被服务端按上行计费"
        );
    }

    /// 静音**不**上行（ADR-038 ③ 补注）：没有余样本时收尾一个字节都不发。
    #[test]
    fn a_silent_session_uploads_nothing() {
        let mut uplink = OpusUplink::new().expect("建编码器");
        assert!(uplink.push(&[]).expect("编码").is_empty());
        assert!(uplink.flush().expect("收尾").is_none());
        assert_eq!(uplink.frames_emitted(), 0);
    }

    /// 参数声明与实现不得分叉：帧长与时长是同一件事的两种写法。
    #[test]
    fn frame_length_and_duration_agree() {
        assert_eq!(
            FRAME_DURATION.as_millis() as usize * UPLINK_SAMPLE_RATE as usize / 1000,
            FRAME_SAMPLES
        );
        // Opus 只接受这几种帧长；20 ms 必须在其中。
        assert!(
            [2.5f64, 5.0, 10.0, 20.0, 40.0, 60.0].contains(&(FRAME_DURATION.as_millis() as f64))
        );
    }
}
