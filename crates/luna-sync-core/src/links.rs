//! 库内链接的解析口径（TDD-06 §3、ADR-177 ⑥）：隐藏路径、父目录、按笔记目录归一化、百分号解码、
//! 链接目标的规范形、标题、wikilink 目标的拆法、「这条链接指向哪一篇」（[`LinkIndex`]）与按嵌入写法
//! 找文件。纯逻辑、零 IO——桌面（`luna-vault`）与网页版（`luna-web` 的 WASM）共用这一份：两端各写一份
//! 的表现是同一条 `[[链接]]` 在桌面能点开、在网页上是「未创建」。

use std::collections::BTreeMap;

use crate::path::name_key;

/// 隐藏路径：库相对路径里任一段以 `.` 开头（`.obsidian/app.json`、`草稿/.私密.md`）。
///
/// Lunarca 把点文件当作「不是用户内容」：不进索引、不进搜索与链接解析，与启动扫描、文件监听、
/// 建树同一口径。「显示隐藏文件」（V4b）只让它们出现在文件树里，这条口径不变。
#[must_use]
pub fn is_hidden_rel(rel: &str) -> bool {
    rel.split('/').any(|segment| segment.starts_with('.'))
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

/// 标题 = 文件名 stem（去末尾 `.md`，大小写不敏感）。不引入独立 title 字段（§3）。
#[must_use]
pub fn title_of(rel_path: &str) -> String {
    let file = rel_path.rsplit('/').next().unwrap_or(rel_path);
    let stem = file
        .strip_suffix(".md")
        .or_else(|| file.strip_suffix(".MD"))
        .or_else(|| file.strip_suffix(".markdown"))
        .unwrap_or(file);
    stem.to_string()
}

/// wikilink `|` 之前的部分拆成「指向哪一篇」与「落到哪一处」：`笔记#标题` →
/// (`笔记`, `Some("标题")`)，`笔记#^块` → (`笔记`, `Some("^块")`)。片段不参与解析——
/// 把 `笔记#标题` 整串当笔记名，结果是一条明明存在的链接被画成「未创建」。
#[must_use]
pub fn split_wikilink_target(target: &str) -> (&str, Option<&str>) {
    match target.split_once('#') {
        Some((note, fragment)) => {
            let fragment = fragment.trim();
            (note.trim(), (!fragment.is_empty()).then_some(fragment))
        }
        None => (target.trim(), None),
    }
}

/// 链接目标 → 笔记路径的解析表（**全仓唯一**的「这条链接指向哪一篇」实现）。
///
/// 图谱建边与知识体检（PRD-16-030 断链 / 失效引用）共用它：两处各写一份解析，必然在
/// "转义要不要解码""相对路径按谁归一化"上漂移，而漂移的表现是**同一条链接在图谱里是
/// 实线、在体检里是断链**。
pub struct LinkIndex<'a> {
    by_path: BTreeMap<String, &'a str>,
    /// 同名笔记全部留下（路径升序）：选哪一篇要看链接写在哪，见 [`Self::pick_same_name`]。
    by_stem: BTreeMap<String, Vec<&'a str>>,
    /// frontmatter `aliases` → 笔记（对标方案 C2）。只在路径与文件名都认不出时才查：
    /// 一篇真叫这个名字的笔记永远优先于别人的别名（Obsidian 同口径）。
    by_alias: BTreeMap<String, Vec<&'a str>>,
}

impl<'a> LinkIndex<'a> {
    /// 由「(路径, 标题)」序列构建。
    #[must_use]
    pub fn build(notes: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        let mut entries: Vec<(&'a str, &'a str)> = notes.into_iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        let mut by_path = BTreeMap::new();
        let mut by_stem: BTreeMap<String, Vec<&'a str>> = BTreeMap::new();
        for (path, title) in entries {
            by_path.insert(name_key(path), path);
            by_stem.entry(name_key(title)).or_default().push(path);
        }
        Self {
            by_path,
            by_stem,
            by_alias: BTreeMap::new(),
        }
    }

