//! iCloud 未下载占位（TDD-06 §4.1「同步盘痕迹」②；笔记库对标轮 V3c）。
//!
//! macOS 13 及更早，开着「优化 Mac 存储空间」时，iCloud 把没下载到本机的文件换成一个点开头的
//! 占位：`周报.md` → `.周报.md.icloud`。点文件一律被当成隐藏文件跳过，于是笔记看起来「消失」、
//! 指向它的链接被判成断链。这里认出占位：树上显示成「未下载」，链接解析把它当作在，打开它时
//! 说清楚并请系统下载。
//!
//! macOS 14 起 iCloud 不再生成占位：未下载的文件保留原名、只打 `SF_DATALESS` 标记，读取时由
//! 系统现场取回。笔记正文按需读、慢一点无妨；但打开库要读 `.luna/` 里的设置与索引，取回一旦
//! 卡住（磁盘紧张时 iCloud 会把整个库移出本机、下载进度停在 0%），读就一直不返回——这一段见
//! [`wait_until_local`]。最低支持 10.15，所以旧式占位仍要认。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// 文件被 iCloud「移到云端」（macOS 14 起保留原名、只打 `SF_DATALESS` 标记）。只看元数据，
/// 不读内容，所以本身不会触发下载、也不会卡住。
#[must_use]
pub fn is_dataless(path: &Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        use std::os::macos::fs::MetadataExt;
        /// `<sys/stat.h>`：内容不在本机，读取时由文件提供者取回。
        const SF_DATALESS: u32 = 0x4000_0000;
        std::fs::symlink_metadata(path).is_ok_and(|meta| meta.st_flags() & SF_DATALESS != 0)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        false
    }
}

/// 等目录第一层的文件都回到本机，最多等 `timeout`；超时返回 `false`，调用方这一次就不该去读它们。
///
/// 打开库（启动装载与切库）在主线程上同步读 `.luna/` 里的设置、索引与续跑档：文件在云端时读会
/// 一直阻塞到 iCloud 取回为止，取回卡住窗口就永远出不来。这里先只看标记；在云端的文件交给后台
/// 线程去读（读一个字节就会让系统取回整个文件，线程卡住也只卡它自己），主线程只轮询标记。
/// 同一个文件已在取回中就不再另起线程，「重试」不会越点越多。
#[must_use]
pub fn wait_until_local(dir: &Path, timeout: Duration) -> bool {
    wait_until_local_with(dir, timeout, is_dataless, fetch_in_background)
}

fn wait_until_local_with(
    dir: &Path,
    timeout: Duration,
    in_cloud: impl Fn(&Path) -> bool,
    fetch: impl Fn(PathBuf),
) -> bool {
    let pending = || files_in_cloud(dir, &in_cloud);
    let first = pending();
    if first.is_empty() {
        return true;
    }
    first.into_iter().for_each(&fetch);
    let deadline = Instant::now() + timeout;
    loop {
        if pending().is_empty() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// 在后台一直等到目录第一层的文件都回到本机，然后调一次 `on_local`。
///
/// [`wait_until_local`] 只等几秒；没等到时库停在空状态「还在从 iCloud 下载」。取回可能很慢、甚至
/// 卡住很久（磁盘紧张时 iCloud 下载会停在 0%），让用户自己估时机反复点「重试」不合理：这里不设
/// 上限地等，回来了由调用方自动打开库。每隔 `poll` 只看一次标记，并把仍在云端的文件再交一次取回
/// （同一个文件已在取回中不会另起线程）。同一个目录已经有人在等就不再另起。
pub fn notify_when_local(dir: PathBuf, poll: Duration, on_local: impl FnOnce() + Send + 'static) {
    static WATCHING: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());
    let Ok(mut watching) = WATCHING.lock() else {
        return;
    };
    if watching.contains(&dir) {
        return;
    }
    watching.push(dir.clone());
    drop(watching);
    let spawned = std::thread::Builder::new()
        .name("icloud-watch".to_owned())
        .spawn(move || {
            poll_until_local(&dir, poll, is_dataless, fetch_in_background);
            if let Ok(mut watching) = WATCHING.lock() {
                watching.retain(|d| d != &dir);
            }
            on_local();
        });
    if spawned.is_err() {
        eprintln!("[vault] 起 iCloud 等待线程失败");
    }
}

fn poll_until_local(
    dir: &Path,
    poll: Duration,
    in_cloud: impl Fn(&Path) -> bool,
    fetch: impl Fn(PathBuf),
) {
    loop {
        let pending = files_in_cloud(dir, &in_cloud);
        if pending.is_empty() {
            return;
        }
        pending.into_iter().for_each(&fetch);
        std::thread::sleep(poll);
    }
}

fn files_in_cloud(dir: &Path, in_cloud: impl Fn(&Path) -> bool) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_ok_and(|t| t.is_file()))
                .map(|entry| entry.path())
                .filter(|path| in_cloud(path))
                .collect()
        })
        .unwrap_or_default()
}

