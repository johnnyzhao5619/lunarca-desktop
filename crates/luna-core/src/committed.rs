//! 「已定稿前缀永不回退」的执行点（ADR-038 ⑤/⑧）。
//!
//! ```text
//! > `committed` 一旦发出永不回退这条不变量**必须跨降级保持**——云端已提交的前缀
//! > 不能因为切回本地而被重写。
//! ```
//!
//! # 为什么住在领域层
//!
//! 这条不变量现在有**两个**执行点，而它们必须给出逐字相同的结论：
//!
//! 1. `luna_provider::asr_failover` —— 云腿断了切本机，本机从头重解，前半场会重来一遍；
//! 2. `luna_provider::asr_realtime` —— 静默重连窗内接上了同一条云腿，**服务端的时间戳
//!    每个连接从零重计**，于是重连后的第一帧看起来像是「整场会话又开始了」。
//!
//! 两处各写一份的话，两份迟早分叉——而分叉的表现是「同一句话在字幕里出现两次」或者
//! 「用户已经复制走的一段被改写」，都属于**只在弱网真机上才现形**的那一类。
//!
//! # 判据是**绝对时间**，不是条数
//!
//! 两条腿是两个引擎、两次解码，切分点不一样，「第几段」在它们之间没有可比性。
//! 段带着会话绝对时间（重连后由调用方先做偏移校正，见
//! `luna_provider::asr_reconnect`），故时间轴同源、可比。
//!
//! 完全落在已发区间内的段丢弃；**跨过边界的段保留**。这是有意的取舍：接缝处重复几个
//! 词是用户看得见、能自己删的；而为了消灭重复去丢一整段，丢掉的是真的说过的话，且
//! 没有任何东西会提示他。
//!
//! # 跨重连：绝对时间挡不住，要叠一层**文本判据**（ADR-065）
//!
//! 上面「按绝对时间判重」在**单条连接内**是对的，跨重连却有一个洞。重连后调用方按
//! 「上次已定稿末端」做偏移校正（`base`，见 `luna_provider::asr_reconnect`）。服务端每条
//! 连接从零重计、又**重解同一段重放音频**，于是重解出的**相同文本**经 `shift(base)` 会落到
//! 「更晚」的绝对区间——`end` 每次都 > `self.end`，单靠时间戳每重连一次就把同一句放行一遍
//! （真机：2 秒的重连周期把一句话复读成十几条独立字幕）。
//!
//! 故 [`Self::begin_resume`] 置位后，`admit` 在绝对时间之上再叠一层：凡文本已被**近期已定稿
//! 尾部**覆盖的段一律丢弃（[`crate::text_norm`] 归一化后判包含）。三条边界钉死：
//! ① 单会话内的既有单调判据不变（`begin_resume` 不置位就是老路径）；② 只对「与已定稿尾部
//! 重复」的段去重，一旦放行了确属新的内容就复位、回到纯时间判据；③ **不误杀真新语音**——
//! 只丢「被尾部完全覆盖」的段（用户真又说一遍同一句仍会带来新音频、落在尾部窗口之外，
//! 且那是新连接上的新内容而非重放同一段音频），骑跨边界、带新内容的段照旧保留。

use crate::result::Segment;
use crate::text_norm::normalize_text;
use std::time::Duration;

/// 跨重连文本判重只回看**近期**已定稿尾部这么久。
///
/// 重连重放的重解只会重复「服务端还没认下 / 刚认下」的那一小段，不会重复整场会话的开头；
/// 回看太远会把「用户很久以前说过的同一句」也判成重复丢掉（越过边界③）。取 60 秒——盖得住
/// 重放缓冲的上界（`luna_provider::asr_reconnect` 的容量同量级），又不至于回溯到无关历史。
const RESUME_TAIL_LOOKBACK: Duration = Duration::from_secs(60);

/// 参与跨重连文本判重的最短归一化文本长度（字符）。
///
/// 更短的段（一两个字）跳过文本判据、只走时间判据：把「的」「是」这类字判成重复丢掉的
/// 代价，比偶尔漏删一个极短重复段更糟（同模块头「宁可接缝重复也不丢真内容」的取舍）。
const RESUME_MIN_DEDUP_CHARS: usize = 2;

/// 已发出的定稿账本。
#[derive(Debug, Default)]
pub struct CommittedLedger {
    segments: Vec<Segment>,
    /// 已发出的定稿覆盖到的最晚时刻。
    end: Duration,
    /// 重连后置位：下一（几）批定稿是**重解重放音频**，`admit` 除绝对时间外再叠文本判重。
    /// 放行了确属新的内容即复位（见模块头「跨重连」一节）。
    resuming: bool,
}

