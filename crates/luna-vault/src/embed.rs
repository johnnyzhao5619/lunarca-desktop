//! Obsidian 嵌入 `![[…]]` 的解析（对标方案 D5）。
//!
//! 一条嵌入指向三种东西之一：库里的一个文件（图片 / 附件，按文件名找，与 Obsidian 同口径）、
//! 一篇笔记或其中一节（`![[笔记]]` / `![[笔记#标题]]` / `![[笔记#^块]]`，按 wikilink 同一份
//! [`crate::graph::LinkIndex`] 认，含别名），或者找不到。前端只拿结果渲染，不比对文件名。
//!
//! 被嵌入笔记里的相对链接按**宿主笔记**的位置改写（[`rebase_links`]）：嵌入内容是在宿主里
//! 显示、跟着宿主导出的，它的 `![](assets/图.png)` 若仍按原笔记目录解析，换个文件夹就成了破图。

use crate::frontmatter::body_of;
use crate::pathutil::{canonical_link_target, encode_link_target, parent_dir, relative_link_path};
use crate::Vault;

/// 一条嵌入指向什么。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EmbedHit {
    /// 库里的一个非笔记文件。`link` 是从宿主笔记指向它的相对路径（未转义），可直接当
    /// `![](link)` 的目标用——阅读面与导出都按「笔记相对」解析图片。
    File {
        path: String,
        link: String,
        image: bool,
    },
    /// 一篇笔记（或其中一节）：正文已去掉 frontmatter，相对链接已按宿主改写。
    Note { path: String, content: String },
    /// 指不到（笔记不存在、片段不在、文件不在）。
    Missing,
}

const IMAGE_EXTENSIONS: [&str; 12] = [
    "png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "avif", "tif", "tiff", "ico", "heic",
];

fn extension_of(name: &str) -> Option<String> {
    let file = name.rsplit('/').next().unwrap_or(name);
    let (_, ext) = file.rsplit_once('.')?;
    (!ext.is_empty()).then(|| ext.to_ascii_lowercase())
}

fn heading_level(line: &str) -> Option<usize> {
    crate::parse::atx_heading_text(line)?;
    Some(
        line.trim_start_matches(' ')
            .chars()
            .take_while(|c| *c == '#')
            .count(),
    )
}

/// 一篇笔记里被嵌入的那一段：没有片段 = 整篇正文；`#标题` = 从那个标题到下一个同级或更高级
/// 标题之前（与大纲、折叠同一口径）；`#^块` = 带这个块标记的那一行（去掉标记）。找不到为 `None`。
#[must_use]
pub fn note_section(content: &str, fragment: Option<&str>) -> Option<String> {
    let body = body_of(content);
    let Some(fragment) = fragment.map(str::trim).filter(|f| !f.is_empty()) else {
        return Some(body.to_owned());
    };
    let lines: Vec<&str> = body.lines().collect();
    let mut fence: Option<&str> = None;
    let mut start: Option<(usize, usize)> = None; // (行号, 标题级别)
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if let Some(open) = fence {
            if trimmed.starts_with(open) {
                fence = None;
            }
            continue;
        }
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fence = Some(&trimmed[..3]);
            continue;
        }
        if let Some(id) = fragment.strip_prefix('^').map(str::trim) {
            let tail = line.trim_end();
            if let Some(rest) = tail.strip_suffix(id) {
                if !id.is_empty() && (rest.ends_with(" ^") || rest == "^") {
                    return Some(rest.trim_end_matches('^').trim_end().to_owned());
                }
            }
            continue;
        }
        let Some(level) = heading_level(line) else {
            continue;
        };
        match start {
            None => {
                let text = crate::parse::atx_heading_text(line).unwrap_or("");
                if text.trim().to_lowercase() == fragment.to_lowercase() {
                    start = Some((index, level));
                }
            }
            Some((from, open_level)) if level <= open_level => {
                return Some(lines[from..index].join("\n").trim_end().to_owned());
            }
            Some(_) => {}
        }
    }
    start.map(|(from, _)| lines[from..].join("\n").trim_end().to_owned())
}