    /// 加上别名（`(笔记路径, 别名)`）；指向库里没有的笔记的别名忽略。几篇同用一个别名时
    /// 与同名笔记同一套挑法（[`Self::pick_same_name`]）。
    #[must_use]
    pub fn with_aliases(mut self, aliases: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        for (path, alias) in aliases {
            if let Some(&known) = self.by_path.get(&name_key(path)) {
                let paths = self.by_alias.entry(name_key(alias.trim())).or_default();
                if !paths.contains(&known) {
                    paths.push(known);
                }
            }
        }
        self
    }

    /// 由索引给的两张表（`(路径, 标题)` 与 `(路径, 别名)`）构建——解析、补全共用的入口。
    #[must_use]
    pub fn from_rows(notes: &'a [(String, String)], aliases: &'a [(String, String)]) -> Self {
        Self::build(
            notes
                .iter()
                .map(|(path, title)| (path.as_str(), title.as_str())),
        )
        .with_aliases(
            aliases
                .iter()
                .map(|(path, alias)| (path.as_str(), alias.as_str())),
        )
    }

    /// 同名笔记里选一篇（Obsidian 口径）：与写链接的笔记**同目录**的优先，其次层级最浅、
    /// 路径最短，最后按路径字典序——任何一步都确定，同一条链接永远指向同一篇。
    fn pick_same_name(src: &str, paths: &[&'a str]) -> Option<&'a str> {
        let src_dir = parent_dir(src);
        paths.iter().copied().min_by_key(|path| {
            (
                parent_dir(path) != src_dir,
                path.matches('/').count(),
                path.chars().count(),
                *path,
            )
        })
    }

    /// 一种写法的查表：依次尝试 ① 精确路径 ② 补 `.md` 的路径（仅带目录的写法）③ 文件名 stem
    /// ④ frontmatter 别名（均大小写不敏感，与反链 `dst = 路径 OR stem OR 别名` 口径同源）。
    fn lookup(&self, src: &str, candidate: &str) -> Option<&'a str> {
        let lower = name_key(candidate.trim().trim_start_matches("./"));
        if lower.is_empty() {
            return None;
        }
        if let Some(path) = self.by_path.get(&lower) {
            return Some(path);
        }
        // 补 `.md` 只给带目录的写法：裸名字要走同名消歧，否则库根那篇永远抢在同目录那篇前面。
        if lower.contains('/') {
            if let Some(path) = self.by_path.get(&format!("{lower}.md")) {
                return Some(path);
            }
        }
        self.by_stem
            .get(&lower)
            .or_else(|| self.by_alias.get(&lower))
            .and_then(|paths| Self::pick_same_name(src, paths))
    }

    /// 解析一条链接（`src` = 写下这条链接的笔记）。三种写法**依次尝试、命中即止**
    /// （严格是原有行为的超集：只会多认出笔记，绝不会少认——第一步就是原样查表）：
    ///
    /// 1. **原样**：wikilink stem 与同目录 md 链接走这条；
    /// 2. **百分号解码后**：Lunarca 自动生成的链接对空格/括号转过义
    ///    （[`encode_link_target`]），`项目%20X.md` 原样查表必然落空；
    /// 3. **按源笔记所在目录归一化后**：跨文件夹的自动链接恒是 `../Notes/x.md` 这种
    ///    形态（出处行、蒸馏来源区、归入源链接），只按字面查表同样落空。
    ///
    /// 后两条缺失的代价不是"少一条边"，而是**一篇存在的笔记被画成幽灵节点**——图谱上
    /// 凭空多出未创建笔记，而知识体检会把它当成真断链报给用户。
    #[must_use]
    pub fn resolve(&self, src: &str, dst: &str) -> Option<&'a str> {
        self.resolve_exact(src, dst).or_else(|| {
            // 旧索引行里的 wikilink 可能还带着 `#标题`（片段剥离之前写入的），按笔记部分再认一次；
            // 只在原样认不出时才试，文件名里真带 `#` 的笔记不受影响。
            let (note, fragment) = split_wikilink_target(dst);
            fragment.and_then(|_| self.resolve_exact(src, note))
        })
    }

