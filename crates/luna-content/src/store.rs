//! 内容层存储：`<根>/<域>/<id>/<版本>/` 下放一份已校验的内容（ADR-159 ④ / ADR-169 ①）。
//!
//! 同一个 id 只留一个版本：装上新版本后删掉旧的。写入先落 `<域>/.staging/` 再整目录改名，所以
//! 最终路径一出现就是完整的——不会有一套「装了一半」的主题被列出来。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use luna_content_schema::{ContentId, PackVersion, RelPath};
use thiserror::Error;

/// 内容域：目录名与两类商品一一对应。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentDomain {
    Themes,
    /// 模板包（ADR-161）：清单加笔记模板、AI 预设、整理模板、提示词与配方。
    TemplatePacks,
}

impl ContentDomain {
    /// 目录名（也是持久化契约：改了它，已装的内容就找不到了）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Themes => "themes",
            Self::TemplatePacks => "template-packs",
        }
    }
}

/// 临时目录名（不会与合法 id 冲突：id 不能以 `.` 开头）。
const STAGING: &str = ".staging";

#[derive(Debug, Error)]
pub enum ContentError {
    /// 这个 id 已经装了同一版本或更新的版本（不覆盖，PRD-18-024 假设）。
    #[error("{id} {installed} is already installed")]
    AlreadyInstalled {
        id: ContentId,
        installed: PackVersion,
    },
    #[error("content storage failed: {0}")]
    Io(#[from] std::io::Error),
}

/// 内容层里的一份已装内容。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledItem {
    pub id: ContentId,
    pub version: PackVersion,
    /// 这份内容的目录（`<根>/<域>/<id>/<版本>/`）。
    pub dir: PathBuf,
}

/// 内容层存储。
#[derive(Clone, Debug)]
pub struct ContentStore {
    root: PathBuf,
}

impl ContentStore {
    /// `root` 是内容层的根（应用数据目录下的 `content/`）；不存在时用到才建。
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn domain_dir(&self, domain: ContentDomain) -> PathBuf {
        self.root.join(domain.as_str())
    }

    /// 某个域里已装的全部内容（按 id 排序；一个 id 若残留多个版本只报最高的那个）。目录名不是
    /// 合法 id / 版本的条目一律跳过——那不是我们写的。
    ///
    /// # Errors
    /// 读目录失败（权限等）时；域目录还不存在不算错，返回空。
    pub fn list(&self, domain: ContentDomain) -> Result<Vec<InstalledItem>, ContentError> {
        let dir = self.domain_dir(domain);
        let Some(entries) = read_dir_if_exists(&dir)? else {
            return Ok(Vec::new());
        };
        let mut items = Vec::new();
        for entry in entries {
            let entry = entry?;
            let Some(id) = name_of(&entry).and_then(|name| ContentId::parse(&name).ok()) else {
                continue;
            };
            if !entry.file_type()?.is_dir() {
                continue;
            }
            if let Some(version) = self.versions_of(&entry.path())?.into_iter().max() {
                items.push(InstalledItem {
                    dir: entry.path().join(version.to_string()),
                    id,
                    version,
                });
            }
        }
        items.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(items)
    }

    /// 查一个 id（没装为 `None`）。
    ///
    /// # Errors
    /// 读目录失败时。
    pub fn get(
        &self,
        domain: ContentDomain,
        id: &ContentId,
    ) -> Result<Option<InstalledItem>, ContentError> {
        let dir = self.domain_dir(domain).join(id.as_str());
        let Some(version) = self.versions_of(&dir)?.into_iter().max() else {
            return Ok(None);
        };
        Ok(Some(InstalledItem {
            dir: dir.join(version.to_string()),
            id: id.clone(),
            version,
        }))
    }

