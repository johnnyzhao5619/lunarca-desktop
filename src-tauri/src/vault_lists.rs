//! 库列表跟着账户走（ADR-151 修订 2026-10-15「续二」；
//! TDD-06 §12；同步库归属与移除方案 八，内部记录）。
//!
//! 库注册表（`luna_vault::registry`）只认不透明的账户键与「列表账户」；这里把它接到此刻谁登录着、
//! 同步绑定归谁、以及主窗口打开哪个库上：
//!
//! - **列表账户**：已登录 = 登录的账户；退出不是切换（列表照旧是退出前那一个，也不另记任何账户
//!   的事）；从没登录过 = 没有，只看得到本机库。可见性的唯一判据是 `registry::visible_to`。
//! - **绑定 → 列表**：有同步绑定的库一定在绑定账户的列表里（[`claim_bound`]：启动时对全部记录
//!   一次——旧清单没有账户这一列，同步库由此列进绑定账户；开库时对那一个库一次）；开启同步把库
//!   列进绑定账户的列表，断开让它回到本机库（调用点在 `sync_ipc` / `sync_cloud` / `sync_service`）。
//! - **换账户**（[`account_changed`]，启动时也对齐一次）：列表账户换了人，就打开新账户上次打开的
//!   库；它没有、而此刻打开的库它又看不见，就交出当前库回到空状态（新建 / 打开文件夹 / 从云端库
//!   下载）。切换走切换器同一条交接 + 装载（`ipc::open_for_account` / `ipc::close_for_account`），
//!   录音护栏照旧：录音中（或队列停不下来）不当场切，打开的库原样留着，而是**推迟**到录音收尾、
//!   处理静下来之后自动接着做（[`PendingFollow`]；列表账户其间又换了就作废）。开着的副库窗口里
//!   新账户看不见的那些当场关掉（走窗口自己的关闭路径，未保存的改动先落盘）。
//! - **广播**：注册表每改写一次就发一次 `vaults://changed`（[`install`] 装在注册表的变更钩子上，
//!   任何一条改写路径都漏不掉）；界面的库清单订阅只靠它刷新。
//!
//! 账户键见 `account_scope::account_key`（分桶 + 账户 id 的摘要），注册表与本模块从不记邮箱。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use luna_sync::state::BindingOwner;
use luna_vault::registry::{VaultEntry, VaultRegistry};
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::Manager as _;
use tauri_specta::Event as _;

use crate::account_scope::account_key;

/// 库清单变了（注册、移除、改名、开库、绑定变化、换列表账户）。载荷为空：界面据此重取
/// `list_vaults`。
#[derive(Debug, Clone, Serialize, Deserialize, Type, tauri_specta::Event)]
#[tauri_specta(event_name = "vaults://changed")]
pub struct VaultsChanged {}

/// 换了账户、主窗口因此换了库（或交出了库、回到空状态）。主窗口工作区据此整体重挂载——切库
/// 由 Rust 发起，不经界面的切换控制器。
#[derive(Debug, Clone, Serialize, Deserialize, Type, tauri_specta::Event)]
#[tauri_specta(event_name = "vault://followed-account")]
pub struct VaultFollowedAccount {}

/// 此刻登录的账户键（`None` = 未登录或还没对齐）。账户档案在钥匙串里，每次取清单都读会在
/// 某些构建上多弹钥匙串框，故只在对齐时（启动一次、每次账户变化一次）读一次记在这里。
#[derive(Default)]
pub struct SignedIn(Mutex<Option<String>>);

/// 推迟的那一次跟随（录音中 / 队列停不下来时换了账户）：记着是为哪个列表账户推迟的。锁同时把
/// 「对齐」与「接着做」串起来——两者都会切库，不能并发。
#[derive(Default)]
pub struct PendingFollow {
    slot: Mutex<Option<String>>,
    /// 有没有推迟着的（零锁快查：触发点在录音状态转移、队列收尾上，不能在那里等切库的锁）。
    armed: std::sync::atomic::AtomicBool,
}

/// 装上注册表的变更广播与登录账户的记忆（`app.manage(AppState)` 之后、启动对齐之前）。
pub fn install(app: &tauri::AppHandle) {
    app.manage(SignedIn::default());
    app.manage(PendingFollow::default());
    let Some(state) = app.try_state::<crate::ipc::AppState>() else {
        return;
    };
    let handle = app.clone();
    state.vault_registry().set_on_change(move || {
        let _ = VaultsChanged {}.emit(&handle);
    });
}

