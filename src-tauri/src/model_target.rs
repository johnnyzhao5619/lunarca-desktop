//! 模型分配目标（`ai.assignment.<slot>` 持久化的 `ModelTargetId`）的**唯一**解析处。
//!
//! # 为什么要有这个模块
//!
//! `ModelTargetId` 对配置层是不透明串（[`luna_config`] 只负责存取），命名空间的知识归
//! 应用层，且只在这里。推荐匹配、删除、安装、嵌入 profile 查找、默认档模型名解析若各自
//! 剥前缀，每一处对同一个字符串做的事都会不一样，且都默认「剥不掉前缀 = 不是我的事」而
//! **静默走空**。
//!
//! 收敛成 typed 解析器的收益不是"少写几行"，而是**把静默走空变成编译错误**：新增一个
//! 命名空间时，每个消费者都被 `match` 逼着回答「遇到它怎么办」。比如「删除一个跑在
//! 服务端的模型」必须是一句明确的拒绝，而不是"剥不掉前缀所以什么也没发生"。
//!
//! # 两种形态（全集）
//!
//! | 形态 | 例 | 含义 |
//! | --- | --- | --- |
//! | `service:<通道>` | `service:asr` | 服务端专用通道；当前仅 ASR 合法。其它载荷只为判旧值不兼容 |
//! | 裸 id | `whisper-small-ggml`、`qwen3.5-2b-q8_0-gguf` | 模型库条目（随包 / 已下载 / 导入的权重文件） |

/// 一个已解析的模型分配目标。
///
/// 借用而不持有：调用方手上本来就有那个字符串，复制一份只是给 `String` 分配一次内存。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelTarget<'a> {
    /// `service:<通道>` —— 服务端专用通道。
    ///
    /// 当前只有 `asr` 合法。解析器仍识别其它载荷，是为了让就绪判定明确返回
    /// `incompatible`，而不是把旧 `service:<model>` 错当成本机裸 id；这不是兼容执行路径。
    Service(&'a str),
    /// 裸 id —— 模型库条目（随包、已下载或导入的权重）。
    StoreEntry(&'a str),
}

impl<'a> ModelTarget<'a> {
    /// 解析一个 `ModelTargetId`。
    ///
    /// 空串与只有前缀没有载荷的串（`service:`）一律判为 `None`：它们不是任何一种有效目标。
    #[must_use]
    pub(crate) fn parse(target: &'a str) -> Option<Self> {
        let target = target.trim();
        if let Some(rest) = target.strip_prefix("service:") {
            return non_empty(rest).map(Self::Service);
        }
        non_empty(target).map(Self::StoreEntry)
    }
}

fn non_empty(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_two_namespaces() {
        assert_eq!(
            ModelTarget::parse("service:asr"),
            Some(ModelTarget::Service("asr"))
        );
        assert_eq!(
            ModelTarget::parse("qwen3.5-2b-q8_0-gguf"),
            Some(ModelTarget::StoreEntry("qwen3.5-2b-q8_0-gguf"))
        );
    }

    /// 只有前缀没有载荷不是"名字为空的目标"，是**无效目标**。
    #[test]
    fn prefix_without_payload_is_not_a_target() {
        for empty in ["", "   ", "service:", "service:   "] {
            assert_eq!(ModelTarget::parse(empty), None, "{empty:?}");
        }
    }
}
