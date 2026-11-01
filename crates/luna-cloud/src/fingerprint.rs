//! 设备指纹（纵切 3 C5，协议 v13；用途与隐私边界见 PRD-13-018
//! 与 TDD-12 §10）。
//!
//! # 它是什么
//!
//! `device_fingerprint` = `SHA-256(FINGERPRINT_SALT ‖ 硬件标识)` 的 64 位小写十六进制。硬件标识按平台取：
//!
//! - **macOS**：IOKit 注册表里 `IOPlatformExpertDevice` 的 `IOPlatformUUID`（只读查询，不弹任何权限框）；
//! - **Windows**：注册表 `HKLM\SOFTWARE\Microsoft\Cryptography` 的 `MachineGuid`（64 位视图）；
//! - **其余平台**，以及上面两处读不出 / 读出全零这类占位值时：应用数据目录里一份随机 id
//!   （[`FALLBACK_ID_FILE`]，首次需要时生成，之后原样读回）。
//!
//! # 它不是什么
//!
//! - 不是硬件标识本身：原始值只在本进程内存里活到算完散列为止，**不落盘、不打日志、不显示、不过 IPC**；
//!   服务端只见散列（盐是本应用的常量，于是同一台机器在别的应用里的散列对不上这一个）。
//! - 不是分析或个性化的依据：只随建设备会话的登录请求发给**登录的那一台服务器**，用途只有「免费额度挂
//!   账户 × 设备」与设备上限（安全用途），不发往任何别处。
//!
//! # 退路的代价（如实写下）
//!
//! 读不出硬件标识时退到持久化的随机 id：重装应用 / 清掉应用数据会得到一个新指纹，于是同一台机器
//! 可能被当成另一台——反滥用在这台机器上变弱，但**绝不会误伤正常用户**（不会把两台真机当成一台）。
//! 随机 id 也写不进时（数据目录只读），本进程内随机生成一枚、只活到退出：每次启动都像一台新设备，
//! 代价同上，同样不误伤。

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use sha2::{Digest, Sha256};

/// 散列前缀在硬件标识前的盐（本应用专属常量）。**改它 = 每台设备的指纹整体换一遍**，服务端的
/// 「这台设备领过免费额度」记录随之全部失联——只能随协议版本一起改。
pub const FINGERPRINT_SALT: &[u8] = b"lunarca/device-fingerprint/v1\0";

/// 读不出硬件标识时，退路随机 id 落在应用数据目录下的文件名。
pub const FALLBACK_ID_FILE: &str = "device-fingerprint-fallback";

/// 一枚设备指纹。`Debug` 不打印值：它是安全用途的标识，任何日志与错误串里都不该出现。
#[derive(Clone, PartialEq, Eq)]
pub struct DeviceFingerprint(String);

impl DeviceFingerprint {
    /// 发往服务端的那一格（64 位小写十六进制）。
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for DeviceFingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DeviceFingerprint(<redacted>)")
    }
}

/// 硬件标识的来源（生产 = [`platform_hardware_id`]；测试注入固定值或 `None`）。
pub type HardwareIdSource = Arc<dyn Fn() -> Option<String> + Send + Sync + 'static>;

/// 散列一个硬件标识（或退路 id）。
#[must_use]
pub fn derive(hardware_id: &str) -> DeviceFingerprint {
    let mut hasher = Sha256::new();
    hasher.update(FINGERPRINT_SALT);
    hasher.update(hardware_id.as_bytes());
    DeviceFingerprint(
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    )
}

/// 读出来的值能不能当硬件标识用：空串、全零（部分虚拟机与刷过的主板给的占位 UUID）、全 F 一律不算
/// ——拿占位值散列，会让成千上万台机器共享同一枚指纹，正好把「账户 × 设备」变成误伤。
fn usable(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let digits: String = trimmed
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect();
    if digits.is_empty()
        || digits.chars().all(|c| c == '0')
        || digits.chars().all(|c| c.eq_ignore_ascii_case(&'f'))
    {
        return None;
    }
    Some(trimmed.to_owned())
}

/// 退路随机 id：在场就读回，不在场就生成并落盘；落不了盘就只活在本进程。
fn fallback_id(path: Option<&Path>) -> String {
    if let Some(existing) = path
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|raw| usable(&raw))
    {
        return existing;
    }
    let fresh = uuid::Uuid::new_v4().simple().to_string();
    if let Some(path) = path {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // 先写临时文件再改名：写到一半退出不会留下一个半截 id（下次读回就成了另一台设备）。
        let staging = path.with_extension("tmp");
        let written =
            std::fs::write(&staging, &fresh).and_then(|()| std::fs::rename(&staging, path));
        if written.is_err() {
            let _ = std::fs::remove_file(&staging);
            // 不带路径与值：只说发生了什么、代价是什么。
            eprintln!("[luna-cloud] 设备指纹退路 id 写不进应用数据目录，本次运行内临时使用");
        }
    }
    fresh
}

