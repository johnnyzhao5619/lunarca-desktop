//! 文件监听（TDD-06 §4）。**真机生效**（依赖真实 FS 事件，headless 不可靠验，留真机清单）。
//!
//! 用 `notify-debouncer-full`：① 编辑器/原子写多用"写临时文件 + rename"，事件是 create/rename
//! 而非 modify，朴素监听会漏；② 部分云盘发幽灵/重复事件。去抖后按内容 `hash` 二次去重。
//!
//! **职责边界（解耦）**：本模块只负责"去抖 + 计算变更路径与当前内容指纹"，把 [`VaultChange`]
//! 交给回调；**自写抑制与冲突判定（[`crate::suppress::decide_external_edit`]）由持有 `Vault` 的
//! 上层（Tauri app）执行**——因 `rusqlite::Connection` 非 `Sync`，`Vault` 经 `Arc<Mutex<…>>` 在主
//! 侧消费，监听线程不直接持有它。这样监听层无状态、可独立替换。

use crate::error::{Result, VaultError};
use crate::hashing::ContentHash;
use notify_debouncer_full::notify::{RecursiveMode, Watcher as _};
use notify_debouncer_full::{new_debouncer, DebounceEventResult};
use std::path::Path;
use std::time::Duration;

/// 一次去抖后的 Vault 变更。
#[derive(Clone, Debug, PartialEq)]
pub struct VaultChange {
    /// 相对路径（forward-slash）。
    pub rel_path: String,
    /// 当前内容指纹；文件已删除/不可读则 `None`（上层据此判删除 vs 修改）。
    pub disk_hash: Option<ContentHash>,
    /// 同一个原生 rename 事件里的旧相对路径；普通创建/修改/删除为 `None`。
    pub moved_from: Option<String>,
}

/// 一次去抖后的**附件**变更（非 `.md` 文件，PRD-06-012 ③）：只告诉上层「这个文件有了 / 没了」，
/// 不读内容、不算指纹——附件可能是几百兆的视频，而上层要的只是让正文里的引用重新判断指不
/// 指得到。附件不进全文索引。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetChange {
    pub rel_path: String,
    pub deleted: bool,
}

/// 默认去抖窗口（与波形/UI 抖动权衡，可调）。
const DEBOUNCE_MS: u64 = 400;

/// 监听句柄；drop 即停止监听（RAII）。
pub struct VaultWatcher {
    _debouncer: notify_debouncer_full::Debouncer<
        notify_debouncer_full::notify::RecommendedWatcher,
        notify_debouncer_full::FileIdMap,
    >,
}

/// 递归监听 Vault 根，去抖后对每个**笔记**变更路径回调 [`VaultChange`]，对每个**附件**变更路径
/// 回调 [`AssetChange`]。
///
/// 跳过 `.luna/` / `.trash/` 与点文件（自写的索引/快照/临时文件不应触发 `vault://changed`）。
pub fn watch<F, A>(root: &Path, on_change: F, on_asset: A) -> Result<VaultWatcher>
where
    F: Fn(VaultChange) + Send + 'static,
    A: Fn(AssetChange) + Send + 'static,
{
    let root_buf = root.to_path_buf();
    let mut debouncer = new_debouncer(
        Duration::from_millis(DEBOUNCE_MS),
        None,
        move |res: DebounceEventResult| {
            let Ok(events) = res else { return };
            let mut seen = std::collections::HashSet::new();
            let mut seen_assets = std::collections::HashSet::new();
            for ev in &events {
                for path in &ev.paths {
                    if let Some(asset) = classify_asset(&root_buf, path) {
                        if seen_assets.insert(asset.rel_path.clone()) {
                            on_asset(asset);
                        }
                    }
                }
            }
            for ev in events {
                let changes = ev
                    .paths
                    .iter()
                    .filter_map(|path| classify(&root_buf, path))
                    .collect::<Vec<_>>();
                if let Some(change) = paired_rename(&changes) {
                    if let Some(from) = change.moved_from.as_ref() {
                        seen.insert(from.clone());
                    }
                    if seen.insert(change.rel_path.clone()) {
                        on_change(change);
                    }
                    continue;
                }
                for change in changes {
                    {
                        // 同一去抖批次内对同路径只回调一次（二次去重的廉价补充）。
                        if seen.insert(change.rel_path.clone()) {
                            on_change(change);
                        }
                    }
                }
            }
        },
    )
    .map_err(|e| VaultError::Decode(format!("watcher 初始化失败：{e}")))?;

    debouncer
        .watcher()
        .watch(root, RecursiveMode::Recursive)
        .map_err(|e| VaultError::Decode(format!("watch 失败：{e}")))?;
    Ok(VaultWatcher {
        _debouncer: debouncer,
    })
}

