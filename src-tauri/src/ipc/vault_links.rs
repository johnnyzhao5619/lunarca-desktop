//! 指向库里内容的 `lunarca://` 链接（ADR-141，笔记库对标轮 W4a）：送达、暂存、交给界面。
//!
//! 解析与路径边界在 [`luna_vault::uri`]。这里做三件事：链接送达（运行中或冷启动由它拉起）
//! 时先存着并亮主窗；界面挂载或收到事件时来取（[`vault_link_pending`]）——此刻才按注册表认库、
//! 判断要不要切库、核对文件在不在、把 `#标题` 算成行号；界面做完或用户拒绝后
//! [`vault_link_done`] 清掉。**切库要用户确认**（ADR-141 ①），而切库会让界面整个重挂载，所以
//! 请求留在这里，切完重新取一次再执行。链接内容不写日志、不进事件载荷（ADR-141 ⑥）。

use super::*;
use luna_vault::uri::{VaultLink, VaultLinkAction, VaultLinkRejection};

/// 一条送达的链接：解析好的请求，或解析失败的理由（也要告诉用户，而不是什么都不发生）。
pub(crate) type PendingVaultLink = Result<VaultLink, VaultLinkRejection>;

/// 有一条链接送达（载荷为空：内容只经 [`vault_link_pending`] 取，不在事件总线上多走一遍）。
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[tauri_specta(event_name = "vault-link://received")]
pub struct VaultLinkReceived {}

/// 链接要做的事（已校验；`open` 的行号在当前库就是目标库时才算）。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VaultLinkActionDto {
    Open {
        path: String,
        /// `#标题` 落在第几行（1 起，含 frontmatter）；没带标题或找不到为 `null`。
        line: Option<u32>,
        heading_index: Option<u32>,
    },
    Search {
        query: String,
    },
    New {
        name: String,
    },
}

/// 链接为什么不能执行。
#[derive(Debug, Clone, Copy, Serialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VaultLinkRejectionDto {
    /// 不是合法的链接（缺参数、未知动作）。
    Invalid,
    /// 路径越出库或指向隐藏数据。
    BadPath,
    /// 这台设备上没有登记这个库。
    UnknownVault,
    /// 库里没有这篇。
    NotFound,
}

/// 界面此刻要处理的那条链接。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VaultLinkPendingDto {
    Request {
        vault_name: String,
        vault_path: String,
        /// 目标库不是当前主库：先问用户要不要切过去。
        needs_switch: bool,
        action: VaultLinkActionDto,
    },
    Rejected {
        reason: VaultLinkRejectionDto,
    },
}

/// 链接送达（入口按 host 分派过来，[`handle_incoming_deep_link`]）。
pub(crate) fn receive_vault_link(app: &tauri::AppHandle, url: &str) {
    use tauri::Manager as _;
    if let Some(state) = app.try_state::<AppState>() {
        state.stash_vault_link(luna_vault::uri::parse(url));
    }
    crate::show_main_window(app);
    let _ = VaultLinkReceived {}.emit(app);
}

/// 取此刻待处理的链接（不清除——要切库时界面会重挂载，切完再取一次；被拒的取一次即清）。
#[tauri::command]
#[specta::specta]
pub(crate) fn vault_link_pending(state: tauri::State<'_, AppState>) -> Option<VaultLinkPendingDto> {
    let pending = state.peek_vault_link()?;
    // 只认列表账户看得见的库：另一个账户的库对这个账户来说不存在（ADR-151 修订 2026-10-15「续二」）。
    let resolved = resolve(
        &state.known_vaults(),
        state.vault_root().as_deref(),
        pending,
    );
    if matches!(resolved, VaultLinkPendingDto::Rejected { .. }) {
        state.clear_vault_link();
    }
    Some(resolved)
}

/// 链接处理完了（执行了，或用户拒绝了切库 / 新建）。
#[tauri::command]
#[specta::specta]
pub(crate) fn vault_link_done(state: tauri::State<'_, AppState>) {
    state.clear_vault_link();
}

