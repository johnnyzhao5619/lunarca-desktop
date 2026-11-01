//! 按用户本人的录音校准口播语速（PRD-19-012，ADR-162 ④）。
//!
//! 只用**用户自己说的话**：本机录音里说话人为 `self` 的分段（双轨录音的麦克风轨），以及没有
//! 说话人标记的分段（纯麦克风录音）。已做说话人区分的分段（`speaker:N` / `remote…`）分不清
//! 哪一个是用户，一律不用；导入的媒体转写（别人的视频、播客）也不用。
//!
//! 每段按中文字多还是英文词多归一类，两类各自累计「字数 / 词数」与「时长」，各满一分钟才算数；
//! 不够的那一类保留默认值。**只存聚合出的两个数**与样本规模，不存任何转写文字
//! （`.luna/speaking_rate.json`，随库走：同一个人在别的设备上念得一样快）。

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::frontmatter::NoteType;
use crate::script::{spoken_counts, SpeakingRate};
use crate::vault::Vault;

/// 一类语言至少要攒够多少时长才拿它当语速（毫秒）。
const MIN_SAMPLE_MS: u64 = 60_000;
/// 太短的分段不计（起止时间粗，字数少，比例失真）。
const MIN_SEGMENT_MS: u64 = 800;
/// 最多看最近多少条录音（够用就停，不扫全库）。
const MAX_RECORDINGS: usize = 200;
/// 合理语速区间：超出即视为样本有问题（时间轴坏了、转写空洞），不采用。
const CJK_RANGE: std::ops::RangeInclusive<f32> = 80.0..=600.0;
const WORDS_RANGE: std::ops::RangeInclusive<f32> = 60.0..=300.0;

const META_NAME: &str = "speaking_rate";

/// 落盘的校准结果（只有聚合数字）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedSpeakingRate {
    /// 每分钟中文字数；`None` = 中文样本不够，沿用默认值。
    pub cjk_per_minute: Option<f32>,
    /// 每分钟英文词数；`None` = 英文样本不够，沿用默认值。
    pub words_per_minute: Option<f32>,
    /// 用到的样本总时长（分钟）。
    pub sample_minutes: f32,
    /// 用到了几条录音。
    pub recordings: u32,
    /// 校准时刻（RFC3339）。
    pub measured_at: String,
}

impl SavedSpeakingRate {
    /// 合成估算用的语速：校准过的一类用校准值，另一类用默认值；有任一类校准即 `calibrated`。
    #[must_use]
    pub fn rate(&self) -> SpeakingRate {
        SpeakingRate {
            cjk_per_minute: self
                .cjk_per_minute
                .unwrap_or(SpeakingRate::DEFAULT.cjk_per_minute),
            words_per_minute: self
                .words_per_minute
                .unwrap_or(SpeakingRate::DEFAULT.words_per_minute),
            calibrated: self.cjk_per_minute.is_some() || self.words_per_minute.is_some(),
        }
    }
}

/// 一次测量的累计量。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RateSample {
    pub cjk_chars: u64,
    pub cjk_ms: u64,
    pub words: u64,
    pub words_ms: u64,
    pub recordings: u32,
}

impl RateSample {
    /// 计入一段话。
    pub fn add_segment(&mut self, text: &str, millis: u64) {
        if millis < MIN_SEGMENT_MS {
            return;
        }
        let (cjk, words) = spoken_counts(text);
        if cjk == 0 && words == 0 {
            return;
        }
        if cjk >= words {
            self.cjk_chars += u64::from(cjk);
            self.cjk_ms += millis;
        } else {
            self.words += u64::from(words);
            self.words_ms += millis;
        }
    }

    /// 样本够不够、算出来的语速合不合理；两类都不够就是 `None`。
    #[must_use]
    pub fn to_saved(&self, measured_at: String) -> Option<SavedSpeakingRate> {
        let per_minute = |count: u64, millis: u64, range: &std::ops::RangeInclusive<f32>| {
            if millis < MIN_SAMPLE_MS {
                return None;
            }
            #[allow(clippy::cast_precision_loss)]
            let rate = count as f32 / (millis as f32 / 60_000.0);
            range.contains(&rate).then_some(rate.round())
        };
        let cjk = per_minute(self.cjk_chars, self.cjk_ms, &CJK_RANGE);
        let words = per_minute(self.words, self.words_ms, &WORDS_RANGE);
        if cjk.is_none() && words.is_none() {
            return None;
        }
        #[allow(clippy::cast_precision_loss)]
        let sample_minutes =
            ((self.cjk_ms + self.words_ms) as f32 / 60_000.0 * 10.0).round() / 10.0;
        Some(SavedSpeakingRate {
            cjk_per_minute: cjk,
            words_per_minute: words,
            sample_minutes,
            recordings: self.recordings,
            measured_at,
        })
    }

    /// 已攒到的可用时长（分钟，一位小数）——样本不够时告诉用户还差多少。
    #[must_use]
    pub fn minutes(&self) -> f32 {
        #[allow(clippy::cast_precision_loss)]
        let minutes = (self.cjk_ms.max(self.words_ms)) as f32 / 60_000.0;
        (minutes * 10.0).round() / 10.0
    }
}

/// 这一段是不是用户自己说的：`self`（麦克风轨）或没有说话人标记（纯麦克风录音）。
fn is_own_speech(speaker: Option<&str>) -> bool {
    matches!(speaker, None | Some("self"))
}

