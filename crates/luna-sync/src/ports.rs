//! 引擎向外要的两样东西：远端（服务端 `/sync/v1`）与本地库。实现在应用装配层——远端用
//! `luna-cloud` 的同步客户端，本地用 `luna_vault::Vault`（写入登记自写抑制、发 `vault://changed`）；
//! 测试与模拟器用内存替身。
//!
//! 这里的类型是本 crate 自己的形状，不是 wire DTO：服务端的 JSON 由 `luna-cloud` 解析，装配层
//! 翻译过来。两边各持一份，中层与 `luna-cloud` 才能互不依赖（TDD-00 §3）。唯一的例外是提交：
//! 条目、请求体与逐条结果用的是零依赖叶子 `luna-sync-core::commit` 的类型——网页版发的是同一份，
//! 两端的提交体必须逐字节相同（ADR-177 ⑥）。

use async_trait::async_trait;
use luna_sync_core::commit::{CommitItem, CommitReply};
use uuid::Uuid;

use crate::core::LocalFile;

/// 变更流 / 清单里的一个条目（密文原样）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEntry {
    pub file_id: Uuid,
    pub rev: i64,
    pub seq: i64,
    pub deleted: bool,
    pub entry_ct: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteChanges {
    pub epoch: Uuid,
    pub entries: Vec<RemoteEntry>,
    pub next_seq: i64,
    pub has_more: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteManifest {
    pub epoch: Uuid,
    pub at_seq: i64,
    pub entries: Vec<RemoteEntry>,
}

/// 远端的失败（稳定码由装配层从服务端错误体翻译过来，TDD-16 §9）。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RemoteError {
    #[error("sync_epoch_changed")]
    EpochChanged,
    #[error("sync_cursor_expired")]
    CursorExpired,
    #[error("sync_read_only")]
    ReadOnly,
    #[error("sync_quota_exceeded")]
    QuotaExceeded,
    #[error("sync_not_entitled")]
    NotEntitled,
    #[error("sync_storage_unavailable")]
    StorageUnavailable,
    /// 账户因付款争议被冻结为只读（服务端 403 `account_frozen`，纵切 3 C3）：写入被拒、读取照常。
    /// 引擎据此暂停上传、照常落地下载（[`crate::engine::RoundReport::account_frozen`]），不当成一轮失败。
    #[error("account_frozen")]
    AccountFrozen,
    #[error("not_found")]
    NotFound,
    /// 云端库被用户删除了（服务端 410 `vault_deleted`，协议 v15）：不是一次可重试的失败——装配层
    /// 据此断开这个库（本地文件不动）并告诉用户一次，不再为它发请求。
    #[error("vault_deleted")]
    VaultDeleted,
    #[error("unauthorized")]
    Unauthorized,
    #[error("rate_limited")]
    RateLimited { retry_after_secs: Option<u64> },
    /// 连不上、超时（原因码只进日志）。
    #[error("network: {0}")]
    Network(String),
    /// 其余服务端稳定码。
    #[error("server: {0}")]
    Other(String),
}

impl RemoteError {
    /// 给界面的稳定码：网络失败只说 `network`（原因码只进日志），其余服务端码原样给出。
    #[must_use]
    pub fn code(&self) -> String {
        match self {
            Self::Network(_) => "network".to_owned(),
            Self::Other(code) => code.clone(),
            other => other.to_string(),
        }
    }
}

/// 服务端同步接口（只列引擎用得到的）。
#[async_trait]
pub trait Remote: Send + Sync {
    async fn changes(
        &self,
        vault_id: Uuid,
        epoch: Uuid,
        since: i64,
        limit: u32,
    ) -> Result<RemoteChanges, RemoteError>;

    /// 全量清单：`at_seq = None` 为当前；给了就是那个序号时的库（整库时间点恢复，PRD-13-007）。
    async fn manifest(
        &self,
        vault_id: Uuid,
        at_seq: Option<i64>,
    ) -> Result<RemoteManifest, RemoteError>;

    /// 这些块里服务端还没有的。
    async fn missing_blobs(
        &self,
        vault_id: Uuid,
        chunk_ids: &[String],
    ) -> Result<Vec<String>, RemoteError>;

    async fn put_blob(
        &self,
        vault_id: Uuid,
        chunk_id: &str,
        sealed: &[u8],
        ct_sha256: [u8; 32],
    ) -> Result<(), RemoteError>;

    async fn get_blob(&self, vault_id: Uuid, chunk_id: &str) -> Result<Vec<u8>, RemoteError>;

