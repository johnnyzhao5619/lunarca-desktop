//! 改名 / 移动的回执与撤销（PRD-06-030 ③、TDD-06 §3 改名重构 ⑤⑥）。
//!
//! 回执随改名 / 移动命令一起返回；撤销凭据**不存后端**，原样交给前端保管、撤销时再交回来
//! （同 ADR-135 标签撤销）——Rust 逐篇核对指纹，之后有任何新写入就整次拒绝。

use super::*;

/// 随笔记一起搬走的独占附件。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub(crate) struct RelocatedAttachmentDto {
    pub from: String,
    pub to: String,
}

/// 被改写的一篇笔记：新路径与改写前 / 改完时的内容指纹。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub(crate) struct RelocatedNoteDto {
    pub path: String,
    pub before_hash: String,
    pub after_hash: String,
}

/// 撤销一次改名 / 移动所需的全部事实。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub(crate) struct RelocationUndoDto {
    pub from: String,
    pub to: String,
    pub attachments: Vec<RelocatedAttachmentDto>,
    pub notes: Vec<RelocatedNoteDto>,
}

/// 一次（或一批）改名 / 移动的回执：「已更新 M 篇笔记中的 N 处链接」与撤销凭据。
///
/// 一批移动给**一份**回执、多份凭据：撤销要整批倒序做完，界面也只说一次。
#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
pub(crate) struct RelocationReceiptDto {
    /// 改写了几处链接。
    pub links: u32,
    /// 改写了几篇笔记（含只改了 frontmatter 引用的）。
    pub notes: u32,
    pub undo: Vec<RelocationUndoDto>,
}

impl RelocationReceiptDto {
    pub(crate) fn absorb(&mut self, receipt: luna_vault::RelocationReceipt) {
        self.links += u32::try_from(receipt.links).unwrap_or(u32::MAX);
        self.notes += u32::try_from(receipt.rewritten.len()).unwrap_or(u32::MAX);
        self.undo.push(receipt.undo.into());
    }
}

impl From<luna_vault::RelocationUndo> for RelocationUndoDto {
    fn from(value: luna_vault::RelocationUndo) -> Self {
        Self {
            from: value.from,
            to: value.to,
            attachments: value
                .attachments
                .into_iter()
                .map(|(from, to)| RelocatedAttachmentDto { from, to })
                .collect(),
            notes: value
                .notes
                .into_iter()
                .map(|note| RelocatedNoteDto {
                    path: note.path,
                    before_hash: note.before_hash,
                    after_hash: note.after_hash,
                })
                .collect(),
        }
    }
}

impl From<RelocationUndoDto> for luna_vault::RelocationUndo {
    fn from(value: RelocationUndoDto) -> Self {
        Self {
            from: value.from,
            to: value.to,
            attachments: value
                .attachments
                .into_iter()
                .map(|item| (item.from, item.to))
                .collect(),
            notes: value
                .notes
                .into_iter()
                .map(|note| luna_vault::RelocationNoteChange {
                    path: note.path,
                    before_hash: note.before_hash,
                    after_hash: note.after_hash,
                })
                .collect(),
        }
    }
}

/// 撤销的结果：每一项改名的新旧路径（界面据此把标签页、选中项映射回去）与还原了的笔记。
#[derive(Debug, Clone, Default, Serialize, Type)]
pub(crate) struct RelocationUndoneDto {
    pub moved_back: Vec<MovedPathDto>,
    pub restored: Vec<String>,
}

/// 撤销改名 / 移动（PRD-06-030 ③）：凭据按发生的倒序逐项撤。任一项之后被改过就停在那里
/// 报 `VAULT_RELOCATION_STALE`——已撤的不再重做（它们本身是完整的一次撤销）。
#[tauri::command]
#[specta::specta]
pub(crate) fn vault_undo_relocation(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    undo: Vec<RelocationUndoDto>,
) -> Result<RelocationUndoneDto, String> {
    let change_scope = VaultChangeScope::for_window(window.label());
    let mut done = RelocationUndoneDto::default();
    let mut changes: Vec<(String, bool)> = Vec::new();
    let root = state.with_vault(window.label(), |vault| {
        let root = vault.root().to_path_buf();
        for item in undo.into_iter().rev() {
            let (from, to) = (item.from.clone(), item.to.clone());
            let restored = vault
                .undo_relocation(&item.into())
                .map_err(super::notes::vault_err)?;
            changes.push((to.clone(), true));
            changes.push((from.clone(), false));
            done.moved_back.push(MovedPathDto { from: to, to: from });
            done.restored.extend(restored);
        }
        Ok(root)
    });
    let root = match root {
        Ok(root) => root,
        Err(error) => {
            // 撤到一半停下：已撤的照常广播，界面才跟得上磁盘。
            emit_vault_changes_in_scope(&app, changes, change_scope);
            return Err(error);
        }
    };
    emit_vault_changes_in_scope(&app, changes, change_scope);
    broadcast_relinked(&app, window.label(), &root, &done.restored);
    Ok(done)
}

/// 改名重构改写了别的笔记（TDD-06 §3 改名重构 ⑥）：那些写入走自写抑制，监听器不会替它们
/// 发 `vault://changed`，打开着它们的编辑器只能靠这条广播跟上新正文——否则标签页停在旧
/// 链接上，下次保存撞基线冲突。只发正文，不逐篇唤起派生刷新：整批的派生刷新已随
/// `emit_vault_changes` 做过一次。
pub(crate) fn broadcast_relinked(
    app: &tauri::AppHandle,
    window_label: &str,
    root: &Path,
    paths: &[String],
) {
    let origin = if VaultChangeScope::for_window(window_label).affects_active_vault() {
        "main".to_owned()
    } else {
        window_label.to_owned()
    };
    for path in paths {
        if let Ok(content) = std::fs::read_to_string(root.join(path)) {
            let _ = (NoteWritten {
                path: path.clone(),
                content,
                origin: origin.clone(),
            })
            .emit(app);
        }
    }
}
