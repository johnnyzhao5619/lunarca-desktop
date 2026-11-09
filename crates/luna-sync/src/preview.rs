//! 合并预览（ADR-178 ④）：本地库加入一个云端库之前，算出这一步会做什么。用与同步轮同一份判定
//! （[`crate::core::decide`]），不写盘、不落绑定。

use std::collections::HashMap;

use luna_sync_core::manifest::EntryPlain;
use uuid::Uuid;

use crate::core::{decide, Action, FileView, LocalFile, LocalSide, RemoteSide};
use crate::state::path_key;

/// 预览的结果（文件数与将上传的字节）。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MergePreview {
    pub download: u64,
    pub upload: u64,
    pub upload_bytes: u64,
    pub same: u64,
    pub differ: u64,
}

/// `remote` 是云端清单里的活文件。
#[must_use]
pub fn merge_preview(local: Vec<LocalFile>, remote: Vec<EntryPlain>) -> MergePreview {
    let mut local: HashMap<String, LocalFile> = local
        .into_iter()
        .filter(|file| !file.settling)
        .map(|file| (path_key(&file.path), file))
        .collect();
    let mut out = MergePreview::default();
    for entry in remote {
        let side = local
            .remove(&path_key(&entry.path))
            .map_or(LocalSide::Absent, LocalSide::Present);
        let view = FileView {
            file_id: Uuid::nil(),
            base: None,
            local: side,
            remote: Some(RemoteSide {
                rev: Some(1),
                deleted: false,
                entry: Some(entry),
            }),
        };
        match decide(&view).0 {
            Action::Apply => out.download += 1,
            Action::AdvanceBase => out.same += 1,
            _ => out.differ += 1,
        }
    }
    for file in local.into_values() {
        out.upload += 1;
        out.upload_bytes += file.size;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(path: &str, hash: &str) -> LocalFile {
        LocalFile {
            path: path.into(),
            content_hash: hash.into(),
            size: 10,
            mtime_ms: None,
            settling: false,
        }
    }

    fn remote(path: &str, hash: &str) -> EntryPlain {
        EntryPlain {
            v: 1,
            path: path.into(),
            size: 10,
            mtime_ms: None,
            content_hash: hash.into(),
            chunks: Vec::new(),
        }
    }

    #[test]
    fn counts_each_side_with_the_same_rules_as_a_sync_round() {
        let preview = merge_preview(
            vec![
                local("same.md", "a"),
                local("differ.md", "b"),
                local("only-here.md", "c"),
            ],
            vec![
                remote("same.md", "a"),
                remote("differ.md", "z"),
                remote("only-there.md", "d"),
            ],
        );
        assert_eq!(
            preview,
            MergePreview {
                download: 1,
                upload: 1,
                upload_bytes: 10,
                same: 1,
                differ: 1,
            }
        );
    }

    #[test]
    fn a_file_still_being_written_is_left_out() {
        let mut settling = local("recording.wav", "");
        settling.settling = true;
        assert_eq!(
            merge_preview(vec![settling], Vec::new()),
            MergePreview::default()
        );
    }
}
