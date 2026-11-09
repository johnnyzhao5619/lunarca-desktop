//! 同步引擎：一轮同步的外层执行器（ADR-148；方案 §4.6）。决策在 [`crate::core`]，本模块只做
//! I/O 与记账。
//!
//! # 一轮
//!
//! 1. **前置**：有绑定；本地库身份（`.luna/vault-id`）等于绑定值——不等就整轮中止（库被换了、
//!    或这是一份复制出来的库）。
//! 2. **拉取**：有游标走增量变更流；纪元变了或游标过期就丢掉游标、改做全量清单对账。
//! 3. **扫描本地**，按基线路径把文件认回它的 `file_id`；基线里有、扫不到的文件交给
//!    `absence` 判定（只有观察到的删除才算删除）；内容与某个「删除」完全相同的新文件当作改名。
//! 4. **决策**，过批量删除熔断。
//! 5. **执行**：先把云端的改动落到本地（应用、删除、合并、冲突副本），再上传缺的块、批量提交。
//! 6. **落账**：基线改动与新游标一个事务。只要有云端改动这一轮没落到本地，游标就不前进——
//!    下一轮重拉同一段（已落地的那些基线已更新，重拉是无害的）。
//!
//! # 护栏
//!
//! - 本地写一律带期望（`Expect`）：用户在这一轮里刚改过的文件不被覆盖，留给下一轮。
//! - 任何一侧整份清单为空而另一侧不空：拒绝把空的一侧当成删除来源，整轮中止。
//! - 云端删除落到本地回收站（`LocalVault::delete_if`），不硬删。
//! - 日志与活动记录只有 file_id 与原因码。

use std::collections::{HashMap, HashSet};

use uuid::Uuid;

use crate::core::{
    decide, deletions_need_confirmation, Action, FileView, LocalFile, LocalSide, Note, RemoteSide,
};
use crate::ports::{
    Expect, LocalError, LocalVault, Remote, RemoteEntry, RemoteError, Staged, WriteOutcome,
};
use crate::state::{path_key, Activity, BaseChange, BaseFile, Cursor, StateError, SyncState};
use luna_sync_core::chunk::{pad, stream_chunks, unpad, WHOLE_FILE_MAX};
use luna_sync_core::commit::{self, CommitChange, CommitItem, CommitResult};
use luna_sync_core::crypto::{CryptoError, KeyRing};
use luna_sync_core::manifest::{
    assemble, content_hash, open_entry, prepare, EntryPlain, ManifestError,
};
use luna_sync_core::naming::{conflict_copy_path, CONFLICT_COPY_ATTEMPTS};

/// 变更流一页拉多少。
const PAGE: u32 = 500;
/// 一次提交至多多少条（服务端上限）。
const COMMIT_BATCH: usize = 500;
/// 一次问「缺哪些块」至多多少个（服务端上限）。
const MISSING_BATCH: usize = 1000;
/// 一个条目至多引用多少块（服务端 `MAX_CHUNKS_PER_ENTRY` 同值）：按最小块 256 KiB 约 2.4 GiB，
/// 按平均 1 MiB 约 10 GiB。
pub const MAX_CHUNKS_PER_ENTRY: usize = 10_000;
/// 一次提交的请求体预算（服务端上限 8 MiB，留出 JSON 与 base64 的余量）。大录音的条目带着上千个
/// 块 id，按条数分批会让几个大文件挤在一次提交里被服务端拒掉。
const COMMIT_BYTES: usize = 6 * 1024 * 1024;

/// 一个上传条目在提交请求里大约占多少字节：密文条目 base64 后约 4/3，块 id 在条目里与
/// `chunk_ids` 里各出现一次（每个约 70 字节 JSON）。
fn commit_item_bytes(file: &luna_sync_core::manifest::PreparedFile) -> usize {
    file.entry.chunks.len() * 200 + file.entry.path.len() * 2 + 1024
}

/// 把要提交的上传按条数与请求体预算分批（一个文件本身超预算时独占一批）。
fn commit_batches(
    puts: &[usize],
    prepared: &[Option<luna_sync_core::manifest::PreparedFile>],
) -> Vec<Vec<usize>> {
    let mut batches: Vec<Vec<usize>> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    let mut bytes = 0;
    for &i in puts {
        let size = prepared[i].as_ref().map_or(1024, commit_item_bytes);
        if !current.is_empty() && (current.len() == COMMIT_BATCH || bytes + size > COMMIT_BYTES) {
            batches.push(std::mem::take(&mut current));
            bytes = 0;
        }
        current.push(i);
        bytes += size;
    }
    if !current.is_empty() {
        batches.push(current);
    }
    batches
}

/// 一波上传的字节上限：满额时最多白传这么多，已提交的波次不受影响。
const UPLOAD_WAVE_BYTES: u64 = 32 * 1024 * 1024;

/// 把要上传的文件分波（ADR-178 ④）：笔记在前，其余从小到大，每波传完就提交。容量不够时笔记已经
/// 落地，停在大文件上；不分波的话一个放不下的录音会让整轮一个文件都提交不了。
pub(crate) fn upload_waves(mut puts: Vec<(usize, bool, u64)>) -> Vec<Vec<usize>> {
    puts.sort_by_key(|&(_, is_note, size)| (!is_note, size));
    let mut waves: Vec<Vec<usize>> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    let mut bytes = 0u64;
    let mut note_wave = true;
    for (index, is_note, size) in puts {
        if !current.is_empty() && (is_note != note_wave || bytes + size > UPLOAD_WAVE_BYTES) {
            waves.push(std::mem::take(&mut current));
            bytes = 0;
        }
        note_wave = is_note;
        current.push(index);
        bytes += size;
    }
    if !current.is_empty() {
        waves.push(current);
    }
    waves
}

/// 一轮的参数。时间与文案由调用方给，核心不读时钟、不认识界面语言。
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// 冲突副本名里的说明（本地化文案，例如「冲突副本」）。
    pub conflict_label: String,
    /// 冲突副本名里的日期（`YYYY-MM-DD`）。
    pub today: String,
    /// 用户已确认这一轮的批量删除。
    pub allow_bulk_delete: bool,
    pub now_ms: i64,
    /// 一个文件至多切成多少块（默认 [`MAX_CHUNKS_PER_ENTRY`]）；超了的文件不上传、记进
    /// [`RoundReport::too_large`]。
    pub max_chunks_per_entry: usize,
    /// 单文件上限（字节，权益 `sync.max_file_bytes`，ADR-156 ④）；`None` = 不知道或不限，不预检
    /// （服务端提交时照样会拒）。超了的文件不切块、不上传，记进 [`RoundReport::too_large`]。
    pub max_file_bytes: Option<u64>,
    /// 这台设备的录音策略（ADR-156）。
    pub recordings: crate::policy::RecordingsMode,
    /// 这一轮不传大文件（按流量计费网络 / 电量低，ADR-156）：大文件的上传与下载都留给下一轮，
    /// 笔记照常。
    pub defer_large: bool,
}

/// 一轮做了什么。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoundReport {
    pub full_reconcile: bool,
    pub pushed: usize,
    pub applied: usize,
    pub merged: usize,
    pub conflict_copies: usize,
    pub deleted_local: usize,
    pub deleted_remote: usize,
    /// 这一轮没动、留给下一轮的文件数（本地说不准、刚被改过、提交时基线过期……）。
    pub deferred: usize,
    /// 被批量删除熔断拦下的文件（等用户确认）。
    pub held_deletions: Vec<Uuid>,
    /// 太大、没有上传的文件（库相对路径）：超过一个条目能引用的块数。其余文件照常同步。
    pub too_large: Vec<String>,
    /// 这一轮的上传被服务端以「账户已冻结」拒绝（纵切 3 C3）：本地改动原样留着、下一轮再试，
    /// 云端的改动照常落到本地。
    pub account_frozen: bool,
}