    fn resolve_exact(&self, src: &str, dst: &str) -> Option<&'a str> {
        if let Some(path) = self.lookup(src, dst) {
            return Some(path);
        }
        let decoded = percent_decode(dst.trim());
        if decoded != dst.trim() {
            if let Some(path) = self.lookup(src, &decoded) {
                return Some(path);
            }
        }
        // **不限于含 `/` 的目标**：同目录链接 `[纪要](四月纪要.md)` 是归入源链接与出处行
        // 最常见的形态，而它按字面既不是库内路径（缺目录）也不是 stem（多了 `.md`）——
        // 不按源笔记目录归一化就永远解析不到，于是一篇存在的笔记被画成幽灵。
        // 换算走 `canonical_link_target`：与写进 `links` 表的口径**同一处实现**（schema v7），
        // 否则"表里存的形态"与"解析时认的形态"会各自漂移。
        let canonical = canonical_link_target(src, dst);
        (canonical != dst)
            .then(|| self.lookup(src, &canonical))
            .flatten()
    }

    /// 这条路径在不在库里（体检判「链接指向的笔记还在不在」用）。
    #[must_use]
    pub fn contains_path(&self, path: &str) -> bool {
        self.by_path.contains_key(&name_key(path))
    }
}

/// `[[` 补全的一条候选（PRD-06-031；网页版 PRD-21-020）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct WikiLinkCandidate {
    /// 笔记标题（= 文件名去扩展名）。
    pub title: String,
    /// 选中后写进 `[[ ]]` 的文字。
    pub insert: String,
    /// 按 frontmatter 别名命中时的那个别名（写成 `[[insert|别名]]`）；按标题命中为 `None`。
    pub alias: Option<String>,
    /// 所在目录（库根为空串），同名笔记靠它区分。
    pub folder: String,
    /// 库内相对路径。
    pub path: String,
}

/// `[[` 补全候选：标题或 frontmatter 别名包含 `query` 的笔记（名字比较键），以它开头的排前、短的排前、
/// 层级浅的排前，最后按路径；`src` 自己不在其中。`insert` 按名字能解析回这一篇（[`LinkIndex`]，同名消歧）
/// 就写名字，否则写不带 `.md` 的路径——同名笔记各有一个指得回自己的写法。
///
/// `notes` 是 `(路径, 标题)`、`aliases` 是 `(路径, 别名)`：桌面由索引给出两张表，网页只有清单、没有别名。
#[must_use]
pub fn wikilink_candidates(
    notes: &[(String, String)],
    aliases: &[(String, String)],
    src: &str,
    query: &str,
    limit: usize,
) -> Vec<WikiLinkCandidate> {
    let index = LinkIndex::from_rows(notes, aliases);
    let titles: BTreeMap<&str, &str> = notes
        .iter()
        .map(|(path, title)| (path.as_str(), title.as_str()))
        .collect();
    let wanted = name_key(query.trim());
    // (路径, 标题, 命中的文字, 别名)
    let mut hits: Vec<(&str, &str, &str, Option<&str>)> = notes
        .iter()
        .filter(|(path, title)| path != src && name_key(title).contains(&wanted))
        .map(|(path, title)| (path.as_str(), title.as_str(), title.as_str(), None))
        .collect();
    hits.extend(aliases.iter().filter_map(|(path, alias)| {
        let title = *titles.get(path.as_str())?;
        (path != src && name_key(alias).contains(&wanted)).then_some((
            path.as_str(),
            title,
            alias.as_str(),
            Some(alias.as_str()),
        ))
    }));
    hits.sort_by_key(|(path, _, matched, alias)| {
        (
            !name_key(matched).starts_with(&wanted),
            matched.chars().count(),
            path.matches('/').count(),
            *path,
            alias.is_some(),
        )
    });
    hits.into_iter()
        .take(limit)
        .map(|(path, title, _, alias)| WikiLinkCandidate {
            insert: if index.resolve(src, title) == Some(path) {
                title.to_owned()
            } else {
                path.strip_suffix(".md").unwrap_or(path).to_owned()
            },
            title: title.to_owned(),
            alias: alias.map(str::to_owned),
            folder: parent_dir(path).to_owned(),
            path: path.to_owned(),
        })
        .collect()
}

