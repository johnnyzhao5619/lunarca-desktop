//! 三态决策（ADR-148；方案 §4.6 决策表）：每个文件的 基线 / 本地 / 云端 → 一个动作。
//!
//! 纯函数、没有 I/O：引擎把观察到的三态喂进来，拿回动作去执行。确定性模拟器（S3e）与决策表
//! 用例都直接打在这一层上。
//!
//! | 本地 \ 云端 | 未变 | 改了 | 删了 |
//! |---|---|---|---|
//! | 未变 | 无 | 应用云端 | 本地删除（进回收站） |
//! | 改了 | 推送 | `.md` 三路合并 / 其余冲突副本 | 修改优先：作为新版本推回 |
//! | 删了 | 推送删除 | 修改优先：把云端写回 | 只推进基线 |
//! | 说不准 | 无 | 等下一轮 | 等下一轮 |
//!
//! 「说不准」来自 `luna_vault::absence`：卷没挂、身份对不上、上层目录读不了、iCloud 占位——
//! 这些情况下既不产生删除，也不往那个位置写。

use uuid::Uuid;

use crate::manifest::EntryPlain;
use crate::state::BaseFile;

/// 本地观察到的一个文件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalFile {
    /// 库相对路径（NFC）。
    pub path: String,
    pub content_hash: String,
    pub size: u64,
    pub mtime_ms: Option<i64>,
    /// 还在变的大文件（正在录的 WAV、正在拷进来的大文件）：扫描时不读不哈希（`content_hash`
    /// 为空），引擎这一轮不碰它，等它稳定下来再同步。
    pub settling: bool,
}

/// 本地这一侧。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalSide {
    Present(LocalFile),
    /// 基线里有、此刻观察到确实被删了。
    Deleted,
    /// 基线里有、此刻不在，但说不准为什么：不得据此删除。
    Unknown,
    /// 本地从来没有（云端新来的文件）。
    Absent,
}

/// 云端这一侧（这一轮拉到的最新头；没拉到改动 = `None`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteSide {
    /// 服务端当前 rev；`None` = 服务端已没有这个文件的任何记录（墓碑过了保留期被裁掉）。
    pub rev: Option<i64>,
    pub deleted: bool,
    /// 活文件的条目（已解密）；墓碑为 `None`。
    pub entry: Option<EntryPlain>,
}

/// 一个文件的三态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileView {
    pub file_id: Uuid,
    pub base: Option<BaseFile>,
    pub local: LocalSide,
    pub remote: Option<RemoteSide>,
}

/// 引擎要做的事。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Nothing,
    /// 推送本地内容。`base_rev` 是提交时给服务端比对的那个数。
    Push {
        base_rev: Option<i64>,
    },
    /// 推送删除。
    PushDelete {
        base_rev: i64,
    },
    /// 本地内容作为一个新文件推上去（新的 file_id），旧的 file_id 从基线里移除。
    PushAsNew,
    /// 把云端的版本写到本地（新建、覆盖或挪位置）。
    Apply,
    /// 云端删了：本地删（进回收站）。
    ApplyDelete,
    /// 两边都改了 `.md`：三路合并（基线 = 钉住的内容）；合不上就冲突副本。
    Merge,
    /// 两边都改了、不能合并：云端版本另存为冲突副本，本地版本推上去。
    ConflictCopy,
    /// 两边结果一样（都删了、或改成了同样的内容）：只推进基线。
    AdvanceBase,
    /// 这一轮不动（本地说不准）。
    Defer,
}

/// 决策附带的说明：修改优先的两种情形要进活动记录。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Note {
    None,
    /// 本地删了、云端改了：撤销本地删除。
    RestoredRemoteEdit,
    /// 本地改了、云端删了：作为新版本推回。
    ResurrectedLocalEdit,
}

/// 本地相对基线的变化。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
    Unchanged,
    Modified,
    Deleted,
    Uncertain,
}

