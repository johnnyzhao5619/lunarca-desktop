//! 官方同步的应用层服务（ADR-148；TDD-14 §5）：把 `luna-sync` 的引擎接到打开的库、云客户端与
//! 系统钥匙串上，按节奏跑同步轮次，并把状态送给界面。
//!
//! # 按库选择，选中即同步（ADR-151）
//!
//! 每个选中同步的库一个会话，与它此刻是否打开无关。打开库有两条路径（启动装载、`bind_vault`），
//! 挂一处就会漏一处；这里反过来：调度循环每一轮对着库注册表重新对齐会话，并看此刻打开的是哪一
//! 份库（库身份 + 代号）——打开的库借用应用的句柄，其余选中的库用同步服务自己开的句柄（挂文件
//! 监听）。切库、重开、新建都在下一轮自然生效，不需要任何一条打开路径记得通知同步。
//!
//! # 什么时候跑
//!
//! - 本地有改动（打开的库经统一发 `vault://changed` 的那一处唤醒，后台库经自己的文件监听；
//!   去抖 2 秒）；
//! - 云端有新提交：每个在跑的会话订一条提示流，序号提示（`head`）比这个库的游标新，就只为这个库
//!   排一轮（[`SyncService::head_hint`]；同样去抖 2 秒，一阵提示攒成一轮，与本地改动的唤醒合并）；
//! - 兜底每 60 秒一轮（提示流断着时云端的改动靠它拉到）；
//! - 用户点「立即同步」。
//!
//! 所有轮次都在同一个调度循环里、持同一把会话锁跑，同一个库不会有两轮同时在跑。
//!
//! 严格本地模式、未登录、全部暂停：一个请求都不发；单库暂停只停那一份。
//!
//! # 绑定归账户（ADR-151 修订 2026-10-15）
//!
//! 一份本地库同一时刻只连一个账户（绑定排他）：绑定记着归哪台服务器的哪个账户。只装此刻登录的
//! 账户名下的绑定；连在另一个账户下的库只显示「已连接到另一个账户」，不打开、不发任何请求。
//! 退出登录 = 会话整套丢掉、绑定原样留着；同一账户再登录即接着同步（基线还在）；换账户当场重排、
//! 重发总览（[`SyncService::account_changed`]），途中那一轮作废、不再发布上一个账户的总览。改由
//! 当前账户同步走正常的开启向导，新绑定落下的那一刻才解除原账户的那一份（[`bind_local`]）。
//!
//! # 云端库被删除（ADR-151 修订 2026-10-15，协议 v15）
//!
//! 别的设备（或账户页「云端库」）删除了云端库之后，它的数据面一律回 410 `vault_deleted`，提示流
//! 发一帧终结帧后关流。两条路收在同一处（[`SyncService::drop_deleted`]）：解除这台设备上的绑定
//! （主密钥缓存、基线钉住、同步目录连同块缓存；本地文件一个不碰），通知中心说一次「已在另一台设备
//! 上被删除，这台设备已停止同步它，本地文件都在」——按远端库 id 只说一次；这台设备自己删的不说。
//! 410 不是一次可重试的失败：不进「同步出错，下一轮重试」，也不会每分钟再撞一次。
//!
//! # 本机状态
//!
//! `<应用配置目录>/sync/<库身份>/state.sqlite`：绑定（含归属账户）、基线、游标与单库暂停（可丢
//! 可重建）；同目录 `chunks/` 是块缓存。排他绑定 ⇒ 一个库一份。旧版没有归属的状态启动时删掉
//! （[`remove_legacy_state`]，未发布即硬切）。库主密钥缓存在钥匙串 `lunarca-cloud` 服务下的
//! `sync-cmk/<远端库 id>`；缓存没了（重装、换机、清钥匙串）就从账户托管信封重取（ADR-150 ②），
//! 没有「输密码解锁」这一步。

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use luna_sync::core::LocalFile;
use luna_sync::engine::{Engine, EngineConfig, RoundReport, SyncAbort};
use luna_sync::keystore::CmkStore;
use luna_sync::ports::{Expect, LocalError, LocalVault, Staged, WriteOutcome};
use luna_sync::state::{Binding, BindingOwner, SyncState};
use luna_sync_core::crypto::KeyRing;
use luna_vault::Vault;
use tauri::Manager;
use uuid::Uuid;

use crate::cloud_sync::{managed_keys, CloudRemote, CredentialCmkStore};

type CloudClient = luna_cloud::client::CloudClient;

/// 两次兜底轮次的间隔。
const PERIOD: Duration = Duration::from_secs(60);
/// 本地改动之后等多久再跑（编辑器连续保存时攒成一轮）。
const DEBOUNCE: Duration = Duration::from_secs(2);

/// 钉住基线的持有者名（快照库 `pins/<持有者>/`）。
#[must_use]
pub fn pin_owner(remote_vault_id: Uuid) -> String {
    format!("sync-{remote_vault_id}")
}

/// 一份库的本机同步状态目录。
#[must_use]
pub fn state_dir(config_dir: &Path, local_vault_id: &str) -> PathBuf {
    config_dir.join("sync").join(local_vault_id)
}

/// 内容哈希缓存：路径 → （大小，修改时间，哈希）。大小与修改时间都没变就不重读。
type HashCache = Arc<Mutex<HashMap<String, (u64, Option<i64>, String)>>>;

/// 打开的库 → 同步引擎的 [`LocalVault`]。
///
/// 每个操作都核对库代号：一轮进行中用户切了库，后续操作一律失败（引擎整轮中止、什么都不落账），
/// 绝不把这个库的改动写进另一个库。
pub struct VaultLocal {
    vault: Arc<Mutex<Option<Vault>>>,
    generation: Arc<AtomicU64>,
    expected_generation: u64,
    pin_owner: String,
    hashes: HashCache,
    /// 这一轮改过的路径（路径 → 是否删除），一轮结束后统一发 `vault://changed`。
    touched: Mutex<BTreeMap<String, bool>>,
    /// 这一轮扫到的同步范围总字节（账户页「本地」大小，ADR-152）。
    scanned_bytes: Mutex<Option<u64>>,
    /// 本机块缓存（下载到一半的大文件，跨轮、跨重启续传）：应用配置目录下，不进库——库可能
    /// 就在第三方同步盘里，几百 MB 的临时块会被它白白同步一遍。
    chunk_dir: PathBuf,
}

/// 边写边算哈希与大小（拼暂存文件用：写完不必再读一遍）。
struct HashingWriter<W> {
    inner: W,
    hasher: sha2::Sha256,
    size: u64,
}

impl<W: std::io::Write> std::io::Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        use sha2::Digest as _;
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        self.size += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// 块缓存里超过这么久没动的块清掉（下了一半、之后再也用不上的）。
const CHUNK_CACHE_TTL_MS: i64 = 7 * 24 * 60 * 60 * 1000;

fn prune_chunk_cache(dir: &Path, now: i64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .and_then(|d| i64::try_from(d.as_millis()).ok())
            .is_some_and(|ms| now - ms > CHUNK_CACHE_TTL_MS);
        if stale {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// 块 id 与暂存名都只认我们自己造的形状，绝不拼出一个逃出目录的路径。
fn is_plain_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 80
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn io_error(op: &str, err: &std::io::Error) -> LocalError {
    LocalError(format!(
        "{op}:io:{:?}:{}",
        err.kind(),
        err.raw_os_error().unwrap_or(0)
    ))
}

/// 本地失败只留「哪一步 + 哪一类」，不留路径与文件名（TDD-12）：中止时打进日志，真机才分得清
/// 是读、写还是钉基线出的错、是权限还是磁盘（2026-10-11 真机：只有一个 `sync_local_io` 无从下手）。
fn local_error(op: &str, err: luna_vault::VaultError) -> LocalError {
    use luna_vault::VaultError as E;
    let kind = match &err {
        E::Io(io) => format!("io:{:?}:{}", io.kind(), io.raw_os_error().unwrap_or(0)),
        E::NotFound(_) => "not_found".to_owned(),
        E::PathEscape(_) => "path_escape".to_owned(),
        E::AlreadyExists(_) => "already_exists".to_owned(),
        E::AtomicPublishUnsupported(_) => "atomic_publish_unsupported".to_owned(),
        E::Decode(_) => "decode".to_owned(),
        _ => "other".to_owned(),
    };
    LocalError(format!("{op}:{kind}"))
}

impl VaultLocal {
    fn with<T>(
        &self,
        op: &str,
        f: impl FnOnce(&Vault) -> Result<T, luna_vault::VaultError>,
    ) -> Result<T, LocalError> {
        if self.generation.load(Ordering::SeqCst) != self.expected_generation {
            return Err(LocalError("vault_switched".into()));
        }
        let guard = self
            .vault
            .lock()
            .map_err(|_| LocalError("vault_poisoned".into()))?;
        let vault = guard
            .as_ref()
            .ok_or_else(|| LocalError("no_vault".into()))?;
        f(vault).map_err(|err| local_error(op, err))
    }

    fn touch(&self, path: &str, deleted: bool) {
        if let Ok(mut touched) = self.touched.lock() {
            touched.insert(path.to_owned(), deleted);
        }
    }

    /// 这一轮改过的路径。
    pub fn take_touched(&self) -> Vec<(String, bool)> {
        self.touched
            .lock()
            .map(|mut t| std::mem::take(&mut *t).into_iter().collect())
            .unwrap_or_default()
    }

    /// 把这个库的仅云端清单交给库对象（文件树与链接解析据此，ADR-156），进出清单的路径记为改过、
    /// 让文件树刷新。
    fn refresh_cloud_only(&self, state: &SyncState) {
        let paths: Vec<String> = state
            .bases()
            .map(|bases| {
                bases
                    .into_iter()
                    .filter(|b| b.cloud_only && !b.deleted)
                    .map(|b| b.path)
                    .collect()
            })
            .unwrap_or_default();
        if let Ok(changed) = self.with("cloud_only", |v| Ok(v.set_sync_cloud_only(paths))) {
            for path in changed {
                self.touch(&path, false);
            }
        }
    }

    fn outcome(done: bool) -> WriteOutcome {
        if done {
            WriteOutcome::Done
        } else {
            WriteOutcome::Changed
        }
    }
}

impl LocalVault for VaultLocal {
    fn vault_id(&self) -> Option<String> {
        self.with("id", |v| Ok(v.id().to_string())).ok()
    }

    fn list(&self) -> Result<Vec<LocalFile>, LocalError> {
        // 库锁里只取库根；整库遍历、逐个 stat 与哈希都在锁外。遍历只读目录项，不碰索引，
        // 而同一把锁挡着改名、删除、取目录树——它们跑在主线程上，等锁就是界面卡住
        // （2026-10-13 真机：开同步后改名删除很慢，同步报错时反而不慢）。
        let root = self.with("list", |v| Ok(v.root().to_path_buf()))?;
        let files = luna_vault::sync_scope_files(&root).map_err(|err| local_error("list", err))?;
        let mut cache = self
            .hashes
            .lock()
            .map_err(|_| LocalError("hash_cache_poisoned".into()))?;
        let mut out = Vec::with_capacity(files.len());
        let now = now_ms();
        for file in files {
            // 还在写的大文件（正在录的 WAV）：不读不哈希——每轮整份重读几百 MB 只为得到一个
            // 马上又变的哈希；引擎这一轮也不碰它（ADR-156）。
            if luna_sync::policy::is_settling(&file.rel, file.size, file.modified_ms, now) {
                out.push(LocalFile {
                    path: luna_sync_core::path::nfc(&file.rel).into_owned(),
                    content_hash: String::new(),
                    size: file.size,
                    mtime_ms: file.modified_ms,
                    settling: true,
                });
                continue;
            }
            let hash = match cache.get(&file.rel) {
                Some((size, modified, hash))
                    if *size == file.size && *modified == file.modified_ms =>
                {
                    hash.clone()
                }
                // 流式算：几百 MB 的录音不整份进内存。
                _ => match std::fs::File::open(root.join(&file.rel))
                    .and_then(luna_sync_core::hashing::ContentHash::of_reader)
                {
                    Ok(hash) => {
                        let hash = hash.as_str().to_owned();
                        cache.insert(
                            file.rel.clone(),
                            (file.size, file.modified_ms, hash.clone()),
                        );
                        hash
                    }
                    // 列出来之后又没了：这一轮当它不在。
                    Err(_) => continue,
                },
            };
            out.push(LocalFile {
                path: luna_sync_core::path::nfc(&file.rel).into_owned(),
                content_hash: hash,
                size: file.size,
                mtime_ms: file.modified_ms,
                settling: false,
            });
        }
        if let Ok(mut scanned) = self.scanned_bytes.lock() {
            *scanned = Some(out.iter().map(|f| f.size).sum());
        }
        Ok(out)
    }

    fn read(&self, path: &str) -> Result<Vec<u8>, LocalError> {
        // 锁里只解析路径；整份读（录音动辄几十 MB）在锁外。引擎按清单哈希核对读到的字节。
        let abs = self.with("read", |v| v.sync_path(path))?;
        luna_vault::read_sync_file(&abs, path).map_err(|err| local_error("read", err))
    }

    fn absence(&self, path: &str) -> luna_vault::absence::Absence {
        self.with("absence", |v| Ok(v.sync_absence(path)))
            .unwrap_or(luna_vault::absence::Absence::Unknown(
                luna_vault::absence::UnknownAbsence::RootUnavailable,
            ))
    }

    fn open(&self, path: &str) -> Result<Box<dyn std::io::Read + Send>, LocalError> {
        // 锁里只解析路径，读在锁外（同 `read`）。
        let abs = self.with("open", |v| v.sync_path(path))?;
        let file = std::fs::File::open(&abs).map_err(|e| io_error("open", &e))?;
        Ok(Box::new(std::io::BufReader::new(file)))
    }

    fn stage(
        &self,
        fill: &mut dyn FnMut(&mut dyn std::io::Write) -> Result<(), LocalError>,
    ) -> Result<Staged, LocalError> {
        use sha2::Digest as _;
        let dir = self.with("stage", luna_vault::Vault::sync_stage_dir)?;
        let token = format!("sync-{}", Uuid::now_v7().simple());
        let path = dir.join(&token);
        let file = std::fs::File::create(&path).map_err(|e| io_error("stage", &e))?;
        let mut writer = HashingWriter {
            inner: std::io::BufWriter::new(file),
            hasher: sha2::Sha256::new(),
            size: 0,
        };
        let filled = fill(&mut writer).and_then(|()| {
            use std::io::Write as _;
            writer.flush().map_err(|e| io_error("stage", &e))?;
            writer
                .inner
                .get_ref()
                .sync_all()
                .map_err(|e| io_error("stage", &e))
        });
        if let Err(err) = filled {
            let _ = std::fs::remove_file(&path);
            return Err(err);
        }
        let content_hash: String = writer
            .hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        Ok(Staged {
            token,
            content_hash,
            size: writer.size,
        })
    }

    fn write_staged_if(
        &self,
        path: &str,
        staged: &Staged,
        expect: &Expect,
    ) -> Result<WriteOutcome, LocalError> {
        if !is_plain_name(&staged.token) {
            return Err(LocalError("staged_token".into()));
        }
        let expected = match expect {
            Expect::Absent => None,
            Expect::Hash(hash) => Some(hash.as_str()),
        };
        let done = self.with("write_staged", |v| {
            let tmp = v.sync_stage_dir()?.join(&staged.token);
            v.sync_write_file_if(path, &tmp, expected)
        })?;
        if done {
            self.touch(path, false);
        }
        Ok(Self::outcome(done))
    }

    fn discard(&self, staged: &Staged) {
        if !is_plain_name(&staged.token) {
            return;
        }
        if let Ok(dir) = self.with("discard", luna_vault::Vault::sync_stage_dir) {
            let _ = std::fs::remove_file(dir.join(&staged.token));
        }
    }

    fn cached_chunk(&self, chunk_id: &str) -> Option<Vec<u8>> {
        is_plain_name(chunk_id)
            .then(|| std::fs::read(self.chunk_dir.join(chunk_id)).ok())
            .flatten()
    }

    fn cache_chunk(&self, chunk_id: &str, plain: &[u8]) -> Result<(), LocalError> {
        if !is_plain_name(chunk_id) {
            return Err(LocalError("chunk_id".into()));
        }
        std::fs::create_dir_all(&self.chunk_dir).map_err(|e| io_error("cache_chunk", &e))?;
        luna_vault::atomic::write_atomic(&self.chunk_dir.join(chunk_id), plain)
            .map_err(|err| local_error("cache_chunk", err))
    }

    fn forget_chunks(&self, chunk_ids: &[String]) {
        for id in chunk_ids.iter().filter(|id| is_plain_name(id)) {
            let _ = std::fs::remove_file(self.chunk_dir.join(id));
        }
    }

    fn write_if(
        &self,
        path: &str,
        bytes: &[u8],
        expect: &Expect,
    ) -> Result<WriteOutcome, LocalError> {
        let expected = match expect {
            Expect::Absent => None,
            Expect::Hash(hash) => Some(hash.as_str()),
        };
        let done = self.with("write", |v| v.sync_write_if(path, bytes, expected))?;
        if done {
            self.touch(path, false);
        }
        Ok(Self::outcome(done))
    }

    fn evict_if(&self, path: &str, expected_hash: &str) -> Result<WriteOutcome, LocalError> {
        let done = self.with("evict", |v| v.sync_evict_if(path, expected_hash))?;
        if done {
            self.touch(path, true);
        }
        Ok(Self::outcome(done))
    }

    fn delete_if(&self, path: &str, expected_hash: &str) -> Result<WriteOutcome, LocalError> {
        let done = self.with("delete", |v| v.sync_delete_if(path, expected_hash))?;
        if done {
            self.touch(path, true);
        }
        Ok(Self::outcome(done))
    }

    fn rename_if(
        &self,
        from: &str,
        to: &str,
        expected_hash: &str,
    ) -> Result<WriteOutcome, LocalError> {
        let done = self.with("rename", |v| v.sync_rename_if(from, to, expected_hash))?;
        if done {
            self.touch(from, true);
            self.touch(to, false);
        }
        Ok(Self::outcome(done))
    }

    fn keep_version(&self, path: &str, content: &[u8]) -> Result<(), LocalError> {
        self.with("keep_version", |v| v.sync_keep_version(path, content))
    }

    fn pin(&self, content: &[u8]) -> Result<String, LocalError> {
        self.with("pin", |v| v.sync_pin(&self.pin_owner, content))
    }

    fn read_pinned(&self, hash: &str) -> Result<Vec<u8>, LocalError> {
        self.with("read_pinned", |v| v.sync_read_pinned(&self.pin_owner, hash))
    }

    fn unpin(&self, hash: &str) -> Result<(), LocalError> {
        self.with("unpin", |v| v.sync_unpin(&self.pin_owner, hash))
    }
}

/// 一个库此刻用哪个句柄同步（ADR-151 ②）。
enum Handle {
    /// 应用此刻打开的那一份：借用应用的句柄（自写抑制、编辑器横幅、派生数据都走既有路径）；
    /// 代号用来发现「这一轮期间用户切了库」。
    Open { generation: u64 },
    /// 没打开：同步服务自己开一份只供同步的句柄，并挂文件监听（本地一有改动就唤醒）。
    Background {
        vault: Arc<Mutex<Option<Vault>>>,
        _watcher: Option<luna_vault::watcher::VaultWatcher>,
    },
}

/// 一份库的同步会话（每个选中的库一个）。
struct Session {
    local_vault_id: String,
    root: PathBuf,
    /// 这个库在应用配置目录里的同步目录（`state.sqlite`、块缓存）。
    dir: PathBuf,
    remote_vault_id: Uuid,
    state: SyncState,
    /// 本机钥匙串与账户托管都取不到库主密钥 = `None`（只会出现在端到端的高级档）。
    ring: Option<KeyRing>,
    hashes: HashCache,
    handle: Option<Handle>,
    phase: SyncPhase,
    last_report: Option<RoundReport>,
    last_synced_ms: Option<i64>,
    last_error: Option<String>,
    /// 用户确认过下一轮的批量删除。
    allow_bulk_delete: bool,
    /// 上一轮扫到的本地同步范围总字节。
    local_bytes: Option<u64>,
    /// 提示流订阅（云端有新提交就排一轮、库被删除了就断开）；暂停、未登录、严格本地时不挂。
    /// 丢掉即停。
    hints: Option<HintWatch>,
    /// 游标序号的镜像（[`NO_CURSOR`] = 还没有游标）：提示流任务拿它判断序号提示是不是比这个库
    /// 已拉到的新。每轮跑完由调度循环刷新；状态库连接不能跨任务共用，故另存一份。
    seen: Arc<AtomicI64>,
}

/// 游标镜像里「还没有游标」（下一轮做全量对账）：任何序号提示都算新。
const NO_CURSOR: i64 = i64::MIN;

/// 状态库里游标的序号（读不出 / 没有 = [`NO_CURSOR`]）。
fn cursor_seq(state: &SyncState) -> i64 {
    state
        .cursor()
        .ok()
        .flatten()
        .map_or(NO_CURSOR, |cursor| cursor.seq)
}

/// 序号提示比这个库已拉到的新（纯函数）：只有新的才值得跑一轮——连上时服务端先发一帧当前
/// head，它与游标相等时什么都不做。
fn head_is_ahead(head: i64, seen: i64) -> bool {
    head > seen
}

/// 一个库的提示流订阅任务（[`watch_hints`]）。丢掉即停，会话没了它就跟着停。
pub(crate) struct HintWatch(tauri::async_runtime::JoinHandle<()>);

impl Drop for HintWatch {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// 这一轮（或取钥）撞上了「云端库已被删除」。
fn hit_deletion(session: &Session) -> bool {
    session.last_error.as_deref() == Some(luna_cloud::sync_client::VAULT_DELETED)
}

/// 一个库的同步此刻在哪一步（界面据此选文案）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum SyncPhase {
    /// 这个库没选中同步。
    Off,
    /// 开了，但本机没有库主密钥、账户也没有托管它（只会出现在端到端的高级档；默认托管下
    /// 钥匙串缺钥会自动重取，ADR-150 ②）。
    Locked,
    /// 库所在的目录不在（外接盘未挂载、被移走）：不跑、不产生任何删除（ADR-148 ⑤）。
    Unavailable,
    Idle,
    Syncing,
    /// 单库暂停，或全部暂停中。
    Paused,
    /// 上一轮出错（`last_error` 是稳定码），下一轮会重试。
    Error,
    /// 这份库连在另一个账户下（ADR-151 修订 2026-10-15）：此刻的账户不同步它，也不为它发任何
    /// 请求；那个账户的打码名在 `bound_account`。
    OtherAccount,
}

/// 同步整体能不能跑（与具体哪个库无关）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum SyncGate {
    Ready,
    /// 严格本地模式：同步整体禁用。
    StrictLocal,
    SignedOut,
}

/// 按需下载的进度（每取好一块一次；`f64` 因 specta 不收 64 位整数）。
#[derive(Debug, Clone, serde::Serialize, specta::Type, tauri_specta::Event)]
#[tauri_specta(event_name = "sync://download-progress")]
pub struct SyncDownloadProgress {
    /// 库相对路径。
    pub path: String,
    pub done: f64,
    pub total: f64,
}

/// 一个已知库的同步状态（账户页每行一个）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, specta::Type)]
pub struct VaultSyncDto {
    /// 库根路径（库注册表的唯一键；命令都按它指认库）。
    pub path: String,
    /// 给人看的路径：主目录缩写成 `~`（完整路径仍在 `path`，界面悬停时给出）。
    pub display_path: String,
    pub name: String,
    /// 是不是应用此刻打开的那一份。
    pub is_open: bool,
    pub phase: SyncPhase,
    /// 单独暂停了（与全部暂停无关）。
    pub paused: bool,
    pub remote_vault_id: Option<String>,
    /// 上一轮成功结束的时刻（Unix 毫秒；`f64` 因 specta 不收 64 位整数）。
    pub last_synced_ms: Option<f64>,
    /// 上一轮失败的稳定码。
    pub last_error: Option<String>,
    /// 上一轮生成的冲突副本数。
    pub conflict_copies: u32,
    /// 被批量删除熔断拦下、等用户确认的文件数。
    pub held_deletions: u32,
    /// 上一轮因为太大没有上传的文件（库相对路径）；其余文件照常同步（ADR-156）。
    pub too_large: Vec<String>,
    /// 本地同步范围的总字节（选中的库取上一轮扫描，没选中的库按需遍历、不打开库；`f64` 因
    /// specta 不收 64 位整数）。`None` = 还没算出来或库不在。
    pub local_bytes: Option<f64>,
    /// 对应云端库此刻占用的密文字节（含历史版本）；没选中 = `None`。
    pub cloud_bytes: Option<f64>,
    /// 连在另一个账户下时是哪个账户（打码名 + 所在服务器）；其余为 `None`。
    pub bound_account: Option<BoundAccountDto>,
}

/// 库连着的那个别的账户：打码名 + 它所在的服务器。
///
/// 只写打码名不够（真机 2026-11-03）：同一个邮箱在测试服务器与 Lunarca 云上各有一个账户时，
/// 「已连接到另一个账户（jo…19@gmail.com）」与此刻登录的邮箱一字不差，读起来像是自相矛盾。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
pub struct BoundAccountDto {
    /// 绑定时账户名的打码写法（`jo…88@gmail.com`，`account_scope::mask_account_name`）。
    pub label: String,
    /// 那台服务器的主机与端口（分桶名）。
    pub server_host: String,
    /// 是不是本构建的 Lunarca 云：界面据此写「Lunarca 云」而不是主机名。
    pub server_official: bool,
}

impl BoundAccountDto {
    fn of(owner: &BindingOwner) -> Self {
        Self {
            label: owner.account_label.clone(),
            server_host: owner.server_bucket.clone(),
            server_official: owner.server_bucket
                == luna_cloud::config::server_bucket(luna_cloud::config::DEFAULT_API_BASE),
        }
    }
}

/// 账户的同步存储容量（ADR-152）：已用是账户全部云端库的密文字节（含历史版本、含这台设备上
/// 没有对应本地库的云端库）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SyncStorageLimitDto {
    Unknown,
    NotIncluded,
    Unlimited,
    Limited { bytes: f64 },
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, specta::Type)]
pub struct SyncStorageDto {
    pub used_bytes: f64,
    pub limit: SyncStorageLimitDto,
    /// 容量到了哪一档：服务端用量里 `sync_storage_bytes` 那一维的 `alert`（协议 v12，与账户页用量表
    /// 同一份判定）；那一维取不到时为 `none`——不在客户端拿阈值自己比。
    pub alert: luna_cloud::client::UsageAlertDto,
    /// 这台设备上没有对应本地库的云端库合计占用（说明「总数比各行加起来大」的那一块）。
    pub unlinked_bytes: f64,
}

