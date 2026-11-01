//! 测试替身：内存服务端（语义照服务端 `SyncVaultStore`：序号、rev、基线过期逐条返回、缺块、
//! 纪元）与内存本地库。引擎场景用例与确定性模拟器共用。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Mutex;

use async_trait::async_trait;
use luna_vault::absence::{Absence, UnknownAbsence};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::core::LocalFile;
use crate::manifest::content_hash;
use crate::ports::{
    Expect, LocalError, LocalVault, Remote, RemoteChange, RemoteChanges, RemoteCommitItem,
    RemoteCommitReply, RemoteEntry, RemoteError, RemoteItemResult, RemoteManifest, Staged,
    WriteOutcome,
};

#[derive(Default)]
struct ServerState {
    epoch: Uuid,
    head_seq: i64,
    entries: Vec<RemoteEntry>,
    heads: HashMap<Uuid, i64>,
    blobs: HashMap<String, Vec<u8>>,
    idempotency: HashMap<String, RemoteCommitReply>,
    /// 下一次调用直接失败（模拟断网）。
    fail_next: Option<RemoteError>,
    /// 再放行这么多次调用之后失败一次（模拟做到一半断网）。
    fail_later: Option<(usize, RemoteError)>,
    /// 块上传 / 下载成功的次数（续传与「只传变了的块」的证据）。
    blob_puts: usize,
    blob_gets: usize,
    /// 下一次提交照常执行，但回包丢了（客户端以为失败）。
    lose_next_commit_reply: bool,
    /// 存储满了（ADR-152）：块上传与带内容的提交回 `QuotaExceeded`，只含删除的提交照常执行。
    full: bool,
    /// 账户被冻结（纵切 3 C3）：块上传与提交一律回 `AccountFrozen`，读取照常。
    frozen: bool,
}

/// 一个远端库的内存服务端。
pub struct FakeServer {
    pub vault_id: Uuid,
    state: Mutex<ServerState>,
}

impl FakeServer {
    #[must_use]
    pub fn new(vault_id: Uuid) -> Self {
        Self {
            vault_id,
            state: Mutex::new(ServerState {
                epoch: Uuid::now_v7(),
                ..ServerState::default()
            }),
        }
    }

    fn with<T>(
        &self,
        f: impl FnOnce(&mut ServerState) -> Result<T, RemoteError>,
    ) -> Result<T, RemoteError> {
        let mut state = self.state.lock().expect("fake server poisoned");
        if let Some(err) = state.fail_next.take() {
            return Err(err);
        }
        if let Some((left, _)) = state.fail_later.as_mut() {
            if *left == 0 {
                let (_, err) = state.fail_later.take().expect("checked above");
                return Err(err);
            }
            *left -= 1;
        }
        f(&mut state)
    }

    /// 库的历史被改写（例如从备份恢复）：换纪元，数据不变。
    pub fn change_epoch(&self) {
        self.state.lock().unwrap().epoch = Uuid::now_v7();
    }

    /// 下一次调用失败。
    /// 存储满了 / 腾出了空间（ADR-152）。
    pub fn set_full(&self, full: bool) {
        self.state.lock().unwrap().full = full;
    }

    /// 账户被冻结 / 解冻（纵切 3 C3）。
    pub fn set_frozen(&self, frozen: bool) {
        self.state.lock().unwrap().frozen = frozen;
    }

    pub fn fail_next(&self, err: RemoteError) {
        self.state.lock().unwrap().fail_next = Some(err);
    }

    /// 再放行 `calls` 次调用，之后那一次失败。
    /// 自上次取数以来成功上传 / 下载了多少块（取完清零）。
    pub fn take_blob_traffic(&self) -> (usize, usize) {
        let mut s = self.state.lock().unwrap();
        let traffic = (s.blob_puts, s.blob_gets);
        s.blob_puts = 0;
        s.blob_gets = 0;
        traffic
    }

    pub fn fail_after(&self, calls: usize, err: RemoteError) {
        self.state.lock().unwrap().fail_later = Some((calls, err));
    }

    /// 下一次提交执行了、回包却丢了。
    pub fn lose_next_commit_reply(&self) {
        self.state.lock().unwrap().lose_next_commit_reply = true;
    }

