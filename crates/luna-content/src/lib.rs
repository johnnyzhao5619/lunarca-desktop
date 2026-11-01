//! 内容基座的 IO 层（ADR-169 ① ②）：三类内容共用的三件事。
//!
//! - [`ContentStore`]：内容层存储 `<根>/<域>/<id>/<版本>/`——安装、卸载、列出。不进库、不随同步
//!   （ADR-159 ④）。
//! - [`layered`]：「内置 < 内容层 < 库内」分层查找，同名外层优先。
//! - [`install`]：安装管线——来源（本地文件 | 市场下载的字节）→ sha256 → 按域解析与校验 →
//!   大小限制 → 临时目录 → 原子改名。**本地导入 = 市场安装去掉下载**，同一条路（ADR-169 ②）。
//!
//! 基座不知道条目长什么样：解析与校验由调用方按域传进来（主题包、模板包各有自己的类型，
//! PRD-13 §1.8 禁止统一条目表）。

mod install;
mod layered;
mod store;

pub use install::{install, InstallError, Prepared, Source};
pub use layered::{layered, Layer, Layered};
pub use store::{ContentDomain, ContentError, ContentStore, InstalledItem};