fn fetch_in_background(path: PathBuf) {
    use std::io::Read;
    static IN_FLIGHT: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());
    let Ok(mut in_flight) = IN_FLIGHT.lock() else {
        return;
    };
    if in_flight.contains(&path) {
        return;
    }
    in_flight.push(path.clone());
    drop(in_flight);
    let spawned = std::thread::Builder::new()
        .name("icloud-fetch".to_owned())
        .spawn(move || {
            if let Ok(mut file) = std::fs::File::open(&path) {
                let _ = file.read(&mut [0_u8; 1]);
            }
            if let Ok(mut in_flight) = IN_FLIGHT.lock() {
                in_flight.retain(|p| p != &path);
            }
        });
    if spawned.is_err() {
        eprintln!("[vault] 起 iCloud 取回线程失败");
    }
}

/// `.名字.icloud` → `名字`（不是占位返回 `None`）。
#[must_use]
pub fn placeholder_real_name(file_name: &str) -> Option<&str> {
    let inner = file_name.strip_prefix('.')?.strip_suffix(".icloud")?;
    (!inner.is_empty() && !inner.starts_with('.')).then_some(inner)
}

/// 一个库相对路径对应的占位文件（绝对路径）。
#[must_use]
pub fn placeholder_for(root: &Path, rel: &str) -> std::path::PathBuf {
    let (dir, name) = match rel.rfind('/') {
        Some(at) => (&rel[..at], &rel[at + 1..]),
        None => ("", rel),
    };
    let placeholder = format!(".{name}.icloud");
    if dir.is_empty() {
        root.join(placeholder)
    } else {
        root.join(dir).join(placeholder)
    }
}

impl crate::Vault {
    /// 这个路径的文件不在本机、但 iCloud 占位在（还没下载）。
    #[must_use]
    pub fn is_cloud_placeholder(&self, rel: &str) -> bool {
        !self.root().join(rel).exists() && placeholder_for(self.root(), rel).is_file()
    }

    /// 最近一次看到的全部占位（库相对的真名路径）：链接解析把它们当作在。由建树与启动扫描
    /// 顺手刷新，不为它单独遍历全库。
    ///
    /// 同步里只在云端的文件（ADR-156）同理算在：指向一段按需录音的 `![[…]]` 不是断链。
    pub(crate) fn cloud_placeholders(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .placeholders
            .lock()
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default();
        out.extend(self.sync_cloud_only_paths());
        out
    }

