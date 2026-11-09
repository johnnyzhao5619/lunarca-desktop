//! 文件命名的纯规则（无 IO、无时钟）：冲突副本名与撞名序号（ADR-148 ⑥）、新笔记的默认文件名、
//! 改名与库内路径的合法性。桌面（`luna-sync` 引擎、`luna-app` 的新建与改名命令）与网页版（`luna-web`）
//! 用的是这同一份——各写一份，同一篇笔记在两端就会得到两个名字。
//!
//! 「这个名字被占了没有」由调用方回答（桌面看磁盘与同步基线，网页看解开的清单）；撞名一律按
//! [`crate::path::name_key`] 比较（NFC + 小写）。

/// 一个本地时刻的年月日时分秒（调用方从自己的时钟取，核心不读时钟）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CivilTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

impl CivilTime {
    /// 冲突副本名里的日期：`YYYY-MM-DD`。
    #[must_use]
    pub fn date(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    /// 新笔记兜底文件名里的时间戳：`YYYYMMDD_HHMMSS`。
    #[must_use]
    pub fn stamp(&self) -> String {
        format!(
            "{:04}{:02}{:02}_{:02}{:02}{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }
}

// ─── 冲突副本 ─────────────────────────────────────────────────────────────────

/// 一个冲突副本最多试多少个名字（第 1 个不带序号，之后 `2`、`3`……）。
pub const CONFLICT_COPY_ATTEMPTS: u32 = 20;

/// 拆出目录（含末尾 `/`）、主名与扩展名（含 `.`）。以 `.` 开头的名字没有扩展名。
fn split_name(path: &str) -> (&str, &str, &str) {
    let (dir, name) = match path.rfind('/') {
        Some(at) => (&path[..=at], &path[at + 1..]),
        None => ("", path),
    };
    let (stem, ext) = match name.rfind('.') {
        Some(at) if at > 0 => (&name[..at], &name[at..]),
        _ => (name, ""),
    };
    (dir, stem, ext)
}

/// 冲突副本的路径：`名字 (说明 日期).扩展名`，重名时在日期后加序号（`attempt` 从 1 起，1 不带序号）。
/// 说明是本地化文案（桌面「冲突副本」、网页「冲突 网页」），日期是 [`CivilTime::date`]。
#[must_use]
pub fn conflict_copy_path(path: &str, label: &str, date: &str, attempt: u32) -> String {
    let (dir, stem, ext) = split_name(path);
    if attempt <= 1 {
        format!("{dir}{stem} ({label} {date}){ext}")
    } else {
        format!("{dir}{stem} ({label} {date} {attempt}){ext}")
    }
}

/// 第一个没被占的冲突副本路径（[`CONFLICT_COPY_ATTEMPTS`] 个名字都被占 → `None`）。
pub fn free_conflict_copy_path(
    path: &str,
    label: &str,
    date: &str,
    mut taken: impl FnMut(&str) -> bool,
) -> Option<String> {
    (1..=CONFLICT_COPY_ATTEMPTS)
        .map(|attempt| conflict_copy_path(path, label, date, attempt))
        .find(|candidate| !taken(candidate))
}

// ─── 新笔记的默认文件名 ───────────────────────────────────────────────────────

/// 由正文派生的文件名主名最多多少个字符。
pub const STEM_MAX_CHARS: usize = 48;

/// 一段正文的**标题行**：第一条有字的行，去掉行首的 Markdown 块标记（标题 `#`、引用 `>`）。
/// 文件名从正文首行派生的唯一判据。只剥标记、不剥字：`#标签 正文` 照旧留下「标签 正文」。
#[must_use]
pub fn title_line(text: &str) -> Option<&str> {
    text.lines().find_map(|line| {
        let title = line.trim().trim_start_matches(['#', '>']).trim();
        (!title.is_empty()).then_some(title)
    })
}

/// 把一段文字变成能当文件名主名的样子：路径分隔符、冒号、控制字符与空白折成单个 `-`，最多
/// [`STEM_MAX_CHARS`] 个字符，去掉首尾的 `-`、`.` 与空格；什么都不剩 → `None`。
#[must_use]
pub fn sanitized_file_stem(input: &str) -> Option<String> {
    let mut out = String::new();
    let mut pending_dash = false;
    for ch in input.chars() {
        if out.chars().count() >= STEM_MAX_CHARS {
            break;
        }
        if ch == '/'
            || ch == '\\'
            || ch == ':'
            || ch == '\0'
            || ch.is_control()
            || ch.is_whitespace()
        {
            pending_dash = true;
            continue;
        }
        if pending_dash && !out.is_empty() {
            out.push('-');
        }
        pending_dash = false;
        out.push(ch);
    }
    let trimmed = out.trim_matches(['-', '.', ' ']).to_owned();
    (!trimmed.is_empty()).then_some(trimmed)
}

/// 新笔记的文件名主名：正文标题行派生的名字；正文没有可用的字就是 `前缀_YYYYMMDD_HHMMSS`
/// （前缀是笔记类型的本地化名字，例如「笔记」/「Note」，由调用方给）。
#[must_use]
pub fn draft_stem(content: &str, fallback_prefix: &str, at: &CivilTime) -> String {
    title_line(content)
        .and_then(sanitized_file_stem)
        .unwrap_or_else(|| format!("{fallback_prefix}_{}", at.stamp()))
}

/// 新笔记重名时最多试多少个名字（第 0 个不带序号，之后 `-1`、`-2`……）。
pub const NUMBERED_NAME_ATTEMPTS: u32 = 100;

/// `dir` 下第 `index` 个候选：`主名.md`、`主名-1.md`、`主名-2.md`……（`dir` 为空即库根）。
#[must_use]
pub fn numbered_note_path(dir: &str, base: &str, index: u32) -> String {
    let name = if index == 0 {
        format!("{base}.md")
    } else {
        format!("{base}-{index}.md")
    };
    if dir.is_empty() {
        name
    } else {
        format!("{dir}/{name}")
    }
}

/// 第一个没被占的新笔记路径（[`NUMBERED_NAME_ATTEMPTS`] 个都被占 → `None`）。
pub fn free_note_path(
    dir: &str,
    base: &str,
    mut taken: impl FnMut(&str) -> bool,
) -> Option<String> {
    (0..NUMBERED_NAME_ATTEMPTS)
        .map(|index| numbered_note_path(dir, base, index))
        .find(|candidate| !taken(candidate))
}

// ─── 名字与路径的合法性 ───────────────────────────────────────────────────────

/// 一个名字（或路径里的一段）为什么不能用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum NameError {
    /// 空的（或只有空白）。
    #[error("invalid_name_empty")]
    Empty,
    /// `.` 或 `..`。
    #[error("invalid_name_reserved")]
    Reserved,
    /// 含路径分隔符或控制字符。
    #[error("invalid_name_separator")]
    Separator,
    /// 以 `.` 开头的段：库里的隐藏文件与机器文件（`.luna/`）不是用户内容。
    #[error("invalid_name_hidden")]
    Hidden,
}

impl NameError {
    /// 稳定码（网页 Worker 原样回给界面）。
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::Empty => "invalid_name_empty",
            Self::Reserved => "invalid_name_reserved",
            Self::Separator => "invalid_name_separator",
            Self::Hidden => "invalid_name_hidden",
        }
    }
}