/// 云端相对基线的变化（云端没有「说不准」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteChange {
    Unchanged,
    Modified,
    Deleted,
}

fn base_live(base: Option<&BaseFile>) -> bool {
    base.is_some_and(|b| !b.deleted)
}

fn local_change(view: &FileView) -> Change {
    let base = view.base.as_ref();
    match &view.local {
        LocalSide::Present(file) => match base {
            Some(b) if !b.deleted && b.content_hash == file.content_hash && b.path == file.path => {
                Change::Unchanged
            }
            _ => Change::Modified,
        },
        LocalSide::Deleted if base_live(base) => Change::Deleted,
        LocalSide::Deleted | LocalSide::Absent => Change::Unchanged,
        LocalSide::Unknown => Change::Uncertain,
    }
}

fn remote_change(view: &FileView) -> RemoteChange {
    let Some(remote) = &view.remote else {
        return RemoteChange::Unchanged;
    };
    let base = view.base.as_ref();
    if remote.deleted {
        return if base_live(base) {
            RemoteChange::Deleted
        } else {
            RemoteChange::Unchanged
        };
    }
    match (base, remote.rev) {
        (Some(b), Some(rev)) if !b.deleted && b.rev == rev => RemoteChange::Unchanged,
        _ => RemoteChange::Modified,
    }
}

/// 提交时给服务端比对的基线版本：云端这一轮报了当前 rev 就用它（只在决定「覆盖云端」的
/// 几种情形下才用到），否则用本地基线的 rev。服务端已没有记录时为 `None`。
fn push_base(view: &FileView) -> Option<i64> {
    match &view.remote {
        Some(remote) => remote.rev,
        None => view.base.as_ref().map(|b| b.rev),
    }
}

fn is_markdown(path: &str) -> bool {
    path.rsplit('.')
        .next()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
}

/// 一个文件这一轮做什么。
#[must_use]
pub fn decide(view: &FileView) -> (Action, Note) {
    use Change::{Deleted, Modified, Uncertain, Unchanged};
    let remote = match remote_change(view) {
        RemoteChange::Unchanged => Unchanged,
        RemoteChange::Modified => Modified,
        RemoteChange::Deleted => Deleted,
    };
    match (local_change(view), remote) {
        (Unchanged, Unchanged) => (Action::Nothing, Note::None),
        (Uncertain, Unchanged) => (Action::Nothing, Note::None),
        (Uncertain, _) => (Action::Defer, Note::None),
        (Modified, Unchanged) => (
            Action::Push {
                base_rev: view.base.as_ref().map(|b| b.rev),
            },
            Note::None,
        ),
        (Deleted, Unchanged) => match view.base.as_ref() {
            Some(b) => (Action::PushDelete { base_rev: b.rev }, Note::None),
            None => (Action::Nothing, Note::None),
        },
        (Unchanged, Modified) => (Action::Apply, Note::None),
        (Unchanged, Deleted) => (Action::ApplyDelete, Note::None),
        (Modified, Modified) => {
            let LocalSide::Present(local) = &view.local else {
                return (Action::Defer, Note::None);
            };
            let remote = view
                .remote
                .as_ref()
                .and_then(|r| r.entry.as_ref())
                .expect("a modified remote side carries its entry");
            if remote.content_hash == local.content_hash && remote.path == local.path {
                (Action::AdvanceBase, Note::None)
            } else if is_markdown(&local.path)
                && is_markdown(&remote.path)
                && view.base.as_ref().is_some_and(|b| b.pin.is_some())
            {
                (Action::Merge, Note::None)
            } else {
                (Action::ConflictCopy, Note::None)
            }
        }
        (Deleted, Modified) => (Action::Apply, Note::RestoredRemoteEdit),
        // 修改优先：作为新版本推回。云端墓碑的 rev 知道（增量拉到的）就基于它推，同一个
        // file_id；不知道（全量对账只列活文件，墓碑没列出来）就作为一个新文件推上去——拿一个
        // 猜的基线去提交只会被判过期，而游标已经越过了那个墓碑，下一轮也拿不到它。
        (Modified, Deleted) => match push_base(view) {
            Some(rev) => (
                Action::Push {
                    base_rev: Some(rev),
                },
                Note::ResurrectedLocalEdit,
            ),
            None => (Action::PushAsNew, Note::ResurrectedLocalEdit),
        },
        (Deleted, Deleted) => (Action::AdvanceBase, Note::None),
        (_, Uncertain) => unreachable!("the remote side is never uncertain"),
    }
}

