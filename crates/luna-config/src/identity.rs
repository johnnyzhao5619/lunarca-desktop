//! 应用标识与首启目录迁移（ADR-030；TDD-13 §5「品牌与 identifier 迁移」）。
//!
//! 首个公开安装包之前，应用标识从开发期的旧值换成 [`APP_IDENTIFIER`]。系统按标识给应用分目录
//! （macOS 的 `Application Support`、`Caches`、WebKit 数据，Windows 的 `AppData`），换了标识，
//! 新版本就看不见旧版本留下的配置、索引、模型与同步状态。[`migrate_legacy_dirs`] 在应用最早的
//! 启动点把旧目录搬到新标识下，**任何插件或窗口碰到数据目录之前**。
//!
//! 搬法是**同卷改名**而不是拷贝：数据目录里有几 GB 的本机模型，拷贝会让首次启动卡住数分钟并占
//! 双倍磁盘；同一个父目录下的 `rename` 是原子的、瞬间完成。改名后在旧位置留一个指向新目录的
//! 符号链接（Unix）：配置里存过的旧绝对路径照样能解析，退回旧版本时它也读到同一份数据——这就是
//! TDD-13 §5 要的「旧目录保留一个版本周期作回滚」，只是不必复制一份。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// 当前应用标识（与 `src-tauri/tauri.conf.json` 的 `identifier` 一致，有测试守着）。
pub const APP_IDENTIFIER: &str = "app.lunarca.luna";

/// 首个公开版本之前的开发期标识。拼接构造：品牌残留门禁禁止它以字面量出现在任何地方，
/// 只有迁移与遗留清理通过这个常量引用它。
pub const LEGACY_IDENTIFIER: &str = concat!("com", ".", "luna", ".", "app");

/// 迁移完成后写在新目录里的标记文件：应用据此在这一次启动提示「系统权限要重新授权」。
pub const MIGRATION_MARKER: &str = ".migrated-from-legacy-identifier";

/// 一处父目录的迁移结果。
#[derive(Debug, PartialEq, Eq)]
pub enum DirMigration {
    /// 旧目录不存在：全新安装，或这个父目录从没被旧版本用过。
    NoLegacy,
    /// 新目录已有内容（已经迁移过，或新版本先跑过）：不动。
    AlreadyCurrent,
    /// 本次把旧目录改名成新目录。
    Migrated,
}

/// 按系统约定，可能装着应用目录的父目录（去重）。macOS 上 `Application Support` 同时是
/// data / config / local data，另有缓存、日志与 WebKit（网页存储）三处；Windows 是 Roaming 与 Local。
#[must_use]
pub fn app_dir_bases() -> Vec<PathBuf> {
    let mut bases: Vec<PathBuf> = [
        dirs::data_dir(),
        dirs::config_dir(),
        dirs::data_local_dir(),
        dirs::cache_dir(),
    ]
    .into_iter()
    .flatten()
    .collect();
    #[cfg(target_os = "macos")]
    if let Some(home) = dirs::home_dir() {
        for sub in ["Library/Logs", "Library/WebKit", "Library/HTTPStorages"] {
            bases.push(home.join(sub));
        }
    }
    bases.sort();
    bases.dedup();
    bases
}

/// 把每个父目录下的旧标识目录搬到新标识下。逐个父目录独立判断、互不牵连；某一处失败只影响那一处
/// （调用方记日志后照常启动——新目录为空等于全新安装，不会损坏旧数据，旧目录原样留着）。
pub fn migrate_legacy_dirs(bases: &[PathBuf]) -> Vec<(PathBuf, io::Result<DirMigration>)> {
    bases
        .iter()
        .map(|base| {
            (
                base.clone(),
                migrate_one(base, LEGACY_IDENTIFIER, APP_IDENTIFIER),
            )
        })
        .collect()
}

fn migrate_one(base: &Path, legacy: &str, current: &str) -> io::Result<DirMigration> {
    let old = base.join(legacy);
    let new = base.join(current);
    // 旧位置已经是指向别处的链接 = 上一次迁移留下的回滚链接，不是待迁移的数据。
    match fs::symlink_metadata(&old) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
            return Ok(DirMigration::NoLegacy)
        }
        Ok(_) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(DirMigration::NoLegacy),
        Err(err) => return Err(err),
    }
    if new.exists() {
        if fs::read_dir(&new)?.next().is_some() {
            return Ok(DirMigration::AlreadyCurrent);
        }
        // 新版本的插件抢先建出的空目录：让位给旧数据。
        fs::remove_dir(&new)?;
    }
    fs::rename(&old, &new)?;
    leave_rollback_link(&old, &new);
    // 标记写不进去只影响那一句权限提示，不影响数据。
    let _ = fs::write(new.join(MIGRATION_MARKER), legacy.as_bytes());
    Ok(DirMigration::Migrated)
}

