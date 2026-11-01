//! 工作区任务观测的应用侧状态（PRD-09-076 / TDD-12 §2.1）。
//!
//! 它比知识度量简单得多，而这**正是隐私边界的结果**：那边要保护 Vault / 笔记派生的关联
//! 值，故有钥匙串、有密钥生命周期、有"密钥不可用就停止记录"的一整套；这边库里只有计数，
//! 没有任何可关联值，于是这些一个都不需要（见 `luna_metrics::workspace` 的抬头）。
//!
//! 这里只负责三件事：**开关**、**授权 epoch**、**惰性开库**。

use std::path::PathBuf;
use std::sync::Mutex;

use crate::metrics_authorization::{AuthorizationGrant, MetricsAuthorization};
use luna_metrics::workspace::{WorkspaceEvent, WorkspaceMetrics, WorkspaceSummary};

/// 读不到观测时的具体原因。
///
/// **只有两个**：`clear_failed` 不在这里——一次失败的清除由清除命令自己的稳定错误码
/// 如实报告；把它塞进**可用性**，等于给同一件事第二个学习渠道，而且它在查询路径上根本
/// 不可能被观察到（那时清除早已返回）。不可达的枚举值只会让每个消费者写一条永不执行的
/// 分支（同批次 4 的 `vault_read_only`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageFailure {
    /// 此刻打不开（IO / 锁）——重试即可。
    StorageUnavailable,
    /// schema 对不上：我们不认识这个库，也不会去动它（fail closed）。
    SchemaInvalid,
}

/// 工作区观测专属凭据；不能误传给知识度量存储。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WorkspaceMetricsGrant(AuthorizationGrant);

/// 设备本地的工作区观测。
pub struct LocalWorkspaceMetrics {
    /// 惰性打开：**关着的时候连库文件都不建**（TDD-12 §2.1）。
    ///
    /// 它同时是本库的**管理锁**：写入、查询、清除都在这把锁里排队，于是"复验凭据"与
    /// "真的写进去"不可能被别的动作插进中间。
    store: Mutex<Option<WorkspaceMetrics>>,
    dir: PathBuf,
    /// 开关 + 世代：关掉与清除都开新一代，在途凭据据此作废。
    auth: MetricsAuthorization,
    /// 只在测试里存在的时序闸：让"取得凭据之后、抢到管理锁之前"这个窗口可以被确定性地
    /// 撑开。没有它，那条竞态用例只能靠反复跑去撞时序——而靠撞的用例既证明不了修好了，
    /// 也会变成 CI 里偶尔红一次的噪音。
    #[cfg(test)]
    commit_gate: std::sync::OnceLock<std::sync::Arc<std::sync::Barrier>>,
}

impl LocalWorkspaceMetrics {
    #[must_use]
    pub fn new(app_data_dir: PathBuf, enabled: bool) -> Self {
        Self {
            store: Mutex::new(None),
            dir: app_data_dir.join("metrics"),
            auth: MetricsAuthorization::new(enabled),
            #[cfg(test)]
            commit_gate: std::sync::OnceLock::new(),
        }
    }

    /// 开关变化（设置写入后调用）。
    ///
    /// **关掉只停止新增，不删历史**：用户关的是"以后别记了"，不是"把已经记的删掉"
    /// ——删除是另一颗按钮，且要二次确认（TDD-12 §2.1）。
    ///
    /// **但它同时收回在途凭据**（同上，"关闭授权与清除历史都使既有凭据失效"）：关掉
    /// 开关就是收回**这一段**的观测授权。此前它只写一个 bool、不动世代，于是
    /// `begin → 关 → 开 → commit` 全程无阻——那条跨越了未授权期的观测照旧落库，而用户
    /// 在那段时间里明确说过"别记了"。关后重开也不补报：新一代的凭据才是新的授权。
    pub fn set_enabled(&self, enabled: bool) {
        self.auth.set_enabled(enabled);
    }

    #[must_use]
    pub fn enabled(&self) -> bool {
        self.auth.enabled()
    }

