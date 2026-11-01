//! 库注册表（PRD-06-049 / ADR-025）：这台设备知道哪些笔记库。
//!
//! 放在 `luna-vault` 是为了应用与 CLI 读同一份清单（ADR-141 ④：`--vault` 认名字或 id）。
//!
//! **是配置数据，不是设置项**——刻意不进 `luna-config` 的 settings registry：
//! 「重置应用 / 恢复默认」不该清掉用户的库清单（丢清单 ≈ 丢「我的库在哪」这份记忆），
//! 而 registry 里的一切按定义都是可被重置的值。落 `<app_config_dir>/vaults.json`，
//! 与 `config.toml` 同级。
//!
//! **清单随设备、库的内容随库**（与 Obsidian 全局 vault 列表同构）：路径 / 显示名 /
//! 最后打开时间是"这台机器上的记忆"，而每个库自己的索引、设置、快照全在它的 `.luna/` 里。
//!
//! **库列表跟着账户走**（ADR-151 修订 2026-10-15「续二」；TDD-06 §12）：每条记录带「列在哪些
//! 账户的列表里」（`accounts`，账户键而非邮箱）。没列在任何账户里的是**本机库**，对每个账户、
//! 对未登录都可见；其余只在「列表账户」在它的集合里时可见。列表账户与每个账户上次打开的库是
//! 设备级的另一份记忆（`vault-lists.json`）：已登录 = 登录的账户；退出后仍是退出前那一个
//! （退出不是切换）；从没登录过 = 没有，只看得到本机库。账户键怎么算、谁登录着，由应用层给出，
//! 这里只当不透明字符串。
//!
//! 隐私：清单含本机绝对路径，与 `config.toml` 记 `storage.vault_root` 同一档次；
//! 不经 IPC 外泄以外的通道，日志不打印完整路径。列表账户只记账户键与打码名，不记邮箱。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

/// 注册表文件名（设备级配置目录下）。
pub const REGISTRY_FILENAME: &str = "vaults.json";

/// 列表账户与各账户上次打开的库（设备级配置目录下）。
pub const LISTS_FILENAME: &str = "vault-lists.json";

/// 一条已知库记录。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultEntry {
    /// 库根绝对路径（唯一键）。
    pub path: PathBuf,
    /// 显示名。**只是清单里的别名**——改名绝不动磁盘上的目录名（PRD-06-049）。
    pub name: String,
    /// 最后打开时间（Unix 毫秒；用于列表排序，`0` = 从未经本应用打开）。
    pub last_opened_ms: i64,
    /// 库身份（`.luna/vault-id`，ADR-140 ②），打开时登记。`None` = 登记了但还没被本版本
    /// 打开过——身份写在库里，不打开就不知道，这是真实状态而非过渡字段。
    #[serde(default)]
    pub id: Option<String>,
    /// 列在哪些账户的库列表里（账户键）。空 = 本机库：不属于任何账户，谁都看得见——这是一种
    /// 真实状态（没开同步、也没被归到账户的库），不是过渡字段；没有这一列的清单读出来就是本机库。
    #[serde(default)]
    pub accounts: Vec<String>,
}

impl VaultEntry {
    /// 本机库：不在任何账户的列表里。
    #[must_use]
    pub fn is_device(&self) -> bool {
        self.accounts.is_empty()
    }

    /// 对列表账户 `list_account` 可见（[`visible_to`] 的成员写法）。
    #[must_use]
    pub fn visible_to(&self, list_account: Option<&str>) -> bool {
        visible_to(self, list_account)
    }

    fn lists(&self, account: &str) -> bool {
        self.accounts.iter().any(|a| a == account)
    }
}

/// 可见性的唯一判据：本机库对谁都可见；其余只在列表账户在它的集合里时可见。没有列表账户（从没
/// 登录过）= 只看得到本机库。
#[must_use]
pub fn visible_to(entry: &VaultEntry, list_account: Option<&str>) -> bool {
    entry.is_device() || list_account.is_some_and(|account| entry.lists(account))
}

