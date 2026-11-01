//! 官方同步专用的文件操作（ADR-148 ⑦）：逐字节复现远端的结果。
//!
//! 与编辑器那一套的差别都是刻意的：
//! - **按内容哈希核对再写**（`expected`），不符就不动——用户在这一轮里刚改过的文件留给下一轮；
//! - **任意字节**：附件、录音、`.luna/settings.toml` 都走这里，不只是 Markdown；
//! - **改名不改写链接**、**删除不带走附件**：远端那台设备已经做过这些，再做一遍就是二次改写；
//! - 写入登记自写抑制（监听器不回声）、更新派生索引、覆盖前留快照；删除进 Lunarca 回收站。
//!
//! 界面刷新（`vault://changed`）由调用方在一轮结束后统一发：自写抑制会吞掉监听器的回声，
//! 不发的话编辑器永远不知道文件变了。

use std::path::Path;

use crate::absence::{classify_absence, Absence};
use crate::error::{Result, VaultError};
use crate::hashing::ContentHash;
use crate::vault::{file_mtime_ms, normalize_rel, DeleteMode, Vault};

/// 参与同步的一个文件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncFile {
    /// 库相对路径（`/` 分隔）。
    pub rel: String,
    pub size: u64,
    pub modified_ms: Option<i64>,
}

/// 同步范围里 `.luna/` 下要带上的东西（TDD-12 §10.2 数据范围）：库级设置与随库边车。
const LUNA_SYNCED_FILE: &str = ".luna/settings.toml";
const LUNA_SYNCED_DIR: &str = ".luna/sidecars";

fn is_markdown(rel: &str) -> bool {
    rel.rsplit('.')
        .next()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
}

/// 同步只碰同步范围内的路径：点开头的路径段一律不碰（`.trash/`、`.obsidian/`、临时文件），
/// `.luna/` 下只许库级设置与边车。
fn in_scope(rel: &str) -> bool {
    // `settings.toml` 要单独判：此前与边车目录共用「截掉目录前缀再查点开头」，按边车目录的
    // 长度截它得到 `.toml`，于是扫描列进来的文件读时被判越界，整轮中止（2026-10-11 真机）。
    if rel == LUNA_SYNCED_FILE {
        return true;
    }
    if let Some(rest) = rel.strip_prefix(&format!("{LUNA_SYNCED_DIR}/")) {
        return !rest
            .split('/')
            .any(|part| part.is_empty() || part.starts_with('.'));
    }
    !rel.split('/')
        .any(|part| part.is_empty() || part.starts_with('.'))
}

fn modified_ms(path: &Path) -> Option<i64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let ms = modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis();
    i64::try_from(ms).ok()
}

/// 一个库同步范围的文件清单（带大小），**不打开库**（ADR-152：没选中同步的库也要显示
/// 「开同步要占多少」，而打开库会往里写 `.luna/`）。不跟随指向目录的链接，读不了的子目录跳过。
///
/// # Errors
/// 库根读不了。
pub fn sync_scope_files(root: &Path) -> Result<Vec<SyncFile>> {
    let mut rels = Vec::new();
    crate::embed::walk_all(root, root, &mut rels)?;
    if root.join(LUNA_SYNCED_FILE).is_file() {
        rels.push(LUNA_SYNCED_FILE.to_owned());
    }
    let sidecars = root.join(LUNA_SYNCED_DIR);
    if sidecars.is_dir() {
        crate::embed::walk_all(root, &sidecars, &mut rels)?;
    }
    let mut out = Vec::with_capacity(rels.len());
    for rel in rels {
        let abs = root.join(&rel);
        let Ok(meta) = std::fs::metadata(&abs) else {
            continue;
        };
        if meta.is_file() {
            out.push(SyncFile {
                modified_ms: modified_ms(&abs),
                size: meta.len(),
                rel,
            });
        }
    }
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(out)
}

/// 一个文件此刻的内容哈希（流式；不存在 = `None`）。
fn hash_file(abs: &Path) -> Result<Option<ContentHash>> {
    match std::fs::File::open(abs) {
        Ok(file) => Ok(Some(ContentHash::of_reader(file)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(VaultError::Io(e)),
    }
}

/// 读一个已解析的同步文件（[`Vault::sync_path`] 的产物）；不存在归 `NotFound`。
///
/// # Errors
/// 文件不存在或读失败。
pub fn read_sync_file(abs: &Path, rel: &str) -> Result<Vec<u8>> {
    std::fs::read(abs).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => VaultError::NotFound(rel.into()),
        _ => VaultError::Io(e),
    })
}

/// 一个库同步范围的总字节（不打开库）。
///
/// # Errors
/// 库根读不了。
pub fn sync_scope_bytes(root: &Path) -> Result<u64> {
    Ok(sync_scope_files(root)?.iter().map(|f| f.size).sum())
}

