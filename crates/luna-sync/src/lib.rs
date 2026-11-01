//! luna-sync —— 官方同步的客户端核心（ADR-148；TDD-14 §5、TDD-12 §10.2）。
//!
//! 纯逻辑优先：网络与系统钥匙串经 trait 注入，实现在应用装配层（用 `luna-cloud` 接上）。本 crate
//! 不认识 HTTP、不碰系统钥匙串，于是「未开启同步时零云网络行为」不需要在这里证明——它根本
//! 发不出请求（TDD-00 §3：`luna-cloud` 与中层互不依赖）。
//!
//! - [`crypto`]：密码 / 恢复密钥 → KEK → CMK → 子密钥；块、条目、库名、信封的加解密。
//! - [`recovery`]：恢复密钥的生成与分组显示。
//! - [`keystore`]：解封后的 CMK 在本机的存取接口。
//! - [`chunk`]：切块与补齐。
//! - [`manifest`]：清单条目，文件 ⇄ 加密块的往返。
//! - [`state`]：本机同步状态（基线与游标，可丢可重建）。
//! - [`core`]：三态决策（纯函数）。
//! - [`ports`]：引擎向外要的远端与本地库接口。
//! - [`engine`]：一轮同步的执行器。

pub mod chunk;
pub mod core;
pub mod crypto;
pub mod engine;
pub mod keystore;
pub mod manifest;
pub mod policy;
pub mod ports;
pub mod recovery;
pub mod restore;
pub mod state;

#[cfg(test)]
mod engine_tests;
#[cfg(test)]
mod sim;
#[cfg(test)]
mod testing;