    /// 开始一次观测；`None` = 此刻未授权，调用方**什么都不用做**。
    ///
    /// 返回凭据而不是布尔，是为了让"开始"与"提交"之间的那段时间可被检验。
    #[must_use]
    pub(crate) fn begin(&self) -> Option<WorkspaceMetricsGrant> {
        self.auth.begin().map(WorkspaceMetricsGrant)
    }

    /// 提交一次观测。失败**从不向上冒泡**——观测永远不能阻断用户的主流程。
    pub(crate) fn commit(&self, epoch: WorkspaceMetricsGrant, event: WorkspaceEvent, now_ms: i64) {
        #[cfg(test)]
        if let Some(gate) = self.commit_gate.get() {
            gate.wait();
        }
        // **复验在锁里，而且在惰性开库之前**（TDD-12 §2.1）：先在锁外查一遍再去抢锁，
        // 等于让清除完成之后仍在等锁的旧写者拿一张过期凭据把刚删掉的库**重新建出来**
        // ——用户按下「清除」，磁盘上却又冒出一个文件，他看到的"已清除"当场失效。
        // `clear` 先作废凭据再抢这把锁，因此等锁的写者一拿到锁就会在这里被拦下。
        let Ok(mut guard) = self.store.lock() else {
            return;
        };
        if !self.auth.is_current(epoch.0) {
            return;
        }
        if guard.is_none() {
            // 惰性开库：第一条要记的观测才建文件。
            match WorkspaceMetrics::open(&self.dir) {
                Ok(store) => *guard = Some(store),
                Err(error) => {
                    // **静默放弃**：观测坏了是我们的问题，不是用户的——它永远不能
                    // 阻断主流程，也不该在用户的控制台里刷存在感。原因不含任何内容。
                    let _ = error;
                    return;
                }
            }
        }
        if let Some(store) = guard.as_ref() {
            // 同上：写失败就放弃这一条，不重试、不上报、不打断任何人。
            let _ = store.record(event, now_ms);
        }
    }

    /// 一次记完（同步路径：开始与提交之间没有可插入的窗口）。
    pub fn record_now(&self, event: WorkspaceEvent, now_ms: i64) {
        if let Some(epoch) = self.begin() {
            self.commit(epoch, event, now_ms);
        }
    }

    /// 查一段窗口。
    ///
    /// **关掉授权后仍可读历史，且空库不因查询而创建**：用户关掉开关之后仍然有权看
    /// 自己已经记下的东西；而"打开设置页顺手建一个空库"会让"我从没打开过这个功能"的
    /// 用户磁盘上凭空多一个文件。
    pub fn summary(
        &self,
        start_day: i64,
        end_day: i64,
    ) -> Result<Result<Option<WorkspaceSummary>, StorageFailure>, String> {
        if start_day > end_day {
            return Err("WORKSPACE_METRICS_INVALID_RANGE: 时间范围不合法。".to_owned());
        }
        let Ok(mut guard) = self.store.lock() else {
            return Ok(Err(StorageFailure::StorageUnavailable));
        };
        if guard.is_none() {
            if !WorkspaceMetrics::database_path(&self.dir).exists() {
                return Ok(Ok(None));
            }
            match WorkspaceMetrics::open(&self.dir) {
                Ok(store) => *guard = Some(store),
                // **两种失败要分开说**：schema 对不上是「这个库我们不认识、也不会去动
                // 它」（fail closed），读不开是「此刻拿不到」。前者要让用户知道我们没在
                // 偷偷迁移或删他的东西，后者只是重试即可——一句笼统的"不可用"两件事都答不了。
                Err(luna_metrics::MetricsError::UnknownSchema(_)) => {
                    return Ok(Err(StorageFailure::SchemaInvalid))
                }
                Err(_) => return Ok(Err(StorageFailure::StorageUnavailable)),
            }
        }
        match guard
            .as_ref()
            .map(|store| store.summary(start_day, end_day))
        {
            None => Ok(Ok(None)),
            Some(Ok(summary)) => Ok(Ok(Some(summary))),
            // **读到不认识的维度取值也算「不认识这个库」**，不是「此刻拿不到」：这两态
            // 在面板上给的是**不同的出口**（重试 vs 清除重建）。报成 `storage_unavailable`
            // 会把用户送去一颗永远重试不出结果的按钮。
            Some(Err(luna_metrics::MetricsError::InvalidValue(_))) => {
                Ok(Err(StorageFailure::SchemaInvalid))
            }
            Some(Err(_)) => Ok(Err(StorageFailure::StorageUnavailable)),
        }
    }