impl Vault {
    /// 同步范围内一个相对路径 → 绝对路径（越出范围 / 逃出库根即拒）。
    ///
    /// 单独暴露给同步适配层：**解析要在库锁里做，读字节不必**——整份音频读在锁里，会让
    /// 同一时刻的改名、删除、取目录树全部排队（2026-10-13 真机：开同步后改名删除很慢）。
    /// 读出的字节由引擎按清单里的哈希核对，读到半途被换掉的内容不会被当真。
    pub fn sync_path(&self, rel: &str) -> Result<std::path::PathBuf> {
        let rel = normalize_rel(rel);
        if !in_scope(&rel) {
            return Err(VaultError::PathEscape(rel.into()));
        }
        self.resolve(&rel)
    }

    /// 同步范围里的全部文件（带大小）。不跟随指向目录的链接，读不了的子目录跳过。
    pub fn sync_list(&self) -> Result<Vec<SyncFile>> {
        sync_scope_files(self.root())
    }

    pub fn sync_read(&self, rel: &str) -> Result<Vec<u8>> {
        read_sync_file(&self.sync_path(rel)?, rel)
    }

    /// 一个基线里有、此刻没扫到的文件为什么不在（库身份按本库的 `vault-id` 核对）。
    #[must_use]
    pub fn sync_absence(&self, rel: &str) -> Absence {
        classify_absence(
            self.root(),
            &normalize_rel(rel),
            Some(&self.id().to_string()),
        )
    }

