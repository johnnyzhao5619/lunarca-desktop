//! 路由器 preset INI 的生成（ADR-143 ②④）。
//!
//! preset 由 Rust 按「档位 + 已装模型 + 内存预检结论」生成，**不手写、不让用户编辑**：
//! 它承载的几条约束都是 P0 用整机 panic 换来的，写错一条就回到那个故障——
//!
//! - 每节显式 `ctx-size`：交给 `--fit` 定上下文时它会保住训练长度（Qwen3.5 是 262 144）、
//!   转而砍 GPU 层，又慢又占内存；
//! - 不写 `n-gpu-layers`：显式给层数时 `--fit` 适配失败只打一行警告，照原参数硬加载；
//! - 不写 `fit = off`；
//! - `batch-size` / `ubatch-size` 只给编码器嵌入模型拉到整窗（BERT 类必须整段入一个
//!   ubatch，否则 500）；给对话模型会把计算缓冲撑到数 GB（Hy-MT2 实测 4.2 GB）。
//!
//! 密钥**不进 preset**：经 preset / 命令行传的密钥不下发子实例（子端口无鉴权），
//! 只能走环境变量 `LLAMA_API_KEY`（见 [`super::supervisor`]）。

use std::fmt::Write as _;
use std::path::PathBuf;

/// KV 缓存的元素类型。默认 f16；q8_0 只作内存预算的退让档（P0-4：速度无差）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KvType {
    /// 半精度（默认）。
    F16,
    /// 8 bit 量化（内存不够时的退让档）。
    Q8_0,
}

impl KvType {
    /// llama-server 的取值写法。
    #[must_use]
    pub fn as_arg(self) -> &'static str {
        match self {
            Self::F16 => "f16",
            Self::Q8_0 => "q8_0",
        }
    }
}

/// 嵌入模型的池化方式（与 GGUF 元数据一致，仍显式写出——默认值跨版本变过）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pooling {
    /// 取 `[CLS]`（bge-m3）。
    Cls,
    /// 取最后一个 token（Qwen3-Embedding 等解码器嵌入）。
    Last,
    /// 均值。
    Mean,
}

impl Pooling {
    fn as_arg(self) -> &'static str {
        match self {
            Self::Cls => "cls",
            Self::Last => "last",
            Self::Mean => "mean",
        }
    }
}

/// 一个模型在路由器里扮演的角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelRole {
    /// 对话 / 翻译。`thinking` = 模型默认会思考（Qwen3.5），须显式关掉——
    /// 不关时思考进 `reasoning_content`，短请求的预算全耗在思考上、正文为空（P0-1）。
    Chat {
        /// 模型默认开启思考。
        thinking: bool,
    },
    /// 嵌入。`encoder` = 编码器结构（BERT 类），整段必须进一个 ubatch。
    Embed {
        /// 池化方式。
        pooling: Pooling,
        /// 编码器结构（批大小须等于上下文）。
        encoder: bool,
    },
}

impl From<luna_model_catalog::GgufRole> for ModelRole {
    /// 目录条目声明的角色 → preset 角色（应用编排与 `luna` CLI 共用这一处）。
    fn from(role: luna_model_catalog::GgufRole) -> Self {
        use luna_model_catalog::{GgufRole, Pooling as CatalogPooling};
        match role {
            GgufRole::Chat { thinking } => Self::Chat { thinking },
            GgufRole::Embed { pooling, encoder } => Self::Embed {
                pooling: match pooling {
                    CatalogPooling::Cls => Pooling::Cls,
                    CatalogPooling::Last => Pooling::Last,
                    CatalogPooling::Mean => Pooling::Mean,
                },
                encoder,
            },
        }
    }
}

/// preset 里的一节：一个已通过内存预检的模型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetModel {
    /// 节名 = 请求里的 `model` 字段（用目录条目 id，稳定且不含空白）。
    pub id: String,
    /// 权重文件绝对路径。
    pub path: PathBuf,
    /// 角色。
    pub role: ModelRole,
    /// 上下文长度（token），由内存预检定下，必须 > 0。
    pub ctx: u32,
    /// KV 元素类型，由内存预检定下。
    pub kv: KvType,
}

/// preset 生成的错误：只有调用方的编程错误会走到这里。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PresetError {
    /// 节名为空或含 INI 结构字符。
    #[error("invalid preset section id: {0:?}")]
    InvalidId(String),
    /// 节名重复。
    #[error("duplicate preset section id: {0}")]
    DuplicateId(String),
    /// 上下文为 0（= 交给 `--fit` 定，正是要禁止的那条）。
    #[error("preset section {0} has ctx 0")]
    ZeroContext(String),
    /// 路径不是 UTF-8 或含换行，无法写进 INI。
    #[error("preset section {0} has an unrepresentable path")]
    BadPath(String),
}

