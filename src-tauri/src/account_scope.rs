//! 此刻登录的是谁，以及「关于这个账户」的本机记录放哪（TDD-14 §5、ADR-151
//! 修订 2026-10-15）。
//!
//! 同一台设备上先后登录不同账户（或同一账户换了服务器）时，凡是描述某个账户处境的本机记录都
//! 不能沿用上一个账户的：同步绑定的归属、只读宽限的提醒、付款处境与用量阈值的提醒标记。2026-10-15
//! 真机：退出 A、登录 B 后，同步拿 A 的远端库 id 去问 B 的服务端，报「云端找不到这个库」。
//!
//! 账户由（服务端分桶，账户 id）认定；提醒标记按它的确定性摘要分目录存（可作文件名、跨平台安全
//! ——分桶里有 `:`，路径里也不出现邮箱）。设备级的偏好（全部暂停、录音同步方式、按流量 / 电池
//! 暂停、选择性同步，PRD-13-044）不在这里。

use std::path::{Path, PathBuf};

use luna_cloud::client::CloudClient;
use luna_sync::state::BindingOwner;

/// 账户键的前缀：一眼看得出是账户目录。
const KEY_PREFIX: &str = "acct-";

/// （服务端分桶，账户 id）→ 账户键 `acct-<32 位 hex>`。
#[must_use]
pub fn account_key(owner: &BindingOwner) -> String {
    use sha2::Digest as _;
    let mut hasher = sha2::Sha256::new();
    hasher.update(owner.server_bucket.as_bytes());
    // 分隔符不可能出现在分桶（主机与端口）里：`("a", "bc")` 与 `("ab", "c")` 不会撞。
    hasher.update(b"\n");
    hasher.update(owner.account_id.as_bytes());
    let digest = hasher.finalize();
    let hex: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
    format!("{KEY_PREFIX}{hex}")
}

/// 其他账户的打码写法（用户裁决 2026-10-15）：本地部分留前 2 个与后 2 个字符、中间一个「…」，
/// 域名照写（`jo…ai@gmail.com`）；本地部分不超过 4 个字符时只留第一个（`a…@x.com`）。
///
/// 只留头会让同前缀的两个账户打成同一串（真机：`johnnymai…` 与 `johnnyzhao…` 都成了
/// `jo***@gmail.com`，切换菜单分不出是哪一个）；头尾各留两个就分得开，又不露出整个名字。
/// **当前登录的账户不走这里**：它显示完整邮箱（[`current_email`]）。
///
/// 按字符而不是按字节截（账户名可能不是 ASCII，按字节切会切在字符中间）。
#[must_use]
pub fn mask_account_name(name: &str) -> String {
    let (local, domain) = match name.split_once('@') {
        Some((local, domain)) => (local, Some(domain)),
        None => (name, None),
    };
    let chars: Vec<char> = local.chars().collect();
    let masked: String = if chars.len() > 4 {
        let head: String = chars[..2].iter().collect();
        let tail: String = chars[chars.len() - 2..].iter().collect();
        format!("{head}…{tail}")
    } else {
        let head: String = chars.iter().take(1).collect();
        format!("{head}…")
    };
    match domain {
        Some(domain) => format!("{masked}@{domain}"),
        None => masked,
    }
}

/// 此刻登录的账户的（账户键，完整邮箱）——界面上「当前账户」显示完整邮箱，不读存下来的打码标签
/// （用户裁决 2026-10-15）。没登录或读不出档案 = `None`（零网络：只读钥匙串）。
#[must_use]
pub fn current_email(app: &tauri::AppHandle) -> Option<(String, String)> {
    use tauri::Manager as _;
    let cloud = app.try_state::<luna_cloud::commands::CloudState>()?;
    let owner = current(&cloud.0)?;
    let email = cloud.0.cached_profile()?.email;
    Some((account_key(&owner), email))
}

/// 此刻登录的账户；没登录、或缓存的账户档案读不出来 = `None`（零网络：只读钥匙串）。
///
/// 读不出档案时当作没有账户：不确定是谁，就不碰任何人的绑定与提醒。
#[must_use]
pub fn current(client: &CloudClient) -> Option<BindingOwner> {
    if !client.logged_in() {
        return None;
    }
    let profile = client.cached_profile()?;
    Some(BindingOwner {
        server_bucket: client.server_bucket(),
        account_id: profile.account_id,
        account_label: mask_account_name(&profile.email),
    })
}

/// 应用里此刻登录的账户（没有云客户端 = `None`）。
#[must_use]
pub fn current_in(app: &tauri::AppHandle) -> Option<BindingOwner> {
    use tauri::Manager as _;
    let cloud = app.try_state::<luna_cloud::commands::CloudState>()?;
    current(&cloud.0)
}

