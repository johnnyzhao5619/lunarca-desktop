//! 打开系统默认浏览器（device-code 批准页）。
//!
//! 零新依赖的平台分发：macOS `open` / Windows `cmd /C start` / Linux `xdg-open`。
//! 只接受 https/http URL，不做通用打开器。调用点：设备码批准页、Stripe 托管付款页与账单管理页
//! （纵切 3 C2，地址由服务端交回、只在 Rust 内流转）。**URL 一律不进日志**：后两者带着会话标识。

use crate::error::CloudError;

pub fn open_url(url: &str) -> Result<(), CloudError> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(CloudError::internal("refusing to open non-http url"));
    }
    let result = {
        #[cfg(target_os = "macos")]
        {
            std::process::Command::new("open").arg(url).spawn()
        }
        #[cfg(target_os = "windows")]
        {
            std::process::Command::new("cmd")
                .args(["/C", "start", "", url])
                .spawn()
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            std::process::Command::new("xdg-open").arg(url).spawn()
        }
    };
    result
        .map(|_| ())
        .map_err(|err| CloudError::internal(format!("open browser failed: {err}")))
}