/// 渲染 preset INI。`sleep_idle_seconds` = 空闲多久休眠：子进程保留、显存释放，
/// 首个请求自动唤醒（P0-2：2.45 s）。`models-max` 是路由器本身的参数，不进 preset
/// （`[*]` 下发给的是子实例），见 [`super::supervisor`]。
///
/// # Errors
/// 节名非法 / 重复、上下文为 0、路径无法写进 INI 时返回 [`PresetError`]。
pub fn render_preset(
    sleep_idle_seconds: u32,
    models: &[PresetModel],
) -> Result<String, PresetError> {
    let mut out = String::new();
    // `[*]` 下发给每个子实例：只放对所有节都成立的东西。`np = 1`：桌面单用户，
    // 多槽只会按槽数成倍预留 KV。
    out.push_str("[*]\n");
    out.push_str("flash-attn = auto\n");
    out.push_str("np = 1\n");
    let _ = writeln!(out, "sleep-idle-seconds = {sleep_idle_seconds}");

    let mut seen = std::collections::HashSet::new();
    for model in models {
        let id = &model.id;
        if id.is_empty()
            || id
                .chars()
                .any(|c| c.is_whitespace() || matches!(c, '[' | ']' | '=' | ';' | '#'))
        {
            return Err(PresetError::InvalidId(id.clone()));
        }
        if !seen.insert(id.as_str()) {
            return Err(PresetError::DuplicateId(id.clone()));
        }
        if model.ctx == 0 {
            return Err(PresetError::ZeroContext(id.clone()));
        }
        let path = model
            .path
            .to_str()
            .filter(|p| !p.contains(['\n', '\r']))
            .ok_or_else(|| PresetError::BadPath(id.clone()))?;

        let _ = write!(out, "\n[{id}]\nmodel = {path}\nctx-size = {}\n", model.ctx);
        let _ = writeln!(out, "cache-type-k = {}", model.kv.as_arg());
        let _ = writeln!(out, "cache-type-v = {}", model.kv.as_arg());
        match model.role {
            ModelRole::Chat { thinking } => {
                if thinking {
                    out.push_str("reasoning = off\n");
                }
            }
            ModelRole::Embed { pooling, encoder } => {
                out.push_str("embeddings = true\n");
                let _ = writeln!(out, "pooling = {}", pooling.as_arg());
                if encoder {
                    let _ = writeln!(out, "batch-size = {}", model.ctx);
                    let _ = writeln!(out, "ubatch-size = {}", model.ctx);
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat(id: &str, thinking: bool) -> PresetModel {
        PresetModel {
            id: id.into(),
            path: PathBuf::from(format!("/m/{id}/weights.gguf")),
            role: ModelRole::Chat { thinking },
            ctx: 16_384,
            kv: KvType::F16,
        }
    }

    const SLEEP: u32 = 1800;

    #[test]
    fn high_tier_preset_snapshot() {
        let models = [
            chat("qwen3.5-4b-q4_k_m-gguf", true),
            PresetModel {
                ctx: 8192,
                ..chat("hy-mt2-1.8b-q8_0-gguf", false)
            },
            PresetModel {
                id: "bge-m3-q8_0-gguf".into(),
                path: PathBuf::from("/m/bge-m3-q8_0-gguf/weights.gguf"),
                role: ModelRole::Embed {
                    pooling: Pooling::Cls,
                    encoder: true,
                },
                ctx: 8192,
                kv: KvType::F16,
            },
        ];
        let ini = render_preset(SLEEP, &models).unwrap();
        assert_eq!(
            ini,
            "[*]\nflash-attn = auto\nnp = 1\nsleep-idle-seconds = 1800\n\
             \n[qwen3.5-4b-q4_k_m-gguf]\nmodel = /m/qwen3.5-4b-q4_k_m-gguf/weights.gguf\nctx-size = 16384\n\
             cache-type-k = f16\ncache-type-v = f16\nreasoning = off\n\
             \n[hy-mt2-1.8b-q8_0-gguf]\nmodel = /m/hy-mt2-1.8b-q8_0-gguf/weights.gguf\nctx-size = 8192\n\
             cache-type-k = f16\ncache-type-v = f16\n\
             \n[bge-m3-q8_0-gguf]\nmodel = /m/bge-m3-q8_0-gguf/weights.gguf\nctx-size = 8192\n\
             cache-type-k = f16\ncache-type-v = f16\nembeddings = true\npooling = cls\n\
             batch-size = 8192\nubatch-size = 8192\n"
        );
    }

    /// P0-7 的三条禁令：出现任何一条都意味着回到整机 panic 的配方。
    #[test]
    fn preset_never_hands_memory_decisions_to_fit() {
        let ini = render_preset(SLEEP, &[chat("a", true), chat("b", false)]).unwrap();
        assert!(!ini.contains("n-gpu-layers"));
        assert!(!ini.contains("fit"));
        assert!(!ini.contains("api-key"));
        assert!(!ini.contains("ctx-size = 0"));
    }

    #[test]
    fn decoder_embedding_keeps_default_batches() {
        let model = PresetModel {
            role: ModelRole::Embed {
                pooling: Pooling::Last,
                encoder: false,
            },
            ..chat("qwen3-embedding", false)
        };
        let ini = render_preset(SLEEP, &[model]).unwrap();
        assert!(ini.contains("pooling = last"));
        assert!(!ini.contains("ubatch-size"));
    }

    #[test]
    fn rejects_what_would_corrupt_or_unsafe_the_ini() {
        let zero = PresetModel {
            ctx: 0,
            ..chat("a", false)
        };
        assert_eq!(
            render_preset(SLEEP, &[zero]),
            Err(PresetError::ZeroContext("a".into()))
        );
        assert_eq!(
            render_preset(SLEEP, &[chat("a b", false)]),
            Err(PresetError::InvalidId("a b".into()))
        );
        assert_eq!(
            render_preset(SLEEP, &[chat("a]", false)]),
            Err(PresetError::InvalidId("a]".into()))
        );
        assert_eq!(
            render_preset(SLEEP, &[chat("a", false), chat("a", true)]),
            Err(PresetError::DuplicateId("a".into()))
        );
        let newline = PresetModel {
            path: PathBuf::from("/m/x\nmodel = /etc/passwd"),
            ..chat("a", false)
        };
        assert_eq!(
            render_preset(SLEEP, &[newline]),
            Err(PresetError::BadPath("a".into()))
        );
    }
}