/// 此刻登录的账户键（零 I/O；对齐之前 = `None`）。开库时据此记「这个账户上次打开的库」。
#[must_use]
pub fn signed_in_key(app: &tauri::AppHandle) -> Option<String> {
    app.try_state::<SignedIn>()
        .and_then(|signed| signed.0.lock().ok().and_then(|key| key.clone()))
}

/// 这个库在这台设备上的同步绑定归谁（零网络；没有绑定 = `None`）。
#[must_use]
pub fn bound_owner(config_dir: &Path, entry: &VaultEntry) -> Option<BindingOwner> {
    let id = crate::sync_service::identity_of(entry)?;
    let path = crate::sync_service::state_dir(config_dir, &id).join("state.sqlite");
    if !path.is_file() {
        return None;
    }
    luna_sync::state::peek_owner(&path).ok().flatten()
}

/// 有同步绑定的库列进绑定账户的列表（`only` = 只看这一个库）。不变式而非迁移：绑定只会在开启
/// 同步时落下（那里已经列进去了），这里补的是没有账户这一列的旧清单与清单外的绑定。
pub fn claim_bound(registry: &VaultRegistry, config_dir: &Path, only: Option<&Path>) {
    for entry in registry.list() {
        if only.is_some_and(|path| path != entry.path) {
            continue;
        }
        if let Some(owner) = bound_owner(config_dir, &entry) {
            registry.add_account(&entry.path, &account_key(&owner));
        }
    }
}

/// 一个库对列表账户 `account` 看不看得见。清单里根本没有它（配置目录不可写、清单落不了盘）就当
/// 看得见——不能因为清单缺席就把用户此刻开着的库关掉。
fn visible_in(entries: &[VaultEntry], account: Option<&str>, path: &Path) -> bool {
    entries
        .iter()
        .find(|entry| entry.path == path)
        .is_none_or(|entry| entry.visible_to(account))
}

/// 上次打开的库还能用：在清单里、看得见、文件夹还在。
fn usable_last(
    entries: &[VaultEntry],
    account: Option<&str>,
    last: Option<&Path>,
    exists: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let last = last?;
    let listed = entries.iter().any(|entry| entry.path == last);
    (listed && visible_in(entries, account, last) && exists(last)).then(|| last.to_path_buf())
}

/// 换了列表账户之后主窗口该怎么办。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Follow {
    /// 不动。
    Stay,
    /// 切到这个库（新账户上次打开的）。
    Open(PathBuf),
    /// 交出当前库，回到空状态。
    Close,
}

/// 换列表账户之后的去向（纯函数）：新账户上次打开的库还能用 → 打开它（就是当前库则不动）；否则
/// 当前库它看得见（或本来就没开库）→ 不动；都不是 → 交出当前库（用户裁决 3：切换账户后自动打开
/// 该账户上次用的库；当前库看不见又没有可去的 → 空状态）。
#[must_use]
pub fn follow_on_switch(
    entries: &[VaultEntry],
    account: &str,
    open: Option<&Path>,
    last: Option<&Path>,
    exists: impl Fn(&Path) -> bool,
) -> Follow {
    if let Some(last) = usable_last(entries, Some(account), last, exists) {
        return if open == Some(last.as_path()) {
            Follow::Stay
        } else {
            Follow::Open(last)
        };
    }
    match open {
        Some(open) if !visible_in(entries, Some(account), open) => Follow::Close,
        _ => Follow::Stay,
    }
}

/// 换列表账户之后要关掉的副库窗口（纯函数）：窗口里的库新账户看不见。清单里没有的库照
/// [`visible_in`] 当作看得见，不关。
#[must_use]
pub fn windows_to_close(
    entries: &[VaultEntry],
    account: &str,
    windows: &[(String, PathBuf)],
) -> Vec<String> {
    windows
        .iter()
        .filter(|(_, root)| !visible_in(entries, Some(account), root))
        .map(|(label, _)| label.clone())
        .collect()
}

