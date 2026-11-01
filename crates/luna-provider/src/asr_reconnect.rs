//! 实时转写的**静默重连窗**与重放缓冲（ADR-038 ⑧）。
//!
//! # 为什么需要它
//!
//! ADR-038 ⑤ 的降级是**单向**的：切到本机之后就再也不切回来（`asr_failover` 里
//! `on_fallback` 一旦置位就不再复位——那是对的，来回横跳会让同一场会议的转写质量
//! 反复变化）。代价是**瞬断的代价被放大成整场**：地铁进隧道、Wi-Fi 漫游、服务端滚动
//! 重启，任何一次几百毫秒的断开都会让剩下一小时全部跑在本机最小档上。
//!
//! 故在降级**之前**插一段窗：断了先安静地重连，窗内接上就当无事发生，超窗才交给
//! 既有的 failover。ADR-038 ⑤「降级不是可选项」一个字不动——本条只是把「什么时候
//! 算断了」从「第一个错误」改成「重连窗关了」。
//!
//! # 三件必须同时成立的事
//!
//! 1. **窗内接得上** —— 指数退避，见 [`backoff_delay`]；
//! 2. **接上之后不丢音频** —— 断线期间采集侧还在说话，[`ReplayLog`] 攒着；接上后先
//!    重放水位线之后的包，再继续发新的；
//! 3. **`committed` 不回退** —— 服务端时间戳**每个连接从零重计**。重放的那段音频
//!    在新连接上会被重新解一遍，如果不做偏移校正，第二次的「0.0 秒」会和第一次的
//!    「0.0 秒」撞在一起，`CommittedLedger` 会把整场会话的开头再放行一遍。偏移由
//!    [`frame_offset`] 从帧序号算出——与重放用的是**同一个**计数器，故不存在
//!    「重放对齐了、时间戳没对齐」。

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::opus_uplink::{UplinkPacket, FRAME_DURATION};

/// 静默重连窗的总时长。
///
/// 取 5 秒的依据是**两侧各有一个下界**：往下，常见的瞬断（Wi-Fi 漫游、蜂窝切换、
/// 服务端滚动重启的一个 pod）都在 1–3 秒量级，窗短于它等于这条机制不生效；往上，
/// 窗内没有云端字幕产出，用户看到的是「重连中」而不是文字——超过五六秒，本机兜底
/// 立刻接管才是他要的（本机 whisper 恒在，接管代价只是模型档位下降）。
pub const RECONNECT_WINDOW: Duration = Duration::from_secs(5);

/// 退避序列（毫秒），逐次翻倍。
///
/// **首跳 250 ms 而不是「立刻重试」**：断连的瞬间对端往往还没恢复（TCP 半关、
/// 反代正在换后端），零延迟重试只会在同一个坏状态上连撞几次，把窗白白烧掉。
/// 翻倍是为了让窗内既有「快速接上」的机会（前两跳），又不至于在对端确实没起来时
/// 高频空转。序列总睡眠 3.75 秒 < 窗长，剩下的余量留给建连本身。
const BACKOFF_MS: [u64; 4] = [250, 500, 1_000, 2_000];

/// 重放缓冲的容量（帧）。
///
/// 有界是必须的：这条缓冲装的是**服务端还没认下**的音频，而「还没认下」的时长
/// 由上游决定，不由我们决定。ADR-038 ⑦ 的真机数字里，CPU 后端的 `small` 档首个
/// committed 要 29 秒——上界必须盖得住那个量级，否则一次正常的慢解码就会让缓冲
/// 自己把待重放的音频丢光。取 60 秒（3000 帧 × 约 50 字节 ≈ 150 KB，一次会话的
/// 内存代价可以忽略）。
///
/// 满了丢最老的：丢掉的那段在重连后不会被转写，结果里缺一小节——用户看得见；
/// 而无界会把内存吃到崩，那是无声的（与网关上行队列同一条纪律）。
const REPLAY_CAPACITY_FRAMES: usize = (60 * 1000 / 20) as usize;