/// 批量删除熔断（方案 §4.6 护栏）：一轮要删的数量超过 max(50, 基线活文件的 30%) 就暂停删除、
/// 请用户确认。
#[must_use]
pub fn deletions_need_confirmation(deletions: usize, live_base: usize) -> bool {
    deletions > 50.max(live_base * 3 / 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    const H1: &str = "1111111111111111111111111111111111111111111111111111111111111111";
    const H2: &str = "2222222222222222222222222222222222222222222222222222222222222222";
    const H3: &str = "3333333333333333333333333333333333333333333333333333333333333333";

    fn base(path: &str, rev: i64, hash: &str, deleted: bool) -> Option<BaseFile> {
        Some(BaseFile {
            file_id: Uuid::from_u128(1),
            path: path.into(),
            rev,
            content_hash: if deleted { String::new() } else { hash.into() },
            pin: (!deleted && path.ends_with(".md")).then(|| "pin".into()),
            deleted,
            cloud_only: false,
        })
    }

    fn local(path: &str, hash: &str) -> LocalSide {
        LocalSide::Present(LocalFile {
            path: path.into(),
            content_hash: hash.into(),
            size: 1,
            mtime_ms: None,
            settling: false,
        })
    }

    fn remote(path: &str, rev: i64, hash: &str) -> Option<RemoteSide> {
        Some(RemoteSide {
            rev: Some(rev),
            deleted: false,
            entry: Some(EntryPlain {
                v: 1,
                path: path.into(),
                size: 1,
                mtime_ms: None,
                content_hash: hash.into(),
                chunks: vec![H1.into()],
            }),
        })
    }

    fn tombstone(rev: Option<i64>) -> Option<RemoteSide> {
        Some(RemoteSide {
            rev,
            deleted: true,
            entry: None,
        })
    }

    fn view(base: Option<BaseFile>, local: LocalSide, remote: Option<RemoteSide>) -> FileView {
        FileView {
            file_id: Uuid::from_u128(1),
            base,
            local,
            remote,
        }
    }

    fn act(v: FileView) -> Action {
        decide(&v).0
    }

    #[test]
    fn the_decision_table_row_by_row() {
        let b = || base("a.md", 3, H1, false);
        // 未变 × 未变 / 云端报了同一个 rev。
        assert_eq!(act(view(b(), local("a.md", H1), None)), Action::Nothing);
        assert_eq!(
            act(view(b(), local("a.md", H1), remote("a.md", 3, H1))),
            Action::Nothing
        );
        // 改了 × 未变：推送，基线 rev 3。
        assert_eq!(
            act(view(b(), local("a.md", H2), None)),
            Action::Push { base_rev: Some(3) }
        );
        // 删了 × 未变。
        assert_eq!(
            act(view(b(), LocalSide::Deleted, None)),
            Action::PushDelete { base_rev: 3 }
        );
        // 未变 × 改了 / 删了。
        assert_eq!(
            act(view(b(), local("a.md", H1), remote("a.md", 4, H2))),
            Action::Apply
        );
        assert_eq!(
            act(view(b(), local("a.md", H1), tombstone(Some(4)))),
            Action::ApplyDelete
        );
        // 改了 × 改了：`.md` 有钉住的基线 → 合并；改成同一份 → 只推进基线。
        assert_eq!(
            act(view(b(), local("a.md", H2), remote("a.md", 4, H3))),
            Action::Merge
        );
        assert_eq!(
            act(view(b(), local("a.md", H2), remote("a.md", 4, H2))),
            Action::AdvanceBase
        );
        // 删了 × 删了。
        assert_eq!(
            act(view(b(), LocalSide::Deleted, tombstone(Some(4)))),
            Action::AdvanceBase
        );
    }

    #[test]
    fn modifications_win_over_deletions_both_ways() {
        let b = || base("a.md", 3, H1, false);
        assert_eq!(
            decide(&view(b(), LocalSide::Deleted, remote("a.md", 4, H2))),
            (Action::Apply, Note::RestoredRemoteEdit)
        );
        // 本地改了、云端删了：基于云端那个墓碑 rev 推回。
        assert_eq!(
            decide(&view(b(), local("a.md", H2), tombstone(Some(4)))),
            (
                Action::Push { base_rev: Some(4) },
                Note::ResurrectedLocalEdit
            )
        );
        // 墓碑的 rev 不知道（全量对账）：作为新文件推回，不拿猜的基线去提交。
        assert_eq!(
            decide(&view(b(), local("a.md", H2), tombstone(None))),
            (Action::PushAsNew, Note::ResurrectedLocalEdit)
        );
    }

    #[test]
    fn binaries_and_unpinned_notes_get_a_conflict_copy_instead_of_a_merge() {
        let bin = base("a.png", 3, H1, false);
        assert_eq!(
            act(view(bin, local("a.png", H2), remote("a.png", 4, H3))),
            Action::ConflictCopy
        );
        let mut unpinned = base("a.md", 3, H1, false);
        unpinned.as_mut().unwrap().pin = None;
        assert_eq!(
            act(view(unpinned, local("a.md", H2), remote("a.md", 4, H3))),
            Action::ConflictCopy
        );
        // 同一内容、不同路径（两边各自改名）：不是同一个结果，不能只推进基线。
        assert_ne!(
            act(view(
                base("a.md", 3, H1, false),
                local("b.md", H2),
                remote("c.md", 4, H2)
            )),
            Action::AdvanceBase
        );
    }

    #[test]
    fn an_uncertain_absence_never_deletes_and_never_writes() {
        let b = || base("a.md", 3, H1, false);
        assert_eq!(act(view(b(), LocalSide::Unknown, None)), Action::Nothing);
        assert_eq!(
            act(view(b(), LocalSide::Unknown, remote("a.md", 4, H2))),
            Action::Defer
        );
        assert_eq!(
            act(view(b(), LocalSide::Unknown, tombstone(Some(4)))),
            Action::Defer
        );
    }

    #[test]
    fn new_files_on_either_side() {
        // 本地新建：推送，服务端还没有这个文件。
        assert_eq!(
            act(view(None, local("n.md", H1), None)),
            Action::Push { base_rev: None }
        );
        // 云端新来的：写到本地。
        assert_eq!(
            act(view(None, LocalSide::Absent, remote("n.md", 1, H1))),
            Action::Apply
        );
        // 本地挪了位置（内容没变）：也是一次推送（条目里的路径变了）。
        assert_eq!(
            act(view(
                base("a.md", 3, H1, false),
                local("dir/a.md", H1),
                None
            )),
            Action::Push { base_rev: Some(3) }
        );
        // 基线是墓碑、本地又出现同一文件：基于墓碑推回。
        assert_eq!(
            act(view(base("a.md", 5, H1, true), local("a.md", H1), None)),
            Action::Push { base_rev: Some(5) }
        );
        // 云端墓碑落在本地从没有过的文件上：无事可做。
        assert_eq!(
            act(view(None, LocalSide::Absent, tombstone(Some(2)))),
            Action::Nothing
        );
    }

    #[test]
    fn the_bulk_delete_breaker_scales_with_the_vault() {
        assert!(!deletions_need_confirmation(50, 10));
        assert!(deletions_need_confirmation(51, 10));
        assert!(!deletions_need_confirmation(300, 1000));
        assert!(deletions_need_confirmation(301, 1000));
    }
}
