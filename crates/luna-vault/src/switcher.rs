//! 快速切换器的条目（PRD-07-040）：库里每个文件一条，带标题、别名与拼音检索键。
//!
//! 文件集合与文件树同源（[`crate::Vault::tree`]）：树上看得见的，切换器里就找得到；知识
//! 产物不进树（ADR-096 ②），这里也不出现。别名来自索引（frontmatter `aliases`），拼音键
//! 对标题与每个别名现算（[`crate::pinyin_keys`]）。

use crate::vault::NoteTree;
use crate::Vault;
use std::collections::HashMap;

/// 一条切换器条目。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwitcherEntry {
    pub path: String,
    /// 文件名去掉 `.md`。
    pub title: String,
    pub aliases: Vec<String>,
    /// 标题与别名的拼音检索键（全拼与首字母，去重）。
    pub pinyin: Vec<String>,
    /// 资料区（ADR-024）：切换器里降权，不排除。
    pub is_source_material: bool,
}

impl Vault {
    /// 快速切换器的全部条目。
    pub fn switcher_entries(&self) -> crate::Result<Vec<SwitcherEntry>> {
        let mut aliases: HashMap<String, Vec<String>> = HashMap::new();
        for (path, alias) in self.index().note_aliases()? {
            aliases.entry(path).or_default().push(alias);
        }
        let mut out = Vec::new();
        collect(&self.tree()?, &mut aliases, &mut out);
        // 排除路径（V4a）不出现在切换器里；正文链接与文件树照常能打开它们。
        let excluded = self.exclusions();
        out.retain(|entry| !excluded.covers(&entry.path));
        Ok(out)
    }
}

fn collect(
    node: &NoteTree,
    aliases: &mut HashMap<String, Vec<String>>,
    out: &mut Vec<SwitcherEntry>,
) {
    for child in &node.children {
        if child.is_dir {
            collect(child, aliases, out);
            continue;
        }
        // 标题与别名只用来显示与匹配，给 NFC 形态：文件名可能以 NFD 落盘，而用户敲的查询是
        // NFC（ADR-148 ⑥）；两种形态显示上没有区别。`path` 照原样，打开文件用它。
        let title =
            luna_sync_core::path::nfc(child.name.strip_suffix(".md").unwrap_or(&child.name))
                .into_owned();
        let aliases: Vec<String> = aliases
            .remove(&child.path)
            .unwrap_or_default()
            .iter()
            .map(|alias| luna_sync_core::path::nfc(alias).into_owned())
            .collect();
        let mut pinyin = crate::pinyin_keys(&title);
        for alias in &aliases {
            for key in crate::pinyin_keys(alias) {
                if !pinyin.contains(&key) {
                    pinyin.push(key);
                }
            }
        }
        out.push(SwitcherEntry {
            path: child.path.clone(),
            title,
            aliases,
            pinyin,
            is_source_material: child.is_source_material,
        });
    }
}

#[cfg(test)]
mod tests {
    use crate::Vault;

    #[test]
    fn entries_carry_titles_aliases_and_pinyin_for_every_file_in_the_tree() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        vault
            .write("项目/会议纪要.md", "---\naliases: [周会]\n---\n正文")
            .unwrap();
        vault.write("Weekly.md", "x").unwrap();
        std::fs::write(dir.path().join("项目/图.png"), b"png").unwrap();
        let mut entries = vault.switcher_entries().unwrap();
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        let paths: Vec<_> = entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec!["Weekly.md", "项目/会议纪要.md", "项目/图.png"]);
        let meeting = &entries[1];
        assert_eq!(meeting.title, "会议纪要");
        assert_eq!(meeting.aliases, vec!["周会".to_owned()]);
        assert!(meeting.pinyin.contains(&"hyjy".to_owned()));
        assert!(
            meeting.pinyin.contains(&"zhouhui".to_owned()),
            "别名也出拼音键"
        );
        assert!(entries[0].pinyin.is_empty());
    }
}
