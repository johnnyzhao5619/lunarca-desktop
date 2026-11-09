//! 提词器语音跟随的对齐（ADR-162 ⑤ C4b，PRD-19-015）：把实时转写对到脚本上「念到哪了」。
//!
//! 纯函数、零 IO：脚本切成记号序列（中文与日文假名按字、其它按词、去标点、统一小写），每个
//! 记号记下它在第几段第几行第几个字；识别文本取最近十几个记号，在当前位置前后一个窗口里按
//! 最长公共子序列打分找结束位置。
//!
//! - 分数不到门槛：原地不动（停顿、跑题、咳嗽都不该让提词器乱跳）。
//! - 小幅回退照走：说错一句重说，会退回那一句的开头接上。
//! - 大幅回跳要更高的分：念到后面忽然和前面某句很像时，不轻易跳回去。

/// 脚本里的一个位置（段、行、行内第几个字，都从 0 起）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ScriptPosition {
    pub block: u32,
    pub line: u32,
    pub char_offset: u32,
}

/// 识别文本只看最近这么多个记号（够分辨一句，又不至于被很早的话拖住）。
const RECENT_TOKENS: usize = 12;
/// 至少攒够这么多个记号才开始对（两三个字太容易撞上别处）。
const MIN_TOKENS: usize = 3;
/// 从当前位置往前最多找多远（说错重说通常只退回一两句）。
const SEARCH_BACK: usize = 40;
/// 往后最多找多远（漏识别、跳读一小段也能追上）。
const SEARCH_AHEAD: usize = 160;
/// 往回超过这么多个记号算「大幅回跳」，门槛提高。
const BIG_JUMP_BACK: usize = 8;
/// 识别文本里最多保留多少个已确定的记号。
const COMMITTED_KEEP: usize = 64;

/// 是不是按字切的文字（汉字、扩展 A、兼容区、日文假名）。
fn is_char_token(ch: char) -> bool {
    matches!(ch,
        '\u{4E00}'..='\u{9FFF}'
        | '\u{3400}'..='\u{4DBF}'
        | '\u{F900}'..='\u{FAFF}'
        | '\u{3040}'..='\u{30FF}')
}

/// 切记号：返回（记号，它在原文里的字符下标）。
#[must_use]
pub fn tokenize(text: &str) -> Vec<(String, usize)> {
    let mut tokens = Vec::new();
    let mut word = String::new();
    let mut word_start = 0;
    for (index, ch) in text.chars().enumerate() {
        if is_char_token(ch) {
            if !word.is_empty() {
                tokens.push((std::mem::take(&mut word), word_start));
            }
            tokens.push((ch.to_string(), index));
        } else if ch.is_alphanumeric() || (ch == '\'' && !word.is_empty()) {
            if word.is_empty() {
                word_start = index;
            }
            word.extend(ch.to_lowercase());
        } else if !word.is_empty() {
            tokens.push((std::mem::take(&mut word), word_start));
        }
    }
    if !word.is_empty() {
        tokens.push((word, word_start));
    }
    tokens
}