    /// 撤掉还没触发的那次失败（网络恢复）。
    pub fn heal(&self) {
        let mut state = self.state.lock().unwrap();
        state.fail_next = None;
        state.fail_later = None;
        state.lose_next_commit_reply = false;
    }

    /// 当前活文件数。
    #[must_use]
    pub fn live_files(&self) -> usize {
        let state = self.state.lock().unwrap();
        latest(&state.entries)
            .values()
            .filter(|e| !e.deleted)
            .count()
    }

    #[must_use]
    pub fn head_seq(&self) -> i64 {
        self.state.lock().unwrap().head_seq
    }

    /// 全部历史条目（按序号）。
    #[must_use]
    pub fn entries(&self) -> Vec<RemoteEntry> {
        self.state.lock().unwrap().entries.clone()
    }

    #[must_use]
    pub fn blob(&self, chunk_id: &str) -> Option<Vec<u8>> {
        self.state.lock().unwrap().blobs.get(chunk_id).cloned()
    }
}

fn latest(entries: &[RemoteEntry]) -> BTreeMap<Uuid, RemoteEntry> {
    let mut out = BTreeMap::new();
    for e in entries {
        out.insert(e.file_id, e.clone());
    }
    out
}

#[async_trait]
impl Remote for FakeServer {
    async fn changes(
        &self,
        _vault_id: Uuid,
        epoch: Uuid,
        since: i64,
        limit: u32,
    ) -> Result<RemoteChanges, RemoteError> {
        self.with(|s| {
            if epoch != s.epoch || since > s.head_seq {
                return Err(RemoteError::EpochChanged);
            }
            let mut page: Vec<RemoteEntry> = s
                .entries
                .iter()
                .filter(|e| e.seq > since)
                .cloned()
                .collect();
            let has_more = page.len() > limit as usize;
            page.truncate(limit as usize);
            let next_seq = if has_more {
                page.last().map_or(since, |e| e.seq)
            } else {
                s.head_seq
            };
            Ok(RemoteChanges {
                epoch: s.epoch,
                entries: page,
                next_seq,
                has_more,
            })
        })
    }

    async fn manifest(
        &self,
        _vault_id: Uuid,
        at_seq: Option<i64>,
    ) -> Result<RemoteManifest, RemoteError> {
        self.with(|s| {
            let at = at_seq.unwrap_or(s.head_seq);
            let upto: Vec<RemoteEntry> =
                s.entries.iter().filter(|e| e.seq <= at).cloned().collect();
            Ok(RemoteManifest {
                epoch: s.epoch,
                at_seq: at,
                entries: latest(&upto).into_values().filter(|e| !e.deleted).collect(),
            })
        })
    }

    async fn missing_blobs(
        &self,
        _vault_id: Uuid,
        ids: &[String],
    ) -> Result<Vec<String>, RemoteError> {
        self.with(|s| {
            Ok(ids
                .iter()
                .filter(|id| !s.blobs.contains_key(*id))
                .cloned()
                .collect())
        })
    }

    async fn put_blob(
        &self,
        _vault_id: Uuid,
        chunk_id: &str,
        sealed: &[u8],
        ct_sha256: [u8; 32],
    ) -> Result<(), RemoteError> {
        self.with(|s| {
            if s.frozen {
                return Err(RemoteError::AccountFrozen);
            }
            if s.full {
                return Err(RemoteError::QuotaExceeded);
            }
            let actual: [u8; 32] = Sha256::digest(sealed).into();
            if actual != ct_sha256 {
                return Err(RemoteError::Other("sync_chunk_digest_mismatch".into()));
            }
            match s.blobs.get(chunk_id) {
                Some(existing) if existing.as_slice() != sealed => {
                    Err(RemoteError::Other("sync_chunk_mismatch".into()))
                }
                _ => {
                    s.blobs.insert(chunk_id.to_owned(), sealed.to_vec());
                    s.blob_puts += 1;
                    Ok(())
                }
            }
        })
    }

    async fn entry_at(
        &self,
        _vault_id: Uuid,
        file_id: Uuid,
        rev: i64,
    ) -> Result<RemoteEntry, RemoteError> {
        self.with(|s| {
            s.entries
                .iter()
                .find(|e| e.file_id == file_id && e.rev == rev)
                .cloned()
                .ok_or(RemoteError::NotFound)
        })
    }