/// 把 `content`（原本在 `from_note` 里）的相对 md 链接 / 图片改写成从 `to_note` 出发的写法。
/// 外链、锚点、绝对路径、尖括号目标与带标题的目标原样不动；围栏代码里的不动。
#[must_use]
pub fn rebase_links(content: &str, from_note: &str, to_note: &str) -> String {
    if parent_dir(from_note) == parent_dir(to_note) {
        return content.to_owned();
    }
    let mut out = String::with_capacity(content.len());
    let mut fence: Option<String> = None;
    for raw in content.split_inclusive('\n') {
        let trimmed = raw.trim_start();
        if let Some(open) = &fence {
            if trimmed.starts_with(open.as_str()) {
                fence = None;
            }
            out.push_str(raw);
            continue;
        }
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fence = Some(trimmed[..3].to_owned());
            out.push_str(raw);
            continue;
        }
        let mut rest = raw;
        while let Some(at) = rest.find("](") {
            let (head, tail) = rest.split_at(at + 2);
            out.push_str(head);
            let Some(close) = tail.find(')') else {
                rest = tail;
                break;
            };
            let dest = &tail[..close];
            let local = !dest.is_empty()
                && !dest.contains([' ', '<', '>', ':'])
                && !dest.starts_with(['#', '/']);
            if local {
                let target = canonical_link_target(from_note, dest);
                out.push_str(&encode_link_target(&relative_link_path(to_note, &target)));
            } else {
                out.push_str(dest);
            }
            rest = &tail[close..];
        }
        out.push_str(rest);
    }
    out
}

impl Vault {
    /// 一批嵌入目标（`![[…]]` 里 `|` 之前的文字）各自指向什么，`host` 是写这条嵌入的笔记。
    pub fn resolve_embeds(&self, host: &str, targets: &[String]) -> crate::Result<Vec<EmbedHit>> {
        let (notes, aliases) = self.link_rows()?;
        let index = crate::graph::LinkIndex::from_rows(&notes, &aliases);
        // 按文件名找附件要走一遍库；只在真有文件嵌入时走，且一批只走一次。
        let mut files: Option<Vec<String>> = None;
        let mut out = Vec::with_capacity(targets.len());
        for target in targets {
            let (note, fragment) = crate::parse::split_wikilink_target(target.trim());
            let note = note.trim();
            let hit = match extension_of(note) {
                Some(ext) if ext != "md" => {
                    let files = match &mut files {
                        Some(files) => files,
                        None => files.insert(self.walk_files()?),
                    };
                    match find_file(files, host, note) {
                        Some(path) => EmbedHit::File {
                            link: relative_link_path(host, &path),
                            image: IMAGE_EXTENSIONS.contains(&ext.as_str()),
                            path,
                        },
                        None => EmbedHit::Missing,
                    }
                }
                _ => {
                    let path = if note.is_empty() {
                        Some(host.to_owned())
                    } else {
                        index.resolve(host, note).map(str::to_owned)
                    };
                    path.and_then(|path| {
                        let doc = self.read(&path).ok()?;
                        let section = note_section(&doc.content, fragment)?;
                        Some(EmbedHit::Note {
                            content: rebase_links(&section, &path, host),
                            path,
                        })
                    })
                    .unwrap_or(EmbedHit::Missing)
                }
            };
            out.push(hit);
        }
        Ok(out)
    }

    /// 库里的全部文件（Vault 相对路径；点开头的目录跳过）。
    pub(crate) fn walk_files(&self) -> crate::Result<Vec<String>> {
        let mut out = Vec::new();
        walk_all(self.root(), self.root(), &mut out)?;
        // iCloud 未下载的文件也算在（`![[图.png]]` 指向它不是断链，[`crate::icloud`]）。
        for path in self.cloud_placeholders() {
            if !out.contains(&path) {
                out.push(path);
            }
        }
        Ok(out)
    }
}