/// 全部暂停（ADR-151 ③）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, specta::Type)]
pub struct GlobalPauseDto {
    /// 到点自动恢复的时刻（Unix 毫秒）；`None` = 直到用户恢复。
    pub until_ms: Option<f64>,
}

/// 退订 / 降级后的只读宽限（PRD-13-015）：上传被拒、拉取照常，到 `purge_after_ms` 清除云副本。
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, specta::Type)]
pub struct ReadOnlyGraceDto {
    pub since_ms: f64,
    pub purge_after_ms: f64,
}

/// 界面要的同步总览：`sync://status` 的载荷就是它（完整快照）。
// `PartialEq` 供发布去重：与上一份完全相同的总览不再广播（见 `SyncService::publish_with`）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, specta::Type)]
pub struct SyncOverviewDto {
    pub gate: SyncGate,
    pub paused: Option<GlobalPauseDto>,
    /// 账户容量；还没取到（未登录、严格本地、服务端没有这一维）= `None`。
    pub storage: Option<SyncStorageDto>,
    /// 只读宽限中（失去同步权益）；不在宽限中或还没取到 = `None`。
    pub read_only: Option<ReadOnlyGraceDto>,
    /// 大文件（录音等）此刻暂停传输的原因（ADR-156 ③）；不暂停 = `None`。笔记照常同步。
    pub large_paused: Option<LargePauseDto>,
    /// 账户因付款争议被冻结，上一轮的上传被服务端拒绝（纵切 3 C3）：上传暂停、下载照常，本地
    /// 改动留着等解冻。判据是服务端这一轮的真实答复（任一选中的库报了冻结），不是缓存的权益。
    pub account_frozen: bool,
    pub vaults: Vec<VaultSyncDto>,
}

/// 大文件为什么暂停（ADR-156 ③）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum LargePauseDto {
    /// 按流量计费的网络（手机热点、蜂窝、低数据模式）。
    Metered,
    /// 低电量模式，或用电池且电量低于 20%。
    LowBattery,
}

/// 此刻要不要暂停大文件、为什么——设备偏好与系统条件合起来的**唯一**判定：引擎参数
/// （`defer_large`）与总览上的说明都问它。
fn large_pause(state: &crate::ipc::AppState) -> Option<LargePauseDto> {
    let settings = state.settings_snapshot();
    let conditions = crate::platform::large_transfer_conditions();
    if conditions.metered
        && settings
            .get_bool("sync.pause_large_on_metered")
            .unwrap_or(true)
    {
        return Some(LargePauseDto::Metered);
    }
    if conditions.low_battery
        && settings
            .get_bool("sync.pause_large_on_battery")
            .unwrap_or(true)
    {
        return Some(LargePauseDto::LowBattery);
    }
    None
}

impl SyncOverviewDto {
    fn empty() -> Self {
        Self {
            gate: SyncGate::Ready,
            paused: None,
            storage: None,
            read_only: None,
            large_paused: None,
            account_frozen: false,
            vaults: Vec::new(),
        }
    }
}

/// 从云端取来的体积快照（每轮刷新一次；取不到就沿用上一次）。
#[derive(Debug, Clone, Default)]
struct CloudSizes {
    /// 远端库 id → 已存密文字节。
    vault_bytes: HashMap<String, f64>,
    /// 账户同步存储容量那一维：（已用，上限，服务端判定的档位）。
    capacity: Option<(f64, Option<f64>, luna_cloud::client::UsageAlertDto)>,
    /// 只读宽限：（起算，清除时刻），Unix 毫秒。
    grace: Option<(i64, i64)>,
}

/// 没选中的库的本地大小缓存：库根 → （字节，算出的时刻）。
type LocalSizes = HashMap<PathBuf, (u64, std::time::Instant)>;

/// 没选中的库多久重算一次本地大小（遍历要逐个 stat，大库不该每分钟走一遍）。
const LOCAL_SIZE_TTL: Duration = Duration::from_secs(300);

/// 同步总览变了。
#[derive(Debug, Clone, serde::Serialize, specta::Type, tauri_specta::Event)]
#[tauri_specta(event_name = "sync://status")]
pub struct SyncStatusChanged(pub SyncOverviewDto);

/// 全部暂停的落盘位置（应用配置目录下；跨重启保持）。
const PAUSE_FILE: &str = "sync-pause.json";

/// 只读宽限已发过的提醒（同一轮宽限不重复提醒；换了一轮宽限就重来）。放在此刻账户的提醒标记
/// 目录下（[`crate::account_scope::marker_dir`]）：宽限是那个账户的处境。
pub(crate) const GRACE_REMINDER_FILE: &str = "sync-grace-reminders.json";
/// 提醒节点：开始一次，期满前 14 / 7 / 1 天与当天各一次（PRD-13 §4）。
const GRACE_START: i64 = -1;
const GRACE_MILESTONES: [i64; 4] = [14, 7, 1, 0];
const DAY_MS: i64 = 86_400_000;

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct GraceReminders {
    purge_after_ms: i64,
    fired: Vec<i64>,
}

/// 此刻落在哪个提醒节点：剩余整天数（向下取整）多于 14 天 = 开始；否则取不小于它的最小节点。
fn grace_milestone(purge_after_ms: i64, now_ms: i64) -> i64 {
    let days_left = (purge_after_ms - now_ms).max(0) / DAY_MS;
    if days_left > GRACE_MILESTONES[0] {
        return GRACE_START;
    }
    GRACE_MILESTONES
        .iter()
        .copied()
        .filter(|m| *m >= days_left)
        .min()
        .unwrap_or(0)
}

/// 这一轮该发的提醒（已发过 → `None`），并把它记进 `state`。
fn next_grace_reminder(
    state: &mut GraceReminders,
    purge_after_ms: i64,
    now_ms: i64,
) -> Option<i64> {
    if state.purge_after_ms != purge_after_ms {
        *state = GraceReminders {
            purge_after_ms,
            fired: Vec::new(),
        };
    }
    let milestone = grace_milestone(purge_after_ms, now_ms);
    if state.fired.contains(&milestone) {
        return None;
    }
    state.fired.push(milestone);
    Some(milestone)
}

/// 只读宽限的倒计时提醒（走通知中心，落点账户页）。不在宽限中就清掉记录。`marker_dir` 是此刻
/// 账户的提醒标记目录。
fn remind_read_only(
    app: &tauri::AppHandle,
    marker_dir: &Path,
    grace: Option<(i64, i64)>,
    now: i64,
) {
    let path = marker_dir.join(GRACE_REMINDER_FILE);
    let Some((_, purge_after)) = grace else {
        let _ = std::fs::remove_file(&path);
        return;
    };
    let mut state: GraceReminders = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let Some(milestone) = next_grace_reminder(&mut state, purge_after, now) else {
        return;
    };
    // 先记下再发：写不进就不发，免得每一轮都弹同一条。
    let written = serde_json::to_vec(&state).ok().is_some_and(|bytes| {
        std::fs::create_dir_all(marker_dir).is_ok() && std::fs::write(&path, bytes).is_ok()
    });
    if !written {
        return;
    }
    let kind = match milestone {
        GRACE_START => "sync_read_only_started",
        0 => "sync_read_only_today",
        _ => "sync_read_only_reminder",
    };
    let date = chrono::DateTime::from_timestamp_millis(purge_after)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d")
                .to_string()
        })
        .unwrap_or_default();
    if let Some(center) = app.try_state::<crate::notifications::NotificationCenter>() {
        center.publish(
            crate::notifications::NotificationEvent::sync_read_only(
                kind,
                (purge_after - now).max(0) / DAY_MS,
                &date,
            ),
            app,
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct GlobalPause {
    /// `None` = 直到用户恢复。
    until_ms: Option<i64>,
}

/// 读全部暂停；到点了就删掉文件、视为没暂停（到点自动恢复）。
fn read_pause(config_dir: &Path, now: i64) -> Option<GlobalPause> {
    let path = config_dir.join(PAUSE_FILE);
    let pause: GlobalPause = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
    if pause.until_ms.is_some_and(|until| until <= now) {
        let _ = std::fs::remove_file(&path);
        return None;
    }
    Some(pause)
}

/// 写 / 清全部暂停。
///
/// # Errors
/// 写不了配置目录。
pub fn write_pause(config_dir: &Path, until_ms: Option<Option<i64>>) -> std::io::Result<()> {
    let path = config_dir.join(PAUSE_FILE);
    match until_ms {
        None => match std::fs::remove_file(&path) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err),
            _ => Ok(()),
        },
        Some(until_ms) => {
            std::fs::create_dir_all(config_dir)?;
            let bytes = serde_json::to_vec(&GlobalPause { until_ms })
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            std::fs::write(&path, bytes)
        }
    }
}

/// 装着的同步会话，以及它们归哪个账户（ADR-151 修订 2026-10-15）。
#[derive(Default)]
struct Loaded {
    /// 会话是按哪个账户装的；没登录 = `None`，此时一个会话都没有。
    owner: Option<BindingOwner>,
    /// 此刻账户名下选中同步的库，按本地库身份。
    live: BTreeMap<String, Session>,
    /// 连在另一个账户下的库：本地库身份 → 那个账户。只显示，绝不打开、绝不发请求。
    elsewhere: BTreeMap<String, BoundAccountDto>,
}

/// 应用托管的同步服务。
pub struct SyncService {
    sessions: tokio::sync::Mutex<Loaded>,
    overview: Mutex<SyncOverviewDto>,
    cloud_sizes: Mutex<CloudSizes>,
    local_sizes: Mutex<LocalSizes>,
    /// 记住的此刻账户与读到它时的账户纪元（见 [`SyncService::owner_now`]）。
    owner_cache: Mutex<Option<(u64, BindingOwner)>>,
    /// 每换一次账户加一：途中那一轮据此作废，不再发布上一个账户的总览。
    account_epoch: AtomicU64,
    wake: tokio::sync::Notify,
    /// 这次唤醒为了什么（[`SyncService::take_scope`] 据此定下一轮跑哪些库）。
    wakeups: Mutex<Wakeups>,
    started: AtomicBool,
    /// 已经说过「云端库被删除了」的远端库（以及这台设备自己删的）：同一个库只说一次。
    deletions_announced: Mutex<std::collections::HashSet<Uuid>>,
}

impl Default for SyncService {
    fn default() -> Self {
        Self {
            sessions: tokio::sync::Mutex::new(Loaded::default()),
            overview: Mutex::new(SyncOverviewDto::empty()),
            cloud_sizes: Mutex::new(CloudSizes::default()),
            local_sizes: Mutex::new(LocalSizes::new()),
            owner_cache: Mutex::new(None),
            account_epoch: AtomicU64::new(0),
            wake: tokio::sync::Notify::new(),
            wakeups: Mutex::new(Wakeups::default()),
            started: AtomicBool::new(false),
            deletions_announced: Mutex::new(std::collections::HashSet::new()),
        }
    }
}

/// 攒着的唤醒理由：要不要整轮跑，以及哪些库的提示流说云端有新提交（本地库身份 → 提示里最大的
/// 序号）。
#[derive(Default)]
struct Wakeups {
    all: bool,
    hinted: BTreeMap<String, i64>,
}

/// 一次调度跑哪些库。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Scope {
    /// 全部选中的库（本地改动、立即同步、暂停变了、60 秒兜底）。
    All,
    /// 只跑这些库，且只在游标还没追上提示里的序号时跑（序号提示）。
    Hinted(BTreeMap<String, i64>),
}

impl Scope {
    /// 这一次调度要不要跑这个库（`seen` = 它此刻的游标序号）。提示攒下之后、轮到它之前，若已有
    /// 一轮把游标推过了提示的序号（提示恰在那一轮途中到达），就不再白跑一轮。
    fn includes(&self, local_vault_id: &str, seen: i64) -> bool {
        match self {
            Self::All => true,
            Self::Hinted(hinted) => hinted
                .get(local_vault_id)
                .is_some_and(|head| head_is_ahead(*head, seen)),
        }
    }
}

/// 同步整体能不能跑：严格本地优先；认不出此刻是谁（没登录、档案读不出）= 未登录。
fn gate_of(strict_local: bool, owner: Option<&BindingOwner>) -> SyncGate {
    if strict_local {
        SyncGate::StrictLocal
    } else if owner.is_none() {
        SyncGate::SignedOut
    } else {
        SyncGate::Ready
    }
}

/// 一轮要读的应用现状（零网络）。
struct TickInput {
    config_dir: PathBuf,
    known: Vec<luna_vault::registry::VaultEntry>,
    open: Option<(String, u64)>,
    strict_local: bool,
    client: Option<Arc<CloudClient>>,
}

