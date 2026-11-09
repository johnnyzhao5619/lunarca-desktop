//! 引擎场景用例：两台（或三台）设备对着同一个内存服务端同步，逐条钉住方案 §4.6 的决策表与护栏。

use std::sync::Arc;

use uuid::Uuid;

use crate::engine::{Engine, EngineConfig, RoundReport, SyncAbort};
use crate::ports::RemoteError;
use crate::state::{Binding, SyncState};
use crate::testing::{FakeServer, MemVault};
use luna_sync_core::crypto::{Cmk, KeyRing};

const LOCAL_ID: &str = "0190f0f0-0000-7000-8000-00000000abcd";

struct Device {
    vault: MemVault,
    state: SyncState,
}

struct World {
    server: Arc<FakeServer>,
    ring: KeyRing,
    max_chunks: usize,
    max_file_bytes: std::cell::Cell<Option<u64>>,
    recordings: std::cell::Cell<crate::policy::RecordingsMode>,
    defer_large: std::cell::Cell<bool>,
}

impl World {
    fn new() -> Self {
        let vault_id = Uuid::now_v7();
        let cmk = Cmk::generate(1);
        Self {
            server: Arc::new(FakeServer::new(vault_id)),
            ring: KeyRing::new(vault_id, &[cmk]).unwrap(),
            max_chunks: crate::engine::MAX_CHUNKS_PER_ENTRY,
            max_file_bytes: std::cell::Cell::new(None),
            recordings: std::cell::Cell::new(crate::policy::RecordingsMode::Full),
            defer_large: std::cell::Cell::new(false),
        }
    }

    fn device(&self) -> Device {
        let mut state = SyncState::in_memory().unwrap();
        state
            .bind(&Binding {
                remote_vault_id: self.server.vault_id,
                server_origin: "https://sync.example.test".into(),
                local_vault_id: LOCAL_ID.into(),
                created_ms: 0,
                owner: crate::state::BindingOwner {
                    server_bucket: "sync.example.test".into(),
                    account_id: "acc".into(),
                    account_label: "a***@sync.example.test".into(),
                },
            })
            .unwrap();
        Device {
            vault: MemVault::new(LOCAL_ID),
            state,
        }
    }

    fn engine<'a>(&'a self, vault: &'a MemVault, allow_bulk_delete: bool) -> Engine<'a> {
        Engine {
            remote: self.server.as_ref(),
            local: vault,
            ring: &self.ring,
            config: EngineConfig {
                conflict_label: "冲突副本".into(),
                today: "2026-10-09".into(),
                allow_bulk_delete,
                now_ms: 0,
                max_chunks_per_entry: self.max_chunks,
                max_file_bytes: self.max_file_bytes.get(),
                recordings: self.recordings.get(),
                defer_large: self.defer_large.get(),
            },
        }
    }

    async fn try_sync(
        &self,
        device: &mut Device,
        allow_bulk_delete: bool,
    ) -> Result<RoundReport, SyncAbort> {
        let engine = self.engine(&device.vault, allow_bulk_delete);
        engine.run_round(&mut device.state).await
    }

    async fn sync(&self, device: &mut Device) -> RoundReport {
        self.try_sync(device, false).await.expect("sync round")
    }
}

fn text(device: &Device, path: &str) -> Option<String> {
    device
        .vault
        .get(path)
        .map(|b| String::from_utf8(b).unwrap())
}

#[tokio::test]
async fn files_created_on_one_device_arrive_on_the_other() {
    let world = World::new();
    let (mut a, mut b) = (world.device(), world.device());
    a.vault.put("notes/a.md", b"# A\n");
    a.vault.put("img/x.png", &[0x89, 0x50, 0x4e, 0x47]);
    let report = world.sync(&mut a).await;
    assert_eq!(report.pushed, 2);
    assert!(report.full_reconcile);

    let report = world.sync(&mut b).await;
    assert_eq!(report.applied, 2);
    assert_eq!(b.vault.snapshot(), a.vault.snapshot());

    // 再跑一轮：两边都没事可做（自己的提交拉回来不算改动）。
    for device in [&mut a, &mut b] {
        let report = world.sync(device).await;
        assert_eq!((report.pushed, report.applied, report.deferred), (0, 0, 0));
        assert!(!report.full_reconcile, "有游标之后走增量");
    }
}

#[tokio::test]
async fn non_overlapping_edits_merge_and_converge() {
    let world = World::new();
    let (mut a, mut b) = (world.device(), world.device());
    a.vault.put("a.md", b"line 1\nline 2\nline 3\n");
    world.sync(&mut a).await;
    world.sync(&mut b).await;

    a.vault.put("a.md", b"line 1 (A)\nline 2\nline 3\n");
    b.vault.put("a.md", b"line 1\nline 2\nline 3 (B)\n");
    world.sync(&mut a).await;
    let report = world.sync(&mut b).await;
    assert_eq!((report.merged, report.conflict_copies), (1, 0));
    world.sync(&mut a).await;
    // 合并前，B 本地那份编辑留了版本（可撤回）。
    assert!(b
        .vault
        .versions()
        .contains(&("a.md".to_owned(), b"line 1\nline 2\nline 3 (B)\n".to_vec())));
    let merged = "line 1 (A)\nline 2\nline 3 (B)\n";
    assert_eq!(text(&a, "a.md").as_deref(), Some(merged));
    assert_eq!(text(&b, "a.md").as_deref(), Some(merged));
    assert_eq!(a.vault.snapshot(), b.vault.snapshot());
}

