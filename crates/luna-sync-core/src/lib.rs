//! 同步与合并共用的纯逻辑（ADR-148、ADR-177 ⑥；TDD-12 §10.2、TDD-17 §2）：
//! 库密钥派生与加解密、分块与补齐、清单条目与提交请求体、恢复密钥、行级 diff 与三路合并、路径名比较口径、
//! frontmatter 属性的就地改写（属性面板）、文件命名规则（冲突副本、新笔记默认文件名、改名与路径合法性），
//! 附件的位置与链接、标签的读法，以及全文检索的查询语法与命中定位。
//!
//! 不依赖文件系统、SQLite、HTTP 或任何异步运行时，所以同一份代码既原生编译进桌面（`luna-sync`、
//! `luna-vault`），也编译成 WASM 给网页版（`luna-web`）——加密格式与合并规则只有这一份实现。

pub mod attachment;
pub mod chunk;
pub mod commit;
pub mod crypto;
pub mod diff;
pub mod frontmatter;
pub mod frontmatter_edit;
pub mod hashing;
pub mod links;
pub mod manifest;
pub mod merge3;
pub mod naming;
pub mod path;
pub mod recovery;
pub mod scaffold;
pub mod search;
pub mod tags;