    async fn get_blob(&self, _vault_id: Uuid, chunk_id: &str) -> Result<Vec<u8>, RemoteError> {
        self.with(|s| {
            let blob = s
                .blobs
                .get(chunk_id)
                .cloned()
                .ok_or(RemoteError::NotFound)?;
            s.blob_gets += 1;
            Ok(blob)
        })
    }

    async fn commit(
        &self,
        _vault_id: Uuid,
        key: &str,
        items: Vec<RemoteCommitItem>,
    ) -> Result<RemoteCommitReply, RemoteError> {
        self.with(|s| {
            if let Some(reply) = s.idempotency.get(key) {
                return Ok(reply.clone());
            }
            if s.frozen {
                return Err(RemoteError::AccountFrozen);
            }
            if s.full
                && items
                    .iter()
                    .any(|item| !matches!(item.change, RemoteChange::Delete))
            {
                return Err(RemoteError::QuotaExceeded);
            }
            let mut results = Vec::with_capacity(items.len());
            for item in items {
                let current = s.heads.get(&item.file_id).copied();
                if current != item.base_rev {
                    results.push(RemoteItemResult::StaleBase {
                        file_id: item.file_id,
                        current_rev: current,
                    });
                    continue;
                }
                let (deleted, entry_ct) = match item.change {
                    RemoteChange::Put {
                        entry_ct,
                        chunk_ids,
                        ..
                    } => {
                        let missing: Vec<String> = chunk_ids
                            .iter()
                            .filter(|id| !s.blobs.contains_key(*id))
                            .cloned()
                            .collect();
                        if !missing.is_empty() {
                            results.push(RemoteItemResult::MissingBlobs {
                                file_id: item.file_id,
                                chunk_ids: missing,
                            });
                            continue;
                        }
                        (false, Some(entry_ct))
                    }
                    RemoteChange::Delete => (true, None),
                };
                s.head_seq += 1;
                let rev = current.unwrap_or(0) + 1;
                s.heads.insert(item.file_id, rev);
                s.entries.push(RemoteEntry {
                    file_id: item.file_id,
                    rev,
                    seq: s.head_seq,
                    deleted,
                    entry_ct,
                });
                results.push(RemoteItemResult::Committed {
                    file_id: item.file_id,
                    rev,
                    seq: s.head_seq,
                });
            }
            let reply = RemoteCommitReply {
                head_seq: s.head_seq,
                results,
            };
            s.idempotency.insert(key.to_owned(), reply.clone());
            if std::mem::take(&mut s.lose_next_commit_reply) {
                return Err(RemoteError::Network("reply lost".into()));
            }
            Ok(reply)
        })
    }
}

#[derive(Default)]
struct VaultState {
    files: BTreeMap<String, Vec<u8>>,
    trash: Vec<(String, Vec<u8>)>,
    pins: HashMap<String, Vec<u8>>,
    /// 留下的历史版本（路径，内容）。
    versions: Vec<(String, Vec<u8>)>,
    /// 这些路径不在、且说不准为什么（iCloud 占位之类）。
    uncertain: HashSet<String>,
    /// 这些文件还在被写（正在录的录音）。
    settling: HashSet<String>,
    /// 暂存（token → 内容）。
    staged: HashMap<String, Vec<u8>>,
    /// 本机块缓存（块 id → 明文）。
    chunk_cache: HashMap<String, Vec<u8>>,
}

/// 内存本地库。
pub struct MemVault {
    vault_id: String,
    state: Mutex<VaultState>,
}

impl MemVault {
    #[must_use]
    pub fn new(vault_id: &str) -> Self {
        Self {
            vault_id: vault_id.to_owned(),
            state: Mutex::new(VaultState::default()),
        }
    }

    /// 用户写一个文件。占位（「说不准」）标记不受影响：真实文件系统里 iCloud 占位是另一个文件，
    /// 在下载完成（[`Self::settle`]）之前一直在。
    /// 块缓存里此刻有几块（续传测试用）。
    #[must_use]
    pub fn cached_chunk_count(&self) -> usize {
        self.state.lock().unwrap().chunk_cache.len()
    }

