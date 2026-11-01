//! 设备状态（ADR-140）：「这台设备怎么看某个库」——布局、文件树的排序与展开状态。
//!
//! 换一台设备就不该带过去的东西不进库：此前布局写在 `.luna/workspace.json`，每切一次标签
//! 就在用户的 git 里留一处改动，放进同步盘则跨设备互相覆盖。现在落应用配置目录，按库身份
//! （`.luna/vault-id`）分存：`<配置目录>/vault-state/<库 id>/<种类>.json`。库被移动、改名
//! 后状态仍在；复制出来的库拿到新身份，不和原库共用。
//!
//! **按种类分文件**而不是一个总文件：布局与树状态由两条命令各自高频写入，一个总文件就要
//! 「读出来、改一块、写回去」，两条命令交错时后写的会把先写的那一块盖回旧值。
//!
//! 是配置数据、不是设置项（同 `luna_vault::registry`）：「恢复默认」不清它。坏了、读不出就当
//! 没有——这些都是可重建的低价值状态，绝不因此阻塞开库。

use std::path::PathBuf;

use luna_vault::VaultId;
use serde::{de::DeserializeOwned, Serialize};

/// 设备状态目录名（配置目录下）。
const DIR_NAME: &str = "vault-state";

/// 设备状态的种类（各自一个文件）。
#[derive(Clone, Copy, Debug)]
pub enum DeviceStateKind {
    /// 工作区主区布局（标签页 / 分屏 / 导航历史 / 侧栏宽度，TDD-07 §4.1）。
    Workspace,
    /// 文件树的排序方式与展开的文件夹（PRD-06-006）。
    Tree,
    /// 最近打开的文件（快速切换器空查询，PRD-07-040 ②）。
    Recent,
}

impl DeviceStateKind {
    fn file_name(self) -> &'static str {
        match self {
            Self::Workspace => "workspace.json",
            Self::Tree => "tree.json",
            Self::Recent => "recent.json",
        }
    }
}

/// 设备状态存储句柄（`None` = 配置目录不可用，读恒为空、写为空操作）。
pub struct DeviceStateStore {
    dir: Option<PathBuf>,
    /// 「读出来、改一处、写回去」的串行锁（[`Self::update`]）。只有增量更新要它：整份
    /// 覆盖写（布局、树状态）本来就是最后一次写赢。
    updating: std::sync::Mutex<()>,
}

impl DeviceStateStore {
    /// 由配置目录构造（`config.toml` 的同级目录）。
    #[must_use]
    pub fn new(config_dir: Option<PathBuf>) -> Self {
        Self {
            dir: config_dir.map(|dir| dir.join(DIR_NAME)),
            updating: std::sync::Mutex::new(()),
        }
    }

    /// `VaultId` 只能由合法 UUID 构造，拼进路径不会穿越。
    fn path(&self, vault: VaultId, kind: DeviceStateKind) -> Option<PathBuf> {
        self.dir
            .as_ref()
            .map(|dir| dir.join(vault.to_string()).join(kind.file_name()))
    }

    /// 读一种状态；没有、读不出或格式不对都当没有。
    #[must_use]
    pub fn read<T: DeserializeOwned>(&self, vault: VaultId, kind: DeviceStateKind) -> Option<T> {
        let bytes = std::fs::read(self.path(vault, kind)?).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// 写一种状态（原子写）。
    pub fn write<T: Serialize>(
        &self,
        vault: VaultId,
        kind: DeviceStateKind,
        value: &T,
    ) -> Result<(), String> {
        let Some(path) = self.path(vault, kind) else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
        luna_vault::atomic::write_atomic(&path, &bytes).map_err(|error| error.to_string())
    }

    /// 在旧值上改一处再写回（串行）：两次几乎同时的「记一次打开」不能互相盖掉。
    pub fn update<T: Serialize + DeserializeOwned + Default>(
        &self,
        vault: VaultId,
        kind: DeviceStateKind,
        change: impl FnOnce(&mut T),
    ) -> Result<(), String> {
        let _serial = self
            .updating
            .lock()
            .map_err(|_| "设备状态锁中毒".to_string())?;
        let mut value = self.read::<T>(vault, kind).unwrap_or_default();
        change(&mut value);
        self.write(vault, kind, &value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(text: &str) -> VaultId {
        VaultId::parse(text).unwrap()
    }

    #[test]
    fn state_round_trips_per_vault_and_per_kind() {
        let config = tempfile::tempdir().unwrap();
        let store = DeviceStateStore::new(Some(config.path().to_path_buf()));
        let a = id("0190f5b2-0000-7000-8000-000000000001");
        let b = id("0190f5b2-0000-7000-8000-000000000002");
        assert_eq!(store.read::<Vec<u32>>(a, DeviceStateKind::Workspace), None);
        store
            .write(a, DeviceStateKind::Workspace, &vec![1u32])
            .unwrap();
        store.write(a, DeviceStateKind::Tree, &vec![2u32]).unwrap();
        assert_eq!(
            store.read::<Vec<u32>>(a, DeviceStateKind::Workspace),
            Some(vec![1])
        );
        assert_eq!(
            store.read::<Vec<u32>>(a, DeviceStateKind::Tree),
            Some(vec![2])
        );
        assert_eq!(
            store.read::<Vec<u32>>(b, DeviceStateKind::Workspace),
            None,
            "按库分存"
        );
    }

    #[test]
    fn unreadable_state_is_treated_as_absent() {
        let config = tempfile::tempdir().unwrap();
        let store = DeviceStateStore::new(Some(config.path().to_path_buf()));
        let a = id("0190f5b2-0000-7000-8000-000000000001");
        let path = config.path().join(DIR_NAME).join(a.to_string());
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("tree.json"), "{坏的").unwrap();
        assert_eq!(store.read::<Vec<u32>>(a, DeviceStateKind::Tree), None);
        assert_eq!(
            DeviceStateStore::new(None).read::<Vec<u32>>(a, DeviceStateKind::Tree),
            None
        );
    }

    #[test]
    fn update_changes_the_stored_value_in_place() {
        let config = tempfile::tempdir().unwrap();
        let store = DeviceStateStore::new(Some(config.path().to_path_buf()));
        let a = id("0190f5b2-0000-7000-8000-000000000001");
        store
            .update::<Vec<u32>>(a, DeviceStateKind::Recent, |v| v.push(1))
            .unwrap();
        store
            .update::<Vec<u32>>(a, DeviceStateKind::Recent, |v| v.push(2))
            .unwrap();
        assert_eq!(
            store.read::<Vec<u32>>(a, DeviceStateKind::Recent),
            Some(vec![1, 2])
        );
    }
}
