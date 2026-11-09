//! 本机同步状态（`state.sqlite`；ADR-148 D3：放应用配置目录 `sync/<库身份>/`）。
//!
//! 记三态里的「已同步」那一态（基线）与拉取游标。**可丢**：丢了就做一次全量清单对账（按内容
//! 哈希比对，相同内容不重新上传），所以这里只放能重建的东西。
//!
//! 绑定记着它归哪台服务器的哪个账户（[`BindingOwner`]，ADR-151 修订 2026-10-15）：一份本地库
//! 同一时刻只归一个账户，别的账户登录时只读得出「它连在另一个账户下」，不会拿它去发任何请求。
//! 没有归属的旧版状态库（`user_version` 低于 [`SCHEMA_VERSION`]）不升级、不迁移：打开即报
//! [`StateError::Schema`]，由应用在启动时删掉（未发布即硬切）。
//!
//! 基线与游标在同一个事务里推进（[`SyncState::commit_round`]）：崩在中间时两者一起回到上一轮，
//! 不会出现「游标走了、基线没跟上」而漏掉一段变更。
//!
//! **不用 WAL**（回滚日志，[`connect`]）：WAL 的索引是内存映射的 `-shm` 文件，系统一旦收回它的底层
//! vnode（2026-10-22 真机：内核记为 ungraft），下一次读就是 SIGBUS，整个应用退出、连报错的机会都没有。
//! 回滚日志没有内存映射，同样的事只是一次 I/O 错误——这一轮同步失败、下一轮重试。这份库很小、
//! 读写都快，进程内多条连接之间的短暂互等由默认的 5 秒忙等超时吸收。
//!
//! 日志纪律（TDD-12）：活动记录只有 file_id、动作与原因码，路径只在 `files` 表里（本机、
//! 用户自己的库），不进任何日志。

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};
use uuid::Uuid;

/// 状态库的结构版本。v4 起绑定带归属账户；更早的版本一律不认（[`is_legacy`]）。
pub const SCHEMA_VERSION: i64 = 4;

#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error("sync state database: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("sync state schema version {0} is not supported")]
    Schema(i64),
    #[error("sync state row malformed")]
    Malformed,
}

/// 绑定归哪个账户（ADR-151 修订 2026-10-15：一份本地库同一时刻只归一个账户）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingOwner {
    /// 服务端分桶（主机与端口，`luna_cloud::config::server_bucket`）。
    pub server_bucket: String,
    /// 服务端的账户 id。
    pub account_id: String,
    /// 绑定时账户名的打码写法（如 `jo…88@gmail.com`）：只给界面说明「连在哪个账户下」，
    /// 不参与任何判定。
    pub account_label: String,
}

impl BindingOwner {
    /// 是不是同一个账户：只比服务器与账户 id（打码的名字会随邮箱改动变，不算换账户）。
    #[must_use]
    pub fn same_account(&self, other: &Self) -> bool {
        self.server_bucket == other.server_bucket && self.account_id == other.account_id
    }
}

/// 本地库与远端库的绑定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub remote_vault_id: Uuid,
    /// 服务端源（`https://host[:port]`）：换服务端就是另一份绑定。
    pub server_origin: String,
    /// 本地库的身份（`.luna/vault-id`）：每轮开始前核对，对不上整轮中止。
    pub local_vault_id: String,
    pub created_ms: i64,
    /// 归属账户。
    pub owner: BindingOwner,
}

/// 拉取游标。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    pub epoch: Uuid,
    pub seq: i64,
}

/// 一个文件的基线（上次同步时两边一致的那一版）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseFile {
    pub file_id: Uuid,
    /// 库相对路径（NFC）。
    pub path: String,
    /// 服务端 rev。
    pub rev: i64,
    /// 内容 sha256；墓碑为空串。
    pub content_hash: String,
    /// 基线内容在快照库里钉住的指纹（`.md` 才钉，三路合并的共同祖先）。
    pub pin: Option<String>,
    pub deleted: bool,
    /// 本机没有副本、只在云端（录音按需下载，ADR-156）：本机不在这个文件不是删除，云端改了
    /// 只更新基线、不下载；打开时再下（[`crate::engine::Engine::materialize`]）。
    pub cloud_only: bool,
}

/// 一轮同步对基线的改动。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BaseChange {
    Upsert(BaseFile),
    Remove(Uuid),
}