/// 跟随被这两道护栏挡下时推迟（与手动切库同一组码），别的失败（库打不开等）照旧只记一笔。
#[must_use]
pub fn deferrable(code: &str) -> bool {
    ["VAULT_SWITCH_RECORDING:", "VAULT_SWITCH_BUSY:"]
        .iter()
        .any(|prefix| code.starts_with(prefix))
}

/// 推迟着的跟随此刻怎么办。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retry {
    /// 还得等（录音没收尾、处理还在跑）。
    Wait,
    /// 列表账户其间又换了（那一次换账户自己会对齐）：作废。
    Drop,
    /// 现在做。
    Apply,
}

/// 推迟着的跟随（为 `pending` 这个列表账户推迟的）此刻怎么办（纯函数）。`quiet` = 录音空闲、
/// 停录收尾已落定、处理队列静着——推迟的那一次等处理收尾而不是当场取消它：停录必入队一件权威
/// 重转写，录音一停就切等于把它取消掉。
#[must_use]
pub fn retry_follow(pending: &str, list_account: Option<&str>, quiet: bool) -> Retry {
    if list_account != Some(pending) {
        Retry::Drop
    } else if quiet {
        Retry::Apply
    } else {
        Retry::Wait
    }
}

/// 启动装载的那个库（启动不经 `bind_vault`，可见性在这里判；纯函数）：启动指针指着的库列表账户
/// 看得见就开它；看不见就开列表账户上次打开的（还能用的话）；都不行 = 不开库（空状态）。返回
/// （要开的库，启动指针是不是因为看不见才没开）——后者为真时空状态不该说「上次的库找不到了」。
#[must_use]
pub fn startup_choice(
    entries: &[VaultEntry],
    account: Option<&str>,
    configured: Option<PathBuf>,
    last: Option<&Path>,
    exists: impl Fn(&Path) -> bool,
) -> (Option<PathBuf>, bool) {
    let Some(configured) = configured else {
        return (None, false);
    };
    if visible_in(entries, account, &configured) {
        return (Some(configured), false);
    }
    (usable_last(entries, account, last, exists), true)
}

/// 按注册表里的记忆挑启动装载的库（[`startup_choice`] 的取数写法）。
#[must_use]
pub fn startup_root(
    registry: &VaultRegistry,
    configured: Option<PathBuf>,
) -> (Option<PathBuf>, bool) {
    let account = registry.list_account().map(|a| a.key);
    let last = account.as_deref().and_then(|key| registry.last_opened(key));
    startup_choice(
        &registry.list(),
        account.as_deref(),
        configured,
        last.as_deref(),
        Path::is_dir,
    )
}

/// 「管理库」移除这一条会不会断开它在这台设备上的同步绑定（纯函数；预检与移除共用）。
///
/// 断开当且仅当：绑定归列表账户自己（正在同步，或退出后仍是它的），或这一条会被整条删掉（不留
/// 清单外的孤儿绑定）。绑定归另一个账户、而那个账户的列表里还有它：只从当前列表拿掉，那个账户
/// 的同步不动（「其他账户的列表不受影响」）。
#[must_use]
pub fn forget_releases_binding(
    entry: &VaultEntry,
    list_account: Option<&str>,
    owner: Option<&str>,
) -> bool {
    let Some(owner) = owner else {
        return false;
    };
    list_account == Some(owner)
        || luna_vault::registry::forget_outcome(entry, list_account)
            == luna_vault::registry::Forgotten::Dropped
}

/// 账户变了（登录、退出、换账户、会话失效）：后台对齐（读钥匙串与切库都是阻塞的）。
pub fn account_changed(app: &tauri::AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || align(&app));
}

/// 启动时一次：旧清单里的同步库列进绑定账户，再按此刻登录的账户对齐。
pub fn startup(app: &tauri::AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        if let (Some(state), Ok(config_dir)) = (
            app.try_state::<crate::ipc::AppState>(),
            app.path().app_config_dir(),
        ) {
            claim_bound(state.vault_registry(), &config_dir, None);
        }
        align(&app);
    });
}

