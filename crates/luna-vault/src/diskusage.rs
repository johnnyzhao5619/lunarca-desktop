//! 一条路径占多少磁盘（递归求和；文件直接给长度）。
//!
//! **两处需要同一个答案**：设置 › 数据足迹要报「回收站占用多少」，回收站列表要报
//! 「这一条占多少」。各写一份递归求和，迟早在"符号链接算不算""跳过哪些子树"上分叉，
//! 而用户会在同一屏上看到两个对不上的数字。
//!
//! 判据固定两条：**符号链接一律算 0**（跟过去会把库外的东西计进来，甚至在环上打转），
//! **读不到就算 0**（权限不足的子树不该让整个数字变成错误——少算一点比报不出来强）。

use std::path::{Path, PathBuf};

/// `path` 占用的字节数；`skip` 里的路径整棵不计。
///
/// 目录递归求和，常规文件取长度，符号链接与读不到的路径计 0。
#[must_use]
pub fn path_bytes(path: &Path, skip: &[PathBuf]) -> u64 {
    if skip.iter().any(|s| s == path) {
        return 0;
    }
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if meta.file_type().is_symlink() {
        return 0;
    }
    if meta.is_file() {
        return meta.len();
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| path_bytes(&entry.path(), skip))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sums_regular_files_and_skips_symlinks_and_skipped_subtrees() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.bin"), [0u8; 10]).unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/b.bin"), [0u8; 5]).unwrap();
        std::fs::create_dir(dir.path().join("skip")).unwrap();
        std::fs::write(dir.path().join("skip/c.bin"), [0u8; 100]).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.path().join("skip"), dir.path().join("link")).unwrap();
        assert_eq!(path_bytes(dir.path(), &[dir.path().join("skip")]), 15);
        assert_eq!(path_bytes(dir.path(), &[]), 115);
    }

    #[test]
    fn missing_path_counts_as_zero() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(path_bytes(&dir.path().join("不存在"), &[]), 0);
    }
}