/// 一条活动记录（同步面板的「最近发生了什么」）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activity {
    pub at_ms: i64,
    pub kind: String,
    pub file_id: Option<Uuid>,
    pub detail_code: Option<String>,
}

/// 活动记录只留最近这么多条。
pub const ACTIVITY_KEPT: i64 = 500;

pub struct SyncState {
    conn: Connection,
}

/// 状态库的结构版本是不是旧版（v4 之前、绑定没有归属）。比现在新的版本不算——那不是这一版
/// 写的，也不该由这一版删。
#[must_use]
pub fn is_legacy(err: &StateError) -> bool {
    matches!(err, StateError::Schema(version) if *version < SCHEMA_VERSION)
}

/// 打开一条连接，日志模式定为回滚日志（`DELETE`，见模块说明）；旧库是 WAL 时就地切换——切换要求没有别的
/// 连接开着，切不成时 SQLite 如实保持原样，下一次打开再切。所有打开状态库的路都经过这里。
fn connect(path: &Path, flags: rusqlite::OpenFlags) -> Result<Connection, StateError> {
    let conn = Connection::open_with_flags(path, flags)?;
    conn.pragma_update(None, "journal_mode", "DELETE")?;
    Ok(conn)
}

/// 已存在的状态库（不新建）。
fn connect_existing(path: &Path) -> Result<Connection, StateError> {
    connect(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
}

/// 只读出一份状态库里绑定的归属，不建表、不升级、不改任何数据（文件不在 = 报错，不新建）。
///
/// 应用每轮都要知道「这份库连在谁名下」，而另一个账户的绑定只该被看一眼、绝不被打开成会话。
/// 结构版本是旧版 → [`StateError::Schema`]；还没绑定 = `Ok(None)`。
pub fn peek_owner(path: &Path) -> Result<Option<BindingOwner>, StateError> {
    let conn = connect_existing(path)?;
    let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    match version {
        0 => return Ok(None),
        SCHEMA_VERSION => {}
        other => return Err(StateError::Schema(other)),
    }
    Ok(conn
        .query_row(
            "select owner_bucket, owner_account_id, owner_label from binding",
            [],
            |row| {
                Ok(BindingOwner {
                    server_bucket: row.get(0)?,
                    account_id: row.get(1)?,
                    account_label: row.get(2)?,
                })
            },
        )
        .optional()?)
}

/// 只读出一份状态库里的整条绑定（远端库 id 与归属），同 [`peek_owner`] 不建表、不升级、不改任何
/// 数据——会话还开着这份状态库时也能放心看（列「云端库」、移除库时判在不在这台设备上）。
pub fn peek_binding(path: &Path) -> Result<Option<Binding>, StateError> {
    let conn = connect_existing(path)?;
    let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    match version {
        0 => return Ok(None),
        SCHEMA_VERSION => {}
        other => return Err(StateError::Schema(other)),
    }
    read_binding(&conn)
}

/// 这份库的归属账户再次登录时，按它此刻的打码写法重写存下来的标签（打码规则改过就会不同，
/// 硬切、不另做迁移）。只改标签，归属本身（分桶 + 账户 id）不动；不是这个账户的绑定什么都不改。
/// 返回是否改了。
pub fn relabel_owner(path: &Path, owner: &BindingOwner) -> Result<bool, StateError> {
    let conn = connect_existing(path)?;
    let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version != SCHEMA_VERSION {
        return Ok(false);
    }
    let changed = conn.execute(
        "update binding set owner_label = ?1 \
         where owner_bucket = ?2 and owner_account_id = ?3 and owner_label <> ?1",
        rusqlite::params![owner.account_label, owner.server_bucket, owner.account_id],
    )?;
    Ok(changed > 0)
}

fn read_binding(conn: &Connection) -> Result<Option<Binding>, StateError> {
    conn.query_row(
        "select remote_vault_id, server_origin, local_vault_id, created_ms, \
         owner_bucket, owner_account_id, owner_label from binding",
        [],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                BindingOwner {
                    server_bucket: row.get(4)?,
                    account_id: row.get(5)?,
                    account_label: row.get(6)?,
                },
            ))
        },
    )
    .optional()?
    .map(|(remote, origin, local, created_ms, owner)| {
        Ok(Binding {
            remote_vault_id: Uuid::parse_str(&remote).map_err(|_| StateError::Malformed)?,
            server_origin: origin,
            local_vault_id: local,
            created_ms,
            owner,
        })
    })
    .transpose()
}

