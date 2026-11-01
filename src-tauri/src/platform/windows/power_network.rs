//! 同步大文件的暂停条件（ADR-156 ③）在 Windows 上的读法：按流量计费的网络与电量低。
//!
//! - 网络：WinRT `NetworkInformation::GetInternetConnectionProfile().GetConnectionCost()`。
//!   费用类型 Fixed / Variable（按量或有套餐上限）、漫游、接近或超出流量上限，都算按流量计费。
//!   `NetworkStatusChanged` 常驻订阅，网络一变就重读一次、写进原子量，同步每轮读它；订阅不上
//!   就退回每次现读。
//! - 电量：`GetSystemPowerStatus` 读供电方式（`ACLineStatus`）、电量（`BatteryLifePercent`）与
//!   节电模式（`SystemStatusFlag` 的节电位）。
//!
//! 只读、无需任何授权；读不到一律当「不暂停」（宁可多传一次，也不让同步无故停住）。
//!
//! 判定写成纯函数（[`cost_is_metered`]、[`battery_of`]、[`saver_on`]），在任何平台上单测；
//! 系统调用只在 `cfg(target_os = "windows")` 的薄层里。

/// 网络费用类型（WinRT `NetworkCostType`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CostType {
    /// 系统说不清。
    Unknown,
    /// 不限量。
    Unrestricted,
    /// 有套餐上限（超出前不额外收费）。
    Fixed,
    /// 按量计费。
    Variable,
}

impl CostType {
    /// 由 `NetworkCostType` 的原始值得到；不认识的值当 [`CostType::Unknown`]。
    #[must_use]
    pub fn from_raw(raw: i32) -> Self {
        match raw {
            1 => Self::Unrestricted,
            2 => Self::Fixed,
            3 => Self::Variable,
            _ => Self::Unknown,
        }
    }
}

/// 一次网络费用读数（WinRT `ConnectionCost` 的四个字段）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CostReading {
    pub cost_type: CostType,
    pub roaming: bool,
    pub approaching_data_limit: bool,
    pub over_data_limit: bool,
}

/// 按流量计费的判定（纯逻辑）：Fixed / Variable、漫游、接近或超出上限任一成立即是；
/// 读不到（`None`）或费用类型说不清且其余三项都没有，算不计费。
#[must_use]
pub fn cost_is_metered(reading: Option<CostReading>) -> bool {
    reading.is_some_and(|r| {
        matches!(r.cost_type, CostType::Fixed | CostType::Variable)
            || r.roaming
            || r.approaching_data_limit
            || r.over_data_limit
    })
}

/// 一次电源读数（`SYSTEM_POWER_STATUS` 里判定要用的四个字节）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowerReading {
    /// 0 = 用电池，1 = 插着电，255 = 未知。
    pub ac_line_status: u8,
    /// 位标志；128 = 没有电池，255 = 未知。
    pub battery_flag: u8,
    /// 0–100；255 = 未知。
    pub battery_life_percent: u8,
    /// 1 = 节电模式开着。
    pub system_status_flag: u8,
}

/// `ACLineStatus`：用电池供电。
const AC_OFFLINE: u8 = 0;
/// `BatteryFlag`：读不到电池状态。
const BATTERY_FLAG_UNKNOWN: u8 = 255;
/// `BatteryFlag`：这台机器没有电池。
const BATTERY_FLAG_NO_BATTERY: u8 = 128;
/// `SystemStatusFlag`：节电模式开着。
const SYSTEM_STATUS_SAVER_ON: u8 = 1;

/// 此刻是不是用电池供电、电量多少（纯逻辑，与 macOS 同形）。没有电池、电池状态或电量读不到
/// = `None`；供电方式未知不算用电池。
#[must_use]
pub fn battery_of(reading: PowerReading) -> Option<(bool, u8)> {
    if reading.battery_flag == BATTERY_FLAG_UNKNOWN
        || reading.battery_flag & BATTERY_FLAG_NO_BATTERY != 0
        || reading.battery_life_percent > 100
    {
        return None;
    }
    Some((
        reading.ac_line_status == AC_OFFLINE,
        reading.battery_life_percent,
    ))
}

/// 节电模式开着（纯逻辑）。
#[must_use]
pub fn saver_on(reading: PowerReading) -> bool {
    reading.system_status_flag & SYSTEM_STATUS_SAVER_ON != 0
}

#[cfg(target_os = "windows")]
pub use sys::{battery, low_power_mode, network_is_metered, start_network_monitor};

/// 系统调用薄层：只负责把读数取出来，判定全交给上面的纯函数。
#[cfg(target_os = "windows")]
mod sys {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::OnceLock;