    fn versions_of(&self, id_dir: &Path) -> Result<Vec<PackVersion>, ContentError> {
        let Some(entries) = read_dir_if_exists(id_dir)? else {
            return Ok(Vec::new());
        };
        let mut versions = Vec::new();
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            if let Some(version) = name_of(&entry).and_then(|name| PackVersion::parse(&name).ok()) {
                versions.push(version);
            }
        }
        Ok(versions)
    }

    /// 装一份内容：`files` 是已经校验过的（相对路径 → 字节）。先写临时目录，再整目录改名到
    /// `<id>/<版本>/`，成功后删掉这个 id 的旧版本。
    ///
    /// # Errors
    /// 已装同一或更新版本（[`ContentError::AlreadyInstalled`]），或写盘失败；失败时内容层保持原样。
    pub fn put(
        &self,
        domain: ContentDomain,
        id: &ContentId,
        version: PackVersion,
        files: &[(RelPath, Vec<u8>)],
    ) -> Result<InstalledItem, ContentError> {
        if let Some(existing) = self.get(domain, id)? {
            if existing.version >= version {
                return Err(ContentError::AlreadyInstalled {
                    id: id.clone(),
                    installed: existing.version,
                });
            }
        }
        let domain_dir = self.domain_dir(domain);
        let staging_root = domain_dir.join(STAGING);
        fs::create_dir_all(&staging_root)?;
        let staging = staging_root.join(unique_name(id, version));
        let outcome = (|| -> Result<InstalledItem, ContentError> {
            fs::create_dir_all(&staging)?;
            for (path, bytes) in files {
                let target = staging.join(path.as_str());
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(&target, bytes)?;
            }
            let id_dir = domain_dir.join(id.as_str());
            fs::create_dir_all(&id_dir)?;
            let final_dir = id_dir.join(version.to_string());
            fs::rename(&staging, &final_dir)?;
            Ok(InstalledItem {
                id: id.clone(),
                version,
                dir: final_dir,
            })
        })();
        let installed = match outcome {
            Ok(installed) => installed,
            Err(error) => {
                let _ = fs::remove_dir_all(&staging);
                return Err(error);
            }
        };
        // 新版本已经完整落位：旧版本此时才删（删失败只是多占一点空间，列出时只报最高版本）。
        for old in self.versions_of(&domain_dir.join(id.as_str()))? {
            if old != version {
                let _ = fs::remove_dir_all(domain_dir.join(id.as_str()).join(old.to_string()));
            }
        }
        Ok(installed)
    }

    /// 卸载一个 id 的全部版本。幂等：本来就没装也算成功。
    ///
    /// # Errors
    /// 删除失败（权限等）时。
    pub fn remove(&self, domain: ContentDomain, id: &ContentId) -> Result<(), ContentError> {
        match fs::remove_dir_all(self.domain_dir(domain).join(id.as_str())) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

fn read_dir_if_exists(dir: &Path) -> Result<Option<fs::ReadDir>, ContentError> {
    match fs::read_dir(dir) {
        Ok(entries) => Ok(Some(entries)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn name_of(entry: &fs::DirEntry) -> Option<String> {
    entry.file_name().into_string().ok()
}

/// 临时目录名：id、版本与时间戳，同一进程里两次安装不撞名（应用单实例，`app.lock`）。
fn unique_name(id: &ContentId, version: PackVersion) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    format!("{id}-{version}-{nanos}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: &str) -> ContentId {
        ContentId::parse(value).expect("id")
    }

    fn version(value: &str) -> PackVersion {
        PackVersion::parse(value).expect("version")
    }

    fn file(path: &str, body: &str) -> (RelPath, Vec<u8>) {
        (
            RelPath::parse(path).expect("path"),
            body.as_bytes().to_vec(),
        )
    }

    #[test]
    fn installs_lists_and_keeps_only_the_newest_version() {
        let root = tempfile::tempdir().expect("tempdir");
        let store = ContentStore::new(root.path());
        assert!(
            store.list(ContentDomain::Themes).expect("list").is_empty(),
            "域目录还不存在不算错"
        );

        let first = store
            .put(
                ContentDomain::Themes,
                &id("cozy"),
                version("1.0.0"),
                &[file("theme.toml", "v1")],
            )
            .expect("install");
        assert_eq!(
            std::fs::read_to_string(first.dir.join("theme.toml")).unwrap(),
            "v1"
        );

        let newer = store
            .put(
                ContentDomain::Themes,
                &id("cozy"),
                version("1.1.0"),
                &[file("theme.toml", "v2")],
            )
            .expect("update");
        assert!(!first.dir.exists(), "装上新版本后旧版本删掉");
        let listed = store.list(ContentDomain::Themes).expect("list");
        assert_eq!(listed, vec![newer]);
    }

    #[test]
    fn the_same_or_an_older_version_is_not_reinstalled() {
        let root = tempfile::tempdir().expect("tempdir");
        let store = ContentStore::new(root.path());
        store
            .put(
                ContentDomain::Themes,
                &id("cozy"),
                version("1.2.0"),
                &[file("theme.toml", "keep")],
            )
            .expect("install");
        for v in ["1.2.0", "1.1.9"] {
            let error = store
                .put(
                    ContentDomain::Themes,
                    &id("cozy"),
                    version(v),
                    &[file("theme.toml", "lose")],
                )
                .expect_err(v);
            assert!(
                matches!(error, ContentError::AlreadyInstalled { .. }),
                "{v}"
            );
        }
        let item = store
            .get(ContentDomain::Themes, &id("cozy"))
            .expect("get")
            .expect("installed");
        assert_eq!(
            std::fs::read_to_string(item.dir.join("theme.toml")).unwrap(),
            "keep"
        );
    }

    #[test]
    fn removing_is_idempotent_and_foreign_entries_are_ignored() {
        let root = tempfile::tempdir().expect("tempdir");
        let store = ContentStore::new(root.path());
        store
            .put(
                ContentDomain::Themes,
                &id("cozy"),
                version("1.0.0"),
                &[file("theme.toml", "x")],
            )
            .expect("install");
        // 不是我们写的东西（非法 id、散落的文件、临时目录残留）不出现在清单里。
        let themes = root.path().join("themes");
        std::fs::create_dir_all(themes.join("Not An Id/1.0.0")).unwrap();
        std::fs::write(themes.join("stray.txt"), "x").unwrap();
        std::fs::create_dir_all(themes.join(".staging/leftover")).unwrap();
        std::fs::create_dir_all(themes.join("cozy/not-a-version")).unwrap();
        let listed = store.list(ContentDomain::Themes).expect("list");
        assert_eq!(
            listed
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["cozy"]
        );

        store
            .remove(ContentDomain::Themes, &id("cozy"))
            .expect("remove");
        store
            .remove(ContentDomain::Themes, &id("cozy"))
            .expect("remove again");
        assert!(store.list(ContentDomain::Themes).expect("list").is_empty());
    }

    #[test]
    fn nested_files_land_under_the_version_directory() {
        let root = tempfile::tempdir().expect("tempdir");
        let store = ContentStore::new(root.path());
        let item = store
            .put(
                ContentDomain::Themes,
                &id("kit"),
                version("0.1.0"),
                &[file("a/b/c.md", "deep"), file("manifest.toml", "m")],
            )
            .expect("install");
        assert_eq!(
            std::fs::read_to_string(item.dir.join("a/b/c.md")).unwrap(),
            "deep"
        );
        assert!(
            !root
                .path()
                .join("themes/.staging")
                .read_dir()
                .unwrap()
                .any(|_| true),
            "临时目录用完即空"
        );
    }
}