/// 第 `attempt` 次重试（从 1 起算）之前该等多久；序列用尽返回 `None`。
#[must_use]
pub fn backoff_delay(attempt: u32) -> Option<Duration> {
    BACKOFF_MS
        .get((attempt as usize).checked_sub(1)?)
        .map(|ms| Duration::from_millis(*ms))
}

/// 纯判定：已经过了 `elapsed`、正要发起第 `attempt` 次重试时，还该不该等、等多久。
///
/// 抽成纯函数是为了让「窗关了」这一支能被单测钉住——埋在 `Instant` 里的话，那一支
/// 只有真的等满五秒才验得到，于是没人会验它。
#[must_use]
pub fn next_delay(attempt: u32, elapsed: Duration) -> Option<Duration> {
    let remaining = RECONNECT_WINDOW.checked_sub(elapsed)?;
    if remaining.is_zero() {
        return None;
    }
    // 退避不得把睡眠拖出窗外：拖出去的话，「5 秒后交给本机」会变成「6.6 秒后」，
    // 而那 1.6 秒里用户既没有云端字幕、也没有本机字幕。
    Some(backoff_delay(attempt)?.min(remaining))
}

/// 一次静默重连窗的进行态。
///
/// 只是 [`next_delay`] 外面的一层时钟——判定逻辑全在那个纯函数里。
pub struct ReconnectWindow {
    opened_at: Instant,
    attempt: u32,
}

impl ReconnectWindow {
    /// 开窗（断线的那一刻）。
    #[must_use]
    pub fn open() -> Self {
        Self {
            opened_at: Instant::now(),
            attempt: 0,
        }
    }

    /// 还剩多少窗——重连的建连动作必须被它封顶，否则一个挂住的 TCP 连接能把整个窗吃掉。
    #[must_use]
    pub fn remaining(&self) -> Duration {
        RECONNECT_WINDOW.saturating_sub(self.opened_at.elapsed())
    }

    /// 推进到下一次重试：返回该等的时长；`None` = 窗关了，交给 failover。
    pub fn step(&mut self) -> Option<Duration> {
        self.attempt += 1;
        next_delay(self.attempt, self.opened_at.elapsed())
    }
}

/// 帧序号 → 会话上行时间轴上的时刻。
#[must_use]
pub fn frame_offset(frame: u64) -> Duration {
    FRAME_DURATION * u32::try_from(frame).unwrap_or(u32::MAX)
}

/// 已定稿覆盖到 `committed_end` 时，重放该从第几帧开始。
///
/// 向下取整：宁可多重放不到一帧的音频（服务端会按 [`crate::asr_realtime`] 的账本
/// 判重、下游看不到重复），也不要少发——少发的那一小节是真的丢了。
#[must_use]
pub fn watermark_frame(committed_end: Duration) -> u64 {
    (committed_end.as_millis() / FRAME_DURATION.as_millis()) as u64
}

/// 待重放的上行包（水位线之后、服务端尚未认下的那些）。
#[derive(Default)]
pub struct ReplayLog {
    packets: VecDeque<UplinkPacket>,
}

impl ReplayLog {
    /// 记下一个刚发出（或断线期间刚编出）的包。超出容量即丢最老的。
    pub fn record(&mut self, packet: UplinkPacket) {
        if self.packets.len() == REPLAY_CAPACITY_FRAMES {
            self.packets.pop_front();
        }
        self.packets.push_back(packet);
    }

    /// 服务端认下到 `frame` 了：这之前的不必再留。
    pub fn retain_from(&mut self, frame: u64) {
        while self.packets.front().is_some_and(|p| p.frame < frame) {
            self.packets.pop_front();
        }
    }

    /// 待重放的第一帧序号 —— 重连后新连接的时间戳零点就是它。
    #[must_use]
    pub fn first_frame(&self) -> Option<u64> {
        self.packets.front().map(|p| p.frame)
    }