    /// 某个文件某一版的条目（按需下载仅云端的文件）。
    async fn entry_at(
        &self,
        vault_id: Uuid,
        file_id: Uuid,
        rev: i64,
    ) -> Result<RemoteEntry, RemoteError>;

    async fn commit(
        &self,
        vault_id: Uuid,
        idempotency_key: &str,
        items: Vec<CommitItem>,
    ) -> Result<CommitReply, RemoteError>;
}

/// 写本地之前对现状的期望（不符就不写：用户在这一轮里刚改过它）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expect {
    /// 那个位置现在不能有文件。
    Absent,
    /// 那个位置现在的内容哈希是这个。
    Hash(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOutcome {
    Done,
    /// 现状与期望不符，没有动。
    Changed,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("local vault: {0}")]
pub struct LocalError(pub String);

/// 下载暂存好的一份大文件（库内的临时文件，由 [`LocalVault::stage`] 造出）：写入时整个换上去，
/// 用不上就 [`LocalVault::discard`]。`token` 只有造它的那个本地库认得。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Staged {
    pub token: String,
    pub content_hash: String,
    pub size: u64,
}

/// 本地库（路径一律库相对、`/` 分隔、NFC）。
pub trait LocalVault: Send + Sync {
    /// `.luna/vault-id`（没有 = `None`）。
    fn vault_id(&self) -> Option<String>;
    /// 参与同步的全部文件（带内容哈希）。
    fn list(&self) -> Result<Vec<LocalFile>, LocalError>;
    fn read(&self, path: &str) -> Result<Vec<u8>, LocalError>;
    /// 流式读（大文件：录音动辄几百 MB，不整份进内存）。读到的内容由调用方按哈希核对。
    fn open(&self, path: &str) -> Result<Box<dyn std::io::Read + Send>, LocalError>;
    /// 造一份暂存文件：`fill` 往里写（逐块拼大文件），写完返回它的哈希与大小。
    fn stage(
        &self,
        fill: &mut dyn FnMut(&mut dyn std::io::Write) -> Result<(), LocalError>,
    ) -> Result<Staged, LocalError>;
    /// 现状符合期望时把暂存文件换到 `path`（原子改名，暂存随之用掉）；不符则原样留着。
    fn write_staged_if(
        &self,
        path: &str,
        staged: &Staged,
        expect: &Expect,
    ) -> Result<WriteOutcome, LocalError>;
    /// 丢掉一份没用上的暂存。
    fn discard(&self, staged: &Staged);
    /// 本机块缓存：下载到一半的大文件已解密的块（按块 id），跨轮、跨重启续传用。
    fn cached_chunk(&self, chunk_id: &str) -> Option<Vec<u8>>;
    fn cache_chunk(&self, chunk_id: &str, plain: &[u8]) -> Result<(), LocalError>;
    /// 一个文件落地之后，它的块就不必再留。
    fn forget_chunks(&self, chunk_ids: &[String]);
    /// 一个基线里有、此刻没扫到的文件为什么不在（见 `luna_vault::absence`）。
    fn absence(&self, path: &str) -> luna_vault::absence::Absence;
    fn write_if(
        &self,
        path: &str,
        bytes: &[u8],
        expect: &Expect,
    ) -> Result<WriteOutcome, LocalError>;
    /// 删除（进 Lunarca 回收站，不硬删）。
    fn delete_if(&self, path: &str, expected_hash: &str) -> Result<WriteOutcome, LocalError>;
    /// 释放本机空间：内容仍是 `expected_hash` 时移除本机副本（不进回收站——这一版在云端，进回收站
    /// 等于没释放）。只用于基线确认云端有这一版的文件（ADR-156）。
    fn evict_if(&self, path: &str, expected_hash: &str) -> Result<WriteOutcome, LocalError>;
    /// 挪位置（目标位置不能已有文件）。
    fn rename_if(
        &self,
        from: &str,
        to: &str,
        expected_hash: &str,
    ) -> Result<WriteOutcome, LocalError>;
    /// 给一份即将被覆盖的本地内容留一个历史版本（生产 = 快照库的普通版本，可在版本历史里
    /// 找回）。合并写盘之前用：本地那份编辑还没提交，合并结果覆盖它之前必须能撤回（ADR-148 ②）。
    fn keep_version(&self, path: &str, content: &[u8]) -> Result<(), LocalError>;
    /// 钉住一份基线内容，返回指纹。
    fn pin(&self, content: &[u8]) -> Result<String, LocalError>;
    fn read_pinned(&self, hash: &str) -> Result<Vec<u8>, LocalError>;
    fn unpin(&self, hash: &str) -> Result<(), LocalError>;
}
