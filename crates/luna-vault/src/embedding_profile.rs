//! 嵌入 profile（TDD-04 §2.5，PRD-10-017/018）：目录里每个嵌入模型的**用法契约**——维度、
//! 查询指令、近重复阈值、分块基准——由 Rust 执行，前端不拼提示词。
//!
//! 住在 luna-vault 而不是 src-tauri：`luna bench-embed`（基准门禁，PRD-10-017）与应用的索引
//! 建库必须用**同一张表、同一套文本预处理与向量归一化**，否则基准测的不是应用跑的那条路。
//! Qwen3-Embedding 三档 `chunk_policy: None` = 分块基准未通过，不得进生产索引；基准通过后
//! 在这里填上 policy 即解锁（`assignment_target_supported` 只看这一个字段）。

/// 经 Lunarca 审核的嵌入 profile。模型“能返回向量”不足以进入检索：输入用途、维度、归一化和
/// 阈值都必须由 Rust 固定在 profile 中，前端和 Provider 不拼提示词。
#[derive(Clone, Copy, Debug)]
pub struct EmbeddingProfile {
    /// profile 的稳定身份串，与 [`embedding_profile`] 的键同名。服务端产出的向量以它为索引
    /// 身份的 `target_id`（随包引擎产出的向量用条目 id，见应用侧 `resolve_embedding_identity`）；
    /// 它不是运行时目标，不参与路由。改它即改索引身份，全库重建一次（ADR-027）。
    pub target_id: &'static str,
    /// 原生服务端协议里的稳定向量空间标识；不等于服务端具体模型名。
    pub cloud_profile_id: &'static str,
    pub profile_version: u32,
    pub native_dimension: usize,
    pub query_instruction: Option<&'static str>,
    pub near_duplicate_threshold: Option<f32>,
    /// 只有完成本机端到端基准验收的模型才有分块策略；`None` = 可研究，不得进入生产索引。
    pub chunk_policy: Option<crate::EmbeddingChunkPolicy>,
}

pub const BGE_M3_PROFILE: EmbeddingProfile = EmbeddingProfile {
    target_id: "bge-m3",
    cloud_profile_id: "bge-m3:v1:1024:l2",
    profile_version: 1,
    native_dimension: 1024,
    query_instruction: None,
    // 阈值的事实源在 luna-vault（与 near_duplicates 的守护测试同一份）；此处只引用不复制。
    near_duplicate_threshold: Some(crate::NEAR_DUPLICATE_THRESHOLD),
    chunk_policy: Some(crate::BGE_M3_CHUNK_POLICY),
};

/// Qwen3-Embedding 全系共用的官方 Query 指令（模型卡原文）；Document 侧不加任何前缀。
pub const QWEN3_EMBED_QUERY_INSTRUCTION: &str =
    "Instruct: Given a search query, retrieve relevant passages that answer the query\nQuery:";

pub const QWEN3_EMBED_06_PROFILE: EmbeddingProfile = EmbeddingProfile {
    target_id: "qwen3-embedding:0.6b",
    cloud_profile_id: "qwen3-embedding-0.6b:v1:1024:l2",
    profile_version: 1,
    native_dimension: 1024,
    query_instruction: Some(QWEN3_EMBED_QUERY_INSTRUCTION),
    near_duplicate_threshold: None,
    chunk_policy: None,
};

pub const QWEN3_EMBED_4_PROFILE: EmbeddingProfile = EmbeddingProfile {
    target_id: "qwen3-embedding:4b",
    cloud_profile_id: "qwen3-embedding-4b:v1:2560:l2",
    profile_version: 1,
    native_dimension: 2560,
    query_instruction: Some(QWEN3_EMBED_QUERY_INSTRUCTION),
    near_duplicate_threshold: None,
    chunk_policy: None,
};

pub const QWEN3_EMBED_8_PROFILE: EmbeddingProfile = EmbeddingProfile {
    target_id: "qwen3-embedding:8b",
    cloud_profile_id: "qwen3-embedding-8b:v1:4096:l2",
    profile_version: 1,
    native_dimension: 4096,
    query_instruction: Some(QWEN3_EMBED_QUERY_INSTRUCTION),
    near_duplicate_threshold: None,
    chunk_policy: None,
};

#[must_use]
pub fn embedding_profile(model: &str) -> Option<EmbeddingProfile> {
    match model {
        "bge-m3" => Some(BGE_M3_PROFILE),
        "qwen3-embedding:0.6b" => Some(QWEN3_EMBED_06_PROFILE),
        "qwen3-embedding:4b" => Some(QWEN3_EMBED_4_PROFILE),
        "qwen3-embedding:8b" => Some(QWEN3_EMBED_8_PROFILE),
        _ => None,
    }
}

