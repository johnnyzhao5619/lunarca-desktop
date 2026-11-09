//! 自写回声抑制 + 外部编辑判定（TDD-06 §4，三路合并的前置防线）。
//!
//! Lunarca 自己的 `Vault::write`（§4.3 原子写、自动保存）会触发自己的文件事件；若不区分，会把
//! **自写误判为外部修改**而触发冲突流程（自己跟自己合并）。`hash` 二次去重只能消重复事件、
//! **不能识别"内容确实变了但是我改的"**（自写后 hash 必与旧态不同）。故须显式机制：每次
//! `Vault::write` 成功后记录该 path 刚写入内容的 hash（`last_written_hash`）；监听事件到达时，
//! 若文件当前 hash == 该 path 的 `last_written_hash` → 判为自写、跳过合并与 `vault://changed`。
//!
//! 本模块只判定「这次事件要不要当外部改动处理」：无脏页静默重载、有脏页给冲突横幅。横幅上的
//! 「合并两边改动」由 [`luna_sync_core::merge3`] 计算（ADR-148 ②、TDD-06 §4.2），不在这里。

use luna_sync_core::hashing::ContentHash;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// 自写指纹登记表：path → 最近一次由 Lunarca 自己写入的内容指纹。
#[derive(Debug, Default)]
pub struct WriteGuard {
    last_written: HashMap<PathBuf, ContentHash>,
}

impl WriteGuard {
    /// 新建空登记表。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 记录一次自写：`Vault::write` 成功落盘后调用，登记刚写入字节的指纹。
    pub fn record_write(&mut self, path: impl Into<PathBuf>, hash: ContentHash) {
        self.last_written.insert(path.into(), hash);
    }

    /// 取某 path 的自写指纹（用于判定）。
    #[must_use]
    pub fn last_written(&self, path: &Path) -> Option<&ContentHash> {
        self.last_written.get(path)
    }

    /// 文件被移除/重命名后清掉其登记（避免陈旧指纹误判后续同路径文件）。
    pub fn forget(&mut self, path: &Path) {
        self.last_written.remove(path);
    }

    /// 文件夹被移除后，清除所有该文件夹下的路径指纹登记。
    pub fn forget_dir(&mut self, dir_path: &Path) {
        self.last_written
            .retain(|path, _| !path.starts_with(dir_path));
    }
}

/// 外部编辑事件的判定结果（粗粒度，Q1/B）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalEditOutcome {
    /// 自写回声：disk == last_written_hash。跳过合并与 `vault://changed`，仅刷新索引一致性元数据。
    SelfWrite,
    /// 无实际变化：disk == base（幽灵/重复事件）。不重索引、不刷 UI。
    NoChange,
    /// 外部改动且编辑器**无脏页** → 静默重载编辑器（先留快照）。
    CleanReload,
    /// 外部改动且编辑器**有脏页** → 留快照 + 提示「重载 / 保留本地」（绝不写冲突标记）。
    Conflict,
}

/// 判定一次到达的文件变更事件如何处理。**纯函数**（无 IO），便于 headless 重测。
///
/// - `disk_hash`：事件到达后文件**当前**内容指纹。
/// - `last_written`：该 path 的自写指纹（[`WriteGuard::last_written`]）。
/// - `base_hash`：编辑器/索引当前持有的基准内容指纹（上次加载或写入的已知态）。
/// - `dirty`：编辑器是否有未落盘改动（脏页）。
#[must_use]
pub fn decide_external_edit(
    disk_hash: &ContentHash,
    last_written: Option<&ContentHash>,
    base_hash: &ContentHash,
    dirty: bool,
) -> ExternalEditOutcome {
    // ① 自写回声优先：哪怕内容与 base 不同（自写后必不同），只要等于自写指纹即判自写。
    if last_written == Some(disk_hash) {
        return ExternalEditOutcome::SelfWrite;
    }
    // ② 与基准相同 = 无实际变化（含幽灵事件）。
    if disk_hash == base_hash {
        return ExternalEditOutcome::NoChange;
    }
    // ③ 真外部改动：有脏页 → 冲突；无脏页 → 静默重载。
    if dirty {
        ExternalEditOutcome::Conflict
    } else {
        ExternalEditOutcome::CleanReload
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(s: &str) -> ContentHash {
        ContentHash::of(s.as_bytes())
    }

    #[test]
    fn self_write_is_suppressed_even_when_content_changed() {
        // 自写后 disk 必与 base 不同，但等于 last_written → 必须判自写，不触发冲突。
        let base = h("v1");
        let written = h("v2"); // Lunarca 刚把 v1 改成 v2。
        let disk = h("v2"); // 监听到的就是这次自写。
        assert_eq!(
            decide_external_edit(&disk, Some(&written), &base, /*dirty*/ false),
            ExternalEditOutcome::SelfWrite
        );
    }

    #[test]
    fn ghost_event_no_change() {
        let base = h("v1");
        let disk = h("v1");
        assert_eq!(
            decide_external_edit(&disk, Some(&h("v0")), &base, false),
            ExternalEditOutcome::NoChange
        );
    }

    #[test]
    fn external_edit_clean_reload_when_not_dirty() {
        let base = h("v1");
        let disk = h("external-v2");
        assert_eq!(
            decide_external_edit(&disk, Some(&h("v1")), &base, false),
            ExternalEditOutcome::CleanReload
        );
    }

    #[test]
    fn external_edit_conflict_when_dirty() {
        let base = h("v1");
        let disk = h("external-v2");
        assert_eq!(
            decide_external_edit(&disk, None, &base, true),
            ExternalEditOutcome::Conflict
        );
    }

    #[test]
    fn guard_records_and_forgets() {
        let mut g = WriteGuard::new();
        let p = Path::new("/vault/a.md");
        g.record_write(p, h("x"));
        assert_eq!(g.last_written(p), Some(&h("x")));
        g.forget(p);
        assert_eq!(g.last_written(p), None);
    }
}