/// 名字比较键（NFC + 小写）：同一路径两种写法、或大小写只差一处，都会撞到同一个键。
#[must_use]
pub fn path_key(path: &str) -> String {
    luna_sync_core::path::name_key(path)
}

impl SyncState {
    /// 打开（没有就建）。
    pub fn open(path: &Path) -> Result<Self, StateError> {
        Self::init(connect(path, rusqlite::OpenFlags::default())?)
    }

    /// 内存库（测试与模拟器）。
    pub fn in_memory() -> Result<Self, StateError> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self, StateError> {
        let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        match version {
            0 => {
                conn.execute_batch(
                    "create table binding (
                         id integer primary key check (id = 1),
                         remote_vault_id text not null,
                         server_origin text not null,
                         local_vault_id text not null,
                         created_ms integer not null,
                         owner_bucket text not null,
                         owner_account_id text not null,
                         owner_label text not null,
                         epoch text,
                         cursor_seq integer not null default 0,
                         paused integer not null default 0
                     );
                     create table files (
                         file_id text primary key,
                         path text not null,
                         path_key text not null,
                         rev integer not null,
                         content_hash text not null,
                         pin text,
                         deleted integer not null,
                         cloud_only integer not null default 0
                     );
                     create unique index files_live_path on files (path_key) where deleted = 0;
                     create table activity (
                         id integer primary key autoincrement,
                         at_ms integer not null,
                         kind text not null,
                         file_id text,
                         detail_code text
                     );",
                )?;
                conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            }
            // v1–v3 的绑定没有归属：不升级（硬切），由应用启动时删掉、用户重新开启一次。
            SCHEMA_VERSION => {}
            other => return Err(StateError::Schema(other)),
        }
        Ok(Self { conn })
    }

    pub fn binding(&self) -> Result<Option<Binding>, StateError> {
        read_binding(&self.conn)
    }

    /// 建立绑定（换绑定 = 清掉全部基线与游标，从头对账）。
    pub fn bind(&mut self, binding: &Binding) -> Result<(), StateError> {
        let tx = self.conn.transaction()?;
        tx.execute("delete from binding", [])?;
        tx.execute("delete from files", [])?;
        tx.execute(
            "insert into binding (id, remote_vault_id, server_origin, local_vault_id, created_ms, \
             owner_bucket, owner_account_id, owner_label) values (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                binding.remote_vault_id.to_string(),
                binding.server_origin,
                binding.local_vault_id,
                binding.created_ms,
                binding.owner.server_bucket,
                binding.owner.account_id,
                binding.owner.account_label
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// 这份库是否被单独暂停（ADR-151 ③；跨重启保持）。没有绑定 = 不暂停。
    pub fn paused(&self) -> Result<bool, StateError> {
        Ok(self
            .conn
            .query_row("select paused from binding", [], |row| row.get::<_, i64>(0))
            .optional()?
            .is_some_and(|v| v != 0))
    }

    /// 单库暂停 / 继续。没有绑定时什么也不做。
    pub fn set_paused(&self, paused: bool) -> Result<(), StateError> {
        self.conn
            .execute("update binding set paused = ?1", params![i64::from(paused)])?;
        Ok(())
    }

    pub fn cursor(&self) -> Result<Option<Cursor>, StateError> {
        let row: Option<(Option<String>, i64)> = self
            .conn
            .query_row("select epoch, cursor_seq from binding", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .optional()?;
        match row {
            Some((Some(epoch), seq)) => Ok(Some(Cursor {
                epoch: Uuid::parse_str(&epoch).map_err(|_| StateError::Malformed)?,
                seq,
            })),
            _ => Ok(None),
        }
    }

    /// 丢掉游标（纪元变了 / 游标过期）：下一轮做全量对账，基线保留——全量对账按基线判断谁改了。
    pub fn forget_cursor(&self) -> Result<(), StateError> {
        self.conn
            .execute("update binding set epoch = null, cursor_seq = 0", [])?;
        Ok(())
    }

    pub fn base(&self, file_id: Uuid) -> Result<Option<BaseFile>, StateError> {
        self.conn
            .query_row(
                "select file_id, path, rev, content_hash, pin, deleted, cloud_only from files where file_id = ?1",
                params![file_id.to_string()],
                base_from_row,
            )
            .optional()?
            .transpose()
    }

    /// 全部基线（含墓碑）。
    pub fn bases(&self) -> Result<Vec<BaseFile>, StateError> {
        let mut stmt = self.conn.prepare(
            "select file_id, path, rev, content_hash, pin, deleted, cloud_only from files order by path_key",
        )?;
        let rows = stmt.query_map([], base_from_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row??);
        }
        Ok(out)
    }

    /// 一轮同步收尾：基线改动与新游标一个事务。`cursor = None` = 游标不动（这一轮有云端改动没
    /// 应用上，下一轮要重拉同一段）。
    pub fn commit_round(
        &mut self,
        changes: &[BaseChange],
        cursor: Option<Cursor>,
    ) -> Result<(), StateError> {
        let tx = self.conn.transaction()?;
        // 先删后插：同一轮里一个文件让出某个路径、另一个文件占上它（改名互换、作为新文件推回），
        // 逐条 upsert 的顺序会让唯一的折叠路径键在中途撞上。先把涉及的行全删了，再插最终状态——
        // 只有最终状态真的撞名才会失败。
        for change in changes {
            let file_id = match change {
                BaseChange::Upsert(base) => base.file_id,
                BaseChange::Remove(file_id) => *file_id,
            };
            tx.execute(
                "delete from files where file_id = ?1",
                params![file_id.to_string()],
            )?;
        }
        let mut last: std::collections::HashMap<Uuid, &BaseFile> = std::collections::HashMap::new();
        for change in changes {
            match change {
                BaseChange::Upsert(base) => {
                    last.insert(base.file_id, base);
                }
                BaseChange::Remove(file_id) => {
                    last.remove(file_id);
                }
            }
        }
        for base in last.values() {
            tx.execute(
                "insert into files (file_id, path, path_key, rev, content_hash, pin, deleted, cloud_only) \
                 values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    base.file_id.to_string(),
                    base.path,
                    path_key(&base.path),
                    base.rev,
                    base.content_hash,
                    base.pin,
                    base.deleted,
                    base.cloud_only
                ],
            )?;
        }
        if let Some(cursor) = cursor {
            tx.execute(
                "update binding set epoch = ?1, cursor_seq = ?2",
                params![cursor.epoch.to_string(), cursor.seq],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn record_activity(&self, activity: &Activity) -> Result<(), StateError> {
        self.conn.execute(
            "insert into activity (at_ms, kind, file_id, detail_code) values (?1, ?2, ?3, ?4)",
            params![
                activity.at_ms,
                activity.kind,
                activity.file_id.map(|id| id.to_string()),
                activity.detail_code
            ],
        )?;
        self.conn.execute(
            "delete from activity where id <= (select max(id) from activity) - ?1",
            params![ACTIVITY_KEPT],
        )?;
        Ok(())
    }

    /// 最近的活动（新的在前）。
    pub fn recent_activity(&self, limit: i64) -> Result<Vec<Activity>, StateError> {
        let mut stmt = self.conn.prepare(
            "select at_ms, kind, file_id, detail_code from activity order by id desc limit ?1",
        )?;
        let rows = stmt.query_map(params![limit], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (at_ms, kind, file_id, detail_code) = row?;
            out.push(Activity {
                at_ms,
                kind,
                file_id: file_id
                    .map(|s| Uuid::parse_str(&s).map_err(|_| StateError::Malformed))
                    .transpose()?,
                detail_code,
            });
        }
        Ok(out)
    }
}

type BaseRow = Result<BaseFile, StateError>;

fn base_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<BaseRow> {
    let file_id: String = row.get(0)?;
    let Ok(file_id) = Uuid::parse_str(&file_id) else {
        return Ok(Err(StateError::Malformed));
    };
    Ok(Ok(BaseFile {
        file_id,
        path: row.get(1)?,
        rev: row.get(2)?,
        content_hash: row.get(3)?,
        pin: row.get(4)?,
        deleted: row.get(5)?,
        cloud_only: row.get(6)?,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner_a() -> BindingOwner {
        BindingOwner {
            server_bucket: "sync.example.test".into(),
            account_id: "acc-a".into(),
            account_label: "jo***@example.test".into(),
        }
    }

    fn binding(n: u128) -> Binding {
        Binding {
            remote_vault_id: Uuid::from_u128(n),
            server_origin: "https://sync.example.test".into(),
            local_vault_id: "0190f0f0-0000-7000-8000-000000000001".into(),
            created_ms: 1,
            owner: owner_a(),
        }
    }

    fn base(n: u128, path: &str, rev: i64) -> BaseFile {
        BaseFile {
            file_id: Uuid::from_u128(n),
            path: path.into(),
            rev,
            content_hash: "a".repeat(64),
            pin: Some("p".into()),
            deleted: false,
            cloud_only: false,
        }
    }

    /// 归属账户再次登录时按新的打码写法重写标签；别的账户、或标签没变时什么都不改。
    #[test]
    fn the_owner_label_is_rewritten_only_for_its_own_account() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.sqlite");
        {
            let mut state = SyncState::open(&path).unwrap();
            state.bind(&binding(1)).unwrap();
        }
        let mut other = owner_a();
        other.account_id = "acc-b".into();
        other.account_label = "zz…zz@example.test".into();
        assert!(!relabel_owner(&path, &other).unwrap(), "别的账户不改");
        let mut relabeled = owner_a();
        relabeled.account_label = "jo…ny@example.test".into();
        assert!(relabel_owner(&path, &relabeled).unwrap());
        assert_eq!(peek_owner(&path).unwrap(), Some(relabeled.clone()));
        assert!(!relabel_owner(&path, &relabeled).unwrap(), "没变就不写");
    }

    fn journal_mode(path: &Path) -> String {
        Connection::open(path)
            .unwrap()
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap()
    }

    /// 状态库不用 WAL：会话开着时也没有内存映射的 `-shm`——系统收回那个文件的底层 vnode 会让读它的进程
    /// SIGBUS 退出（2026-10-22 真机：装「创作」触发的一轮同步里读游标时整个应用崩溃）。
    #[test]
    fn the_state_uses_a_rollback_journal_and_never_maps_a_shm_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.sqlite");
        let mut state = SyncState::open(&path).unwrap();
        state.bind(&binding(1)).unwrap();
        state.cursor().unwrap();
        assert_eq!(
            peek_owner(&path).unwrap(),
            Some(owner_a()),
            "会话开着照样看得了"
        );
        assert!(!dir.path().join("state.sqlite-shm").exists());
        assert!(!dir.path().join("state.sqlite-wal").exists());
        drop(state);
        assert_eq!(journal_mode(&path), "delete");
    }

    /// 以前落下的 WAL 状态库：打开会话时就地切回回滚日志，`-shm` 随之消失；只看一眼也走同一条路。
    #[test]
    fn a_state_left_in_wal_mode_is_switched_to_a_rollback_journal() {
        let dir = tempfile::tempdir().unwrap();
        for opener in ["open", "peek"] {
            let path = dir.path().join(format!("{opener}.sqlite"));
            {
                let mut state = SyncState::open(&path).unwrap();
                state.bind(&binding(1)).unwrap();
            }
            Connection::open(&path)
                .unwrap()
                .pragma_update(None, "journal_mode", "WAL")
                .unwrap();
            assert_eq!(journal_mode(&path), "wal");
            if opener == "open" {
                drop(SyncState::open(&path).unwrap());
            } else {
                assert_eq!(peek_owner(&path).unwrap(), Some(owner_a()));
            }
            assert_eq!(journal_mode(&path), "delete", "{opener}");
            assert!(
                !dir.path().join(format!("{opener}.sqlite-shm")).exists(),
                "{opener}"
            );
        }
    }

    /// 旧版状态库（绑定没有归属）不升级：打开报旧版，只看一眼也报旧版，一个字节都不改。
    #[test]
    fn a_legacy_state_without_an_owner_is_refused_and_left_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.sqlite");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "create table binding (id integer primary key check (id = 1),
                     remote_vault_id text not null, server_origin text not null,
                     local_vault_id text not null, created_ms integer not null,
                     epoch text, cursor_seq integer not null default 0,
                     paused integer not null default 0);
                 insert into binding (id, remote_vault_id, server_origin, local_vault_id, created_ms)
                     values (1, '00000000-0000-0000-0000-000000000001', 'https://x', 'v', 1);
                 pragma user_version = 3;",
            )
            .unwrap();
        }
        let opened = SyncState::open(&path).err().unwrap();
        assert!(is_legacy(&opened));
        let peeked = peek_owner(&path).err().unwrap();
        assert!(is_legacy(&peeked));
        let conn = Connection::open(&path).unwrap();
        let version: i64 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 3, "不升级、不改写");
        // 比这一版新的不算旧版：那不是这一版该删的。
        assert!(!is_legacy(&StateError::Schema(SCHEMA_VERSION + 1)));
    }

    /// 绑定带着归属落盘；只看一眼就读得出归谁，不必把状态库打开成会话。
    #[test]
    fn the_owner_is_recorded_with_the_binding_and_can_be_peeked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.sqlite");
        {
            let state = SyncState::open(&path).unwrap();
            assert_eq!(peek_owner(&path).unwrap(), None, "还没绑定");
            drop(state);
        }
        let mut state = SyncState::open(&path).unwrap();
        state.bind(&binding(3)).unwrap();
        drop(state);
        let owner = peek_owner(&path).unwrap().unwrap();
        assert_eq!(owner, owner_a());
        // 整条绑定也能只读看一眼，且会话开着这份状态库时照样读得出。
        let open_session = SyncState::open(&path).unwrap();
        assert_eq!(peek_binding(&path).unwrap(), Some(binding(3)));
        drop(open_session);
        assert!(
            peek_binding(&dir.path().join("missing.sqlite")).is_err(),
            "不新建"
        );
        assert!(!dir.path().join("missing.sqlite").exists());
        assert_eq!(
            SyncState::open(&path)
                .unwrap()
                .binding()
                .unwrap()
                .unwrap()
                .owner,
            owner_a()
        );
        // 同一个账户：只看服务器与账户 id；打码的名字变了不算换账户。
        let renamed = BindingOwner {
            account_label: "x***@example.test".into(),
            ..owner_a()
        };
        assert!(owner.same_account(&renamed));
        let other_account = BindingOwner {
            account_id: "acc-b".into(),
            ..owner_a()
        };
        assert!(!owner.same_account(&other_account));
        let other_server = BindingOwner {
            server_bucket: "10.0.0.62:8443".into(),
            ..owner_a()
        };
        assert!(!owner.same_account(&other_server));
    }

    #[test]
    fn a_vault_can_be_paused_on_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.sqlite");
        let mut state = SyncState::open(&path).unwrap();
        state.bind(&binding(1)).unwrap();
        assert!(!state.paused().unwrap());
        state.set_paused(true).unwrap();
        drop(state);
        // 跨重启保持。
        let state = SyncState::open(&path).unwrap();
        assert!(state.paused().unwrap());
        state.set_paused(false).unwrap();
        assert!(!state.paused().unwrap());
        // 没有绑定：不算暂停，设了也不落地。
        let empty = SyncState::in_memory().unwrap();
        empty.set_paused(true).unwrap();
        assert!(!empty.paused().unwrap());
    }

    #[test]
    fn cloud_only_files_are_remembered() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.sqlite");
        let mut state = SyncState::open(&path).unwrap();
        state.bind(&binding(1)).unwrap();
        let mut cloud = base(8, "b.wav", 1);
        cloud.cloud_only = true;
        state
            .commit_round(&[BaseChange::Upsert(cloud.clone())], None)
            .unwrap();
        drop(state);
        let state = SyncState::open(&path).unwrap();
        assert_eq!(state.base(cloud.file_id).unwrap(), Some(cloud));
    }

    #[test]
    fn bases_and_cursor_advance_together() {
        let mut state = SyncState::in_memory().unwrap();
        assert!(state.binding().unwrap().is_none());
        state.bind(&binding(1)).unwrap();
        assert_eq!(state.binding().unwrap(), Some(binding(1)));
        assert_eq!(state.cursor().unwrap(), None);

        let epoch = Uuid::from_u128(77);
        state
            .commit_round(
                &[
                    BaseChange::Upsert(base(1, "a.md", 1)),
                    BaseChange::Upsert(base(2, "b.md", 1)),
                ],
                Some(Cursor { epoch, seq: 2 }),
            )
            .unwrap();
        assert_eq!(state.cursor().unwrap(), Some(Cursor { epoch, seq: 2 }));
        assert_eq!(state.bases().unwrap().len(), 2);

        let mut moved = base(1, "dir/a.md", 2);
        moved.deleted = false;
        state
            .commit_round(
                &[
                    BaseChange::Upsert(moved.clone()),
                    BaseChange::Remove(Uuid::from_u128(2)),
                ],
                Some(Cursor { epoch, seq: 4 }),
            )
            .unwrap();
        assert_eq!(state.base(Uuid::from_u128(1)).unwrap(), Some(moved));
        assert_eq!(state.base(Uuid::from_u128(2)).unwrap(), None);

        // 游标作废：基线留着（全量对账要用）。
        state.forget_cursor().unwrap();
        assert_eq!(state.cursor().unwrap(), None);
        assert_eq!(state.bases().unwrap().len(), 1);

        // 换绑定：从头来。
        state.bind(&binding(2)).unwrap();
        assert!(state.bases().unwrap().is_empty());
    }

    #[test]
    fn two_live_files_cannot_share_a_folded_path() {
        let mut state = SyncState::in_memory().unwrap();
        state.bind(&binding(1)).unwrap();
        let epoch = Uuid::from_u128(1);
        state
            .commit_round(
                &[BaseChange::Upsert(base(1, "Notes/Café.md", 1))],
                Some(Cursor { epoch, seq: 1 }),
            )
            .unwrap();
        // NFD + 大小写不同：同一个折叠键，两个活文件不许共存——整轮回滚，游标不动。
        let clash = base(2, "notes/Cafe\u{301}.md", 1);
        assert!(state
            .commit_round(
                &[BaseChange::Upsert(clash.clone())],
                Some(Cursor { epoch, seq: 2 })
            )
            .is_err());
        assert_eq!(state.cursor().unwrap(), Some(Cursor { epoch, seq: 1 }));
        // 墓碑不占路径。
        let mut tomb = clash;
        tomb.deleted = true;
        tomb.content_hash.clear();
        state
            .commit_round(&[BaseChange::Upsert(tomb)], Some(Cursor { epoch, seq: 3 }))
            .unwrap();
    }

    #[test]
    fn a_round_may_hand_a_path_from_one_file_to_another() {
        let mut state = SyncState::in_memory().unwrap();
        state.bind(&binding(1)).unwrap();
        let epoch = Uuid::from_u128(1);
        state
            .commit_round(
                &[
                    BaseChange::Upsert(base(1, "a.md", 1)),
                    BaseChange::Upsert(base(2, "b.md", 1)),
                ],
                Some(Cursor { epoch, seq: 2 }),
            )
            .unwrap();
        // 互换路径，以及「新文件接手旧文件的路径、旧文件移除」，写的先后都不该撞。
        state
            .commit_round(
                &[
                    BaseChange::Upsert(base(1, "b.md", 2)),
                    BaseChange::Upsert(base(2, "a.md", 2)),
                ],
                Some(Cursor { epoch, seq: 4 }),
            )
            .unwrap();
        state
            .commit_round(
                &[
                    BaseChange::Upsert(base(3, "a.md", 1)),
                    BaseChange::Remove(Uuid::from_u128(2)),
                ],
                Some(Cursor { epoch, seq: 5 }),
            )
            .unwrap();
        let paths: Vec<(u128, String)> = state
            .bases()
            .unwrap()
            .into_iter()
            .map(|b| (b.file_id.as_u128(), b.path))
            .collect();
        assert_eq!(paths, vec![(3, "a.md".into()), (1, "b.md".into())]);
    }

    #[test]
    fn activity_is_capped_and_newest_first() {
        let state = SyncState::in_memory().unwrap();
        for i in 0..(ACTIVITY_KEPT + 20) {
            state
                .record_activity(&Activity {
                    at_ms: i,
                    kind: "pushed".into(),
                    file_id: Some(Uuid::from_u128(i as u128)),
                    detail_code: None,
                })
                .unwrap();
        }
        let recent = state.recent_activity(1000).unwrap();
        assert_eq!(recent.len() as i64, ACTIVITY_KEPT);
        assert_eq!(recent[0].at_ms, ACTIVITY_KEPT + 19);
    }

    #[test]
    fn the_state_file_survives_reopening() {
        let dir = std::env::temp_dir().join(format!("luna-sync-state-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.sqlite");
        {
            let mut state = SyncState::open(&path).unwrap();
            state.bind(&binding(5)).unwrap();
        }
        let state = SyncState::open(&path).unwrap();
        assert_eq!(state.binding().unwrap(), Some(binding(5)));
        drop(state);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
