//! 原子写（TDD-06 §4.3）。
//!
//! 避免外部编辑器读到半截文件。**临时文件必须与目标同目录/同卷**——`rename` 仅在同一
//! 文件系统上才原子（写 `/tmp` 再跨卷移动不原子）。流程：同目录唯一名临时文件 → 写入 →
//! `fsync` 文件 → `rename` 覆盖目标 → 尽力 `fsync` 目录使重命名落地。临时文件在任何错误
//! 路径上都被清理，绝不残留半截 `.tmp`。

use crate::error::{Result, VaultError};
use std::fs::{self, File};
use std::io::Write as _;
use std::path::Path;

/// 原子地把 `bytes` 写入 `target`（覆盖既有）。
///
/// `target` 的父目录必须已存在（调用方负责 `create_dir_all`，避免本函数隐式造目录）。
pub fn write_atomic(target: &Path, bytes: &[u8]) -> Result<()> {
    write_atomic_with(target, bytes, Durability::Full)
}

/// 一批写里的一个文件：同样原子（内容先落到磁盘控制器再改名，崩溃后不会是空的或半截的），
/// 但不逼磁盘把缓存刷进介质——那是 macOS `F_FULLFSYNC` 的每次十几毫秒。调用方写完整批后
/// 必须调一次 [`flush_durable`]，整批才算持久。快照库的写入恒走这条：它总是紧跟在一次
/// 完整保存之前或之中，那次保存的全量刷新会把它一并带进介质。
pub fn write_atomic_batched(target: &Path, bytes: &[u8]) -> Result<()> {
    write_atomic_with(target, bytes, Durability::Batched)
}

/// 让此前所有 [`write_atomic_batched`] 写入真正持久：一次全量刷新（macOS 上 `F_FULLFSYNC`
/// 刷的是整块盘的缓存，不只这一个文件）。`path` 是库里任一已存在的文件或目录。
pub fn flush_durable(path: &Path) -> Result<()> {
    // Windows：目录不能以 `File::open` 打开（拒绝访问），目录句柄也不能 FlushFileBuffers；
    // 而批量写在 Windows 上已逐个 `sync_data`（即 FlushFileBuffers，见 `sync_light`），
    // 这里没有可补的。此前调用方传的都是库根目录，于是 Windows 上每次移动 / 改名都在文件
    // 已搬完之后报「拒绝访问」（Windows 真机 2026-10-14）。
    #[cfg(windows)]
    if path.is_dir() {
        return Ok(());
    }
    File::open(path)?.sync_all()?;
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Durability {
    Full,
    Batched,
}

fn sync_light(file: &File) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        rustix::fs::fsync(file).map_err(std::io::Error::from)
    }
    #[cfg(not(target_os = "macos"))]
    {
        file.sync_data()
    }
}

fn write_atomic_with(target: &Path, bytes: &[u8], durability: Durability) -> Result<()> {
    let parent = target.parent().ok_or_else(|| {
        VaultError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "目标无父目录",
        ))
    })?;

    // 同目录唯一名临时文件：`.<uuid>.luna-tmp`（点号前缀避免被监听/索引误当笔记）。
    let tmp = parent.join(format!(".{}.luna-tmp", uuid::Uuid::now_v7()));

    // 用闭包集中错误处理，确保任何失败都清理临时文件。
    let result = (|| -> Result<()> {
        let mut f = File::create(&tmp)?;
        f.write_all(bytes)?;
        f.flush()?;
        // 内容落盘后才 rename，防崩溃后 rename 出空/截断文件。
        match durability {
            Durability::Full => f.sync_all()?,
            Durability::Batched => sync_light(&f)?,
        }
        fs::rename(&tmp, target)?;
        Ok(())
    })();

    if result.is_err() {
        // 失败：尽力清理临时文件，不掩盖原始错误。
        let _ = fs::remove_file(&tmp);
        return result;
    }

    // 成功后尽力 fsync 目录，使重命名条目持久（失败不致命，不回滚已成功的写）。批量写的目录
    // 条目由整批末尾的 `flush_durable` 一并落地。
    if durability == Durability::Full {
        if let Ok(dir) = File::open(parent) {
            let _ = dir.sync_all();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("note.md");
        write_atomic(&target, b"v1").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"v1");
        write_atomic(&target, b"v2-longer").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"v2-longer");
    }

    #[test]
    fn leaves_no_tmp_files() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("note.md");
        write_atomic(&target, b"hi").unwrap();
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".luna-tmp"))
            .collect();
        assert!(leftovers.is_empty(), "残留临时文件：{leftovers:?}");
    }

    #[test]
    fn errors_when_parent_missing() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("nope").join("note.md");
        // 父目录不存在 → 报错且不 panic。
        assert!(write_atomic(&target, b"x").is_err());
    }
}