    /// 标记一个文件还在被写（扫描时报 `settling`）。
    pub fn set_settling(&self, path: &str, settling: bool) {
        let mut state = self.state.lock().unwrap();
        if settling {
            state.settling.insert(path.to_owned());
        } else {
            state.settling.remove(path);
        }
    }

    pub fn put(&self, path: &str, bytes: &[u8]) {
        self.state
            .lock()
            .unwrap()
            .files
            .insert(path.to_owned(), bytes.to_vec());
    }

    /// 占位消失（下载完成）。
    pub fn settle(&self, path: &str) {
        self.state.lock().unwrap().uncertain.remove(path);
    }

    /// 用户删一个文件。
    pub fn remove(&self, path: &str) {
        self.state.lock().unwrap().files.remove(path);
    }

    /// 文件变成「说不准」（例如 iCloud 只留了占位）。
    pub fn make_uncertain(&self, path: &str) {
        let mut s = self.state.lock().unwrap();
        s.files.remove(path);
        s.uncertain.insert(path.to_owned());
    }

    /// 用户改名。
    pub fn rename(&self, from: &str, to: &str) {
        let mut s = self.state.lock().unwrap();
        if let Some(bytes) = s.files.remove(from) {
            s.files.insert(to.to_owned(), bytes);
        }
    }

    #[must_use]
    pub fn get(&self, path: &str) -> Option<Vec<u8>> {
        self.state.lock().unwrap().files.get(path).cloned()
    }

    /// 全部文件（路径 → 内容）。
    #[must_use]
    pub fn snapshot(&self) -> BTreeMap<String, Vec<u8>> {
        self.state.lock().unwrap().files.clone()
    }

    #[must_use]
    pub fn trash(&self) -> Vec<(String, Vec<u8>)> {
        self.state.lock().unwrap().trash.clone()
    }

    #[must_use]
    pub fn versions(&self) -> Vec<(String, Vec<u8>)> {
        self.state.lock().unwrap().versions.clone()
    }
}

impl LocalVault for MemVault {
    fn vault_id(&self) -> Option<String> {
        Some(self.vault_id.clone())
    }

    fn list(&self) -> Result<Vec<LocalFile>, LocalError> {
        let state = self.state.lock().unwrap();
        Ok(state
            .files
            .iter()
            .map(|(path, bytes)| {
                let settling = state.settling.contains(path);
                LocalFile {
                    path: path.clone(),
                    content_hash: if settling {
                        String::new()
                    } else {
                        content_hash(bytes)
                    },
                    size: bytes.len() as u64,
                    mtime_ms: None,
                    settling,
                }
            })
            .collect())
    }

    fn read(&self, path: &str) -> Result<Vec<u8>, LocalError> {
        self.get(path).ok_or_else(|| LocalError("not_found".into()))
    }

    fn absence(&self, path: &str) -> Absence {
        let s = self.state.lock().unwrap();
        if s.files.contains_key(path) {
            Absence::Present
        } else if s.uncertain.contains(path) {
            Absence::Unknown(UnknownAbsence::CloudPlaceholder)
        } else {
            Absence::Deleted
        }
    }

    fn open(&self, path: &str) -> Result<Box<dyn std::io::Read + Send>, LocalError> {
        let bytes = self.read(path)?;
        Ok(Box::new(std::io::Cursor::new(bytes)))
    }

    fn stage(
        &self,
        fill: &mut dyn FnMut(&mut dyn std::io::Write) -> Result<(), LocalError>,
    ) -> Result<Staged, LocalError> {
        let mut bytes = Vec::new();
        fill(&mut bytes)?;
        let token = Uuid::now_v7().to_string();
        let staged = Staged {
            token: token.clone(),
            content_hash: content_hash(&bytes),
            size: bytes.len() as u64,
        };
        self.state.lock().unwrap().staged.insert(token, bytes);
        Ok(staged)
    }