#[tokio::test]
async fn overlapping_edits_keep_both_versions_as_a_conflict_copy() {
    let world = World::new();
    let (mut a, mut b) = (world.device(), world.device());
    a.vault.put("a.md", b"same line\n");
    world.sync(&mut a).await;
    world.sync(&mut b).await;

    a.vault.put("a.md", b"A wrote this\n");
    b.vault.put("a.md", b"B wrote this\n");
    world.sync(&mut a).await;
    let report = world.sync(&mut b).await;
    assert_eq!(report.conflict_copies, 1);
    world.sync(&mut a).await;

    // 主文件是后同步那台（B）的版本，A 的版本在冲突副本里；两台一致，没有冲突标记。
    let copy = "a (冲突副本 2026-10-09).md";
    for device in [&a, &b] {
        assert_eq!(text(device, "a.md").as_deref(), Some("B wrote this\n"));
        assert_eq!(text(device, copy).as_deref(), Some("A wrote this\n"));
        assert!(!text(device, "a.md").unwrap().contains("<<<<<<<"));
    }
}

#[tokio::test]
async fn a_remote_deletion_goes_to_the_local_trash() {
    let world = World::new();
    let (mut a, mut b) = (world.device(), world.device());
    a.vault.put("a.md", b"x");
    world.sync(&mut a).await;
    world.sync(&mut b).await;

    a.vault.remove("a.md");
    assert_eq!(world.sync(&mut a).await.deleted_remote, 1);
    assert_eq!(world.sync(&mut b).await.deleted_local, 1);
    assert_eq!(b.vault.get("a.md"), None);
    assert_eq!(b.vault.trash(), vec![("a.md".to_owned(), b"x".to_vec())]);
    assert_eq!(world.server.live_files(), 0);
}

#[tokio::test]
async fn an_edit_beats_a_deletion_in_both_directions() {
    let world = World::new();
    let (mut a, mut b) = (world.device(), world.device());
    a.vault.put("a.md", b"v1");
    a.vault.put("b.md", b"v1");
    world.sync(&mut a).await;
    world.sync(&mut b).await;

    // a.md：A 删、B 改。b.md：A 改、B 删。
    a.vault.remove("a.md");
    a.vault.put("b.md", b"A edit");
    b.vault.put("a.md", b"B edit");
    b.vault.remove("b.md");
    world.sync(&mut a).await;
    world.sync(&mut b).await;
    world.sync(&mut a).await;
    world.sync(&mut b).await;

    for device in [&a, &b] {
        assert_eq!(text(device, "a.md").as_deref(), Some("B edit"));
        assert_eq!(text(device, "b.md").as_deref(), Some("A edit"));
    }
    assert_eq!(world.server.live_files(), 2);
}

#[tokio::test]
async fn a_rename_moves_the_file_instead_of_duplicating_it() {
    let world = World::new();
    let (mut a, mut b) = (world.device(), world.device());
    a.vault.put("old.md", b"content");
    world.sync(&mut a).await;
    world.sync(&mut b).await;

    a.vault.rename("old.md", "dir/new.md");
    let report = world.sync(&mut a).await;
    assert_eq!(
        (report.pushed, report.deleted_remote),
        (1, 0),
        "同一个 file_id 换了路径"
    );
    world.sync(&mut b).await;
    assert_eq!(b.vault.get("old.md"), None);
    assert_eq!(text(&b, "dir/new.md").as_deref(), Some("content"));
    assert!(b.vault.trash().is_empty(), "改名不是删除");
    assert_eq!(world.server.live_files(), 1);
}

#[tokio::test]
async fn an_unobserved_disappearance_never_becomes_a_deletion() {
    let world = World::new();
    let (mut a, mut b) = (world.device(), world.device());
    a.vault.put("a.md", b"x");
    a.vault.put("b.md", b"y");
    world.sync(&mut a).await;
    world.sync(&mut b).await;

    a.vault.make_uncertain("a.md");
    let report = world.sync(&mut a).await;
    assert_eq!(report.deleted_remote, 0);
    world.sync(&mut b).await;
    assert_eq!(text(&b, "a.md").as_deref(), Some("x"));

    // 云端在此期间改了它：A 不往一个说不准的位置写，留到下一轮；游标不前进。
    b.vault.put("a.md", b"x2");
    world.sync(&mut b).await;
    let report = world.sync(&mut a).await;
    assert_eq!((report.applied, report.deferred), (0, 1));
    // 文件回来了（下载完成）：下一轮照常应用。
    a.vault.put("a.md", b"x");
    a.vault.settle("a.md");
    let report = world.sync(&mut a).await;
    assert_eq!(report.applied, 1);
    assert_eq!(text(&a, "a.md").as_deref(), Some("x2"));
}

#[tokio::test]
async fn the_bulk_delete_breaker_holds_until_confirmed() {
    let world = World::new();
    let mut a = world.device();
    for i in 0..60 {
        a.vault.put(&format!("n{i}.md"), format!("{i}").as_bytes());
    }
    a.vault.put("keep.md", b"keep");
    world.sync(&mut a).await;

    for i in 0..60 {
        a.vault.remove(&format!("n{i}.md"));
    }
    let report = world.sync(&mut a).await;
    assert_eq!(report.held_deletions.len(), 60);
    assert_eq!(report.deleted_remote, 0);
    assert_eq!(world.server.live_files(), 61);

    let report = world.try_sync(&mut a, true).await.unwrap();
    assert_eq!(report.deleted_remote, 60);
    assert_eq!(world.server.live_files(), 1);
}

