//! Vault 内相对路径工具（纯逻辑，无 IO）。附件位置策略（PRD-06-012）与附件跟随移动
//! （PRD-06-012 生命周期）共用同一套路径推导——单一事实源，两侧（save / move）不各写一份。
//!
//! 约定：路径均为 **Vault 相对、`/` 分隔**；空串 = 仓库根目录。

/// NFC 规范化。只用于**比较与显示**，从不用来改写路径：macOS 列目录给的是文件落盘时的形态
/// （可能是 NFD），按路径查找却不分两种形态；Linux / Windows 的文件系统则两种形态是两个
/// 名字。所以路径照原样存、照原样开，只在比较时两边都过一遍这里（ADR-148 ⑥）。
#[must_use]
pub fn nfc(text: &str) -> std::borrow::Cow<'_, str> {
    icu_normalizer::ComposingNormalizerBorrowed::new_nfc().normalize(text)
}

/// 名字比较键：NFC + 小写。链接解析（[`crate::graph::LinkIndex`]）、改名重构的候选名与
/// `[[` 补全共用这一个口径——各写一份，就会出现「图谱里连得上、补全里搜不到」。
#[must_use]
pub fn name_key(text: &str) -> String {
    nfc(text).to_lowercase()
}

/// 隐藏路径：库相对路径里任一段以 `.` 开头（`.obsidian/app.json`、`草稿/.私密.md`）。
///
/// Lunarca 把点文件当作「不是用户内容」：不进索引、不进搜索与链接解析，与启动扫描、文件监听、
/// 建树同一口径。「显示隐藏文件」（V4b）只让它们出现在文件树里，这条口径不变。
#[must_use]
pub fn is_hidden_rel(rel: &str) -> bool {
    rel.split('/').any(|segment| segment.starts_with('.'))
}

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

/// 拼接两段库内相对路径（任一为空取另一，避免前导/多余斜杠）。
#[must_use]
pub fn join_vault_rel(a: &str, b: &str) -> String {
    match (a.is_empty(), b.is_empty()) {
        (true, _) => b.to_owned(),
        (_, true) => a.to_owned(),
        _ => format!("{a}/{b}"),
    }
}

/// 清洗用户配置的库内相对目录：丢弃 `.`/`..`/空段与前后斜杠，杜绝越出 Vault。
#[must_use]
pub fn sanitize_vault_rel_dir(raw: &str) -> String {
    raw.split('/')
        .filter(|seg| !seg.is_empty() && *seg != "." && *seg != "..")
        .collect::<Vec<_>>()
        .join("/")
}

/// 计算 `from_dir`（库内目录）到 `to_path`（库内文件路径）的**相对路径**（含 `../`）。
/// 供附件位置策略产出可移植的笔记相对链接（PRD-06-030）。
#[must_use]
pub fn relative_vault_path(from_dir: &str, to_path: &str) -> String {
    let from: Vec<&str> = from_dir.split('/').filter(|s| !s.is_empty()).collect();
    let to: Vec<&str> = to_path.split('/').filter(|s| !s.is_empty()).collect();
    let common = from
        .iter()
        .zip(to.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let mut parts: Vec<&str> = vec![".."; from.len() - common];
    parts.extend_from_slice(&to[common..]);
    parts.join("/")
}

/// 把一条**笔记相对链接**（`assets/x.png` / `../Att/x.png`）按笔记所在目录归一化为
/// **Vault 相对路径**（解析 `.`/`..`）。越出仓库根返回 `None`（不解析库外）。
#[must_use]
pub fn normalize_vault_rel(note_dir: &str, link: &str) -> Option<String> {
    let mut stack: Vec<&str> = note_dir.split('/').filter(|s| !s.is_empty()).collect();
    for seg in link.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                stack.pop()?;
            }
            s => stack.push(s),
        }
    }
    Some(stack.join("/"))
}