    use ::windows::Networking::Connectivity::{
        NetworkInformation, NetworkStatusChangedEventHandler,
    };
    use ::windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};

    use super::{battery_of, cost_is_metered, saver_on, CostReading, CostType, PowerReading};

    /// 当前网络按流量计费（由订阅回调写）。
    static METERED: AtomicBool = AtomicBool::new(false);
    /// 订阅成功：之后读原子量；订阅不上则每次现读。
    static WATCHING: AtomicBool = AtomicBool::new(false);
    /// 订阅只起一次，随进程常驻（令牌不注销）。
    static MONITOR: OnceLock<()> = OnceLock::new();

    /// 起网络变化订阅（幂等）。在独立线程里注册：WinRT 静态工厂在没初始化 COM 的线程上会自行
    /// 进多线程套间，回调落在系统线程池里，不占主线程。
    pub fn start_network_monitor() {
        MONITOR.get_or_init(|| {
            let _ = std::thread::Builder::new()
                .name("luna-network-cost".into())
                .spawn(|| {
                    refresh();
                    let handler = NetworkStatusChangedEventHandler::new(|_| {
                        refresh();
                        Ok(())
                    });
                    if NetworkInformation::NetworkStatusChanged(&handler).is_ok() {
                        WATCHING.store(true, Ordering::Release);
                    }
                });
        });
    }

    fn refresh() {
        METERED.store(cost_is_metered(read_cost()), Ordering::Relaxed);
    }

    /// 当前联网配置的费用；没联网（没有 Internet 配置）或读不到 = `None`。
    fn read_cost() -> Option<CostReading> {
        let profile = NetworkInformation::GetInternetConnectionProfile().ok()?;
        let cost = profile.GetConnectionCost().ok()?;
        Some(CostReading {
            cost_type: cost
                .NetworkCostType()
                .map_or(CostType::Unknown, |t| CostType::from_raw(t.0)),
            roaming: cost.Roaming().unwrap_or(false),
            approaching_data_limit: cost.ApproachingDataLimit().unwrap_or(false),
            over_data_limit: cost.OverDataLimit().unwrap_or(false),
        })
    }

    /// 当前网络按流量计费。
    #[must_use]
    pub fn network_is_metered() -> bool {
        if WATCHING.load(Ordering::Acquire) {
            METERED.load(Ordering::Relaxed)
        } else {
            cost_is_metered(read_cost())
        }
    }

    fn read_power() -> Option<PowerReading> {
        let mut status = SYSTEM_POWER_STATUS::default();
        // SAFETY: 只读查询，写入调用方提供的结构体。
        unsafe { GetSystemPowerStatus(&mut status) }.ok()?;
        Some(PowerReading {
            ac_line_status: status.ACLineStatus,
            battery_flag: status.BatteryFlag,
            battery_life_percent: status.BatteryLifePercent,
            system_status_flag: status.SystemStatusFlag,
        })
    }

    /// 节电模式开着（对应 macOS 的低电量模式）。
    #[must_use]
    pub fn low_power_mode() -> bool {
        read_power().is_some_and(saver_on)
    }

    /// 此刻是不是用电池供电、电量多少（百分比）。读不到 = `None`（台式机没有电池也是 `None`）。
    #[must_use]
    pub fn battery() -> Option<(bool, u8)> {
        read_power().and_then(battery_of)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cost(cost_type: CostType) -> CostReading {
        CostReading {
            cost_type,
            roaming: false,
            approaching_data_limit: false,
            over_data_limit: false,
        }
    }

    #[test]
    fn fixed_variable_roaming_and_data_limits_count_as_metered() {
        assert!(cost_is_metered(Some(cost(CostType::Fixed))));
        assert!(cost_is_metered(Some(cost(CostType::Variable))));
        for flag in 0..3 {
            let mut reading = cost(CostType::Unrestricted);
            match flag {
                0 => reading.roaming = true,
                1 => reading.approaching_data_limit = true,
                _ => reading.over_data_limit = true,
            }
            assert!(cost_is_metered(Some(reading)), "第 {flag} 项");
        }
    }

    #[test]
    fn unrestricted_unknown_or_unreadable_never_pauses() {
        assert!(!cost_is_metered(Some(cost(CostType::Unrestricted))));
        assert!(!cost_is_metered(Some(cost(CostType::Unknown))));
        assert!(!cost_is_metered(None));
    }

    #[test]
    fn raw_cost_types_map_and_unknown_values_fall_back() {
        assert_eq!(CostType::from_raw(0), CostType::Unknown);
        assert_eq!(CostType::from_raw(1), CostType::Unrestricted);
        assert_eq!(CostType::from_raw(2), CostType::Fixed);
        assert_eq!(CostType::from_raw(3), CostType::Variable);
        assert_eq!(CostType::from_raw(42), CostType::Unknown);
    }

    fn power(ac: u8, flag: u8, percent: u8, status: u8) -> PowerReading {
        PowerReading {
            ac_line_status: ac,
            battery_flag: flag,
            battery_life_percent: percent,
            system_status_flag: status,
        }
    }

    #[test]
    fn battery_reads_supply_and_percent() {
        assert_eq!(battery_of(power(0, 2, 15, 0)), Some((true, 15)));
        assert_eq!(battery_of(power(1, 8, 15, 0)), Some((false, 15)));
        // 供电方式未知：不算用电池。
        assert_eq!(battery_of(power(255, 1, 80, 0)), Some((false, 80)));
    }

    #[test]
    fn no_battery_or_unknown_readings_are_none() {
        assert_eq!(battery_of(power(1, 128, 255, 0)), None, "台式机没有电池");
        assert_eq!(battery_of(power(0, 255, 50, 0)), None, "电池状态未知");
        assert_eq!(battery_of(power(0, 2, 255, 0)), None, "电量未知");
    }

    #[test]
    fn saver_bit_is_low_power_mode() {
        assert!(saver_on(power(1, 8, 90, 1)));
        assert!(!saver_on(power(1, 8, 90, 0)));
    }

    /// 与跨平台的电量门槛合起来：用电池且低于 20%、或节电模式开着才暂停；插着电不算。
    #[test]
    fn combined_with_the_shared_threshold() {
        use crate::platform::battery_is_low;
        let low = power(0, 2, 10, 0);
        assert!(battery_is_low(battery_of(low), saver_on(low)));
        let plugged = power(1, 8, 10, 0);
        assert!(!battery_is_low(battery_of(plugged), saver_on(plugged)));
        let saver_desktop = power(1, 128, 255, 1);
        assert!(battery_is_low(
            battery_of(saver_desktop),
            saver_on(saver_desktop)
        ));
        let unknown = power(255, 255, 255, 0);
        assert!(!battery_is_low(battery_of(unknown), saver_on(unknown)));
    }
}