/// 一轮里碰应用与网络的那几步。生产走 [`AppEnv`]；测试换成只记账的实现，断言另一个账户的库
/// 一个请求都不发、换账户后不再发布上一个账户的总览。
trait TickEnv: Sync {
    fn input(&self) -> Option<TickInput>;
    /// 库主密钥的本机缓存。
    fn keys(&self) -> CredentialCmkStore;
    /// 后台库的文件监听唤醒谁（`None` = 不挂监听）。
    fn waker(&self) -> Option<Arc<SyncService>>;
    /// 从账户托管信封取回库主密钥（网络）。
    fn fetch_keys(
        &self,
        session: &mut Session,
        client: &Arc<CloudClient>,
    ) -> impl Future<Output = bool> + Send;
    /// 跑这个库的一轮（网络）；返回这一轮改过的路径。
    fn run_round(
        &self,
        session: &mut Session,
        client: &Arc<CloudClient>,
    ) -> impl Future<Output = Vec<(String, bool)>> + Send;
    /// 取一次账户的云端体积与容量（网络）。
    fn cloud_sizes(
        &self,
        client: &Arc<CloudClient>,
        previous: CloudSizes,
    ) -> impl Future<Output = CloudSizes> + Send;
    fn remind_read_only(&self, owner: &BindingOwner, grace: Option<(i64, i64)>);
    /// 订阅这个库的提示流：云端有新提交就为它排一轮、库被删除了就断开（`None` = 不订阅）。
    fn watch(&self, session: &Session, client: &Arc<CloudClient>) -> Option<HintWatch>;
    /// 解除一份云端已删除的库在这台设备上的绑定（会话已摘下）：主密钥缓存、基线钉住、同步目录与
    /// 块缓存。本地文件不动。
    fn release(&self, session: Session) -> Result<(), String>;
    /// 通知中心说一次「云端库『name』已在另一台设备上被删除……」。
    fn notify_deleted(&self, name: &str);
    fn publish(&self, service: &SyncService, overview: SyncOverviewDto);
    /// 同步改过打开的库的路径：通知界面与派生数据。
    fn vault_changes(&self, touched: Vec<(String, bool)>);
}

/// 生产环境：一切取自应用。
struct AppEnv<'a> {
    app: &'a tauri::AppHandle,
}

impl TickEnv for AppEnv<'_> {
    fn input(&self) -> Option<TickInput> {
        let state = self.app.try_state::<crate::ipc::AppState>()?;
        Some(TickInput {
            config_dir: self.app.path().app_config_dir().ok()?,
            known: state.known_vaults(),
            open: state.sync_vault_identity(),
            strict_local: state.strict_local(),
            client: self
                .app
                .try_state::<luna_cloud::commands::CloudState>()
                .map(|s| Arc::clone(&s.0)),
        })
    }

    fn keys(&self) -> CredentialCmkStore {
        crate::secrets::cmk_store()
    }

    fn waker(&self) -> Option<Arc<SyncService>> {
        self.app
            .try_state::<Arc<SyncService>>()
            .map(|s| Arc::clone(&s))
    }

    async fn fetch_keys(&self, session: &mut Session, client: &Arc<CloudClient>) -> bool {
        fetch_keys(session, client).await
    }

    async fn run_round(
        &self,
        session: &mut Session,
        client: &Arc<CloudClient>,
    ) -> Vec<(String, bool)> {
        match self.app.try_state::<crate::ipc::AppState>() {
            Some(state) => run_round(&state, session, client).await,
            None => Vec::new(),
        }
    }

    async fn cloud_sizes(&self, client: &Arc<CloudClient>, previous: CloudSizes) -> CloudSizes {
        fetch_cloud_sizes(self.app, client, previous).await
    }

    fn remind_read_only(&self, owner: &BindingOwner, grace: Option<(i64, i64)>) {
        if let Ok(config_dir) = self.app.path().app_config_dir() {
            let dir = crate::account_scope::marker_dir(&config_dir, owner);
            remind_read_only(self.app, &dir, grace, now_ms());
        }
    }

    fn watch(&self, session: &Session, client: &Arc<CloudClient>) -> Option<HintWatch> {
        Some(HintWatch(tauri::async_runtime::spawn(watch_hints(
            self.app.clone(),
            Arc::clone(client),
            session.remote_vault_id,
            session.local_vault_id.clone(),
            Arc::clone(&session.seen),
        ))))
    }

    fn release(&self, session: Session) -> Result<(), String> {
        let (local_vault_id, root, dir, remote) = (
            session.local_vault_id.clone(),
            session.root.clone(),
            session.dir.clone(),
            session.remote_vault_id,
        );
        // 先放掉会话（状态库连接、后台句柄与监听）：Windows 上删不掉开着的状态库，解钉也要
        // 另开一份库句柄。
        drop(session);
        release_binding(&dir, remote, &self.keys(), |holder| {
            unpin_all(self.app, &local_vault_id, &root, holder);
        })?;
        // 断开 = 回到本机库：不再属于任何账户（ADR-151 修订 2026-10-15「续二」）。
        if let Some(state) = self.app.try_state::<crate::ipc::AppState>() {
            state.vault_registry().make_device(&root);
        }
        Ok(())
    }

    fn notify_deleted(&self, name: &str) {
        if let Some(center) = self
            .app
            .try_state::<crate::notifications::NotificationCenter>()
        {
            center.publish(
                crate::notifications::NotificationEvent::sync_vault_deleted_remotely(name),
                self.app,
            );
        }
    }

    fn publish(&self, service: &SyncService, overview: SyncOverviewDto) {
        service.publish(self.app, overview);
    }

    fn vault_changes(&self, touched: Vec<(String, bool)>) {
        if !touched.is_empty() {
            crate::ipc::emit_vault_changes(self.app, touched);
        }
    }
}

/// 解掉一个库里某个持有者钉住的全部基线：库开着就用应用的句柄，没打开（且还在）就临时开一份。
pub(crate) fn unpin_all(app: &tauri::AppHandle, local_vault_id: &str, root: &Path, holder: &str) {
    let Some(state) = app.try_state::<crate::ipc::AppState>() else {
        return;
    };
    let open_here = state
        .sync_vault_identity()
        .is_some_and(|(open_id, _)| open_id == local_vault_id);
    if open_here {
        if let Ok(guard) = state.vault_arc().lock() {
            if let Some(vault) = guard.as_ref() {
                let _ = vault.sync_unpin_all(holder);
            }
        }
    } else if let Ok(vault) = Vault::open(root) {
        let _ = vault.sync_unpin_all(holder);
    }
}

/// 提示流断了以后多久重连（起步）。
const HINT_RETRY_MIN: Duration = Duration::from_secs(30);
/// 重连间隔的上限（连不上时逐次翻倍到这里）。
const HINT_RETRY_MAX: Duration = Duration::from_secs(300);
/// 多久一个字节都没收到就当连接已死（服务端每 30 秒发一次保活）。
const HINT_IDLE: Duration = Duration::from_secs(90);

