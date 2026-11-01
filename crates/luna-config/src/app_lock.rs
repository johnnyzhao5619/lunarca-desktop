//! 「应用正在运行」的标记（ADR-141 ④）：配置目录里一个文件，应用整个生命周期持有它的独占锁。
//!
//! CLI 据此决定两件事：`luna vault index` 拒绝执行（两个进程同时写同一份派生索引没有赢家）；
//! 只读命令在应用没开时才去比对「索引是不是最新」（应用开着时有监听在维护索引）。
//!
//! 用操作系统的文件锁而不是「pid 文件 + 进程是否存活」：进程崩溃时锁随句柄自动释放，不会留下
//! 一个永远显示「应用在跑」的陈旧标记；pid 也会被复用。文件本身不承载内容，删了也只是下次重建。

use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;

/// 配置目录下的锁文件名。
pub const APP_LOCK_FILE: &str = "app.lock";

/// 持有中的锁；丢弃即释放（应用把它托管到退出）。
#[derive(Debug)]
pub struct AppLock {
    _file: File,
}

/// 取独占锁。已被别的进程持有（另一个应用实例，或正在跑的 `luna vault index`）或文件打不开
/// 时返回 `None`——调用方照常运行，只是这一份不算数。
#[must_use]
pub fn hold(config_dir: &Path) -> Option<AppLock> {
    std::fs::create_dir_all(config_dir).ok()?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(config_dir.join(APP_LOCK_FILE))
        .ok()?;
    file.try_lock().ok()?;
    Some(AppLock { _file: file })
}

/// 此刻是否有进程持有锁。文件不存在 = 从没有应用在这台设备上跑过 = 没有。
#[must_use]
pub fn is_held(config_dir: &Path) -> bool {
    let Ok(file) = File::open(config_dir.join(APP_LOCK_FILE)) else {
        return false;
    };
    // 拿得到就说明没人持有（随即随句柄释放）；只有「会阻塞」才是被占着。
    matches!(file.try_lock(), Err(TryLockError::WouldBlock))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn held_until_dropped() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_held(dir.path()));
        let lock = hold(dir.path()).unwrap();
        assert!(is_held(dir.path()));
        assert!(hold(dir.path()).is_none(), "第二个持有者拿不到");
        drop(lock);
        assert!(!is_held(dir.path()));
        assert!(hold(dir.path()).is_some());
    }
}
