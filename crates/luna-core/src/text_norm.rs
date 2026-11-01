//! 转写结果层「文本判据」的共享归一化原语。
//!
//! 跨轨回声去重（[`crate::echo`]）与跨重连定稿去重（[`crate::committed`]）是**同族**能力
//! （时间前沿 + 文本判据），归一化只留一处实现（DRY——发现复制即待收口的债）。
//!
//! 归一化口径：小写 + 仅保留字母数字（Unicode，覆盖 CJK），丢弃空白与标点——这样
//! 「同一句。」与「同一句」、大小写与多空格差异都判为同一串，重解出的细微标点差异不会
//! 让判重漏掉。

/// 文本归一化：小写 + 仅保留字母数字（Unicode 口径，覆盖 CJK），丢空白/标点。
#[must_use]
pub(crate) fn normalize_text(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_punctuation_whitespace_and_case_across_scripts() {
        assert_eq!(normalize_text(" 同一句。"), "同一句");
        assert_eq!(normalize_text("Hello, World!"), "helloworld");
        assert_eq!(normalize_text("同一句"), normalize_text("同一句 "));
        assert_eq!(normalize_text("——"), "");
    }
}