/// 一个库的提示流订阅（ADR-148 ⑧：推送只是提示）：序号提示（`head`）比游标新就为这个库排一轮
/// （[`SyncService::head_hint`]，真正的改动随后按游标拉）；终结帧 `vault_deleted`（连上时库已被
/// 删除 = 410 `vault_deleted`，同样处置）交给删除处置。断了按退避重连，断着的这段时间靠 60 秒兜底；
/// 会话没了（停止同步、暂停、退出、换账户、严格本地）任务随之取消。
async fn watch_hints(
    app: tauri::AppHandle,
    client: Arc<CloudClient>,
    remote: Uuid,
    local_vault_id: String,
    seen: Arc<AtomicI64>,
) {
    use luna_cloud::sync_client::{is_vault_deleted, SyncEventDto};
    let id = remote.to_string();
    let mut backoff = HINT_RETRY_MIN;
    loop {
        match client.sync_events(&id).await {
            Ok(mut stream) => loop {
                match stream.next_event(HINT_IDLE).await {
                    Ok(Some(SyncEventDto::VaultDeleted { .. })) => {
                        remote_deleted(&app, remote).await;
                        return;
                    }
                    Ok(Some(SyncEventDto::Head { seq })) => {
                        backoff = HINT_RETRY_MIN;
                        if let Some(service) = app.try_state::<Arc<SyncService>>() {
                            service.head_hint(&local_vault_id, seq, &seen);
                        }
                    }
                    // 关流、读失败、连保活都收不到：重连。
                    Ok(None) | Err(_) => break,
                }
            },
            Err(err) if is_vault_deleted(&err) => {
                remote_deleted(&app, remote).await;
                return;
            }
            Err(_) => {}
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(HINT_RETRY_MAX);
    }
}

/// 提示流说这个库被删除了：交给同步服务的同一处处置（与数据面 410 同一条路）。
///
/// 注意：处置会丢掉这个会话，连同正在跑这段代码的订阅任务一起被取消——取消只在下一个 `await`
/// 生效，而处置在拿到会话锁之后全是同步代码，做完才回到这里。
async fn remote_deleted(app: &tauri::AppHandle, remote: Uuid) {
    let Some(service) = app.try_state::<Arc<SyncService>>().map(|s| Arc::clone(&s)) else {
        return;
    };
    service.remote_deleted_with(&AppEnv { app }, remote).await;
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// 本机这份库的身份：注册表登记过就用登记的，否则读库里的身份文件（只读，不补写）。
#[must_use]
pub fn identity_of(entry: &luna_vault::registry::VaultEntry) -> Option<String> {
    entry.id.clone().or_else(|| {
        let text = std::fs::read_to_string(
            entry
                .path
                .join(".luna")
                .join(luna_vault::identity::VAULT_ID_FILE),
        )
        .ok()?;
        luna_vault::VaultId::parse(text.trim()).map(|id| id.to_string())
    })
}

impl SyncService {
    /// 当前总览（零 I/O）。
    #[must_use]
    pub fn overview(&self) -> SyncOverviewDto {
        self.overview
            .lock()
            .map(|s| s.clone())
            .unwrap_or_else(|_| SyncOverviewDto::empty())
    }

    /// 本地有改动 / 用户点了立即同步 / 暂停状态变了：尽快跑一轮（全部选中的库）。
    pub fn nudge(&self) {
        if let Ok(mut wakeups) = self.wakeups.lock() {
            wakeups.all = true;
        }
        self.wake.notify_one();
    }

    /// 提示流收到一帧序号提示（`seen` = 这个库的游标镜像）：比游标新才为这个库排一轮，返回排没
    /// 排。一阵提示只记最大的序号、只留一次唤醒（`Notify` 至多存一张许可），去抖之后跑成一轮。
    pub(crate) fn head_hint(&self, local_vault_id: &str, head: i64, seen: &AtomicI64) -> bool {
        if !head_is_ahead(head, seen.load(Ordering::SeqCst)) {
            return false;
        }
        if let Ok(mut wakeups) = self.wakeups.lock() {
            let max = wakeups
                .hinted
                .entry(local_vault_id.to_owned())
                .or_insert(head);
            *max = (*max).max(head);
        }
        self.wake.notify_one();
        true
    }

    /// 取走攒着的唤醒理由，定下这一次跑哪些库（`timed_out` = 60 秒兜底到点）：兜底或有任何一处
    /// 要整轮跑 → 全部；否则只跑提示过的库；什么都没攒（许可是上一轮途中留下的、理由已被那一轮
    /// 一并取走）→ `None`，这一次不跑。
    fn take_scope(&self, timed_out: bool) -> Option<Scope> {
        let Ok(mut wakeups) = self.wakeups.lock() else {
            return Some(Scope::All);
        };
        let taken = std::mem::take(&mut *wakeups);
        if timed_out || taken.all {
            Some(Scope::All)
        } else if taken.hinted.is_empty() {
            None
        } else {
            Some(Scope::Hinted(taken.hinted))
        }
    }

    fn publish(&self, app: &tauri::AppHandle, mut overview: SyncOverviewDto) {
        use tauri_specta::Event as _;
        // 大文件暂停由此刻的系统条件定（每个出口都过这里，不必每处构造各算一遍）。
        overview.large_paused = (overview.gate == SyncGate::Ready)
            .then(|| {
                app.try_state::<crate::ipc::AppState>()
                    .and_then(|s| large_pause(&s))
            })
            .flatten();
        self.publish_with(overview, |overview| {
            let _ = SyncStatusChanged(overview).emit(app);
        });
    }

    /// 存下新总览，**与此刻那份完全相同就不广播**（2026-10-15 真机卡顿）。
    ///
    /// 一轮同步里有好几处出口各发一次（逐库开跑、收尾、命令改了状态），其中多数与上一份一字
    /// 不差；而每一次广播都要在界面那头反序列化、分发给全部订阅者。任何一处真实变化（库的
    /// 阶段、闸门、账户、容量、暂停……）照常广播——判据是整份快照逐字段相等，不挑字段。
    /// 锁中毒时照发：宁可多发一次，也不让界面停在旧状态上。
    fn publish_with(&self, overview: SyncOverviewDto, emit: impl FnOnce(SyncOverviewDto)) {
        if let Ok(mut current) = self.overview.lock() {
            if *current == overview {
                return;
            }
            current.clone_from(&overview);
        }
        emit(overview);
    }

    /// 启动调度循环（应用装配时调一次）。
    pub fn start(app: tauri::AppHandle) {
        let Some(service) = app.try_state::<Arc<SyncService>>().map(|s| Arc::clone(&s)) else {
            return;
        };
        if service.started.swap(true, Ordering::SeqCst) {
            return;
        }
        tauri::async_runtime::spawn(async move {
            // 启动后先跑一轮：不跑就要等满一个周期，这一分钟里账户页与状态栏都停在默认的
            // 「未开启」（真机 S4b：开过同步的库重启后显示未开启）。
            tokio::time::sleep(DEBOUNCE).await;
            let _ = service.take_scope(true);
            service.tick(&app).await;
            loop {
                let timed_out = tokio::time::timeout(PERIOD, service.wake.notified())
                    .await
                    .is_err();
                // 去抖：编辑器连续保存、云端一阵提交时都攒成一轮。
                tokio::time::sleep(DEBOUNCE).await;
                if let Some(scope) = service.take_scope(timed_out) {
                    service.tick_in(&AppEnv { app: &app }, &scope).await;
                }
            }
        });
    }

    /// 跑一轮：逐个选中的库判定、同步，并随时发布总览。
    pub async fn tick(&self, app: &tauri::AppHandle) {
        self.tick_with(&AppEnv { app }).await;
    }

    async fn tick_with(&self, env: &impl TickEnv) {
        self.tick_in(env, &Scope::All).await;
    }

    /// 跑一次调度：`scope` 之外的库这一次不跑（照常对齐、照常出现在总览里）。
    async fn tick_in(&self, env: &impl TickEnv, scope: &Scope) {
        let epoch = self.account_epoch.load(Ordering::SeqCst);
        let switched = || self.account_epoch.load(Ordering::SeqCst) != epoch;
        let Some(input) = env.input() else {
            return;
        };
        let owner = self.owner_now(input.client.as_deref());
        let gate = gate_of(input.strict_local, owner.as_ref());
        let pause = read_pause(&input.config_dir, now_ms());
        let known = &input.known;
        let open = input.open.as_ref();

        let mut loaded = self.sessions.lock().await;
        // 等锁期间换了账户：这一轮作废，换账户那一侧已按新账户重排并发布过。
        if switched() {
            return;
        }
        self.align(
            &input.config_dir,
            known,
            owner.as_ref(),
            &mut loaded,
            &env.keys(),
        );
        // 没选中的库的本地大小先用缓存：重算要逐个 stat，排在同步前面会拖慢这一轮（大库可能
        // 要好几秒），放到本轮同步之后再刷新。
        let mut local = self.local_sizes_snapshot();
        let mut cloud = if gate == SyncGate::Ready {
            self.cloud_sizes_snapshot()
        } else {
            // 未登录 / 严格本地：上一个账户的容量不该挂在这一屏上。
            CloudSizes::default()
        };

        let (Some(client), Some(owner), SyncGate::Ready, None) =
            (input.client.clone(), owner, gate, pause)
        else {
            // 严格本地、未登录、全部暂停：一个请求都不发（提示流也断开）。
            for session in loaded.live.values_mut() {
                session.hints = None;
                session.phase = if pause.is_some() {
                    SyncPhase::Paused
                } else {
                    SyncPhase::Off
                };
            }
            local = self.refresh_local(known, live_roots(&loaded)).await;
            let overview = build_overview(gate, pause, known, open, &loaded, &cloud, &local);
            // 持锁发布：换账户那一侧要等这里发完才能发新账户的，次序不会颠倒。
            env.publish(self, overview);
            return;
        };

        let waker = env.waker();
        let ids: Vec<String> = loaded.live.keys().cloned().collect();
        let mut open_touched = Vec::new();
        // 这一轮撞上「云端库已被删除」的库：轮次之后统一解除绑定。
        let mut deleted = Vec::new();
        for id in ids {
            // 换了账户：剩下的库不再拿上一个账户的绑定去跑。
            if switched() {
                break;
            }
            let Some(session) = loaded.live.get_mut(&id) else {
                continue;
            };
            if !scope.includes(&id, cursor_seq(&session.state)) {
                continue;
            }
            if !prepare(session, open, waker.as_ref()) {
                continue;
            }
            if session.state.paused().unwrap_or(false) {
                session.phase = SyncPhase::Paused;
                session.hints = None;
                continue;
            }
            if session.ring.is_none() && !env.fetch_keys(session, &client).await {
                if hit_deletion(session) {
                    deleted.push(id);
                }
                continue;
            }
            if session.hints.is_none() {
                session.hints = env.watch(session, &client);
            }
            session.phase = SyncPhase::Syncing;
            let overview = build_overview(gate, None, known, open, &loaded, &cloud, &local);
            env.publish(self, overview);
            let Some(session) = loaded.live.get_mut(&id) else {
                continue;
            };
            let touched = env.run_round(session, &client).await;
            session
                .seen
                .store(cursor_seq(&session.state), Ordering::SeqCst);
            if matches!(session.handle, Some(Handle::Open { .. })) {
                open_touched.extend(touched);
            }
            if hit_deletion(session) {
                deleted.push(id);
            }
        }
        if !switched() {
            for id in deleted {
                self.drop_deleted(env, &mut loaded, &id, known);
            }
            // 体积在这一轮的推送之后取，才是推完之后的占用。
            cloud = env.cloud_sizes(&client, cloud).await;
        }
        // 取体积的途中换了账户：上一个账户的体积不进缓存、不提醒、不发布。
        if !switched() {
            env.remind_read_only(&owner, cloud.grace);
            if let Ok(mut cached) = self.cloud_sizes.lock() {
                cached.clone_from(&cloud);
            }
            local = self.refresh_local(known, live_roots(&loaded)).await;
            let overview = build_overview(gate, None, known, open, &loaded, &cloud, &local);
            env.publish(self, overview);
        }
        drop(loaded);
        // 自写抑制吞掉了监听器的回声：同步改过打开的库的路径由这里通知界面与派生数据（换了
        // 账户也照发——文件确实改了）。后台库没有界面，打开时由启动扫描自愈。
        env.vault_changes(open_touched);
    }

    /// 此刻登录的账户。档案在钥匙串里，每轮都读会在某些构建上多弹一次钥匙串框（同
    /// `CloudClient::logged_in` 的说明），故记住上一次读到的；换账户（[`Self::account_changed`]）
    /// 或换了服务器时作废重读。没登录 = `None`。
    ///
    /// 记下的那一份带着读它时的账户纪元：读档案途中换了账户，读到的就不记（否则一次慢读会把
    /// 上一个账户写回去，之后每轮都拿它当此刻的账户）。
    fn owner_now(&self, client: Option<&CloudClient>) -> Option<BindingOwner> {
        let epoch = self.account_epoch.load(Ordering::SeqCst);
        let client = client.filter(|c| c.logged_in())?;
        let bucket = client.server_bucket();
        let cached = self
            .owner_cache
            .lock()
            .ok()
            .and_then(|c| c.clone())
            .filter(|(at, owner)| *at == epoch && owner.server_bucket == bucket);
        if let Some((_, owner)) = cached {
            return Some(owner);
        }
        let owner = crate::account_scope::current(client)?;
        if let Ok(mut cache) = self.owner_cache.lock() {
            if self.account_epoch.load(Ordering::SeqCst) == epoch {
                *cache = Some((epoch, owner.clone()));
            }
        }
        Some(owner)
    }

    /// 会话按此刻的账户对齐；换了账户（含退出）时连上一个账户的云端体积缓存一起清掉。
    fn align(
        &self,
        config_dir: &Path,
        known: &[luna_vault::registry::VaultEntry],
        owner: Option<&BindingOwner>,
        loaded: &mut Loaded,
        keys: &dyn CmkStore,
    ) {
        if reconcile(config_dir, known, owner, loaded, keys) {
            if let Ok(mut cached) = self.cloud_sizes.lock() {
                *cached = CloudSizes::default();
            }
        }
    }

    /// 账户变了（登录、退出、换账户、会话失效——`AccountChange::Session`；ADR-151 修订
    /// 2026-10-15）：当场按新账户重排会话、重发总览，再唤醒一轮。途中那一轮作废（见
    /// [`Self::tick_with`]），绝不再发布上一个账户的总览。
    pub fn account_changed(app: &tauri::AppHandle) {
        let Some(service) = app.try_state::<Arc<SyncService>>().map(|s| Arc::clone(&s)) else {
            return;
        };
        service.forget_account();
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            service.realign_with(&AppEnv { app: &app }).await;
            service.nudge();
        });
    }

    /// 作废记住的账户与途中的那一轮。
    fn forget_account(&self) {
        self.account_epoch.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut cache) = self.owner_cache.lock() {
            *cache = None;
        }
    }

    /// 只按此刻的账户重排会话并发布总览（零网络）。
    async fn realign_with(&self, env: &impl TickEnv) {
        let Some(input) = env.input() else {
            return;
        };
        let owner = self.owner_now(input.client.as_deref());
        let gate = gate_of(input.strict_local, owner.as_ref());
        let pause = read_pause(&input.config_dir, now_ms());
        let mut loaded = self.sessions.lock().await;
        self.align(
            &input.config_dir,
            &input.known,
            owner.as_ref(),
            &mut loaded,
            &env.keys(),
        );
        for session in loaded.live.values_mut() {
            if gate != SyncGate::Ready {
                session.hints = None;
                session.phase = if pause.is_some() {
                    SyncPhase::Paused
                } else {
                    SyncPhase::Off
                };
            } else if pause.is_some() || session.state.paused().unwrap_or(false) {
                session.hints = None;
                session.phase = SyncPhase::Paused;
            }
        }
        let cloud = if gate == SyncGate::Ready {
            self.cloud_sizes_snapshot()
        } else {
            CloudSizes::default()
        };
        let overview = build_overview(
            gate,
            pause,
            &input.known,
            input.open.as_ref(),
            &loaded,
            &cloud,
            &self.local_sizes_snapshot(),
        );
        env.publish(self, overview);
    }

    /// 打开的库里按需下载一个仅云端的文件，或释放一个文件的本机空间（ADR-156）。与同步轮互斥
    /// （同一把会话锁）。下载进度经 `sync://download-progress` 发出。
    ///
    /// # Errors
    /// 稳定码：`sync_not_enabled`（打开的库没开同步）、`sync_signed_out`、`sync_keys_missing`、
    /// 远端与本地失败的码。
    pub async fn on_demand(
        &self,
        app: &tauri::AppHandle,
        path: &str,
        evict: bool,
    ) -> Result<luna_sync::engine::OnDemand, String> {
        use tauri_specta::Event as _;
        let state = app
            .try_state::<crate::ipc::AppState>()
            .ok_or_else(|| "no_state".to_owned())?;
        let (open_id, generation) = state
            .sync_vault_identity()
            .ok_or_else(|| "sync_not_enabled".to_owned())?;
        let client = app
            .try_state::<luna_cloud::commands::CloudState>()
            .map(|s| Arc::clone(&s.0))
            .filter(|c| c.logged_in())
            .ok_or_else(|| "sync_signed_out".to_owned())?;
        let mut sessions = self.sessions.lock().await;
        let session = sessions
            .live
            .get_mut(&open_id)
            .ok_or_else(|| "sync_not_enabled".to_owned())?;
        session.handle = Some(Handle::Open { generation });
        let local = vault_local(&state, session).ok_or_else(|| "sync_not_enabled".to_owned())?;
        let ring = session
            .ring
            .as_ref()
            .ok_or_else(|| "sync_keys_missing".to_owned())?;
        let config = engine_config(&state, &client, false);
        let remote = CloudRemote::new(client);
        let engine = Engine {
            remote: &remote,
            local: &local,
            ring,
            config,
        };
        let outcome = if evict {
            engine.evict(&mut session.state, path)
        } else {
            let progress_app = app.clone();
            let progress_path = path.to_owned();
            engine
                .materialize(&mut session.state, path, &mut |done, total| {
                    let _ = SyncDownloadProgress {
                        path: progress_path.clone(),
                        done: done as f64,
                        total: total as f64,
                    }
                    .emit(&progress_app);
                })
                .await
        };
        local.refresh_cloud_only(&session.state);
        let touched = local.take_touched();
        drop(sessions);
        if !touched.is_empty() {
            crate::ipc::emit_vault_changes(app, touched);
        }
        outcome.map_err(|err| abort_code(&err).1)
    }

    /// 下一轮放行这个库的批量删除（用户在界面上确认过）。
    pub async fn confirm_bulk_delete(&self, local_vault_id: &str) {
        if let Some(session) = self.sessions.lock().await.live.get_mut(local_vault_id) {
            session.allow_bulk_delete = true;
        }
        self.nudge();
    }

    /// 单库暂停 / 继续（ADR-151 ③；落在这个库的本机同步状态里，跨重启保持）。
    ///
    /// # Errors
    /// 这个库没选中同步，或状态库写不了。
    pub async fn set_vault_paused(&self, local_vault_id: &str, paused: bool) -> Result<(), String> {
        let sessions = self.sessions.lock().await;
        let session = sessions
            .live
            .get(local_vault_id)
            .ok_or_else(|| "sync_not_enabled".to_owned())?;
        session
            .state
            .set_paused(paused)
            .map_err(|_| "sync_state_db".to_owned())?;
        drop(sessions);
        self.nudge();
        Ok(())
    }

    /// 暂停状态刚改过（单库或全部）：当场按新的暂停状态重发总览，不等下一轮——点下去界面就该
    /// 变（真机：等调度那两三秒里按钮像没反应）。恢复的库先回到「已同步」，下一轮再如实更新。
    pub async fn refresh(&self, app: &tauri::AppHandle) {
        let Ok(config_dir) = app.path().app_config_dir() else {
            return;
        };
        let pause = read_pause(&config_dir, now_ms());
        let mut sessions = self.sessions.lock().await;
        for session in sessions.live.values_mut() {
            let paused = pause.is_some() || session.state.paused().unwrap_or(false);
            if paused {
                session.phase = SyncPhase::Paused;
                session.hints = None;
            } else if session.phase == SyncPhase::Paused {
                session.phase = SyncPhase::Idle;
            }
        }
        let mut overview = self.overview_with(app, &sessions);
        overview.paused = pause.map(|p| GlobalPauseDto {
            until_ms: p.until_ms.map(|ms| ms as f64),
        });
        self.publish(app, overview);
    }

    /// 把一个已解封主密钥的库装进会话（开启或加入之后），并当场发布「同步中」。
    ///
    /// 只在绑定归此刻装着会话的那个账户时装（开启途中换了账户：交给下一轮按新账户对齐）。
    pub async fn install(
        &self,
        app: &tauri::AppHandle,
        local_vault_id: &str,
        root: PathBuf,
        remote_vault_id: Uuid,
        state: SyncState,
        ring: KeyRing,
    ) {
        let mut loaded = self.sessions.lock().await;
        let owned = match (state.binding().ok().flatten(), loaded.owner.as_ref()) {
            (Some(binding), Some(owner)) => binding.owner.same_account(owner),
            // 启动后第一轮还没跑（还没按谁装过会话）：绑定刚由此刻登录的账户落下，就按它装。
            (Some(binding), None) => {
                loaded.owner = Some(binding.owner);
                true
            }
            (None, _) => false,
        };
        if owned {
            let dir = app.path().app_config_dir().map_or_else(
                |_| {
                    std::env::temp_dir()
                        .join("lunarca-sync")
                        .join(local_vault_id)
                },
                |config| state_dir(&config, local_vault_id),
            );
            let mut session = new_session(
                local_vault_id,
                root,
                dir,
                remote_vault_id,
                state,
                Some(ring),
            );
            session.phase = SyncPhase::Syncing;
            loaded.elsewhere.remove(local_vault_id);
            loaded.live.insert(local_vault_id.to_owned(), session);
        }
        let overview = self.overview_with(app, &loaded);
        self.publish(app, overview);
        drop(loaded);
        self.nudge();
    }

    /// 停止同步：丢掉这个库的会话（钥匙串、状态目录与钉住由调用方清）。返回原来绑定的远端库。
    pub async fn detach(&self, app: &tauri::AppHandle, local_vault_id: &str) -> Option<Uuid> {
        let mut loaded = self.sessions.lock().await;
        let remote = loaded
            .live
            .remove(local_vault_id)
            .map(|s| s.remote_vault_id);
        let overview = self.overview_with(app, &loaded);
        self.publish(app, overview);
        remote
    }

    /// 云端库被删除了（数据面 410 或提示流的终结帧）：摘下会话、解除这台设备上的绑定（本地文件
    /// 不动），通知中心说一次。同一个远端库只说一次；这台设备自己删的（[`Self::expect_deletion`]）
    /// 不说。解除失败（钥匙串）就不说——绑定还在盘上，下一轮再撞一次 410 再解。
    fn drop_deleted(
        &self,
        env: &impl TickEnv,
        loaded: &mut Loaded,
        local_vault_id: &str,
        known: &[luna_vault::registry::VaultEntry],
    ) {
        let Some(session) = loaded.live.remove(local_vault_id) else {
            return;
        };
        let remote = session.remote_vault_id;
        let name = known
            .iter()
            .find(|entry| entry.path == session.root)
            .map(|entry| entry.name.clone())
            .unwrap_or_default();
        match env.release(session) {
            Ok(()) => {
                let first = self
                    .deletions_announced
                    .lock()
                    .map(|mut seen| seen.insert(remote))
                    .unwrap_or(true);
                if first {
                    env.notify_deleted(&name);
                }
            }
            // 只有稳定码（TDD-12）。
            Err(code) => eprintln!("[sync] release after remote deletion failed: {code}"),
        }
    }

    /// 提示流收到终结帧（或连上时就是 410）：与数据面 410 同一处处置，随即重发总览。这个库已经被
    /// 别的路径处置过（同一轮撞上了 410）就什么都不做。
    async fn remote_deleted_with(&self, env: &impl TickEnv, remote: Uuid) {
        let Some(input) = env.input() else {
            return;
        };
        let mut loaded = self.sessions.lock().await;
        let Some(id) = loaded
            .live
            .iter()
            .find(|(_, s)| s.remote_vault_id == remote)
            .map(|(id, _)| id.clone())
        else {
            return;
        };
        self.drop_deleted(env, &mut loaded, &id, &input.known);
        let current = self.overview();
        let pause = current.paused.as_ref().map(|p| GlobalPause {
            until_ms: p.until_ms.map(|ms| ms as i64),
        });
        let overview = build_overview(
            current.gate,
            pause,
            &input.known,
            input.open.as_ref(),
            &loaded,
            &self.cloud_sizes_snapshot(),
            &self.local_sizes_snapshot(),
        );
        env.publish(self, overview);
    }

    /// 这台设备要删除这个云端库（账户页「云端库」）：之后撞上的 410 / 终结帧不再通知「在另一台
    /// 设备上被删除」。
    pub fn expect_deletion(&self, remote: Uuid) {
        if let Ok(mut seen) = self.deletions_announced.lock() {
            seen.insert(remote);
        }
    }

    /// 删除没成，或库被恢复了：它之后若再被删除，照常说一次。
    pub fn reset_deletion_notice(&self, remote: Uuid) {
        if let Ok(mut seen) = self.deletions_announced.lock() {
            seen.remove(&remote);
        }
    }

    /// 用当前快照的准入门与暂停，按给定会话重算总览（命令改完会话后当场回显）。
    fn overview_with(&self, app: &tauri::AppHandle, sessions: &Loaded) -> SyncOverviewDto {
        let current = self.overview();
        let Some(state) = app.try_state::<crate::ipc::AppState>() else {
            return current;
        };
        let pause = current.paused.as_ref().map(|p| GlobalPause {
            until_ms: p.until_ms.map(|ms| ms as i64),
        });
        build_overview(
            current.gate,
            pause,
            &state.known_vaults(),
            state.sync_vault_identity().as_ref(),
            sessions,
            &self.cloud_sizes_snapshot(),
            &self.local_sizes_snapshot(),
        )
    }

    /// 刷新没选中的库的本地大小（过期了才重算）并写回缓存。
    /// `selected` 是选中同步的库根（调用方先取出来：会话里的 SQLite 连接不能跨 await 借用）。
    async fn refresh_local(
        &self,
        known: &[luna_vault::registry::VaultEntry],
        selected: Vec<PathBuf>,
    ) -> LocalSizes {
        let local = refresh_local_sizes(known, &selected, self.local_sizes_snapshot()).await;
        if let Ok(mut cached) = self.local_sizes.lock() {
            cached.clone_from(&local);
        }
        local
    }

    fn cloud_sizes_snapshot(&self) -> CloudSizes {
        self.cloud_sizes
            .lock()
            .map(|c| c.clone())
            .unwrap_or_default()
    }

    fn local_sizes_snapshot(&self) -> LocalSizes {
        self.local_sizes
            .lock()
            .map(|c| c.clone())
            .unwrap_or_default()
    }
}

fn new_session(
    local_vault_id: &str,
    root: PathBuf,
    dir: PathBuf,
    remote_vault_id: Uuid,
    state: SyncState,
    ring: Option<KeyRing>,
) -> Session {
    Session {
        local_vault_id: local_vault_id.to_owned(),
        root,
        dir,
        remote_vault_id,
        ring,
        hashes: HashCache::default(),
        handle: None,
        phase: SyncPhase::Idle,
        last_report: None,
        last_synced_ms: None,
        last_error: None,
        allow_bulk_delete: false,
        local_bytes: None,
        hints: None,
        seen: Arc::new(AtomicI64::new(cursor_seq(&state))),
        state,
    }
}

/// 选中同步的库根（调用方先取出来：会话里的 SQLite 连接不能跨 await 借用）。
fn live_roots(loaded: &Loaded) -> Vec<PathBuf> {
    loaded.live.values().map(|s| s.root.clone()).collect()
}

/// 会话与库清单、此刻的账户对齐（ADR-151 修订 2026-10-15）。返回账户变没变。
///
/// - 换了账户（含退出登录）：上一个账户的会话整套丢掉——绑定都还在盘上，换回来再装。
/// - 清单里的库只装此刻账户名下的绑定；连在别的账户下的只看一眼归属、记下那个账户的打码名，
///   不打开成会话，也就不会为它发任何请求。
/// - 没登录：一个会话都没有，也不去认别人的绑定。
fn reconcile(
    config_dir: &Path,
    known: &[luna_vault::registry::VaultEntry],
    owner: Option<&BindingOwner>,
    loaded: &mut Loaded,
    keys: &dyn CmkStore,
) -> bool {
    let same = match (loaded.owner.as_ref(), owner) {
        (Some(was), Some(now)) => was.same_account(now),
        (None, None) => true,
        _ => false,
    };
    if !same {
        loaded.live.clear();
        loaded.owner = owner.cloned();
    }
    loaded.elsewhere.clear();
    let Some(owner) = owner else {
        return !same;
    };
    let mut keep = std::collections::BTreeSet::new();
    for entry in known {
        let Some(id) = identity_of(entry) else {
            continue;
        };
        let path = state_dir(config_dir, &id).join("state.sqlite");
        if !path.is_file() {
            continue;
        }
        if let Some(session) = loaded.live.get_mut(&id) {
            session.root.clone_from(&entry.path);
            keep.insert(id);
            continue;
        }
        match luna_sync::state::peek_owner(&path) {
            Ok(Some(bound)) if bound.same_account(owner) => {
                // 打码规则改过（2026-10-15 硬切）：本账户再次登录时把存下来的标签按新写法重写，
                // 别的账户看到的「已连接到另一个账户（…）」随之更新。写不进只是标签旧，不影响同步。
                if let Err(err) = luna_sync::state::relabel_owner(&path, owner) {
                    eprintln!("[sync] 重写库归属标签失败：{err}");
                }
                if let Some(session) = load_session(&path, &id, entry.path.clone(), owner, keys) {
                    keep.insert(id.clone());
                    loaded.live.insert(id, session);
                }
            }
            Ok(Some(bound)) => {
                loaded.elsewhere.insert(id, BoundAccountDto::of(&bound));
            }
            // 还没绑定、旧版（启动时已清）或读不出：都不算选中同步。
            _ => {}
        }
    }
    loaded.live.retain(|id, _| keep.contains(id));
    !same
}

/// 决定这一轮用哪个句柄；库不在就标出来并跳过。返回能不能跑。
fn prepare(
    session: &mut Session,
    open: Option<&(String, u64)>,
    me: Option<&Arc<SyncService>>,
) -> bool {
    if let Some((id, generation)) = open {
        if *id == session.local_vault_id {
            // 应用打开了它：改用应用的句柄，后台句柄（含监听）随之释放。
            session.handle = Some(Handle::Open {
                generation: *generation,
            });
            return true;
        }
    }
    if matches!(session.handle, Some(Handle::Background { .. })) && session.root.is_dir() {
        return true;
    }
    if !session.root.is_dir() {
        session.handle = None;
        session.phase = SyncPhase::Unavailable;
        return false;
    }
    match Vault::open(&session.root) {
        Ok(vault) if vault.id().to_string() == session.local_vault_id => {
            let watcher = me.and_then(|service| {
                let on_change = Arc::clone(service);
                let on_asset = Arc::clone(service);
                luna_vault::watcher::watch(
                    &session.root,
                    move |_| on_change.nudge(),
                    move |_| on_asset.nudge(),
                )
                .ok()
            });
            session.handle = Some(Handle::Background {
                vault: Arc::new(Mutex::new(Some(vault))),
                _watcher: watcher,
            });
            true
        }
        // 目录还在，但已经不是这份库了（被换成别的库）：同 ADR-148 ⑤ 的哨兵，不跑。
        Ok(_) => {
            session.handle = None;
            session.phase = SyncPhase::Error;
            session.last_error = Some("sync_identity_mismatch".into());
            false
        }
        Err(_) => {
            session.handle = None;
            session.phase = SyncPhase::Unavailable;
            false
        }
    }
}