/// 列表账户：此刻登录的账户；退出后是退出前那一个。`label` 是打码的账户名（只给界面说
/// 「谁的库」），不是邮箱。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListAccount {
    pub key: String,
    pub label: String,
}

/// `vault-lists.json` 的内容。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Lists {
    list_account: Option<ListAccount>,
    /// 账户键 → 那个账户在这台设备上最后打开的库（只在登录时记）。
    last_opened: BTreeMap<String, PathBuf>,
}

/// 「管理库」移除一条之后它还在不在清单里。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Forgotten {
    /// 整条删掉了（本机库；或最后一个列着它的账户也移除了它）。
    Dropped,
    /// 只从这个账户的列表里拿掉了，别的账户的列表里还有它。
    Kept,
    /// 清单里本来就没有这一条。
    Absent,
}

/// 注册一个库时可能的拒绝理由。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegisterRejection {
    /// 与已知库存在**包含关系**（父或子）。
    ///
    /// 拒绝而非容忍：两个库的文件监听与派生索引会重叠——子库的笔记会被父库当成普通笔记
    /// 索引进图谱与搜索，父库的一次「重建索引」还会连子库一起扫。这不是能靠提示绕过的
    /// 使用不当，是两套事实源指向同一批文件。
    Nested { existing: PathBuf },
}

/// 两个库路径是否**重叠**（相等或互为祖先）——嵌套注册的唯一判据。
///
/// 按**路径段**比较而非字符串前缀：`~/V/work` 不该被 `~/V/work-old` 判为嵌套
/// （与 `crate::scope_covers` 同一教训）。
#[must_use]
pub fn paths_overlap(a: &Path, b: &Path) -> bool {
    a == b || a.starts_with(b) || b.starts_with(a)
}

/// 由库路径推一个默认显示名（目录名；取不到则用完整路径）。
#[must_use]
pub fn default_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// 把一条记录并入清单：**已在清单则不重复添加**（返回既有记录的位置），
/// 与已知库重叠则拒绝。新登记的一律是本机库。
///
/// "已注册就直接用"是刻意的：用户从「打开文件夹为库」选中一个已在清单的目录，
/// 他的意图显然是"去那个库"，弹一句"已存在"再让他自己去列表里点一次是多余的。
/// 判重看全部记录而不只是看得见的：另一个账户的库也是这台设备上的同一个文件夹。
pub fn upsert(
    entries: &mut Vec<VaultEntry>,
    path: &Path,
    name: Option<String>,
) -> Result<(), RegisterRejection> {
    if let Some(existing) = entries.iter_mut().find(|e| e.path == path) {
        if let Some(name) = name {
            existing.name = name;
        }
        return Ok(());
    }
    if let Some(overlap) = entries.iter().find(|e| paths_overlap(&e.path, path)) {
        return Err(RegisterRejection::Nested {
            existing: overlap.path.clone(),
        });
    }
    entries.push(VaultEntry {
        path: path.to_path_buf(),
        name: name.unwrap_or_else(|| default_name(path)),
        last_opened_ms: 0,
        id: None,
        accounts: Vec::new(),
    });
    Ok(())
}

/// 清单呈现序：**最近打开在前**，同刻按显示名——用户找的多半是刚用过的那个。
pub fn sort_for_display(entries: &mut [VaultEntry]) {
    entries.sort_by(|a, b| {
        b.last_opened_ms
            .cmp(&a.last_opened_ms)
            .then_with(|| a.name.cmp(&b.name))
    });
}

/// 清单变了之后调的钩子（应用层据此向界面发一次 `vaults://changed`）。
type ChangeHook = Box<dyn Fn() + Send + Sync>;

/// 设备级注册表句柄（持存档路径；`None` = 配置目录不可用，退化为内存态不落盘）。
///
/// 每一次改写都在同一把锁里读、改、写（应用里几条路径会同时改它：开库登记、同步开关、账户
/// 切换），写完调一次变更钩子——「清单变了」只在这一处广播，任何一条改写路径都漏不掉。
pub struct VaultRegistry {
    path: Option<PathBuf>,
    lists_path: Option<PathBuf>,
    gate: Mutex<()>,
    on_change: OnceLock<ChangeHook>,
}

