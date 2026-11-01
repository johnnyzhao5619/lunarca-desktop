//! `luna vault search | index`、`luna note list | show | new`（PRD-08-003 / 004，ADR-141 ④）。
//!
//! **只读**：搜索、列清单、看正文都以只读方式打开库（[`luna_vault::Vault::open_read_only`]），
//! 不往库里写任何东西；应用开着时与它并发读同一份索引（SQLite WAL）。唯二例外：
//! - `vault index` 写的是**派生索引**，且应用在跑时拒绝执行——两个进程同时写同一份索引没有赢家；
//! - `note new` 不写库，只把「新建」转成 `lunarca://new` 链接交给应用，由用户确认后才落盘。
//!
//! 结果走 stdout（`--json` 时是一份 JSON），诊断走 stderr；查询词、正文不进任何日志
//! （ADR-141 ⑥）。

use clap::{Args, Subcommand};
use luna_vault::registry::VaultEntry;
use luna_vault::scan::Drift;
use luna_vault::search::NoteMatch;
use luna_vault::NoteListEntry;
use luna_vault::{SearchQuery, Vault, VaultError};
use serde::Serialize;
use std::io::{IsTerminal as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::{EXIT_APP_RUNNING, EXIT_EMPTY, EXIT_INDEX, EXIT_NOTE, EXIT_USAGE};

/// 库命令共用的选项。
#[derive(Args, Clone, Debug, Default)]
pub(crate) struct VaultOpts {
    /// 库名或库 id（应用里登记过的库）；缺省依次取 `LUNA_VAULT_ROOT`、应用当前打开的库。
    #[arg(long)]
    vault: Option<String>,
    /// 以 JSON 输出（字段与应用里的结果模型同名）。
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand)]
pub(crate) enum VaultCommand {
    /// 搜索（与应用搜索视图同一套查询语法：`"短语"`、`-排除`、`tag:`、`path:` …）。
    /// 没有命中时退出码 4。
    Search {
        /// 查询。
        query: String,
        /// 区分大小写。
        #[arg(long)]
        match_case: bool,
        #[command(flatten)]
        opts: VaultOpts,
    },
    /// 把索引更新到磁盘上的样子（应用在跑时拒绝：应用自己在维护索引）。
    Index {
        /// 全部重读一遍（缺省只读新出现与改过的）。
        #[arg(long)]
        full: bool,
        #[command(flatten)]
        opts: VaultOpts,
    },
}

#[derive(Subcommand)]
pub(crate) enum NoteCommand {
    /// 列出库里的笔记（按路径排序，一行一篇）。
    List {
        /// 只列这个文件夹（库相对路径）下的。
        folder: Option<String>,
        #[command(flatten)]
        opts: VaultOpts,
    },
    /// 输出一篇的正文（不含 frontmatter）；`路径#标题` 只取那一节。`.md` 可省。
    Show {
        /// 库相对路径，可带 `#标题`。
        note: String,
        /// 输出整个文件（含 frontmatter），不能与 `#标题` 同用。
        #[arg(long)]
        raw: bool,
        #[command(flatten)]
        opts: VaultOpts,
    },
    /// 新建一篇：交给 Lunarca 弹出预填名字的新建卡，你确认后才写盘（CLI 不直接写库）。
    New {
        /// 笔记名字。
        name: String,
        /// 只打印链接，不交给应用。
        #[arg(long)]
        print: bool,
        #[command(flatten)]
        opts: VaultOpts,
    },
}

/// 一次失败：退出码 + 给人看的一句话（走 stderr）。
struct Failure {
    code: u8,
    message: String,
}