/// 整轮中止的原因（什么都没落账）。
#[derive(Debug, thiserror::Error)]
pub enum SyncAbort {
    #[error("vault is not bound to a remote vault")]
    NotBound,
    /// 本地库身份与绑定不符：这不是绑定的那份库。
    #[error("local vault identity does not match the binding")]
    IdentityMismatch,
    /// 本机没有解这份数据所需的钥匙版本（别的设备轮换过）：要重新解锁。
    #[error("keys for this vault are missing on this device")]
    KeysMissing,
    /// 本地一个文件都没扫到、基线却有不止一个活文件：不把它当成「全删了」（用户确认后放行）。
    #[error("local vault looks empty")]
    LocalLooksEmpty,
    /// 云端清单为空、基线却有不止一个活文件：不把它当成「全删了」（用户确认后放行）。
    #[error("remote vault looks empty")]
    RemoteLooksEmpty,
    #[error(transparent)]
    Remote(#[from] RemoteError),
    #[error(transparent)]
    Local(#[from] LocalError),
    #[error("sync state: {0}")]
    State(String),
    /// 云端数据解不开或对不上（不是钥匙版本问题）。
    #[error("remote data failed verification: {0}")]
    Corrupt(String),
}

impl From<StateError> for SyncAbort {
    fn from(err: StateError) -> Self {
        Self::State(err.to_string())
    }
}

/// 这一轮里谁占着哪个路径（折叠键 → file_id）。活基线一开始各占自己的路径，只有真的让出
/// （本地删了、挪走了）才释放；云端文件要写进一个被别的文件占着的路径时改写成冲突副本——
/// 否则两个 file_id 的基线指向同一个路径，下一轮按路径认文件会把一个的内容认成另一个的。
#[derive(Default)]
struct Claims {
    owners: HashMap<String, Uuid>,
    /// 本轮的孤立本地文件：没有基线、云端也没有对应的那一份（只会被当成新文件推）。
    /// 只有它们可以被冲突副本认回——别的文件这一轮还要认领自己的路径。
    orphans: HashSet<String>,
}

impl Claims {
    fn held_by_other(&self, path: &str, me: Uuid) -> bool {
        self.owners
            .get(&path_key(path))
            .is_some_and(|owner| *owner != me)
    }

    fn held(&self, path: &str) -> bool {
        self.owners.contains_key(&path_key(path))
    }

    fn claim(&mut self, path: &str, owner: Uuid) {
        self.owners.insert(path_key(path), owner);
    }

    fn is_orphan(&self, path: &str) -> bool {
        self.orphans.contains(&path_key(path))
    }

    fn release(&mut self, path: &str, owner: Uuid) {
        let key = path_key(path);
        if self.owners.get(&key) == Some(&owner) {
            self.owners.remove(&key);
        }
    }
}

/// 先执行会腾出路径的动作（本地删除、挪走），再执行写入：「云端删了 X、在原位置新建了 Y」
/// 这种常见情形不至于让 Y 撞上 X 的占用。
fn frees_a_path(view: &FileView, action: &Action) -> bool {
    match action {
        Action::ApplyDelete => true,
        Action::AdvanceBase => view.remote.as_ref().is_none_or(|r| r.deleted),
        Action::Apply => matches!(
            (&view.local, view.remote.as_ref().and_then(|r| r.entry.as_ref())),
            (LocalSide::Present(file), Some(entry)) if file.path != entry.path
        ),
        _ => false,
    }
}

/// 选择性同步对一个文件的处置（ADR-156）。
enum Selective {
    /// 照常走三态决策。
    Decide,
    /// 仅云端的文件，这台设备要全部下载：下载它。
    Download,
    /// 只改基线（登记 / 更新仅云端、记墓碑），不碰本机文件。
    Record(BaseChange),
    Defer,
    Skip,
}

/// 这个动作要不要搬一个大文件（上传本机的，或下载云端的）。
fn moves_large(view: &FileView, action: &Action) -> bool {
    let local_large = matches!(&view.local, LocalSide::Present(file)
        if crate::policy::is_large(&file.path, file.size));
    let remote_large = view
        .remote
        .as_ref()
        .and_then(|r| r.entry.as_ref())
        .is_some_and(|entry| crate::policy::is_large(&entry.path, entry.size));
    match action {
        Action::Push { .. } | Action::PushAsNew => local_large,
        Action::Apply => remote_large,
        Action::Merge | Action::ConflictCopy => local_large || remote_large,
        _ => false,
    }
}

/// 云端这一版登记为本机的仅云端基线。
fn cloud_only_base(view: &FileView, entry: &EntryPlain, rev: i64) -> BaseFile {
    BaseFile {
        file_id: view.file_id,
        path: entry.path.clone(),
        rev,
        content_hash: entry.content_hash.clone(),
        pin: None,
        deleted: false,
        cloud_only: true,
    }
}

/// 按需下载 / 释放本机空间的结果（ADR-156）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnDemand {
    Done,
    /// 本来就在本机（按需下载）/ 本来就只在云端（释放空间）。
    AlreadyThere,
    /// 同步基线里没有这个文件，或云端已没有这一版（下一轮同步会更新基线）。
    NotFound,
    /// 本机那个位置有别的文件，或本机这份改过还没同步（不能释放）。
    Changed,
    /// 块解不开或拼出来对不上。
    Failed,
}

/// 一个待推送的改动。
struct PendingPush {
    file_id: Uuid,
    base_rev: Option<i64>,
    kind: PushKind,
    /// 推成功后要解钉的旧基线。
    old_pin: Option<String>,
    /// 作为新文件推回时，它取代的那个旧 file_id（推成功后从基线里移除）。
    replaces: Option<Uuid>,
}

enum PushKind {
    Put {
        path: String,
        body: Body,
        mtime_ms: Option<i64>,
    },
    Delete,
}

/// 一份文件内容在引擎里的形态（ADR-156）：笔记与小文件整份在内存；大文件不进内存——本机的那份
/// 推送时流式读两遍，下载的那份逐块进本机块缓存、拼进库内暂存文件再整个换上。
#[derive(Debug, Clone)]
enum Body {
    Bytes(Vec<u8>),
    /// 本机磁盘上的一份大文件（扫描时的哈希与大小；推送时流式读、读完核对）。
    OnDisk {
        path: String,
        hash: String,
        size: u64,
    },
    /// 下载暂存好的大文件，还没换到位。
    Staged(Staged),
}

impl Body {
    fn hash(&self) -> String {
        match self {
            Self::Bytes(bytes) => content_hash(bytes),
            Self::OnDisk { hash, .. } => hash.clone(),
            Self::Staged(staged) => staged.content_hash.clone(),
        }
    }

    /// 写到 `path` 之后它在本机的样子（暂存用掉了，内容就在那个路径上）。
    fn landed_at(&self, path: &str) -> Self {
        match self {
            Self::Staged(staged) => Self::OnDisk {
                path: path.to_owned(),
                hash: staged.content_hash.clone(),
                size: staged.size,
            },
            other => other.clone(),
        }
    }
}

fn is_markdown(path: &str) -> bool {
    path.rsplit('.')
        .next()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
}

/// 一轮同步。
pub struct Engine<'a> {
    pub remote: &'a dyn Remote,
    pub local: &'a dyn LocalVault,
    pub ring: &'a KeyRing,
    pub config: EngineConfig,
}

/// 拉取的结果。
struct Pulled {
    remote: HashMap<Uuid, RemoteSide>,
    cursor: Cursor,
    full: bool,
}

