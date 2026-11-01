//! 库身份（ADR-140 ②）：`.luna/vault-id` 里的一个随机 UUID。
//!
//! 「这台设备怎么看这个库」（布局、树的排序与展开、最近打开）存在应用配置目录、按库身份
//! 分存，不进库。身份不能是路径——库被移动、改名后状态要还在；也不能是内容指纹——内容天天
//! 在变。于是首次打开时在 `.luna/` 写一个随机 id（PRD-06-004 允许的 `.luna/` 纯附加写）。
//!
//! 复制整个库会把 id 一起复制过去：两份库共用一套标签页会互相剪枝。哪一份「后打开」只有
//! 应用层的库注册表知道，所以判定在那里，这里只提供 [`crate::Vault::reassign_id`]。

use crate::atomic::write_atomic;
use crate::error::Result;
use std::path::Path;
use uuid::Uuid;

/// `.luna/` 下的身份文件名。
pub const VAULT_ID_FILE: &str = "vault-id";

/// 库身份。只能由合法 UUID 构造——它会被拼进应用配置目录下的路径，一个被改成 `../x`
/// 的身份文件不能变成路径穿越。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VaultId(Uuid);

impl VaultId {
    /// 从字符串解析（非法即 `None`）。
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Uuid::parse_str(text.trim()).ok().map(Self)
    }

    fn generate() -> Self {
        Self(Uuid::now_v7())
    }
}

impl std::fmt::Display for VaultId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.hyphenated().fmt(f)
    }
}

/// 读出库身份；没有、读不出或不是合法 UUID 为 `None`（只读打开不补写）。
pub(crate) fn read(luna: &Path) -> Option<VaultId> {
    std::fs::read_to_string(luna.join(VAULT_ID_FILE))
        .ok()
        .and_then(|text| VaultId::parse(&text))
}

/// 读出库身份；没有、读不出或不是合法 UUID 时生成一个并写入。
pub(crate) fn ensure(luna: &Path) -> Result<VaultId> {
    match read(luna) {
        Some(id) => Ok(id),
        None => write(luna),
    }
}

/// 生成一个新身份并写入（覆盖旧的）。
pub(crate) fn write(luna: &Path) -> Result<VaultId> {
    let id = VaultId::generate();
    write_atomic(&luna.join(VAULT_ID_FILE), format!("{id}\n").as_bytes())?;
    Ok(id)
}

/// 删掉库内的旧布局文件（ADR-140 ③）：设备状态已移到库外，按 ADR-046 不读旧文件，升级后
/// 首次打开时删除——它留在库里，用户的 git 每切一次标签就多一处改动，这正是要收回的泄漏。
pub(crate) fn remove_legacy_device_state(luna: &Path) {
    let _ = std::fs::remove_file(luna.join("workspace.json"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_creates_once_and_then_reads_the_same_id() {
        let dir = tempfile::tempdir().unwrap();
        let first = ensure(dir.path()).unwrap();
        assert_eq!(ensure(dir.path()).unwrap(), first);
        let text = std::fs::read_to_string(dir.path().join(VAULT_ID_FILE)).unwrap();
        assert_eq!(VaultId::parse(&text), Some(first));
    }

    #[test]
    fn a_tampered_id_is_replaced_not_trusted() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(VAULT_ID_FILE), "../../etc").unwrap();
        let id = ensure(dir.path()).unwrap();
        assert!(VaultId::parse(&id.to_string()).is_some());
        assert_ne!(
            std::fs::read_to_string(dir.path().join(VAULT_ID_FILE)).unwrap(),
            "../../etc"
        );
    }

    #[test]
    fn write_replaces_the_id() {
        let dir = tempfile::tempdir().unwrap();
        let first = ensure(dir.path()).unwrap();
        let second = write(dir.path()).unwrap();
        assert_ne!(first, second);
        assert_eq!(ensure(dir.path()).unwrap(), second);
    }
}