/// 这一篇的 `lunarca://open` 链接（右键「复制 Lunarca 链接」）。按窗口所在的库生成。
#[tauri::command]
#[specta::specta]
pub(crate) fn vault_note_link(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<String, String> {
    state.with_vault(window.label(), |vault| {
        Ok(luna_vault::uri::open_link(&vault.id().to_string(), &path))
    })
}

fn rejected(reason: VaultLinkRejectionDto) -> VaultLinkPendingDto {
    VaultLinkPendingDto::Rejected { reason }
}

/// 把送达的链接对上这台设备：认库、判断要不要切库、核对文件、算标题行号（可测的纯判定，
/// 只读文件系统）。
fn resolve(
    registry: &[luna_vault::registry::VaultEntry],
    current_root: Option<&std::path::Path>,
    pending: PendingVaultLink,
) -> VaultLinkPendingDto {
    let link = match pending {
        Ok(link) => link,
        Err(VaultLinkRejection::Invalid) => return rejected(VaultLinkRejectionDto::Invalid),
        Err(VaultLinkRejection::BadPath) => return rejected(VaultLinkRejectionDto::BadPath),
    };
    // 只认注册表里的库（ADR-141 ①）：`vault` 是 id，不是路径。
    let Some(entry) = registry
        .iter()
        .find(|entry| entry.id.as_deref() == Some(link.vault.as_str()))
    else {
        return rejected(VaultLinkRejectionDto::UnknownVault);
    };
    let needs_switch = current_root != Some(entry.path.as_path());
    let action = match link.action {
        VaultLinkAction::Open { path, heading } => {
            // 文件在不在、标题在哪一行，只在它就是当前库时核对——要切库的，切完再取时核对。
            let (line, heading_index) = if needs_switch {
                (None, None)
            } else {
                let Ok(abs) = super::shell::resolve_vault_entry(&entry.path, &path) else {
                    return rejected(VaultLinkRejectionDto::NotFound);
                };
                if !abs.is_file() {
                    return rejected(VaultLinkRejectionDto::NotFound);
                }
                match (heading, std::fs::read_to_string(&abs)) {
                    (Some(heading), Ok(content)) => {
                        luna_vault::parse::locate_fragment(&content, &heading)
                    }
                    _ => (None, None),
                }
            };
            VaultLinkActionDto::Open {
                path,
                line,
                heading_index,
            }
        }
        VaultLinkAction::Search { query } => VaultLinkActionDto::Search { query },
        VaultLinkAction::New { name } => VaultLinkActionDto::New { name },
    };
    VaultLinkPendingDto::Request {
        vault_name: entry.name.clone(),
        vault_path: entry.path.to_string_lossy().into_owned(),
        needs_switch,
        action,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luna_vault::registry::VaultEntry;

    const ID: &str = "01a10c9a-670b-72d1-a3cb-675742d38201";

    fn entry(path: &std::path::Path) -> VaultEntry {
        VaultEntry {
            path: path.to_path_buf(),
            name: "工作库".into(),
            last_opened_ms: 0,
            id: Some(ID.into()),
            accounts: Vec::new(),
        }
    }

    fn open(path: &str, heading: Option<&str>) -> PendingVaultLink {
        Ok(VaultLink {
            vault: ID.into(),
            action: VaultLinkAction::Open {
                path: path.into(),
                heading: heading.map(Into::into),
            },
        })
    }

    #[test]
    fn a_link_into_the_current_vault_resolves_the_heading_line() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("项目")).unwrap();
        std::fs::write(
            dir.path().join("项目/计划.md"),
            "---\na: 1\n---\n# 计划\n\n## 议程\n",
        )
        .unwrap();
        let registry = [entry(dir.path())];
        let resolved = resolve(
            &registry,
            Some(dir.path()),
            open("项目/计划.md", Some("议程")),
        );
        match resolved {
            VaultLinkPendingDto::Request {
                needs_switch,
                action:
                    VaultLinkActionDto::Open {
                        line,
                        heading_index,
                        ..
                    },
                ..
            } => {
                assert!(!needs_switch);
                assert_eq!(line, Some(6));
                assert_eq!(heading_index, Some(1));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn unknown_vaults_missing_notes_and_other_vaults() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let registry = [entry(dir.path())];
        assert!(matches!(
            resolve(&[], Some(dir.path()), open("a.md", None)),
            VaultLinkPendingDto::Rejected {
                reason: VaultLinkRejectionDto::UnknownVault
            }
        ));
        assert!(matches!(
            resolve(&registry, Some(dir.path()), open("没有.md", None)),
            VaultLinkPendingDto::Rejected {
                reason: VaultLinkRejectionDto::NotFound
            }
        ));
        // 目标库不是当前库：先问要不要切，文件切过去再核对。
        assert!(matches!(
            resolve(&registry, Some(other.path()), open("没有.md", None)),
            VaultLinkPendingDto::Request {
                needs_switch: true,
                ..
            }
        ));
        assert!(matches!(
            resolve(
                &registry,
                Some(dir.path()),
                Err(VaultLinkRejection::BadPath)
            ),
            VaultLinkPendingDto::Rejected {
                reason: VaultLinkRejectionDto::BadPath
            }
        ));
    }
}