pub const IMAGE_EXTENSIONS: [&str; 12] = [
    "png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "avif", "tif", "tiff", "ico", "heic",
];

pub fn extension_of(name: &str) -> Option<String> {
    let file = name.rsplit('/').next().unwrap_or(name);
    let (_, ext) = file.rsplit_once('.')?;
    (!ext.is_empty()).then(|| ext.to_ascii_lowercase())
}

/// 按嵌入写法找文件（Obsidian 口径）：先按宿主目录的相对路径、再按库根路径精确找；都没有就按
/// 文件名在全库找，同名时与宿主同目录的优先，其次层级最浅、路径最短、字典序——结果确定。
pub fn find_file(files: &[String], host: &str, wanted: &str) -> Option<String> {
    let exact = |candidate: &str| {
        files
            .iter()
            .find(|f| f.eq_ignore_ascii_case(candidate))
            .cloned()
    };
    let decoded = percent_decode(wanted);
    if let Some(path) = normalize_vault_rel(parent_dir(host), &decoded).and_then(|p| exact(&p)) {
        return Some(path);
    }
    if let Some(path) = exact(decoded.trim_start_matches('/')) {
        return Some(path);
    }
    if decoded.contains('/') {
        return None;
    }
    let host_dir = parent_dir(host);
    files
        .iter()
        .filter(|f| {
            f.rsplit('/')
                .next()
                .is_some_and(|name| name.eq_ignore_ascii_case(&decoded))
        })
        .min_by_key(|f| {
            (
                parent_dir(f) != host_dir,
                f.matches('/').count(),
                f.chars().count(),
                (*f).clone(),
            )
        })
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(items: &[(&str, &str)]) -> Vec<(String, String)> {
        items
            .iter()
            .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
            .collect()
    }

    #[test]
    fn wikilink_candidates_rank_and_disambiguate() {
        let notes = rows(&[
            ("周报.md", "周报"),
            ("项目/周报.md", "周报"),
            ("项目/周报草稿.md", "周报草稿"),
            ("本周周报.md", "本周周报"),
            ("甲.md", "甲"),
        ]);
        let got = wikilink_candidates(&notes, &[], "甲.md", "周报", 10);
        let paths: Vec<&str> = got.iter().map(|c| c.path.as_str()).collect();
        // 以查询开头的排前、短的排前、浅的排前；不以它开头的垫底。
        assert_eq!(
            paths,
            ["周报.md", "项目/周报.md", "项目/周报草稿.md", "本周周报.md"]
        );
        // 同名两篇：库根那篇按名字解析得回自己，子目录那篇写路径。
        assert_eq!(got[0].insert, "周报");
        assert_eq!(got[1].insert, "项目/周报");
        assert_eq!(got[1].folder, "项目");
        // 自己不在候选里；限额生效。
        assert!(wikilink_candidates(&notes, &[], "甲.md", "甲", 10).is_empty());
        assert_eq!(wikilink_candidates(&notes, &[], "甲.md", "", 2).len(), 2);
    }

    #[test]
    fn wikilink_candidates_by_alias() {
        let notes = rows(&[("计划.md", "计划")]);
        let aliases = rows(&[("计划.md", "Roadmap")]);
        let got = wikilink_candidates(&notes, &aliases, "甲.md", "road", 10);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].alias.as_deref(), Some("Roadmap"));
        assert_eq!(got[0].insert, "计划");
    }

    #[test]
    fn hidden_paths_are_any_dot_segment() {
        assert!(is_hidden_rel(".obsidian/app.json"));
        assert!(is_hidden_rel("草稿/.私密.md"));
        assert!(!is_hidden_rel("草稿/私密.md"));
        assert!(!is_hidden_rel("v1.0 方案.md"));
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
    fn parent_dir_of_paths() {
        assert_eq!(parent_dir("Chats/a.md"), "Chats");
        assert_eq!(parent_dir("root.md"), "");
        assert_eq!(parent_dir("a/b/c.md"), "a/b");
    }
}
