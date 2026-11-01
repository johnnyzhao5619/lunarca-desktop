//! Vault 错误模型（TDD-06）。携带稳定错误码供 i18n（TDD-15 §4.3），不向上层泄漏原始 IO 细节。

use std::path::PathBuf;

/// Vault 操作错误。
#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    /// 路径逃逸 Vault 根（路径穿越防护，§4.3/TDD-12）。
    #[error("path escapes the vault root: {0}")]
    PathEscape(PathBuf),

    /// 目标不存在。
    #[error("note not found: {0}")]
    NotFound(PathBuf),

    /// 已存在（create 重名等）。
    #[error("target already exists: {0}")]
    AlreadyExists(PathBuf),

    /// 文件非 UTF-8（Markdown 笔记须 UTF-8）。
    #[error("file is not UTF-8 text: {0}")]
    NotUtf8(PathBuf),

    /// 边车 / 快照元数据反序列化失败。
    #[error("metadata decode failed: {0}")]
    Decode(String),

    /// 底层 IO 失败。
    #[error("io failure: {0}")]
    Io(#[from] std::io::Error),

    /// 存放 Vault 的文件系统做不了"占名即发布"的原子发布（不支持硬链接，或该目录不许创建）。
    ///
    /// 单独成一档而不是笼统的 `Io`：这条的解法是**换个 Vault 位置**，与"磁盘满了""文件被
    /// 占用"完全不同；混进 `Io` 用户只会看到一句读不懂的 errno，不知道该动哪儿。
    #[error("atomic publish unsupported here (no hard links, or directory not creatable): {0}")]
    AtomicPublishUnsupported(PathBuf),

    /// 无效的元数据名（不在白名单或含非法字符）。
    #[error("invalid metadata name: {0}")]
    InvalidMetaName(String),

    /// 录音 cue 更新请求无效或找不到匹配分段。
    #[error("invalid recording cue: {0}")]
    InvalidCue(String),

    /// 无效移动（如把文件夹移入自身子目录）。
    #[error("invalid move: {0}")]
    InvalidMove(String),

    /// 跨库替换请求无效（如检索词不唯一，无从确定"替换什么"）。
    #[error("invalid replace request: {0}")]
    InvalidReplace(String),

    /// 无效的 AI 标签生成范围（空路径 / 指向 Vault 根 / 越界，PRD-06-035c）。
    #[error("invalid generation scope: {0}")]
    InvalidScope(String),

    /// 这一篇不能被「标记为记录」（PRD-06-058）：资料笔记（stage 对它无意义）、
    /// AI 维护的主题页与知识地图（成熟度由所有权指纹表达，不走 stage）。
    #[error("cannot be marked as a record: {0}")]
    NotMarkableAsDraft(String),

    /// 列表游标不成立（TDD-07 §4.1a 的稳定错误码 `LIST_CURSOR_INVALID`）：不可解析、
    /// 与本次请求的排序键不匹配，或落在一条根本不分页的查询上。
    ///
    /// **只扩这一个码**（契约明写），不为每种解析失败造同义码——调用方要做的事在
    /// 三种情形下完全一样：丢掉手上这个游标、从第一页重来。
    ///
    /// **它的 Display 此前手工带着码本身**，与同族其它变体不同——因为 IPC 面把
    /// `VaultError` 一律按 `to_string()` 交给前端，码只活在 [`VaultError::code`] 的话
    /// 就永远到不了消费者手里。那个绕法已随批次 I 撤销：`vault_err` 现在对**每一条**
    /// 都发 `码: 载荷`，本变体不再需要特例（ADR-105）。
    #[error("list cursor invalid: {0}")]
    ListCursorInvalid(String),

    /// 改名 / 移动的预检没过：这些笔记写不进去（只读、被占用），整次没有执行（PRD-06-030 ③）。
    #[error("not writable: {0}")]
    NotWritable(String),

    /// 撤销改名被拒：之后这些文件又被改过、挪过或版本找不到了（PRD-06-030 ③）。
    #[error("relocation changed since: {0}")]
    RelocationStale(String),

    /// 改名写到一半失败，回滚时这些文件没能还原（PRD-06-030 ③，如实报告，不宣称原子）。
    #[error("relocation partially rolled back: {0}")]
    RelocationPartial(String),

    /// 这篇还在 iCloud 上没下载到本机（只有 `.名字.icloud` 占位，TDD-06 §4.1 ②）。
    #[error("not downloaded from iCloud yet: {0}")]
    CloudPlaceholder(PathBuf),
}

impl VaultError {
    /// 这一条错误的**载荷**——`Display` 里 `{0}` 那一格，不含任何措辞。
    ///
    /// # 为什么要把它单拆出来
    ///
    /// IPC 面要发的是「稳定码 + 载荷」两样（[`Self::code`] 给前一半，本函数给后一半），
    /// 而措辞归前端按码取（TDD-15 §4.3）。此前 IPC 面发的是 `to_string()`——措辞、载荷、
    /// 语言三样黏成一句中文，于是英文界面读到中文、前端也拿不到码可以映射。
    ///
    /// 载荷本身**不翻译**：它是一条路径、一个 OS errno、或一句已经说清的原因，三者都不
    /// 随界面语言变。
    #[must_use]
    pub fn detail(&self) -> String {
        match self {
            Self::PathEscape(p)
            | Self::NotFound(p)
            | Self::AlreadyExists(p)
            | Self::NotUtf8(p)
            | Self::AtomicPublishUnsupported(p)
            | Self::CloudPlaceholder(p) => p.display().to_string(),
            Self::Io(e) => e.to_string(),
            Self::Decode(s)
            | Self::InvalidMetaName(s)
            | Self::InvalidCue(s)
            | Self::InvalidMove(s)
            | Self::InvalidReplace(s)
            | Self::InvalidScope(s)
            | Self::NotMarkableAsDraft(s)
            | Self::ListCursorInvalid(s)
            | Self::NotWritable(s)
            | Self::RelocationStale(s)
            | Self::RelocationPartial(s) => s.clone(),
        }
    }