/// 一轮执行期间的账本。
#[derive(Default)]
struct Ledger {
    changes: Vec<BaseChange>,
    unpins: Vec<String>,
    pushes: Vec<PendingPush>,
    remote_incomplete: bool,
    report: RoundReport,
    activity: Vec<(&'static str, Option<Uuid>)>,
}

impl Engine<'_> {
    /// 跑一轮。`Err` = 整轮中止，什么都没落账；唯一的例外是容量满（`QuotaExceeded`）：已提交的波次
    /// 照常落账，这一轮仍以这个错误结束，界面照旧提示满了（ADR-178 ④）。
    pub async fn run_round(&self, state: &mut SyncState) -> Result<RoundReport, SyncAbort> {
        let binding = state.binding()?.ok_or(SyncAbort::NotBound)?;
        if self.local.vault_id().as_deref() != Some(binding.local_vault_id.as_str()) {
            return Err(SyncAbort::IdentityMismatch);
        }
        let vault = binding.remote_vault_id;
        let bases: Vec<BaseFile> = state.bases()?;
        let pulled = self.pull(state, vault, &bases).await?;
        let scanned = self.local.list()?;
        // 仅云端的文件本来就不在本机：不算进「本地看起来被清空了」的护栏，否则一台只有按需
        // 录音的设备每一轮都被拦下。
        let live_bases = bases.iter().filter(|b| !b.deleted && !b.cloud_only).count();
        if scanned.is_empty() && self.empty_side_is_suspicious(live_bases) {
            return Err(SyncAbort::LocalLooksEmpty);
        }

        // 仅云端的登记不碰磁盘，引擎「位置被占 → 冲突副本」那一套走不到它：登记前核对这个
        // 路径没人占（本机的文件、别的活基线、本轮别的登记），被占就退回正常下载。
        let local_keys: HashSet<String> = scanned.iter().map(|f| path_key(&f.path)).collect();
        let base_keys: HashMap<String, Uuid> = bases
            .iter()
            .filter(|b| !b.deleted)
            .map(|b| (path_key(&b.path), b.file_id))
            .collect();
        let views = self.views(&bases, scanned, &pulled.remote);
        let mut ledger = Ledger {
            report: RoundReport {
                full_reconcile: pulled.full,
                ..RoundReport::default()
            },
            ..Ledger::default()
        };

        // 选择性同步（ADR-156）先过一遍：仅云端的文件、按需的录音不进三态决策。
        let mut cloud_only_paths: Vec<(String, Uuid)> = Vec::new();
        let mut decided: Vec<(FileView, Action, Note)> = Vec::new();
        for view in views {
            match self.selective(&view) {
                Selective::Decide => {
                    let (action, note) = decide(&view);
                    decided.push((view, action, note));
                }
                Selective::Download => decided.push((view, Action::Apply, Note::None)),
                Selective::Record(change) => {
                    if let BaseChange::Upsert(base) = &change {
                        if !base.deleted && base.cloud_only {
                            let key = path_key(&base.path);
                            let occupied = local_keys.contains(&key)
                                || base_keys.get(&key).is_some_and(|id| *id != base.file_id)
                                || cloud_only_paths
                                    .iter()
                                    .any(|(p, id)| *id != base.file_id && path_key(p) == key);
                            if occupied {
                                decided.push((view, Action::Apply, Note::None));
                                continue;
                            }
                            cloud_only_paths.push((base.path.clone(), base.file_id));
                        }
                    }
                    ledger.changes.push(change);
                }
                Selective::Defer => self.defer(&view, &mut ledger),
                Selective::Skip => {}
            }
        }
        let deletions = decided
            .iter()
            .filter(|(_, a, _)| matches!(a, Action::PushDelete { .. } | Action::ApplyDelete))
            .count();
        if !self.config.allow_bulk_delete && deletions_need_confirmation(deletions, live_bases) {
            for (view, action, _) in &mut decided {
                if matches!(action, Action::PushDelete { .. } | Action::ApplyDelete) {
                    ledger.report.held_deletions.push(view.file_id);
                    if view.remote.is_some() {
                        ledger.remote_incomplete = true;
                    }
                    *action = Action::Nothing;
                }
            }
        }

        let mut taken = Claims::default();
        for base in bases.iter().filter(|b| !b.deleted) {
            taken.claim(&base.path, base.file_id);
        }
        for (path, owner) in &cloud_only_paths {
            taken.claim(path, *owner);
        }
        for (view, _, _) in &decided {
            if let (None, None, LocalSide::Present(file)) = (&view.base, &view.remote, &view.local)
            {
                taken.orphans.insert(path_key(&file.path));
            }
        }
        decided.sort_by_key(|(view, action, _)| !frees_a_path(view, action));
        for (view, action, note) in decided {
            // 还在写的大文件：这一轮不碰（推送会读到半截，拉取会覆盖正在写的那一份）。
            // 按流量计费网络 / 电量低：大文件的上传下载留给下一轮。
            if matches!(&view.local, LocalSide::Present(file) if file.settling)
                || (self.config.defer_large && moves_large(&view, &action))
            {
                self.defer(&view, &mut ledger);
                continue;
            }
            self.execute(vault, view, action, note, &mut ledger, &mut taken)
                .await?;
        }
        // 账户被冻结（纵切 3 C3）：服务端拒绝一切写入、读取照常。这不是一轮失败——云端的改动
        // 已经落到本地，照常落账；没推上去的本地改动不进基线，下一轮还会再推（服务端一解冻就
        // 自愈，不等客户端重取权益）。游标不前进：合并后待推的那几份引用着这一段云端改动，
        // 下一轮要重拉同一段才认得出它们（已落地的那些重拉是无害的）。
        let mut quota_exceeded = false;
        match self.push_all(vault, &mut ledger).await {
            Ok(()) => {}
            Err(SyncAbort::Remote(RemoteError::AccountFrozen)) => {
                ledger.report.account_frozen = true;
                ledger.remote_incomplete = true;
            }
            Err(SyncAbort::Remote(RemoteError::QuotaExceeded)) => {
                quota_exceeded = true;
                ledger.remote_incomplete = true;
            }
            Err(err) => return Err(err),
        }

        let cursor = (!ledger.remote_incomplete).then_some(pulled.cursor);
        state.commit_round(&ledger.changes, cursor)?;
        // 只解没有任何基线还在用的钉住：内容相同的两个文件共用同一个指纹。
        let still: HashSet<String> = state.bases()?.into_iter().filter_map(|b| b.pin).collect();
        for pin in ledger.unpins.iter().filter(|p| !still.contains(*p)) {
            let _ = self.local.unpin(pin);
        }
        for (kind, file_id) in &ledger.activity {
            state.record_activity(&Activity {
                at_ms: self.config.now_ms,
                kind: (*kind).to_owned(),
                file_id: *file_id,
                detail_code: None,
            })?;
        }
        if quota_exceeded {
            return Err(SyncAbort::Remote(RemoteError::QuotaExceeded));
        }
        Ok(ledger.report)
    }

    /// 一侧整份清单为空、基线里却有不止一个活文件：多半是卷没挂好、库被换成空目录或服务端
    /// 出了事，而不是用户真把它们全删了——不把空的一侧当成删除来源。用户确认过（与批量删除
    /// 熔断同一个确认）才放行；只剩一个文件时删掉它是平常事，不拦。
    fn empty_side_is_suspicious(&self, live_bases: usize) -> bool {
        live_bases >= 2 && !self.config.allow_bulk_delete
    }

    // ─── 拉取 ─────────────────────────────────────────────────────────────────

    async fn pull(
        &self,
        state: &mut SyncState,
        vault: Uuid,
        bases: &[BaseFile],
    ) -> Result<Pulled, SyncAbort> {
        if let Some(cursor) = state.cursor()? {
            match self.pull_changes(vault, cursor).await {
                Err(SyncAbort::Remote(RemoteError::EpochChanged | RemoteError::CursorExpired)) => {
                    state.forget_cursor()?;
                }
                other => return other,
            }
        }
        self.pull_manifest(vault, bases).await
    }

    async fn pull_changes(&self, vault: Uuid, cursor: Cursor) -> Result<Pulled, SyncAbort> {
        let mut latest: HashMap<Uuid, RemoteEntry> = HashMap::new();
        let mut since = cursor.seq;
        let epoch = loop {
            let page = self
                .remote
                .changes(vault, cursor.epoch, since, PAGE)
                .await?;
            for entry in page.entries {
                latest.insert(entry.file_id, entry);
            }
            since = page.next_seq;
            if !page.has_more {
                break page.epoch;
            }
        };
        let mut remote = HashMap::with_capacity(latest.len());
        for (file_id, entry) in latest {
            remote.insert(file_id, self.remote_side(&entry)?);
        }
        Ok(Pulled {
            remote,
            cursor: Cursor { epoch, seq: since },
            full: false,
        })
    }

    async fn pull_manifest(&self, vault: Uuid, bases: &[BaseFile]) -> Result<Pulled, SyncAbort> {
        let manifest = self.remote.manifest(vault, None).await?;
        let live_bases = bases.iter().filter(|b| !b.deleted).count();
        if manifest.entries.is_empty() && self.empty_side_is_suspicious(live_bases) {
            return Err(SyncAbort::RemoteLooksEmpty);
        }
        let mut remote = HashMap::with_capacity(manifest.entries.len());
        for entry in &manifest.entries {
            remote.insert(entry.file_id, self.remote_side(entry)?);
        }
        // 清单只列活文件：基线里有、清单里没有 = 云端删了（墓碑可能已被裁掉，服务端没有记录）。
        for base in bases.iter().filter(|b| !b.deleted) {
            remote.entry(base.file_id).or_insert(RemoteSide {
                rev: None,
                deleted: true,
                entry: None,
            });
        }
        Ok(Pulled {
            remote,
            cursor: Cursor {
                epoch: manifest.epoch,
                seq: manifest.at_seq,
            },
            full: true,
        })
    }

    fn remote_side(&self, entry: &RemoteEntry) -> Result<RemoteSide, SyncAbort> {
        if entry.deleted {
            return Ok(RemoteSide {
                rev: Some(entry.rev),
                deleted: true,
                entry: None,
            });
        }
        let ct = entry
            .entry_ct
            .as_deref()
            .ok_or_else(|| SyncAbort::Corrupt("live entry without ciphertext".into()))?;
        let plain = open_entry(self.ring, entry.file_id, entry.rev, ct).map_err(|e| match e {
            ManifestError::Crypto(CryptoError::KeyVersion { .. }) => SyncAbort::KeysMissing,
            other => SyncAbort::Corrupt(other.to_string()),
        })?;
        Ok(RemoteSide {
            rev: Some(entry.rev),
            deleted: false,
            entry: Some(plain),
        })
    }

    // ─── 三态 ─────────────────────────────────────────────────────────────────

