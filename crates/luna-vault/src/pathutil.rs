//! Vault 内相对路径工具（纯逻辑，无 IO）。附件位置策略（PRD-06-012）与附件跟随移动
//! （PRD-06-012 生命周期）共用同一套路径推导（拼接、清洗、相对路径），它住在 `luna_sync_core::attachment`——
//! 网页版上传附件用的是同一份（ADR-177 ⑥）；调用方直接从那里引用。
//!
//! 约定：路径均为 **Vault 相对、`/` 分隔**；空串 = 仓库根目录。
//!
//! 链接目标的解析口径（隐藏路径、父目录、归一化、百分号解码、规范形、相对链接）住在
//! `luna_sync_core::links`——网页版的 WASM 与桌面共用同一份（ADR-177 ⑥）；调用方直接从那里引用。

use luna_sync_core::links::relative_link_path;

/// 外部传进来的库内路径（`lunarca://` 链接、CLI 参数，ADR-141）→ 规范的库相对路径；不合法为
/// `None`。越界（`..`）、绝对路径、盘符、反斜杠、任一段以 `.` 开头（`.luna`、`.trash`、点文件）
/// 一律拒绝——外部入口只能指向用户的笔记与附件，碰不到 Lunarca 自己的数据。
#[must_use]
pub fn external_rel_path(raw: &str) -> Option<String> {
    if raw.contains('\\') || raw.starts_with('/') || raw.contains(':') {
        return None;
    }
    let segments: Vec<&str> = raw
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    if segments.is_empty()
        || segments
            .iter()
            .any(|segment| *segment == ".." || segment.starts_with('.'))
    {
        return None;
    }
    Some(segments.join("/"))
}

/// 显示隐藏文件时也**永远不列**的点目录 / 点文件：Lunarca 自己的数据（`.luna`、回收站
/// `.trash`）、git 仓库本体、Finder 的 `.DS_Store`——列出来只会招来误删误改。
pub const ALWAYS_HIDDEN: &[&str] = &[".luna", ".trash", ".git", ".DS_Store"];

/// Markdown 链接目标里的空格与括号需转义，否则链接在渲染时断掉。
#[must_use]
pub fn encode_link_target(path: &str) -> String {
    path.replace(' ', "%20")
        .replace('(', "%28")
        .replace(')', "%29")
}

/// Lunarca **自动生成**的笔记间链接的唯一构造点（PRD-06-030 链接语法策略）。
///
/// 恒产出**标准相对 md 链接**而非 wikilink：可移植性是明示裁决——VSCode 等外部编辑器
/// 也能点开，最大化「不锁定」。索引侧对两种语法同口径解析，故用户手写 wikilink 照常
/// 被认作链接；这里管的只是「Lunarca 自己写出去的长什么样」。
///
/// 收敛为一处是因为它已有三个调用面（蒸馏来源区、问答出处与洞见回流、未链接提及转正），
/// 各写一份必然在转义规则上漂移。
#[must_use]
pub fn markdown_link(from: &str, to: &str, text: &str) -> String {
    format!(
        "[{text}]({})",
        encode_link_target(&relative_link_path(from, to))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_link_is_relative_portable_and_escaped() {
        assert_eq!(markdown_link("Notes/a.md", "Notes/b.md", "B"), "[B](b.md)");
        assert_eq!(
            markdown_link("Notes/sub/a.md", "Transcripts/c.md", "C"),
            "[C](../../Transcripts/c.md)"
        );
        // 空格与括号必须转义，否则渲染时链接断在半路。
        assert_eq!(
            markdown_link("a.md", "项目 X (2026).md", "T"),
            "[T](%E9%A1%B9%E7%9B%AE%20X%20%282026%29.md)".replace("%E9%A1%B9%E7%9B%AE", "项目")
        );
    }
}