impl VaultRegistry {
    /// 由配置目录构造（`config.toml` 的同级目录）。
    #[must_use]
    pub fn new(config_dir: Option<PathBuf>) -> Self {
        Self {
            path: config_dir.as_ref().map(|dir| dir.join(REGISTRY_FILENAME)),
            lists_path: config_dir.map(|dir| dir.join(LISTS_FILENAME)),
            gate: Mutex::new(()),
            on_change: OnceLock::new(),
        }
    }

    /// 装上变更钩子（只装一次；应用启动时）。
    pub fn set_on_change(&self, hook: impl Fn() + Send + Sync + 'static) {
        let _ = self.on_change.set(Box::new(hook));
    }

    fn changed(&self) {
        if let Some(hook) = self.on_change.get() {
            hook();
        }
    }

    /// 同一时刻只有一个改写者。锁中毒（某次改写中途 panic）不影响下一次：文件本身是整份覆写的。
    fn lock(&self) -> std::sync::MutexGuard<'_, ()> {
        self.gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn read_entries(&self) -> Vec<VaultEntry> {
        let mut entries = self
            .path
            .as_deref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|bytes| serde_json::from_slice::<Vec<VaultEntry>>(&bytes).ok())
            .unwrap_or_default();
        sort_for_display(&mut entries);
        entries
    }

    fn read_lists(&self) -> Lists {
        self.lists_path
            .as_deref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|bytes| serde_json::from_slice::<Lists>(&bytes).ok())
            .unwrap_or_default()
    }

    fn write_json(path: Option<&Path>, value: &impl Serialize) {
        let Some(path) = path else {
            return;
        };
        let Ok(json) = serde_json::to_string_pretty(value) else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(path, json) {
            // 脱敏：只报失败原因，不打印完整路径（TDD-12）。
            eprintln!("[vaults] 库清单写入失败（仅本次会话生效）：{e}");
        }
    }

    fn save(&self, entries: &[VaultEntry]) {
        Self::write_json(self.path.as_deref(), &entries);
        self.changed();
    }

    fn save_lists(&self, lists: &Lists) {
        Self::write_json(self.lists_path.as_deref(), lists);
        self.changed();
    }

    /// 读全部记录（已按呈现序排好），不论对谁可见——判重、判嵌套、认身份用它。文件缺失/损坏 →
    /// 空清单，绝不 panic、绝不阻塞启动：清单是便利记录，坏了最多是"列表空了"，用户重新打开
    /// 文件夹即可恢复。
    #[must_use]
    pub fn list(&self) -> Vec<VaultEntry> {
        let _gate = self.lock();
        self.read_entries()
    }

    /// 此刻的列表账户（`None` = 从没登录过）。
    #[must_use]
    pub fn list_account(&self) -> Option<ListAccount> {
        let _gate = self.lock();
        self.read_lists().list_account
    }

    /// 列表账户看得见的记录（切换菜单、「管理库」、同步列表的来源）。
    #[must_use]
    pub fn visible(&self) -> Vec<VaultEntry> {
        let _gate = self.lock();
        let account = self.read_lists().list_account.map(|a| a.key);
        self.read_entries()
            .into_iter()
            .filter(|entry| entry.visible_to(account.as_deref()))
            .collect()
    }

    /// 注册（或确认已注册）一个库并落盘。新登记的是本机库。
    pub fn register(&self, path: &Path, name: Option<String>) -> Result<(), RegisterRejection> {
        let _gate = self.lock();
        let mut entries = self.read_entries();
        upsert(&mut entries, path, name)?;
        self.save(&entries);
        Ok(())
    }

    /// 从清单整条删掉一条记录（撤销一次没做完的登记用）。**只删记录，绝不碰磁盘上的库**。
    pub fn remove(&self, path: &Path) {
        let _gate = self.lock();
        let mut entries = self.read_entries();
        entries.retain(|e| e.path != path);
        self.save(&entries);
    }

    /// 「管理库」移除（PRD-06-049）：本机库整条删掉；账户的库只从 `account` 的列表里拿掉，别的
    /// 账户的列表不动，最后一个列着它的账户也拿掉时整条删掉。**只动记录，绝不碰磁盘上的库**。
    pub fn forget(&self, path: &Path, account: Option<&str>) -> Forgotten {
        let _gate = self.lock();
        let mut entries = self.read_entries();
        let Some(index) = entries.iter().position(|e| e.path == path) else {
            return Forgotten::Absent;
        };
        let outcome = forget_outcome(&entries[index], account);
        match (outcome, account) {
            (Forgotten::Kept, Some(account)) => entries[index].accounts.retain(|a| a != account),
            (Forgotten::Kept, None) => return outcome,
            _ => {
                entries.remove(index);
            }
        }
        self.save(&entries);
        outcome
    }

    /// 改显示名（只改别名，不动目录名）。
    pub fn rename(&self, path: &Path, name: String) {
        let _gate = self.lock();
        let mut entries = self.read_entries();
        if let Some(entry) = entries.iter_mut().find(|e| e.path == path) {
            entry.name = name;
            self.save(&entries);
        }
    }

    /// 标记为"刚打开"并登记身份（顺带确保它在清单里——从启动指针进来的库可能还没被注册过）。
    ///
    /// `account` 是此刻登录的账户（未登录 = `None`，不记任何账户的事）：记下它上次打开的是这个库；
    /// 这个库已经是账户的库（另一个账户的也算——「打开文件夹为库」把它重新加回来）就把它列进这个
    /// 账户的列表。本机库照旧是本机库：打开它不改变谁看得见它。
    pub fn touch(&self, path: &Path, id: crate::VaultId, now_ms: i64, account: Option<&str>) {
        let _gate = self.lock();
        let mut entries = self.read_entries();
        if upsert(&mut entries, path, None).is_err() {
            return; // 与已知库嵌套：不静默把它并进清单（判据同注册路径）。
        }
        if let Some(entry) = entries.iter_mut().find(|e| e.path == path) {
            entry.last_opened_ms = now_ms;
            entry.id = Some(id.to_string());
            if let Some(account) = account {
                if !entry.is_device() && !entry.lists(account) {
                    entry.accounts.push(account.to_owned());
                }
            }
        }
        self.save(&entries);
        if let Some(account) = account {
            self.remember_locked(account, path);
        }
    }

    /// 记下 `account` 上次在这台设备上打开的是这个库（启动装载不经开库登记，对齐账户时补记）。
    pub fn remember_opened(&self, account: &str, path: &Path) {
        let _gate = self.lock();
        self.remember_locked(account, path);
    }

    fn remember_locked(&self, account: &str, path: &Path) {
        let mut lists = self.read_lists();
        if lists.last_opened.get(account).map(PathBuf::as_path) != Some(path) {
            lists
                .last_opened
                .insert(account.to_owned(), path.to_path_buf());
            self.save_lists(&lists);
        }
    }

    /// 把一个库列进 `account` 的列表（开启同步、加入云端库、下载到本机：绑定归谁就列进谁的）。
    /// 返回是否真的改了。
    pub fn add_account(&self, path: &Path, account: &str) -> bool {
        let _gate = self.lock();
        let mut entries = self.read_entries();
        let Some(entry) = entries.iter_mut().find(|e| e.path == path) else {
            return false;
        };
        if entry.lists(account) {
            return false;
        }
        entry.accounts.push(account.to_owned());
        self.save(&entries);
        true
    }

    /// 让一个库回到本机库（停止同步：它不再属于任何账户，谁都看得见）。
    pub fn make_device(&self, path: &Path) {
        let _gate = self.lock();
        let mut entries = self.read_entries();
        if let Some(entry) = entries.iter_mut().find(|e| e.path == path) {
            entry.accounts.clear();
        }
        // 没变也照样落一次并广播：绑定变了，界面要按新的同步状态重取。
        self.save(&entries);
    }

    /// 换列表账户（有账户登录了）。返回列表账户是否换了人——同一账户再登录不算（打码名变了就
    /// 顺手改掉）。
    pub fn set_list_account(&self, key: &str, label: &str) -> bool {
        let _gate = self.lock();
        let mut lists = self.read_lists();
        let switched = lists.list_account.as_ref().map(|a| a.key.as_str()) != Some(key);
        let next = ListAccount {
            key: key.to_owned(),
            label: label.to_owned(),
        };
        if lists.list_account.as_ref() != Some(&next) {
            lists.list_account = Some(next);
            self.save_lists(&lists);
        }
        switched
    }

    /// `account` 上次在这台设备上打开的库（可能已经看不见或不在了，由调用方判）。
    #[must_use]
    pub fn last_opened(&self, account: &str) -> Option<PathBuf> {
        let _gate = self.lock();
        self.read_lists().last_opened.remove(account)
    }

    /// 这个身份是否被清单里**另一份仍在的库**持有（ADR-140 ②：用户复制了整个库）。
    ///
    /// 「仍在」按那一份磁盘上的 `.luna/vault-id` 现读判定，不只看清单：清单里的旧条目可能
    /// 早就被移走或删掉（移动库后旧路径条目还在），只凭清单会让被移动的库每次打开都换身份、
    /// 布局永远找不回来。
    #[must_use]
    pub fn id_held_elsewhere(&self, path: &Path, id: crate::VaultId) -> bool {
        let id_text = id.to_string();
        self.list().iter().any(|entry| {
            entry.path != path
                && entry.id.as_deref() == Some(id_text.as_str())
                && std::fs::read_to_string(
                    entry
                        .path
                        .join(".luna")
                        .join(crate::identity::VAULT_ID_FILE),
                )
                .ok()
                .and_then(|text| crate::VaultId::parse(&text))
                    == Some(id)
        })
    }
}