    /// 清除：**先作废在途观测，再关连接，最后删文件**。
    ///
    /// 顺序不可换。先删文件再作废，那段窗口里提交的观测会把库重新建出来——用户按下
    /// 「清除」之后磁盘上又出现了一个文件，而他以为已经没有了。
    pub fn clear(&self) -> Result<(), String> {
        // 管理屏障同时作废旧凭据并阻止清除途中签发新凭据；离开作用域后按最新设置恢复。
        let _authorization_suspension = self.auth.suspend();
        let mut guard = self
            .store
            .lock()
            .map_err(|_| "WORKSPACE_METRICS_CLEAR_FAILED: 观测存储不可用。".to_owned())?;
        if let Some(store) = guard.take() {
            // **失败如实报错，不能报已清除**。
            return store
                .clear()
                .map_err(|error| format!("WORKSPACE_METRICS_CLEAR_FAILED: {error}"));
        }
        // 没开过库也要把可能存在的文件删掉（上次运行留下的）。
        //
        // **这里绝不能先 `open` 一次**：`open` 对未知 schema fail closed，于是坏 schema
        // 时清除永远失败——而清除重建正是 §2.1 给出的唯一恢复路径，堵死它等于把用户锁在
        // 一个既读不了、也删不掉的库上。`destroy` 不解析 schema，只按 `database_path` 算出
        // 要删的文件（因此也不可能碰到同目录下的知识度量库）。
        WorkspaceMetrics::destroy(&self.dir)
            .map_err(|error| format!("WORKSPACE_METRICS_CLEAR_FAILED: {error}"))
    }

