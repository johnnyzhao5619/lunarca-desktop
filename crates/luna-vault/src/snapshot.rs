//! 版本快照（TDD-06 §4.7）。
//!
//! 触发点：保存覆盖 / 外部冲突合并 / AI 替换正文**之前**对旧版本留快照。实现 = 内容寻址
//! （按 hash 去重 blob）+ 每文件版本链。**快照是"非缓存"**：历史版本一旦删除不可恢复，
//! 故物理上虽放 `.luna/snapshots/`，但必须在"清缓存/重建索引"路径中被显式排除（清缓存只
//! 动 `index.sqlite` 与 `cache/`）。
//!
//! **降级（§4.7）**：快照失败（磁盘满/权限）**绝不阻断保存**——调用方把 `Err` 当非致命，
//! 仅提示"本次未留历史版本"，照常写盘。
//!
//! 内容寻址去重的指纹口径与自写抑制共用 [`ContentHash`]（DRY）。

use crate::atomic::write_atomic_batched as write_atomic;
use crate::error::{Result, VaultError};
use luna_sync_core::hashing::ContentHash;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// 一条快照的元数据（版本链条目）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotMeta {
    /// 稳定 id（UUIDv7，时间有序）。
    pub id: String,
    /// 留快照时刻（Unix 毫秒）。
    pub created_ms: u64,
    /// 内容指纹（= blob 键）。
    pub hash: String,
    /// 内容字节数。
    pub size: u64,
    /// 新格式主题的系统底版身份；旧快照缺省为普通历史版本，不能凭正文相同冒充底版。
    #[serde(default, skip_serializing_if = "is_false")]
    pub system_baseline: bool,
}

/// 默认每文件保留版本上限（避免无限膨胀，§4.7）。
pub const DEFAULT_MAX_VERSIONS: usize = 50;

/// 内容寻址快照库，根目录通常为 `<Vault>/.luna/snapshots/`。
#[derive(Debug, Clone)]
pub struct SnapshotStore {
    root: PathBuf,
    max_versions: usize,
}