impl CommittedLedger {
    /// 收下这一批定稿，返回**可以发出去的那些**（调用方只把这些交给下游）。
    pub fn admit(&mut self, incoming: Vec<Segment>) -> Vec<Segment> {
        // 单会话内的既有判据：绝对时间单调，只放行末端更晚的段（跨边界的段保留）。
        let time_fresh = incoming.into_iter().filter(|s| s.end > self.end);
        let fresh: Vec<Segment> = if self.resuming {
            // 跨重连：先算出近期已定稿尾部（归一化），再丢掉被它覆盖的重解重复段。
            let tail = self.recent_committed_norm();
            time_fresh
                .filter(|s| !text_covered_by_tail(&tail, &s.text))
                .collect()
        } else {
            time_fresh.collect()
        };
        if let Some(latest) = fresh.iter().map(|s| s.end).max() {
            self.end = latest;
        }
        // 只有在放行了确属新的内容之后才退出 resume：若这一批全是重复（`fresh` 为空），
        // 说明重放的重解区间还没越过，下一批仍要按文本判重（否则复读会漏网）。
        if self.resuming && !fresh.is_empty() {
            self.resuming = false;
        }
        self.segments.extend(fresh.iter().cloned());
        fresh
    }

    /// 重连后调用：声明「下一（几）批定稿是重解重放音频」，`admit` 据此叠加跨重连文本判重。
    ///
    /// 只在**重连**（`base` 变化）后置位，不在单会话正常收发时置位——单会话内服务端
    /// 重发整份列表由绝对时间判据兜住（见 `admit` 与本模块测试），叠文本判重是多余的。
    pub fn begin_resume(&mut self) {
        self.resuming = true;
    }

    /// 近期已定稿尾部的归一化拼接（跨重连文本判重的比对基准）。
    fn recent_committed_norm(&self) -> String {
        let cutoff = self.end.saturating_sub(RESUME_TAIL_LOOKBACK);
        self.segments
            .iter()
            .filter(|s| s.end >= cutoff)
            .map(|s| normalize_text(&s.text))
            .collect()
    }

    /// 已定稿覆盖到的最晚时刻 —— 重放缓冲据此裁掉「服务端已经认下」的那段音频。
    #[must_use]
    pub fn end(&self) -> Duration {
        self.end
    }

    /// 整场会话的定稿（终态载荷）。
    #[must_use]
    pub fn into_segments(self) -> Vec<Segment> {
        self.segments
    }
}