    /// 同步里只在云端、此刻本机确实没有的文件。
    pub(crate) fn sync_cloud_only_paths(&self) -> Vec<String> {
        self.sync_cloud_only
            .lock()
            .map(|set| {
                set.iter()
                    .filter(|rel| !self.root().join(rel).exists())
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 换上同步服务给的仅云端清单（ADR-156），返回进出清单的路径（文件树据此刷新）。
    pub fn set_sync_cloud_only(&self, paths: impl IntoIterator<Item = String>) -> Vec<String> {
        let next: std::collections::HashSet<String> = paths.into_iter().collect();
        let Ok(mut set) = self.sync_cloud_only.lock() else {
            return Vec::new();
        };
        let changed = set.symmetric_difference(&next).cloned().collect();
        *set = next;
        changed
    }

    /// 这个路径是同步里只在云端、本机没有的文件。
    #[must_use]
    pub fn is_sync_cloud_only(&self, rel: &str) -> bool {
        !self.root().join(rel).exists()
            && self
                .sync_cloud_only
                .lock()
                .is_ok_and(|set| set.contains(rel))
    }

    /// 把仅云端的文件按原位置挂到树上（所在目录本机不存在就补出来）。
    pub(crate) fn insert_sync_cloud_only(&self, tree: &mut crate::vault::NoteTree) {
        for rel in self.sync_cloud_only_paths() {
            let parts: Vec<&str> = rel.split('/').collect();
            let Some((name, dirs)) = parts.split_last() else {
                continue;
            };
            let mut node = &mut *tree;
            let mut prefix = String::new();
            for dir in dirs {
                prefix = if prefix.is_empty() {
                    (*dir).to_owned()
                } else {
                    format!("{prefix}/{dir}")
                };
                let at = match node
                    .children
                    .iter()
                    .position(|c| c.is_dir && c.name == *dir)
                {
                    Some(at) => at,
                    None => {
                        node.children
                            .push(crate::vault::NoteTree::virtual_dir(dir, &prefix));
                        node.children.len() - 1
                    }
                };
                node = &mut node.children[at];
            }
            if node.children.iter().any(|c| c.path == rel) {
                continue;
            }
            node.children
                .push(crate::vault::NoteTree::sync_cloud_only_file(name, &rel));
        }
    }

    /// 换上新看到的占位清单（全库遍历的结果）。
    pub fn set_cloud_placeholders(&self, paths: impl IntoIterator<Item = String>) {
        if let Ok(mut set) = self.placeholders.lock() {
            *set = paths.into_iter().collect();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 同步里只在云端的录音（ADR-156）：按原位置挂上树（目录本机还不存在也补出来）、标成仅云端；
    /// 指向它的嵌入不是断链、它不是孤儿；本机有了真文件之后不再重复列出。
    #[test]
    fn a_sync_cloud_only_file_is_listed_and_resolved_until_it_lands() {
        use crate::Vault;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let vault = Vault::open(root).unwrap();
        vault.write("录音/会议.md", "![[s-p01.wav]]").unwrap();
        assert_eq!(
            vault.set_sync_cloud_only(["录音/assets/s-p01.wav".to_owned()]),
            vec!["录音/assets/s-p01.wav".to_owned()]
        );

        let tree = vault.tree().unwrap();
        let rec = tree.children.iter().find(|c| c.name == "录音").unwrap();
        let assets = rec.children.iter().find(|c| c.name == "assets").unwrap();
        assert!(assets.is_dir);
        let file = &assets.children[0];
        assert_eq!(
            (
                file.path.as_str(),
                file.sync_cloud_only,
                file.cloud_placeholder
            ),
            ("录音/assets/s-p01.wav", true, false)
        );
        assert!(vault
            .walk_files()
            .unwrap()
            .contains(&"录音/assets/s-p01.wav".to_owned()));
        assert!(vault.orphan_assets().unwrap().is_empty());
        assert!(vault.is_sync_cloud_only("录音/assets/s-p01.wav"));

        std::fs::create_dir_all(root.join("录音/assets")).unwrap();
        std::fs::write(root.join("录音/assets/s-p01.wav"), b"RIFF").unwrap();
        assert!(!vault.is_sync_cloud_only("录音/assets/s-p01.wav"));
        let tree = vault.tree().unwrap();
        let rec = tree.children.iter().find(|c| c.name == "录音").unwrap();
        let assets = rec.children.iter().find(|c| c.name == "assets").unwrap();
        assert_eq!(assets.children.len(), 1);
        assert!(!assets.children[0].sync_cloud_only);
    }

    /// 在云端的文件交出去取回、取回到了就放行；没有在云端的文件时不等、也不取。
    #[test]
    fn waits_for_cloud_files_until_they_are_fetched() {
        use std::sync::Mutex;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("settings.toml"), "").unwrap();
        std::fs::write(dir.path().join("index.sqlite"), "").unwrap();
        std::fs::create_dir(dir.path().join("sidecars")).unwrap();
        let in_cloud = Mutex::new(vec![dir.path().join("settings.toml")]);
        let fetched = Mutex::new(Vec::new());
        let local = wait_until_local_with(
            dir.path(),
            Duration::from_secs(5),
            |p| in_cloud.lock().unwrap().contains(&p.to_path_buf()),
            |p| {
                fetched.lock().unwrap().push(p.clone());
                in_cloud.lock().unwrap().retain(|c| c != &p);
            },
        );
        assert!(local);
        assert_eq!(
            *fetched.lock().unwrap(),
            vec![dir.path().join("settings.toml")]
        );

        let untouched = Mutex::new(0);
        assert!(wait_until_local_with(
            dir.path(),
            Duration::from_secs(5),
            |_| false,
            |_| *untouched.lock().unwrap() += 1,
        ));
        assert_eq!(*untouched.lock().unwrap(), 0);
    }

    /// 取回一直不来（iCloud 下载停在 0%）：到点就返回「还在云端」，不无限等。
    #[test]
    fn gives_up_after_the_timeout_when_the_fetch_never_lands() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("settings.toml"), "").unwrap();
        let started = Instant::now();
        let local = wait_until_local_with(dir.path(), Duration::from_millis(300), |_| true, |_| {});
        assert!(!local);
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    /// 后台等待不设上限：取回一直没到就接着等、每一轮把仍在云端的文件再交一次取回，到了才返回。
    #[test]
    fn polls_until_the_cloud_files_land_and_asks_again_each_round() {
        use std::sync::Mutex;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("settings.toml"), "").unwrap();
        let rounds = Mutex::new(0_u32);
        poll_until_local(
            dir.path(),
            Duration::from_millis(1),
            |_| *rounds.lock().unwrap() < 3,
            |_| *rounds.lock().unwrap() += 1,
        );
        assert_eq!(*rounds.lock().unwrap(), 3);
    }

    /// 目录已经都在本机：后台线程第一轮就调回调。
    #[test]
    fn notifies_once_when_the_directory_is_already_local() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("settings.toml"), "").unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        notify_when_local(
            dir.path().to_path_buf(),
            Duration::from_millis(1),
            move || {
                tx.send(()).unwrap();
            },
        );
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok());
    }

    /// 普通文件不是「移到云端」的文件；不存在的路径也不是。
    #[test]
    fn a_plain_local_file_is_not_dataless() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.md");
        std::fs::write(&file, "x").unwrap();
        assert!(!is_dataless(&file));
        assert!(!is_dataless(&dir.path().join("missing.md")));
        assert!(wait_until_local(dir.path(), Duration::from_millis(10)));
    }

    #[test]
    fn only_dot_name_dot_icloud_is_a_placeholder() {
        assert_eq!(placeholder_real_name(".周报.md.icloud"), Some("周报.md"));
        assert_eq!(placeholder_real_name(".图.png.icloud"), Some("图.png"));
        assert_eq!(placeholder_real_name("周报.md.icloud"), None);
        assert_eq!(placeholder_real_name(".icloud"), None);
        assert_eq!(placeholder_real_name("..icloud"), None);
        assert_eq!(placeholder_real_name(".DS_Store"), None);
    }

    #[test]
    fn placeholder_sits_next_to_where_the_file_would_be() {
        let root = Path::new("/库");
        assert_eq!(
            placeholder_for(root, "项目/周报.md"),
            Path::new("/库/项目/.周报.md.icloud")
        );
        assert_eq!(
            placeholder_for(root, "周报.md"),
            Path::new("/库/.周报.md.icloud")
        );
    }

    /// 占位在树上按真名出现、标成未下载；指向它的 `[[名字]]` 与 `![[图.png]]` 不算断链；
    /// 打开它说「还在 iCloud 上」而不是「找不到」；真文件下载回来后不再重复列出。
    #[test]
    fn a_placeholder_is_listed_resolved_and_explained() {
        use crate::Vault;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let vault = Vault::open(root).unwrap();
        std::fs::create_dir_all(root.join("项目")).unwrap();
        std::fs::write(root.join("项目/.周报.md.icloud"), b"bplist").unwrap();
        std::fs::write(root.join("项目/.图.png.icloud"), b"bplist").unwrap();
        vault.write("引用.md", "[[周报]] ![[图.png]]").unwrap();

        let tree = vault.tree().unwrap();
        let project = tree.children.iter().find(|c| c.name == "项目").unwrap();
        let listed: Vec<(&str, bool)> = project
            .children
            .iter()
            .map(|c| (c.path.as_str(), c.cloud_placeholder))
            .collect();
        assert_eq!(listed, vec![("项目/周报.md", true), ("项目/图.png", true)]);

        let resolved = vault
            .resolve_wikilinks("引用.md", &["周报".to_owned()])
            .unwrap();
        assert_eq!(resolved, vec![Some("项目/周报.md".to_owned())]);
        assert!(
            vault.orphan_assets().unwrap().is_empty(),
            "没下载的图不是孤儿"
        );

        assert!(matches!(
            vault.read("项目/周报.md"),
            Err(crate::VaultError::CloudPlaceholder(_))
        ));
        assert!(matches!(
            vault.read("项目/不存在.md"),
            Err(crate::VaultError::NotFound(_))
        ));

        std::fs::write(root.join("项目/周报.md"), "下载回来了").unwrap();
        let tree = vault.tree().unwrap();
        let project = tree.children.iter().find(|c| c.name == "项目").unwrap();
        let weekly: Vec<_> = project
            .children
            .iter()
            .filter(|c| c.name == "周报.md")
            .collect();
        assert_eq!(weekly.len(), 1);
        assert!(!weekly[0].cloud_placeholder);
    }
}
