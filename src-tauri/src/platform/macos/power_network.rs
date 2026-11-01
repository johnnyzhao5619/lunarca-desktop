//! 同步大文件的暂停条件（ADR-156 ③）：按流量计费的网络与电量低。
//!
//! - 网络：Network.framework 的 `nw_path_monitor` 常驻监听当前路径，`is_expensive`（手机热点、
//!   蜂窝）或 `is_constrained`（低数据模式）即按流量计费。回调在系统队列里更新一个原子量，
//!   同步每轮读它——不为一次判定去等系统回调。
//! - 电量：IOKit 电源信息（`IOPSCopyPowerSourcesInfo`）读供电方式与电量；低电量模式读
//!   `NSProcessInfo.isLowPowerModeEnabled`。
//!
//! 只读、无需任何授权；读不到一律当「不暂停」（宁可多传一次，也不让同步无故停住）。

#![allow(non_upper_case_globals)]

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use core_foundation::array::{CFArray, CFArrayRef};
use core_foundation::base::{CFType, CFTypeRef, TCFType};
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use objc2::runtime::AnyObject;
use objc2::{class, msg_send};

/// 当前网络路径按流量计费（由监听回调写）。
static METERED: AtomicBool = AtomicBool::new(false);
/// 监听只起一次，随进程常驻（指针以整数存，进程退出前不释放）。
static MONITOR: OnceLock<usize> = OnceLock::new();

#[link(name = "Network", kind = "framework")]
extern "C" {
    fn nw_path_monitor_create() -> *mut c_void;
    fn nw_path_monitor_set_queue(monitor: *mut c_void, queue: *mut c_void);
    fn nw_path_monitor_set_update_handler(
        monitor: *mut c_void,
        handler: &block2::Block<dyn Fn(*mut c_void)>,
    );
    fn nw_path_monitor_start(monitor: *mut c_void);
    fn nw_path_is_expensive(path: *mut c_void) -> bool;
    fn nw_path_is_constrained(path: *mut c_void) -> bool;
}

extern "C" {
    fn dispatch_get_global_queue(identifier: isize, flags: usize) -> *mut c_void;
}

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOPSCopyPowerSourcesInfo() -> CFTypeRef;
    fn IOPSCopyPowerSourcesList(blob: CFTypeRef) -> CFArrayRef;
    fn IOPSGetPowerSourceDescription(blob: CFTypeRef, source: CFTypeRef) -> CFDictionaryRef;
}

/// 起网络路径监听（幂等）。
pub fn start_network_monitor() {
    MONITOR.get_or_init(|| {
        // SAFETY: Network.framework 的 C 接口；回调块由框架拷贝持有；监听对象随进程常驻。
        unsafe {
            let monitor = nw_path_monitor_create();
            if monitor.is_null() {
                return 0;
            }
            let handler = block2::RcBlock::new(|path: *mut c_void| {
                let metered =
                    !path.is_null() && (nw_path_is_expensive(path) || nw_path_is_constrained(path));
                METERED.store(metered, Ordering::Relaxed);
            });
            nw_path_monitor_set_update_handler(monitor, &handler);
            nw_path_monitor_set_queue(monitor, dispatch_get_global_queue(0, 0));
            nw_path_monitor_start(monitor);
            monitor as usize
        }
    });
}

/// 当前网络按流量计费（监听还没起来 / 还没回调过 = 否）。
#[must_use]
pub fn network_is_metered() -> bool {
    METERED.load(Ordering::Relaxed)
}

/// 低电量模式开着。
#[must_use]
pub fn low_power_mode() -> bool {
    // SAFETY: `+[NSProcessInfo processInfo]` 恒返回单例；`isLowPowerModeEnabled` 是 BOOL 属性。
    unsafe {
        let info: *mut AnyObject = msg_send![class!(NSProcessInfo), processInfo];
        if info.is_null() {
            return false;
        }
        msg_send![info, isLowPowerModeEnabled]
    }
}

/// 此刻是不是用电池供电、电量多少（百分比）。读不到 = `None`（台式机没有电池也是 `None`）。
#[must_use]
pub fn battery() -> Option<(bool, u8)> {
    // SAFETY: IOKit 电源信息只读查询；Copy 函数的返回按 create 规则包装，Get 函数按 get 规则。
    unsafe {
        let blob = IOPSCopyPowerSourcesInfo();
        if blob.is_null() {
            return None;
        }
        let blob = CFType::wrap_under_create_rule(blob);
        let list = IOPSCopyPowerSourcesList(blob.as_CFTypeRef());
        if list.is_null() {
            return None;
        }
        let list: CFArray<CFType> = CFArray::wrap_under_create_rule(list);
        for source in list.iter() {
            let description =
                IOPSGetPowerSourceDescription(blob.as_CFTypeRef(), source.as_CFTypeRef());
            if description.is_null() {
                continue;
            }
            let description: CFDictionary = CFDictionary::wrap_under_get_rule(description);
            let state = string_value(&description, "Power Source State");
            let current = number_value(&description, "Current Capacity");
            let max = number_value(&description, "Max Capacity");
            if let (Some(state), Some(current), Some(max)) = (state, current, max) {
                if max > 0 {
                    let percent = (current * 100 / max).clamp(0, 100) as u8;
                    return Some((state == "Battery Power", percent));
                }
            }
        }
        None
    }
}

/// 字典里某个键的值（键与值都按 get 规则看，不改引用计数）。
#[allow(clippy::cmp_owned)]
fn lookup(dict: &CFDictionary, wanted: &str) -> Option<CFType> {
    let (keys, values) = dict.get_keys_and_values();
    for (k, v) in keys.into_iter().zip(values) {
        // SAFETY: 键值来自字典自身，按 get 规则包装。
        let key = unsafe { CFType::wrap_under_get_rule(k as CFTypeRef) };
        if key
            .downcast::<CFString>()
            .is_some_and(|s| s.to_string() == wanted)
        {
            return Some(unsafe { CFType::wrap_under_get_rule(v as CFTypeRef) });
        }
    }
    None
}

fn string_value(dict: &CFDictionary, key: &str) -> Option<String> {
    lookup(dict, key)?
        .downcast::<CFString>()
        .map(|s| s.to_string())
}

fn number_value(dict: &CFDictionary, key: &str) -> Option<i64> {
    lookup(dict, key)?.downcast::<CFNumber>()?.to_i64()
}