impl Vault {
    /// 已保存的校准结果；没校准过为 `None`。
    ///
    /// # Errors
    /// 元数据文件损坏时返回错误。
    pub fn saved_speaking_rate(&self) -> Result<Option<SavedSpeakingRate>> {
        Ok(
            crate::meta::read_meta::<SavedSpeakingRate>(self.root(), META_NAME)?
                .into_iter()
                .next(),
        )
    }

    /// 估算脚本时长用的语速：校准过用校准值，否则默认值。读失败也退回默认值——
    /// 一个坏掉的元数据文件不该让大纲整片空白。
    #[must_use]
    pub fn speaking_rate(&self) -> SpeakingRate {
        self.saved_speaking_rate()
            .ok()
            .flatten()
            .map_or(SpeakingRate::DEFAULT, |saved| saved.rate())
    }

    /// 从最近的本机录音里测一次语速（只读，不落盘）。
    ///
    /// # Errors
    /// 派生索引查询失败时返回错误。
    pub fn measure_speaking_rate(&self) -> Result<RateSample> {
        let mut sample = RateSample::default();
        let mut cursor: Option<String> = None;
        let mut seen = 0usize;
        'pages: loop {
            let (entries, next) =
                self.list_notes_of_type(NoteType::Recording, 50, cursor.as_deref())?;
            for entry in entries {
                if seen >= MAX_RECORDINGS {
                    break 'pages;
                }
                seen += 1;
                let Ok(doc) = self.read(&entry.path) else {
                    continue;
                };
                let before = sample;
                for segment in crate::note_segments(self.root(), &doc.meta) {
                    if !is_own_speech(segment.speaker.as_deref()) {
                        continue;
                    }
                    let millis =
                        u64::try_from(segment.end.saturating_sub(segment.start).as_millis())
                            .unwrap_or(u64::MAX);
                    sample.add_segment(&segment.text, millis);
                }
                if sample != before {
                    sample.recordings += 1;
                }
            }
            match next {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        Ok(sample)
    }

    /// 保存一次校准结果（整份替换）。
    ///
    /// # Errors
    /// 写盘失败时返回错误。
    pub fn save_speaking_rate(&self, saved: &SavedSpeakingRate) -> Result<()> {
        crate::meta::write_meta(self.root(), META_NAME, std::slice::from_ref(saved))
    }

    /// 清掉校准，回到默认语速。
    ///
    /// # Errors
    /// 写盘失败时返回错误。
    pub fn clear_speaking_rate(&self) -> Result<()> {
        crate::meta::write_meta::<SavedSpeakingRate>(self.root(), META_NAME, &[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_are_counted_by_their_dominant_language_and_short_ones_are_skipped() {
        let mut sample = RateSample::default();
        sample.add_segment("今天我们聊三件事", 2_000);
        sample.add_segment("let us talk about three things", 2_000);
        sample.add_segment("嗯", 300);
        assert_eq!(sample.cjk_chars, 8);
        assert_eq!(sample.cjk_ms, 2_000);
        assert_eq!(sample.words, 6);
        assert_eq!(sample.words_ms, 2_000);
    }

    #[test]
    fn a_language_needs_a_full_minute_before_it_replaces_the_default() {
        let mut sample = RateSample::default();
        // 中文：90 秒念了 450 字 = 300 字 / 分；英文只有 10 秒，不够。
        for _ in 0..9 {
            sample.add_segment(&"字".repeat(50), 10_000);
        }
        sample.add_segment("one two three four five", 10_000);
        sample.recordings = 2;
        let saved = sample.to_saved("2026-10-17T00:00:00Z".into()).unwrap();
        assert_eq!(saved.cjk_per_minute, Some(300.0));
        assert_eq!(saved.words_per_minute, None);
        let rate = saved.rate();
        assert!(rate.calibrated);
        assert!(
            (rate.words_per_minute - SpeakingRate::DEFAULT.words_per_minute).abs() < f32::EPSILON
        );
    }

    #[test]
    fn too_little_or_implausible_speech_is_not_a_calibration() {
        let mut sample = RateSample::default();
        sample.add_segment("你好", 30_000);
        assert!(sample.to_saved(String::new()).is_none());
        // 一分钟只念了 10 个字：时间轴或转写有问题，不采用。
        let mut sparse = RateSample::default();
        sparse.add_segment(&"字".repeat(10), 60_000);
        assert!(sparse.to_saved(String::new()).is_none());
    }

    #[test]
    fn only_the_users_own_track_counts() {
        assert!(is_own_speech(Some("self")));
        assert!(is_own_speech(None));
        assert!(!is_own_speech(Some("remote")));
        assert!(!is_own_speech(Some("speaker:1")));
    }

    #[test]
    fn saving_and_clearing_round_trip_through_vault_meta() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        assert!(!vault.speaking_rate().calibrated);
        let saved = SavedSpeakingRate {
            cjk_per_minute: Some(280.0),
            words_per_minute: None,
            sample_minutes: 3.5,
            recordings: 4,
            measured_at: "2026-10-17T00:00:00Z".into(),
        };
        vault.save_speaking_rate(&saved).unwrap();
        assert_eq!(vault.saved_speaking_rate().unwrap(), Some(saved));
        assert!((vault.speaking_rate().cjk_per_minute - 280.0).abs() < f32::EPSILON);
        vault.clear_speaking_rate().unwrap();
        assert_eq!(vault.saved_speaking_rate().unwrap(), None);
        // 没有任何录音时测量得到空样本，而不是错误。
        assert_eq!(
            vault.measure_speaking_rate().unwrap(),
            RateSample::default()
        );
    }
}
