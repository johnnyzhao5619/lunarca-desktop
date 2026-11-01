//! 新建库的默认位置（PRD-14-040）与「这个位置在 OneDrive 里」的判定。
//!
//! Windows 上 OneDrive 的「电脑文件夹备份」会把「文档」重定向进 OneDrive；默认库若照旧落在
//! 「文档」下，OneDrive 与 Lunarca 同步就会同时改同一批文件，互相生出冲突副本。所以「文档」在
//! OneDrive 里时，默认库改到用户目录下的 [`ONEDRIVE_AVOIDING_DIR`]；用户自己选了 OneDrive 里的
//! 位置只提示、不拦（有人就是要用 OneDrive 同步笔记）。
//!
//! OneDrive 的根取自它自己设的环境变量（[`ONEDRIVE_ENV_VARS`]）；「文档」取自
//! `SHGetKnownFolderPath(FOLDERID_Documents)`（Tauri 的 `document_dir` 在 Windows 上即此调用）。
//! 其它平台没有这些环境变量，判定自然不生效。判定全是纯函数，在任何平台上单测。

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

/// 默认库的目录名（「文档」下）。
pub const DEFAULT_VAULT_DIR: &str = "LunaVault";
/// 「文档」在 OneDrive 里时，默认库在用户目录下用的目录名。
pub const ONEDRIVE_AVOIDING_DIR: &str = "Lunarca";
/// OneDrive 客户端设的同步根环境变量（个人版、工作或学校版，以及最近登录的那个）。
pub const ONEDRIVE_ENV_VARS: [&str; 3] = ["OneDrive", "OneDriveConsumer", "OneDriveCommercial"];

/// 建议的默认库位置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultVaultLocation {
    pub path: PathBuf,
    /// 「文档」在 OneDrive 里，所以改放到了用户目录下。
    pub avoided_onedrive: bool,
}

/// 从环境变量取 OneDrive 的同步根（`get` 注入便于单测）：空值与相对路径不算。
pub fn onedrive_roots_from(get: impl Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    ONEDRIVE_ENV_VARS
        .iter()
        .filter_map(|name| get(name))
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .collect()
}

/// 本机 OneDrive 的同步根（读进程环境变量）。
#[must_use]
pub fn onedrive_roots() -> Vec<PathBuf> {
    onedrive_roots_from(|name| std::env::var_os(name))
}

/// 路径分量的比较形：Windows 路径不分大小写，`\\?\C:` 与 `C:` 是同一个盘。
fn comparable(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|component| match component {
            Component::Prefix(prefix) => Some(match prefix.kind() {
                std::path::Prefix::Disk(letter) | std::path::Prefix::VerbatimDisk(letter) => {
                    format!("{}:", char::from(letter).to_ascii_lowercase())
                }
                std::path::Prefix::UNC(server, share)
                | std::path::Prefix::VerbatimUNC(server, share) => format!(
                    r"\\{}\{}",
                    server.to_string_lossy().to_lowercase(),
                    share.to_string_lossy().to_lowercase()
                ),
                _ => prefix.as_os_str().to_string_lossy().to_lowercase(),
            }),
            Component::RootDir => Some(String::from("/")),
            Component::CurDir => None,
            Component::ParentDir => Some(String::from("..")),
            Component::Normal(name) => Some(name.to_string_lossy().to_lowercase()),
        })
        .collect()
}

/// `path` 在 `root` 里（含 `root` 本身）：按路径分量比，不分大小写——`C:\Users\me\OneDriveX`
/// 不在 `C:\Users\me\OneDrive` 里。
#[must_use]
pub fn is_within(path: &Path, root: &Path) -> bool {
    let path = comparable(path);
    let root = comparable(root);
    !root.is_empty() && path.len() >= root.len() && path[..root.len()] == root[..]
}

/// `path` 落在任一 OneDrive 同步根里。
#[must_use]
pub fn inside_onedrive(path: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|root| is_within(path, root))
}