/// 按此刻登录的账户对齐列表账户与主窗口打开的库。
fn align(app: &tauri::AppHandle) {
    let Some(state) = app.try_state::<crate::ipc::AppState>() else {
        return;
    };
    let owner = crate::account_scope::current_in(app);
    let key = owner.as_ref().map(account_key);
    if let Some(signed) = app.try_state::<SignedIn>() {
        if let Ok(mut current) = signed.0.lock() {
            current.clone_from(&key);
        }
    }
    let (Some(owner), Some(key)) = (owner, key) else {
        // 退出不是切换（用户裁决 4）：列表照旧；只让界面重取（「云端库」一节随之收起）。
        let _ = VaultsChanged {}.emit(app);
        return;
    };
    let registry = state.vault_registry();
    if registry.set_list_account(&key, &owner.account_label) {
        close_invisible_windows(app, &state, &key);
        let pending = app.try_state::<PendingFollow>();
        // 持着推迟槽的锁切库：与「接着做」串行；新的一次换账户顶替上一次推迟的。
        let mut slot = pending.as_ref().and_then(|p| p.slot.lock().ok());
        let outcome = follow(app, &state, &key);
        let deferred = outcome.as_ref().err().is_some_and(|code| deferrable(code));
        if let (Some(pending), Some(slot)) = (pending.as_ref(), slot.as_mut()) {
            **slot = deferred.then(|| key.clone());
            pending
                .armed
                .store(deferred, std::sync::atomic::Ordering::SeqCst);
        }
        drop(slot);
        if let Err(code) = outcome {
            // 录音中 / 队列停不下来：打开的库原样留着，收尾之后接着做；其余失败只记稳定码。
            eprintln!("[vaults] follow account switch deferred={deferred}: {code}");
        }
    }
    // 推迟着的时候开着的还是上一个账户的库：不记成这个账户上次打开的（否则接着做时会把它当成
    // 这个账户的去处，而它恰恰看不见）。推迟期间同一账户再对齐一次（会话续期）同样不记。
    let deferring = app
        .try_state::<PendingFollow>()
        .is_some_and(|p| p.armed.load(std::sync::atomic::Ordering::SeqCst));
    if !deferring {
        remember_open(&state, &key);
    }
    let _ = VaultsChanged {}.emit(app);
}

/// 启动装载不经 `bind_vault`：此刻开着的库在这里记为这个账户上次打开的。
fn remember_open(state: &crate::ipc::AppState, key: &str) {
    if let Some(open) = crate::ipc::current_vault_root(state).ok().flatten() {
        state.vault_registry().remember_opened(key, &open);
    }
}

/// 按列表账户 `key` 决定主窗口的去向并执行（对齐与推迟后接着做共用）。
fn follow(app: &tauri::AppHandle, state: &crate::ipc::AppState, key: &str) -> Result<(), String> {
    let registry = state.vault_registry();
    let open = crate::ipc::current_vault_root(state).ok().flatten();
    let last = registry.last_opened(key);
    match follow_on_switch(
        &registry.list(),
        key,
        open.as_deref(),
        last.as_deref(),
        Path::is_dir,
    ) {
        Follow::Stay => Ok(()),
        Follow::Open(root) => crate::ipc::open_for_account(app, state, root),
        Follow::Close => crate::ipc::close_for_account(app, state),
    }
}

/// 关掉新列表账户看不见的副库窗口（[`windows_to_close`]），各走窗口自己的关闭路径。
fn close_invisible_windows(app: &tauri::AppHandle, state: &crate::ipc::AppState, key: &str) {
    let entries = state.vault_registry().list();
    for label in windows_to_close(&entries, key, &state.secondary_window_roots()) {
        crate::ipc::close_vault_window(app, state, &label);
    }
}

/// 推迟着的跟随可能可以接着做了（录音状态转移出会话、停录收尾落定、处理队列收尾时调）：没有
/// 推迟着的就什么都不做（零锁）；有就后台判一次——切库会等队列落定，不能挂在调用方的线程上。
pub fn retry_pending(app: &tauri::AppHandle) {
    let armed = app
        .try_state::<PendingFollow>()
        .is_some_and(|p| p.armed.load(std::sync::atomic::Ordering::SeqCst));
    if !armed {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || apply_pending(&app));
}

