//! Windows 系统集成（ADR-157：Windows 本地只做兜底，能力主要靠云端；ADR-163：弹窗翻译、
//! 文本注入与非激活浮层）。
//!
//! 每个子模块把判定写成纯函数、在任何平台上单测（本模块在测试构建里全平台编译）；系统调用
//! 只在 `cfg(target_os = "windows")` 的薄层里。

/// 剪贴板读写、序号与多格式备份还原。
pub mod clipboard;
/// 前台窗口事实、输入法开关、把前台还给目标窗口。
#[cfg(target_os = "windows")]
pub mod foreground;
/// 文本注入（`TextInjector`）：前台核对 → 逐字输入 / 粘贴。
#[cfg(target_os = "windows")]
pub mod injector;
/// `SendInput` 薄层。
#[cfg(target_os = "windows")]
pub mod input;
/// 键位表与合成按键序列。
pub mod keys;
/// 双击手势：低层键盘钩子。
pub mod keytap;
/// 非激活浮层的样式守卫。
pub mod panel;
/// 同步大文件的暂停条件（ADR-156 ③）：按流量计费的网络与电量。
pub mod power_network;
/// 取选区、注入方式与前台核对的判定（纯函数）。
pub mod routing;
/// 取选中文本：UI Automation 优先，剪贴板回落。
#[cfg(target_os = "windows")]
pub mod selection;