impl SnapshotStore {
    /// 以根目录建库（用默认保留上限）。
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            max_versions: DEFAULT_MAX_VERSIONS,
        }
    }

    /// 设保留上限。
    #[must_use]
    pub fn with_max_versions(mut self, max: usize) -> Self {
        self.max_versions = max.max(1);
        self
    }

    fn blobs_dir(&self) -> PathBuf {
        self.root.join("blobs")
    }

    fn index_dir(&self) -> PathBuf {
        self.root.join("index")
    }

    /// 每文件版本链的索引文件路径（按相对路径指纹命名，避开路径分隔符问题）。
    fn index_file(&self, rel_path: &str) -> PathBuf {
        let key = ContentHash::of(rel_path.as_bytes());
        self.index_dir().join(format!("{key}.json"))
    }

    fn load_chain(&self, rel_path: &str) -> Vec<SnapshotMeta> {
        let path = self.index_file(rel_path);
        let Ok(bytes) = fs::read(&path) else {
            return Vec::new();
        };
        serde_json::from_slice(&bytes).unwrap_or_default()
    }

    fn save_chain(&self, rel_path: &str, chain: &[SnapshotMeta]) -> Result<()> {
        fs::create_dir_all(self.index_dir())?;
        let bytes =
            serde_json::to_vec_pretty(chain).map_err(|e| VaultError::Decode(e.to_string()))?;
        write_atomic(&self.index_file(rel_path), &bytes)
    }

    /// 对 `rel_path` 的旧内容 `content` 留一份快照。
    ///
    /// **按内容去重**：若该文件版本链末条与本次内容指纹相同（连续相同/无改动），不新增条目，
    /// 直接返回既有条目（实现 §4.7"连续小改不每次留版本，按 hash 去重"的节流）。
    pub fn snapshot(&self, rel_path: &str, content: &[u8]) -> Result<SnapshotMeta> {
        self.snapshot_with_role(rel_path, content, false)
    }

    /// 记录新格式主题的系统底版。最近一份底版在普通历史版本清理时受到保护。
    pub fn snapshot_system_baseline(&self, rel_path: &str, content: &[u8]) -> Result<SnapshotMeta> {
        self.snapshot_with_role(rel_path, content, true)
    }

    fn snapshot_with_role(
        &self,
        rel_path: &str,
        content: &[u8],
        system_baseline: bool,
    ) -> Result<SnapshotMeta> {
        let hash = ContentHash::of(content);
        let mut chain = self.load_chain(rel_path);

        if let Some(last_index) = chain.len().checked_sub(1) {
            if chain[last_index].hash == hash.as_str() {
                if system_baseline {
                    let blob = self.blobs_dir().join(hash.as_str());
                    if !fs::read(&blob).is_ok_and(|bytes| bytes == content) {
                        fs::create_dir_all(self.blobs_dir())?;
                        write_atomic(&blob, content)?;
                    }
                }
                if system_baseline && !chain[last_index].system_baseline {
                    chain[last_index].system_baseline = true;
                    self.save_chain(rel_path, &chain)?;
                }
                return Ok(chain[last_index].clone());
            }
        }

        // 写 blob（去重：已存在同 hash blob 则跳过）。
        let blobs = self.blobs_dir();
        fs::create_dir_all(&blobs)?;
        let blob_path = blobs.join(hash.as_str());
        if !blob_path.exists() {
            write_atomic(&blob_path, content)?;
        }

        let meta = SnapshotMeta {
            id: uuid::Uuid::now_v7().to_string(),
            created_ms: now_ms(),
            hash: hash.as_str().to_owned(),
            size: content.len() as u64,
            system_baseline,
        };
        chain.push(meta.clone());

        // 普通历史仍保留设定的版本数；最近系统底版占额外名额，避免 max=1 时
        // 普通保存的新快照刚写入便被清掉。
        let limit = self
            .max_versions
            .saturating_add(usize::from(chain.iter().any(|entry| entry.system_baseline)));
        let mut trimmed = false;
        while chain.len() > limit {
            let protected = chain.iter().rposition(|entry| entry.system_baseline);
            let drop = (0..chain.len())
                .find(|index| Some(*index) != protected)
                .expect("overfull chain has a removable entry");
            chain.remove(drop);
            trimmed = true;
        }
        self.save_chain(rel_path, &chain)?;
        // 只有版本链真删了旧条目，才可能留下没人引用的 blob。每次都全量回收要读遍所有版本链
        // 与 blob 目录——改名一篇被 500 篇引用的笔记时那是 500 次全库读，实测 1000 篇的库要
        // 16 秒（笔记库对标轮 V1 基准）。
        if trimmed {
            self.gc_blobs()?;
        }
        Ok(meta)
    }

    /// 文件改名 / 移动后，把它的版本链挂到新路径下（TDD-06 §3 改名重构 ⑦）。
    ///
    /// 版本链按路径指纹命名，不迁移的话改名之后「版本历史」是空的——旧版本还在库里，却再也
    /// 找不到。新路径上若已有版本链（此前删掉过同名文件），两条按时间合并；blob 是内容寻址的，
    /// 不必搬。只改大小写的改名同样要迁：路径指纹区分大小写。
    pub fn rename_chain(&self, from: &str, to: &str) -> Result<()> {
        if from == to {
            return Ok(());
        }
        let moved = self.load_chain(from);
        if moved.is_empty() {
            return Ok(());
        }
        let mut chain = self.load_chain(to);
        chain.extend(moved);
        chain.sort_by_key(|entry| entry.created_ms);
        self.save_chain(to, &chain)?;
        match fs::remove_file(self.index_file(from)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    /// 列出某文件的版本链（旧→新）。
    #[must_use]
    pub fn list(&self, rel_path: &str) -> Vec<SnapshotMeta> {
        self.load_chain(rel_path)
    }

    /// 最新系统底版的元数据（不读 blob）；只用于「何时由系统写入」这类展示事实。
    #[must_use]
    pub fn latest_system_baseline_meta(&self, rel_path: &str) -> Option<SnapshotMeta> {
        self.load_chain(rel_path)
            .into_iter()
            .rev()
            .find(|entry| entry.system_baseline)
    }

    /// 只按显式身份读取最新系统底版，校验 blob 内容后才允许作为维护依据。
    pub fn latest_system_baseline(&self, rel_path: &str) -> Result<Option<Vec<u8>>> {
        let chain = self.load_chain(rel_path);
        let Some(meta) = chain.iter().rev().find(|entry| entry.system_baseline) else {
            return Ok(None);
        };
        let bytes = fs::read(self.blobs_dir().join(&meta.hash))?;
        if bytes.len() as u64 != meta.size || ContentHash::of(&bytes).as_str() != meta.hash {
            return Err(VaultError::Decode("system baseline blob mismatch".into()));
        }
        Ok(Some(bytes))
    }

    /// 取某快照的内容字节（调用方负责写回 Vault，留快照前可再快照当前态）。
    pub fn read_snapshot(&self, rel_path: &str, id: &str) -> Result<Vec<u8>> {
        let chain = self.load_chain(rel_path);
        let meta = chain
            .iter()
            .find(|m| m.id == id)
            .ok_or_else(|| VaultError::NotFound(self.index_file(rel_path)))?;
        let blob = self.blobs_dir().join(&meta.hash);
        Ok(fs::read(&blob)?)
    }

    fn pins_dir(&self, owner: &str) -> Result<PathBuf> {
        if owner.is_empty()
            || !owner
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
            || owner.starts_with('.')
        {
            return Err(VaultError::Decode(
                "snapshot pin owner is not a plain name".into(),
            ));
        }
        Ok(self.root.join("pins").join(owner))
    }

    /// 钉住一份内容（同步基线，ADR-148）：按内容寻址存 blob，并以 `pins/<owner>/<hash>` 标记它
    /// 有人要——钉住的 blob 不跟任何文件的版本链走，改名不丢，也不占 50 版的名额，回收时视为
    /// 有引用。返回内容指纹（之后凭它读回 / 解钉）。
    ///
    /// `owner` 是持有者的名字（同步用 `sync-<远端库 id>`），只许字母、数字与 `-_.`。
    pub fn pin(&self, owner: &str, content: &[u8]) -> Result<String> {
        let pins = self.pins_dir(owner)?;
        let hash = ContentHash::of(content);
        let blob = self.blobs_dir().join(hash.as_str());
        if !fs::read(&blob).is_ok_and(|bytes| bytes == content) {
            fs::create_dir_all(self.blobs_dir())?;
            write_atomic(&blob, content)?;
        }
        fs::create_dir_all(&pins)?;
        fs::write(pins.join(hash.as_str()), b"")?;
        Ok(hash.as_str().to_owned())
    }

    /// 读回一份钉住的内容（核对指纹）。没钉住、blob 不在或内容对不上都是错误——基线错了
    /// 比没有基线更危险（三路合并会拿它当共同祖先）。
    pub fn read_pinned(&self, owner: &str, hash: &str) -> Result<Vec<u8>> {
        let marker = self.pins_dir(owner)?.join(hash);
        if !marker.is_file() {
            return Err(VaultError::NotFound(marker));
        }
        let bytes = fs::read(self.blobs_dir().join(hash))?;
        if ContentHash::of(&bytes).as_str() != hash {
            return Err(VaultError::Decode("pinned snapshot blob mismatch".into()));
        }
        Ok(bytes)
    }

    /// 解钉一份内容。blob 留到下一次回收（[`Self::collect_garbage`]）——它可能还被版本链引用。
    pub fn unpin(&self, owner: &str, hash: &str) -> Result<()> {
        match fs::remove_file(self.pins_dir(owner)?.join(hash)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    /// 解掉一个持有者的全部钉住（停用同步）。
    pub fn unpin_all(&self, owner: &str) -> Result<()> {
        match fs::remove_dir_all(self.pins_dir(owner)?) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    /// 回收没有任何版本链引用、也没被钉住的 blob（解钉之后由调用方择机调，它要读遍全部版本链）。
    pub fn collect_garbage(&self) -> Result<()> {
        self.gc_blobs()
    }

    /// GC：删除不被任何文件版本链引用、也没被钉住的 blob。
    fn gc_blobs(&self) -> Result<()> {
        let mut referenced = std::collections::HashSet::new();
        if let Ok(owners) = fs::read_dir(self.root.join("pins")) {
            for owner in owners.flatten() {
                if let Ok(markers) = fs::read_dir(owner.path()) {
                    for marker in markers.flatten() {
                        if let Some(name) = marker.file_name().to_str() {
                            referenced.insert(name.to_owned());
                        }
                    }
                }
            }
        }
        if let Ok(entries) = fs::read_dir(self.index_dir()) {
            for e in entries.flatten() {
                if let Ok(bytes) = fs::read(e.path()) {
                    if let Ok(chain) = serde_json::from_slice::<Vec<SnapshotMeta>>(&bytes) {
                        for m in chain {
                            referenced.insert(m.hash);
                        }
                    }
                }
            }
        }
        if let Ok(entries) = fs::read_dir(self.blobs_dir()) {
            for e in entries.flatten() {
                if let Some(name) = e.file_name().to_str() {
                    if !referenced.contains(name) {
                        let _ = fs::remove_file(e.path());
                    }
                }
            }
        }
        Ok(())
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 给定 Vault 根，返回快照库根（`<root>/.luna/snapshots`）。集中路径口径（§1）。
#[must_use]
pub fn snapshots_root(vault_root: &Path) -> PathBuf {
    vault_root.join(".luna").join("snapshots")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_baselines_survive_trimming_renames_and_garbage_collection() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path()).with_max_versions(1);
        let hash = store.pin("sync-abc", b"base").unwrap();
        // 版本链挤掉同内容的旧条目、触发回收：钉住的 blob 仍在。
        store.snapshot("a.md", b"base").unwrap();
        store.snapshot("a.md", b"v2").unwrap();
        store.snapshot("a.md", b"v3").unwrap();
        store.rename_chain("a.md", "b.md").unwrap();
        store.collect_garbage().unwrap();
        assert_eq!(store.read_pinned("sync-abc", &hash).unwrap(), b"base");
        // 别的持有者读不到它（标记按持有者分）。
        assert!(store.read_pinned("sync-other", &hash).is_err());

        store.unpin("sync-abc", &hash).unwrap();
        assert!(store.read_pinned("sync-abc", &hash).is_err());
        store.collect_garbage().unwrap();
        assert!(
            !dir.path().join("blobs").join(&hash).exists(),
            "解钉后没人要的 blob 被回收"
        );

        let again = store.pin("sync-abc", b"other").unwrap();
        store.unpin_all("sync-abc").unwrap();
        assert!(store.read_pinned("sync-abc", &again).is_err());
        store.unpin_all("sync-abc").unwrap();
    }

    #[test]
    fn pin_owners_cannot_escape_the_pins_directory() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path());
        for owner in ["", "../x", "a/b", ".hidden"] {
            assert!(store.pin(owner, b"x").is_err(), "{owner}");
        }
    }

    #[test]
    fn a_corrupted_pinned_blob_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path());
        let hash = store.pin("sync-abc", b"base").unwrap();
        fs::write(dir.path().join("blobs").join(&hash), b"tampered").unwrap();
        assert!(store.read_pinned("sync-abc", &hash).is_err());
    }

    #[test]
    fn snapshots_and_restores() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path());
        let m1 = store.snapshot("notes/a.md", b"version-1").unwrap();
        let m2 = store.snapshot("notes/a.md", b"version-2").unwrap();
        let chain = store.list("notes/a.md");
        assert_eq!(chain.len(), 2);
        assert_eq!(
            store.read_snapshot("notes/a.md", &m1.id).unwrap(),
            b"version-1"
        );
        assert_eq!(
            store.read_snapshot("notes/a.md", &m2.id).unwrap(),
            b"version-2"
        );
    }

    #[test]
    fn dedup_identical_consecutive_content() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path());
        let a = store.snapshot("a.md", b"same").unwrap();
        let b = store.snapshot("a.md", b"same").unwrap();
        assert_eq!(a.id, b.id, "连续相同内容应去重、不新增版本");
        assert_eq!(store.list("a.md").len(), 1);
    }

    #[test]
    fn dedup_blob_across_versions() {
        // A→B→A：三次快照、两个不同内容 → 仅两个 blob。
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path());
        store.snapshot("a.md", b"AAA").unwrap();
        store.snapshot("a.md", b"BBB").unwrap();
        store.snapshot("a.md", b"AAA").unwrap();
        assert_eq!(store.list("a.md").len(), 3);
        let blob_count = fs::read_dir(dir.path().join("blobs")).unwrap().count();
        assert_eq!(blob_count, 2, "相同内容 blob 应去重");
    }

    #[test]
    fn retention_cap_prunes_oldest() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path()).with_max_versions(3);
        for i in 0..6 {
            store.snapshot("a.md", format!("v{i}").as_bytes()).unwrap();
        }
        let chain = store.list("a.md");
        assert_eq!(chain.len(), 3, "保留上限应裁到 3 条");
        // 最旧的 v0/v1/v2 被裁，剩 v3/v4/v5；其 blob 也被 GC。
        let blob_count = fs::read_dir(dir.path().join("blobs")).unwrap().count();
        assert_eq!(blob_count, 3, "无引用 blob 应被 GC");
    }

    #[test]
    fn system_baseline_is_distinct_from_ordinary_history_and_survives_retention() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path()).with_max_versions(3);
        store.snapshot("topic.md", b"ordinary").unwrap();
        assert!(store.latest_system_baseline("topic.md").unwrap().is_none());
        store
            .snapshot_system_baseline("topic.md", b"system")
            .unwrap();
        for i in 0..7 {
            store
                .snapshot("topic.md", format!("edit-{i}").as_bytes())
                .unwrap();
        }
        assert_eq!(store.list("topic.md").len(), 4);
        assert_eq!(
            store.latest_system_baseline("topic.md").unwrap(),
            Some(b"system".to_vec())
        );
        store
            .snapshot_system_baseline("topic.md", b"new-system")
            .unwrap();
        for i in 0..4 {
            store
                .snapshot("topic.md", format!("later-{i}").as_bytes())
                .unwrap();
        }
        assert_eq!(
            store.latest_system_baseline("topic.md").unwrap(),
            Some(b"new-system".to_vec())
        );
    }

    #[test]
    fn a_matching_ordinary_snapshot_cannot_become_a_baseline_by_reading() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path());
        store.snapshot("topic.md", b"same-body").unwrap();
        assert!(store.latest_system_baseline("topic.md").unwrap().is_none());
        store
            .snapshot_system_baseline("topic.md", b"same-body")
            .unwrap();
        assert_eq!(store.list("topic.md").len(), 1);
        assert_eq!(
            store.latest_system_baseline("topic.md").unwrap(),
            Some(b"same-body".to_vec())
        );
        let meta = store.list("topic.md").pop().unwrap();
        fs::write(dir.path().join("blobs").join(meta.hash), b"tampered").unwrap();
        assert!(store.latest_system_baseline("topic.md").is_err());
    }

    #[test]
    fn recording_baseline_repairs_a_missing_deduplicated_blob() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path());
        let meta = store.snapshot("topic.md", b"system").unwrap();
        fs::remove_file(dir.path().join("blobs").join(meta.hash)).unwrap();
        store
            .snapshot_system_baseline("topic.md", b"system")
            .unwrap();
        assert_eq!(
            store.latest_system_baseline("topic.md").unwrap(),
            Some(b"system".to_vec())
        );
    }

    #[test]
    fn minimum_history_limit_keeps_the_latest_ordinary_snapshot_beside_baseline() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path()).with_max_versions(1);
        store
            .snapshot_system_baseline("topic.md", b"system")
            .unwrap();
        let edit = store.snapshot("topic.md", b"edited").unwrap();
        let chain = store.list("topic.md");
        assert_eq!(chain.len(), 2);
        assert!(chain.iter().any(|entry| entry.id == edit.id));
        assert_eq!(
            store.latest_system_baseline("topic.md").unwrap(),
            Some(b"system".to_vec())
        );
    }
}