    /// 待重放的包，按序。
    pub fn pending(&self) -> impl Iterator<Item = &UplinkPacket> {
        self.packets.iter()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.packets.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.packets.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(frame: u64) -> UplinkPacket {
        UplinkPacket {
            frame,
            bytes: vec![0u8; 50],
        }
    }

    /// 退避序列必须与声明一致：逐次翻倍、总睡眠落在窗内。
    ///
    /// 这条钉的是「参数只有一份」——把窗长改短而忘了改序列，表现是最后一跳被
    /// 截断到几毫秒（等于连撞两次），而没有任何东西会因此变红。
    #[test]
    fn the_backoff_doubles_and_fits_inside_the_window() {
        let delays: Vec<Duration> = (1..).map_while(backoff_delay).collect();
        assert_eq!(delays.len(), BACKOFF_MS.len());
        for pair in delays.windows(2) {
            assert_eq!(pair[1], pair[0] * 2, "退避不是逐次翻倍：{delays:?}");
        }
        let total: Duration = delays.iter().sum();
        assert!(
            total < RECONNECT_WINDOW,
            "退避总睡眠 {total:?} 已经填满 {RECONNECT_WINDOW:?} 的窗，\
             一次建连的时间都不剩了"
        );
    }

    /// 窗的边界：已经用满就不再等，且最后一跳不得把睡眠拖出窗外。
    #[test]
    fn the_window_is_a_hard_ceiling() {
        assert_eq!(
            next_delay(1, Duration::ZERO),
            Some(Duration::from_millis(250))
        );
        assert_eq!(
            next_delay(4, RECONNECT_WINDOW - Duration::from_millis(300)),
            Some(Duration::from_millis(300)),
            "退避序列说等 2 秒，而窗只剩 300 ms——必须按窗截断"
        );
        assert_eq!(next_delay(1, RECONNECT_WINDOW), None, "窗用满仍在等");
        assert_eq!(
            next_delay(1, RECONNECT_WINDOW + Duration::from_secs(1)),
            None
        );
        assert_eq!(
            next_delay(5, Duration::ZERO),
            None,
            "序列用尽即超窗——不得凭空再来一跳"
        );
    }

    /// 时间戳偏移与重放起点出自同一个计数器。
    #[test]
    fn the_offset_and_the_replay_start_agree() {
        assert_eq!(frame_offset(0), Duration::ZERO);
        assert_eq!(frame_offset(50), Duration::from_secs(1));
        // 服务端认下到 1.0 秒 → 第 50 帧之前的都不必重放，新连接的零点正是 1.0 秒。
        let w = watermark_frame(Duration::from_secs(1));
        assert_eq!(w, 50);
        assert_eq!(frame_offset(w), Duration::from_secs(1));
        // 向下取整：定稿停在半帧上时宁可多重放半帧。
        assert_eq!(watermark_frame(Duration::from_millis(1_010)), 50);
    }

    /// 水位线推进即释放缓冲；重放起点跟着走。
    #[test]
    fn the_watermark_releases_what_the_server_already_took() {
        let mut log = ReplayLog::default();
        for f in 0..100 {
            log.record(packet(f));
        }
        assert_eq!(log.first_frame(), Some(0));
        log.retain_from(watermark_frame(Duration::from_secs(1)));
        assert_eq!(log.len(), 50);
        assert_eq!(log.first_frame(), Some(50));
        assert_eq!(log.pending().next().map(|p| p.frame), Some(50));
    }

    /// 有界：上游久不定稿时缓冲丢最老的，绝不无界增长。
    #[test]
    fn an_upstream_that_never_commits_cannot_grow_the_buffer() {
        let mut log = ReplayLog::default();
        for f in 0..(REPLAY_CAPACITY_FRAMES as u64 + 500) {
            log.record(packet(f));
        }
        assert_eq!(log.len(), REPLAY_CAPACITY_FRAMES);
        assert_eq!(log.first_frame(), Some(500), "丢的必须是最老的那些");
    }

    /// 容量要盖得住 ADR-038 ⑦ 实测的定稿滞后（CPU 后端 small 档约 29 秒）。
    #[test]
    fn the_buffer_outlasts_a_slow_upstream() {
        let covered = FRAME_DURATION * u32::try_from(REPLAY_CAPACITY_FRAMES).unwrap();
        assert!(
            covered >= Duration::from_secs(30),
            "重放缓冲只盖得住 {covered:?}，而真机上定稿滞后可达 29 秒——\
             一次正常的慢解码就会把待重放的音频丢光"
        );
    }
}