    fn views(
        &self,
        bases: &[BaseFile],
        scanned: Vec<LocalFile>,
        remote: &HashMap<Uuid, RemoteSide>,
    ) -> Vec<FileView> {
        let mut local: HashMap<String, LocalFile> = scanned
            .into_iter()
            .map(|f| (path_key(&f.path), f))
            .collect();
        let mut views: Vec<FileView> = Vec::new();
        let mut seen: HashSet<Uuid> = HashSet::new();

        // 基线里的活文件：按路径认回本地文件；扫不到的交给 absence 判定。
        let mut deleted_by_hash: HashMap<String, Vec<usize>> = HashMap::new();
        for base in bases {
            seen.insert(base.file_id);
            let side = if base.deleted {
                LocalSide::Absent
            } else if let Some(file) = local.remove(&path_key(&base.path)) {
                LocalSide::Present(file)
            } else if base.cloud_only {
                // 仅云端：本机本来就没有，不是删除。
                LocalSide::Absent
            } else {
                match self.local.absence(&base.path) {
                    luna_vault::absence::Absence::Deleted => {
                        deleted_by_hash
                            .entry(base.content_hash.clone())
                            .or_default()
                            .push(views.len());
                        LocalSide::Deleted
                    }
                    // 扫描与判定之间刚出现：交给下一轮。
                    luna_vault::absence::Absence::Present
                    | luna_vault::absence::Absence::Unknown(_) => LocalSide::Unknown,
                }
            };
            views.push(FileView {
                file_id: base.file_id,
                base: Some(base.clone()),
                local: side,
                remote: remote.get(&base.file_id).cloned(),
            });
        }

        // 改名：一个「删除」与一个新出现的文件内容完全相同（且一一对应）→ 同一个文件挪了位置。
        let mut new_by_hash: HashMap<String, Vec<String>> = HashMap::new();
        for (key, file) in local.iter().filter(|(_, file)| !file.settling) {
            new_by_hash
                .entry(file.content_hash.clone())
                .or_default()
                .push(key.clone());
        }
        for (hash, indices) in &deleted_by_hash {
            if let ([index], Some([key])) =
                (indices.as_slice(), new_by_hash.get(hash).map(Vec::as_slice))
            {
                if let Some(file) = local.remove(key) {
                    views[*index].local = LocalSide::Present(file);
                }
            }
        }

        // 云端新来的文件：同一路径本地也新建了一个 → 同一个文件的两端（没有基线）。
        for (file_id, side) in remote {
            if seen.contains(file_id) {
                continue;
            }
            seen.insert(*file_id);
            let local_side = side
                .entry
                .as_ref()
                .and_then(|entry| local.remove(&path_key(&entry.path)))
                .map_or(LocalSide::Absent, LocalSide::Present);
            views.push(FileView {
                file_id: *file_id,
                base: None,
                local: local_side,
                remote: Some(side.clone()),
            });
        }

        // 剩下的本地文件都是新文件。
        let mut fresh: Vec<LocalFile> = local.into_values().collect();
        fresh.sort_by(|a, b| a.path.cmp(&b.path));
        for file in fresh {
            views.push(FileView {
                file_id: Uuid::now_v7(),
                base: None,
                local: LocalSide::Present(file),
                remote: None,
            });
        }
        views
    }

    /// 按需下载（ADR-156）：把一个仅云端的文件取到本机（打开录音时）。大文件逐块进块缓存，
    /// 中断了下次接着下。调用方须与同步轮互斥（同一个状态库）。
    pub async fn materialize(
        &self,
        state: &mut SyncState,
        path: &str,
        progress: &mut (dyn FnMut(u64, u64) + Send),
    ) -> Result<OnDemand, SyncAbort> {
        let binding = state.binding()?.ok_or(SyncAbort::NotBound)?;
        let vault = binding.remote_vault_id;
        let Some(base) = live_base(state, path)? else {
            return Ok(OnDemand::NotFound);
        };
        if !base.cloud_only {
            return Ok(OnDemand::AlreadyThere);
        }
        let remote = match self.remote.entry_at(vault, base.file_id, base.rev).await {
            Ok(remote) => remote,
            Err(RemoteError::NotFound) => return Ok(OnDemand::NotFound),
            Err(err) => return Err(err.into()),
        };
        let Some(ct) = remote.entry_ct.as_deref() else {
            return Ok(OnDemand::NotFound);
        };
        let entry = match open_entry(self.ring, base.file_id, base.rev, ct) {
            Ok(entry) => entry,
            Err(ManifestError::Crypto(CryptoError::KeyVersion { .. })) => {
                return Err(SyncAbort::KeysMissing)
            }
            Err(_) => return Ok(OnDemand::Failed),
        };
        let Some(body) = self.download_with(vault, &entry, progress).await? else {
            return Ok(OnDemand::Failed);
        };
        match self.write_body(&base.path, &body, &Expect::Absent, &entry.chunks)? {
            WriteOutcome::Done => {
                state.commit_round(
                    &[BaseChange::Upsert(BaseFile {
                        cloud_only: false,
                        ..base
                    })],
                    None,
                )?;
                Ok(OnDemand::Done)
            }
            WriteOutcome::Changed => {
                self.release(&body);
                Ok(OnDemand::Changed)
            }
        }
    }

    /// 释放本机空间（ADR-156）：本机这份与同步基线一致（云端确有这一版）时移除本机副本，基线
    /// 标成仅云端。本机改过还没同步的不动。
    pub fn evict(&self, state: &mut SyncState, path: &str) -> Result<OnDemand, SyncAbort> {
        let Some(base) = live_base(state, path)? else {
            return Ok(OnDemand::NotFound);
        };
        if base.cloud_only {
            return Ok(OnDemand::AlreadyThere);
        }
        match self.local.evict_if(&base.path, &base.content_hash)? {
            WriteOutcome::Done => {
                state.commit_round(
                    &[BaseChange::Upsert(BaseFile {
                        cloud_only: true,
                        ..base
                    })],
                    None,
                )?;
                Ok(OnDemand::Done)
            }
            WriteOutcome::Changed => Ok(OnDemand::Changed),
        }
    }

    /// 选择性同步（ADR-156）：仅云端的文件与按需的录音怎么处置。
    fn selective(&self, view: &FileView) -> Selective {
        use crate::policy::{is_media, RecordingsMode};
        let mode = self.config.recordings;
        let live = view
            .remote
            .as_ref()
            .filter(|r| !r.deleted)
            .and_then(|r| Some((r.entry.as_ref()?, r.rev?)));
        // ① 仅云端、本机没有：云端删了记墓碑；改了只更新基线（全部下载档才下）；没动就不碰。
        if let (Some(base), LocalSide::Absent) = (&view.base, &view.local) {
            if base.cloud_only && !base.deleted {
                return match (&view.remote, live) {
                    (None, _) => Selective::Skip,
                    (Some(_), None) => Selective::Record(tombstone(view)),
                    (Some(_), Some((entry, _))) if mode == RecordingsMode::Full => {
                        if self.config.defer_large
                            && crate::policy::is_large(&entry.path, entry.size)
                        {
                            Selective::Defer
                        } else {
                            Selective::Download
                        }
                    }
                    (Some(_), Some((entry, rev))) => {
                        Selective::Record(BaseChange::Upsert(cloud_only_base(view, entry, rev)))
                    }
                };
            }
        }
        // 仅云端的文件本机又有了内容相同的一份（在别处取来的）：转成本机有副本。内容不同的
        // 照常决策（本机改过 → 推），落下的基线自然是本机有副本。
        if let (Some(base), LocalSide::Present(file)) = (&view.base, &view.local) {
            if base.cloud_only
                && !base.deleted
                && view.remote.is_none()
                && file.content_hash == base.content_hash
            {
                return Selective::Record(BaseChange::Upsert(BaseFile {
                    cloud_only: false,
                    ..base.clone()
                }));
            }
        }
        // ② 云端新来的录音：按需 / 不同步档只登记为仅云端，不下载。
        if view.base.is_none() && matches!(view.local, LocalSide::Absent) {
            if let Some((entry, rev)) = live {
                if is_media(&entry.path) && mode != RecordingsMode::Full {
                    return Selective::Record(BaseChange::Upsert(cloud_only_base(
                        view, entry, rev,
                    )));
                }
            }
        }
        // ③ 不同步录音：本机新录的不上传，留在本机。
        if mode == RecordingsMode::Off && view.base.is_none() && view.remote.is_none() {
            if let LocalSide::Present(file) = &view.local {
                if is_media(&file.path) {
                    return Selective::Skip;
                }
            }
        }
        Selective::Decide
    }

    // ─── 执行 ─────────────────────────────────────────────────────────────────