impl Failure {
    fn new(code: u8, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

type Outcome = Result<ExitCode, Failure>;

fn finish(outcome: Outcome) -> ExitCode {
    outcome.unwrap_or_else(|failure| {
        eprintln!("luna: {}", failure.message);
        ExitCode::from(failure.code)
    })
}

pub(crate) fn run_vault(command: VaultCommand) -> ExitCode {
    finish(match command {
        VaultCommand::Search {
            query,
            match_case,
            opts,
        } => search(&query, match_case, &opts),
        VaultCommand::Index { full, opts } => index(full, &opts),
    })
}

pub(crate) fn run_note(command: NoteCommand) -> ExitCode {
    finish(match command {
        NoteCommand::List { folder, opts } => list(folder.as_deref(), &opts),
        NoteCommand::Show { note, raw, opts } => show(&note, raw, &opts),
        NoteCommand::New { name, print, opts } => new(&name, print, &opts),
    })
}

// ---- 认库 ----

/// `--vault` 认不出库的理由。
#[derive(Debug, PartialEq, Eq)]
enum TargetError {
    /// 没有这个名字或 id 的库。
    Unknown,
    /// 好几个库同名：列出它们的 id，让用户改用 id。
    Ambiguous(Vec<String>),
    /// 没给 `--vault`，也没有环境变量与应用当前库。
    Undetermined,
}

/// 选库（纯判定，可单测）：`--vault` 只认注册表里的库——先按 id、再按名字；没给时用
/// `fallback`（`LUNA_VAULT_ROOT` → 应用当前打开的库）。
fn pick_vault(
    flag: Option<&str>,
    registry: &[VaultEntry],
    fallback: Option<PathBuf>,
) -> Result<PathBuf, TargetError> {
    let Some(wanted) = flag.map(str::trim).filter(|w| !w.is_empty()) else {
        return fallback.ok_or(TargetError::Undetermined);
    };
    if let Some(entry) = registry
        .iter()
        .find(|entry| entry.id.as_deref() == Some(wanted))
    {
        return Ok(entry.path.clone());
    }
    let named: Vec<&VaultEntry> = registry.iter().filter(|e| e.name == wanted).collect();
    match named.as_slice() {
        [] => Err(TargetError::Unknown),
        [only] => Ok(only.path.clone()),
        many => Err(TargetError::Ambiguous(
            many.iter().filter_map(|e| e.id.clone()).collect(),
        )),
    }
}

fn registry() -> Vec<VaultEntry> {
    luna_vault::registry::VaultRegistry::new(crate::app_config_dir()).list()
}

fn target_root(opts: &VaultOpts) -> Result<PathBuf, Failure> {
    pick_vault(
        opts.vault.as_deref(),
        &registry(),
        crate::resolve_vault_root(None),
    )
    .map_err(|error| match error {
        TargetError::Unknown => Failure::new(
            EXIT_USAGE,
            "没有这个库——`--vault` 只认应用里登记过的库名或库 id",
        ),
        TargetError::Ambiguous(ids) => Failure::new(
            EXIT_USAGE,
            format!("有好几个库叫这个名字，请改用库 id：{}", ids.join("、")),
        ),
        TargetError::Undetermined => Failure::new(
            EXIT_USAGE,
            "无法确定库——请传 --vault，或设置 LUNA_VAULT_ROOT，或先在应用里打开一个库",
        ),
    })
}

/// 应用此刻在不在跑（配置目录都定不下来时按「不在」）。
fn app_running() -> bool {
    crate::app_config_dir().is_some_and(|dir| luna_config::app_lock::is_held(&dir))
}

fn open_read_only(root: &Path) -> Result<Vault, Failure> {
    Vault::open_read_only(root).map_err(|error| match error {
        VaultError::NotFound(path) if path == root => {
            Failure::new(EXIT_USAGE, format!("库目录不存在：{}", root.display()))
        }
        VaultError::NotFound(_) => Failure::new(
            EXIT_INDEX,
            "这个库还没有索引——打开应用，或运行 `luna vault index`",
        ),
        VaultError::Decode(_) => Failure::new(
            EXIT_INDEX,
            "索引读不出或与这个 luna 的版本不一致——打开应用，或运行 `luna vault index` 重建",
        ),
        other => Failure::new(1, format!("打开库失败：{other}")),
    })
}

/// 应用没开时比对索引与磁盘（应用开着时有监听在维护，不比，为 `None`）。落后就在 stderr
/// 如实提示，并把差多少交给 JSON 输出。
fn drift_notice(vault: &Vault) -> Option<Drift> {
    if app_running() {
        return None;
    }
    let drift = vault.index_drift().ok()?;
    if !drift.is_current() {
        eprintln!(
            "luna: 索引可能不是最新（{} 篇有改动、{} 篇已删除）——运行 `luna vault index` 或打开应用",
            drift.changed, drift.removed
        );
    }
    Some(drift)
}

// ---- 输出 ----
//
// `--json` 直接序列化应用里同一份结果模型（`NoteMatch`、`NoteListEntry`、`Drift`），字段名
// 不在这里另写一遍。

#[derive(Serialize)]
struct SearchOutput<'a> {
    vault: &'a Path,
    total_matches: u32,
    truncated: bool,
    /// 应用开着时为 `null`（索引由应用维护，不比对）。
    index_drift: Option<Drift>,
    notes: &'a [NoteMatch],
}

#[derive(Serialize)]
struct IndexOutput<'a> {
    vault: &'a Path,
    indexed: usize,
    full: bool,
    #[serde(flatten)]
    drift: Drift,
}