/// 「管理库」移除一条之后它还在不在（纯函数）：本机库 → 整条删；账户的库 → 拿掉 `account`，
/// 没有账户再列着它时整条删。`account` 为空（从没登录过）时看得见的只有本机库。
#[must_use]
pub fn forget_outcome(entry: &VaultEntry, account: Option<&str>) -> Forgotten {
    if entry.is_device() || entry.accounts.iter().all(|a| Some(a.as_str()) == account) {
        Forgotten::Dropped
    } else {
        Forgotten::Kept
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlap_compares_path_segments_not_string_prefix() {
        // `work-old` 不是 `work` 的子目录——按字符串前缀判会把它误拒。
        assert!(!paths_overlap(
            Path::new("/v/work"),
            Path::new("/v/work-old")
        ));
        assert!(paths_overlap(Path::new("/v/work"), Path::new("/v/work")));
        assert!(paths_overlap(Path::new("/v"), Path::new("/v/work")));
        assert!(paths_overlap(Path::new("/v/work/sub"), Path::new("/v")));
    }

    #[test]
    fn nested_registration_is_rejected_naming_the_blocking_vault() {
        let mut entries = Vec::new();
        upsert(&mut entries, Path::new("/v/work"), None).unwrap();
        // 子目录：两库的监听与索引会重叠，故拒绝并指名是哪一个挡的。
        let err = upsert(&mut entries, Path::new("/v/work/inner"), None).unwrap_err();
        assert_eq!(
            err,
            RegisterRejection::Nested {
                existing: PathBuf::from("/v/work")
            }
        );
        // 父目录同理（反向包含）。
        assert!(upsert(&mut entries, Path::new("/v"), None).is_err());
        // 平级不受影响。
        assert!(upsert(&mut entries, Path::new("/v/study"), None).is_ok());
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn registering_a_known_path_is_idempotent_not_an_error() {
        // 「打开文件夹为库」选中已在清单的目录 = "去那个库"，不该报错也不该重复添加。
        let mut entries = Vec::new();
        upsert(&mut entries, Path::new("/v/work"), Some("工作库".into())).unwrap();
        upsert(&mut entries, Path::new("/v/work"), None).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "工作库", "重复注册不覆盖已有显示名");
    }

    #[test]
    fn display_order_puts_recently_opened_first() {
        let mut entries = vec![
            VaultEntry {
                path: "/v/a".into(),
                name: "甲".into(),
                last_opened_ms: 100,
                id: None,
                accounts: Vec::new(),
            },
            VaultEntry {
                path: "/v/b".into(),
                name: "乙".into(),
                last_opened_ms: 300,
                id: None,
                accounts: Vec::new(),
            },
            VaultEntry {
                path: "/v/c".into(),
                name: "丙".into(),
                last_opened_ms: 0,
                id: None,
                accounts: Vec::new(),
            },
        ];
        sort_for_display(&mut entries);
        assert_eq!(
            entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            vec!["乙", "甲", "丙"]
        );
    }

    #[test]
    fn default_name_is_the_folder_name() {
        assert_eq!(default_name(Path::new("/v/我的工作库")), "我的工作库");
    }

    /// 复制出来的库与原库同 id 时才算「被占」；原库被移走（磁盘上已不是这个 id）不算——
    /// 否则移动过的库每次打开都换身份，布局永远找不回来（ADR-140 ②）。
    #[test]
    fn an_id_is_held_elsewhere_only_by_a_live_copy_on_disk() {
        let config = tempfile::tempdir().unwrap();
        let original = tempfile::tempdir().unwrap();
        let copy = tempfile::tempdir().unwrap();
        let registry = VaultRegistry::new(Some(config.path().to_path_buf()));
        let id = crate::Vault::open(original.path()).unwrap().id();
        registry.touch(original.path(), id, 1, None);
        std::fs::create_dir_all(copy.path().join(".luna")).unwrap();
        std::fs::copy(
            original.path().join(".luna/vault-id"),
            copy.path().join(".luna/vault-id"),
        )
        .unwrap();
        assert!(
            registry.id_held_elsewhere(copy.path(), id),
            "复制品撞上仍在的原库"
        );
        assert!(!registry.id_held_elsewhere(original.path(), id), "自己不算");
        std::fs::remove_file(original.path().join(".luna/vault-id")).unwrap();
        assert!(
            !registry.id_held_elsewhere(copy.path(), id),
            "原库已不持有这个 id"
        );
    }

    /// 旧清单（没有 `id` 字段）照常读出：条目只是「还没被本版本打开过」。
    #[test]
    fn entries_without_an_id_still_load() {
        let config = tempfile::tempdir().unwrap();
        std::fs::write(
            config.path().join(REGISTRY_FILENAME),
            r#"[{"path":"/v/a","name":"甲","last_opened_ms":1}]"#,
        )
        .unwrap();
        let entries = VaultRegistry::new(Some(config.path().to_path_buf())).list();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, None);
    }

    fn entry_with(path: &str, accounts: &[&str]) -> VaultEntry {
        VaultEntry {
            path: path.into(),
            name: path.into(),
            last_opened_ms: 0,
            id: None,
            accounts: accounts.iter().map(|a| (*a).to_owned()).collect(),
        }
    }

    /// 可见性矩阵（方案 八）：本机库谁都看得见（含未登录、从没登录过）；A 的库只有 A（含 A 退出
    /// 后——列表账户仍是 A）看得见；B 重新加回来的 A 的库 A、B 都看得见；从没登录过只看得到本机库。
    #[test]
    fn visibility_matrix() {
        let device = entry_with("/v/device", &[]);
        let a_vault = entry_with("/v/a", &["acct-a"]);
        let b_readded = entry_with("/v/a-shared", &["acct-a", "acct-b"]);
        for account in [Some("acct-a"), Some("acct-b"), None] {
            assert!(device.visible_to(account), "本机库对 {account:?} 可见");
        }
        assert!(a_vault.visible_to(Some("acct-a")));
        assert!(
            !a_vault.visible_to(Some("acct-b")),
            "登录 B 时完全看不到 A 的库"
        );
        assert!(!a_vault.visible_to(None), "从没登录过：只有本机库");
        assert!(b_readded.visible_to(Some("acct-a")));
        assert!(b_readded.visible_to(Some("acct-b")));
        assert!(!b_readded.visible_to(Some("acct-c")));
    }

    /// 未登录 = 退出前那个账户的列表：退出不改列表账户（只有登录才改），于是 `visible` 照旧按 A 算。
    #[test]
    fn signed_out_keeps_the_last_accounts_list_and_never_signed_in_sees_device_vaults_only() {
        let config = tempfile::tempdir().unwrap();
        let registry = VaultRegistry::new(Some(config.path().to_path_buf()));
        registry.register(Path::new("/v/device"), None).unwrap();
        registry.register(Path::new("/v/a"), None).unwrap();
        registry.add_account(Path::new("/v/a"), "acct-a");
        let names = |registry: &VaultRegistry| {
            let mut paths: Vec<_> = registry.visible().into_iter().map(|e| e.path).collect();
            paths.sort();
            paths
        };
        assert_eq!(registry.list_account(), None);
        assert_eq!(
            names(&registry),
            vec![PathBuf::from("/v/device")],
            "从没登录过"
        );
        assert!(
            registry.set_list_account("acct-a", "a***@x.org"),
            "首次登录算换了人"
        );
        assert_eq!(
            names(&registry),
            vec![PathBuf::from("/v/a"), PathBuf::from("/v/device")]
        );
        // 退出：不调 set_list_account，列表账户照旧是 A。
        let reread = VaultRegistry::new(Some(config.path().to_path_buf()));
        assert_eq!(
            reread.list_account().map(|a| a.key).as_deref(),
            Some("acct-a")
        );
        assert_eq!(names(&reread).len(), 2);
        assert!(
            !reread.set_list_account("acct-a", "a***@x.org"),
            "同一账户再登录不算换"
        );
        assert!(reread.set_list_account("acct-b", "b***@x.org"));
        assert_eq!(names(&reread), vec![PathBuf::from("/v/device")]);
    }

    /// 开库：已登录时记下这个账户上次打开的库；账户的库（含另一个账户的）列进这个账户的列表；
    /// 本机库照旧是本机库；未登录什么账户的事都不记。
    #[test]
    fn opening_records_the_account_and_joins_account_vaults_only() {
        let config = tempfile::tempdir().unwrap();
        let device = tempfile::tempdir().unwrap();
        let shared = tempfile::tempdir().unwrap();
        let registry = VaultRegistry::new(Some(config.path().to_path_buf()));
        let device_id = crate::Vault::open(device.path()).unwrap().id();
        let shared_id = crate::Vault::open(shared.path()).unwrap().id();

        registry.touch(device.path(), device_id, 1, Some("acct-b"));
        let entry = |path: &Path| {
            registry
                .list()
                .into_iter()
                .find(|e| e.path == path)
                .unwrap()
        };
        assert!(entry(device.path()).is_device(), "打开本机库不把它归到账户");
        assert_eq!(
            registry.last_opened("acct-b").as_deref(),
            Some(device.path())
        );

        registry.register(shared.path(), None).unwrap();
        registry.add_account(shared.path(), "acct-a");
        registry.touch(shared.path(), shared_id, 2, Some("acct-b"));
        assert_eq!(
            entry(shared.path()).accounts,
            vec!["acct-a", "acct-b"],
            "B 重新加回来"
        );
        assert_eq!(
            registry.last_opened("acct-b").as_deref(),
            Some(shared.path())
        );
        assert_eq!(registry.last_opened("acct-a"), None);

        registry.touch(device.path(), device_id, 3, None);
        assert_eq!(
            registry.last_opened("acct-b").as_deref(),
            Some(shared.path()),
            "未登录时不记任何账户的事"
        );
    }

    /// 开启同步列进绑定账户的列表；断开回到本机库。
    #[test]
    fn enabling_sync_lists_the_vault_and_disconnecting_makes_it_a_device_vault() {
        let config = tempfile::tempdir().unwrap();
        let registry = VaultRegistry::new(Some(config.path().to_path_buf()));
        registry.register(Path::new("/v/a"), None).unwrap();
        assert!(registry.add_account(Path::new("/v/a"), "acct-a"));
        assert!(!registry.add_account(Path::new("/v/a"), "acct-a"), "幂等");
        assert!(
            !registry.add_account(Path::new("/v/none"), "acct-a"),
            "不在清单里不凭空加"
        );
        assert_eq!(registry.list()[0].accounts, vec!["acct-a"]);
        registry.make_device(Path::new("/v/a"));
        assert!(registry.list()[0].is_device());
    }

    /// 移除：本机库整条删；账户的库只从当前账户的列表拿掉，别的账户不动；最后一个账户拿掉即整条删。
    #[test]
    fn forgetting_drops_device_vaults_and_only_unlists_account_vaults() {
        let config = tempfile::tempdir().unwrap();
        let registry = VaultRegistry::new(Some(config.path().to_path_buf()));
        for path in ["/v/device", "/v/shared", "/v/a"] {
            registry.register(Path::new(path), None).unwrap();
        }
        registry.add_account(Path::new("/v/shared"), "acct-a");
        registry.add_account(Path::new("/v/shared"), "acct-b");
        registry.add_account(Path::new("/v/a"), "acct-a");

        assert_eq!(
            registry.forget(Path::new("/v/device"), Some("acct-b")),
            Forgotten::Dropped
        );
        assert_eq!(
            registry.forget(Path::new("/v/shared"), Some("acct-b")),
            Forgotten::Kept
        );
        assert_eq!(
            registry.forget(Path::new("/v/a"), Some("acct-a")),
            Forgotten::Dropped
        );
        assert_eq!(
            registry.forget(Path::new("/v/missing"), Some("acct-a")),
            Forgotten::Absent
        );
        let left = registry.list();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].accounts, vec!["acct-a"], "A 的列表不动");
    }

    /// 硬切：没有 `accounts` 一列的旧清单照常读出，每一条都是本机库（同步库由应用启动时按绑定的
    /// 归属账户列进去，见 `vault_lists::claim_bound`）。
    #[test]
    fn entries_without_accounts_load_as_device_vaults() {
        let config = tempfile::tempdir().unwrap();
        std::fs::write(
            config.path().join(REGISTRY_FILENAME),
            r#"[{"path":"/v/a","name":"甲","last_opened_ms":1,"id":null}]"#,
        )
        .unwrap();
        let registry = VaultRegistry::new(Some(config.path().to_path_buf()));
        assert!(registry.list()[0].is_device());
        assert_eq!(registry.visible().len(), 1, "谁都看得见");
    }

    /// 每一次改写都广播一次（界面的库清单订阅只靠它刷新）；只读不广播。
    #[test]
    fn every_change_fires_the_hook_and_reads_do_not() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        let config = tempfile::tempdir().unwrap();
        let vault_dir = tempfile::tempdir().unwrap();
        let id = crate::Vault::open(vault_dir.path()).unwrap().id();
        let registry = VaultRegistry::new(Some(config.path().to_path_buf()));
        let fired = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&fired);
        registry.set_on_change(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        let mut expected = 0;
        let mut step = |label: &str, run: &dyn Fn(&VaultRegistry)| {
            let before = fired.load(Ordering::SeqCst);
            run(&registry);
            assert!(fired.load(Ordering::SeqCst) > before, "{label} 没有广播");
            expected += 1;
        };
        step("登记", &|r| r.register(Path::new("/v/x"), None).unwrap());
        step("改名", &|r| r.rename(Path::new("/v/x"), "新名".into()));
        step("开库", &|r| {
            r.touch(vault_dir.path(), id, 1, Some("acct-a"))
        });
        step("开启同步", &|r| {
            r.add_account(Path::new("/v/x"), "acct-a");
        });
        step("断开同步", &|r| r.make_device(Path::new("/v/x")));
        step("换账户", &|r| {
            r.set_list_account("acct-b", "b***");
        });
        step("移除", &|r| {
            r.forget(Path::new("/v/x"), Some("acct-b"));
        });
        step("撤销登记", &|r| r.remove(vault_dir.path()));
        assert!(expected > 0);
        let before = fired.load(Ordering::SeqCst);
        let _ = (registry.list(), registry.visible(), registry.list_account());
        assert_eq!(fired.load(Ordering::SeqCst), before, "只读不广播");
    }
}