#[tokio::test]
async fn an_empty_side_is_never_taken_as_a_mass_deletion() {
    let world = World::new();
    let (mut a, mut b) = (world.device(), world.device());
    a.vault.put("a.md", b"x");
    a.vault.put("b.md", b"y");
    world.sync(&mut a).await;
    world.sync(&mut b).await;

    // 本地一个文件都扫不到（卷没挂好、文件被别的工具挪空）：整轮中止，云端不动。
    b.vault.remove("a.md");
    b.vault.remove("b.md");
    assert!(matches!(
        world.try_sync(&mut b, false).await,
        Err(SyncAbort::LocalLooksEmpty)
    ));
    assert_eq!(world.server.live_files(), 2);
    // 用户确认「确实要清空」：放行。
    let report = world.try_sync(&mut b, true).await.unwrap();
    assert_eq!(report.deleted_remote, 2);
    assert_eq!(world.server.live_files(), 0);
}

#[tokio::test]
async fn an_epoch_change_falls_back_to_a_full_reconcile_without_duplicates() {
    let world = World::new();
    let (mut a, mut b) = (world.device(), world.device());
    a.vault.put("a.md", b"x");
    world.sync(&mut a).await;
    world.sync(&mut b).await;

    world.server.change_epoch();
    b.vault.put("b.md", b"new");
    let report = world.sync(&mut b).await;
    assert!(report.full_reconcile);
    assert_eq!((report.pushed, report.applied), (1, 0));
    world.sync(&mut a).await;
    assert_eq!(a.vault.snapshot(), b.vault.snapshot());
    assert_eq!(world.server.live_files(), 2);
}

#[tokio::test]
async fn a_lost_state_database_reconciles_by_content_without_reuploading() {
    let world = World::new();
    let mut a = world.device();
    a.vault.put("a.md", b"x");
    a.vault.put("b.md", b"y");
    world.sync(&mut a).await;
    let seq = world.server.head_seq();

    // 状态库丢了：同一份库重新绑定，全量对账按内容认回来，不重复上传、不生成副本。
    let mut fresh = world.device();
    fresh.vault = MemVault::new(LOCAL_ID);
    fresh.vault.put("a.md", b"x");
    fresh.vault.put("b.md", b"y");
    let report = world.sync(&mut fresh).await;
    assert_eq!(
        (report.pushed, report.conflict_copies, report.applied),
        (0, 0, 0)
    );
    assert_eq!(world.server.head_seq(), seq);
}

#[tokio::test]
async fn a_swapped_vault_aborts_the_round() {
    let world = World::new();
    let mut a = world.device();
    a.vault = MemVault::new("0190f0f0-0000-7000-8000-00000000ffff");
    a.vault.put("a.md", b"x");
    assert!(matches!(
        world.try_sync(&mut a, false).await,
        Err(SyncAbort::IdentityMismatch)
    ));
}

#[tokio::test]
async fn a_network_failure_aborts_without_committing_anything() {
    let world = World::new();
    let mut a = world.device();
    a.vault.put("a.md", b"x");
    world
        .server
        .fail_next(RemoteError::Network("timeout".into()));
    assert!(world.try_sync(&mut a, false).await.is_err());
    assert!(a.state.bases().unwrap().is_empty());
    // 下一轮照常。
    assert_eq!(world.sync(&mut a).await.pushed, 1);
}

#[tokio::test]
async fn binary_conflicts_get_a_copy() {
    let world = World::new();
    let (mut a, mut b) = (world.device(), world.device());
    a.vault.put("x.png", &[1, 2, 3]);
    world.sync(&mut a).await;
    world.sync(&mut b).await;
    a.vault.put("x.png", &[1, 2, 4]);
    b.vault.put("x.png", &[1, 2, 5]);
    world.sync(&mut a).await;
    assert_eq!(world.sync(&mut b).await.conflict_copies, 1);
    world.sync(&mut a).await;
    assert_eq!(a.vault.snapshot(), b.vault.snapshot());
    assert_eq!(
        a.vault.get("x (冲突副本 2026-10-09).png"),
        Some(vec![1, 2, 4])
    );
}

#[tokio::test]
async fn both_sides_creating_the_same_path_keep_both() {
    let world = World::new();
    let (mut a, mut b) = (world.device(), world.device());
    a.vault.put("a.md", b"seed");
    world.sync(&mut a).await;
    world.sync(&mut b).await;
    a.vault.put("new.md", b"from A");
    b.vault.put("new.md", b"from B");
    world.sync(&mut a).await;
    world.sync(&mut b).await;
    world.sync(&mut a).await;
    assert_eq!(a.vault.snapshot(), b.vault.snapshot());
    let files = a.vault.snapshot();
    let contents: Vec<&[u8]> = files.values().map(Vec::as_slice).collect();
    assert!(contents.contains(&&b"from A"[..]) && contents.contains(&&b"from B"[..]));
}

#[tokio::test]
async fn a_shared_baseline_survives_deleting_its_twin() {
    let world = World::new();
    let (mut a, mut b) = (world.device(), world.device());
    // 两个文件内容完全相同：钉住的是同一个指纹。
    a.vault.put("a.md", b"one\ntwo\nthree\n");
    a.vault.put("b.md", b"one\ntwo\nthree\n");
    world.sync(&mut a).await;
    world.sync(&mut b).await;

    // 删掉其中一个：另一个的基线不能跟着被解钉，否则之后的合并只能退化成冲突副本。
    a.vault.remove("b.md");
    world.sync(&mut a).await;
    world.sync(&mut b).await;

    a.vault.put("a.md", b"one (A)\ntwo\nthree\n");
    b.vault.put("a.md", b"one\ntwo\nthree (B)\n");
    world.sync(&mut a).await;
    let report = world.sync(&mut b).await;
    assert_eq!((report.merged, report.conflict_copies), (1, 0));
}