    async fn execute(
        &self,
        vault: Uuid,
        view: FileView,
        action: Action,
        note: Note,
        ledger: &mut Ledger,
        taken: &mut Claims,
    ) -> Result<(), SyncAbort> {
        let old_pin = view.base.as_ref().and_then(|b| b.pin.clone());
        match action {
            Action::Nothing => {}
            Action::Defer => self.defer(&view, ledger),
            Action::Push { base_rev } => {
                let LocalSide::Present(file) = &view.local else {
                    self.defer(&view, ledger);
                    return Ok(());
                };
                // 本地新文件的路径这一轮已被别的文件认领（典型：上一轮中止时留下的冲突副本，
                // 本轮由云端那份认回）：不再当新文件推，否则两个 file_id 落在同一路径上。
                // 新文件排在最后执行（`views` 末尾、排序稳定），认领一定先发生。
                if view.base.is_none() && taken.held_by_other(&file.path, view.file_id) {
                    self.defer(&view, ledger);
                    return Ok(());
                }
                let Some(body) = self.read_body(file)? else {
                    self.defer(&view, ledger);
                    return Ok(());
                };
                if note == Note::ResurrectedLocalEdit {
                    ledger
                        .activity
                        .push(("resurrected_local_edit", Some(view.file_id)));
                }
                ledger.pushes.push(PendingPush {
                    file_id: view.file_id,
                    base_rev,
                    kind: PushKind::Put {
                        path: file.path.clone(),
                        body,
                        mtime_ms: file.mtime_ms,
                    },
                    old_pin,
                    replaces: None,
                });
            }
            Action::PushAsNew => {
                let LocalSide::Present(file) = &view.local else {
                    self.defer(&view, ledger);
                    return Ok(());
                };
                let Some(body) = self.read_body(file)? else {
                    self.defer(&view, ledger);
                    return Ok(());
                };
                ledger
                    .activity
                    .push(("resurrected_local_edit", Some(view.file_id)));
                ledger.pushes.push(PendingPush {
                    file_id: Uuid::now_v7(),
                    base_rev: None,
                    kind: PushKind::Put {
                        path: file.path.clone(),
                        body,
                        mtime_ms: file.mtime_ms,
                    },
                    old_pin,
                    replaces: Some(view.file_id),
                });
            }
            Action::PushDelete { base_rev } => ledger.pushes.push(PendingPush {
                file_id: view.file_id,
                base_rev: Some(base_rev),
                kind: PushKind::Delete,
                old_pin,
                replaces: None,
            }),
            Action::Apply => self.apply(vault, &view, note, ledger, taken).await?,
            Action::ApplyDelete => {
                let LocalSide::Present(file) = &view.local else {
                    self.defer(&view, ledger);
                    return Ok(());
                };
                match self.local.delete_if(&file.path, &file.content_hash)? {
                    WriteOutcome::Done => {
                        taken.release(&file.path, view.file_id);
                        ledger.changes.push(tombstone(&view));
                        ledger.unpins.extend(old_pin);
                        ledger.report.deleted_local += 1;
                        ledger.activity.push(("deleted_local", Some(view.file_id)));
                    }
                    WriteOutcome::Changed => self.defer(&view, ledger),
                }
            }
            Action::AdvanceBase => {
                if let Some(base) = &view.base {
                    if view.remote.as_ref().is_none_or(|r| r.deleted) {
                        taken.release(&base.path, view.file_id);
                    }
                }
                self.advance_base(&view, ledger)?;
            }
            Action::Merge => self.merge(vault, &view, ledger, taken).await?,
            Action::ConflictCopy => {
                let entry = remote_entry(&view);
                let theirs = match self.download(vault, entry).await? {
                    Some(body) => body,
                    None => {
                        self.defer(&view, ledger);
                        return Ok(());
                    }
                };
                self.conflict_copy(&view, theirs, &entry.chunks, ledger, taken)?;
            }
        }
        Ok(())
    }

    fn defer(&self, view: &FileView, ledger: &mut Ledger) {
        ledger.report.deferred += 1;
        if view.remote.is_some() {
            ledger.remote_incomplete = true;
        }
    }

    /// 读本地内容，并确认它还是扫描时那一份（不是就 `None`：用户刚改过，下一轮再说）。
    fn read_unchanged(&self, file: &LocalFile) -> Result<Option<Vec<u8>>, SyncAbort> {
        let bytes = self.local.read(&file.path)?;
        Ok((content_hash(&bytes) == file.content_hash).then_some(bytes))
    }

    /// 要推的本地内容：小文件整份读出并核对；大文件不读，推送时流式读、读完再核对。
    fn read_body(&self, file: &LocalFile) -> Result<Option<Body>, SyncAbort> {
        if file.size > WHOLE_FILE_MAX as u64 {
            return Ok(Some(Body::OnDisk {
                path: file.path.clone(),
                hash: file.content_hash.clone(),
                size: file.size,
            }));
        }
        Ok(self.read_unchanged(file)?.map(Body::Bytes))
    }

    /// 本机某个路径上此刻的内容哈希（流式；不在 = `None`）。
    fn local_hash(&self, path: &str) -> Option<String> {
        let reader = self.local.open(path).ok()?;
        luna_sync_core::hashing::ContentHash::of_reader(reader)
            .ok()
            .map(|h| h.as_str().to_owned())
    }

    /// 取云端版本的内容。块解不开或拼出来对不上 → `None`（这个文件这一轮不动，记一笔）；
    /// 网络失败 → 整轮中止（大文件已下好的块留在本机块缓存，下一轮接着下）。
    async fn download(&self, vault: Uuid, entry: &EntryPlain) -> Result<Option<Body>, SyncAbort> {
        self.download_with(vault, entry, &mut |_, _| {}).await
    }

    /// 同 [`Self::download`]，大文件每取好一块报一次进度（已取的块数，总块数）。
    async fn download_with(
        &self,
        vault: Uuid,
        entry: &EntryPlain,
        progress: &mut (dyn FnMut(u64, u64) + Send),
    ) -> Result<Option<Body>, SyncAbort> {
        if entry.size > WHOLE_FILE_MAX as u64 {
            return self.download_large(vault, entry, progress).await;
        }
        let mut blobs: HashMap<String, Vec<u8>> = HashMap::new();
        for id in &entry.chunks {
            if !blobs.contains_key(id) {
                let sealed = self.remote.get_blob(vault, id).await?;
                blobs.insert(id.clone(), sealed);
            }
        }
        match assemble(self.ring, entry, |id| {
            blobs.get(id).cloned().ok_or_else(|| "missing".into())
        }) {
            Ok(bytes) => Ok(Some(Body::Bytes(bytes))),
            Err(ManifestError::Crypto(CryptoError::KeyVersion { .. })) => {
                Err(SyncAbort::KeysMissing)
            }
            Err(_) => Ok(None),
        }
    }

    /// 大文件：逐块下载、解密进本机块缓存（已在缓存里的跳过——断网续传就靠它），再按清单顺序
    /// 拼进一份暂存文件，核对大小与哈希。内存里同时只有一块。
    async fn download_large(
        &self,
        vault: Uuid,
        entry: &EntryPlain,
        progress: &mut (dyn FnMut(u64, u64) + Send),
    ) -> Result<Option<Body>, SyncAbort> {
        let total = entry.chunks.len() as u64;
        let mut fetched: HashSet<&str> = HashSet::new();
        for (done, id) in entry.chunks.iter().enumerate() {
            progress(done as u64, total);
            if !fetched.insert(id) || self.local.cached_chunk(id).is_some() {
                continue;
            }
            let sealed = self.remote.get_blob(vault, id).await?;
            let opened = self
                .ring
                .for_ciphertext(&sealed)
                .and_then(|keys| keys.open_chunk(id, &sealed));
            let padded = match opened {
                Ok(padded) => padded,
                Err(CryptoError::KeyVersion { .. }) => return Err(SyncAbort::KeysMissing),
                Err(_) => return Ok(None),
            };
            let Ok(plain) = unpad(&padded) else {
                return Ok(None);
            };
            self.local.cache_chunk(id, plain)?;
        }
        progress(total, total);
        let staged = self.local.stage(&mut |out| {
            for id in &entry.chunks {
                let plain = self
                    .local
                    .cached_chunk(id)
                    .ok_or_else(|| LocalError("chunk_cache_miss".into()))?;
                out.write_all(&plain)
                    .map_err(|e| LocalError(format!("stage:{:?}", e.kind())))?;
            }
            Ok(())
        })?;
        if staged.size != entry.size || staged.content_hash != entry.content_hash {
            self.local.discard(&staged);
            // 缓存里有块坏了：清掉，下一轮重新下。
            self.local.forget_chunks(&entry.chunks);
            return Ok(None);
        }
        Ok(Some(Body::Staged(staged)))
    }

