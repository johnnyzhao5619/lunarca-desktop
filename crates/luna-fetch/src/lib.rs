//! `luna-fetch` — 网络媒体抓取（**yt-dlp 进程外编排**，ADR-023）。
//!
//! 服务 `PRD-05-006`「从链接导入」：用户粘一条 YouTube /
//! bilibili / TikTok 地址，Lunarca 把它下到本地，然后**汇入既有的导入管线**（解码 → 建笔记 →
//! 落库 → 转写），故本 crate 的职责边界止于"交出一个本地媒体文件"，不碰 Vault、不碰转写。
//!
//! ## 为什么是进程外工具而不是内建实现
//! 站点提取逻辑与反爬对抗每周都在变（yt-dlp 一年发数十个版本），把它编进应用等于让用户的
//! 下载成功率随版本落后而单调下降；而我们没有独立的组件更新通道。故取**进程外、检测在场、
//! 缺失则诚实引导**的形态，用户可独立升级。详见 ADR-023。
//!
//! ## 模块
//! - [`tool`]：工具发现（GUI `PATH` 陷阱）与状态探测；
//! - [`url`]：唯一的不可信输入校验点（仅 http(s)，拒选项形状）；
//! - [`probe`]：下载前的元信息与播放列表清单（拦直播、如实报截断）；
//! - [`format`]：格式选择（按 ffmpeg 在场分档，只取 Lunarca 解得开的容器）；
//! - [`install`]：**受管安装**（解析上游 release、选平台资产、校验和；ADR-023 修订）；
//! - [`download`]：子进程执行 + 确定性进度 + 真取消（kill）；
//! - [`progress`]：机器可读进度模板与解析；
//! - [`error`]：稳定错误码与 stderr 分类（前端本地化为人话）。
//!
//! 隐私铁律（TDD-12）：本 crate 的日志与错误值**绝不含完整 URL**（用 [`url::redact`]），
//! 也不读写任何浏览器凭据——需要登录的内容一律诚实拒绝（ADR-023 裁决）。
#![forbid(unsafe_code)]

pub mod download;
pub mod error;
pub mod format;
pub mod install;
pub mod probe;
pub mod progress;
pub mod tool;
pub mod url;

pub use download::{download, FetchOutcome, FetchRequest};
pub use error::{retryable_for_code, FetchError};
pub use format::{FetchMediaKind, MAX_VIDEO_HEIGHT};
pub use install::{InstallPlan, LATEST_RELEASE_API};
pub use probe::{probe, probe_with_timeout, FetchEntry, FetchProbe, PLAYLIST_LIMIT};
pub use progress::{FetchProgress, ProgressThrottle};
pub use tool::{FetchTool, ToolStatus};
