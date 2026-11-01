//! 拼音检索键（PRD-07-040 ④）：让「hy」「huiyi」都能找到「会议」。
//!
//! 一个标题给出若干条键：全拼（`huiyi`）与首字母（`hy`）。多音字取前两个读音（拼音表按
//! 常用度排序，第一个最常用），组合总数封顶——「重重重重」这类标题按笛卡尔积展开会爆炸，
//! 超过上限后其余字只取第一个读音。非汉字的字母数字原样（小写）并入，空白与标点跳过：
//! `Q3 会议` → `q3huiyi` / `q3hy`。没有汉字的标题不出键（拼音对它没有意义）。
//!
//! 现算不入索引：开快速切换器时 Rust 对全部标题算一遍（1 万篇实测见 TDD-06 §8），换来的是
//! 不为它再升一次索引版本——升版本会整库重建并清掉语义向量。

use pinyin::ToPinyinMulti;

/// 每个多音字最多取几个读音。
const READINGS_PER_CHAR: usize = 2;
/// 一个标题最多展开几种读音组合。
const MAX_COMBINATIONS: usize = 8;

/// 一个标题的拼音检索键（全拼与首字母，去重；无汉字时为空）。
#[must_use]
pub fn pinyin_keys(text: &str) -> Vec<String> {
    // 每个字的候选「(全拼片段, 首字母片段)」。
    let mut slots: Vec<Vec<(String, String)>> = Vec::new();
    let mut has_han = false;
    for (ch, multi) in text.chars().zip(text.to_pinyin_multi()) {
        if let Some(multi) = multi {
            has_han = true;
            let mut readings: Vec<(String, String)> = Vec::new();
            for reading in multi {
                let pair = (
                    reading.plain().to_owned(),
                    reading.first_letter().to_owned(),
                );
                if !readings.contains(&pair) {
                    readings.push(pair);
                }
                if readings.len() == READINGS_PER_CHAR {
                    break;
                }
            }
            if !readings.is_empty() {
                slots.push(readings);
            }
        } else if ch.is_alphanumeric() {
            let lower: String = ch.to_lowercase().collect();
            slots.push(vec![(lower.clone(), lower)]);
        }
    }
    if !has_han {
        return Vec::new();
    }
    let mut combos: Vec<(String, String)> = vec![(String::new(), String::new())];
    for readings in &slots {
        let take = if combos.len() * readings.len() <= MAX_COMBINATIONS {
            readings.len()
        } else {
            1
        };
        combos = combos
            .iter()
            .flat_map(|(full, initials)| {
                readings[..take]
                    .iter()
                    .map(move |(f, i)| (format!("{full}{f}"), format!("{initials}{i}")))
            })
            .collect();
    }
    let mut out: Vec<String> = Vec::new();
    for (full, initials) in combos {
        for key in [full, initials] {
            if !out.contains(&key) {
                out.push(key);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_pinyin_and_initials() {
        let keys = pinyin_keys("会议纪要");
        assert!(keys.contains(&"huiyijiyao".to_owned()), "{keys:?}");
        assert!(keys.contains(&"hyjy".to_owned()), "{keys:?}");
    }

    #[test]
    fn letters_and_digits_stay_spaces_and_punctuation_go() {
        let keys = pinyin_keys("Q3 会议！");
        assert!(keys.contains(&"q3huiyi".to_owned()), "{keys:?}");
        assert!(keys.contains(&"q3hy".to_owned()), "{keys:?}");
    }

    #[test]
    fn heteronyms_give_more_than_one_reading() {
        // 「重」：zhong / chong。
        let keys = pinyin_keys("重庆");
        assert!(keys.iter().any(|k| k.starts_with("chong")), "{keys:?}");
        assert!(keys.iter().any(|k| k.starts_with("zhong")), "{keys:?}");
    }

    #[test]
    fn combinations_are_capped_and_plain_titles_have_no_keys() {
        assert!(pinyin_keys("重重重重重重重重重重").len() <= 2 * MAX_COMBINATIONS);
        assert!(pinyin_keys("Weekly review").is_empty());
        assert!(pinyin_keys("").is_empty());
    }
}