/// 库内路径的父目录（`Chats/a.md` → `Chats`；根文件 → `""`）。
#[must_use]
pub fn parent_dir(rel: &str) -> &str {
    rel.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// 一条链接目标的**唯一规范形**：写进 `links` 表、以及任何要拿它当路径用的地方都走这里。
///
/// **这是「链接目标」与「库内路径」之间唯一的换算口径**。链接是**相对源笔记**写的、
/// 且空格/括号转过义（[`encode_link_target`]），而库内路径是相对**库根**、不转义的——
/// 两者混用的代价是真实的：外链面板把 `../Transcripts/名字%20带空格.md` 直接交给打开
/// 笔记的命令，于是用户读到「路径越出 Vault 根」（第五轮真机上报）；反链按 `dst = 路径`
/// 匹配同样对不上，一篇被引用的笔记会显示"没有反链"。
///
/// 规则（顺序即优先级）：
/// 1. **裸 stem 原样返回**（既不含 `/`、也没有扩展名）——`[[项目 X]]` 靠 stem 匹配，
///    把它归一化成 `Notes/项目 X` 会让 wikilink 的反链与图谱边**一起断掉**；
/// 2. 百分号解码 → 按源笔记所在目录归一化；
/// 3. 归一化失败（真的越出库根）**原样返回**：它就是一条断链，如实留着让体检去报，
///    而不是在这里悄悄改成一个能打开的路径（那才是撒谎）。
#[must_use]
pub fn canonical_link_target(src_rel: &str, raw: &str) -> String {
    let trimmed = raw.trim();
    // 「有扩展名」而不是「以 .md 结尾」：同目录的附件链接（`封面%20图.png`）同样是路径，
    // 同样会被当作路径去打开——只放过 md 会留下同一个坑的另一半。
    let last_segment = trimmed.rsplit('/').next().unwrap_or(trimmed);
    let looks_like_path = trimmed.contains('/') || last_segment.contains('.');
    if trimmed.is_empty() || !looks_like_path {
        return raw.to_owned();
    }
    let decoded = percent_decode(trimmed);
    match normalize_vault_rel(parent_dir(src_rel), &decoded) {
        Some(normalized) if !normalized.is_empty() => normalized,
        _ => raw.to_owned(),
    }
}

/// 由 `from` 指向 `to` 的库内相对路径（两者都是 Vault 相对路径）。
#[must_use]
pub fn relative_link_path(from: &str, to: &str) -> String {
    let from_dirs: Vec<&str> = from.split('/').collect();
    let to_dirs: Vec<&str> = to.split('/').collect();
    let from_dir = &from_dirs[..from_dirs.len().saturating_sub(1)];
    let to_dir = &to_dirs[..to_dirs.len().saturating_sub(1)];
    let common = from_dir
        .iter()
        .zip(to_dir.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let mut parts: Vec<String> = std::iter::repeat_n("..".to_owned(), from_dir.len() - common)
        .chain(to_dir[common..].iter().map(|s| (*s).to_owned()))
        .collect();
    parts.push(to_dirs.last().copied().unwrap_or(to).to_owned());
    parts.join("/")
}

/// Markdown 链接目标里的空格与括号需转义，否则链接在渲染时断掉。
#[must_use]
pub fn encode_link_target(path: &str) -> String {
    path.replace(' ', "%20")
        .replace('(', "%28")
        .replace(')', "%29")
}

/// [`encode_link_target`] 的逆操作（`%XX` → 字节）。
///
/// 存在的理由是**解析侧要认得自己写出去的东西**：Lunarca 自动生成的链接对空格与括号做过
/// 转义（`项目%20X.md`），而链接解析（图谱/断链体检）拿这串去与笔记路径比对时必然对不上，
/// 表现为「一篇明明存在的笔记被画成未创建的幽灵节点」。
///
/// 只认标准 `%XX`：非法转义序列原样留下（用户手写的 `100%` 不该被吃掉半个字符）。
/// 解出的字节按 UTF-8 解释，非法字节走 lossy——链接目标不是可信输入，绝不 panic。
#[must_use]
pub fn percent_decode(target: &str) -> String {
    if !target.contains('%') {
        return target.to_owned();
    }
    let bytes = target.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_value(bytes[i + 1]), hex_value(bytes[i + 2])) {
                out.push(hi * 16 + lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
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
    #[test]
    fn hidden_paths_are_any_dot_segment() {
        assert!(is_hidden_rel(".obsidian/app.json"));
        assert!(is_hidden_rel("草稿/.私密.md"));
        assert!(!is_hidden_rel("草稿/私密.md"));
        assert!(!is_hidden_rel("v1.0 方案.md"));
    }

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

    #[test]
    fn relative_path_dotdot_and_descend() {
        assert_eq!(
            relative_vault_path("Chats", "Chats/assets/x.png"),
            "assets/x.png"
        );
        assert_eq!(relative_vault_path("", "x.png"), "x.png");
        assert_eq!(
            relative_vault_path("Chats", "Attachments/x.png"),
            "../Attachments/x.png"
        );
        assert_eq!(relative_vault_path("a/b", "a/c/x.png"), "../c/x.png");
    }

    #[test]
    fn name_key_matches_nfd_and_nfc_spellings_case_insensitively() {
        let nfd = "Cafe\u{301} \u{1100}\u{1167}\u{11a8}"; // e + 组合重音、韩文字母分解形
        assert_eq!(name_key(nfd), name_key("CAFÉ 격"));
        assert_eq!(nfc("周报"), "周报", "汉字的 NFC 与原文相同，不分配");
        assert!(matches!(nfc("周报"), std::borrow::Cow::Borrowed(_)));
    }

    #[test]
    fn normalize_resolves_dots_and_rejects_escape() {
        assert_eq!(
            normalize_vault_rel("Chats", "assets/x.png").as_deref(),
            Some("Chats/assets/x.png")
        );
        assert_eq!(
            normalize_vault_rel("Chats", "../Att/x.png").as_deref(),
            Some("Att/x.png")
        );
        assert_eq!(normalize_vault_rel("", "x.png").as_deref(), Some("x.png"));
        assert_eq!(
            normalize_vault_rel("a", "./x.png").as_deref(),
            Some("a/x.png")
        );
        assert_eq!(normalize_vault_rel("Chats", "../../etc/x"), None);
    }

    #[test]
    fn sanitize_strips_traversal_segments() {
        assert_eq!(sanitize_vault_rel_dir("../evil/sub"), "evil/sub");
        assert_eq!(sanitize_vault_rel_dir("/Att/"), "Att");
        assert_eq!(sanitize_vault_rel_dir("a/./b"), "a/b");
    }

    #[test]
    fn parent_dir_of_paths() {
        assert_eq!(parent_dir("Chats/a.md"), "Chats");
        assert_eq!(parent_dir("root.md"), "");
        assert_eq!(parent_dir("a/b/c.md"), "a/b");
    }
}
