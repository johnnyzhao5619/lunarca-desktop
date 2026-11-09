//! 路径名的比较口径（纯逻辑，无 IO）：NFC 规范化与「NFC + 小写」的名字比较键。
//! 本机库、同步引擎与网页版用的是这同一份（ADR-148 ⑥、ADR-177 ⑥）。

/// NFC 规范化。只用于**比较与显示**，从不用来改写路径：macOS 列目录给的是文件落盘时的形态
/// （可能是 NFD），按路径查找却不分两种形态；Linux / Windows 的文件系统则两种形态是两个
/// 名字。所以路径照原样存、照原样开，只在比较时两边都过一遍这里（ADR-148 ⑥）。
#[must_use]
pub fn nfc(text: &str) -> std::borrow::Cow<'_, str> {
    icu_normalizer::ComposingNormalizerBorrowed::new_nfc().normalize(text)
}

/// 名字比较键：NFC + 小写。链接解析（`luna_vault::graph::LinkIndex`）、改名重构的候选名与
/// `[[` 补全共用这一个口径——各写一份，就会出现「图谱里连得上、补全里搜不到」。
#[must_use]
pub fn name_key(text: &str) -> String {
    nfc(text).to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_key_matches_nfd_and_nfc_spellings_case_insensitively() {
        let nfd = "Cafe\u{301} \u{1100}\u{1167}\u{11a8}"; // e + 组合重音、韩文字母分解形
        assert_eq!(name_key(nfd), name_key("CAFÉ 격"));
        assert_eq!(nfc("周报"), "周报", "汉字的 NFC 与原文相同，不分配");
        assert!(matches!(nfc("周报"), std::borrow::Cow::Borrowed(_)));
    }
}