/// 两段记号的最长公共子序列长度。
fn lcs(a: &[String], b: &[String]) -> usize {
    let mut previous = vec![0usize; b.len() + 1];
    let mut current = vec![0usize; b.len() + 1];
    for x in a {
        for (j, y) in b.iter().enumerate() {
            current[j + 1] = if x == y {
                previous[j] + 1
            } else {
                previous[j + 1].max(current[j])
            };
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

/// 一篇脚本的跟读状态。
#[derive(Debug, Clone)]
pub struct PrompterAligner {
    tokens: Vec<String>,
    positions: Vec<ScriptPosition>,
    /// 已经念到第几个记号（下一处应当念的记号下标）。
    cursor: usize,
    /// 已确定的识别记号（只留最近一段）。
    committed: Vec<String>,
}

impl PrompterAligner {
    /// 由脚本的各行建立：每行是（段号，行号，文字）。
    #[must_use]
    pub fn new<'a>(lines: impl IntoIterator<Item = (u32, u32, &'a str)>) -> Self {
        let mut tokens = Vec::new();
        let mut positions = Vec::new();
        for (block, line, text) in lines {
            for (token, offset) in tokenize(text) {
                tokens.push(token);
                positions.push(ScriptPosition {
                    block,
                    line,
                    char_offset: u32::try_from(offset).unwrap_or(u32::MAX),
                });
            }
        }
        Self {
            tokens,
            positions,
            cursor: 0,
            committed: Vec::new(),
        }
    }

    /// 此刻念到的位置（还没开始念时是开头）。
    #[must_use]
    pub fn position(&self) -> ScriptPosition {
        self.cursor
            .checked_sub(1)
            .and_then(|index| self.positions.get(index))
            .or_else(|| self.positions.first())
            .copied()
            .unwrap_or_default()
    }

    /// 从头再来（用户点了「回到开头」）。
    pub fn reset(&mut self) {
        self.cursor = 0;
        self.committed.clear();
    }

    /// 从某一行接着跟（ADR-176 ⑥，PRD-20-013：点哪行从哪行开始、按键换行换段）：游标放到这一行
    /// 第一个记号上，已确定的识别记号清空——之前念过的不再参与这一行之后的对齐。这一行没有记号
    /// （空行）时放到它之后的第一个记号；在稿末之后就放到最后。
    pub fn seek(&mut self, block: u32, line: u32) {
        let at = self
            .positions
            .iter()
            .position(|p| (p.block, p.line) >= (block, line))
            .unwrap_or(self.positions.len());
        self.cursor = at;
        self.committed.clear();
    }

    /// 喂一次实时转写：`committed` 是这次新确定的文字（可空），`hypothesis` 是此刻的暂定尾
    /// （每次整句替换）。位置变了返回新位置；没变或对不上返回 `None`。
    pub fn feed(&mut self, committed: &str, hypothesis: &str) -> Option<ScriptPosition> {
        self.committed
            .extend(tokenize(committed).into_iter().map(|(token, _)| token));
        if self.committed.len() > COMMITTED_KEEP {
            let drop = self.committed.len() - COMMITTED_KEEP;
            self.committed.drain(..drop);
        }
        let mut recent: Vec<String> = self.committed.clone();
        recent.extend(tokenize(hypothesis).into_iter().map(|(token, _)| token));
        if recent.len() > RECENT_TOKENS {
            recent.drain(..recent.len() - RECENT_TOKENS);
        }
        if recent.len() < MIN_TOKENS || self.tokens.is_empty() {
            return None;
        }
        let end = self.best_end(&recent)?;
        if end == self.cursor {
            return None;
        }
        let before = self.position();
        self.cursor = end;
        let after = self.position();
        (after != before).then_some(after)
    }

    /// 在窗口里找最像的结束位置；不够像返回 `None`。
    fn best_end(&self, recent: &[String]) -> Option<usize> {
        let span = recent.len() * 2;
        let lo = self.cursor.saturating_sub(SEARCH_BACK).max(1);
        let hi = (self.cursor + SEARCH_AHEAD).min(self.tokens.len());
        let mut best: Option<(usize, usize)> = None; // (分数, 结束位置)
        for end in lo..=hi {
            let window = &self.tokens[end.saturating_sub(span)..end];
            let matched = lcs(recent, window);
            // 「收得紧」：结束位置上的那个脚本记号确实对上了（去掉它分数就掉）。窗口比识别文本
            // 宽，紧跟在真正结束处之后的几个位置分数一样；不分辨的话，同分时离当前位置近的
            // 那个赢——高亮越过句末几个字之后，重念那一句就退不回去（真机 2026-10-17）。
            let tight = usize::from(matched > lcs(recent, &window[..window.len() - 1]));
            let score = matched * 2 + tight;
            let better = match best {
                None => true,
                Some((best_score, best_end)) => {
                    score > best_score
                        || (score == best_score
                            && end.abs_diff(self.cursor) < best_end.abs_diff(self.cursor))
                }
            };
            if better {
                best = Some((score, end));
            }
        }
        let (score, end) = best?;
        let matched = score / 2;
        // 门槛：六成记号对得上；往回跳得远要八成。
        let needed = if end + BIG_JUMP_BACK < self.cursor {
            recent.len() * 8 / 10
        } else {
            (recent.len() * 6).div_ceil(10)
        };
        (matched >= needed.max(MIN_TOKENS)).then_some(end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aligner() -> PrompterAligner {
        PrompterAligner::new([
            (0, 0, "大家好，今天我们聊一聊怎么写视频脚本。"),
            (0, 1, "第一件事是先想清楚观众要带走什么。"),
            (1, 0, "然后把开头的三十秒剪得紧凑一点。"),
            (1, 1, "Finally, don't forget the subtitles."),
        ])
    }

    #[test]
    fn tokens_are_characters_for_chinese_and_words_otherwise() {
        let tokens: Vec<String> = tokenize("说 Hello, World's 三十秒！")
            .into_iter()
            .map(|(t, _)| t)
            .collect();
        assert_eq!(tokens, ["说", "hello", "world's", "三", "十", "秒"]);
        assert_eq!(tokenize("ab 你")[1], ("你".to_owned(), 3));
    }

    #[test]
    fn reading_in_order_moves_forward_line_by_line() {
        let mut a = aligner();
        assert_eq!(a.feed("", "大家"), None, "两个字太少，不对");
        let p = a.feed("大家好今天我们", "聊一聊").unwrap();
        assert_eq!((p.block, p.line), (0, 0));
        let p = a.feed("聊一聊怎么写视频脚本", "第一件事是先想").unwrap();
        assert_eq!((p.block, p.line), (0, 1));
        let p = a
            .feed("第一件事是先想清楚观众要带走什么", "然后把开头的")
            .unwrap();
        assert_eq!((p.block, p.line), (1, 0));
        let p = a
            .feed("然后把开头的三十秒剪得紧凑一点", "finally don't forget")
            .unwrap();
        assert_eq!((p.block, p.line), (1, 1));
    }

    #[test]
    fn misrecognised_words_still_follow_and_off_topic_talk_does_not_move() {
        let mut a = aligner();
        // 识别错了几个字（「视屏角本」）照样跟上。
        let p = a.feed("大家好今天我们聊一聊怎么写视屏角本", "").unwrap();
        assert_eq!((p.block, p.line), (0, 0));
        let before = a.position();
        assert_eq!(a.feed("", "等一下我喝口水"), None);
        assert_eq!(a.position(), before);
    }

    #[test]
    fn restarting_a_sentence_goes_back_to_it() {
        let mut a = aligner();
        a.feed("大家好今天我们聊一聊怎么写视频脚本", "");
        a.feed("第一件事是先想清楚观众要带走什么然后把开头", "");
        assert_eq!(a.position().block, 1);
        // 说错了，从第二句重来。
        let p = a.feed("", "不对 第一件事是先想清楚").unwrap();
        assert_eq!((p.block, p.line), (0, 1));
    }

    #[test]
    fn reset_goes_back_to_the_start() {
        let mut a = aligner();
        a.feed("大家好今天我们聊一聊怎么写视频脚本第一件事", "");
        a.reset();
        assert_eq!(a.position(), ScriptPosition::default());
        assert_eq!(PrompterAligner::new([]).feed("大家好今天", ""), None);
    }

    fn script() -> PrompterAligner {
        PrompterAligner::new([
            (0, 0, "大家好，今天我们聊一聊怎么把视频脚本写得更顺口。"),
            (0, 1, "很多人写完稿子一念，才发现句子太长、喘不过气。"),
            (1, 0, "先把每句话控制在一口气能念完的长度。"),
        ])
    }

    #[test]
    fn a_trailing_filler_word_does_not_push_the_highlight_into_the_next_line() {
        // 真机 2026-10-17：念完第一句后识别尾巴多了个语气词，高亮就停到了下一句上。
        let mut a = script();
        a.feed("大家好今天我们聊一聊怎么把视频脚本写得更顺口", "");
        assert_eq!((a.position().block, a.position().line), (0, 0));
        a.feed("", "啊");
        assert_eq!((a.position().block, a.position().line), (0, 0));
        a.feed("", "嗯 好");
        assert_eq!((a.position().block, a.position().line), (0, 0));
    }

    #[test]
    fn rereading_the_previous_line_moves_back_even_without_an_exact_last_word() {
        // 真机：高亮已越过一句的末尾几个字，把那一句重念一遍时要退回去，不能因为「原地不动」
        // 与「退回去」打平就原地不动。
        let mut a = script();
        a.feed("大家好今天我们聊一聊怎么把视频脚本写得更顺口很多人写", "");
        assert_eq!((a.position().block, a.position().line), (0, 1));
        a.feed("", "大家好今天我们聊一聊怎么把视频脚本写得更顺口啊");
        assert_eq!((a.position().block, a.position().line), (0, 0));
    }

    #[test]
    fn seeking_moves_the_follow_cursor_to_that_line() {
        let mut a = script();
        a.seek(1, 0);
        assert_eq!(
            a.position().block,
            0,
            "还没念到这一行时，位置停在它前一个记号上"
        );
        let moved = a.feed("先把每句话控制在一口气", "");
        assert_eq!(moved.map(|p| (p.block, p.line)), Some((1, 0)));
        // 往回跳也照样接得上。
        a.seek(0, 1);
        let moved = a.feed("很多人写完稿子一念", "");
        assert_eq!(moved.map(|p| (p.block, p.line)), Some((0, 1)));
        // 稿末之后：放到最后。
        a.seek(9, 9);
        assert_eq!((a.position().block, a.position().line), (1, 0));
    }
}