#[tokio::test]
async fn a_lost_commit_reply_reconciles_without_duplicates() {
    let world = World::new();
    let mut a = world.device();
    a.vault.put("a.md", b"x");
    world.server.lose_next_commit_reply();
    assert!(world.try_sync(&mut a, false).await.is_err());
    assert_eq!(world.server.live_files(), 1, "服务端其实已经收下了");
    // 下一轮：内容一样 → 只推进基线，不重复上传、不生成副本。
    let report = world.sync(&mut a).await;
    assert_eq!((report.pushed, report.conflict_copies), (0, 0));
    assert_eq!(world.server.live_files(), 1);
    assert_eq!(a.vault.snapshot().len(), 1);
}

/// 存储满了（ADR-152）：新内容进不去，但删除照常同步——删除先于块上传单独提交，块上传的
/// 402 不能把同一轮里的删除一起挡在外面；腾出空间后新内容随下一轮补上。
#[tokio::test]
async fn deletions_still_reach_a_full_server_and_new_content_follows_once_space_frees() {
    let world = World::new();
    let (mut a, mut b) = (world.device(), world.device());
    a.vault.put("a.md", b"# A\n");
    a.vault.put("b.md", b"# B\n");
    world.sync(&mut a).await;
    world.sync(&mut b).await;
    assert_eq!(world.server.live_files(), 2);

    world.server.set_full(true);
    a.vault.remove("a.md");
    a.vault.put("c.md", b"# C\n");
    let aborted = world.try_sync(&mut a, false).await;
    assert!(
        matches!(aborted, Err(SyncAbort::Remote(RemoteError::QuotaExceeded))),
        "{aborted:?}"
    );
    assert_eq!(
        world.server.live_files(),
        1,
        "删除已经落到服务端，新文件没进去"
    );
    world.sync(&mut b).await;
    assert_eq!(text(&b, "a.md"), None, "另一台设备收到了删除");
    assert_eq!(text(&b, "c.md"), None);

    world.server.set_full(false);
    world.sync(&mut a).await;
    world.sync(&mut b).await;
    assert_eq!(text(&b, "c.md").as_deref(), Some("# C\n"));
    assert_eq!(b.vault.snapshot(), a.vault.snapshot());
}

/// 2026-10-11 真机：一轮写下冲突副本后中止（基线没落盘），下一轮不认那份副本、另起一个名字，
/// 一个文件 50 秒里生成了 20 份。重试必须认回内容相同的那一份。
#[tokio::test]
async fn retrying_an_aborted_round_reuses_its_conflict_copy() {
    let world = World::new();
    let (mut a, mut b) = (world.device(), world.device());
    a.vault.put("x.png", &[1, 2, 3]);
    world.sync(&mut a).await;
    // 新设备上同名、不同内容的文件：云端那份只能另存为冲突副本；而上传一直被拒，整轮中止。
    b.vault.put("x.png", &[9, 9, 9]);
    world.server.set_full(true);
    for _ in 0..3 {
        assert!(world.try_sync(&mut b, false).await.is_err());
    }
    world.server.set_full(false);
    world.sync(&mut b).await;
    let copies: Vec<_> = b
        .vault
        .snapshot()
        .into_keys()
        .filter(|path| path.contains("冲突副本"))
        .collect();
    assert_eq!(copies, ["x (冲突副本 2026-10-09).png"]);
}

/// 整库时间点恢复（PRD-13-007）：恢复出某个序号时的样子到新文件夹，不碰当前库；目标不空就拒。
#[tokio::test]
async fn a_point_in_time_restores_into_a_new_folder_without_touching_the_vault() {
    use crate::restore::{restore_to_folder, RestoreError};
    let world = World::new();
    let mut a = world.device();
    a.vault.put("notes/a.md", b"version one");
    a.vault.put("b.md", b"keep me");
    world.sync(&mut a).await;
    let then = world.server.head_seq();
    a.vault.put("notes/a.md", b"version two");
    a.vault.put("c.md", b"added later");
    world.sync(&mut a).await;
    assert!(world.server.head_seq() > then);
    let before = a.vault.snapshot();

    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("恢复");
    let mut seen = Vec::new();
    let report = restore_to_folder(
        world.server.as_ref(),
        &world.ring,
        world.server.vault_id,
        then,
        &dest,
        |done, total| seen.push((done, total)),
    )
    .await
    .unwrap();
    assert_eq!((report.files, report.skipped), (2, 0));
    assert_eq!(
        std::fs::read(dest.join("notes/a.md")).unwrap(),
        b"version one"
    );
    assert_eq!(std::fs::read(dest.join("b.md")).unwrap(), b"keep me");
    assert!(!dest.join("c.md").exists(), "那一刻还没有它");
    assert_eq!(seen.last(), Some(&(2, 2)));
    assert_eq!(a.vault.snapshot(), before, "当前库一个字节没动");

    // 目标已有东西：拒绝，什么都不写。
    let err = restore_to_folder(
        world.server.as_ref(),
        &world.ring,
        world.server.vault_id,
        then,
        &dest,
        |_, _| {},
    )
    .await
    .unwrap_err();
    assert!(matches!(err, RestoreError::DestinationNotEmpty));
}