/// 取硬件标识（读不出或是占位值时退到 [`fallback_id`]），散列成指纹。
#[must_use]
pub fn resolve(
    source: &dyn Fn() -> Option<String>,
    fallback_path: Option<&Path>,
) -> DeviceFingerprint {
    match source().and_then(|raw| usable(&raw)) {
        Some(hardware_id) => derive(&hardware_id),
        None => derive(&fallback_id(fallback_path)),
    }
}

/// 进程内的指纹提供者：第一次要用时算一次，之后原样复用（同一进程里不会因为退路文件被删而变）。
pub struct FingerprintProvider {
    source: HardwareIdSource,
    fallback_path: Mutex<Option<PathBuf>>,
    cached: OnceLock<DeviceFingerprint>,
}

impl FingerprintProvider {
    /// 生产形态：按平台读硬件标识。
    #[must_use]
    pub fn platform() -> Self {
        Self::with_source(Arc::new(platform_hardware_id))
    }

    /// 注入硬件标识来源（测试用；生产用 [`Self::platform`]）。
    #[must_use]
    pub fn with_source(source: HardwareIdSource) -> Self {
        Self {
            source,
            fallback_path: Mutex::new(None),
            cached: OnceLock::new(),
        }
    }

    /// 注入退路 id 的落盘位置（插件 setup 调用；不注入 = 退路只活在本进程）。
    pub fn attach_fallback_path(&self, path: PathBuf) {
        *self
            .fallback_path
            .lock()
            .expect("fingerprint path mutex poisoned") = Some(path);
    }

    /// 这台设备的指纹。
    #[must_use]
    pub fn get(&self) -> DeviceFingerprint {
        self.cached
            .get_or_init(|| {
                let path = self
                    .fallback_path
                    .lock()
                    .expect("fingerprint path mutex poisoned")
                    .clone();
                resolve(self.source.as_ref(), path.as_deref())
            })
            .clone()
    }
}

/// 本平台的硬件标识（原始值，**只在本进程内存里**用来算散列）。
#[must_use]
pub fn platform_hardware_id() -> Option<String> {
    platform::hardware_id()
}

#[cfg(target_os = "macos")]
mod platform {
    //! IOKit：`IOPlatformExpertDevice` 的 `IOPlatformUUID`（与 `ioreg -rd1 -c IOPlatformExpertDevice`
    //! 同源，只读、无需任何权限）。

    #![allow(non_camel_case_types)]

    use std::os::raw::{c_char, c_void};

    use core_foundation::base::{CFType, CFTypeRef, TCFType};
    use core_foundation::dictionary::{CFDictionaryRef, CFMutableDictionaryRef};
    use core_foundation::string::{CFString, CFStringRef};

    type kern_return_t = i32;
    type mach_port_t = u32;
    type io_object_t = mach_port_t;
    type io_service_t = io_object_t;
    type io_registry_entry_t = io_object_t;

    /// `kIOMainPortDefault`（= `kIOMasterPortDefault` = 0）。
    const K_IO_MAIN_PORT_DEFAULT: mach_port_t = 0;

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOServiceMatching(name: *const c_char) -> CFMutableDictionaryRef;
        fn IOServiceGetMatchingService(
            main_port: mach_port_t,
            matching: CFDictionaryRef,
        ) -> io_service_t;
        fn IORegistryEntryCreateCFProperty(
            entry: io_registry_entry_t,
            key: CFStringRef,
            allocator: *const c_void,
            options: u32,
        ) -> CFTypeRef;
        fn IOObjectRelease(object: io_object_t) -> kern_return_t;
    }

    pub(super) fn hardware_id() -> Option<String> {
        // SAFETY: 标准 IOKit 只读查询。matching 字典的所有权由 IOServiceGetMatchingService 接管
        // （无论成败）；取回的 CF 属性按 create 规则包装、离开作用域即释放；service 显式 release。
        unsafe {
            let matching = IOServiceMatching(c"IOPlatformExpertDevice".as_ptr());
            if matching.is_null() {
                return None;
            }
            let service = IOServiceGetMatchingService(K_IO_MAIN_PORT_DEFAULT, matching as _);
            if service == 0 {
                return None;
            }
            let key = CFString::new("IOPlatformUUID");
            let property = IORegistryEntryCreateCFProperty(
                service,
                key.as_concrete_TypeRef(),
                std::ptr::null(),
                0,
            );
            IOObjectRelease(service);
            if property.is_null() {
                return None;
            }
            CFType::wrap_under_create_rule(property)
                .downcast::<CFString>()
                .map(|uuid| uuid.to_string())
        }
    }
}