/// 用户给的一个名字（文件或文件夹，不含路径）能不能用；能用就返回去掉首尾空白的样子。
///
/// # Errors
/// [`NameError::Empty`] / [`NameError::Reserved`] / [`NameError::Separator`]。
pub fn check_name(name: &str) -> Result<&str, NameError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(NameError::Empty);
    }
    if trimmed == "." || trimmed == ".." {
        return Err(NameError::Reserved);
    }
    if trimmed
        .chars()
        .any(|ch| ch == '/' || ch == '\\' || ch == '\0' || ch.is_control())
    {
        return Err(NameError::Separator);
    }
    Ok(trimmed)
}

/// 改名后的库相对路径：同目录、用户给的名字、**原扩展名**（目录没有扩展名）。用户已经把原扩展名
/// 打进去的（`x.png` → `y.png`）不再重复追加。
///
/// # Errors
/// 名字不能用（[`check_name`]）。
pub fn renamed_entry_path(
    current_path: &str,
    name: &str,
    is_dir: bool,
) -> Result<String, NameError> {
    let name = check_name(name)?;
    let (dir, _, ext) = split_name(current_path);
    let file_name = if is_dir || ext.is_empty() || name.ends_with(ext) {
        name.to_owned()
    } else {
        format!("{name}{ext}")
    };
    Ok(format!("{dir}{file_name}"))
}