    fn write_staged_if(
        &self,
        path: &str,
        staged: &Staged,
        expect: &Expect,
    ) -> Result<WriteOutcome, LocalError> {
        let bytes = self
            .state
            .lock()
            .unwrap()
            .staged
            .get(&staged.token)
            .cloned()
            .ok_or_else(|| LocalError("staged_missing".into()))?;
        let outcome = self.write_if(path, &bytes, expect)?;
        if outcome == WriteOutcome::Done {
            self.state.lock().unwrap().staged.remove(&staged.token);
        }
        Ok(outcome)
    }

    fn discard(&self, staged: &Staged) {
        self.state.lock().unwrap().staged.remove(&staged.token);
    }

    fn cached_chunk(&self, chunk_id: &str) -> Option<Vec<u8>> {
        self.state
            .lock()
            .unwrap()
            .chunk_cache
            .get(chunk_id)
            .cloned()
    }

    fn cache_chunk(&self, chunk_id: &str, plain: &[u8]) -> Result<(), LocalError> {
        self.state
            .lock()
            .unwrap()
            .chunk_cache
            .insert(chunk_id.to_owned(), plain.to_vec());
        Ok(())
    }

    fn forget_chunks(&self, chunk_ids: &[String]) {
        let mut s = self.state.lock().unwrap();
        for id in chunk_ids {
            s.chunk_cache.remove(id);
        }
    }

    fn write_if(
        &self,
        path: &str,
        bytes: &[u8],
        expect: &Expect,
    ) -> Result<WriteOutcome, LocalError> {
        let mut s = self.state.lock().unwrap();
        let ok = match (expect, s.files.get(path)) {
            (Expect::Absent, None) => true,
            (Expect::Hash(hash), Some(current)) => content_hash(current) == *hash,
            _ => false,
        };
        if !ok {
            return Ok(WriteOutcome::Changed);
        }
        s.files.insert(path.to_owned(), bytes.to_vec());
        Ok(WriteOutcome::Done)
    }

    fn evict_if(&self, path: &str, expected_hash: &str) -> Result<WriteOutcome, LocalError> {
        let mut s = self.state.lock().unwrap();
        match s.files.get(path) {
            Some(current) if content_hash(current) == expected_hash => {
                s.files.remove(path);
                Ok(WriteOutcome::Done)
            }
            _ => Ok(WriteOutcome::Changed),
        }
    }

    fn delete_if(&self, path: &str, expected_hash: &str) -> Result<WriteOutcome, LocalError> {
        let mut s = self.state.lock().unwrap();
        match s.files.get(path) {
            Some(current) if content_hash(current) == expected_hash => {
                let bytes = s.files.remove(path).unwrap_or_default();
                s.trash.push((path.to_owned(), bytes));
                Ok(WriteOutcome::Done)
            }
            _ => Ok(WriteOutcome::Changed),
        }
    }

    fn rename_if(
        &self,
        from: &str,
        to: &str,
        expected_hash: &str,
    ) -> Result<WriteOutcome, LocalError> {
        let mut s = self.state.lock().unwrap();
        let ok = s
            .files
            .get(from)
            .is_some_and(|b| content_hash(b) == expected_hash)
            && !s.files.contains_key(to);
        if !ok {
            return Ok(WriteOutcome::Changed);
        }
        let bytes = s.files.remove(from).unwrap_or_default();
        s.files.insert(to.to_owned(), bytes);
        Ok(WriteOutcome::Done)
    }

    fn keep_version(&self, path: &str, content: &[u8]) -> Result<(), LocalError> {
        self.state
            .lock()
            .unwrap()
            .versions
            .push((path.to_owned(), content.to_vec()));
        Ok(())
    }

    fn pin(&self, content: &[u8]) -> Result<String, LocalError> {
        let hash = content_hash(content);
        self.state
            .lock()
            .unwrap()
            .pins
            .insert(hash.clone(), content.to_vec());
        Ok(hash)
    }

    fn read_pinned(&self, hash: &str) -> Result<Vec<u8>, LocalError> {
        self.state
            .lock()
            .unwrap()
            .pins
            .get(hash)
            .cloned()
            .ok_or_else(|| LocalError("pin_missing".into()))
    }

    fn unpin(&self, hash: &str) -> Result<(), LocalError> {
        self.state.lock().unwrap().pins.remove(hash);
        Ok(())
    }
}