#[cfg(target_os = "windows")]
mod platform {
    //! 注册表 `HKLM\SOFTWARE\Microsoft\Cryptography\MachineGuid`（安装系统时生成；读 64 位视图，
    //! 免得 32 位进程被重定向到 WOW6432Node 读到另一份）。

    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY};
    use winreg::RegKey;

    pub(super) fn hardware_id() -> Option<String> {
        RegKey::predef(HKEY_LOCAL_MACHINE)
            .open_subkey_with_flags(
                r"SOFTWARE\Microsoft\Cryptography",
                KEY_READ | KEY_WOW64_64KEY,
            )
            .ok()?
            .get_value::<String, _>("MachineGuid")
            .ok()
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod platform {
    //! 其余平台不读硬件标识：一律走持久化的随机 id（模块头注释里的代价同样适用）。

    pub(super) fn hardware_id() -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_lower_hex_64(value: &str) -> bool {
        value.len() == 64
            && value
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
    }

    #[test]
    fn a_fingerprint_is_64_lowercase_hex_of_the_salted_id() {
        let fingerprint = derive("ABCDEF01-2345-6789-ABCD-EF0123456789");
        assert!(
            is_lower_hex_64(fingerprint.as_str()),
            "{}",
            fingerprint.as_str()
        );
        // 同一个标识恒得同一枚；换一个标识就换一枚。
        assert_eq!(fingerprint, derive("ABCDEF01-2345-6789-ABCD-EF0123456789"));
        assert_ne!(fingerprint, derive("ABCDEF01-2345-6789-ABCD-EF012345678A"));
        // 加了盐：不是裸标识的 SHA-256（别的应用拿同一个硬件标识算不出这一枚）。
        let bare: String = Sha256::digest(b"ABCDEF01-2345-6789-ABCD-EF0123456789")
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_ne!(fingerprint.as_str(), bare);
        // 散列里看不出原值。
        assert!(!fingerprint.as_str().contains("abcdef01"));
    }

    #[test]
    fn debug_output_never_carries_the_value() {
        let fingerprint = derive("some-hardware-id");
        let printed = format!("{fingerprint:?}");
        assert!(!printed.contains(fingerprint.as_str()));
        assert!(printed.contains("redacted"));
    }

    #[test]
    fn a_readable_hardware_id_wins_and_is_stable_across_calls() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FALLBACK_ID_FILE);
        let source = || Some("C02XYZ-UUID-1".to_owned());
        let first = resolve(&source, Some(&path));
        assert_eq!(first, derive("C02XYZ-UUID-1"));
        assert_eq!(first, resolve(&source, Some(&path)));
        assert!(!path.exists(), "读得出硬件标识时不该写退路文件");
    }

    #[test]
    fn placeholder_ids_fall_back_instead_of_colliding() {
        for placeholder in [
            "",
            "   ",
            "00000000-0000-0000-0000-000000000000",
            "FFFFFFFF-FFFF-FFFF-FFFF-FFFFFFFFFFFF",
        ] {
            assert_eq!(usable(placeholder), None, "{placeholder:?}");
        }
        assert_eq!(usable(" 1234-abcd \n").as_deref(), Some("1234-abcd"));
    }

    #[test]
    fn an_unreadable_hardware_id_falls_back_to_a_persisted_random_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join(FALLBACK_ID_FILE);
        let unreadable = || None;
        let first = resolve(&unreadable, Some(&path));
        assert!(is_lower_hex_64(first.as_str()));
        assert!(
            path.exists(),
            "退路 id 必须落盘，否则每次启动都像一台新设备"
        );
        // 重启（新的提供者）读回同一枚。
        assert_eq!(first, resolve(&unreadable, Some(&path)));
        // 占位值同样走退路，并与之前那枚一致。
        let placeholder = || Some("00000000-0000-0000-0000-000000000000".to_owned());
        assert_eq!(first, resolve(&placeholder, Some(&path)));
        // 退路文件里存的是随机 id 本身，不是指纹（指纹只在请求里出现）。
        let stored = std::fs::read_to_string(&path).unwrap();
        assert_ne!(stored, first.as_str());
        assert_eq!(first, derive(&stored));
    }

    #[test]
    fn without_a_writable_place_the_fallback_lives_for_the_process() {
        let provider = FingerprintProvider::with_source(Arc::new(|| None));
        let first = provider.get();
        assert!(is_lower_hex_64(first.as_str()));
        assert_eq!(first, provider.get(), "同一进程里不变");
    }

    #[test]
    fn the_provider_computes_once() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        let provider = FingerprintProvider::with_source(Arc::new(move || {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Some("machine-guid".to_owned())
        }));
        assert_eq!(provider.get(), derive("machine-guid"));
        assert_eq!(provider.get(), derive("machine-guid"));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// 本机真实读一次（macOS 上必须读得出；不打印值）。
    #[cfg(target_os = "macos")]
    #[test]
    fn this_platform_has_a_hardware_id() {
        assert!(platform_hardware_id()
            .and_then(|raw| usable(&raw))
            .is_some());
    }
}
