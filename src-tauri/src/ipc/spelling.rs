//! 西文拼写检查（笔记库对标轮 W6a）。系统接口在 `platform::macos::spelling`；这里决定**查哪些词**，
//! 把错词位置交给编辑器画波浪线、把候选交给右键菜单。开关是设置 `editor.spellcheck`，只管编辑器
//! 正文——不碰 WebKit 的进程级「输入时检查拼写」（理由见平台模块）。
//!
//! 只查西文单词：中日韩文字系统拼写检查器本就不管，代码标识符、数字、网址交过来只会得到
//! 一串莫名其妙的「建议」。判据只在 [`is_checkable_word`] 一处，编辑器不另写一份。

use super::*;

/// 一段文字里的一个错词：UTF-16 区间 `[start, end)`（与 CodeMirror 的位置同一单位）。
#[derive(Debug, Clone, Copy, Serialize, Type, PartialEq, Eq)]
pub struct MisspellingDto {
    pub start: u32,
    pub end: u32,
}

/// 一次最多检查多少（UTF-16 码元）：编辑器只送可见的那几屏，超出的是异常输入。
const MAX_CHECK_UNITS: usize = 50_000;

/// 一个拼错的词的候选更正（按系统给的顺序，可能为空）。
#[derive(Debug, Clone, Serialize, Type)]
pub struct SpellingSuggestionsDto {
    pub guesses: Vec<String>,
}

/// 西文单词：拉丁字母（含带变音符号的）加词内的撇号、连字符，至少两个字母，不含数字。
pub(crate) fn is_checkable_word(word: &str) -> bool {
    let latin = |c: char| {
        c.is_ascii_alphabetic()
            || (('\u{00C0}'..='\u{024F}').contains(&c) && c != '\u{00D7}' && c != '\u{00F7}')
            || ('\u{1E00}'..='\u{1EFF}').contains(&c)
    };
    let letters = word.chars().filter(|c| latin(*c)).count();
    letters >= 2
        && word
            .chars()
            .all(|c| latin(c) || matches!(c, '\'' | '\u{2019}' | '-'))
        && !word.starts_with(['\'', '\u{2019}', '-'])
        && !word.ends_with(['\'', '\u{2019}', '-'])
}

/// 这个词拼错了吗：拼错了给候选，没拼错、不是西文单词或平台不支持为 `null`。
#[tauri::command]
#[specta::specta]
pub(crate) fn spelling_suggestions(word: String) -> Option<SpellingSuggestionsDto> {
    if !is_checkable_word(&word) {
        return None;
    }
    #[cfg(target_os = "macos")]
    {
        crate::platform::macos::spelling::guesses(&word)
            .map(|guesses| SpellingSuggestionsDto { guesses })
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// 「学习拼写」：记进系统用户词典，此后不再标它。
#[tauri::command]
#[specta::specta]
// 学习拼写只在 macOS 有动作，其它平台提前返回即函数末尾。
#[cfg_attr(not(target_os = "macos"), allow(clippy::needless_return))]
pub(crate) fn learn_spelling(word: String) {
    if !is_checkable_word(&word) {
        return;
    }
    #[cfg(target_os = "macos")]
    crate::platform::macos::spelling::learn(&word);
}

/// 一段文字里拼错的西文词（编辑器送可见段落，按返回的区间画波浪线）。
#[tauri::command]
#[specta::specta]
pub(crate) fn spelling_check(text: String) -> Vec<MisspellingDto> {
    misspellings_in(&text)
}

fn misspellings_in(text: &str) -> Vec<MisspellingDto> {
    let units: Vec<u16> = text.encode_utf16().collect();
    if units.len() > MAX_CHECK_UNITS {
        return Vec::new();
    }
    #[cfg(target_os = "macos")]
    let found = crate::platform::macos::spelling::misspellings(text);
    #[cfg(not(target_os = "macos"))]
    let found: Vec<(usize, usize)> = Vec::new();
    found
        .into_iter()
        .filter(|&(start, len)| start + len <= units.len())
        .filter(|&(start, len)| {
            String::from_utf16(&units[start..start + len])
                .is_ok_and(|word| is_checkable_word(&word))
        })
        .map(|(start, len)| MisspellingDto {
            start: start as u32,
            end: (start + len) as u32,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_latin_words_are_checked() {
        for word in ["teh", "don't", "well-known", "café", "Ångström", "naïve"] {
            assert!(is_checkable_word(word), "{word}");
        }
        for word in [
            "a",
            "速记",
            "日本語",
            "한국어",
            "abc123",
            "x_y",
            "http",
            "-ab",
            "ab'",
            "C#",
            "mixed中文",
        ] {
            // `http` 是正常单词，交给拼写检查器判；这里只拦形态不对的。
            if word == "http" {
                assert!(is_checkable_word(word));
                continue;
            }
            assert!(!is_checkable_word(word), "{word}");
        }
    }

    /// 区间是 UTF-16 偏移，前面有中文与表情时也要落在原词上；中文、带数字的词不报。
    #[cfg(target_os = "macos")]
    #[test]
    fn misspelling_ranges_point_at_the_words_in_utf16() {
        let text = "中文😀 The wrold is fine, abc123 不查。";
        let units: Vec<u16> = text.encode_utf16().collect();
        // 系统拼写服务（AppleSpell）冷启动时的头几次查询可能在它就绪前返回空——全量测试占满 CPU 时
        // 实测出现过两次，单跑从不出现。本用例验的是区间换算，故有限次等它预热；一直为空照样判失败。
        let mut found = misspellings_in(text);
        for _ in 0..10 {
            if !found.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
            found = misspellings_in(text);
        }
        let words: Vec<String> = found
            .iter()
            .map(|m| String::from_utf16(&units[m.start as usize..m.end as usize]).unwrap())
            .collect();
        assert!(words.contains(&"wrold".to_owned()), "{words:?}");
        assert!(words.iter().all(|w| is_checkable_word(w)), "{words:?}");
    }
}