    /// 仅当现状符合期望时写入：`expected = None` 要求那里没有文件，`Some(hash)` 要求内容哈希
    /// 等于它。不符返回 `Ok(false)`、一个字节不写。覆盖前对旧内容留快照。
    pub fn sync_write_if(&self, rel: &str, bytes: &[u8], expected: Option<&str>) -> Result<bool> {
        let rel = normalize_rel(rel);
        let abs = self.sync_path(&rel)?;
        let current = match std::fs::read(&abs) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(VaultError::Io(e)),
        };
        let matches = match (expected, &current) {
            (None, None) => true,
            (Some(hash), Some(old)) => ContentHash::of(old).as_str() == hash,
            _ => false,
        };
        if !matches {
            return Ok(false);
        }
        if let Some(old) = &current {
            let _ = self.snapshots.snapshot(&rel, old);
        }
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent)?;
        }
        crate::atomic::write_atomic(&abs, bytes)?;
        if let Ok(mut guard) = self.guard.lock() {
            guard.record_write(&abs, ContentHash::of(bytes));
        }
        if is_markdown(&rel) {
            if let Ok(text) = std::str::from_utf8(bytes) {
                let _ = self.index.index_note(&rel, text, file_mtime_ms(&abs));
            }
        }
        Ok(true)
    }

    /// 仅当现状符合期望时把一份已写好的临时文件（[`Self::sync_stage_path`] 里造的）换到 `rel`：
    /// 原子改名，临时文件随之用掉；不符返回 `Ok(false)`、临时文件原样留着。
    ///
    /// 大文件专用（录音几百 MB）：哈希流式算，**覆盖前不留本地快照**——这份内容在服务端有历史
    /// 版本（ADR-156），再在本机快照库里存一份几百 MB 的副本只是占盘。
    pub fn sync_write_file_if(
        &self,
        rel: &str,
        staged: &Path,
        expected: Option<&str>,
    ) -> Result<bool> {
        let rel = normalize_rel(rel);
        let abs = self.sync_path(&rel)?;
        let current = hash_file(&abs)?;
        let matches = match (expected, &current) {
            (None, None) => true,
            (Some(hash), Some(old)) => old.as_str() == hash,
            _ => false,
        };
        if !matches {
            return Ok(false);
        }
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let new_hash = hash_file(staged)?.ok_or_else(|| VaultError::NotFound(staged.into()))?;
        std::fs::rename(staged, &abs)?;
        if let Ok(mut guard) = self.guard.lock() {
            guard.record_write(&abs, new_hash);
        }
        // 极少见的超长笔记也走这里：照常进索引。
        if is_markdown(&rel) {
            if let Ok(text) = std::fs::read_to_string(&abs) {
                let _ = self.index.index_note(&rel, &text, file_mtime_ms(&abs));
            }
        }
        Ok(true)
    }

    /// 清掉上一轮遗留的暂存（应用在拼大文件途中退出）。同一个库同一时刻只有一轮在跑，
    /// 所以一轮开始前这里的 `sync-*` 都是没人要的。
    pub fn sync_clear_stage(&self) {
        let Ok(entries) = std::fs::read_dir(self.root().join(".luna").join("tmp")) else {
            return;
        };
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with("sync-") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    /// 造暂存文件的目录：库内 `.luna/tmp/`（与目标同一个卷，换上去是一次原子改名；
    /// 不在同步范围内）。
    pub fn sync_stage_dir(&self) -> Result<std::path::PathBuf> {
        let dir = self.root().join(".luna").join("tmp");
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    /// 释放本机空间：内容仍是 `expected` 时移除本机副本，**不进回收站**——这一版在云端
    /// （调用方按同步基线确认过），进回收站等于没释放（ADR-156）。
    pub fn sync_evict_if(&self, rel: &str, expected: &str) -> Result<bool> {
        let rel = normalize_rel(rel);
        let abs = self.sync_path(&rel)?;
        match hash_file(&abs)? {
            Some(old) if old.as_str() == expected => {}
            _ => return Ok(false),
        }
        std::fs::remove_file(&abs)?;
        if let Ok(mut guard) = self.guard.lock() {
            guard.forget(&abs);
        }
        Ok(true)
    }

    /// 仅当内容哈希仍是 `expected` 时删除：进 Lunarca 回收站（可还原），不带走附件。
    pub fn sync_delete_if(&self, rel: &str, expected: &str) -> Result<bool> {
        let rel = normalize_rel(rel);
        let abs = self.sync_path(&rel)?;
        match hash_file(&abs)? {
            Some(old) if old.as_str() == expected => {}
            _ => return Ok(false),
        }
        self.remove_entry(&rel, &abs, false, DeleteMode::VaultTrash, None)?;
        if let Ok(mut guard) = self.guard.lock() {
            guard.forget(&abs);
        }
        if is_markdown(&rel) {
            let _ = self.index.forget_note(&rel);
        }
        Ok(true)
    }

    /// 仅当源内容哈希仍是 `expected`、目标处没有文件时改名。不改写任何链接；版本链跟着走。
    pub fn sync_rename_if(&self, from: &str, to: &str, expected: &str) -> Result<bool> {
        let (from, to) = (normalize_rel(from), normalize_rel(to));
        let (from_abs, to_abs) = (self.sync_path(&from)?, self.sync_path(&to)?);
        let hash = match hash_file(&from_abs)? {
            Some(hash) if hash.as_str() == expected => hash,
            _ => return Ok(false),
        };
        // 只改大小写时目标「存在」的是它自己（大小写不敏感的卷）：照常改。
        let same_file = from.eq_ignore_ascii_case(&to);
        if to_abs.exists() && !same_file {
            return Ok(false);
        }
        if let Some(parent) = to_abs.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(&from_abs, &to_abs)?;
        if let Ok(mut guard) = self.guard.lock() {
            guard.forget(&from_abs);
            guard.record_write(&to_abs, hash);
        }
        let _ = self.snapshots.rename_chain(&from, &to);
        if is_markdown(&from) {
            let _ = self.index.forget_note(&from);
        }
        if is_markdown(&to) {
            if let Ok(text) = std::fs::read_to_string(&to_abs) {
                let _ = self.index.index_note(&to, &text, file_mtime_ms(&to_abs));
            }
        }
        Ok(true)
    }

    /// 给一份即将被覆盖的内容留一个历史版本（合并写盘前用，ADR-148 ②）。
    pub fn sync_keep_version(&self, rel: &str, bytes: &[u8]) -> Result<()> {
        self.snapshots
            .snapshot(&normalize_rel(rel), bytes)
            .map(|_| ())
    }

    /// 钉住一份基线内容（三路合并的共同祖先）。`owner` 是同步绑定的名字。
    pub fn sync_pin(&self, owner: &str, bytes: &[u8]) -> Result<String> {
        self.snapshots.pin(owner, bytes)
    }

    pub fn sync_read_pinned(&self, owner: &str, hash: &str) -> Result<Vec<u8>> {
        self.snapshots.read_pinned(owner, hash)
    }

    pub fn sync_unpin(&self, owner: &str, hash: &str) -> Result<()> {
        self.snapshots.unpin(owner, hash)
    }

    /// 停用同步：解掉这个绑定的全部钉住，并回收没人要的 blob。
    pub fn sync_unpin_all(&self, owner: &str) -> Result<()> {
        self.snapshots.unpin_all(owner)?;
        self.snapshots.collect_garbage()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault() -> (tempfile::TempDir, Vault) {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        (dir, vault)
    }

    fn hash(bytes: &[u8]) -> String {
        ContentHash::of(bytes).as_str().to_owned()
    }

    #[test]
    fn writes_check_the_expected_content_and_take_any_bytes() {
        let (_dir, vault) = vault();
        assert!(vault.sync_write_if("img/a.png", &[1, 2, 3], None).unwrap());
        assert!(
            !vault.sync_write_if("img/a.png", &[9], None).unwrap(),
            "那里已经有文件了"
        );
        assert!(
            !vault
                .sync_write_if("img/a.png", &[9], Some(&hash(&[0])))
                .unwrap(),
            "内容不是期望的那份"
        );
        assert!(vault
            .sync_write_if("img/a.png", &[4, 5], Some(&hash(&[1, 2, 3])))
            .unwrap());
        assert_eq!(vault.sync_read("img/a.png").unwrap(), vec![4, 5]);

        // Markdown 写入进索引，且登记了自写（监听器不回声）。
        assert!(vault
            .sync_write_if("n/a.md", "# 标题\n".as_bytes(), None)
            .unwrap());
        assert_eq!(
            vault
                .last_written_hash("n/a.md")
                .map(|h| h.as_str().to_owned()),
            Some(hash("# 标题\n".as_bytes()))
        );
    }

    #[test]
    fn the_scope_keeps_hidden_paths_out_but_carries_vault_settings() {
        let (dir, vault) = vault();
        vault.sync_write_if("a.md", b"x", None).unwrap();
        std::fs::write(dir.path().join(".luna/settings.toml"), "k = 1\n").unwrap();
        std::fs::create_dir_all(dir.path().join(".obsidian")).unwrap();
        std::fs::write(dir.path().join(".obsidian/app.json"), "{}").unwrap();
        let rels: Vec<String> = vault
            .sync_list()
            .unwrap()
            .into_iter()
            .map(|f| f.rel)
            .collect();
        assert!(rels.contains(&"a.md".to_owned()));
        assert!(rels.contains(&".luna/settings.toml".to_owned()));
        // 列得出来的就读得到：扫描与读取同一个范围（2026-10-11 真机：settings.toml 读时被判越界）。
        for rel in &rels {
            assert!(vault.sync_read(rel).is_ok(), "{rel} 列出来却读不了");
        }
        assert!(vault
            .sync_write_if(".luna/settings.toml", b"k = 2\n", Some(&hash(b"k = 1\n")))
            .unwrap());
        assert!(matches!(
            vault.sync_read(".luna/sidecars/.hidden"),
            Err(VaultError::PathEscape(_))
        ));
        assert!(!rels
            .iter()
            .any(|r| r.starts_with(".obsidian") || r.starts_with(".trash")));
        assert!(vault
            .sync_write_if(".obsidian/x.json", b"{}", None)
            .is_err());
        assert!(vault.sync_write_if(".luna/vault-id", b"x", None).is_err());
        assert!(vault.sync_write_if("../escape.md", b"x", None).is_err());
    }

    #[test]
    fn delete_goes_to_the_vault_trash_without_taking_attachments() {
        let (dir, vault) = vault();
        vault
            .sync_write_if("a.md", b"![[pic.png]]\n", None)
            .unwrap();
        vault.sync_write_if("pic.png", &[7], None).unwrap();
        assert!(!vault.sync_delete_if("a.md", &hash(b"other")).unwrap());
        assert!(vault
            .sync_delete_if("a.md", &hash(b"![[pic.png]]\n"))
            .unwrap());
        assert!(!dir.path().join("a.md").exists());
        assert!(dir.path().join("pic.png").exists(), "附件不跟着走");
        assert_eq!(vault.list_trash().len(), 1);
    }

    #[test]
    fn rename_moves_bytes_only_and_never_rewrites_links() {
        let (dir, vault) = vault();
        vault.sync_write_if("a.md", b"body", None).unwrap();
        vault.sync_write_if("b.md", b"see [[a]]\n", None).unwrap();
        assert!(
            !vault
                .sync_rename_if("a.md", "b.md", &hash(b"body"))
                .unwrap(),
            "目标被占着"
        );
        assert!(vault
            .sync_rename_if("a.md", "dir/c.md", &hash(b"body"))
            .unwrap());
        assert_eq!(std::fs::read(dir.path().join("dir/c.md")).unwrap(), b"body");
        assert_eq!(
            std::fs::read(dir.path().join("b.md")).unwrap(),
            b"see [[a]]\n",
            "链接原样"
        );
        // 只改大小写。
        assert!(vault
            .sync_rename_if("dir/c.md", "dir/C.md", &hash(b"body"))
            .unwrap());
    }

    #[test]
    fn pins_and_versions_go_through_the_snapshot_store() {
        let (_dir, vault) = vault();
        let pin = vault.sync_pin("sync-x", b"base").unwrap();
        assert_eq!(vault.sync_read_pinned("sync-x", &pin).unwrap(), b"base");
        vault.sync_keep_version("a.md", b"draft").unwrap();
        vault.sync_unpin_all("sync-x").unwrap();
        assert!(vault.sync_read_pinned("sync-x", &pin).is_err());
        assert_eq!(vault.sync_absence("gone.md"), Absence::Deleted);
    }
}