/// 恢复做到一半断了：不留半成品。目标与暂存都不在，再试一次照样能用同一个名字。
#[tokio::test]
async fn a_restore_that_fails_halfway_leaves_nothing_behind() {
    use crate::restore::restore_to_folder;
    let world = World::new();
    let mut a = world.device();
    for i in 0..4 {
        a.vault
            .put(&format!("n{i}.md"), format!("note {i}").as_bytes());
    }
    world.sync(&mut a).await;
    let at = world.server.head_seq();

    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("恢复");
    // 清单 + 第一个文件的块放行，之后断网。
    world
        .server
        .fail_after(2, RemoteError::Network("断网".into()));
    let err = restore_to_folder(
        world.server.as_ref(),
        &world.ring,
        world.server.vault_id,
        at,
        &dest,
        |_, _| {},
    )
    .await;
    assert!(err.is_err());
    let left: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
    assert!(left.is_empty(), "留下了半成品：{left:?}");

    // 用户事先建好的空文件夹也收；恢复完它就是完整的那一份。
    world.server.heal();
    std::fs::create_dir(&dest).unwrap();
    let report = restore_to_folder(
        world.server.as_ref(),
        &world.ring,
        world.server.vault_id,
        at,
        &dest,
        |_, _| {},
    )
    .await
    .unwrap();
    assert_eq!(report.files, 4);
    assert_eq!(std::fs::read(dest.join("n3.md")).unwrap(), b"note 3");
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        1,
        "暂存没收干净"
    );
}

/// 还在写的大文件（正在录的 WAV）：这一轮不推、不当成改名，写完之后再推一次完整的。
#[tokio::test]
async fn a_file_still_being_written_waits_until_it_settles() {
    let world = World::new();
    let mut a = world.device();
    a.vault.put("note.md", b"hello");
    a.vault.put("rec/live.wav", b"partial");
    a.vault.set_settling("rec/live.wav", true);
    let report = world.sync(&mut a).await;
    assert_eq!(report.pushed, 1, "只推了笔记");
    assert!(report.deferred >= 1);

    a.vault.put("rec/live.wav", b"partial plus the rest");
    a.vault.set_settling("rec/live.wav", false);
    let report = world.sync(&mut a).await;
    assert_eq!(report.pushed, 1);

    let mut b = world.device();
    world.sync(&mut b).await;
    assert_eq!(
        b.vault.snapshot().get("rec/live.wav").map(Vec::as_slice),
        Some(&b"partial plus the rest"[..])
    );
}

/// 超过一个条目能引用的块数的文件：不上传、记下来，其余文件照常同步（而不是整轮中止）。
#[tokio::test]
async fn a_file_with_too_many_chunks_is_skipped_without_stopping_the_round() {
    // 上限压到 1 块：9 MiB 的文件按 FastCDC 最大块 4 MiB 至少切成三块。
    let mut world = World::new();
    world.max_chunks = 1;
    let mut a = world.device();
    a.vault.put("note.md", b"hello");
    let big: Vec<u8> = (0..9 * 1024 * 1024u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8)
        .collect();
    a.vault.put("rec/long.wav", &big);
    let report = world.sync(&mut a).await;
    assert_eq!(report.pushed, 1, "笔记照常推上去");
    assert_eq!(report.too_large, vec!["rec/long.wav".to_owned()]);

    let mut b = world.device();
    world.sync(&mut b).await;
    assert!(b.vault.snapshot().contains_key("note.md"));
    assert!(!b.vault.snapshot().contains_key("rec/long.wav"));
}

/// 伪随机内容（FastCDC 才切得开）。
fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

/// 大文件（录音）走流式路径：推送不整份进内存、拉取逐块拼进暂存再换上——两台设备逐字节一致，
/// 换上之后块缓存清空。
#[tokio::test]
async fn a_large_recording_round_trips_through_the_streaming_paths() {
    let world = World::new();
    let mut a = world.device();
    let audio = noise(9 << 20, 11);
    a.vault.put("录音/assets/s-p01.wav", &audio);
    a.vault.put("录音/会议.md", "# 会议".as_bytes());
    let report = world.sync(&mut a).await;
    assert_eq!(report.pushed, 2);
    let (puts, _) = world.server.take_blob_traffic();
    assert!(puts > 3, "九兆的录音切成了多块：{puts}");

    let mut b = world.device();
    world.sync(&mut b).await;
    assert_eq!(
        b.vault.get("录音/assets/s-p01.wav").as_deref(),
        Some(audio.as_slice())
    );
    assert!(b.vault.cached_chunk_count() == 0, "落地之后块缓存应清空");
}

/// 下载到一半断网：已下好的块留在本机块缓存，下一轮只补缺的块（续传）。
#[tokio::test]
async fn an_interrupted_download_resumes_without_refetching_chunks() {
    let world = World::new();
    let mut a = world.device();
    let audio = noise(12 << 20, 12);
    a.vault.put("rec.wav", &audio);
    world.sync(&mut a).await;
    world.server.take_blob_traffic();

    let mut b = world.device();
    // 拉变更流一次 + 下几块，然后断网。
    world
        .server
        .fail_after(4, RemoteError::Network("断网".into()));
    assert!(world.try_sync(&mut b, false).await.is_err());
    let (_, first) = world.server.take_blob_traffic();
    assert!(first >= 1, "断网前下了几块");
    assert!(b.vault.get("rec.wav").is_none());

    world.server.heal();
    world.sync(&mut b).await;
    let (_, second) = world.server.take_blob_traffic();
    assert_eq!(b.vault.get("rec.wav").as_deref(), Some(audio.as_slice()));
    let chunks = luna_sync_core::chunk::boundaries(&audio).len();
    assert_eq!(
        first + second,
        chunks,
        "已下好的块不再下：{first} + {second} ≠ {chunks}"
    );
}

