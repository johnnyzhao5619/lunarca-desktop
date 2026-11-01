//! macOS GPU 利用率（IOKit `IOAccelerator` → `PerformanceStatistics` → `Device Utilization %`）。
//!
//! 状态栏资源监控用（PRD-04-033）。此路径**无需 root**，读集成/独立 GPU 的实时利用率
//! （`asitop`/`macmon` 等同源思路）。读取失败（key 缺失/机型差异）优雅返回 `None`，状态栏
//! 据此仅显示 CPU——绝不因监控失败影响主流程。

#![allow(non_camel_case_types)]

use std::os::raw::{c_char, c_void};
// c_char 用于 extern IOServiceMatching 的入参类型（c"..." 字面量经 CStr::as_ptr 得 *const c_char）。

use core_foundation::base::{CFType, CFTypeRef, TCFType};
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef, CFMutableDictionaryRef};
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};

type kern_return_t = i32;
type mach_port_t = u32;
type io_object_t = mach_port_t;
type io_iterator_t = mach_port_t;
type io_registry_entry_t = mach_port_t;

const KERN_SUCCESS: kern_return_t = 0;
/// `kIOMainPortDefault`（= `kIOMasterPortDefault` = 0）。
const K_IO_MAIN_PORT_DEFAULT: mach_port_t = 0;

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOServiceMatching(name: *const c_char) -> CFMutableDictionaryRef;
    fn IOServiceGetMatchingServices(
        main_port: mach_port_t,
        matching: CFDictionaryRef,
        existing: *mut io_iterator_t,
    ) -> kern_return_t;
    fn IOIteratorNext(iterator: io_iterator_t) -> io_object_t;
    fn IORegistryEntryCreateCFProperty(
        entry: io_registry_entry_t,
        key: CFStringRef,
        allocator: *const c_void,
        options: u32,
    ) -> CFTypeRef;
    fn IOObjectRelease(object: io_object_t) -> kern_return_t;
}

/// 当前 GPU 利用率（0–100）。取所有 `IOAccelerator` 服务中 `Device Utilization %` 的最大值。
#[must_use]
pub fn utilization_percent() -> Option<f32> {
    // SAFETY: 标准 IOKit 只读查询；所有 CF 对象经 core-foundation 安全包装（create/get 规则），
    // io_object 显式 IOObjectRelease；matching 字典所有权由 IOServiceGetMatchingServices 接管。
    unsafe {
        let matching = IOServiceMatching(c"IOAccelerator".as_ptr());
        if matching.is_null() {
            return None;
        }
        let mut iterator: io_iterator_t = 0;
        if IOServiceGetMatchingServices(
            K_IO_MAIN_PORT_DEFAULT,
            matching as CFDictionaryRef,
            &mut iterator,
        ) != KERN_SUCCESS
        {
            return None;
        }

        let perf_key = CFString::new("PerformanceStatistics");
        let mut best: Option<f32> = None;
        loop {
            let service = IOIteratorNext(iterator);
            if service == 0 {
                break;
            }
            let prop = IORegistryEntryCreateCFProperty(
                service,
                perf_key.as_concrete_TypeRef(),
                std::ptr::null(),
                0,
            );
            if !prop.is_null() {
                let prop_type = CFType::wrap_under_create_rule(prop);
                if let Some(dict) = prop_type.downcast::<CFDictionary>() {
                    if let Some(util) = read_device_utilization(&dict) {
                        best = Some(best.map_or(util, |b| b.max(util)));
                    }
                }
            }
            IOObjectRelease(service);
        }
        IOObjectRelease(iterator);
        best
    }
}

/// 从 `PerformanceStatistics` 字典读 `Device Utilization %`（CFNumber → f32，0–100）。
///
/// `cmp_owned` allow：`CFString` 未实现 `PartialEq<str>`，比较前必须 `to_string()` 得 owned。
#[allow(clippy::cmp_owned)]
fn read_device_utilization(dict: &CFDictionary) -> Option<f32> {
    let (keys, values) = dict.get_keys_and_values();
    for (k, v) in keys.into_iter().zip(values) {
        // keys/values 为 CFTypeRef（core-foundation 中 = *const c_void），经 CFType 安全包装再类型判定。
        let key = unsafe { CFType::wrap_under_get_rule(k as CFTypeRef) };
        let Some(key_str) = key.downcast::<CFString>() else {
            continue;
        };
        if key_str.to_string() != "Device Utilization %" {
            continue;
        }
        let value = unsafe { CFType::wrap_under_get_rule(v as CFTypeRef) };
        if let Some(num) = value.downcast::<CFNumber>() {
            let pct = num
                .to_i64()
                .map(|n| n as f32)
                .or_else(|| num.to_f64().map(|f| f as f32))?;
            return Some(pct.clamp(0.0, 100.0));
        }
    }
    None
}
