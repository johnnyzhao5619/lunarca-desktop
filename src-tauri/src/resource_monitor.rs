//! 状态栏系统资源监控（CPU/GPU 占用，PRD-04-033）。
//!
//! 后台低频（~1.5s）采样：CPU 经 `sysinfo`（跨平台全局占用），GPU 经
//! [`crate::platform::gpu_utilization_percent`]（macOS IOKit；其它平台/失败为 `None`）。
//! 结果经 typed event `resource://usage` 推前端状态栏，与模型指示器同排展示。
//!
//! 隐私铁律：仅设备级聚合百分比，绝不含任何用户内容。

use std::time::Duration;

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri_specta::Event;

/// 相邻两次采样间隔（低频，避免监控自身成为负载）。
const SAMPLE_INTERVAL: Duration = Duration::from_millis(1500);

/// 系统资源占用快照（状态栏渲染）。
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[tauri_specta(event_name = "resource://usage")]
pub struct ResourceUsage {
    /// 全局 CPU 占用百分比（0–100）。
    pub cpu_percent: f32,
    /// GPU 利用率百分比（0–100）；`None` = 该平台不可得（状态栏仅显示 CPU）。
    pub gpu_percent: Option<f32>,
}

/// 启动后台采样任务（随 App 生命周期常驻）。
pub fn start(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut sys = sysinfo::System::new();
        loop {
            // CPU 占用需两次刷新之间隔一小段（sysinfo 语义）：首刷建基线，隔 ~200ms 再刷得增量。
            sys.refresh_cpu_usage();
            tokio::time::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL).await;
            sys.refresh_cpu_usage();
            let cpu_percent = sys.global_cpu_usage();

            // GPU 读取是阻塞 IOKit 调用 → spawn_blocking，绝不占用 async worker。
            let gpu_percent = tokio::task::spawn_blocking(crate::platform::gpu_utilization_percent)
                .await
                .ok()
                .flatten();

            let _ = ResourceUsage {
                cpu_percent,
                gpu_percent,
            }
            .emit(&app);

            tokio::time::sleep(SAMPLE_INTERVAL).await;
        }
    });
}