    /// 撑开"取得凭据之后、抢到管理锁之前"那个窗口，让竞态用例不必靠撞时序。
    #[cfg(test)]
    fn install_commit_gate(&self, gate: std::sync::Arc<std::sync::Barrier>) {
        assert!(self.commit_gate.set(gate).is_ok(), "时序闸只能装一次");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luna_metrics::workspace::WorkspaceSurface;

    fn metrics(enabled: bool) -> (tempfile::TempDir, LocalWorkspaceMetrics) {
        let dir = tempfile::tempdir().unwrap();
        let m = LocalWorkspaceMetrics::new(dir.path().to_path_buf(), enabled);
        (dir, m)
    }

    const VIEW: WorkspaceEvent = WorkspaceEvent::SurfaceViewed {
        surface: WorkspaceSurface::Search,
    };

    #[test]
    fn while_the_switch_is_off_nothing_is_recorded_and_no_file_is_created() {
        // "不建库"是承诺的一部分：从没打开过这个功能的用户，磁盘上不该多一个文件。
        let (dir, m) = metrics(false);
        assert_eq!(m.begin(), None);
        m.record_now(VIEW, 0);
        assert!(!WorkspaceMetrics::database_path(dir.path().join("metrics")).exists());
        assert_eq!(
            m.summary(0, 10).unwrap().unwrap(),
            None,
            "空库不因查询而创建"
        );
    }

    #[test]
    fn turning_it_off_stops_new_records_but_keeps_the_history() {
        // 用户关的是"以后别记了"，不是"把已经记的删掉"。
        let (_dir, m) = metrics(true);
        m.record_now(VIEW, 0);
        m.set_enabled(false);
        m.record_now(VIEW, 0);
        let summary = m.summary(0, 10).unwrap().unwrap().unwrap();
        assert_eq!(summary.surfaces[0].count, 1, "关掉后不再新增");
        // 重新打开之后照常记新的——收回的是**那一段**的授权，不是把功能停掉。
        m.set_enabled(true);
        m.record_now(VIEW, 0);
        let summary = m.summary(0, 10).unwrap().unwrap().unwrap();
        assert_eq!(summary.surfaces[0].count, 2, "重开之后新的观测照记");
    }

    #[test]
    fn an_observation_that_spans_a_clear_is_dropped() {
        // 后台观测跨越几秒到几分钟；这期间用户清了一次库，那条观测就不该再落进去
        // ——否则他按下「清除」之后，库里又冒出一条来自"清除之前"的记录。
        let (dir, m) = metrics(true);
        m.record_now(VIEW, 0);
        let epoch = m.begin().expect("已授权");
        m.clear().unwrap();
        m.commit(epoch, VIEW, 0);
        assert!(
            !WorkspaceMetrics::database_path(dir.path().join("metrics")).exists(),
            "作废的观测不得把库重建出来"
        );
        assert_eq!(m.summary(0, 10).unwrap().unwrap(), None);
    }

    #[test]
    fn an_observation_that_spans_the_switch_being_turned_off_is_dropped() {
        // 判据是**凭据**，不是"此刻开关开着没有"：这里在提交前把开关重新打开，于是
        // `enabled()` 为真、库也可写——唯一还能拦下它的只有那张跨越了未授权期的凭据。
        // 旧写法（只看 `!self.enabled()`）在这个序列上一路放行，而观测跨过的正是用户
        // 明确说过"别记了"的那一段（TDD-12 §2.1「关后重开不补报」）。
        let (dir, m) = metrics(true);
        let epoch = m.begin().expect("已授权");
        m.set_enabled(false);
        m.set_enabled(true);
        assert!(m.enabled(), "前提：提交时开关是开着的");
        m.commit(epoch, VIEW, 0);
        assert!(
            !WorkspaceMetrics::database_path(dir.path().join("metrics")).exists(),
            "关后重开不补报，连库都不该建出来"
        );
        // 新的一段授权照常工作——作废的是那张凭据，不是这个功能。
        m.record_now(VIEW, 0);
        assert_eq!(
            m.summary(0, 10).unwrap().unwrap().unwrap().surfaces[0].count,
            1
        );
    }

    #[test]
    fn the_credential_is_re_checked_inside_the_management_lock_and_before_any_lazy_open() {
        // F04 的根因是**顺序**，而顺序恰好是行为测试从外面看不见的东西：把复验挪回锁外
        // 之后，本文件里所有竞态用例照样全绿——因为"过了检查、正在等锁"这个中间态没有任何
        // 外部手段能停住它（能停住它的探针只在有 bug 的那个版本里才有位置可放）。这类只能
        // 靠结构看住的判据，本仓已有先例（`ipc/knowledge.rs` 的"纯登记版不碰 vault 锁"）。
        //
        // 两条顺序都要：复验在**取锁之后**（否则清除完成后仍在等锁的旧写者会过检），
        // 且在**惰性开库之前**（否则它拿到锁先把库建出来，再发现自己没资格写）。
        let body_src = crate::ipc::source_without_tests(include_str!("workspace_metrics.rs"));
        let body = body_src
            .split("pub(crate) fn commit(")
            .nth(1)
            .expect("找不到 commit")
            .split("\n    /// ")
            .next()
            .expect("commit 的边界变了");
        let lock = body.find("self.store.lock()").expect("commit 必须取管理锁");
        let recheck = body
            .find("self.auth.is_current(epoch.0)")
            .expect("commit 必须复验凭据");
        let lazy_open = body
            .find("WorkspaceMetrics::open(")
            .expect("commit 必须有惰性开库分支");
        assert!(lock < recheck, "凭据复验必须在管理锁之内（TDD-12 §2.1）");
        assert!(recheck < lazy_open, "复验必须在惰性开库之前");
    }

    #[test]
    fn a_commit_already_under_way_cannot_rebuild_a_database_that_was_just_cleared() {
        // F04 的核心时序，用屏障**强制**出来而不是靠撞：提交方取得凭据后停在
        // "还没抢到管理锁"的位置，清除完整跑完，它才继续。旧写法在这里会把库重新建出来
        // ——epoch 检查在锁外，等锁期间库已被删，拿到锁后惰性开库分支照建不误。
        let (dir, m) = metrics(true);
        m.record_now(VIEW, 0);
        let path = WorkspaceMetrics::database_path(dir.path().join("metrics"));
        assert!(path.exists(), "前提：清除前确实有一个库");

        let gate = std::sync::Arc::new(std::sync::Barrier::new(2));
        m.install_commit_gate(std::sync::Arc::clone(&gate));
        let epoch = m.begin().expect("已授权");
        std::thread::scope(|scope| {
            scope.spawn(|| m.commit(epoch, VIEW, 0));
            m.clear().expect("清除必须成功");
            // 清除已经返回，才放这条在途观测过去。
            gate.wait();
        });

        assert!(!path.exists(), "「已清除」不得当场失效");
        assert_eq!(m.summary(0, 10).unwrap().unwrap(), None);
    }

    #[test]
    fn a_commit_racing_the_clear_never_leaves_a_database_behind() {
        // 上一条钉的是"清除已完成"这一个时刻；这条把两者放到同一瞬间起跑，覆盖"提交方
        // 正卡在锁上、清除方在锁里删库"的那一半。无论谁先拿到锁，结局只有一个：清除返回
        // 之后磁盘上没有这个库。
        for _ in 0..50 {
            let (dir, m) = metrics(true);
            m.record_now(VIEW, 0);
            let path = WorkspaceMetrics::database_path(dir.path().join("metrics"));
            let gate = std::sync::Arc::new(std::sync::Barrier::new(2));
            m.install_commit_gate(std::sync::Arc::clone(&gate));
            let epoch = m.begin().expect("已授权");
            std::thread::scope(|scope| {
                scope.spawn(|| m.commit(epoch, VIEW, 0));
                gate.wait();
                m.clear().expect("清除必须成功");
            });
            assert!(!path.exists(), "清除返回之后不得还有库");
        }
    }

    #[test]
    fn a_range_the_wrong_way_round_gets_the_stable_code() {
        let (_dir, m) = metrics(true);
        let err = m.summary(9, 3).unwrap_err();
        assert!(err.starts_with("WORKSPACE_METRICS_INVALID_RANGE"), "{err}");
    }

    #[test]
    fn clearing_twice_is_not_an_error_and_leaves_nothing_behind() {
        let (dir, m) = metrics(true);
        m.record_now(VIEW, 0);
        m.clear().unwrap();
        m.clear().unwrap();
        assert!(!WorkspaceMetrics::database_path(dir.path().join("metrics")).exists());
    }

    #[test]
    fn a_database_we_cannot_read_can_still_be_cleared_and_rebuilt() {
        // F07：坏 schema 一律 fail closed，而**显式清除重建是唯一恢复路径**
        //（TDD-12 §2.1）。此前"未持连接"那条分支先 `open` 再 `clear`，于是坏 schema 时清除
        // 永远失败——面板永远显示 schema_invalid、按钮永远报错，用户被锁死在里面。
        // 既有用例撞不到它：它们都先记一条，走的永远是"持连接"那半边。
        let dir = tempfile::tempdir().unwrap();
        let metrics_dir = dir.path().join("metrics");
        std::fs::create_dir_all(&metrics_dir).unwrap();
        let path = WorkspaceMetrics::database_path(&metrics_dir);
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE alien(x); PRAGMA user_version=99;")
            .unwrap();
        drop(conn);

        let m = LocalWorkspaceMetrics::new(dir.path().to_path_buf(), true);
        assert_eq!(
            m.summary(0, 10).unwrap(),
            Err(StorageFailure::SchemaInvalid),
            "先得如实说我们不认识这个库"
        );
        m.clear().expect("坏 schema 也必须能清掉");
        assert!(!path.exists());
        // 清完之后是一个**新的观测周期**：能重新记、能重新读。
        m.record_now(VIEW, 0);
        assert_eq!(
            m.summary(0, 10).unwrap().unwrap().unwrap().surfaces[0].count,
            1
        );
    }

    #[test]
    fn a_database_from_the_previous_schema_generation_is_what_the_user_actually_meets() {
        // 上一条造的是"外星表"，而 schema 抬升之后用户真正会遇到的是**上一代自己写的
        // 库**：表全在、只少一张新表，`user_version` 停在上一代。这条把那个真实场景走完
        //——如实报 `schema_invalid`（面板据此给出清除出口），清掉之后是一个新的观测周期。
        let dir = tempfile::tempdir().unwrap();
        let metrics_dir = dir.path().join("metrics");
        std::fs::create_dir_all(&metrics_dir).unwrap();
        let path = WorkspaceMetrics::database_path(&metrics_dir);
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE workspace_surfaces (day INTEGER NOT NULL, surface TEXT NOT NULL,
               count INTEGER NOT NULL, PRIMARY KEY (day, surface)) WITHOUT ROWID;
             CREATE TABLE workspace_entries (day INTEGER NOT NULL, action TEXT NOT NULL,
               entry TEXT NOT NULL, result TEXT NOT NULL, bucket TEXT NOT NULL,
               count INTEGER NOT NULL,
               PRIMARY KEY (day, action, entry, result, bucket)) WITHOUT ROWID;
             CREATE TABLE workspace_settled (day INTEGER NOT NULL PRIMARY KEY,
               count INTEGER NOT NULL) WITHOUT ROWID;
             CREATE TABLE workspace_imports (day INTEGER NOT NULL, result TEXT NOT NULL,
               bucket TEXT NOT NULL, count INTEGER NOT NULL,
               PRIMARY KEY (day, result, bucket)) WITHOUT ROWID;
             INSERT INTO workspace_surfaces VALUES (0, 'search', 7);
             PRAGMA user_version=1;",
        )
        .unwrap();
        drop(conn);

        let m = LocalWorkspaceMetrics::new(dir.path().to_path_buf(), true);
        assert_eq!(
            m.summary(0, 10).unwrap(),
            Err(StorageFailure::SchemaInvalid),
            "上一代的库必须如实报 schema_invalid，而不是当成空库"
        );
        // 写入同样被拦下：不迁移、不双读双写，也不往一个我们不认识的库里追加（ADR-046）。
        m.record_now(VIEW, 0);
        assert_eq!(
            m.summary(0, 10).unwrap(),
            Err(StorageFailure::SchemaInvalid)
        );

        m.clear().expect("清除重建是唯一恢复路径，必须走得通");
        assert!(!path.exists());
        m.record_now(VIEW, 0);
        assert_eq!(
            m.summary(0, 10).unwrap().unwrap().unwrap().surfaces[0].count,
            1,
            "清除之后是一个新的观测周期"
        );
    }