/// 钥匙串里没有：从账户托管信封取回并缓存（ADR-150 ②）。返回取到没有。
async fn fetch_keys(session: &mut Session, client: &luna_cloud::client::CloudClient) -> bool {
    match managed_keys(client, session.remote_vault_id).await {
        Ok(cmks) if !cmks.is_empty() => {
            let store = crate::secrets::cmk_store();
            if cmks
                .iter()
                .any(|cmk| store.save(session.remote_vault_id, cmk).is_err())
            {
                // 缓存不了只是下次再取一遍，本轮照常用内存里的钥匙。
                eprintln!("[sync] keychain cache failed");
            }
            session.ring = KeyRing::new(session.remote_vault_id, &cmks);
            session.ring.is_some()
        }
        Ok(_) => {
            session.phase = SyncPhase::Locked;
            false
        }
        Err(err) => {
            // 显示形如 `network: tls`：只有稳定码与类别（TDD-12）。
            eprintln!("[sync] vault keys unavailable: {err}");
            session.last_error = Some(err.code());
            session.phase = SyncPhase::Error;
            false
        }
    }
}

/// 跑这个库的一轮；返回这一轮改过的路径。
/// 这个会话用哪份库句柄：打开的那份借应用的（代号用来发现中途切了库），后台的用自己的。
fn vault_local(state: &crate::ipc::AppState, session: &Session) -> Option<VaultLocal> {
    let (vault, generation, expected) = match &session.handle {
        Some(Handle::Open { generation }) => {
            (state.vault_arc(), state.vault_generation_arc(), *generation)
        }
        Some(Handle::Background { vault, .. }) => {
            (Arc::clone(vault), Arc::new(AtomicU64::new(0)), 0)
        }
        None => return None,
    };
    Some(VaultLocal {
        vault,
        generation,
        expected_generation: expected,
        pin_owner: pin_owner(session.remote_vault_id),
        hashes: Arc::clone(&session.hashes),
        touched: Mutex::new(BTreeMap::new()),
        scanned_bytes: Mutex::new(None),
        chunk_dir: session.dir.join("chunks"),
    })
}

/// 一轮（或一次按需下载 / 释放）的引擎参数：设备级偏好此刻的值。
fn engine_config(
    state: &crate::ipc::AppState,
    client: &luna_cloud::client::CloudClient,
    allow_bulk_delete: bool,
) -> EngineConfig {
    let settings = state.settings_snapshot();
    EngineConfig {
        conflict_label: state
            .backend_copy_arc()
            .and_then(|i18n| i18n.translate("sync:conflict_copy_label"))
            .unwrap_or_else(|| "conflict copy".to_owned()),
        today: crate::ipc::civil_now().date(),
        allow_bulk_delete,
        now_ms: now_ms(),
        max_chunks_per_entry: luna_sync::engine::MAX_CHUNKS_PER_ENTRY,
        // 账户的单文件上限（权益 `sync.max_file_bytes`，ADR-156 ④；云客户端缓存，零网络）。
        max_file_bytes: client.sync_max_file_bytes(),
        recordings: luna_sync::policy::RecordingsMode::from_setting(
            &settings.get_str("sync.recordings_mode").unwrap_or_default(),
        ),
        defer_large: large_pause(state).is_some(),
    }
}

/// 跑这个库的一轮；返回这一轮改过的路径。
async fn run_round(
    state: &crate::ipc::AppState,
    session: &mut Session,
    client: &Arc<luna_cloud::client::CloudClient>,
) -> Vec<(String, bool)> {
    let Some(local) = vault_local(state, session) else {
        return Vec::new();
    };
    // 上一轮遗留的暂存与久未用上的块（下了一半、云端那份后来没了）：不让它们在磁盘上越攒越多。
    let _ = local.with("clear_stage", |v| {
        v.sync_clear_stage();
        Ok(())
    });
    prune_chunk_cache(&local.chunk_dir, now_ms());
    let remote = CloudRemote::new(Arc::clone(client));
    let allow_bulk_delete = std::mem::take(&mut session.allow_bulk_delete);
    let Some(ring) = session.ring.as_ref() else {
        return Vec::new();
    };
    let engine = Engine {
        remote: &remote,
        local: &local,
        ring,
        config: engine_config(state, client, allow_bulk_delete),
    };
    let outcome = engine.run_round(&mut session.state).await;
    if let Some(bytes) = local.scanned_bytes.lock().ok().and_then(|b| *b) {
        session.local_bytes = Some(bytes);
    }
    match outcome {
        Ok(report) => {
            session.last_synced_ms = Some(now_ms());
            session.last_error = None;
            session.last_report = Some(report);
            session.phase = SyncPhase::Idle;
        }
        Err(err) => {
            let (phase, code) = abort_code(&err);
            if phase == SyncPhase::Locked {
                session.ring = None;
            }
            // 日志只有稳定码与本地失败的「哪一步 + 哪一类」（TDD-12：不记路径、文件名与正文）。
            match &err {
                SyncAbort::Local(local) => eprintln!("[sync] round aborted: {code} ({})", local.0),
                SyncAbort::Remote(remote) => eprintln!("[sync] round aborted: {code} ({remote})"),
                _ => eprintln!("[sync] round aborted: {code}"),
            }
            session.last_error = Some(code);
            session.phase = phase;
        }
    }
    local.refresh_cloud_only(&session.state);
    local.take_touched()
}

/// 按库清单排出总览：每个已知库一行，选中的带上会话状态；附上体积与账户容量（ADR-152）。
fn build_overview(
    gate: SyncGate,
    pause: Option<GlobalPause>,
    known: &[luna_vault::registry::VaultEntry],
    open: Option<&(String, u64)>,
    loaded: &Loaded,
    cloud: &CloudSizes,
    local: &LocalSizes,
) -> SyncOverviewDto {
    let sessions = &loaded.live;
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let vaults = known
        .iter()
        .map(|entry| {
            let id = identity_of(entry);
            let session = id.as_ref().and_then(|id| sessions.get(id));
            // 连在另一个账户下：只说连在谁那儿，别的（远端库、错误、体积）一概不带。
            let elsewhere = session
                .is_none()
                .then(|| id.as_ref().and_then(|id| loaded.elsewhere.get(id)))
                .flatten();
            let is_open = matches!((open, &id), (Some((open_id, _)), Some(id)) if open_id == id);
            let report = session.and_then(|s| s.last_report.as_ref());
            let local_bytes = session
                .and_then(|s| s.local_bytes)
                .or_else(|| local.get(&entry.path).map(|(bytes, _)| *bytes));
            VaultSyncDto {
                path: entry.path.to_string_lossy().into_owned(),
                display_path: display_path(&entry.path, home.as_deref()),
                name: entry.name.clone(),
                is_open,
                phase: match (session, elsewhere) {
                    (Some(s), _) => s.phase,
                    (None, Some(_)) => SyncPhase::OtherAccount,
                    (None, None) => SyncPhase::Off,
                },
                paused: session.is_some_and(|s| s.state.paused().unwrap_or(false)),
                remote_vault_id: session.map(|s| s.remote_vault_id.to_string()),
                last_synced_ms: session.and_then(|s| s.last_synced_ms).map(|ms| ms as f64),
                last_error: session.and_then(|s| s.last_error.clone()),
                conflict_copies: report.map_or(0, |r| r.conflict_copies as u32),
                held_deletions: report.map_or(0, |r| r.held_deletions.len() as u32),
                too_large: report.map_or_else(Vec::new, |r| r.too_large.clone()),
                local_bytes: local_bytes.map(|b| b as f64),
                cloud_bytes: session
                    .and_then(|s| cloud.vault_bytes.get(&s.remote_vault_id.to_string()))
                    .copied(),
                bound_account: elsewhere.cloned(),
            }
        })
        .collect();
    SyncOverviewDto {
        gate,
        paused: pause.map(|p| GlobalPauseDto {
            until_ms: p.until_ms.map(|ms| ms as f64),
        }),
        storage: storage_of(cloud, sessions),
        read_only: cloud.grace.map(|(since, purge_after)| ReadOnlyGraceDto {
            since_ms: since as f64,
            purge_after_ms: purge_after as f64,
        }),
        large_paused: None,
        account_frozen: sessions.values().any(|s| {
            s.phase != SyncPhase::Off && s.last_report.as_ref().is_some_and(|r| r.account_frozen)
        }),
        vaults,
    }
}

/// 主目录下的路径缩写成 `~/…`；其余原样。
pub(crate) fn display_path(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Some(rest) => format!("~/{}", rest.to_string_lossy()),
        None => path.to_string_lossy().into_owned(),
    }
}

/// 账户容量：已用与上限取服务端用量里的 `sync_storage_bytes` 那一维（与账户页用量表同一个
/// 数）；缺少这一维时已用取各云端库之和，上限明确未知。云端一个库都没取到过 = `None`。
fn storage_of(cloud: &CloudSizes, sessions: &BTreeMap<String, Session>) -> Option<SyncStorageDto> {
    let linked: std::collections::HashSet<String> = sessions
        .values()
        .map(|s| s.remote_vault_id.to_string())
        .collect();
    let unlinked_bytes = cloud
        .vault_bytes
        .iter()
        .filter(|(id, _)| !linked.contains(*id))
        .map(|(_, bytes)| *bytes)
        .sum();
    let (used_bytes, alert) = match cloud.capacity {
        Some((used, _, alert)) => (used, alert),
        None if cloud.vault_bytes.is_empty() => return None,
        None => (
            cloud.vault_bytes.values().sum(),
            luna_cloud::client::UsageAlertDto::None,
        ),
    };
    let limit = match cloud.capacity {
        None => SyncStorageLimitDto::Unknown,
        Some((_, None, _)) => SyncStorageLimitDto::Unlimited,
        Some((_, Some(0.0), _)) => SyncStorageLimitDto::NotIncluded,
        Some((_, Some(bytes), _)) if bytes.is_finite() && bytes > 0.0 => {
            SyncStorageLimitDto::Limited { bytes }
        }
        Some(_) => SyncStorageLimitDto::Unknown,
    };
    Some(SyncStorageDto {
        used_bytes,
        limit,
        alert,
        unlinked_bytes,
    })
}

/// 服务端用量里同步存储容量那一维的稳定名（ADR-152）。
const CAPACITY_DIMENSION: &str = "sync_storage_bytes";

/// 取一次云端体积：各远端库的占用 + 账户容量那一维。取不到的部分沿用上一次。
async fn fetch_cloud_sizes(
    app: &tauri::AppHandle,
    client: &luna_cloud::client::CloudClient,
    previous: CloudSizes,
) -> CloudSizes {
    let mut next = previous;
    if let Ok(vaults) = client.sync_vaults().await {
        // 已删除的库不计容量（删除即不计，恢复才加回）：不进「未在本机同步的云端库」那一块。
        next.vault_bytes = vaults
            .into_iter()
            .filter(|v| !v.is_deleted())
            .map(|v| (v.id, v.bytes_stored as f64))
            .collect();
    }
    if let Ok(grace) = client.sync_grace().await {
        let ms = |raw: &str| {
            chrono::DateTime::parse_from_rfc3339(raw)
                .ok()
                .map(|t| t.timestamp_millis())
        };
        next.grace = grace.and_then(|g| Some((ms(&g.read_only_since)?, ms(&g.purge_after)?)));
    }
    // 取用量走唯一那条 `fetch_usage`：同步顺带取到的 95% 同样要提醒（PRD-13-040 ③）。
    let usage = match app.try_state::<luna_cloud::commands::AccountObserverState>() {
        Some(observer) => luna_cloud::commands::fetch_usage(client, &observer)
            .await
            .ok(),
        None => None,
    };
    if let Some(usage) = usage {
        next.capacity = usage
            .ledgers
            .iter()
            .filter(|l| l.kind == "subscription")
            .flat_map(|l| l.dimensions.iter())
            .find(|d| d.dimension == CAPACITY_DIMENSION)
            .map(|d| (d.used, d.limit, d.alert));
    }
    next
}