    /// 把一份内容写到 `path`（现状符合 `expect` 时）。暂存写成功即用掉，块缓存随之清掉。
    fn write_body(
        &self,
        path: &str,
        body: &Body,
        expect: &Expect,
        chunks: &[String],
    ) -> Result<WriteOutcome, SyncAbort> {
        let outcome = match body {
            Body::Bytes(bytes) => self.local.write_if(path, bytes, expect)?,
            Body::Staged(staged) => {
                let outcome = self.local.write_staged_if(path, staged, expect)?;
                if outcome == WriteOutcome::Done {
                    self.local.forget_chunks(chunks);
                }
                outcome
            }
            Body::OnDisk { .. } => return Err(LocalError("on_disk_body_written".into()).into()),
        };
        Ok(outcome)
    }

    /// 一份没写出去的暂存不留在库里。
    fn release(&self, body: &Body) {
        if let Body::Staged(staged) = body {
            self.local.discard(staged);
        }
    }

    /// 钉基线要内容：小文件就在手上，大的（极少见的超长笔记）再读一次。
    fn body_bytes(&self, body: &Body, path: &str) -> Result<Vec<u8>, SyncAbort> {
        match body {
            Body::Bytes(bytes) => Ok(bytes.clone()),
            Body::OnDisk { path: src, .. } => Ok(self.local.read(src)?),
            Body::Staged(_) => Ok(self.local.read(path)?),
        }
    }

    /// 大文件推送的第一遍：流式切块算出条目。读不到或内容已不是扫描时那一份 → `None`（下一轮再推）。
    fn prepare_on_disk(
        &self,
        keys: &luna_sync_core::crypto::VaultKeys,
        path: &str,
        src: &str,
        hash: &str,
        size: u64,
        mtime_ms: Option<i64>,
    ) -> Result<Option<luna_sync_core::manifest::PreparedFile>, SyncAbort> {
        let Ok(reader) = self.local.open(src) else {
            return Ok(None);
        };
        Ok(
            luna_sync_core::manifest::prepare_streamed(keys, path, reader, size, mtime_ms)
                .ok()
                .filter(|file| file.entry.content_hash == hash),
        )
    }

    /// 大文件推送的第二遍：再流式读一次，只把服务端缺的块加密上传（逐块，内存里同时只有一块）。
    /// 读完核对内容哈希；途中被改了 → `false`（已传的块无害，服务端 7 天后回收没被引用的块）。
    async fn upload_on_disk(
        &self,
        vault: Uuid,
        keys: &luna_sync_core::crypto::VaultKeys,
        src: &str,
        hash: &str,
        size: u64,
        missing: &mut HashSet<String>,
    ) -> Result<bool, SyncAbort> {
        use sha2::Digest as _;
        let Ok(reader) = self.local.open(src) else {
            return Ok(false);
        };
        let mut hasher = sha2::Sha256::new();
        for chunk in stream_chunks(reader, size) {
            let Ok(chunk) = chunk else {
                return Ok(false);
            };
            hasher.update(&chunk);
            let padded = pad(&chunk);
            let id = keys.chunk_id(&padded);
            if missing.remove(&id) {
                let sealed = keys.seal_chunk(&id, &padded);
                let sha: [u8; 32] = sha2::Sha256::digest(&sealed).into();
                self.remote.put_blob(vault, &id, &sealed, sha).await?;
            }
        }
        let digest = hasher.finalize();
        let got: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        Ok(got == hash)
    }

    async fn apply(
        &self,
        vault: Uuid,
        view: &FileView,
        note: Note,
        ledger: &mut Ledger,
        taken: &mut Claims,
    ) -> Result<(), SyncAbort> {
        let entry = remote_entry(view);
        let Some(body) = self.download(vault, entry).await? else {
            ledger
                .activity
                .push(("download_failed", Some(view.file_id)));
            self.defer(view, ledger);
            return Ok(());
        };
        let mut target = entry.path.clone();
        let outcome = match &view.local {
            LocalSide::Present(file) => {
                let mut moved = WriteOutcome::Done;
                if file.path != target
                    && (taken.held_by_other(&target, view.file_id)
                        || self
                            .local
                            .rename_if(&file.path, &target, &file.content_hash)?
                            == WriteOutcome::Changed)
                {
                    // 云端改名的目标被一个不相干的本地文件占着：挪到冲突副本名下（同「新建撞名」）。
                    // 源文件这一轮刚被改过时，挪副本也会失败，留给下一轮。
                    moved = match self.move_to_copy(
                        &file.path,
                        &file.content_hash,
                        &target,
                        view.file_id,
                        taken,
                    )? {
                        Some(copy) => {
                            target = copy;
                            ledger.report.conflict_copies += 1;
                            ledger.activity.push(("conflict_copy", Some(view.file_id)));
                            WriteOutcome::Done
                        }
                        None => WriteOutcome::Changed,
                    };
                }
                if moved == WriteOutcome::Changed {
                    WriteOutcome::Changed
                } else {
                    if file.path != target {
                        taken.release(&file.path, view.file_id);
                    }
                    self.write_body(
                        &target,
                        &body,
                        &Expect::Hash(file.content_hash.clone()),
                        &entry.chunks,
                    )?
                }
            }
            _ => match if taken.held_by_other(&target, view.file_id) {
                WriteOutcome::Changed
            } else {
                self.write_body(&target, &body, &Expect::Absent, &entry.chunks)?
            } {
                // 那个位置被一个不相干的文件占着（磁盘上，或另一个基线）：云端版本另存为冲突副本。
                WriteOutcome::Changed => {
                    match self.write_copy(&entry.path, &body, &entry.chunks, view.file_id, taken)? {
                        Some(copy) => {
                            target = copy;
                            ledger.report.conflict_copies += 1;
                            ledger.activity.push(("conflict_copy", Some(view.file_id)));
                            WriteOutcome::Done
                        }
                        None => WriteOutcome::Changed,
                    }
                }
                done => done,
            },
        };
        if outcome == WriteOutcome::Changed {
            self.release(&body);
            self.defer(view, ledger);
            return Ok(());
        }
        let body = body.landed_at(&target);
        taken.claim(&target, view.file_id);
        let pin = if is_markdown(&target) {
            Some(self.local.pin(&self.body_bytes(&body, &target)?)?)
        } else {
            None
        };
        ledger.changes.push(BaseChange::Upsert(BaseFile {
            file_id: view.file_id,
            path: target.clone(),
            rev: view.remote.as_ref().and_then(|r| r.rev).unwrap_or(0),
            content_hash: entry.content_hash.clone(),
            pin,
            deleted: false,
            cloud_only: false,
        }));
        // 冲突副本落在别的路径上时，基线记的是副本那个路径——云端那份文件此后在本机就是它。
        if target != entry.path {
            ledger.pushes.push(PendingPush {
                file_id: view.file_id,
                base_rev: view.remote.as_ref().and_then(|r| r.rev),
                kind: PushKind::Put {
                    path: target,
                    body,
                    mtime_ms: entry.mtime_ms,
                },
                old_pin: None,
                replaces: None,
            });
        }
        ledger
            .unpins
            .extend(view.base.as_ref().and_then(|b| b.pin.clone()));
        ledger.report.applied += 1;
        if note == Note::RestoredRemoteEdit {
            ledger
                .activity
                .push(("restored_remote_edit", Some(view.file_id)));
        }
        Ok(())
    }

    fn advance_base(&self, view: &FileView, ledger: &mut Ledger) -> Result<(), SyncAbort> {
        let remote = view.remote.as_ref();
        if remote.is_none_or(|r| r.deleted) {
            ledger.changes.push(tombstone(view));
            ledger
                .unpins
                .extend(view.base.as_ref().and_then(|b| b.pin.clone()));
            return Ok(());
        }
        let LocalSide::Present(file) = &view.local else {
            return Ok(());
        };
        let pin = if is_markdown(&file.path) {
            match self.read_unchanged(file)? {
                Some(bytes) => Some(self.local.pin(&bytes)?),
                None => {
                    self.defer(view, ledger);
                    return Ok(());
                }
            }
        } else {
            None
        };
        ledger.changes.push(BaseChange::Upsert(BaseFile {
            file_id: view.file_id,
            path: file.path.clone(),
            rev: remote.and_then(|r| r.rev).unwrap_or(0),
            content_hash: file.content_hash.clone(),
            pin,
            deleted: false,
            cloud_only: false,
        }));
        ledger
            .unpins
            .extend(view.base.as_ref().and_then(|b| b.pin.clone()));
        Ok(())
    }