/// 这一段的文本是否已被近期已定稿尾部**完全覆盖**（跨重连的重解重复）。
///
/// 只判「被尾部包含」这**一个方向**：反向（尾部被这一段包含）意味着这一段在重复之外
/// 还带了新内容——那种要保留，丢掉它就是丢真的说过的话（越过边界③）。归一化后判包含，
/// 故大小写 / 标点 / 空白差异不影响；过短的段（< [`RESUME_MIN_DEDUP_CHARS`]）不参与，
/// 只走时间判据。
fn text_covered_by_tail(tail_norm: &str, text: &str) -> bool {
    let norm = normalize_text(text);
    norm.chars().count() >= RESUME_MIN_DEDUP_CHARS && tail_norm.contains(&norm)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::SegmentId;

    fn seg(start_ms: u64, end_ms: u64, text: &str) -> Segment {
        Segment {
            id: SegmentId::new(),
            start: Duration::from_millis(start_ms),
            end: Duration::from_millis(end_ms),
            text: text.to_owned(),
            translation: None,
            speaker: None,
            confidence: None,
            words: None,
        }
    }

    fn texts(segments: &[Segment]) -> Vec<&str> {
        segments.iter().map(|s| s.text.as_str()).collect()
    }

    /// 只放行增量：上游重发已发过的整份列表时，下游不得看到「同一句话反复到达」。
    #[test]
    fn resending_the_whole_list_yields_only_the_new_tail() {
        let mut ledger = CommittedLedger::default();
        let a = seg(0, 1000, "第一句");
        let b = seg(1000, 2000, "第二句");
        assert_eq!(texts(&ledger.admit(vec![a.clone()])), ["第一句"]);
        assert_eq!(texts(&ledger.admit(vec![a.clone(), b.clone()])), ["第二句"]);
        assert!(
            ledger.admit(vec![a, b]).is_empty(),
            "第三遍不该再放行任何段"
        );
        assert_eq!(ledger.end(), Duration::from_millis(2000));
    }

    /// 本模块存在的理由：换了一条腿（或换了一条连接）从头重解，已发出的前缀不得重来。
    ///
    /// 兜底腿是另一个引擎、切分点不同；重连后的云腿时间戳从零重计（调用方已做偏移
    /// 校正）。两种情形在这里是同一件事：**按绝对时间判重，不按条数**。
    #[test]
    fn a_restarted_engine_cannot_replay_the_prefix() {
        let mut ledger = CommittedLedger::default();
        ledger.admit(vec![seg(0, 1000, "第一句"), seg(1000, 2000, "第二句")]);
        // 新引擎从头重解，切分点还不一样（把前两句并成了一句）。
        let fresh = ledger.admit(vec![
            seg(0, 2000, "第一句第二句"),
            seg(2000, 3000, "第三句"),
        ]);
        assert_eq!(texts(&fresh), ["第三句"], "前缀被重写了");
        assert_eq!(
            texts(&ledger.into_segments()),
            ["第一句", "第二句", "第三句"]
        );
    }

    /// **跨过边界的段保留**——见模块头那条取舍。
    #[test]
    fn a_segment_straddling_the_boundary_survives() {
        let mut ledger = CommittedLedger::default();
        ledger.admit(vec![seg(0, 1000, "已发")]);
        let fresh = ledger.admit(vec![seg(800, 1500, "骑跨这条边界的一句")]);
        assert_eq!(
            texts(&fresh),
            ["骑跨这条边界的一句"],
            "为了消灭接缝处的重复而丢掉一整段，丢的是用户真的说过的话"
        );
    }

    /// **缺陷 B 会红守护**：重连后服务端重解**重放音频**吐出相同文本，`shift(base)` 把它
    /// 推到「更晚」的绝对区间，单靠时间判据会把同一句反复放行成十几条。`begin_resume` 后的
    /// 文本判重必须挡住它。
    ///
    /// 反向验证：删掉 `admit` 里 `self.resuming` 那一支的文本判重（或不调 `begin_resume`），
    /// 这条当场红——同一句被放行第二遍。
    #[test]
    fn a_reconnect_replay_of_the_same_text_is_not_admitted_twice() {
        let mut ledger = CommittedLedger::default();
        // 断线前认下 [0, 2.0] "同一句"。
        assert_eq!(
            texts(&ledger.admit(vec![seg(0, 2000, "同一句")])),
            ["同一句"]
        );

        // 重连：base = 上次已定稿末端（2.0 秒）。服务端从零重计、重解重放音频，吐出**相同
        // 文本**；调用方 shift(base) 后它落在 [2.0, 3.2]——end 3.2 > self.end 2.0，绝对时间判据
        // 拦不住。
        ledger.begin_resume();
        let replayed = ledger.admit(vec![seg(2000, 3200, "同一句")]);
        assert!(
            replayed.is_empty(),
            "重连重放的重解（相同文本）被当成新内容放行了：{:?}",
            texts(&replayed)
        );
        assert_eq!(
            texts(&ledger.into_segments()),
            ["同一句"],
            "同一句进了两遍——跨重连去重被 base 前移绕过"
        );
    }

    /// resume 状态遇到**确属新的内容**即复位，且新内容照常放行（不误杀，越过边界③）。
    #[test]
    fn resume_lets_genuinely_new_speech_through_and_then_clears() {
        let mut ledger = CommittedLedger::default();
        ledger.admit(vec![seg(0, 2000, "第一句")]);

        ledger.begin_resume();
        // 重解只重复了「第一句」，随后接上真正的新内容「第二句」（不同文本、更晚区间）。
        let fresh = ledger.admit(vec![seg(2000, 3200, "第一句"), seg(3200, 4000, "第二句")]);
        assert_eq!(
            texts(&fresh),
            ["第二句"],
            "重复被丢、新内容被误杀二选一都不对"
        );

        // resume 已复位：此后回到纯时间判据（哪怕文本恰好又与尾部相同也照单放行）。
        let after = ledger.admit(vec![seg(4000, 5000, "第二句")]);
        assert_eq!(
            texts(&after),
            ["第二句"],
            "resume 没有在放行新内容后复位——普通收发被文本判重误伤"
        );
    }

    /// resume 下**带新内容的骑跨段**要保留：只丢「被尾部完全覆盖」的，不丢「重复+新内容」。
    #[test]
    fn resume_keeps_a_segment_that_repeats_then_adds_new_words() {
        let mut ledger = CommittedLedger::default();
        ledger.admit(vec![seg(0, 2000, "你好世界")]);
        ledger.begin_resume();
        // 重解把「世界」又带了一遍，但同一段里还接了「再见」——整段丢掉就丢了「再见」。
        let fresh = ledger.admit(vec![seg(1800, 3000, "世界再见")]);
        assert_eq!(
            texts(&fresh),
            ["世界再见"],
            "为了消灭接缝重复丢掉带新内容的整段——丢的是用户真说过的话"
        );
    }
}