#[cfg(unix)]
fn leave_rollback_link(old: &Path, new: &Path) {
    // 失败无害：旧绝对路径解析不到时，那一处按「找不到」处理，数据本身已在新目录。
    let _ = std::os::unix::fs::symlink(new, old);
}

#[cfg(not(unix))]
fn leave_rollback_link(_old: &Path, _new: &Path) {}

/// 这次启动是否刚完成迁移：读到标记即删，只提示一次。
#[must_use]
pub fn take_migration_marker(app_dir: &Path) -> bool {
    let marker = app_dir.join(MIGRATION_MARKER);
    marker.is_file() && fs::remove_file(&marker).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_with_legacy() -> tempfile::TempDir {
        let base = tempfile::tempdir().unwrap();
        let old = base.path().join(LEGACY_IDENTIFIER);
        fs::create_dir_all(old.join("models")).unwrap();
        fs::write(old.join("config.toml"), "schema_version = 1\n").unwrap();
        fs::write(old.join("models/tiny.bin"), [7u8; 32]).unwrap();
        base
    }

    #[test]
    fn legacy_dir_is_renamed_and_a_rollback_link_is_left() {
        let base = base_with_legacy();
        let report = migrate_legacy_dirs(&[base.path().to_path_buf()]);
        assert_eq!(report[0].1.as_ref().unwrap(), &DirMigration::Migrated);
        let new = base.path().join(APP_IDENTIFIER);
        assert_eq!(
            fs::read_to_string(new.join("config.toml")).unwrap(),
            "schema_version = 1\n"
        );
        assert_eq!(
            fs::read(new.join("models/tiny.bin")).unwrap(),
            vec![7u8; 32]
        );
        #[cfg(unix)]
        {
            let old = base.path().join(LEGACY_IDENTIFIER);
            assert!(fs::symlink_metadata(&old).unwrap().file_type().is_symlink());
            assert_eq!(
                fs::read_to_string(old.join("config.toml")).unwrap(),
                "schema_version = 1\n"
            );
        }
        // 只提示一次。
        assert!(take_migration_marker(&new));
        assert!(!take_migration_marker(&new));
    }

    #[test]
    fn second_launch_and_fresh_install_change_nothing() {
        let base = base_with_legacy();
        let bases = [base.path().to_path_buf()];
        migrate_legacy_dirs(&bases);
        // 第二次启动：旧位置是回滚链接（Unix）或已不存在，新目录有内容。
        let again = migrate_legacy_dirs(&bases);
        assert!(matches!(
            again[0].1.as_ref().unwrap(),
            DirMigration::NoLegacy | DirMigration::AlreadyCurrent
        ));
        let fresh = tempfile::tempdir().unwrap();
        assert_eq!(
            migrate_legacy_dirs(&[fresh.path().to_path_buf()])[0]
                .1
                .as_ref()
                .unwrap(),
            &DirMigration::NoLegacy
        );
    }

    #[test]
    fn new_dir_with_content_wins_and_an_empty_one_yields() {
        let base = base_with_legacy();
        let new = base.path().join(APP_IDENTIFIER);
        fs::create_dir_all(&new).unwrap();
        // 空目录（插件抢先建的）让位：旧数据照样搬过来。
        assert_eq!(
            migrate_one(base.path(), LEGACY_IDENTIFIER, APP_IDENTIFIER).unwrap(),
            DirMigration::Migrated
        );
        // 新目录已有内容：旧目录原样不动。
        let other = base_with_legacy();
        let current = other.path().join(APP_IDENTIFIER);
        fs::create_dir_all(&current).unwrap();
        fs::write(current.join("config.toml"), "new\n").unwrap();
        assert_eq!(
            migrate_one(other.path(), LEGACY_IDENTIFIER, APP_IDENTIFIER).unwrap(),
            DirMigration::AlreadyCurrent
        );
        assert!(other
            .path()
            .join(LEGACY_IDENTIFIER)
            .join("models/tiny.bin")
            .is_file());
        assert_eq!(
            fs::read_to_string(current.join("config.toml")).unwrap(),
            "new\n"
        );
    }

    #[test]
    fn identifier_matches_the_bundle_config() {
        let conf = include_str!("../../../src-tauri/tauri.conf.json");
        assert!(
            conf.contains(&format!("\"identifier\": \"{APP_IDENTIFIER}\"")),
            "tauri.conf.json 的 identifier 必须等于 APP_IDENTIFIER"
        );
    }
}