    /// 稳定错误码（SCREAMING_SNAKE，TDD-15 §4.3）。
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::PathEscape(_) => "VAULT_PATH_ESCAPE",
            Self::NotFound(_) => "VAULT_NOT_FOUND",
            Self::AlreadyExists(_) => "VAULT_ALREADY_EXISTS",
            Self::NotUtf8(_) => "VAULT_NOT_UTF8",
            Self::AtomicPublishUnsupported(_) => "VAULT_ATOMIC_PUBLISH_UNSUPPORTED",
            Self::Decode(_) => "VAULT_DECODE",
            Self::Io(_) => "VAULT_IO",
            Self::InvalidMetaName(_) => "VAULT_INVALID_META_NAME",
            Self::InvalidCue(_) => "VAULT_INVALID_CUE",
            Self::InvalidMove(_) => "VAULT_INVALID_MOVE",
            Self::InvalidReplace(_) => "VAULT_INVALID_REPLACE",
            Self::InvalidScope(_) => "VAULT_INVALID_SCOPE",
            Self::NotMarkableAsDraft(_) => "VAULT_NOT_MARKABLE_AS_DRAFT",
            // 不带 `VAULT_` 前缀：TDD-07 §4.1a 逐字定下的是 `LIST_CURSOR_INVALID`，
            // 而它是**列表契约**的码（前端按它决定"丢掉游标重来"），不是 Vault 存储的码。
            Self::ListCursorInvalid(_) => "LIST_CURSOR_INVALID",
            Self::NotWritable(_) => "VAULT_NOT_WRITABLE",
            Self::RelocationStale(_) => "VAULT_RELOCATION_STALE",
            Self::RelocationPartial(_) => "VAULT_RELOCATION_PARTIAL",
            Self::CloudPlaceholder(_) => "VAULT_CLOUD_PLACEHOLDER",
        }
    }
}

/// Vault 结果别名。
pub type Result<T> = std::result::Result<T, VaultError>;

#[cfg(test)]
mod tests {
    use super::*;

    /// **每一条变体都要能给出码与载荷**（ADR-105）。
    ///
    /// `vault_err` 发的是 `码: 载荷`，两半各由一个函数给。新增变体时 `code()` 有穷尽
    /// `match` 兜着（漏了不编译），而 `detail()` 同理——本条守的是第三件事：**两者都不
    /// 许是空的**。空码会让前端拿到一条 `: 说明` 的畸形串，空载荷会让「找不到这篇笔记」
    /// 后面跟一个孤零零的冒号。
    #[test]
    fn every_variant_yields_a_code_and_a_detail() {
        let io = std::io::Error::other("boom");
        let cases: Vec<VaultError> = vec![
            VaultError::PathEscape("a.md".into()),
            VaultError::NotFound("a.md".into()),
            VaultError::AlreadyExists("a.md".into()),
            VaultError::NotUtf8("a.md".into()),
            VaultError::Decode("bad".into()),
            VaultError::Io(io),
            VaultError::AtomicPublishUnsupported("a.md".into()),
            VaultError::InvalidMetaName("x".into()),
            VaultError::InvalidCue("x".into()),
            VaultError::InvalidMove("x".into()),
            VaultError::InvalidReplace("x".into()),
            VaultError::InvalidScope("x".into()),
            VaultError::NotMarkableAsDraft("x".into()),
            VaultError::ListCursorInvalid("x".into()),
            VaultError::NotWritable("x".into()),
            VaultError::RelocationStale("x".into()),
            VaultError::RelocationPartial("x".into()),
            VaultError::CloudPlaceholder("a.md".into()),
        ];
        for case in &cases {
            assert!(!case.code().is_empty(), "{case:?} 没有码");
            assert!(!case.detail().is_empty(), "{case:?} 的载荷是空的");
            // 码是给机器聚类的形状：全大写 + 数字 + 下划线（`VAULT_NOT_UTF8` 带数字），
            // 与本仓其它扫码处的 `[A-Z][A-Z0-9_]{2,}` 同一口径。
            assert!(
                case.code()
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'),
                "{} 不是稳定码的形状",
                case.code()
            );
        }
    }

    /// **`Display` 里不许再有中文**（ADR-101 ②）：它的受众是开发者与日志聚类。
    /// 用户那半句由 `code()` 经前端 i18n 产生，两者混为一谈时同一个字符串要同时满足
    /// 四个互相冲突的要求，而它做不到——本族 13 条正是因此在批次 I 里英文化的。
    #[test]
    fn the_display_strings_are_diagnostics_not_user_copy() {
        let rendered = VaultError::NotFound("a.md".into()).to_string();
        assert!(
            !rendered
                .chars()
                .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
            "Display 里出现了中文：{rendered}"
        );
        // 载荷**不在**码里，码**不在** Display 里——`ListCursorInvalid` 此前手工把码
        // 塞进了自己的 Display（因为 IPC 面丢了码），那个特例随本批撤销，不许回潮。
        assert!(
            !VaultError::ListCursorInvalid("x".into())
                .to_string()
                .contains("LIST_CURSOR_INVALID"),
            "码又被塞回 Display 了——那是 `vault_err` 的活"
        );
    }
}