#[derive(Serialize)]
struct ListedNote<'a> {
    #[serde(flatten)]
    note: &'a NoteListEntry,
    /// 落在排除路径里（V4a：只是降噪，照样列出）。
    excluded: bool,
}

#[derive(Serialize)]
struct ListOutput<'a> {
    vault: &'a Path,
    index_drift: Option<Drift>,
    notes: Vec<ListedNote<'a>>,
}

#[derive(Serialize)]
struct ShowOutput<'a> {
    path: &'a str,
    heading: Option<&'a str>,
    content: &'a str,
}

#[derive(Serialize)]
struct NewOutput<'a> {
    link: &'a str,
    /// 已交给应用（`--print` 时为 `false`）。
    opened: bool,
}

fn print_json(value: &impl Serialize) -> Result<(), Failure> {
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer_pretty(&mut stdout, value)
        .map_err(|e| Failure::new(1, format!("写出失败：{e}")))?;
    writeln!(stdout).map_err(|e| Failure::new(1, format!("写出失败：{e}")))
}

fn print_text(text: &str) -> Result<(), Failure> {
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(text.as_bytes())
        .and_then(|()| stdout.flush())
        .map_err(|e| Failure::new(1, format!("写出失败：{e}")))
}

/// 命中段在终端里加粗（管道 / `NO_COLOR` 时原样）。
fn emphasis() -> Option<(&'static str, &'static str)> {
    (std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none())
        .then_some(("\x1b[1m", "\x1b[0m"))
}

// ---- 命令 ----

fn search(query: &str, match_case: bool, opts: &VaultOpts) -> Outcome {
    let root = target_root(opts)?;
    let parsed = SearchQuery::parse(query, match_case);
    if parsed.is_empty() {
        return Err(Failure::new(EXIT_USAGE, "查询是空的"));
    }
    let vault = open_read_only(&root)?;
    let drift = drift_notice(&vault);
    let results = vault
        .search_matches(&parsed)
        .map_err(|e| Failure::new(1, format!("搜索失败：{e}")))?;
    if results.truncated {
        eprintln!("luna: 结果太多，只列出了前一部分——把查询收窄一些");
    }
    if opts.json {
        print_json(&SearchOutput {
            vault: vault.root(),
            total_matches: results.total_matches,
            truncated: results.truncated,
            index_drift: drift,
            notes: &results.notes,
        })?;
    } else {
        let marks = emphasis();
        let mut out = String::new();
        for note in &results.notes {
            out.push_str(&note.path);
            out.push('\n');
            for line in &note.lines {
                out.push_str(&format!("{:>6}  ", line.line));
                for segment in &line.segments {
                    match (segment.hit, marks) {
                        (true, Some((on, off))) => {
                            out.push_str(on);
                            out.push_str(&segment.text);
                            out.push_str(off);
                        }
                        _ => out.push_str(&segment.text),
                    }
                }
                out.push('\n');
            }
        }
        print_text(&out)?;
    }
    Ok(if results.notes.is_empty() {
        ExitCode::from(EXIT_EMPTY)
    } else {
        ExitCode::SUCCESS
    })
}

