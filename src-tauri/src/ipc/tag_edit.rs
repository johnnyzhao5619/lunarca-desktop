//! 标签改名 / 合并命令域（ADR-135 / PRD-16-051，IPC 层）。
//!
//! 判定与写入全在 `luna_vault::tag_edit`：前端只发「从哪个标签改成哪个」、把预览原样带回确认、
//! 把回执原样带回撤销。写盘后逐篇广播，打开着的编辑器增量跟进、各视图按既有链路重取。

use super::*;

/// 这一篇为什么没改。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TagEditSkipDto {
    UnsupportedForm,
    InlineComment,
    MultiLine,
    InvalidMetadata,
}

impl From<luna_vault::tag_edit::TagEditSkip> for TagEditSkipDto {
    fn from(skip: luna_vault::tag_edit::TagEditSkip) -> Self {
        use luna_vault::tag_edit::TagEditSkip as S;
        match skip {
            S::UnsupportedForm => Self::UnsupportedForm,
            S::InlineComment => Self::InlineComment,
            S::MultiLine => Self::MultiLine,
            S::InvalidMetadata => Self::InvalidMetadata,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub(crate) struct TagEditTargetDto {
    pub path: String,
    pub before_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub(crate) struct TagEditSkippedDto {
    pub path: String,
    pub reason: TagEditSkipDto,
}

/// 改名影响预览：要改的笔记（确认时原样带回）、是否其实是合并、跳过的笔记与原因。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "snake_case", tag = "status")]
pub(crate) enum TagRenamePreviewDto {
    Ready {
        from: String,
        to: String,
        merge: bool,
        targets: Vec<TagEditTargetDto>,
        skipped: Vec<TagEditSkippedDto>,
    },
    /// 名字不合法（空、含空格或不允许的字符、首尾 `/`）或与原名相同。
    InvalidName,
    SameTag,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub(crate) struct TagEditChangeDto {
    pub path: String,
    pub before_hash: String,
    pub after_hash: String,
}

/// 逐篇回执；`changed` 原样带回即可整批撤销。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct TagEditReceiptDto {
    pub changed: Vec<TagEditChangeDto>,
    pub stale: Vec<String>,
    pub skipped: Vec<TagEditSkippedDto>,
    pub failed: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct TagUndoReceiptDto {
    pub restored: Vec<String>,
    pub kept: Vec<String>,
}

fn skipped_dto(
    skipped: Vec<(String, luna_vault::tag_edit::TagEditSkip)>,
) -> Vec<TagEditSkippedDto> {
    skipped
        .into_iter()
        .map(|(path, reason)| TagEditSkippedDto {
            path,
            reason: reason.into(),
        })
        .collect()
}

/// 写完之后逐篇广播：打开着的编辑器增量跟进（`note://written`），视图重取（`vault://changed`）。
fn announce_written(app: &tauri::AppHandle, written: Vec<(String, String)>) {
    for (path, content) in written {
        let _ = (NoteWritten {
            path: path.clone(),
            content,
            origin: "main".to_owned(),
        })
        .emit(app);
        emit_vault_changed(app, &path, false);
    }
}

fn read_all(vault: &luna_vault::Vault, paths: &[String]) -> Vec<(String, String)> {
    paths
        .iter()
        .filter_map(|path| vault.read(path).ok().map(|doc| (path.clone(), doc.content)))
        .collect()
}

/// 预览把一个标签（及其子标签）改名为另一个（ADR-135）。只读。
#[tauri::command]
#[specta::specta]
pub(crate) fn preview_tag_rename(
    state: tauri::State<'_, AppState>,
    from: String,
    to: String,
) -> Result<TagRenamePreviewDto, String> {
    use luna_vault::tag_edit::TagEditError;
    let guard = state.vault.lock().map_err(|_| state.vault_not_open())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    match vault.preview_tag_rename(&from, &to) {
        Ok(preview) => Ok(TagRenamePreviewDto::Ready {
            from: from.trim().to_owned(),
            to: to.trim().to_owned(),
            merge: preview.merge,
            targets: preview
                .targets
                .into_iter()
                .map(|target| TagEditTargetDto {
                    path: target.path,
                    before_hash: target.before_hash,
                })
                .collect(),
            skipped: skipped_dto(preview.skipped),
        }),
        Err(TagEditError::InvalidName) => Ok(TagRenamePreviewDto::InvalidName),
        Err(TagEditError::SameTag) => Ok(TagRenamePreviewDto::SameTag),
        Err(TagEditError::Vault(error)) => Err(error),
    }
}

/// 按预览逐篇改名；每篇仍是预览那一版才写，逐篇回执。
#[tauri::command]
#[specta::specta]
pub(crate) fn apply_tag_rename(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    from: String,
    to: String,
    targets: Vec<TagEditTargetDto>,
) -> Result<TagEditReceiptDto, String> {
    let targets: Vec<luna_vault::tag_edit::TagEditTarget> = targets
        .into_iter()
        .map(|target| luna_vault::tag_edit::TagEditTarget {
            path: target.path,
            before_hash: target.before_hash,
        })
        .collect();
    let (receipt, written) = {
        let guard = state.vault.lock().map_err(|_| state.vault_not_open())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        let receipt =
            vault
                .apply_tag_rename(&from, &to, &targets)
                .map_err(|error| match error {
                    luna_vault::tag_edit::TagEditError::Vault(coded) => coded,
                    // 只有绕过预览才会走到：名字在预览时就被拦下了。
                    _ => coded_error(&app, "TAG_RENAME_INVALID", "tag-rename-invalid"),
                })?;
        let paths: Vec<String> = receipt.changed.iter().map(|c| c.path.clone()).collect();
        let written = read_all(vault, &paths);
        (receipt, written)
    };
    announce_written(&app, written);
    Ok(TagEditReceiptDto {
        changed: receipt
            .changed
            .into_iter()
            .map(|change| TagEditChangeDto {
                path: change.path,
                before_hash: change.before_hash,
                after_hash: change.after_hash,
            })
            .collect(),
        stale: receipt.stale,
        skipped: skipped_dto(receipt.skipped),
        failed: receipt.failed,
    })
}

/// 整批撤销：只恢复之后未再改动的文件。
#[tauri::command]
#[specta::specta]
pub(crate) fn undo_tag_rename(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    changed: Vec<TagEditChangeDto>,
) -> Result<TagUndoReceiptDto, String> {
    let changes: Vec<luna_vault::tag_edit::TagEditChange> = changed
        .into_iter()
        .map(|change| luna_vault::tag_edit::TagEditChange {
            path: change.path,
            before_hash: change.before_hash,
            after_hash: change.after_hash,
        })
        .collect();
    let (receipt, written) = {
        let guard = state.vault.lock().map_err(|_| state.vault_not_open())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        let receipt = vault.undo_tag_edit(&changes);
        let written = read_all(vault, &receipt.restored);
        (receipt, written)
    };
    announce_written(&app, written);
    Ok(TagUndoReceiptDto {
        restored: receipt.restored,
        kept: receipt.kept,
    })
}

/// 单篇就地增删一个用户标签的结果（ADR-135）。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "snake_case", tag = "status")]
pub(crate) enum NoteTagEditDto {
    Written,
    /// 本来就有（加）/ 本来就没有（删）。
    Unchanged,
    /// 要删的标签写在正文里（`#标签`）：这里不改正文，请在正文里删。
    InlineOnly,
    /// 要删的是 AI 关系标签：到「知识库 → 标签」全库忽略。
    AiOnly,
    Skipped {
        reason: TagEditSkipDto,
    },
    InvalidName,
}

/// 给当前笔记加（`add = true`）或删一个用户标签。与编辑器保存走同一条写后尾段
/// （`broadcast_note_write`：打开着的编辑器跟进、标签与候选刷新）。
#[tauri::command]
#[specta::specta]
pub(crate) fn edit_note_tag(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    path: String,
    tag: String,
    add: bool,
) -> Result<NoteTagEditDto, String> {
    use luna_vault::tag_edit::{NoteTagEdit, TagEditError};
    let outcome = state.with_vault(window.label(), |vault| {
        match vault.edit_note_tag(&path, &tag, add) {
            Ok(outcome) => Ok(Ok(outcome)),
            Err(TagEditError::Vault(coded)) => Err(coded),
            Err(_) => Ok(Err(())),
        }
    })?;
    Ok(match outcome {
        Err(()) => NoteTagEditDto::InvalidName,
        Ok(NoteTagEdit::Written(content)) => {
            broadcast_note_write(&app, window.label(), &path, content);
            NoteTagEditDto::Written
        }
        Ok(NoteTagEdit::Unchanged) => NoteTagEditDto::Unchanged,
        Ok(NoteTagEdit::InlineOnly) => NoteTagEditDto::InlineOnly,
        Ok(NoteTagEdit::AiOnly) => NoteTagEditDto::AiOnly,
        Ok(NoteTagEdit::Skipped(reason)) => NoteTagEditDto::Skipped {
            reason: reason.into(),
        },
    })
}