/// 把绝对事件路径转为笔记相关的 [`VaultChange`]；非笔记 / 私有目录返回 `None`。
fn classify(root: &Path, path: &Path) -> Option<VaultChange> {
    // 跳过任何点目录/点文件段（.luna / .trash / .*.luna-tmp 等自写产物）。
    let rel_str = visible_rel(root, path)?;
    // 笔记走这里；附件走 [`classify_asset`]（不读内容、不进索引）。
    if !rel_str.to_lowercase().ends_with(".md") {
        return None;
    }
    let disk_hash = std::fs::read(path).ok().map(|b| ContentHash::of(&b));
    Some(VaultChange {
        rel_path: rel_str,
        disk_hash,
        moved_from: None,
    })
}

/// 库相对路径（跳过任何点目录 / 点文件段）。
fn visible_rel(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    for comp in rel.components() {
        if let std::path::Component::Normal(seg) = comp {
            if seg.to_string_lossy().starts_with('.') {
                return None;
            }
        }
    }
    Some(rel.to_string_lossy().replace('\\', "/"))
}

/// 非 `.md` 文件的变更；目录本身不算（目录的增删会带出其中文件各自的事件）。
fn classify_asset(root: &Path, path: &Path) -> Option<AssetChange> {
    let rel_path = visible_rel(root, path)?;
    if rel_path.is_empty() || rel_path.to_lowercase().ends_with(".md") || path.is_dir() {
        return None;
    }
    Some(AssetChange {
        rel_path,
        deleted: !path.exists(),
    })
}

fn paired_rename(changes: &[VaultChange]) -> Option<VaultChange> {
    if changes.len() != 2 {
        return None;
    }
    let deleted = changes.iter().find(|change| change.disk_hash.is_none())?;
    let created = changes.iter().find(|change| change.disk_hash.is_some())?;
    if deleted.rel_path == created.rel_path {
        return None;
    }
    let mut moved = created.clone();
    moved.moved_from = Some(deleted.rel_path.clone());
    Some(moved)
}

#[cfg(test)]
mod tests {
    use super::*;

    // classify 是纯路径逻辑，可 headless 测；真实 FS 事件流转留真机清单。
    #[test]
    fn classify_skips_private_and_non_md() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join(".luna")).unwrap();
        std::fs::write(root.join(".luna/index.sqlite"), b"x").unwrap();
        std::fs::write(root.join("note.md"), b"hi").unwrap();
        std::fs::write(root.join("image.png"), b"x").unwrap();

        assert!(classify(root, &root.join(".luna/index.sqlite")).is_none());
        assert!(classify(root, &root.join("image.png")).is_none());
        assert_eq!(
            classify_asset(root, &root.join("image.png")),
            Some(AssetChange {
                rel_path: "image.png".into(),
                deleted: false
            })
        );
        assert!(classify_asset(root, &root.join("note.md")).is_none());
        assert!(classify_asset(root, &root.join(".luna/index.sqlite")).is_none());
        assert!(classify_asset(root, &root.join(".luna")).is_none());
        assert!(
            classify_asset(root, &root.join("gone.png"))
                .unwrap()
                .deleted
        );
        let c = classify(root, &root.join("note.md")).unwrap();
        assert_eq!(c.rel_path, "note.md");
        assert_eq!(c.disk_hash, Some(ContentHash::of(b"hi")));
        assert_eq!(c.moved_from, None);
    }

    #[test]
    fn classify_deleted_file_has_no_hash() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // 文件不存在（已删）→ disk_hash None，rel 仍可解析。
        let c = classify(root, &root.join("gone.md")).unwrap();
        assert_eq!(c.rel_path, "gone.md");
        assert!(c.disk_hash.is_none());
    }

    #[test]
    fn pairs_one_deleted_and_one_created_markdown_path_as_a_move() {
        let moved = paired_rename(&[
            VaultChange {
                rel_path: "old.md".into(),
                disk_hash: None,
                moved_from: None,
            },
            VaultChange {
                rel_path: "folder/new.md".into(),
                disk_hash: Some(ContentHash::of(b"body")),
                moved_from: None,
            },
        ])
        .unwrap();
        assert_eq!(moved.rel_path, "folder/new.md");
        assert_eq!(moved.moved_from.as_deref(), Some("old.md"));
    }
}