fn index(full: bool, opts: &VaultOpts) -> Outcome {
    let root = target_root(opts)?;
    let refuse = || {
        Failure::new(
            EXIT_APP_RUNNING,
            "Lunarca 正在运行，索引由应用维护——不需要也不能同时从命令行更新（两个进程同时写同一份索引没有赢家）",
        )
    };
    // 先看一眼给出明白的理由；真正的护栏是拿锁——拿到之后应用再启动也拿不到，二者不会同时写。
    if app_running() {
        return Err(refuse());
    }
    let _lock = match crate::app_config_dir() {
        Some(dir) => Some(luna_config::app_lock::hold(&dir).ok_or_else(refuse)?),
        None => None,
    };
    let vault = Vault::open(&root).map_err(|e| match e {
        VaultError::NotFound(_) => {
            Failure::new(EXIT_USAGE, format!("库目录不存在：{}", root.display()))
        }
        other => Failure::new(1, format!("打开库失败：{other}")),
    })?;
    let drift = vault
        .index_drift()
        .map_err(|e| Failure::new(1, format!("比对索引失败：{e}")))?;
    let indexed = if full {
        vault.rebuild_index()
    } else {
        vault.scan()
    }
    .map_err(|e| Failure::new(1, format!("更新索引失败：{e}")))?;
    if opts.json {
        print_json(&IndexOutput {
            vault: vault.root(),
            indexed,
            full,
            drift,
        })?;
    } else {
        eprintln!(
            "luna: 索引已更新——共 {indexed} 篇（{} 篇有改动、{} 篇已删除）",
            drift.changed, drift.removed
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn list(folder: Option<&str>, opts: &VaultOpts) -> Outcome {
    let root = target_root(opts)?;
    let prefix = match folder.map(str::trim).filter(|f| !f.is_empty() && *f != "/") {
        Some(raw) => Some(
            luna_vault::pathutil::external_rel_path(raw)
                .map(|dir| format!("{dir}/"))
                .ok_or_else(|| {
                    Failure::new(EXIT_USAGE, "文件夹路径不合法（须是库内的相对路径）")
                })?,
        ),
        None => None,
    };
    let vault = open_read_only(&root)?;
    let drift = drift_notice(&vault);
    let excluded = vault.exclusions();
    let notes: Vec<_> = vault
        .indexed_notes()
        .map_err(|e| Failure::new(1, format!("读取索引失败：{e}")))?
        .into_iter()
        .filter(|note| {
            prefix
                .as_deref()
                .is_none_or(|prefix| note.path.starts_with(prefix))
        })
        .collect();
    if opts.json {
        print_json(&ListOutput {
            vault: vault.root(),
            index_drift: drift,
            notes: notes
                .iter()
                .map(|note| ListedNote {
                    note,
                    excluded: excluded.covers(&note.path),
                })
                .collect(),
        })?;
    } else {
        let mut out = String::new();
        for note in &notes {
            out.push_str(&note.path);
            out.push('\n');
        }
        print_text(&out)?;
    }
    Ok(ExitCode::SUCCESS)
}

/// `路径[#标题]` → （库相对路径, 标题）。整串就是一个文件时不拆（文件名里可以有 `#`）；
/// 没写扩展名而 `<路径>.md` 存在时补上。路径走与 `lunarca://` 同一道外部输入校验。
fn note_target(root: &Path, raw: &str) -> Option<(String, Option<String>)> {
    let exists = |rel: &str| root.join(rel).is_file();
    let with_md = |rel: String| {
        if !exists(&rel)
            && !rel.to_ascii_lowercase().ends_with(".md")
            && exists(&format!("{rel}.md"))
        {
            format!("{rel}.md")
        } else {
            rel
        }
    };
    let whole = luna_vault::pathutil::external_rel_path(raw)?;
    if exists(&with_md(whole.clone())) {
        return Some((with_md(whole), None));
    }
    let Some((path, heading)) = raw.split_once('#') else {
        return Some((with_md(whole), None)); // 不存在的那一篇由读取时如实报「没有」
    };
    let path = luna_vault::pathutil::external_rel_path(path)?;
    let heading = heading.trim();
    Some((
        with_md(path),
        (!heading.is_empty()).then(|| heading.to_owned()),
    ))
}

fn show(raw_target: &str, raw: bool, opts: &VaultOpts) -> Outcome {
    let root = target_root(opts)?;
    let vault = open_read_only(&root)?;
    let Some((path, heading)) = note_target(vault.root(), raw_target.trim()) else {
        return Err(Failure::new(
            EXIT_USAGE,
            "路径不合法（须是库内的相对路径，不能指向隐藏文件或 Lunarca 自己的数据）",
        ));
    };
    if raw && heading.is_some() {
        return Err(Failure::new(
            EXIT_USAGE,
            "--raw 输出整个文件，不能与 #标题 同用",
        ));
    }
    let doc = vault.read(&path).map_err(|e| match e {
        VaultError::NotFound(_) | VaultError::CloudPlaceholder(_) => {
            Failure::new(EXIT_NOTE, format!("库里没有这一篇：{path}"))
        }
        other => Failure::new(EXIT_NOTE, format!("读不出这一篇（{path}）：{other}")),
    })?;
    let content = if raw {
        doc.content.clone()
    } else {
        luna_vault::embed::note_section(&doc.content, heading.as_deref())
            .ok_or_else(|| Failure::new(EXIT_NOTE, "这一篇里没有这个标题"))?
    };
    if opts.json {
        print_json(&ShowOutput {
            path: &path,
            heading: heading.as_deref(),
            content: &content,
        })?;
    } else {
        let mut text = content;
        if !text.ends_with('\n') {
            text.push('\n');
        }
        print_text(&text)?;
    }
    Ok(ExitCode::SUCCESS)
}

fn new(name: &str, print: bool, opts: &VaultOpts) -> Outcome {
    let root = target_root(opts)?;
    let canonical = std::fs::canonicalize(&root).unwrap_or(root);
    // 链接只认库 id：没在应用里打开过的库没有登记、也就没有 id。
    let Some(id) = registry()
        .into_iter()
        .find(|entry| std::fs::canonicalize(&entry.path).is_ok_and(|p| p == canonical))
        .and_then(|entry| entry.id)
    else {
        return Err(Failure::new(
            EXIT_USAGE,
            "这个库还没在应用里打开过——先在 Lunarca 里打开它",
        ));
    };
    let link = luna_vault::uri::new_link(&id, name.trim());
    // 名字的规矩（非空、不带路径分隔符）与应用收链接时同一道判定。
    if luna_vault::uri::parse(&link).is_err() {
        return Err(Failure::new(
            EXIT_USAGE,
            "名字不合法（不能为空，不能带 / 或 \\）",
        ));
    }
    if opts.json {
        print_json(&NewOutput {
            link: &link,
            opened: !print,
        })?;
    } else if print {
        print_text(&format!("{link}\n"))?;
    }
    if !print {
        open_link(&link).map_err(|e| Failure::new(1, format!("交给应用失败：{e}")))?;
        if !opts.json {
            eprintln!("luna: 已交给 Lunarca——在应用里确认后才会新建");
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// 用系统的 URL 打开方式把链接交给应用（应用没开时由系统拉起）。
fn open_link(link: &str) -> std::io::Result<()> {
    let status = if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(link).status()?
    } else if cfg!(target_os = "windows") {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", link])
            .status()?
    } else {
        std::process::Command::new("xdg-open").arg(link).status()?
    };
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!("exit status {status}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, id: Option<&str>, path: &str) -> VaultEntry {
        VaultEntry {
            path: PathBuf::from(path),
            name: name.into(),
            last_opened_ms: 0,
            id: id.map(Into::into),
            accounts: Vec::new(),
        }
    }

    #[test]
    fn vault_flag_matches_id_then_name_only_from_the_registry() {
        let registry = [
            entry("工作", Some("id-a"), "/a"),
            entry("生活", Some("id-b"), "/b"),
            entry("生活", Some("id-c"), "/c"),
        ];
        assert_eq!(pick_vault(Some("id-b"), &registry, None), Ok("/b".into()));
        assert_eq!(pick_vault(Some("工作"), &registry, None), Ok("/a".into()));
        assert_eq!(
            pick_vault(Some("生活"), &registry, None),
            Err(TargetError::Ambiguous(vec!["id-b".into(), "id-c".into()]))
        );
        // 不接受任意路径：注册表里没有的一律不认。
        assert_eq!(
            pick_vault(Some("/a"), &registry, Some("/fallback".into())),
            Err(TargetError::Unknown)
        );
        assert_eq!(
            pick_vault(None, &registry, Some("/fallback".into())),
            Ok("/fallback".into())
        );
        assert_eq!(
            pick_vault(None, &registry, None),
            Err(TargetError::Undetermined)
        );
    }

    #[test]
    fn note_targets_split_headings_fill_extensions_and_refuse_hidden_data() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("项目")).unwrap();
        std::fs::write(dir.path().join("项目/计划.md"), "# 计划\n").unwrap();
        std::fs::write(dir.path().join("C#.md"), "x").unwrap();
        let root = dir.path();
        assert_eq!(
            note_target(root, "项目/计划#议程"),
            Some(("项目/计划.md".into(), Some("议程".into())))
        );
        assert_eq!(
            note_target(root, "项目/计划.md"),
            Some(("项目/计划.md".into(), None))
        );
        // 文件名本身带 `#`：整串是文件就不拆。
        assert_eq!(note_target(root, "C#.md"), Some(("C#.md".into(), None)));
        assert_eq!(note_target(root, "C#"), Some(("C#.md".into(), None)));
        assert_eq!(note_target(root, "没有"), Some(("没有".into(), None)));
        for bad in [
            "../x.md",
            "/etc/passwd",
            ".luna/index.sqlite",
            "a/.trash/x.md",
        ] {
            assert_eq!(note_target(root, bad), None, "{bad}");
        }
    }

    #[test]
    fn read_only_commands_never_write_into_the_vault() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.md"), "# 甲\n\n季度复盘\n").unwrap();
        {
            let vault = Vault::open(dir.path()).unwrap();
            vault.scan().unwrap();
        }
        let snapshot = |root: &Path| -> Vec<(PathBuf, u64)> {
            let mut out = Vec::new();
            let mut stack = vec![root.to_path_buf()];
            while let Some(dir) = stack.pop() {
                for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                    let meta = entry.metadata().unwrap();
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if meta.is_dir() {
                        stack.push(entry.path());
                    } else if name.ends_with("-shm") || name.ends_with("-wal") {
                        // SQLite 读 WAL 库时自己要用的协调文件，不是库里的内容。
                    } else {
                        out.push((entry.path(), meta.len()));
                    }
                }
            }
            out.sort();
            out
        };
        let before = snapshot(dir.path());
        let vault = Vault::open_read_only(dir.path()).unwrap();
        let hits = vault
            .search_matches(&SearchQuery::parse("季度", false))
            .unwrap();
        assert_eq!(hits.notes.len(), 1);
        assert_eq!(vault.indexed_notes().unwrap().len(), 1);
        assert!(vault.index_drift().unwrap().is_current());
        drop(vault);
        assert_eq!(snapshot(dir.path()), before);
    }

    #[test]
    fn a_vault_never_indexed_is_reported_not_created() {
        let dir = tempfile::tempdir().unwrap();
        match open_read_only(dir.path()) {
            Err(failure) => assert_eq!(failure.code, EXIT_INDEX),
            Ok(_) => panic!("没有索引的库不该打开成功"),
        }
        assert!(!dir.path().join(".luna").exists());
    }
}