/// 判一次推迟着的跟随：列表账户还是推迟时那一个、录音收尾了、处理静了才做；挡下的码仍可推迟就
/// 继续等下一次触发。
fn apply_pending(app: &tauri::AppHandle) {
    let (Some(state), Some(pending)) = (
        app.try_state::<crate::ipc::AppState>(),
        app.try_state::<PendingFollow>(),
    ) else {
        return;
    };
    let Ok(mut slot) = pending.slot.lock() else {
        return;
    };
    let Some(key) = slot.clone() else {
        return;
    };
    let list_account = state.vault_registry().list_account().map(|a| a.key);
    let quiet = !crate::ipc::recording_blocks_switch(&state)
        && !crate::ipc::post_stop_pending()
        && state.processing().is_quiet();
    let outcome = match retry_follow(&key, list_account.as_deref(), quiet) {
        Retry::Wait => return,
        Retry::Drop => {
            *slot = None;
            pending
                .armed
                .store(false, std::sync::atomic::Ordering::SeqCst);
            return;
        }
        Retry::Apply => follow(app, &state, &key),
    };
    let deferred = outcome.as_ref().err().is_some_and(|code| deferrable(code));
    if !deferred {
        *slot = None;
        pending
            .armed
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }
    drop(slot);
    match outcome {
        Ok(()) => {
            remember_open(&state, &key);
            let _ = VaultsChanged {}.emit(app);
        }
        Err(code) => eprintln!("[vaults] deferred account follow deferred={deferred}: {code}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, accounts: &[&str]) -> VaultEntry {
        VaultEntry {
            path: path.into(),
            name: path.into(),
            last_opened_ms: 0,
            id: None,
            accounts: accounts.iter().map(|a| (*a).to_owned()).collect(),
        }
    }

    fn all_exist(_: &Path) -> bool {
        true
    }

    /// 换账户：新账户上次打开的库还能用就打开它；没有、而当前库它看不见 → 空状态；当前库它看得见
    /// （本机库）→ 不动。
    #[test]
    fn switching_accounts_opens_the_new_accounts_last_vault_or_closes_an_invisible_one() {
        let entries = vec![
            entry("/v/device", &[]),
            entry("/v/a", &["acct-a"]),
            entry("/v/b", &["acct-b"]),
        ];
        let open_a = Some(Path::new("/v/a"));
        assert_eq!(
            follow_on_switch(
                &entries,
                "acct-b",
                open_a,
                Some(Path::new("/v/b")),
                all_exist
            ),
            Follow::Open(PathBuf::from("/v/b"))
        );
        assert_eq!(
            follow_on_switch(&entries, "acct-b", open_a, None, all_exist),
            Follow::Close,
            "B 没打开过任何库，而 A 的库 B 看不见"
        );
        assert_eq!(
            follow_on_switch(
                &entries,
                "acct-b",
                Some(Path::new("/v/device")),
                None,
                all_exist
            ),
            Follow::Stay,
            "本机库谁都看得见"
        );
        assert_eq!(
            follow_on_switch(
                &entries,
                "acct-b",
                Some(Path::new("/v/device")),
                Some(Path::new("/v/b")),
                all_exist
            ),
            Follow::Open(PathBuf::from("/v/b")),
            "裁决 3：自动打开该账户上次用的库"
        );
        assert_eq!(
            follow_on_switch(
                &entries,
                "acct-b",
                Some(Path::new("/v/b")),
                Some(Path::new("/v/b")),
                all_exist
            ),
            Follow::Stay
        );
        assert_eq!(
            follow_on_switch(&entries, "acct-b", None, None, all_exist),
            Follow::Stay,
            "本来就没开库：留在空状态"
        );
    }

    /// 上次打开的库已经看不见（被移除出 B 的列表）、不在清单里或文件夹没了：不去它那里。
    #[test]
    fn a_last_vault_that_is_gone_or_hidden_is_not_reopened() {
        let entries = vec![entry("/v/a", &["acct-a"]), entry("/v/gone", &["acct-b"])];
        let open_a = Some(Path::new("/v/a"));
        assert_eq!(
            follow_on_switch(
                &entries,
                "acct-b",
                open_a,
                Some(Path::new("/v/a")),
                all_exist
            ),
            Follow::Close,
            "A 的库不在 B 的列表里"
        );
        assert_eq!(
            follow_on_switch(
                &entries,
                "acct-b",
                open_a,
                Some(Path::new("/v/forgotten")),
                all_exist
            ),
            Follow::Close,
            "已从清单移除"
        );
        assert_eq!(
            follow_on_switch(
                &entries,
                "acct-b",
                open_a,
                Some(Path::new("/v/gone")),
                |_| false
            ),
            Follow::Close,
            "文件夹没了"
        );
    }

    /// 清单里没有的库（配置目录不可写）当作看得见：不因清单缺席关掉用户开着的库。
    #[test]
    fn an_unlisted_open_vault_is_never_closed() {
        assert_eq!(
            follow_on_switch(&[], "acct-b", Some(Path::new("/v/x")), None, all_exist),
            Follow::Stay
        );
    }

    /// 录音中 / 队列停不下来挡下的跟随推迟（只认这两个码），别的失败不推迟。
    #[test]
    fn only_the_recording_and_busy_guards_defer_a_follow() {
        assert!(deferrable("VAULT_SWITCH_RECORDING: 录音进行中"));
        assert!(deferrable(
            "VAULT_SWITCH_BUSY: Some tasks could not be paused"
        ));
        assert!(!deferrable("VAULT_MISSING: gone"));
        assert!(!deferrable("Vault 锁中毒"));
        assert!(!deferrable("XVAULT_SWITCH_BUSY: no"));
    }

    /// 推迟着的跟随：录音收尾、处理静了且列表账户还是那一个才做；还在忙就等；列表账户其间又换了
    /// （或退出后被另一个账户登录顶替）就作废。
    #[test]
    fn a_deferred_follow_waits_for_quiet_and_is_dropped_when_the_account_changed_again() {
        assert_eq!(retry_follow("acct-b", Some("acct-b"), false), Retry::Wait);
        assert_eq!(retry_follow("acct-b", Some("acct-b"), true), Retry::Apply);
        assert_eq!(retry_follow("acct-b", Some("acct-c"), true), Retry::Drop);
        assert_eq!(retry_follow("acct-b", Some("acct-c"), false), Retry::Drop);
        assert_eq!(retry_follow("acct-b", None, true), Retry::Drop);
    }

    /// 换账户后关掉的副库窗口：新账户看不见的才关；本机库、新账户自己的库、清单里没有的库不关。
    #[test]
    fn switching_accounts_closes_only_windows_on_vaults_the_new_account_cannot_see() {
        let entries = vec![
            entry("/v/device", &[]),
            entry("/v/a", &["acct-a"]),
            entry("/v/b", &["acct-b"]),
            entry("/v/shared", &["acct-a", "acct-b"]),
        ];
        let windows: Vec<(String, PathBuf)> = [
            ("vault-window-0", "/v/device"),
            ("vault-window-1", "/v/a"),
            ("vault-window-2", "/v/b"),
            ("vault-window-3", "/v/shared"),
            ("vault-window-4", "/v/unlisted"),
        ]
        .iter()
        .map(|(label, root)| ((*label).to_owned(), PathBuf::from(root)))
        .collect();
        assert_eq!(
            windows_to_close(&entries, "acct-b", &windows),
            vec!["vault-window-1".to_owned()]
        );
        assert!(windows_to_close(&entries, "acct-b", &[]).is_empty());
    }

    /// 接线：换账户时先关看不见的副库窗口、挡下的跟随记进推迟槽（推迟时不把开着的旧库记成新账户
    /// 上次打开的）；录音转出会话、停录收尾落定、处理队列收尾三处都会再看一眼；副库窗口经窗口
    /// 自己的关闭路径关（会落盘），且副库窗口拿得到关窗与收事件的权限——否则 `onCloseRequested`
    /// 注册不上，Rust 一关就直接销毁、未保存的改动丢掉。任一处被删即红。
    #[test]
    fn deferred_follows_and_window_closing_are_wired() {
        let squash = |text: &str| text.split_whitespace().collect::<String>();
        let own = squash(&crate::ipc::source_without_tests(include_str!(
            "vault_lists.rs"
        )));
        assert!(own.contains("close_invisible_windows(app,&state,&key);letpending"));
        assert!(own.contains("**slot=deferred.then(||key.clone());"));
        assert!(own.contains("if!deferring{remember_open(&state,&key);}"));
        assert!(own.contains("crate::ipc::close_vault_window(app,state,&label);"));
        let recording_source = crate::ipc::source_without_tests(include_str!("ipc/recording.rs"));
        let observer = recording_source
            .split("pub(crate) fn install_recording_state_broadcast(")
            .nth(1)
            .and_then(|tail| tail.split("\n}\n").next())
            .expect("install_recording_state_broadcast");
        assert!(squash(observer).contains("crate::vault_lists::retry_pending(&app);"));
        let recording = squash(&recording_source);
        assert!(recording.contains(
            "letticket=PostStopTicket::take(&app);tauri::async_runtime::spawn(asyncmove{let_ticket=ticket;"
        ));
        assert!(recording.contains("crate::vault_lists::retry_pending(&self.0);"));
        let processing = squash(&crate::ipc::source_without_tests(include_str!(
            "ipc/processing.rs"
        )));
        assert!(processing.contains("crate::vault_lists::retry_pending(&app);"));
        let workspace = squash(&crate::ipc::source_without_tests(include_str!(
            "ipc/workspace.rs"
        )));
        assert!(workspace
            .contains("ifletSome(window)=app.get_webview_window(label){let_=window.close();"));
        for capability in [
            include_str!("../capabilities/default.json"),
            include_str!("../capabilities/note-editor.json"),
        ] {
            let json: serde_json::Value = serde_json::from_str(capability).unwrap();
            assert!(
                json["windows"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|w| w == "vault-window-*"),
                "副库窗口不在 {} 里",
                json["identifier"]
            );
        }
    }

    /// 启动装载：启动指针看得见就开它；看不见就开列表账户上次打开的；都没有就空状态，且不说
    /// 「上次的库找不到了」。
    #[test]
    fn startup_opens_a_visible_vault_only() {
        let entries = vec![
            entry("/v/device", &[]),
            entry("/v/a", &["acct-a"]),
            entry("/v/b", &["acct-b"]),
        ];
        assert_eq!(
            startup_choice(
                &entries,
                Some("acct-a"),
                Some("/v/a".into()),
                None,
                all_exist
            ),
            (Some(PathBuf::from("/v/a")), false)
        );
        assert_eq!(
            startup_choice(
                &entries,
                Some("acct-b"),
                Some("/v/a".into()),
                Some(Path::new("/v/b")),
                all_exist
            ),
            (Some(PathBuf::from("/v/b")), true)
        );
        assert_eq!(
            startup_choice(&entries, None, Some("/v/a".into()), None, all_exist),
            (None, true),
            "从没登录过：A 的库不开"
        );
        assert_eq!(
            startup_choice(&entries, None, Some("/v/new".into()), None, all_exist),
            (Some(PathBuf::from("/v/new")), false),
            "清单里还没有的库（首启、环境变量指定）照开"
        );
        assert_eq!(
            startup_choice(&entries, None, None, None, all_exist),
            (None, false)
        );
    }

    /// 移除何时顺带断开：绑定归列表账户自己 → 断开；归另一个账户、它的列表里还有 → 不动；整条删掉
    /// → 不留孤儿绑定，断开；没有绑定 → 不涉及。
    #[test]
    fn forgetting_releases_the_binding_only_when_it_is_ours_or_the_entry_goes() {
        let a_only = entry("/v/a", &["acct-a"]);
        let shared = entry("/v/s", &["acct-a", "acct-b"]);
        assert!(forget_releases_binding(
            &a_only,
            Some("acct-a"),
            Some("acct-a")
        ));
        assert!(
            !forget_releases_binding(&shared, Some("acct-b"), Some("acct-a")),
            "B 移除 A 的库：A 的同步不动"
        );
        assert!(forget_releases_binding(
            &shared,
            Some("acct-a"),
            Some("acct-a")
        ));
        let orphan = entry("/v/o", &["acct-b"]);
        assert!(
            forget_releases_binding(&orphan, Some("acct-b"), Some("acct-a")),
            "整条删掉：不留清单外的绑定"
        );
        assert!(!forget_releases_binding(&a_only, Some("acct-a"), None));
    }

    /// 旧清单（没有账户这一列）里的同步库：启动时列进绑定账户的列表；没绑定的照旧是本机库。
    #[test]
    fn bound_vaults_are_claimed_by_their_owner_at_startup() {
        let config = tempfile::tempdir().unwrap();
        let synced = tempfile::tempdir().unwrap();
        let plain = tempfile::tempdir().unwrap();
        let synced_id = luna_vault::Vault::open(synced.path())
            .unwrap()
            .id()
            .to_string();
        let _ = luna_vault::Vault::open(plain.path()).unwrap();
        std::fs::write(
            config.path().join(luna_vault::registry::REGISTRY_FILENAME),
            serde_json::to_string(&serde_json::json!([
                {"path": synced.path(), "name": "s", "last_opened_ms": 1, "id": synced_id},
                {"path": plain.path(), "name": "p", "last_opened_ms": 1, "id": null},
            ]))
            .unwrap(),
        )
        .unwrap();
        let owner = BindingOwner {
            server_bucket: "api.lunarca.app".into(),
            account_id: "acc-a".into(),
            account_label: "a***@x.org".into(),
        };
        let dir = crate::sync_service::state_dir(config.path(), &synced_id);
        std::fs::create_dir_all(&dir).unwrap();
        let mut state = luna_sync::state::SyncState::open(&dir.join("state.sqlite")).unwrap();
        state
            .bind(&luna_sync::state::Binding {
                remote_vault_id: uuid::Uuid::from_u128(1),
                server_origin: "https://api.lunarca.app".into(),
                local_vault_id: synced_id.clone(),
                created_ms: 0,
                owner: owner.clone(),
            })
            .unwrap();
        drop(state);

        let registry = VaultRegistry::new(Some(config.path().to_path_buf()));
        assert!(
            registry.list().iter().all(VaultEntry::is_device),
            "硬切：先都是本机库"
        );
        claim_bound(&registry, config.path(), None);
        let entries = registry.list();
        let of = |path: &Path| entries.iter().find(|e| e.path == path).unwrap().clone();
        assert_eq!(of(synced.path()).accounts, vec![account_key(&owner)]);
        assert!(of(plain.path()).is_device());
        assert!(
            registry.visible().iter().all(|e| e.path != synced.path()),
            "从没登录过：看不见"
        );
    }

    /// 绑定变化把库挪到对的列表上：开启 / 加入 / 改连 / 下载到本机（都经 `bind_and_install`）列进
    /// 绑定账户；停止同步、删除云端库、别处删了云端库（三条断开路径）回到本机库；注册表的每次改写
    /// 都广播 `vaults://changed`。任一处被删即红。
    #[test]
    fn binding_changes_move_vaults_between_lists_and_every_change_is_broadcast() {
        let squash = |text: &str| text.split_whitespace().collect::<String>();
        let body = |source: &str, marker: &str| -> String {
            let source = crate::ipc::source_without_tests(source);
            let tail = source.split(marker).nth(1).expect(marker).to_owned();
            squash(tail.split("\n}\n").next().unwrap_or(&tail))
        };
        let sync_ipc = include_str!("ipc/sync_ipc.rs");
        assert!(
            body(sync_ipc, "pub(super) async fn bind_and_install(")
                .contains(".vault_registry.add_account(&root,&account)"),
            "开启同步没有列进绑定账户的列表"
        );
        assert!(
            body(sync_ipc, "pub async fn sync_disable(").contains(".make_device(&root)"),
            "停止同步没有回到本机库"
        );
        assert!(
            body(
                include_str!("ipc/sync_cloud.rs"),
                "pub async fn sync_cloud_vault_delete("
            )
            .contains(".make_device(&entry.path)"),
            "删除云端库没有回到本机库"
        );
        let service = include_str!("sync_service.rs");
        let release = service
            .split("fn release(&self, session: Session) -> Result<(), String> {")
            .nth(1)
            .expect("AppEnv::release");
        assert!(
            squash(release.split("\n    }\n").next().unwrap_or(release))
                .contains(".make_device(&root)"),
            "别处删了云端库、自动断开后没有回到本机库"
        );
        let install = body(include_str!("vault_lists.rs"), "pub fn install(");
        assert!(install.contains("set_on_change(") && install.contains("VaultsChanged{}.emit"));
        let lib = squash(&crate::ipc::source_without_tests(include_str!("lib.rs")));
        assert!(lib.contains("vault_lists::install(app.handle())"));
        assert!(
            lib.contains("vault_lists::account_changed(app)"),
            "换账户没有换列表"
        );
    }
}