/// 随包推理引擎（GGUF）目录条目 → profile 键（ADR-143）。
///
/// profile 是**模型**的属性：同一个 bge-m3 无论跑在哪个运行时，维度、分块基准、
/// 查询指令都一样，故条目映射回同一张表。量化不同 / 运行时不同产出的向量确实不同，那由
/// 索引身份里的权重摘要区分（换了就全库重建），不靠 profile。
///
/// 只认目录条目：导入条目（`gguf-<sha12>`）不知道是哪个模型家族，没有经审核的 profile，
/// 不得进生产索引。目录新增嵌入条目时必须同步这里——应用侧有守护测试逐条核对。
#[must_use]
pub fn engine_entry_profile_key(entry_id: &str) -> Option<&'static str> {
    match entry_id {
        "bge-m3-q8_0-gguf" => Some("bge-m3"),
        "qwen3-embedding-0.6b-q8_0-gguf" => Some("qwen3-embedding:0.6b"),
        _ => None,
    }
}

#[must_use]
pub fn profile_texts(
    profile: EmbeddingProfile,
    purpose: luna_core::EmbeddingPurpose,
    texts: Vec<String>,
) -> Vec<String> {
    match (purpose, profile.query_instruction) {
        (luna_core::EmbeddingPurpose::Query, Some(instruction)) => texts
            .into_iter()
            .map(|text| format!("{instruction} {text}"))
            .collect(),
        _ => texts,
    }
}

/// 引擎交回的向量**不符合约定**。
///
/// # 它不是一句给用户的话，而是一条给排查的事实
///
/// 维数不符、含非有限值、范数为零、条数对不上、载荷根本不是向量——五种情形对用户是
/// **同一件事**（这一批向量不能用，本篇不写入索引），差别只对排查有意义。故稳定码逐种
/// 保留（日志按它聚类、运营据它定位），而给人读的那半句只有一条：把五句"维数期望 1024
/// 实得 768"式的话摆到界面上，用户既看不懂也无从处置。
///
/// 归成类型而不是字符串，是为了让归一化本身保持**纯函数**：它没有语言、没有 `AppHandle`、
/// 单测里也不需要 Tauri 运行时，翻成人话是边界那一层的事。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VectorContract {
    /// 维数与 profile 声明的不一致。
    Dimension { expected: usize, got: usize },
    /// 含 NaN / 无穷。
    NonFinite,
    /// 范数为零，无法归一化。
    ZeroNorm,
    /// 条数与请求的文本数对不上。
    Count { expected: usize, got: usize },
    /// 终态载荷根本不是向量。
    NotVectors,
}

impl VectorContract {
    /// 稳定码（日志聚类与排查按它分支；界面只显示它对应的那一句话）。
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::Dimension { .. } => "EMBED_DIMENSION",
            Self::NonFinite => "EMBED_NON_FINITE",
            Self::ZeroNorm => "EMBED_ZERO_VECTOR",
            Self::Count { .. } | Self::NotVectors => "EMBED_SHAPE",
        }
    }

    /// 排查用的形状事实。**只有数字与形状，绝不含笔记正文或查询文本**（TDD-12 §1）。
    #[must_use]
    pub fn detail(self) -> String {
        match self {
            Self::Dimension { expected, got } => format!("dimension expected={expected} got={got}"),
            Self::NonFinite => "non-finite component".to_owned(),
            Self::ZeroNorm => "zero norm".to_owned(),
            Self::Count { expected, got } => format!("count expected={expected} got={got}"),
            Self::NotVectors => "payload is not embeddings".to_owned(),
        }
    }
}

/// L2 归一化，并逐条核对与 profile 的约定。**纯函数**：无语言、无 `AppHandle`。
/// # Errors
/// 任一向量维数不符 / 含非有限值 / 范数为零即整批拒绝。
pub fn normalize_embedding_vectors(
    vectors: Vec<Vec<f32>>,
    profile: EmbeddingProfile,
) -> Result<Vec<Vec<f32>>, VectorContract> {
    vectors
        .into_iter()
        .map(|mut vector| {
            if vector.len() != profile.native_dimension {
                return Err(VectorContract::Dimension {
                    expected: profile.native_dimension,
                    got: vector.len(),
                });
            }
            let norm_sq = vector.iter().try_fold(0.0_f64, |sum, value| {
                if !value.is_finite() {
                    return Err(VectorContract::NonFinite);
                }
                Ok(sum + f64::from(*value) * f64::from(*value))
            })?;
            let norm = norm_sq.sqrt();
            if !norm.is_finite() || norm <= f64::EPSILON {
                return Err(VectorContract::ZeroNorm);
            }
            for value in &mut vector {
                *value = (f64::from(*value) / norm) as f32;
            }
            Ok(vector)
        })
        .collect()
}