/// 上传到一半断网：下一轮问服务端缺哪些块，已传的不再传（续传）。
#[tokio::test]
async fn an_interrupted_upload_resumes_without_resending_chunks() {
    let world = World::new();
    let mut a = world.device();
    let audio = noise(12 << 20, 13);
    a.vault.put("rec.wav", &audio);
    // 拉变更流 + 问缺块 + 传几块，然后断网。
    world
        .server
        .fail_after(5, RemoteError::Network("断网".into()));
    assert!(world.try_sync(&mut a, false).await.is_err());
    let (first, _) = world.server.take_blob_traffic();
    assert!(first >= 1);

    world.server.heal();
    let report = world.sync(&mut a).await;
    assert_eq!(report.pushed, 1);
    let (second, _) = world.server.take_blob_traffic();
    let chunks = luna_sync_core::chunk::boundaries(&audio).len();
    assert_eq!(
        first + second,
        chunks,
        "已传的块不再传：{first} + {second} ≠ {chunks}"
    );
}

/// 只改尾部（录音续录）：只重传变了的块。
#[tokio::test]
async fn appending_to_a_recording_uploads_only_the_new_tail() {
    let world = World::new();
    let mut a = world.device();
    let mut audio = noise(12 << 20, 14);
    a.vault.put("rec.wav", &audio);
    world.sync(&mut a).await;
    let (first, _) = world.server.take_blob_traffic();

    audio.extend_from_slice(&noise(300 * 1024, 15));
    a.vault.put("rec.wav", &audio);
    let report = world.sync(&mut a).await;
    assert_eq!(report.pushed, 1);
    let (second, _) = world.server.take_blob_traffic();
    assert!(
        second <= 2,
        "续录只该多传尾部一两块，实际 {second}（首次 {first}）"
    );
}

/// 时间点恢复里的大文件（录音）逐块写盘：恢复出来逐字节一致。
#[tokio::test]
async fn a_point_in_time_restore_streams_large_recordings_to_disk() {
    use crate::restore::restore_to_folder;
    let world = World::new();
    let mut a = world.device();
    let audio = noise(9 << 20, 21);
    a.vault.put("录音/assets/s-p01.wav", &audio);
    a.vault.put("录音/会议.md", "# 会议".as_bytes());
    world.sync(&mut a).await;
    let at = world.server.head_seq();

    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("恢复");
    let report = restore_to_folder(
        world.server.as_ref(),
        &world.ring,
        world.server.vault_id,
        at,
        &dest,
        |_, _| {},
    )
    .await
    .unwrap();
    assert_eq!((report.files, report.skipped), (2, 0));
    assert_eq!(
        std::fs::read(dest.join("录音/assets/s-p01.wav")).unwrap(),
        audio
    );
}

/// 按需档（ADR-156）：别的设备录的音在本机只登记为仅云端——笔记照常到、录音不下载；之后几轮
/// 本机没有它不算删除，云端那份毫发无伤；云端改了只更新基线。
#[tokio::test]
async fn on_demand_devices_keep_recordings_in_the_cloud_without_deleting_them() {
    use crate::policy::RecordingsMode;
    let world = World::new();
    let mut a = world.device();
    a.vault.put("录音/assets/s.wav", &noise(3 << 20, 31));
    a.vault.put("录音/会议.md", "# 会议".as_bytes());
    world.sync(&mut a).await;

    world.recordings.set(RecordingsMode::OnDemand);
    let mut b = world.device();
    world.sync(&mut b).await;
    assert!(b.vault.get("录音/会议.md").is_some(), "笔记照常同步");
    assert!(b.vault.get("录音/assets/s.wav").is_none(), "录音不下载");
    let base = b
        .state
        .bases()
        .unwrap()
        .into_iter()
        .find(|f| f.path == "录音/assets/s.wav")
        .unwrap();
    assert!(base.cloud_only);
    for _ in 0..2 {
        let report = world.sync(&mut b).await;
        assert_eq!(
            (report.pushed, report.deleted_remote),
            (0, 0),
            "本机没有不是删除"
        );
    }
    assert_eq!(world.server.live_files(), 2);

    world.recordings.set(RecordingsMode::Full);
    a.vault.put("录音/assets/s.wav", &noise(3 << 20, 32));
    world.sync(&mut a).await;
    world.recordings.set(RecordingsMode::OnDemand);
    world.sync(&mut b).await;
    assert!(b.vault.get("录音/assets/s.wav").is_none());
    let updated = b.state.base(base.file_id).unwrap().unwrap();
    assert!(
        updated.cloud_only && updated.rev > base.rev,
        "云端改了只更新基线"
    );
}

