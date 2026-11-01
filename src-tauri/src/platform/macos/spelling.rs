//! 西文拼写检查（笔记库对标轮 W6a）：系统拼写检查器 `NSSpellChecker`。
//!
//! 波浪线由编辑器自己画，错词位置在这里算——不靠 WebKit 的「输入时检查拼写」：那一套只检查
//! 刚编辑过的一小段，打开一篇已有的笔记什么都不标（真机 2026-10-05），而且它是进程级开关，会
//! 连带所有输入框。建议与「学习拼写」同走这里（与系统其它应用同一本用户词典）。
//!
//! 被检查的文字不写日志。

use objc2::rc::Retained;
use objc2_app_kit::NSSpellChecker;
use objc2_foundation::{NSNotFound, NSRange, NSString};

/// 一段文字里系统认为拼错的词：`(起点, 长度)`，UTF-16 码元（与 NSString、JS 字符串、CodeMirror
/// 的位置同一单位）。
#[must_use]
pub fn misspellings(text: &str) -> Vec<(usize, usize)> {
    let checker: Retained<NSSpellChecker> = NSSpellChecker::sharedSpellChecker();
    let string = NSString::from_str(text);
    let total = string.length();
    let mut out = Vec::new();
    let mut start = 0usize;
    while start < total {
        // SAFETY: `word_count` 传空指针是文档允许的（不要字数）。
        let range: NSRange = unsafe {
            checker.checkSpellingOfString_startingAt_language_wrap_inSpellDocumentWithTag_wordCount(
                &string,
                start as isize,
                None,
                false,
                0,
                std::ptr::null_mut(),
            )
        };
        if range.length == 0 || range.location == NSNotFound as usize || range.location < start {
            break;
        }
        out.push((range.location, range.length));
        start = range.location + range.length;
    }
    out
}

/// 这个词拼错了吗？拼错了给出候选（可能为空），没拼错为 `None`。
#[must_use]
pub fn guesses(word: &str) -> Option<Vec<String>> {
    let checker: Retained<NSSpellChecker> = NSSpellChecker::sharedSpellChecker();
    let text = NSString::from_str(word);
    let range: NSRange = checker.checkSpellingOfString_startingAt(&text, 0);
    // 没有拼错时返回 `{NSNotFound, 0}`。
    if range.length == 0 {
        return None;
    }
    let list =
        checker.guessesForWordRange_inString_language_inSpellDocumentWithTag(range, &text, None, 0);
    Some(
        list.map(|list| list.iter().map(|guess| guess.to_string()).collect())
            .unwrap_or_default(),
    )
}

/// 记进用户词典（与系统「学习拼写」同一本，其它应用也认）。
pub fn learn(word: &str) {
    NSSpellChecker::sharedSpellChecker().learnWord(&NSString::from_str(word));
}