    async fn merge(
        &self,
        vault: Uuid,
        view: &FileView,
        ledger: &mut Ledger,
        taken: &mut Claims,
    ) -> Result<(), SyncAbort> {
        let LocalSide::Present(file) = &view.local else {
            self.defer(view, ledger);
            return Ok(());
        };
        let entry = remote_entry(view);
        let Some(theirs) = self.download(vault, entry).await? else {
            self.defer(view, ledger);
            return Ok(());
        };
        // 合并只对整份在手的文本有意义；超大的那一份（暂存）直接走冲突副本。
        let Body::Bytes(theirs) = theirs else {
            return self.conflict_copy(view, theirs, &entry.chunks, ledger, taken);
        };
        let Some(ours) = self.read_unchanged(file)? else {
            self.defer(view, ledger);
            return Ok(());
        };
        let base = view
            .base
            .as_ref()
            .and_then(|b| b.pin.as_deref())
            .and_then(|pin| self.local.read_pinned(pin).ok());
        let merged = match (
            base.as_deref().map(std::str::from_utf8),
            std::str::from_utf8(&ours),
            std::str::from_utf8(&theirs),
        ) {
            (Some(Ok(base)), Ok(ours), Ok(theirs)) => {
                match luna_sync_core::merge3::merge3(base, ours, theirs) {
                    luna_sync_core::merge3::MergeOutcome::Merged(text) => Some(text),
                    _ => None,
                }
            }
            _ => None,
        };
        let Some(text) = merged else {
            return self.conflict_copy(view, Body::Bytes(theirs), &entry.chunks, ledger, taken);
        };
        // 合并结果覆盖本地之前，给本地那份（还没提交的编辑）留个版本：合并可撤回。
        self.local.keep_version(&file.path, &ours)?;
        match self.local.write_if(
            &file.path,
            text.as_bytes(),
            &Expect::Hash(file.content_hash.clone()),
        )? {
            WriteOutcome::Done => {
                ledger.report.merged += 1;
                ledger.activity.push(("merged", Some(view.file_id)));
                ledger.pushes.push(PendingPush {
                    file_id: view.file_id,
                    base_rev: view.remote.as_ref().and_then(|r| r.rev),
                    kind: PushKind::Put {
                        path: file.path.clone(),
                        body: Body::Bytes(text.into_bytes()),
                        mtime_ms: file.mtime_ms,
                    },
                    old_pin: view.base.as_ref().and_then(|b| b.pin.clone()),
                    replaces: None,
                });
            }
            WriteOutcome::Changed => self.defer(view, ledger),
        }
        Ok(())
    }

    /// 云端版本另存为冲突副本（作为一个新文件推上去），本地版本照常推上去覆盖云端。
    fn conflict_copy(
        &self,
        view: &FileView,
        theirs: Body,
        chunks: &[String],
        ledger: &mut Ledger,
        taken: &mut Claims,
    ) -> Result<(), SyncAbort> {
        let LocalSide::Present(file) = &view.local else {
            self.release(&theirs);
            self.defer(view, ledger);
            return Ok(());
        };
        let Some(ours) = self.read_body(file)? else {
            self.release(&theirs);
            self.defer(view, ledger);
            return Ok(());
        };
        let copy_id = Uuid::now_v7();
        let Some(copy) = self.write_copy(&file.path, &theirs, chunks, copy_id, taken)? else {
            self.release(&theirs);
            self.defer(view, ledger);
            return Ok(());
        };
        ledger.report.conflict_copies += 1;
        ledger.activity.push(("conflict_copy", Some(view.file_id)));
        ledger.pushes.push(PendingPush {
            file_id: copy_id,
            base_rev: None,
            kind: PushKind::Put {
                body: theirs.landed_at(&copy),
                path: copy,
                mtime_ms: None,
            },
            old_pin: None,
            replaces: None,
        });
        ledger.pushes.push(PendingPush {
            file_id: view.file_id,
            base_rev: view.remote.as_ref().and_then(|r| r.rev),
            kind: PushKind::Put {
                path: file.path.clone(),
                body: ours,
                mtime_ms: file.mtime_ms,
            },
            old_pin: view.base.as_ref().and_then(|b| b.pin.clone()),
            replaces: None,
        });
        Ok(())
    }

    /// 把一个本地文件挪到冲突副本名下，返回新路径（源文件已变或 [`CONFLICT_COPY_ATTEMPTS`] 个名字都被占 → `None`）。
    fn move_to_copy(
        &self,
        from: &str,
        expected_hash: &str,
        near: &str,
        owner: Uuid,
        taken: &mut Claims,
    ) -> Result<Option<String>, SyncAbort> {
        for attempt in 1..=CONFLICT_COPY_ATTEMPTS {
            let path = conflict_copy_path(
                near,
                &self.config.conflict_label,
                &self.config.today,
                attempt,
            );
            if taken.held(&path) {
                continue;
            }
            if self.local.rename_if(from, &path, expected_hash)? == WriteOutcome::Done {
                taken.claim(&path, owner);
                return Ok(Some(path));
            }
        }
        Ok(None)
    }

    /// 写一个冲突副本，返回它的路径（[`CONFLICT_COPY_ATTEMPTS`] 个名字都被占着 → `None`）。
    ///
    /// 名字上已有**内容相同**的孤立文件就认回它：那是上一轮写下、但那一轮没走完（基线没落盘）的
    /// 同一份副本。只认孤立文件——属于别的视图的文件这一轮还要认领自己的路径（万种子仿真）。否则每次重试都另造一份，20 个名字用完才停（2026-10-11 真机：一个文件
    /// 50 秒里生成了 20 份「冲突副本」）。
    fn write_copy(
        &self,
        near: &str,
        body: &Body,
        chunks: &[String],
        owner: Uuid,
        taken: &mut Claims,
    ) -> Result<Option<String>, SyncAbort> {
        let hash = body.hash();
        for attempt in 1..=CONFLICT_COPY_ATTEMPTS {
            let path = conflict_copy_path(
                near,
                &self.config.conflict_label,
                &self.config.today,
                attempt,
            );
            if taken.held(&path) {
                continue;
            }
            let reusable = match self.write_body(&path, body, &Expect::Absent, chunks)? {
                WriteOutcome::Done => true,
                WriteOutcome::Changed => {
                    let same =
                        taken.is_orphan(&path) && self.local_hash(&path).is_some_and(|h| h == hash);
                    // 认回上一轮写下的同一份副本：这一份暂存就用不上了。
                    if same {
                        self.release(body);
                    }
                    same
                }
            };
            if reusable {
                taken.claim(&path, owner);
                return Ok(Some(path));
            }
        }
        Ok(None)
    }

    // ─── 推送 ─────────────────────────────────────────────────────────────────

    async fn push_all(&self, vault: Uuid, ledger: &mut Ledger) -> Result<(), SyncAbort> {
        let pushes = std::mem::take(&mut ledger.pushes);
        if pushes.is_empty() {
            return Ok(());
        }
        let keys = self.ring.current();

        // 删除先于上传与内容提交，单独成批（ADR-152）：云端存储满了时服务端只放行只含删除的
        // 提交，而块上传一碰到 402 整轮就中止——删除排在后面就永远到不了服务端，用户被锁在
        // 「满了、又删不掉」里。删除先落地也与「先腾后写」的路径占用顺序一致。
        let (deletes, mut puts): (Vec<usize>, Vec<usize>) =
            (0..pushes.len()).partition(|&i| matches!(pushes[i].kind, PushKind::Delete));

        // 切块：小文件整份加密；大文件流式算出块 id（第一遍），读不到或已变的留给下一轮。
        let mut prepared: Vec<Option<luna_sync_core::manifest::PreparedFile>> =
            Vec::with_capacity(pushes.len());
        let mut oversize: HashSet<usize> = HashSet::new();
        for (index, push) in pushes.iter().enumerate() {
            // 超过单文件上限：不切块、不上传（几个 GB 传完才在提交时被拒，是白跑一趟）。
            if let PushKind::Put { path, body, .. } = &push.kind {
                let size = match body {
                    Body::Bytes(bytes) => bytes.len() as u64,
                    Body::OnDisk { size, .. } => *size,
                    Body::Staged(staged) => staged.size,
                };
                if self.config.max_file_bytes.is_some_and(|cap| size > cap) {
                    ledger.report.too_large.push(path.clone());
                    oversize.insert(index);
                    prepared.push(None);
                    continue;
                }
            }
            prepared.push(match &push.kind {
                PushKind::Put {
                    path,
                    body: Body::Bytes(bytes),
                    mtime_ms,
                } => Some(prepare(keys, path, bytes, *mtime_ms)),
                PushKind::Put {
                    path,
                    body:
                        Body::OnDisk {
                            path: src,
                            hash,
                            size,
                        },
                    mtime_ms,
                } => self.prepare_on_disk(keys, path, src, hash, *size, *mtime_ms)?,
                PushKind::Put {
                    body: Body::Staged(_),
                    ..
                }
                | PushKind::Delete => None,
            });
        }
        puts.retain(|&i| {
            let ready = prepared[i].is_some();
            if !ready && !oversize.contains(&i) {
                ledger.report.deferred += 1;
            }
            ready
        });
        // 一个条目引用的块数有上限（服务端同一常量）：超了的文件不上传、单独记下，而不是让提交被拒、
        // 整轮中止——那样一个超长录音会把整个库的同步卡死在每一轮。
        puts.retain(|&i| {
            let fits = prepared[i]
                .as_ref()
                .is_none_or(|file| file.entry.chunks.len() <= self.config.max_chunks_per_entry);
            if !fits {
                if let PushKind::Put { path, .. } = &pushes[i].kind {
                    ledger.report.too_large.push(path.clone());
                }
            }
            fits
        });
        let mut sealed: HashMap<String, (&[u8], [u8; 32])> = HashMap::new();
        for file in puts.iter().filter_map(|&i| prepared[i].as_ref()) {
            for chunk in &file.chunks {
                sealed
                    .entry(chunk.id.clone())
                    .or_insert((&chunk.sealed, chunk.ct_sha256));
            }
        }
        // 提交（分批）。墓碑已被服务端裁掉时（基线过期且服务端没有这个文件），按「服务端没有」
        // 重试一次——否则这个文件会每一轮都卡在同一个过期基线上。
        let mut retry: Vec<usize> = Vec::new();
        for batch in deletes.chunks(COMMIT_BATCH) {
            self.commit_batch(
                vault,
                batch,
                &pushes,
                &prepared,
                |_| None,
                ledger,
                &mut retry,
            )
            .await?;
        }

        let waves = upload_waves(
            puts.iter()
                .filter_map(|&i| {
                    let entry = &prepared[i].as_ref()?.entry;
                    Some((i, is_markdown(&entry.path), entry.size))
                })
                .collect(),
        );
        for wave in &waves {
            self.push_wave(vault, wave, &pushes, &prepared, &sealed, ledger, &mut retry)
                .await?;
        }
        if !retry.is_empty() {
            let mut again = Vec::new();
            self.commit_batch(
                vault,
                &retry,
                &pushes,
                &prepared,
                |_| Some(None),
                ledger,
                &mut again,
            )
            .await?;
            ledger.report.deferred += again.len();
        }
        Ok(())
    }