    #[test]
    fn clearing_the_workspace_history_never_touches_the_knowledge_database() {
        // N16：两套观测同处 `<app-data>/metrics/`，各有各的按钮与边界。这里不满足于
        // "路径不同"——真的把知识度量库建出来，清掉工作区那份，再逐字节确认它还在。
        let (dir, m) = metrics(true);
        m.record_now(VIEW, 0);
        let metrics_dir = dir.path().join("metrics");
        let knowledge = luna_metrics::MetricsStore::open(&metrics_dir, vec![3; 32]).unwrap();
        knowledge
            .suggestion(
                "/v",
                "token",
                luna_metrics::SuggestionEvent::Shown,
                luna_metrics::SuggestionSurface::Graph,
                0,
            )
            .unwrap();
        drop(knowledge);
        let knowledge_path = luna_metrics::MetricsStore::database_path(&metrics_dir);
        let before = std::fs::read(&knowledge_path).unwrap();

        m.clear().unwrap();

        assert!(!WorkspaceMetrics::database_path(&metrics_dir).exists());
        assert!(knowledge_path.exists(), "清工作区不得动知识库");
        assert_eq!(std::fs::read(&knowledge_path).unwrap(), before);
    }

    #[test]
    fn recording_never_panics_or_propagates_when_the_store_cannot_open() {
        // 观测坏了是我们的问题，不是用户的——它永远不能阻断主流程。
        let dir = tempfile::tempdir().unwrap();
        let blocked = dir.path().join("metrics");
        std::fs::write(&blocked, b"not a directory").unwrap();
        let m = LocalWorkspaceMetrics::new(dir.path().to_path_buf(), true);
        m.record_now(VIEW, 0); // 不 panic、不返回错误
    }
}