/// 默认库位置（纯逻辑）：「文档」不在 OneDrive 里 → 「文档」下的 [`DEFAULT_VAULT_DIR`]；在的话 →
/// 用户目录下的 [`ONEDRIVE_AVOIDING_DIR`]（用户目录取不到、或它本身也在 OneDrive 里，就只好照旧）；
/// 「文档」取不到 → 用户目录下的 [`DEFAULT_VAULT_DIR`]。两者都取不到 = `None`。
#[must_use]
pub fn default_vault_location(
    documents: Option<&Path>,
    home: Option<&Path>,
    onedrive: &[PathBuf],
) -> Option<DefaultVaultLocation> {
    match (documents, home) {
        (Some(documents), Some(home))
            if inside_onedrive(documents, onedrive) && !inside_onedrive(home, onedrive) =>
        {
            Some(DefaultVaultLocation {
                path: home.join(ONEDRIVE_AVOIDING_DIR),
                avoided_onedrive: true,
            })
        }
        (Some(base), _) | (None, Some(base)) => Some(DefaultVaultLocation {
            path: base.join(DEFAULT_VAULT_DIR),
            avoided_onedrive: false,
        }),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用的用户目录：两种平台各用自己的绝对路径写法，判定逻辑不变。
    fn home() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\Users\me")
        } else {
            PathBuf::from("/Users/me")
        }
    }

    #[test]
    fn documents_inside_onedrive_moves_the_default_to_the_user_folder() {
        let onedrive = vec![home().join("OneDrive")];
        let documents = home().join("OneDrive").join("Documents");
        let location = default_vault_location(Some(&documents), Some(&home()), &onedrive);
        assert_eq!(
            location,
            Some(DefaultVaultLocation {
                path: home().join("Lunarca"),
                avoided_onedrive: true,
            })
        );
    }

    #[test]
    fn documents_outside_onedrive_keeps_the_usual_default() {
        let onedrive = vec![home().join("OneDrive")];
        let documents = home().join("Documents");
        let location =
            default_vault_location(Some(&documents), Some(&home()), &onedrive).expect("location");
        assert_eq!(location.path, documents.join("LunaVault"));
        assert!(!location.avoided_onedrive);
        let none = default_vault_location(Some(&documents), Some(&home()), &[]).expect("location");
        assert!(!none.avoided_onedrive, "没有 OneDrive 就不改");
    }

    #[test]
    fn missing_folders_fall_back_in_order() {
        let onedrive = vec![home().join("OneDrive")];
        let only_home = default_vault_location(None, Some(&home()), &onedrive).expect("home");
        assert_eq!(only_home.path, home().join("LunaVault"));
        let documents = home().join("OneDrive").join("Documents");
        let no_home = default_vault_location(Some(&documents), None, &onedrive).expect("docs");
        assert_eq!(no_home.path, documents.join("LunaVault"));
        assert!(!no_home.avoided_onedrive);
        assert_eq!(default_vault_location(None, None, &onedrive), None);
    }

    #[test]
    fn home_itself_inside_onedrive_has_nowhere_better_to_go() {
        let onedrive = vec![home()];
        let documents = home().join("Documents");
        let location =
            default_vault_location(Some(&documents), Some(&home()), &onedrive).expect("location");
        assert_eq!(location.path, documents.join("LunaVault"));
        assert!(!location.avoided_onedrive);
    }

    #[test]
    fn within_compares_whole_components_and_ignores_case() {
        let root = home().join("OneDrive");
        assert!(is_within(&root, &root));
        assert!(is_within(&home().join("onedrive").join("Notes"), &root));
        assert!(!is_within(&home().join("OneDriveX").join("Notes"), &root));
        assert!(!is_within(&home(), &root));
        assert!(!is_within(&home(), Path::new("")));
    }

    #[cfg(windows)]
    #[test]
    fn verbatim_and_plain_drive_prefixes_are_the_same_drive() {
        let root = Path::new(r"C:\Users\me\OneDrive - Contoso");
        assert!(is_within(
            Path::new(r"\\?\c:\users\ME\OneDrive - Contoso\Docs"),
            root
        ));
        assert!(!is_within(
            Path::new(r"D:\Users\me\OneDrive - Contoso"),
            root
        ));
    }

    #[test]
    fn roots_come_from_the_three_variables_and_skip_empty_or_relative() {
        let commercial = home().join("OneDrive - Contoso");
        let consumer = home().join("OneDrive");
        let roots = onedrive_roots_from(|name| match name {
            "OneDrive" => Some(consumer.clone().into_os_string()),
            "OneDriveConsumer" => Some(OsString::new()),
            "OneDriveCommercial" => Some(commercial.clone().into_os_string()),
            _ => Some(OsString::from("relative")),
        });
        assert_eq!(roots, vec![consumer.clone(), commercial.clone()]);
        assert!(inside_onedrive(&commercial.join("Notes"), &roots));
        assert!(!inside_onedrive(&home().join("Lunarca"), &roots));
        assert!(onedrive_roots_from(|_| None).is_empty());
    }
}