pub(crate) fn walk_all(
    root: &std::path::Path,
    dir: &std::path::Path,
    out: &mut Vec<String>,
) -> crate::Result<()> {
    for entry in std::fs::read_dir(dir)?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            // 读不了的子目录跳过，不让整次遍历失败；与增量扫描、文件树同一口径（V1）。
            let _ = walk_all(root, &path, out);
        } else if kind.is_symlink() && path.is_dir() {
            // 指向目录的链接不跟随，也不当成文件列出来。
            continue;
        } else if let Ok(rel) = path.strip_prefix(root) {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}

/// 按嵌入写法找文件（Obsidian 口径）：先按宿主目录的相对路径、再按库根路径精确找；都没有就按
/// 文件名在全库找，同名时与宿主同目录的优先，其次层级最浅、路径最短、字典序——结果确定。
pub(crate) fn find_file(files: &[String], host: &str, wanted: &str) -> Option<String> {
    let exact = |candidate: &str| {
        files
            .iter()
            .find(|f| f.eq_ignore_ascii_case(candidate))
            .cloned()
    };
    let decoded = crate::pathutil::percent_decode(wanted);
    if let Some(path) =
        crate::pathutil::normalize_vault_rel(parent_dir(host), &decoded).and_then(|p| exact(&p))
    {
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

    const NOTE: &str = "---\ntags: [a]\n---\n# 周会\n\n导语\n\n## 结论\n\n- 上线 ^k1\n\n### 细节\n\n```\n## 不是标题\n```\n\n## 下周\n\n计划\n";

    #[test]
    fn sections_follow_heading_levels_and_block_markers() {
        assert_eq!(note_section(NOTE, None).unwrap(), body_of(NOTE));
        assert_eq!(
            note_section(NOTE, Some("结论")).unwrap(),
            "## 结论\n\n- 上线 ^k1\n\n### 细节\n\n```\n## 不是标题\n```"
        );
        assert_eq!(note_section(NOTE, Some("下周")).unwrap(), "## 下周\n\n计划");
        assert_eq!(note_section(NOTE, Some("^k1")).unwrap(), "- 上线");
        assert_eq!(note_section(NOTE, Some("不存在")), None);
        assert_eq!(note_section(NOTE, Some("不是标题")), None);
    }

    #[test]
    fn links_are_rebased_to_the_host_note() {
        let content = "![图](assets/图.png) [另一篇](../其它/笔记.md) [站](https://x.com) [锚](#h)\n```\n![x](assets/x.png)\n```\n";
        assert_eq!(
            rebase_links(content, "项目/周会.md", "日记/今天.md"),
            "![图](../项目/assets/图.png) [另一篇](../其它/笔记.md) [站](https://x.com) [锚](#h)\n```\n![x](assets/x.png)\n```\n"
        );
        assert_eq!(rebase_links(content, "a/x.md", "a/y.md"), content);
    }

    #[test]
    fn files_are_found_by_path_then_by_name() {
        let files = vec![
            "assets/图.png".to_owned(),
            "项目/assets/图.png".to_owned(),
            "项目/周会.md".to_owned(),
            "深/层/目录/报告.pdf".to_owned(),
        ];
        assert_eq!(
            find_file(&files, "项目/周会.md", "assets/图.png").as_deref(),
            Some("项目/assets/图.png")
        );
        assert_eq!(
            find_file(&files, "日记.md", "图.png").as_deref(),
            Some("assets/图.png")
        );
        assert_eq!(
            find_file(&files, "日记.md", "报告.pdf").as_deref(),
            Some("深/层/目录/报告.pdf")
        );
        assert_eq!(find_file(&files, "日记.md", "没有.png"), None);
    }
}