/// 没选中的库的本地大小：过期了才重算（在阻塞线程上逐个 stat，不打开库）。
async fn refresh_local_sizes(
    known: &[luna_vault::registry::VaultEntry],
    selected: &[PathBuf],
    previous: LocalSizes,
) -> LocalSizes {
    let stale: Vec<PathBuf> = known
        .iter()
        .map(|e| &e.path)
        .filter(|path| !selected.contains(path))
        .filter(|path| {
            previous
                .get(*path)
                .is_none_or(|(_, at)| at.elapsed() >= LOCAL_SIZE_TTL)
        })
        .cloned()
        .collect();
    let mut sizes = previous;
    sizes.retain(|path, _| known.iter().any(|e| &e.path == path));
    if stale.is_empty() {
        return sizes;
    }
    let computed = tauri::async_runtime::spawn_blocking(move || {
        stale
            .into_iter()
            .filter_map(|path| {
                luna_vault::sync_scope_bytes(&path)
                    .ok()
                    .map(|bytes| (path, (bytes, std::time::Instant::now())))
            })
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default();
    sizes.extend(computed);
    sizes
}

/// 整轮中止 → （界面阶段，稳定码）。
fn abort_code(err: &SyncAbort) -> (SyncPhase, String) {
    let code = match err {
        SyncAbort::NotBound => "sync_not_bound".to_owned(),
        SyncAbort::IdentityMismatch => "sync_identity_mismatch".to_owned(),
        SyncAbort::KeysMissing => return (SyncPhase::Locked, "sync_keys_missing".to_owned()),
        SyncAbort::LocalLooksEmpty => "sync_local_looks_empty".to_owned(),
        SyncAbort::RemoteLooksEmpty => "sync_remote_looks_empty".to_owned(),
        SyncAbort::Remote(remote) => remote.code(),
        SyncAbort::Local(_) => "sync_local_io".to_owned(),
        SyncAbort::State(_) => "sync_state_db".to_owned(),
        SyncAbort::Corrupt(_) => "sync_remote_corrupt".to_owned(),
    };
    (SyncPhase::Error, code)
}

/// 装载一份库的会话：状态库里有归 `owner` 的绑定才算选中了同步；钥匙串里有主密钥就先用上。
fn load_session(
    path: &Path,
    local_vault_id: &str,
    root: PathBuf,
    owner: &BindingOwner,
    keys: &dyn CmkStore,
) -> Option<Session> {
    let state = SyncState::open(path).ok()?;
    let binding = state.binding().ok()??;
    if !binding.owner.same_account(owner) {
        return None;
    }
    let ring = keys
        .load(binding.remote_vault_id)
        .ok()
        .and_then(|cmks| KeyRing::new(binding.remote_vault_id, &cmks));
    Some(new_session(
        local_vault_id,
        root,
        path.parent().map(Path::to_path_buf).unwrap_or_default(),
        binding.remote_vault_id,
        state,
        ring,
    ))
}

/// 删掉旧版写下、绑定没有归属账户的同步状态（启动时一次；ADR-151 修订 2026-10-15，未发布即
/// 硬切、不迁移）。返回删了几份。
///
/// 只认 `sync/<库身份>/state.sqlite` 这一种形状，且只在状态库的结构版本是旧版时删那一个库的
/// 同步目录（状态库与它的块缓存）；别的目录、别的文件、库里的文件（钉住的基线）一概不碰。
/// 状态可重建：重新开启同步走首次合并，不丢任何文件。
pub fn remove_legacy_state(config_dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(config_dir.join("sync")) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let named_after_a_vault = entry
            .file_name()
            .to_str()
            .and_then(luna_vault::VaultId::parse)
            .is_some();
        let dir = entry.path();
        let state = dir.join("state.sqlite");
        if !named_after_a_vault || !state.is_file() {
            continue;
        }
        let legacy = matches!(
            luna_sync::state::peek_owner(&state),
            Err(err) if luna_sync::state::is_legacy(&err)
        );
        if legacy && std::fs::remove_dir_all(&dir).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// 这份库能不能在 `owner` 名下开启同步（绑定排他，ADR-151 修订 2026-10-15）。返回要被替换的那份
/// 绑定：连在另一个账户下、且用户确认了「改为同步到当前账户」（`take_over`）。
///
/// # Errors
/// `sync_already_enabled`（这个账户下已经开着）、`sync_bound_elsewhere`（连在另一个账户下、没有
/// 确认改连）、`sync_state_db`（状态库读不出）。
pub fn bindable(
    state_path: &Path,
    owner: &BindingOwner,
    take_over: bool,
) -> Result<Option<Binding>, String> {
    if !state_path.is_file() {
        return Ok(None);
    }
    let binding = SyncState::open(state_path)
        .and_then(|state| state.binding())
        .map_err(|_| "sync_state_db".to_owned())?;
    match binding {
        None => Ok(None),
        Some(binding) if binding.owner.same_account(owner) => Err("sync_already_enabled".into()),
        Some(binding) if take_over => Ok(Some(binding)),
        Some(_) => Err("sync_bound_elsewhere".into()),
    }
}

/// 解除这台设备上的一份绑定：忘掉库主密钥缓存、解掉基线钉住（`unpin` 收钉住持有者名，库开着
/// 与否由调用方决定用哪个句柄）、删掉这个库的同步目录（状态库与块缓存）。云端库与本地文件都
/// 不动。钥匙串忘不掉就整件不做，免得留下没有绑定却还存着主密钥的残局。
///
/// # Errors
/// `sync_keychain`。
pub fn release_binding(
    dir: &Path,
    remote_vault_id: Uuid,
    keys: &dyn CmkStore,
    unpin: impl FnOnce(&str),
) -> Result<(), String> {
    keys.forget(remote_vault_id)
        .map_err(|_| "sync_keychain".to_owned())?;
    unpin(&pin_owner(remote_vault_id));
    let _ = std::fs::remove_dir_all(dir);
    Ok(())
}

/// 停止这台设备上一份库的同步，不论绑定归哪个账户（「停止同步」、「管理库」移除、删除云端库共用；
/// 会话由调用方先摘下，`detached` 是摘下的那个会话绑定的远端库）：读盘上的绑定，解除它
/// （[`release_binding`]）。没有可读的绑定就只清掉同步目录，不留半份状态。返回原来绑定的远端库。
/// 云端库与库里的文件一概不碰。
///
/// # Errors
/// `sync_keychain`。
pub fn release_vault(
    dir: &Path,
    detached: Option<Uuid>,
    keys: &dyn CmkStore,
    unpin: impl FnOnce(&str),
) -> Result<Option<Uuid>, String> {
    let path = dir.join("state.sqlite");
    let bound = path
        .is_file()
        .then(|| luna_sync::state::peek_binding(&path).ok().flatten())
        .flatten()
        .map(|binding| binding.remote_vault_id);
    let Some(remote) = bound.or(detached) else {
        if dir.exists() {
            let _ = std::fs::remove_dir_all(dir);
        }
        return Ok(None);
    };
    release_binding(dir, remote, keys, unpin)?;
    Ok(Some(remote))
}

/// 落一份新绑定（`dir` 是这个库的同步目录）。`replaced` 是被改连过来的另一个账户的绑定（见
/// [`bindable`]）：新绑定落下的这一刻才解除它——向导取消、建库或取钥失败都走不到这里，原绑定
/// 原样留着。
///
/// # Errors
/// `sync_keychain`、`sync_state_db`。
pub fn bind_local(
    dir: &Path,
    binding: &Binding,
    replaced: Option<&Binding>,
    keys: &dyn CmkStore,
    unpin: impl FnOnce(&str),
) -> Result<SyncState, String> {
    if let Some(old) = replaced {
        // 新旧不会是同一个云端库（不同账户的库）；万一是，也不能把刚存下的主密钥忘掉。
        if old.remote_vault_id == binding.remote_vault_id {
            let _ = std::fs::remove_dir_all(dir);
        } else {
            release_binding(dir, old.remote_vault_id, keys, unpin)?;
        }
    }
    std::fs::create_dir_all(dir).map_err(|_| "sync_state_db".to_owned())?;
    let mut state =
        SyncState::open(&dir.join("state.sqlite")).map_err(|_| "sync_state_db".to_owned())?;
    state
        .bind(binding)
        .map_err(|_| "sync_state_db".to_owned())?;
    Ok(state)
}

/// 「管理库」移除一个库之前，它在这台设备上的同步绑定（确认框据此说清移除会顺带断开什么；同步库
/// 归属与移除方案 三.B）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VaultSyncLinkDto {
    /// 没有同步绑定：移除只删库清单那一行。
    None,
    /// 此刻登录的账户正同步它：移除后这台设备停止同步它，云端副本保留。
    Syncing,
    /// 连在别的账户下（或此刻没登录）：移除时这份连接一并解除。
    Linked { account: BoundAccountDto },
}

/// 一个库在这台设备上的同步绑定（零网络）。`owner` 是此刻登录的账户（没登录 = `None`）。
#[must_use]
pub fn sync_link_of(
    config_dir: &Path,
    entry: &luna_vault::registry::VaultEntry,
    owner: Option<&BindingOwner>,
) -> VaultSyncLinkDto {
    let Some(id) = identity_of(entry) else {
        return VaultSyncLinkDto::None;
    };
    match luna_sync::state::peek_owner(&state_dir(config_dir, &id).join("state.sqlite")) {
        Ok(Some(bound)) if owner.is_some_and(|owner| bound.same_account(owner)) => {
            VaultSyncLinkDto::Syncing
        }
        Ok(Some(bound)) => VaultSyncLinkDto::Linked {
            account: BoundAccountDto::of(&bound),
        },
        _ => VaultSyncLinkDto::None,
    }
}

/// 这台设备上归 `owner` 的绑定：远端库 id → 库注册表里的那一行（零网络）。只认注册表里的库——
/// 从「管理库」移除过的库即使盘上还留着同步目录（此前的版本移除时不解绑），也不算在这台设备上。
#[must_use]
pub fn linked_here(
    config_dir: &Path,
    known: &[luna_vault::registry::VaultEntry],
    owner: &BindingOwner,
) -> HashMap<Uuid, luna_vault::registry::VaultEntry> {
    known
        .iter()
        .filter_map(|entry| {
            let path = state_dir(config_dir, &identity_of(entry)?).join("state.sqlite");
            // 只读看一眼：会话可能正开着这份状态库。
            let binding = luna_sync::state::peek_binding(&path).ok()??;
            binding
                .owner
                .same_account(owner)
                .then(|| (binding.remote_vault_id, entry.clone()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::io::Read as _;

    use luna_sync_core::manifest::content_hash;

    use super::*;

    /// 只读宽限的提醒（PRD-13 §4）：开始一次，14 / 7 / 1 天与当天各一次；同一轮不重复，换一轮重来。
    #[test]
    fn read_only_reminders_fire_once_per_milestone() {
        let purge = 100 * DAY_MS;
        let at = |days_left: f64| purge - (days_left * DAY_MS as f64) as i64;
        assert_eq!(grace_milestone(purge, at(30.0)), GRACE_START);
        assert_eq!(grace_milestone(purge, at(14.5)), 14);
        assert_eq!(grace_milestone(purge, at(10.0)), 14);
        assert_eq!(grace_milestone(purge, at(5.0)), 7);
        assert_eq!(grace_milestone(purge, at(1.5)), 1);
        assert_eq!(grace_milestone(purge, at(0.5)), 0);
        assert_eq!(grace_milestone(purge, purge + DAY_MS), 0, "过期也按当天");

        let mut state = GraceReminders::default();
        let fired: Vec<Option<i64>> = [30.0, 29.0, 13.0, 12.0, 6.0, 0.9, 0.2]
            .into_iter()
            .map(|d| next_grace_reminder(&mut state, purge, at(d)))
            .collect();
        assert_eq!(
            fired,
            [
                Some(GRACE_START),
                None,
                Some(14),
                None,
                Some(7),
                Some(0),
                None
            ]
        );
        // 恢复权益后又失去：新的一轮宽限从头提醒。
        assert_eq!(
            next_grace_reminder(&mut state, purge + 50 * DAY_MS, at(0.2)),
            Some(GRACE_START)
        );
    }

    /// 发布去重：与上一份完全相同的总览不再广播；任何一处真实变化（库的阶段、闸门 / 账户、
    /// 冻结）照常广播，且存下的是最新那份（命令取到的与界面收到的同一份）。
    #[test]
    fn publish_skips_identical_overview_and_emits_real_changes() {
        let service = SyncService::default();
        let emitted = std::cell::RefCell::new(Vec::<SyncOverviewDto>::new());
        let publish = |overview: SyncOverviewDto| {
            service.publish_with(overview, |sent| emitted.borrow_mut().push(sent));
        };
        let vault = |phase: SyncPhase| VaultSyncDto {
            path: "/v/a".into(),
            display_path: "~/a".into(),
            name: "a".into(),
            is_open: true,
            phase,
            paused: false,
            remote_vault_id: Some("r".into()),
            last_synced_ms: Some(1.0),
            last_error: None,
            conflict_copies: 0,
            held_deletions: 0,
            too_large: Vec::new(),
            local_bytes: Some(42.0),
            cloud_bytes: None,
            bound_account: None,
        };
        let syncing = SyncOverviewDto {
            vaults: vec![vault(SyncPhase::Syncing)],
            ..SyncOverviewDto::empty()
        };

        publish(syncing.clone());
        publish(syncing.clone());
        assert_eq!(emitted.borrow().len(), 1, "一字不差的第二次不广播");

        publish(SyncOverviewDto {
            vaults: vec![vault(SyncPhase::Idle)],
            ..SyncOverviewDto::empty()
        });
        publish(SyncOverviewDto {
            gate: SyncGate::SignedOut,
            vaults: vec![vault(SyncPhase::Idle)],
            ..SyncOverviewDto::empty()
        });
        publish(SyncOverviewDto {
            gate: SyncGate::SignedOut,
            account_frozen: true,
            vaults: vec![vault(SyncPhase::Idle)],
            ..SyncOverviewDto::empty()
        });
        assert_eq!(
            emitted.borrow().len(),
            4,
            "阶段、闸门、冻结各变一次，各广播一次"
        );
        assert_eq!(service.overview(), emitted.borrow()[3], "存下的是最新那份");

        // 与初始空总览相同的那份也不广播：命令取到的本来就是它。
        let fresh = SyncService::default();
        let mut sent = 0;
        fresh.publish_with(SyncOverviewDto::empty(), |_| sent += 1);
        assert_eq!(sent, 0);
    }

    fn local(dir: &Path) -> (VaultLocal, Arc<AtomicU64>) {
        let vault = Vault::open(dir).unwrap();
        let generation = Arc::new(AtomicU64::new(1));
        (
            VaultLocal {
                vault: Arc::new(Mutex::new(Some(vault))),
                generation: Arc::clone(&generation),
                expected_generation: 1,
                pin_owner: pin_owner(Uuid::from_u128(1)),
                hashes: HashCache::default(),
                touched: Mutex::new(BTreeMap::new()),
                scanned_bytes: Mutex::new(None),
                chunk_dir: dir.join(".test-chunks"),
            },
            generation,
        )
    }

    /// 大文件的本机一侧（B1）：拼好的暂存按现状核对后换上、用掉；丢弃不留残留；块缓存跨轮可取、
    /// 落地后清掉；越出目录的名字一律不收。
    #[test]
    fn large_files_are_staged_swapped_in_and_their_chunks_cached() {
        let dir = tempfile::tempdir().unwrap();
        let (local, _) = local(dir.path());
        let staged = local
            .stage(&mut |out| {
                out.write_all(b"first half ").unwrap();
                out.write_all(b"second half").unwrap();
                Ok(())
            })
            .unwrap();
        assert_eq!(staged.size, 22);
        assert_eq!(staged.content_hash, content_hash(b"first half second half"));
        assert_eq!(
            local
                .write_staged_if("rec.wav", &staged, &Expect::Absent)
                .unwrap(),
            WriteOutcome::Done
        );
        assert_eq!(
            std::fs::read(dir.path().join("rec.wav")).unwrap(),
            b"first half second half"
        );
        let stage_dir = dir.path().join(".luna/tmp");
        assert_eq!(
            std::fs::read_dir(&stage_dir).unwrap().count(),
            0,
            "暂存已用掉"
        );

        // 现状不符：不换、暂存留着；丢弃后不留残留。
        let other = local
            .stage(&mut |out| {
                out.write_all(b"x").unwrap();
                Ok(())
            })
            .unwrap();
        assert_eq!(
            local
                .write_staged_if("rec.wav", &other, &Expect::Absent)
                .unwrap(),
            WriteOutcome::Changed
        );
        local.discard(&other);
        assert_eq!(std::fs::read_dir(&stage_dir).unwrap().count(), 0);

        let id = "ab".repeat(32);
        local.cache_chunk(&id, b"plain").unwrap();
        assert_eq!(local.cached_chunk(&id).as_deref(), Some(&b"plain"[..]));
        local.forget_chunks(std::slice::from_ref(&id));
        assert!(local.cached_chunk(&id).is_none());
        assert!(local.cache_chunk("../escape", b"x").is_err());
        assert!(local.cached_chunk("../escape").is_none());

        // 释放本机空间：内容对得上才移除（不进回收站由引擎用例对内存库断言）。
        assert_eq!(
            local.evict_if("rec.wav", "not-the-hash").unwrap(),
            WriteOutcome::Changed
        );
        let copy = local
            .stage(&mut |out| {
                out.write_all(b"evict me").unwrap();
                Ok(())
            })
            .unwrap();
        local
            .write_staged_if("gone.wav", &copy, &Expect::Absent)
            .unwrap();
        assert_eq!(
            local
                .evict_if("gone.wav", &content_hash(b"evict me"))
                .unwrap(),
            WriteOutcome::Done
        );
        assert!(!dir.path().join("gone.wav").exists());

        let mut reader = local.open("rec.wav").unwrap();
        let mut back = Vec::new();
        reader.read_to_end(&mut back).unwrap();
        assert_eq!(back, b"first half second half");
    }

    #[test]
    fn the_local_side_reads_writes_and_records_what_it_touched() {
        let dir = tempfile::tempdir().unwrap();
        let (local, _) = local(dir.path());
        assert_eq!(
            local.write_if("a.md", b"# A\n", &Expect::Absent).unwrap(),
            WriteOutcome::Done
        );
        assert_eq!(
            local.write_if("a.md", b"x", &Expect::Absent).unwrap(),
            WriteOutcome::Changed
        );
        let listed = local.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].content_hash, content_hash(b"# A\n"));
        // 内容不变、大小与时间也没变：第二次列举直接用缓存（结果一样）。
        assert_eq!(local.list().unwrap(), listed);
        assert_eq!(
            local
                .rename_if("a.md", "b.md", &content_hash(b"# A\n"))
                .unwrap(),
            WriteOutcome::Done
        );
        assert_eq!(
            local.delete_if("b.md", &content_hash(b"# A\n")).unwrap(),
            WriteOutcome::Done
        );
        assert_eq!(
            local.take_touched(),
            vec![("a.md".to_owned(), true), ("b.md".to_owned(), true)]
        );
        let pin = local.pin(b"base").unwrap();
        assert_eq!(local.read_pinned(&pin).unwrap(), b"base");
        assert!(local.vault_id().is_some());
    }

    #[test]
    fn a_timed_global_pause_resumes_by_itself_and_an_open_ended_one_does_not() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_pause(dir.path(), 1_000), None);
        write_pause(dir.path(), Some(Some(5_000))).unwrap();
        assert_eq!(
            read_pause(dir.path(), 1_000),
            Some(GlobalPause {
                until_ms: Some(5_000)
            })
        );
        // 到点：自动恢复，文件随之删掉。
        assert_eq!(read_pause(dir.path(), 5_000), None);
        assert!(!dir.path().join(PAUSE_FILE).exists());
        // 直到我恢复：多久都不会自己恢复。
        write_pause(dir.path(), Some(None)).unwrap();
        assert_eq!(
            read_pause(dir.path(), i64::MAX),
            Some(GlobalPause { until_ms: None })
        );
        write_pause(dir.path(), None).unwrap();
        assert_eq!(read_pause(dir.path(), 0), None);
        // 恢复两次也不报错。
        write_pause(dir.path(), None).unwrap();
    }

    fn entry(path: &Path, name: &str, id: Option<&str>) -> luna_vault::registry::VaultEntry {
        luna_vault::registry::VaultEntry {
            path: path.to_path_buf(),
            name: name.into(),
            last_opened_ms: 0,
            id: id.map(str::to_owned),
            accounts: Vec::new(),
        }
    }

    #[test]
    fn the_overview_lists_every_known_vault_and_marks_the_selected_ones() {
        let a_id = "0190f0f0-0000-7000-8000-00000000000a";
        let b_id = "0190f0f0-0000-7000-8000-00000000000b";
        let d_id = "0190f0f0-0000-7000-8000-00000000000d";
        let known = vec![
            entry(Path::new("/v/a"), "工作", Some(a_id)),
            entry(Path::new("/v/b"), "读书", Some(b_id)),
            entry(Path::new("/v/c"), "从没打开过", None),
            entry(Path::new("/v/d"), "家里", Some(d_id)),
        ];
        let mut state = SyncState::in_memory().unwrap();
        state
            .bind(&luna_sync::state::Binding {
                remote_vault_id: Uuid::from_u128(9),
                server_origin: "https://x".into(),
                local_vault_id: b_id.into(),
                created_ms: 1,
                owner: owner("acc-a"),
            })
            .unwrap();
        state.set_paused(true).unwrap();
        let mut session = new_session(
            b_id,
            "/v/b".into(),
            "/tmp/sync-b".into(),
            Uuid::from_u128(9),
            state,
            None,
        );
        session.phase = SyncPhase::Paused;
        let sessions = Loaded {
            owner: Some(owner("acc-a")),
            live: BTreeMap::from([(b_id.to_owned(), session)]),
            elsewhere: BTreeMap::from([(
                d_id.to_owned(),
                BoundAccountDto {
                    label: "jo***@example.test".to_owned(),
                    server_host: "10.0.0.62:8443".to_owned(),
                    server_official: false,
                },
            )]),
        };
        let open = (a_id.to_owned(), 3);
        let cloud = CloudSizes::default();
        let local = LocalSizes::new();
        let overview = build_overview(
            SyncGate::Ready,
            None,
            &known,
            Some(&open),
            &sessions,
            &cloud,
            &local,
        );
        let rows: Vec<_> = overview
            .vaults
            .iter()
            .map(|v| (v.name.as_str(), v.phase, v.is_open, v.paused))
            .collect();
        assert_eq!(
            rows,
            vec![
                ("工作", SyncPhase::Off, true, false),
                ("读书", SyncPhase::Paused, false, true),
                ("从没打开过", SyncPhase::Off, false, false),
                ("家里", SyncPhase::OtherAccount, false, false),
            ]
        );
        // 连在另一个账户下：只说连在谁那儿，不带远端库与错误。
        assert_eq!(
            overview.vaults[3]
                .bound_account
                .as_ref()
                .map(|a| a.label.as_str()),
            Some("jo***@example.test")
        );
        // 同一个邮箱可能在两台服务器上各有一个账户：连在哪台服务器上一并下发。
        assert_eq!(
            overview.vaults[3]
                .bound_account
                .as_ref()
                .map(|a| (a.server_host.as_str(), a.server_official)),
            Some(("10.0.0.62:8443", false))
        );
        assert_eq!(overview.vaults[3].remote_vault_id, None);
        assert_eq!(overview.vaults[1].bound_account, None);
        assert_eq!(
            overview.vaults[1].remote_vault_id.as_deref(),
            Some(Uuid::from_u128(9).to_string().as_str())
        );
        let paused = build_overview(
            SyncGate::Ready,
            Some(GlobalPause { until_ms: Some(7) }),
            &known,
            None,
            &sessions,
            &cloud,
            &local,
        );
        assert_eq!(paused.paused.and_then(|p| p.until_ms), Some(7.0));
    }

    #[test]
    fn capacity_comes_from_the_server_dimension_and_names_the_unlinked_part() {
        use luna_cloud::client::UsageAlertDto as L;
        let linked = Uuid::from_u128(9);
        let session = new_session(
            "v",
            "/v".into(),
            "/tmp/sync-v".into(),
            linked,
            SyncState::in_memory().unwrap(),
            None,
        );
        let sessions = BTreeMap::from([("v".to_owned(), session)]);
        // 一个云端库都没取到过：不显示容量。
        assert!(storage_of(&CloudSizes::default(), &sessions).is_none());

        let mut cloud = CloudSizes {
            vault_bytes: HashMap::from([
                (linked.to_string(), 300.0),
                (Uuid::from_u128(7).to_string(), 500.0),
            ]),
            capacity: None,
            grace: None,
        };
        // 缺失容量维度不等于不限。
        let fallback = storage_of(&cloud, &sessions).unwrap();
        assert_eq!(fallback.used_bytes, 800.0);
        assert!(matches!(fallback.limit, SyncStorageLimitDto::Unknown));
        assert_eq!(
            fallback.unlinked_bytes, 500.0,
            "这台设备上没有对应本地库的那一块"
        );
        assert_eq!(
            fallback.alert,
            L::None,
            "那一维取不到就没有档位可言，不在本地比阈值"
        );

        // 有容量：已用与档位都取服务端那一维（同账户页用量表一个数、一份判定）。
        cloud.capacity = Some((850.0, Some(1000.0), L::P80));
        let warn = storage_of(&cloud, &sessions).unwrap();
        assert_eq!(warn.used_bytes, 850.0);
        assert_eq!(warn.alert, L::P80);
        cloud.capacity = Some((1000.0, Some(1000.0), L::Exhausted));
        assert_eq!(storage_of(&cloud, &sessions).unwrap().alert, L::Exhausted);
        // 服务端说 none 就是 none，哪怕比例看着很高——判定只有一处。
        cloud.capacity = Some((990.0, Some(1000.0), L::None));
        assert_eq!(storage_of(&cloud, &sessions).unwrap().alert, L::None);
        // 上限 0（不含同步）。
        cloud.capacity = Some((0.0, Some(0.0), L::None));
        assert_eq!(storage_of(&cloud, &sessions).unwrap().alert, L::None);
        assert!(matches!(
            storage_of(&cloud, &sessions).unwrap().limit,
            SyncStorageLimitDto::NotIncluded
        ));
        cloud.capacity = Some((800.0, None, L::None));
        assert!(matches!(
            storage_of(&cloud, &sessions).unwrap().limit,
            SyncStorageLimitDto::Unlimited
        ));
        cloud.capacity = Some((800.0, Some(1000.0), L::None));
        assert!(matches!(
            storage_of(&cloud, &sessions).unwrap().limit,
            SyncStorageLimitDto::Limited { bytes: 1000.0 }
        ));
    }

    #[test]
    fn paths_under_home_are_shown_with_a_tilde() {
        let home = Path::new("/Users/me");
        assert_eq!(
            display_path(Path::new("/Users/me/Documents/LunaVault"), Some(home)),
            "~/Documents/LunaVault"
        );
        assert_eq!(display_path(Path::new("/Users/me"), Some(home)), "~");
        assert_eq!(
            display_path(Path::new("/Volumes/Disk/Notes"), Some(home)),
            "/Volumes/Disk/Notes"
        );
        // 只按路径段比，`/Users/meow` 不是 `/Users/me` 的子目录。
        assert_eq!(
            display_path(Path::new("/Users/meow/x"), Some(home)),
            "/Users/meow/x"
        );
        assert_eq!(display_path(Path::new("/a"), None), "/a");
    }

    #[test]
    fn an_unselected_vault_is_sized_without_being_opened() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.md"), b"12345").unwrap();
        std::fs::create_dir(dir.path().join(".trash")).unwrap();
        std::fs::write(dir.path().join(".trash/x.md"), b"ignored").unwrap();
        assert_eq!(luna_vault::sync_scope_bytes(dir.path()).unwrap(), 5);
        assert!(
            !dir.path().join(".luna").exists(),
            "算大小不打开库、不写 .luna/"
        );
    }

    #[test]
    fn a_vault_never_opened_by_this_version_is_identified_from_its_own_file() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(identity_of(&entry(dir.path(), "x", None)), None);
        let vault = Vault::open(dir.path()).unwrap();
        let id = vault.id().to_string();
        drop(vault);
        assert_eq!(identity_of(&entry(dir.path(), "x", None)), Some(id));
        // 注册表登记过的优先（不读盘）。
        assert_eq!(
            identity_of(&entry(Path::new("/nope"), "x", Some("given"))).as_deref(),
            Some("given")
        );
    }

    #[test]
    fn a_vault_switch_mid_round_fails_every_later_operation() {
        let dir = tempfile::tempdir().unwrap();
        let (local, generation) = local(dir.path());
        generation.store(2, Ordering::SeqCst);
        assert!(local.list().is_err());
        assert!(local.write_if("a.md", b"x", &Expect::Absent).is_err());
        assert!(local.vault_id().is_none());
        assert!(matches!(
            local.absence("a.md"),
            luna_vault::absence::Absence::Unknown(_)
        ));
        assert!(!dir.path().join("a.md").exists(), "切库之后一个字节都没写");
    }

    // ── 绑定归账户（ADR-151 修订 2026-10-15）────────────────────────────────────────

    /// 测试里的云客户端都连默认服务器，归属也记在它名下。
    /// 夹具账户都在本构建的 Lunarca 云上（`owner` 的分桶）。
    fn official_account(label: &str) -> BoundAccountDto {
        BoundAccountDto {
            label: label.to_owned(),
            server_host: luna_cloud::config::server_bucket(luna_cloud::config::DEFAULT_API_BASE),
            server_official: true,
        }
    }

    fn owner(account_id: &str) -> BindingOwner {
        BindingOwner {
            server_bucket: luna_cloud::config::server_bucket(luna_cloud::config::DEFAULT_API_BASE),
            account_id: account_id.into(),
            account_label: format!("{}***@example.test", &account_id[..1]),
        }
    }

    /// 测试用的凭据存储：同一份内存存储，客户端拿一份、测试手里留一份（好写登录态）。
    #[derive(Clone, Default)]
    struct SharedStore(Arc<luna_cloud::store::MemoryStore>);

    impl luna_cloud::store::CredentialStore for SharedStore {
        fn get(&self, key: &str) -> Result<Option<String>, luna_cloud::error::CloudError> {
            self.0.get(key)
        }
        fn set(&self, key: &str, value: &str) -> Result<(), luna_cloud::error::CloudError> {
            self.0.set(key, value)
        }
        fn delete(&self, key: &str) -> Result<(), luna_cloud::error::CloudError> {
            self.0.delete(key)
        }
    }

    /// 登录成某个账户的云客户端；每一次出站请求都会先过严格本地探针，探针在这里只计数（并当作
    /// 严格本地拦下，绝不真发）。
    fn signed_in(account_id: &str, email: &str) -> (Arc<CloudClient>, Arc<AtomicU64>) {
        use luna_cloud::store::CredentialStore as _;
        let requests = Arc::new(AtomicU64::new(0));
        let counter = Arc::clone(&requests);
        let store = SharedStore::default();
        let client = CloudClient::new(
            Box::new(store.clone()),
            Arc::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
                true
            }),
        );
        let bucket = client.server_bucket();
        store
            .set(&format!("{bucket}/{}", luna_cloud::store::KEY_REFRESH), "r")
            .unwrap();
        store
            .set(
                &format!("{bucket}/{}", luna_cloud::store::KEY_PROFILE),
                &serde_json::json!({ "account_id": account_id, "email": email }).to_string(),
            )
            .unwrap();
        (Arc::new(client), requests)
    }

    /// 只记账的环境：网络动作一律不真发，只记下为哪个库调过。
    struct FakeEnv {
        config_dir: PathBuf,
        known: Vec<luna_vault::registry::VaultEntry>,
        client: Mutex<Option<Arc<CloudClient>>>,
        keys: Arc<luna_cloud::store::MemoryStore>,
        rounds: Mutex<Vec<String>>,
        key_fetches: Mutex<Vec<String>>,
        size_fetches: AtomicU64,
        published: Mutex<Vec<SyncOverviewDto>>,
        /// 跑某一轮时顺手做的事（模拟这一轮途中换了账户）。
        during_round: Mutex<Option<Box<dyn FnOnce() + Send>>>,
        /// 这些远端库在服务端已被删除：取钥与跑一轮都回 410 `vault_deleted`。
        deleted_remotes: Mutex<std::collections::HashSet<Uuid>>,
        /// 通知中心收到的「云端库已被删除」（库名）。
        notices: Mutex<Vec<String>>,
    }

    impl FakeEnv {
        fn new(config_dir: &Path, known: Vec<luna_vault::registry::VaultEntry>) -> Self {
            Self {
                config_dir: config_dir.to_path_buf(),
                known,
                client: Mutex::new(None),
                keys: Arc::new(luna_cloud::store::MemoryStore::new()),
                rounds: Mutex::new(Vec::new()),
                key_fetches: Mutex::new(Vec::new()),
                size_fetches: AtomicU64::new(0),
                published: Mutex::new(Vec::new()),
                during_round: Mutex::new(None),
                deleted_remotes: Mutex::new(std::collections::HashSet::new()),
                notices: Mutex::new(Vec::new()),
            }
        }

        fn delete_remote(&self, remote: u128) {
            self.deleted_remotes
                .lock()
                .unwrap()
                .insert(Uuid::from_u128(remote));
        }

        fn deleted(&self, session: &mut Session) -> bool {
            let gone = self
                .deleted_remotes
                .lock()
                .unwrap()
                .contains(&session.remote_vault_id);
            if gone {
                session.phase = SyncPhase::Error;
                session.last_error = Some("vault_deleted".into());
            }
            gone
        }

        fn notices(&self) -> Vec<String> {
            self.notices.lock().unwrap().clone()
        }

        fn sign_in(&self, client: Option<Arc<CloudClient>>) {
            *self.client.lock().unwrap() = client;
        }

        fn rounds(&self) -> Vec<String> {
            self.rounds.lock().unwrap().clone()
        }

        fn key_fetches(&self) -> usize {
            self.key_fetches.lock().unwrap().len()
        }

        fn last(&self) -> SyncOverviewDto {
            self.published.lock().unwrap().last().cloned().unwrap()
        }

        fn publish_count(&self) -> usize {
            self.published.lock().unwrap().len()
        }
    }

    impl TickEnv for FakeEnv {
        fn input(&self) -> Option<TickInput> {
            Some(TickInput {
                config_dir: self.config_dir.clone(),
                known: self.known.clone(),
                open: None,
                strict_local: false,
                client: self.client.lock().unwrap().clone(),
            })
        }

        fn keys(&self) -> CredentialCmkStore {
            CredentialCmkStore::new(Arc::clone(&self.keys) as Arc<_>)
        }

        fn waker(&self) -> Option<Arc<SyncService>> {
            None
        }

        async fn fetch_keys(&self, session: &mut Session, _client: &Arc<CloudClient>) -> bool {
            self.key_fetches
                .lock()
                .unwrap()
                .push(session.local_vault_id.clone());
            !self.deleted(session)
        }

        async fn run_round(
            &self,
            session: &mut Session,
            _client: &Arc<CloudClient>,
        ) -> Vec<(String, bool)> {
            self.rounds
                .lock()
                .unwrap()
                .push(session.local_vault_id.clone());
            session.phase = SyncPhase::Idle;
            self.deleted(session);
            if let Some(hook) = self.during_round.lock().unwrap().take() {
                hook();
            }
            Vec::new()
        }

        async fn cloud_sizes(
            &self,
            _client: &Arc<CloudClient>,
            previous: CloudSizes,
        ) -> CloudSizes {
            self.size_fetches.fetch_add(1, Ordering::SeqCst);
            previous
        }

        fn remind_read_only(&self, _owner: &BindingOwner, _grace: Option<(i64, i64)>) {}

        fn watch(&self, _session: &Session, _client: &Arc<CloudClient>) -> Option<HintWatch> {
            None
        }

        fn release(&self, session: Session) -> Result<(), String> {
            let (root, dir, remote) = (
                session.root.clone(),
                session.dir.clone(),
                session.remote_vault_id,
            );
            drop(session);
            release_binding(&dir, remote, &self.keys(), |holder| {
                if let Ok(vault) = Vault::open(&root) {
                    let _ = vault.sync_unpin_all(holder);
                }
            })
        }

        fn notify_deleted(&self, name: &str) {
            self.notices.lock().unwrap().push(name.to_owned());
        }

        fn publish(&self, service: &SyncService, overview: SyncOverviewDto) {
            service.publish_with(overview, |sent| self.published.lock().unwrap().push(sent));
        }

        fn vault_changes(&self, _touched: Vec<(String, bool)>) {}
    }

    /// 一份真的库（有身份）+ 它在注册表里的那一行。
    fn vault_on_disk(dir: &Path) -> (String, luna_vault::registry::VaultEntry) {
        let id = Vault::open(dir).unwrap().id().to_string();
        let entry = entry(dir, "笔记", Some(&id));
        (id, entry)
    }

    fn binding_for(id: &str, remote: u128, owner: BindingOwner) -> Binding {
        Binding {
            remote_vault_id: Uuid::from_u128(remote),
            server_origin: "https://sync.example.test".into(),
            local_vault_id: id.into(),
            created_ms: 1,
            owner,
        }
    }

    /// 在 `owner` 名下落一份绑定，带一条基线与一块块缓存。
    fn bind_with_baseline(config: &Path, id: &str, remote: u128, owner: BindingOwner) {
        let dir = state_dir(config, id);
        let keys = CredentialCmkStore::new(Arc::new(luna_cloud::store::MemoryStore::new()));
        let mut state =
            bind_local(&dir, &binding_for(id, remote, owner), None, &keys, |_| {}).unwrap();
        state
            .commit_round(
                &[luna_sync::state::BaseChange::Upsert(
                    luna_sync::state::BaseFile {
                        file_id: Uuid::from_u128(1),
                        path: "a.md".into(),
                        rev: 1,
                        content_hash: "a".repeat(64),
                        pin: None,
                        deleted: false,
                        cloud_only: false,
                    },
                )],
                Some(luna_sync::state::Cursor {
                    epoch: Uuid::from_u128(5),
                    seq: 3,
                }),
            )
            .unwrap();
        std::fs::create_dir_all(dir.join("chunks")).unwrap();
        std::fs::write(dir.join("chunks").join("abc"), b"chunk").unwrap();
    }

    fn baseline_of(config: &Path, id: &str) -> usize {
        SyncState::open(&state_dir(config, id).join("state.sqlite"))
            .unwrap()
            .bases()
            .unwrap()
            .len()
    }

    /// 绑定落盘时记下归属；A 同步 → 退出（不留会话、不跑）→ B 登录（显示「已连接到另一个账户」、
    /// 为它一个请求都不发、不报错）→ 换回 A（自动接着同步，基线还在）。
    #[test]
    fn a_binding_follows_its_account_across_sign_out_and_account_switch() {
        let config = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let (id, entry) = vault_on_disk(root.path());
        bind_with_baseline(config.path(), &id, 9, owner("acc-a"));
        assert_eq!(
            luna_sync::state::peek_owner(&state_dir(config.path(), &id).join("state.sqlite"))
                .unwrap(),
            Some(owner("acc-a")),
            "归属随绑定落盘"
        );
        let service = SyncService::default();
        let env = FakeEnv::new(config.path(), vec![entry]);
        let run = |f: std::pin::Pin<Box<dyn Future<Output = ()> + '_>>| {
            tauri::async_runtime::block_on(f);
        };

        // A 登录：装上会话、跑一轮。
        let (a, _) = signed_in("acc-a", "alice@example.test");
        env.sign_in(Some(a));
        run(Box::pin(service.tick_with(&env)));
        assert_eq!(env.rounds(), vec![id.clone()]);
        assert_eq!(env.last().vaults[0].phase, SyncPhase::Idle);

        // 退出：当场一个会话都不剩，不跑、不取钥。
        env.sign_in(None);
        service.forget_account();
        run(Box::pin(service.realign_with(&env)));
        let out = env.last();
        assert_eq!(out.gate, SyncGate::SignedOut);
        assert_eq!(out.vaults[0].phase, SyncPhase::Off);
        assert_eq!(out.vaults[0].bound_account, None);
        assert!(tauri::async_runtime::block_on(service.sessions.lock())
            .live
            .is_empty());
        run(Box::pin(service.tick_with(&env)));
        assert_eq!(env.rounds().len(), 1, "未登录不跑");
        assert_eq!(env.key_fetches(), 1);

        // B 登录：A 的库显示为连在另一个账户下；为它一个请求都没发、没有错误。
        let (b, b_requests) = signed_in("acc-b", "bob@example.test");
        env.sign_in(Some(b));
        service.forget_account();
        run(Box::pin(service.realign_with(&env)));
        run(Box::pin(service.tick_with(&env)));
        let out = env.last();
        assert_eq!(out.gate, SyncGate::Ready);
        let row = &out.vaults[0];
        assert_eq!(row.phase, SyncPhase::OtherAccount);
        // A 登录那一轮已按新的打码写法重写了存下来的标签（`relabel_owner`）。
        assert_eq!(
            row.bound_account.as_ref().map(|a| a.label.as_str()),
            Some("al…ce@example.test")
        );
        assert_eq!(row.last_error, None);
        assert_eq!(row.remote_vault_id, None);
        assert_eq!(env.rounds().len(), 1, "不为另一个账户的库跑同步");
        assert_eq!(env.key_fetches(), 1, "不为另一个账户的库取钥");
        assert_eq!(
            b_requests.load(Ordering::SeqCst),
            0,
            "B 的客户端一个请求都没发"
        );
        assert_eq!(
            luna_sync::state::peek_owner(&state_dir(config.path(), &id).join("state.sqlite"))
                .unwrap(),
            Some(BindingOwner {
                account_label: "al…ce@example.test".into(),
                ..owner("acc-a")
            }),
            "A 的绑定原样留着（只是标签按新写法重写了）"
        );

        // 换回 A：不走向导，接着同步，基线还在。
        let (a, _) = signed_in("acc-a", "alice@example.test");
        env.sign_in(Some(a));
        service.forget_account();
        run(Box::pin(service.tick_with(&env)));
        assert_eq!(env.rounds(), vec![id.clone(), id.clone()]);
        assert_eq!(baseline_of(config.path(), &id), 1, "基线还在");
        assert!(state_dir(config.path(), &id).join("chunks/abc").exists());
        assert_eq!(env.last().vaults[0].phase, SyncPhase::Idle);
    }

    /// 一轮途中换了账户：剩下的库不再跑，上一个账户的总览不再发布；换账户那一侧发的才是最后一份。
    #[test]
    fn a_round_cut_short_by_an_account_switch_publishes_nothing_more() {
        let config = tempfile::tempdir().unwrap();
        let (one, two) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (id1, e1) = vault_on_disk(one.path());
        let (id2, e2) = vault_on_disk(two.path());
        bind_with_baseline(config.path(), &id1, 1, owner("acc-a"));
        bind_with_baseline(config.path(), &id2, 2, owner("acc-a"));
        let service = Arc::new(SyncService::default());
        let env = Arc::new(FakeEnv::new(config.path(), vec![e1, e2]));
        let (a, _) = signed_in("acc-a", "alice@example.test");
        env.sign_in(Some(a));
        let (b, _) = signed_in("acc-b", "bob@example.test");
        let published_at_switch = Arc::new(AtomicU64::new(0));
        {
            let service = Arc::clone(&service);
            let env_for_hook = Arc::clone(&env);
            let at = Arc::clone(&published_at_switch);
            *env.during_round.lock().unwrap() = Some(Box::new(move || {
                env_for_hook.sign_in(Some(b));
                service.forget_account();
                at.store(env_for_hook.publish_count() as u64, Ordering::SeqCst);
            }));
        }
        tauri::async_runtime::block_on(service.tick_with(&*env));
        assert_eq!(env.rounds().len(), 1, "换账户之后剩下的库不再跑");
        assert_eq!(
            env.publish_count() as u64,
            published_at_switch.load(Ordering::SeqCst),
            "换账户之后这一轮一份总览都没再发"
        );
        assert_eq!(
            env.size_fetches.load(Ordering::SeqCst),
            0,
            "也不再取上一个账户的体积"
        );
        tauri::async_runtime::block_on(service.realign_with(&*env));
        let out = env.last();
        assert!(out
            .vaults
            .iter()
            .all(|v| v.phase == SyncPhase::OtherAccount));
    }

    /// 两个同步着的库（游标都在 3）、登录着、已跑过一轮装上会话；返回服务、环境与两个库身份。
    fn two_synced_vaults(
        config: &Path,
        one: &Path,
        two: &Path,
    ) -> (SyncService, FakeEnv, String, String) {
        let (id1, e1) = vault_on_disk(one);
        let (id2, e2) = vault_on_disk(two);
        bind_with_baseline(config, &id1, 1, owner("acc-a"));
        bind_with_baseline(config, &id2, 2, owner("acc-a"));
        let service = SyncService::default();
        let env = FakeEnv::new(config, vec![e1, e2]);
        let (a, _) = signed_in("acc-a", "alice@example.test");
        env.sign_in(Some(a));
        tauri::async_runtime::block_on(service.tick_with(&env));
        assert_eq!(env.rounds().len(), 2);
        (service, env, id1, id2)
    }

    fn seen_of(service: &SyncService, id: &str) -> Arc<AtomicI64> {
        Arc::clone(&tauri::async_runtime::block_on(service.sessions.lock()).live[id].seen)
    }

    /// 按调度循环的写法跑一次：取走唤醒理由，有事才跑。
    fn schedule(service: &SyncService, env: &FakeEnv, timed_out: bool) -> bool {
        match service.take_scope(timed_out) {
            Some(scope) => {
                tauri::async_runtime::block_on(service.tick_in(env, &scope));
                true
            }
            None => false,
        }
    }

    /// 序号提示比游标新：只为那一个库跑一轮；一阵提示攒成一轮；不比游标新的提示（连上时那一帧
    /// 当前 head）什么都不做。
    #[test]
    fn a_head_hint_ahead_of_the_cursor_runs_one_round_for_that_vault_only() {
        let config = tempfile::tempdir().unwrap();
        let (one, two) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (service, env, id1, _id2) = two_synced_vaults(config.path(), one.path(), two.path());
        let seen = seen_of(&service, &id1);
        assert_eq!(seen.load(Ordering::SeqCst), 3, "镜像取自状态库的游标");

        assert!(!service.head_hint(&id1, 3, &seen), "与游标相等：不排");
        assert!(!service.head_hint(&id1, 2, &seen));
        assert!(!schedule(&service, &env, false), "没有新提示：这一次不跑");
        assert_eq!(env.rounds().len(), 2);

        assert!(service.head_hint(&id1, 4, &seen));
        assert!(schedule(&service, &env, false));
        assert_eq!(
            env.rounds()[2..],
            *std::slice::from_ref(&id1),
            "只跑提示过的那个库"
        );

        // 一阵提示（去抖期间连来五帧）：攒成一轮。
        for head in 5..10 {
            assert!(service.head_hint(&id1, head, &seen));
        }
        assert!(schedule(&service, &env, false));
        assert!(!schedule(&service, &env, false), "剩下的那张唤醒许可不再跑");
        assert_eq!(env.rounds()[3..], *std::slice::from_ref(&id1));
    }

    /// 接线：提示流的 `head` 帧经 `head_hint` 进调度，调度循环按 `take_scope` 只跑该跑的库，轮后
    /// 刷新游标镜像。任一处被删即红（真机之外唯一能看见「提示收到了却没人用」的地方）。
    #[test]
    fn the_hint_stream_feeds_the_scheduler() {
        let squash = |text: &str| text.split_whitespace().collect::<String>();
        let source = squash(&crate::ipc::source_without_tests(include_str!(
            "sync_service.rs"
        )));
        assert!(source.contains(
            "Ok(Some(SyncEventDto::Head{seq}))=>{backoff=HINT_RETRY_MIN;ifletSome(service)=app.try_state::<Arc<SyncService>>(){service.head_hint(&local_vault_id,seq,&seen);"
        ));
        assert!(source.contains(
            "ifletSome(scope)=service.take_scope(timed_out){service.tick_in(&AppEnv{app:&app},&scope).await;"
        ));
        assert!(source.contains(".seen.store(cursor_seq(&session.state),Ordering::SeqCst);"));
    }

    /// 提示攒下之后、轮到它之前已有一轮把游标推过了提示的序号：不白跑。
    #[test]
    fn a_hint_already_covered_by_a_finished_round_does_not_run_again() {
        let scope = Scope::Hinted(BTreeMap::from([("v".to_owned(), 7)]));
        assert!(scope.includes("v", 6));
        assert!(!scope.includes("v", 7));
        assert!(!scope.includes("w", NO_CURSOR), "没被提示的库不跑");
        assert!(scope.includes("v", NO_CURSOR) && Scope::All.includes("w", 9));
    }

    /// 暂停的库（单库或全部）收到提示也不跑；本地改动、立即同步与提示同时攒着时整轮跑。
    #[test]
    fn a_hint_for_a_paused_vault_runs_nothing() {
        let config = tempfile::tempdir().unwrap();
        let (one, two) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (service, env, id1, id2) = two_synced_vaults(config.path(), one.path(), two.path());
        let seen = seen_of(&service, &id1);
        tauri::async_runtime::block_on(service.set_vault_paused(&id1, true)).unwrap();
        let _ = service.take_scope(false); // 暂停那一下的唤醒。
        assert!(service.head_hint(&id1, 4, &seen));
        assert!(schedule(&service, &env, false));
        assert_eq!(env.rounds().len(), 2, "单库暂停：不跑");

        tauri::async_runtime::block_on(service.set_vault_paused(&id1, false)).unwrap();
        write_pause(config.path(), Some(None)).unwrap();
        assert!(service.head_hint(&id1, 4, &seen));
        assert!(schedule(&service, &env, false));
        assert_eq!(env.rounds().len(), 2, "全部暂停：一个请求都不发");

        write_pause(config.path(), None).unwrap();
        assert!(service.head_hint(&id1, 4, &seen));
        service.nudge();
        assert!(schedule(&service, &env, false));
        let mut all = env.rounds()[2..].to_vec();
        all.sort();
        let mut both = vec![id1, id2];
        both.sort();
        assert_eq!(all, both, "有整轮的理由就整轮跑");
    }

    /// 提示流断着（一帧提示都没有）：60 秒兜底照样整轮跑。
    #[test]
    fn without_hints_the_periodic_round_still_runs_every_vault() {
        let config = tempfile::tempdir().unwrap();
        let (one, two) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (service, env, id1, id2) = two_synced_vaults(config.path(), one.path(), two.path());
        assert!(schedule(&service, &env, true));
        let mut all = env.rounds()[2..].to_vec();
        all.sort();
        let mut both = vec![id1, id2];
        both.sort();
        assert_eq!(all, both);
    }

    /// 改为同步到当前账户：没确认 / 向导取消时原绑定原样留着；新绑定落下的那一刻才解除原绑定
    /// （状态库、块缓存、基线钉住、主密钥缓存）。
    #[test]
    fn taking_over_releases_the_old_binding_only_when_the_new_one_lands() {
        let config = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let (id, entry) = vault_on_disk(root.path());
        bind_with_baseline(config.path(), &id, 9, owner("acc-a"));
        let dir = state_dir(config.path(), &id);
        let path = dir.join("state.sqlite");
        let keys = CredentialCmkStore::new(Arc::new(luna_cloud::store::MemoryStore::new()));
        let old_remote = Uuid::from_u128(9);
        keys.save(old_remote, &luna_sync_core::crypto::Cmk::generate(1))
            .unwrap();
        let vault = Vault::open(root.path()).unwrap();
        let pin = vault.sync_pin(&pin_owner(old_remote), b"base").unwrap();
        let intact = |why: &str| {
            assert_eq!(
                luna_sync::state::peek_owner(&path).unwrap(),
                Some(owner("acc-a")),
                "{why}"
            );
            assert!(dir.join("chunks/abc").exists(), "{why}");
            assert!(
                vault.sync_read_pinned(&pin_owner(old_remote), &pin).is_ok(),
                "{why}"
            );
            assert_eq!(keys.load(old_remote).unwrap().len(), 1, "{why}");
        };

        // 没确认改连：拒绝，什么都不动。
        assert_eq!(
            bindable(&path, &owner("acc-b"), false).unwrap_err(),
            "sync_bound_elsewhere"
        );
        intact("没确认");
        // 确认了、向导打开了：还没落新绑定之前（取消、建库失败）原绑定都在。
        let replaced = bindable(&path, &owner("acc-b"), true).unwrap().unwrap();
        assert_eq!(replaced.remote_vault_id, old_remote);
        intact("向导还没绑定 / 取消");

        // 向导落下新绑定：原绑定这时才解除。
        let new = binding_for(&id, 77, owner("acc-b"));
        let state = bind_local(&dir, &new, Some(&replaced), &keys, |holder| {
            vault.sync_unpin_all(holder).unwrap();
        })
        .unwrap();
        assert_eq!(state.binding().unwrap().unwrap().owner, owner("acc-b"));
        assert!(state.bases().unwrap().is_empty(), "新账户从首次同步开始");
        assert!(!dir.join("chunks/abc").exists(), "原绑定的块缓存清掉");
        assert!(
            vault
                .sync_read_pinned(&pin_owner(old_remote), &pin)
                .is_err(),
            "原绑定的钉住解掉"
        );
        assert!(
            keys.load(old_remote).unwrap().is_empty(),
            "原库的主密钥缓存忘掉"
        );
        assert_eq!(
            bindable(&path, &owner("acc-b"), false).unwrap_err(),
            "sync_already_enabled"
        );
        drop(state);

        // 现在它是 B 的库；A 再登录时看到的是「连在另一个账户下」。
        let service = SyncService::default();
        let env = FakeEnv::new(config.path(), vec![entry]);
        let (a, _) = signed_in("acc-a", "alice@example.test");
        env.sign_in(Some(a));
        tauri::async_runtime::block_on(service.realign_with(&env));
        let row = &env.last().vaults[0];
        assert_eq!(row.phase, SyncPhase::OtherAccount);
        assert_eq!(
            row.bound_account.as_ref().map(|a| a.label.as_str()),
            Some("a***@example.test")
        );
    }

    /// 旧版（没有归属）的绑定启动时删掉——只删那一个库的同步目录，别的一概不碰。
    #[test]
    fn legacy_ownerless_state_is_removed_at_startup_and_nothing_else() {
        let config = tempfile::tempdir().unwrap();
        let sync = config.path().join("sync");
        let legacy_id = "0190f0f0-0000-7000-8000-0000000000a1";
        let current_id = "0190f0f0-0000-7000-8000-0000000000a2";
        let legacy = sync.join(legacy_id);
        std::fs::create_dir_all(legacy.join("chunks")).unwrap();
        std::fs::write(legacy.join("chunks/abc"), b"chunk").unwrap();
        {
            let conn = rusqlite::Connection::open(legacy.join("state.sqlite")).unwrap();
            conn.execute_batch(
                "create table binding (id integer primary key, remote_vault_id text not null,
                     server_origin text not null, local_vault_id text not null,
                     created_ms integer not null);
                 insert into binding values (1, '00000000-0000-0000-0000-000000000001', 'x', 'v', 1);
                 pragma user_version = 3;",
            )
            .unwrap();
        }
        bind_with_baseline(config.path(), current_id, 2, owner("acc-a"));
        // 名字不是库身份的目录，哪怕里面也是旧版状态库：不是这一版的形状，不碰。
        let stray = sync.join("not-a-vault");
        std::fs::create_dir_all(&stray).unwrap();
        std::fs::copy(legacy.join("state.sqlite"), stray.join("state.sqlite")).unwrap();
        std::fs::write(sync.join("readme.txt"), b"x").unwrap();
        std::fs::write(config.path().join("sync-pause.json"), b"{}").unwrap();

        assert_eq!(remove_legacy_state(config.path()), 1);
        assert!(!legacy.exists(), "旧版的状态与块缓存一起删掉");
        assert_eq!(
            baseline_of(config.path(), current_id),
            1,
            "有归属的绑定不动"
        );
        assert!(stray.join("state.sqlite").exists());
        assert!(sync.join("readme.txt").exists());
        assert!(config.path().join("sync-pause.json").exists());
        assert_eq!(remove_legacy_state(config.path()), 0, "再跑一次什么都不删");
        // 没有 sync 目录也不报错。
        assert_eq!(remove_legacy_state(&config.path().join("nope")), 0);
    }

    // ── 断开、移除与云端库被删除（ADR-151 修订 2026-10-15）──────────────────────────

    /// 一份同步着的库：绑定 + 基线 + 块缓存 + 钥匙串里的主密钥 + 库里钉住的一份基线 + 一篇笔记。
    struct Bound {
        _root: tempfile::TempDir,
        root: PathBuf,
        id: String,
        entry: luna_vault::registry::VaultEntry,
        remote: Uuid,
        pin: String,
    }

    fn bound_vault(config: &Path, keys: &dyn CmkStore, remote: u128, account: &str) -> Bound {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().to_path_buf();
        let (id, entry) = vault_on_disk(&path);
        std::fs::write(path.join("note.md"), b"# keep me").unwrap();
        bind_with_baseline(config, &id, remote, owner(account));
        let remote = Uuid::from_u128(remote);
        keys.save(remote, &luna_sync_core::crypto::Cmk::generate(1))
            .unwrap();
        let pin = Vault::open(&path)
            .unwrap()
            .sync_pin(&pin_owner(remote), b"base")
            .unwrap();
        Bound {
            _root: root,
            root: path,
            id,
            entry,
            remote,
            pin,
        }
    }

    /// 解除之后：同步目录（状态库与块缓存）、主密钥缓存、钉住都没了；库里的文件一个不少。
    fn assert_released(config: &Path, keys: &dyn CmkStore, vault: &Bound) {
        assert!(
            !state_dir(config, &vault.id).exists(),
            "同步目录（状态库与块缓存）删掉"
        );
        assert!(
            keys.load(vault.remote).unwrap().is_empty(),
            "主密钥缓存忘掉"
        );
        assert!(
            Vault::open(&vault.root)
                .unwrap()
                .sync_read_pinned(&pin_owner(vault.remote), &vault.pin)
                .is_err(),
            "钉住的基线解掉"
        );
        assert_eq!(
            std::fs::read(vault.root.join("note.md")).unwrap(),
            b"# keep me",
            "本地文件不动"
        );
    }

    /// 停止同步 / 「管理库」移除共用的解除：不论绑定归哪个账户，都解掉盘上那一份（含另一个账户的），
    /// 只清同步状态、不碰库里的文件；没有绑定 = `None`、什么都不删。
    #[test]
    fn releasing_a_vault_removes_every_sync_trace_but_never_a_file() {
        let config = tempfile::tempdir().unwrap();
        let keys = CredentialCmkStore::new(Arc::new(luna_cloud::store::MemoryStore::new()));
        for account in ["acc-a", "acc-b"] {
            let vault = bound_vault(config.path(), &keys, 41, account);
            let unpin_root = vault.root.clone();
            let released = release_vault(&state_dir(config.path(), &vault.id), None, &keys, |h| {
                Vault::open(&unpin_root).unwrap().sync_unpin_all(h).unwrap();
            })
            .unwrap();
            assert_eq!(released, Some(vault.remote), "{account}");
            assert_released(config.path(), &keys, &vault);
        }
        let never = tempfile::tempdir().unwrap();
        let (id, _) = vault_on_disk(never.path());
        assert_eq!(
            release_vault(&state_dir(config.path(), &id), None, &keys, |_| {}).unwrap(),
            None
        );
    }

    /// 「管理库」移除前的判定：此刻账户在同步 / 连在别的账户下（含此刻没登录）/ 没有同步，各一句。
    #[test]
    fn the_removal_preflight_names_the_binding_it_will_release() {
        let config = tempfile::tempdir().unwrap();
        let keys = CredentialCmkStore::new(Arc::new(luna_cloud::store::MemoryStore::new()));
        let vault = bound_vault(config.path(), &keys, 42, "acc-a");
        assert_eq!(
            sync_link_of(config.path(), &vault.entry, Some(&owner("acc-a"))),
            VaultSyncLinkDto::Syncing
        );
        assert_eq!(
            sync_link_of(config.path(), &vault.entry, Some(&owner("acc-b"))),
            VaultSyncLinkDto::Linked {
                account: official_account("a***@example.test")
            }
        );
        assert_eq!(
            sync_link_of(config.path(), &vault.entry, None),
            VaultSyncLinkDto::Linked {
                account: official_account("a***@example.test")
            }
        );
        let plain = tempfile::tempdir().unwrap();
        let (_, entry) = vault_on_disk(plain.path());
        assert_eq!(
            sync_link_of(config.path(), &entry, Some(&owner("acc-a"))),
            VaultSyncLinkDto::None
        );
    }

    /// 同步列表只列库清单里的库：一份绑定留在盘上、库却已不在清单里（移除过），总览里没有它，
    /// 也不为它跑同步；「云端库」也不把它算作在这台设备上。
    #[test]
    fn a_binding_whose_vault_left_the_registry_is_never_listed() {
        let config = tempfile::tempdir().unwrap();
        let keys = CredentialCmkStore::new(Arc::new(luna_cloud::store::MemoryStore::new()));
        let kept = bound_vault(config.path(), &keys, 43, "acc-a");
        let orphan = bound_vault(config.path(), &keys, 44, "acc-a");
        let service = SyncService::default();
        let env = FakeEnv::new(config.path(), vec![kept.entry.clone()]);
        let (a, _) = signed_in("acc-a", "alice@example.test");
        env.sign_in(Some(a));
        tauri::async_runtime::block_on(service.tick_with(&env));
        let out = env.last();
        assert_eq!(out.vaults.len(), 1);
        assert_eq!(out.vaults[0].path, kept.entry.path.to_string_lossy());
        assert_eq!(env.rounds(), vec![kept.id.clone()], "不为清单外的绑定跑");
        let linked = linked_here(
            config.path(),
            std::slice::from_ref(&kept.entry),
            &owner("acc-a"),
        );
        assert_eq!(linked.len(), 1);
        assert!(linked.contains_key(&kept.remote));
        assert!(!linked.contains_key(&orphan.remote));
        assert!(
            linked_here(
                config.path(),
                std::slice::from_ref(&kept.entry),
                &owner("acc-b")
            )
            .is_empty(),
            "别的账户名下一个都不算"
        );
    }

    /// 同步撞上 410 `vault_deleted`：解除绑定（本地文件不动）、通知中心说一次带库名；之后不再为
    /// 它跑、不再说，也不留「同步出错」。
    #[test]
    fn a_vault_deleted_elsewhere_is_released_once_with_one_notice() {
        let config = tempfile::tempdir().unwrap();
        let service = SyncService::default();
        let env = FakeEnv::new(config.path(), Vec::new());
        let keys = env.keys();
        let vault = bound_vault(config.path(), &keys, 45, "acc-a");
        let env = FakeEnv {
            known: vec![vault.entry.clone()],
            ..env
        };
        let (a, _) = signed_in("acc-a", "alice@example.test");
        env.sign_in(Some(a));
        tauri::async_runtime::block_on(service.tick_with(&env));
        assert_eq!(env.rounds().len(), 1);
        assert!(env.notices().is_empty());

        env.delete_remote(45);
        tauri::async_runtime::block_on(service.tick_with(&env));
        assert_eq!(env.notices(), vec!["笔记".to_owned()], "说一次，带库名");
        assert_released(config.path(), &keys, &vault);
        let row = &env.last().vaults[0];
        assert_eq!(row.phase, SyncPhase::Off, "不是「同步出错」");
        assert_eq!(row.last_error, None);
        assert_eq!(row.remote_vault_id, None);

        tauri::async_runtime::block_on(service.tick_with(&env));
        tauri::async_runtime::block_on(service.tick_with(&env));
        assert_eq!(env.rounds().len(), 2, "解除之后不再为它跑");
        assert_eq!(env.notices().len(), 1, "只说一次");
    }

    /// 取钥（信封）就撞上 410：同一处处置。
    #[test]
    fn a_deletion_seen_while_fetching_keys_is_handled_the_same_way() {
        let config = tempfile::tempdir().unwrap();
        let service = SyncService::default();
        let env = FakeEnv::new(config.path(), Vec::new());
        let keys = env.keys();
        let vault = bound_vault(config.path(), &keys, 46, "acc-a");
        // 钥匙串里没有它：这一轮先去取钥。
        keys.forget(vault.remote).unwrap();
        let env = FakeEnv {
            known: vec![vault.entry.clone()],
            ..env
        };
        let (a, _) = signed_in("acc-a", "alice@example.test");
        env.sign_in(Some(a));
        env.delete_remote(46);
        tauri::async_runtime::block_on(service.tick_with(&env));
        assert_eq!(env.key_fetches(), 1);
        assert!(env.rounds().is_empty());
        assert_eq!(env.notices().len(), 1);
        assert_released(config.path(), &keys, &vault);
    }

    /// 提示流的终结帧：与 410 同一处处置、只说一次；随后同一轮再撞上 410 也不再说。这台设备自己删的
    /// 一句都不说。
    #[test]
    fn the_terminal_event_releases_once_and_a_deletion_made_here_is_not_announced() {
        let config = tempfile::tempdir().unwrap();
        let service = SyncService::default();
        let env = FakeEnv::new(config.path(), Vec::new());
        let keys = env.keys();
        let first = bound_vault(config.path(), &keys, 47, "acc-a");
        let mine = bound_vault(config.path(), &keys, 48, "acc-a");
        let env = FakeEnv {
            known: vec![first.entry.clone(), mine.entry.clone()],
            ..env
        };
        let (a, _) = signed_in("acc-a", "alice@example.test");
        env.sign_in(Some(a));
        tauri::async_runtime::block_on(service.tick_with(&env));
        assert_eq!(env.rounds().len(), 2);

        tauri::async_runtime::block_on(service.remote_deleted_with(&env, first.remote));
        assert_eq!(env.notices().len(), 1);
        assert_released(config.path(), &keys, &first);
        assert_eq!(
            env.last()
                .vaults
                .iter()
                .find(|v| v.path == first.entry.path.to_string_lossy())
                .unwrap()
                .phase,
            SyncPhase::Off,
            "当场重发总览"
        );
        tauri::async_runtime::block_on(service.remote_deleted_with(&env, first.remote));
        assert_eq!(env.notices().len(), 1, "再来一帧也不再说");

        // 这台设备自己删的：撞上 410 照样解除，但不说「在另一台设备上被删除」。
        service.expect_deletion(mine.remote);
        env.delete_remote(48);
        tauri::async_runtime::block_on(service.tick_with(&env));
        assert_released(config.path(), &keys, &mine);
        assert_eq!(env.notices().len(), 1);
    }
}