/// 打开时按需下载；之后它就是本机文件，云端再改照常下；释放本机空间后变回仅云端、云端不受影响。
#[tokio::test]
async fn a_cloud_only_recording_downloads_on_demand_and_can_be_evicted_again() {
    use crate::engine::OnDemand;
    use crate::policy::RecordingsMode;
    let world = World::new();
    let mut a = world.device();
    let first = noise(3 << 20, 33);
    a.vault.put("rec.wav", &first);
    world.sync(&mut a).await;

    world.recordings.set(RecordingsMode::OnDemand);
    let mut b = world.device();
    world.sync(&mut b).await;
    assert!(b.vault.get("rec.wav").is_none());

    let outcome = world
        .engine(&b.vault, false)
        .materialize(&mut b.state, "rec.wav", &mut |_, _| {})
        .await
        .unwrap();
    assert_eq!(outcome, OnDemand::Done);
    assert_eq!(b.vault.get("rec.wav").as_deref(), Some(first.as_slice()));
    assert_eq!(
        world
            .engine(&b.vault, false)
            .materialize(&mut b.state, "rec.wav", &mut |_, _| {})
            .await
            .unwrap(),
        OnDemand::AlreadyThere
    );

    // 下到本机之后，云端再改照常下（按需只管「还没下过的」）。
    world.recordings.set(RecordingsMode::Full);
    let second = noise(3 << 20, 34);
    a.vault.put("rec.wav", &second);
    world.sync(&mut a).await;
    world.recordings.set(RecordingsMode::OnDemand);
    world.sync(&mut b).await;
    assert_eq!(b.vault.get("rec.wav").as_deref(), Some(second.as_slice()));

    // 释放本机空间：本机副本没了、不进回收站，基线仅云端，云端那份还在。
    let outcome = world
        .engine(&b.vault, false)
        .evict(&mut b.state, "rec.wav")
        .unwrap();
    assert_eq!(outcome, OnDemand::Done);
    assert!(b.vault.get("rec.wav").is_none());
    assert!(b.vault.trash().is_empty(), "释放空间不进回收站");
    let report = world.sync(&mut b).await;
    assert_eq!(report.deleted_remote, 0);
    assert_eq!(world.server.live_files(), 1);
}

/// 本机改过、还没同步的录音不能释放（那一版云端没有）。
#[tokio::test]
async fn an_unsynced_edit_is_never_evicted() {
    use crate::engine::OnDemand;
    let world = World::new();
    let mut a = world.device();
    a.vault.put("rec.wav", &noise(2 << 20, 35));
    world.sync(&mut a).await;
    a.vault.put("rec.wav", &noise(2 << 20, 36));
    let outcome = world
        .engine(&a.vault, false)
        .evict(&mut a.state, "rec.wav")
        .unwrap();
    assert_eq!(outcome, OnDemand::Changed);
    assert!(a.vault.get("rec.wav").is_some());
}

/// 云端删掉一个仅云端的录音：本机只记墓碑。仅云端的文件在本机又出现了同一份：转成本机有副本。
#[tokio::test]
async fn cloud_only_files_follow_remote_deletes_and_local_copies() {
    use crate::policy::RecordingsMode;
    let world = World::new();
    let mut a = world.device();
    let gone = noise(2 << 20, 37);
    let kept = noise(2 << 20, 38);
    a.vault.put("gone.wav", &gone);
    a.vault.put("kept.wav", &kept);
    world.sync(&mut a).await;

    world.recordings.set(RecordingsMode::OnDemand);
    let mut b = world.device();
    world.sync(&mut b).await;

    world.recordings.set(RecordingsMode::Full);
    a.vault.remove("gone.wav");
    world.sync(&mut a).await;
    world.recordings.set(RecordingsMode::OnDemand);
    world.sync(&mut b).await;
    let bases = b.state.bases().unwrap();
    assert!(bases.iter().any(|f| f.path == "gone.wav" && f.deleted));

    b.vault.put("kept.wav", &kept);
    world.sync(&mut b).await;
    let kept_base = b
        .state
        .bases()
        .unwrap()
        .into_iter()
        .find(|f| f.path == "kept.wav")
        .unwrap();
    assert!(!kept_base.cloud_only, "本机有了同一份");
}

/// 不同步录音：本机新录的不上传；别的设备的只登记为仅云端。
#[tokio::test]
async fn recordings_off_keeps_new_local_recordings_on_this_device() {
    use crate::policy::RecordingsMode;
    let world = World::new();
    world.recordings.set(RecordingsMode::Off);
    let mut b = world.device();
    b.vault.put("mine.wav", &noise(2 << 20, 39));
    b.vault.put("note.md", b"hello");
    let report = world.sync(&mut b).await;
    assert_eq!(report.pushed, 1, "只推了笔记");
    assert_eq!(world.server.live_files(), 1);
}

/// 全部下载档：此前按需登记的仅云端文件下到本机。
#[tokio::test]
async fn switching_to_full_downloads_cloud_only_files() {
    use crate::policy::RecordingsMode;
    let world = World::new();
    let mut a = world.device();
    let audio = noise(2 << 20, 40);
    a.vault.put("rec.wav", &audio);
    world.sync(&mut a).await;

    world.recordings.set(RecordingsMode::OnDemand);
    let mut b = world.device();
    world.sync(&mut b).await;
    assert!(b.vault.get("rec.wav").is_none());
    // 基线没变时云端不会再发这个文件的变化：全量对账才看得到——改一下云端那份再看。
    world.recordings.set(RecordingsMode::Full);
    let audio2 = noise(2 << 20, 41);
    a.vault.put("rec.wav", &audio2);
    world.sync(&mut a).await;
    world.sync(&mut b).await;
    assert_eq!(b.vault.get("rec.wav").as_deref(), Some(audio2.as_slice()));
}