/// 一个账户的提醒标记目录：`<应用配置目录>/accounts/<账户键>/`。
#[must_use]
pub fn marker_dir(config_dir: &Path, owner: &BindingOwner) -> PathBuf {
    config_dir.join("accounts").join(account_key(owner))
}

/// 旧版放在应用配置目录根下、不分账户的提醒标记（未发布即硬切，不迁移）。删掉的代价只是
/// 同一处境可能再提醒一次。文件名与新版各账户目录里的同名（名字只在各自的模块里写一处）。
const LEGACY_MARKER_FILES: [&str; 3] = [
    crate::sync_service::GRACE_REMINDER_FILE,
    crate::cloud::BILLING_NOTICE_FILE,
    crate::cloud::USAGE_NOTICE_FILE,
];

/// 删掉旧版不分账户的提醒标记（启动时一次）。只删这三个文件名，别的一概不碰。
pub fn remove_legacy_markers(config_dir: &Path) {
    for name in LEGACY_MARKER_FILES {
        let _ = std::fs::remove_file(config_dir.join(name));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner(bucket: &str, id: &str) -> BindingOwner {
        BindingOwner {
            server_bucket: bucket.into(),
            account_id: id.into(),
            account_label: String::new(),
        }
    }

    #[test]
    fn the_key_is_stable_path_safe_and_separates_accounts_and_servers() {
        let a = account_key(&owner("api.lunarca.app", "acc-a"));
        assert_eq!(a, account_key(&owner("api.lunarca.app", "acc-a")), "确定性");
        assert!(a.starts_with(KEY_PREFIX));
        assert_eq!(a.len(), KEY_PREFIX.len() + 32);
        assert!(a.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'));
        assert_ne!(a, account_key(&owner("api.lunarca.app", "acc-b")), "换账户");
        assert_ne!(
            a,
            account_key(&owner("10.0.0.62:8443", "acc-a")),
            "换服务器"
        );
        assert_ne!(
            account_key(&owner("a", "bc")),
            account_key(&owner("ab", "c"))
        );
        // 打码的名字不进键：邮箱改了还是同一个账户。
        let mut renamed = owner("api.lunarca.app", "acc-a");
        renamed.account_label = "x***@y".into();
        assert_eq!(a, account_key(&renamed));
    }

    #[test]
    fn account_names_are_masked_by_character() {
        assert_eq!(mask_account_name("johnny@gmail.com"), "jo…ny@gmail.com");
        assert_eq!(mask_account_name("abcd@x.org"), "a…@x.org");
        assert_eq!(mask_account_name("ab@x.org"), "a…@x.org");
        assert_eq!(mask_account_name("a@x.org"), "a…@x.org");
        assert_eq!(mask_account_name("abcde@x.org"), "ab…de@x.org");
        assert_eq!(mask_account_name("张三丰李四@例子.cn"), "张三…李四@例子.cn");
        assert_eq!(mask_account_name("张三丰@例子.cn"), "张…@例子.cn");
        assert_eq!(mask_account_name("émilie@exemple.fr"), "ém…ie@exemple.fr");
        assert_eq!(mask_account_name("operator"), "op…or");
        assert_eq!(mask_account_name(""), "…");
    }

    /// 同前缀的两个账户打码后必须分得开（真机 2026-10-15：两个账户都成了 `jo***@gmail.com`）。
    #[test]
    fn accounts_sharing_a_prefix_stay_distinguishable() {
        let a = mask_account_name("johnnymai5788@gmail.com");
        let b = mask_account_name("johnnyzhao5619@gmail.com");
        assert_eq!(a, "jo…88@gmail.com");
        assert_eq!(b, "jo…19@gmail.com");
        assert_ne!(a, b);
    }

    #[test]
    fn markers_live_under_the_account_and_legacy_ones_are_removed() {
        let dir = tempfile::tempdir().unwrap();
        for name in LEGACY_MARKER_FILES {
            std::fs::write(dir.path().join(name), b"{}").unwrap();
        }
        std::fs::write(dir.path().join("sync-pause.json"), b"{}").unwrap();
        remove_legacy_markers(dir.path());
        for name in LEGACY_MARKER_FILES {
            assert!(!dir.path().join(name).exists());
        }
        assert!(
            dir.path().join("sync-pause.json").exists(),
            "全部暂停是设备级的，不动"
        );
        assert_ne!(
            marker_dir(dir.path(), &owner("s", "a")),
            marker_dir(dir.path(), &owner("s", "b"))
        );
        assert!(marker_dir(dir.path(), &owner("s", "a")).starts_with(dir.path().join("accounts")));
    }
}