/// 移到 `dir`（库相对的文件夹，库根为空串）之后的路径：文件名不变。
///
/// # Errors
/// `dir` 不是合法的库路径（[`check_vault_path`]）。
pub fn moved_entry_path(current_path: &str, dir: &str) -> Result<String, NameError> {
    let name = current_path.rsplit('/').next().unwrap_or(current_path);
    let dir = dir.trim_end_matches('/');
    if dir.is_empty() {
        return Ok(name.to_owned());
    }
    check_vault_path(dir)?;
    Ok(format!("{dir}/{name}"))
}

/// `dir`（库根为空串）里名为 `name` 的子项路径（新建文件夹）：名字过 [`check_name`]，拼好的路径过
/// [`check_vault_path`]。
///
/// # Errors
/// 名字或文件夹不能用。
pub fn child_entry_path(dir: &str, name: &str) -> Result<String, NameError> {
    let name = check_name(name)?;
    let dir = dir.trim_end_matches('/');
    let path = if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    };
    check_vault_path(&path)?;
    Ok(path)
}

/// 一个要写入的库相对路径（`/` 分隔）能不能用：每一段都过 [`check_name`] 且原样（不带首尾空白），
/// 没有空段、不以 `/` 开头，也不落在隐藏路径里。
///
/// # Errors
/// 第一段不合格的原因。
pub fn check_vault_path(path: &str) -> Result<(), NameError> {
    if path.is_empty() {
        return Err(NameError::Empty);
    }
    for segment in path.split('/') {
        if check_name(segment)? != segment {
            return Err(NameError::Separator);
        }
        if segment.starts_with('.') {
            return Err(NameError::Hidden);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_entry_path_checks_name_and_dir() {
        assert_eq!(child_entry_path("", " 归档 "), Ok("归档".to_owned()));
        assert_eq!(child_entry_path("项目", "2026"), Ok("项目/2026".to_owned()));
        assert_eq!(child_entry_path("项目", "a/b"), Err(NameError::Separator));
        assert_eq!(child_entry_path("项目", ".私密"), Err(NameError::Hidden));
        assert_eq!(child_entry_path("", "  "), Err(NameError::Empty));
    }

    #[test]
    fn moved_entry_path_keeps_the_name() {
        assert_eq!(
            moved_entry_path("项目/计划.md", ""),
            Ok("计划.md".to_owned())
        );
        assert_eq!(
            moved_entry_path("计划.md", "归档/2026"),
            Ok("归档/2026/计划.md".to_owned())
        );
        assert_eq!(
            moved_entry_path("计划.md", "归档/"),
            Ok("归档/计划.md".to_owned())
        );
        assert_eq!(moved_entry_path("计划.md", ".私密"), Err(NameError::Hidden));
    }

    const AT: CivilTime = CivilTime {
        year: 2026,
        month: 11,
        day: 6,
        hour: 9,
        minute: 5,
        second: 7,
    };

    #[test]
    fn civil_time_formats_the_date_and_the_stamp() {
        assert_eq!(AT.date(), "2026-11-06");
        assert_eq!(AT.stamp(), "20261106_090507");
    }

    #[test]
    fn conflict_copy_names_keep_the_extension_and_folder() {
        assert_eq!(
            conflict_copy_path("notes/周报.md", "冲突副本", "2026-10-09", 1),
            "notes/周报 (冲突副本 2026-10-09).md"
        );
        assert_eq!(
            conflict_copy_path("a.md", "conflict", "2026-10-09", 3),
            "a (conflict 2026-10-09 3).md"
        );
        assert_eq!(
            conflict_copy_path("Makefile", "c", "d", 1),
            "Makefile (c d)"
        );
        assert_eq!(conflict_copy_path(".env", "c", "d", 1), ".env (c d)");
        assert_eq!(
            conflict_copy_path("工作/周报.md", "冲突 网页", "2026-11-06", 1),
            "工作/周报 (冲突 网页 2026-11-06).md"
        );
    }

    #[test]
    fn a_taken_conflict_copy_name_gets_the_next_number_and_gives_up_after_the_budget() {
        let taken = [
            "a (冲突 网页 2026-11-06).md",
            "A (冲突 网页 2026-11-06 2).MD",
        ];
        let got = free_conflict_copy_path("a.md", "冲突 网页", "2026-11-06", |candidate| {
            taken
                .iter()
                .any(|t| crate::path::name_key(t) == crate::path::name_key(candidate))
        });
        assert_eq!(got.as_deref(), Some("a (冲突 网页 2026-11-06 3).md"));
        assert!(free_conflict_copy_path("a.md", "c", "d", |_| true).is_none());
    }

    #[test]
    fn the_title_line_skips_blank_lines_and_block_markers() {
        assert_eq!(title_line("\n\n## 周会纪要\n正文"), Some("周会纪要"));
        assert_eq!(title_line("> 引用\n"), Some("引用"));
        assert_eq!(title_line("#标签 正文"), Some("标签 正文"));
        assert_eq!(title_line("  \n#\n"), None);
    }

    #[test]
    fn file_stems_fold_separators_and_cap_their_length() {
        assert_eq!(
            sanitized_file_stem("hello from luna").as_deref(),
            Some("hello-from-luna")
        );
        assert_eq!(sanitized_file_stem("a/b:c\\d").as_deref(), Some("a-b-c-d"));
        assert_eq!(sanitized_file_stem(" .. -- ").as_deref(), None);
        assert_eq!(
            sanitized_file_stem(&"字".repeat(80))
                .unwrap()
                .chars()
                .count(),
            STEM_MAX_CHARS
        );
    }

    #[test]
    fn a_draft_stem_comes_from_the_heading_or_falls_back_to_the_prefix_and_stamp() {
        assert_eq!(draft_stem("# 周会纪要\n\n正文", "笔记", &AT), "周会纪要");
        assert_eq!(draft_stem("   ", "笔记", &AT), "笔记_20261106_090507");
        assert_eq!(draft_stem("", "Note", &AT), "Note_20261106_090507");
    }

    #[test]
    fn numbered_note_paths_start_bare_then_count_up() {
        assert_eq!(numbered_note_path("Notes", "周报", 0), "Notes/周报.md");
        assert_eq!(numbered_note_path("Notes", "周报", 2), "Notes/周报-2.md");
        assert_eq!(numbered_note_path("", "周报", 1), "周报-1.md");
        let got = free_note_path("Notes", "周报", |p| p == "Notes/周报.md");
        assert_eq!(got.as_deref(), Some("Notes/周报-1.md"));
        assert!(free_note_path("", "x", |_| true).is_none());
    }

    #[test]
    fn renamed_paths_keep_the_folder_and_the_extension() {
        assert_eq!(
            renamed_entry_path("projects/old.md", "weekly", false).unwrap(),
            "projects/weekly.md"
        );
        assert_eq!(
            renamed_entry_path("old.md", "weekly.md", false).unwrap(),
            "weekly.md"
        );
        assert_eq!(
            renamed_entry_path("Recordings/assets/a.wav", "会议录音", false).unwrap(),
            "Recordings/assets/会议录音.wav"
        );
        assert_eq!(
            renamed_entry_path("Notes/v1.0", "v2.0", true).unwrap(),
            "Notes/v2.0"
        );
        assert_eq!(
            renamed_entry_path("Makefile", " 新名 ", false).unwrap(),
            "新名"
        );
        assert_eq!(
            renamed_entry_path("old.md", "../escape", false),
            Err(NameError::Separator)
        );
        assert_eq!(
            renamed_entry_path("old.md", "nested/name", false),
            Err(NameError::Separator)
        );
        assert_eq!(
            renamed_entry_path("old.md", "  ", false),
            Err(NameError::Empty)
        );
        assert_eq!(
            renamed_entry_path("old.md", "..", false),
            Err(NameError::Reserved)
        );
    }

    #[test]
    fn vault_paths_reject_empty_reserved_hidden_and_padded_segments() {
        assert_eq!(check_vault_path("工作/周报.md"), Ok(()));
        assert_eq!(check_vault_path(""), Err(NameError::Empty));
        assert_eq!(check_vault_path("/abs.md"), Err(NameError::Empty));
        assert_eq!(check_vault_path("a//b.md"), Err(NameError::Empty));
        assert_eq!(check_vault_path("a/../b.md"), Err(NameError::Reserved));
        assert_eq!(check_vault_path(".luna/x.json"), Err(NameError::Hidden));
        assert_eq!(check_vault_path("a/ b.md"), Err(NameError::Separator));
        assert_eq!(check_vault_path("a\\b.md"), Err(NameError::Separator));
        assert_eq!(NameError::Hidden.code(), NameError::Hidden.to_string());
    }
}