/// 按流量计费网络 / 电量低：大文件这一轮不传，笔记照常；条件消失后补上。
#[tokio::test]
async fn large_transfers_wait_while_the_network_is_metered() {
    let world = World::new();
    let mut a = world.device();
    a.vault.put("rec.wav", &noise(2 << 20, 42));
    a.vault.put("note.md", b"hello");
    world.defer_large.set(true);
    let report = world.sync(&mut a).await;
    assert_eq!(report.pushed, 1, "只推了笔记");
    assert!(report.deferred >= 1);
    world.defer_large.set(false);
    let report = world.sync(&mut a).await;
    assert_eq!(report.pushed, 1, "条件消失后补推录音");
}

/// 单文件上限（ADR-156 ④）：超了的文件不切块不上传、列进「太大」，其余照常。
#[tokio::test]
async fn a_file_over_the_account_cap_is_skipped_before_uploading() {
    let world = World::new();
    world.max_file_bytes.set(Some(2 << 20));
    let mut a = world.device();
    a.vault.put("note.md", b"hello");
    a.vault.put("rec.wav", &noise(3 << 20, 51));
    let report = world.sync(&mut a).await;
    assert_eq!(report.pushed, 1, "只推了笔记");
    assert_eq!(report.too_large, vec!["rec.wav".to_owned()]);
    let (puts, _) = world.server.take_blob_traffic();
    assert_eq!(puts, 1, "超限的录音一块都没传");
}

/// 账户被冻结（纵切 3 C3）：上传暂停、下载照常，这一轮不算失败；本地改动不丢，解冻后的下一轮
/// 自己推上去。合并出来、还没推上去的那一份在解冻后照样收敛，不生成冲突副本——游标没前进，
/// 下一轮重拉同一段云端改动，认得出它的来历。
#[tokio::test]
async fn a_frozen_account_pauses_uploads_keeps_downloads_and_recovers_on_unfreeze() {
    let world = World::new();
    let (mut a, mut b) = (world.device(), world.device());
    a.vault.put("a.md", b"line 1\nline 2\nline 3\n");
    world.sync(&mut a).await;
    world.sync(&mut b).await;

    // 冻结前 B 推了两处改动；冻结后 A 才改。
    b.vault.put("a.md", b"line 1\nline 2\nline 3 (B)\n");
    b.vault.put("b.md", b"# B\n");
    world.sync(&mut b).await;
    world.server.set_frozen(true);
    a.vault.put("a.md", b"line 1 (A)\nline 2\nline 3\n");
    a.vault.put("c.md", b"# C\n");

    let report = world.sync(&mut a).await;
    assert!(report.account_frozen, "上传被冻结拒绝，要如实报出来");
    assert_eq!(report.pushed, 0);
    assert_eq!(text(&a, "b.md").as_deref(), Some("# B\n"), "下载照常落地");
    let merged = "line 1 (A)\nline 2\nline 3 (B)\n";
    assert_eq!(
        text(&a, "a.md").as_deref(),
        Some(merged),
        "合并结果留在本地"
    );
    assert_eq!(world.server.live_files(), 2, "冻结时一个字节都没写进去");

    // 还冻着：下一轮照样暂停，什么都不丢、也不生成冲突副本。
    let report = world.sync(&mut a).await;
    assert!(report.account_frozen);
    assert_eq!(report.conflict_copies, 0);

    world.server.set_frozen(false);
    let report = world.sync(&mut a).await;
    assert!(!report.account_frozen);
    assert_eq!(report.conflict_copies, 0);
    world.sync(&mut b).await;
    assert_eq!(text(&b, "a.md").as_deref(), Some(merged));
    assert_eq!(text(&b, "c.md").as_deref(), Some("# C\n"));
    assert_eq!(b.vault.snapshot(), a.vault.snapshot());
}

/// 冻结只改上传那一步的处置：别的写入失败照旧整轮中止（额度用尽不能被当成「暂停」吞掉）。
#[tokio::test]
async fn only_the_frozen_refusal_is_absorbed_other_write_failures_still_abort() {
    let world = World::new();
    let mut a = world.device();
    a.vault.put("a.md", b"x");
    world.server.set_full(true);
    assert!(matches!(
        world.try_sync(&mut a, false).await,
        Err(SyncAbort::Remote(RemoteError::QuotaExceeded))
    ));
}

#[tokio::test]
async fn notes_land_before_a_full_account_stops_the_attachments() {
    let world = World::new();
    let mut a = world.device();
    a.vault.put("note.md", b"note");
    a.vault.put("big.png", b"attachment");
    // 拉清单、笔记那一波的问缺块、传块、提交，共四次调用；第五次（附件的问缺块）起账户满了。
    world.server.fail_after(4, RemoteError::QuotaExceeded);
    assert!(matches!(
        world.try_sync(&mut a, false).await,
        Err(SyncAbort::Remote(RemoteError::QuotaExceeded))
    ));
    assert_eq!(world.server.live_files(), 1, "笔记已经提交，附件停在满额上");
    world.server.heal();
    world.sync(&mut a).await;
    assert_eq!(world.server.live_files(), 2, "腾出空间后接着传");
}

#[test]
fn upload_waves_put_notes_first_then_grow_by_size() {
    use crate::engine::upload_waves;
    let waves = upload_waves(vec![
        (0, false, 5),
        (1, true, 9),
        (2, false, 1),
        (3, true, 1),
    ]);
    assert_eq!(waves, vec![vec![3, 1], vec![2, 0]]);
    let big = 20 * 1024 * 1024;
    let waves = upload_waves(vec![(0, false, big), (1, false, big)]);
    assert_eq!(waves, vec![vec![0], vec![1]]);
}