    /// 传一波并提交（[`upload_waves`]）：问服务端缺哪些块，小文件的缺块直接补传，大文件流式补传，
    /// 然后分批提交。
    #[allow(clippy::too_many_arguments)]
    async fn push_wave(
        &self,
        vault: Uuid,
        wave: &[usize],
        pushes: &[PendingPush],
        prepared: &[Option<luna_sync_core::manifest::PreparedFile>],
        sealed: &HashMap<String, (&[u8], [u8; 32])>,
        ledger: &mut Ledger,
        retry: &mut Vec<usize>,
    ) -> Result<(), SyncAbort> {
        let keys = self.ring.current();
        let mut wanted: Vec<String> = Vec::new();
        let mut asked: HashSet<&str> = HashSet::new();
        for file in wave.iter().filter_map(|&i| prepared[i].as_ref()) {
            for id in &file.entry.chunks {
                if asked.insert(id) {
                    wanted.push(id.clone());
                }
            }
        }
        let mut missing: HashSet<String> = HashSet::new();
        for batch in wanted.chunks(MISSING_BATCH) {
            missing.extend(self.remote.missing_blobs(vault, batch).await?);
        }
        for id in &wanted {
            if let Some((bytes, sha)) = sealed.get(id) {
                if missing.remove(id) {
                    self.remote.put_blob(vault, id, bytes, *sha).await?;
                }
            }
        }
        // 大文件逐个流式补传（断网后下一轮重问缺哪些，已传的不再传）。
        let mut changed: Vec<usize> = Vec::new();
        for &i in wave {
            let PushKind::Put {
                body:
                    Body::OnDisk {
                        path: src,
                        hash,
                        size,
                    },
                ..
            } = &pushes[i].kind
            else {
                continue;
            };
            let needs = prepared[i]
                .as_ref()
                .is_some_and(|file| file.entry.chunks.iter().any(|id| missing.contains(id)));
            if needs
                && !self
                    .upload_on_disk(vault, keys, src, hash, *size, &mut missing)
                    .await?
            {
                changed.push(i);
            }
        }
        ledger.report.deferred += changed.len();
        let ready: Vec<usize> = wave
            .iter()
            .copied()
            .filter(|i| !changed.contains(i))
            .collect();
        for batch in commit_batches(&ready, prepared) {
            self.commit_batch(vault, &batch, pushes, prepared, |_| None, ledger, retry)
                .await?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn commit_batch(
        &self,
        vault: Uuid,
        batch: &[usize],
        pushes: &[PendingPush],
        prepared: &[Option<luna_sync_core::manifest::PreparedFile>],
        override_base: impl Fn(usize) -> Option<Option<i64>>,
        ledger: &mut Ledger,
        retry: &mut Vec<usize>,
    ) -> Result<(), SyncAbort> {
        let keys = self.ring.current();
        let mut items = Vec::with_capacity(batch.len());
        for &i in batch {
            let push = &pushes[i];
            let base_rev = override_base(i).unwrap_or(push.base_rev);
            items.push(match (&push.kind, &prepared[i]) {
                (PushKind::Put { .. }, Some(file)) => {
                    commit::put(keys, push.file_id, base_rev, file)
                }
                _ => CommitItem {
                    file_id: push.file_id,
                    base_rev,
                    change: CommitChange::Delete,
                },
            });
        }
        let key = Uuid::now_v7().to_string();
        let reply = self.remote.commit(vault, &key, items).await?;
        for (&i, result) in batch.iter().zip(reply.results) {
            let push = &pushes[i];
            match result {
                CommitResult::Committed { rev, .. } => {
                    match (&push.kind, &prepared[i]) {
                        (PushKind::Put { path, body, .. }, Some(file)) => {
                            let pin = if is_markdown(path) {
                                Some(self.local.pin(&self.body_bytes(body, path)?)?)
                            } else {
                                None
                            };
                            ledger.changes.push(BaseChange::Upsert(BaseFile {
                                file_id: push.file_id,
                                path: file.entry.path.clone(),
                                rev,
                                content_hash: file.entry.content_hash.clone(),
                                pin,
                                deleted: false,
                                cloud_only: false,
                            }));
                            ledger.report.pushed += 1;
                        }
                        _ => {
                            ledger.changes.push(BaseChange::Upsert(BaseFile {
                                file_id: push.file_id,
                                path: String::new(),
                                rev,
                                content_hash: String::new(),
                                pin: None,
                                deleted: true,
                                cloud_only: false,
                            }));
                            ledger.report.deleted_remote += 1;
                            ledger.activity.push(("deleted_remote", Some(push.file_id)));
                        }
                    }
                    ledger.unpins.extend(push.old_pin.clone());
                    if let Some(old) = push.replaces {
                        ledger.changes.push(BaseChange::Remove(old));
                    }
                }
                CommitResult::StaleBase {
                    current_rev: None, ..
                } if push.base_rev.is_some() && matches!(push.kind, PushKind::Put { .. }) => {
                    retry.push(i);
                }
                CommitResult::StaleBase { .. } | CommitResult::MissingBlobs { .. } => {
                    ledger.report.deferred += 1;
                }
                // 服务端按引用块的密文字节判超了单文件上限（客户端没取到上限、或刚被调低）。
                CommitResult::TooLarge { .. } => {
                    if let PushKind::Put { path, .. } = &push.kind {
                        ledger.report.too_large.push(path.clone());
                    }
                }
            }
        }
        Ok(())
    }
}

fn remote_entry(view: &FileView) -> &EntryPlain {
    view.remote
        .as_ref()
        .and_then(|r| r.entry.as_ref())
        .expect("apply / merge / conflict copy always carry a live remote entry")
}

/// 某个路径上的活基线（按折叠键认）。
fn live_base(state: &SyncState, path: &str) -> Result<Option<BaseFile>, SyncAbort> {
    let key = path_key(path);
    Ok(state
        .bases()?
        .into_iter()
        .find(|b| !b.deleted && path_key(&b.path) == key))
}

/// 这个文件此刻在两边都删了：有服务端 rev 就记墓碑，服务端已没有记录就从基线里移除。
fn tombstone(view: &FileView) -> BaseChange {
    match view.remote.as_ref().and_then(|r| r.rev) {
        Some(rev) => BaseChange::Upsert(BaseFile {
            file_id: view.file_id,
            path: view
                .base
                .as_ref()
                .map(|b| b.path.clone())
                .unwrap_or_default(),
            rev,
            content_hash: String::new(),
            pin: None,
            deleted: true,
            cloud_only: false,
        }),
        None => BaseChange::Remove(view.file_id),
    }
}
