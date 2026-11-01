//! 模型管理（MVP-0，TDD-04 §模型管理与License）。
//!
//! 职责：列出本地可见模型（LunaManaged：whisper 与随包引擎的 GGUF 条目）、占用统计、安全删除、安装入口。
//! 边界：
//! - 内置（bundled）模型随 App 分发、**不可删除**（删除即破坏开箱可用，TDD-04）。
//! - LLM / 嵌入由进程外随包引擎推理（ADR-143），此处只管权重的列表 / 安装 / 删除，不接管推理。
//! - 安装/联网行为须显式、可被严格本地模式拦截（AGENTS.md §7）。
//!
//! 隐私：仅处理模型 id / 路径 / 体积等元数据，绝不触碰转写文本/笔记正文。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use specta::Type;

/// 内置 whisper 模型 id（随 App 分发，不可删，TDD-04）。
pub const BUNDLED_WHISPER_ID: &str = "whisper-tiny-bundled";

/// 首个 LunaManaged 远程 ASR 权重（whisper.cpp GGML；权重许可 MIT）。
pub const WHISPER_BASE_REMOTE_ID: &str = "whisper-base-ggml";
/// 平衡档远程 ASR 权重（多语种 small）。
pub const WHISPER_SMALL_REMOTE_ID: &str = "whisper-small-ggml";
/// small 量化档（q5_1，体积约 40%，精度近似）。
pub const WHISPER_SMALL_Q5_REMOTE_ID: &str = "whisper-small-q5_1-ggml";
/// 高精度档远程 ASR 权重（多语种 medium）。
pub const WHISPER_MEDIUM_REMOTE_ID: &str = "whisper-medium-ggml";
/// 最高精度/最快大模型档（large-v3-turbo，4 解码层裁剪版）。
pub const WHISPER_LARGE_V3_TURBO_REMOTE_ID: &str = "whisper-large-v3-turbo-ggml";
/// large-v3-turbo 量化档（q5_0，体积约 35%）。
pub const WHISPER_LARGE_V3_TURBO_Q5_REMOTE_ID: &str = "whisper-large-v3-turbo-q5_0-ggml";
pub const LUNA_MANAGED_WEIGHT_FILE: &str = "weights.bin";
/// GGUF 条目（`llm` / `embed`，随包 llama-server）的权重文件名：`<id>/weights.gguf`。
pub const LUNA_MANAGED_GGUF_FILE: &str = "weights.gguf";

/// 说话人细分-分割模型（pyannote segmentation-3.0 **sherpa 转换版**，MIT；TDD-03 §10.2）。
pub const DIARIZE_SEGMENTATION_REMOTE_ID: &str = "diarize-segmentation-pyannote3";
/// 说话人细分-声纹嵌入模型（3D-Speaker CAM++ zh-cn，Apache-2.0，中文会议主场景）。
pub const DIARIZE_EMBEDDING_ZH_REMOTE_ID: &str = "diarize-embedding-campplus-zh";
/// Silero VAD 权重的目录 id（ADR-012）。
pub const SILERO_VAD_REMOTE_ID: &str = "silero-vad";
/// 说话人细分-嵌入模型英文档（wespeaker VoxCeleb CAM++，Apache-2.0；TDD-03 §10.2 按语言选档）。
pub const DIARIZE_EMBEDDING_EN_REMOTE_ID: &str = "diarize-embedding-wespeaker-en";

/// 设备档位（按内存划分，推导在 [`crate::hardware::derive_tier`]；PRD-14-010，TDD-04 §4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum DeviceTier {
    /// 低配（<8GiB）：只推荐小档，避免下载跑不动的大模型（PRD-14-011）。
    Low,
    /// 标准（8–16GiB）：平衡档。
    Standard,
    /// 高配（≥16GiB）：精度优先档。
    High,
}

/// 档位 → 推荐 whisper 档（单一来源；首启引导与模型面板共用，PRD-14-010）。
///
/// > 假设：低配 base、标准 small-q5_1（体积约 40% 精度近似）、高配 large-v3-turbo-q5_0
/// >（理由：量化档以近似精度换显著更小的磁盘/内存占用，正是首启下载场景的最优权衡；
/// > 非量化档仍可在模型面板自选）。
#[must_use]
pub fn recommended_asr_for_tier(tier: DeviceTier) -> &'static ModelEntry {
    let id = match tier {
        DeviceTier::Low => WHISPER_BASE_REMOTE_ID,
        DeviceTier::Standard => WHISPER_SMALL_Q5_REMOTE_ID,
        DeviceTier::High => WHISPER_LARGE_V3_TURBO_Q5_REMOTE_ID,
    };
    remote_model_spec(id)
        .expect("推荐档位 id 必须存在于随包目录（luna-model-catalog 的 bundled.json）")
}

/// 档位 + 算力落点 → 推荐的本机 whisper 档（首启引导与模型面板共用）。
///
/// **算力在服务端时不按档位推荐，一律给内置 tiny**：那种形态下本机模型的角色是
/// 「服务端连不上时接管」，不是主力。按档位推等于让一台 32GiB 的机器为一个但愿用不上的
/// 东西下载 1.6GB —— 而内置 tiny 随包就位、下载量为零，接管期间的字幕足以撑到服务端恢复。
/// 想要更好的兜底质量，完整设置里的模型面板随时可以另下一档。
///
/// 反过来，用户选了「完全在本机跑」时按档位推荐**必须保留**：那时本机模型就是主力，
/// 给他 tiny 等于把产品质量钉死在最低档，而他的机器可能跑得动大得多的模型。
#[must_use]
pub fn recommended_asr_for(
    tier: DeviceTier,
    location: crate::cloud::ComputeLocation,
) -> &'static ModelEntry {
    if location.is_server() {
        return bundled_tiny_spec();
    }
    recommended_asr_for_tier(tier)
}

/// 档位 → 推荐的对话模型（随包引擎目录条目 id；单一来源，PRD-14-010/021；ADR-145）。
///
/// Low 0.8B Q8_0、Standard 2B Q8_0、High 4B Q4_K_M（小模型 4-bit 损失大，故两小档取 Q8；
/// 4B 取 Q4_K_M：P0-4 出字快 27%、P0-5 质量不劣）。均为 Apache-2.0（ADR-005）。
#[must_use]
pub fn recommended_llm_for_tier(tier: DeviceTier) -> &'static str {
    match tier {
        DeviceTier::Low => "qwen3.5-0.8b-q8_0-gguf",
        DeviceTier::Standard => "qwen3.5-2b-q8_0-gguf",
        DeviceTier::High => "qwen3.5-4b-q4_k_m-gguf",
    }
}

/// 档位 → 推荐的专用翻译模型（「翻译与实时档」，ADR-145）：Low 档取 Q4_K_M
/// （1.13 GB），其余 Q8_0（1.9 GB）。
#[must_use]
pub fn recommended_translation_for_tier(tier: DeviceTier) -> &'static str {
    match tier {
        DeviceTier::Low => "hy-mt2-1.8b-q4_k_m-gguf",
        DeviceTier::Standard | DeviceTier::High => "hy-mt2-1.8b-q8_0-gguf",
    }
}

/// 推荐的嵌入模型（默认嵌入槽的设备策略：bge-m3 Q8_0，ADR-145）。
pub const RECOMMENDED_EMBED: &str = "bge-m3-q8_0-gguf";

/// 档位 + 算力落点 → 推荐的本机 LLM 标签。
///
/// 服务端是主路由时，本机模型只承担不可达兜底，因此恒推荐目录中的最小档；完全本机时
/// 才按设备档位推荐主力模型。这个函数只负责尚未安装时的引导；一旦存在已安装清单，
/// [`device_policy_llm`] 会从中确定性选择最小、能力已确认的 Chat 模型。
#[must_use]
pub fn recommended_llm_for(
    tier: DeviceTier,
    location: crate::cloud::ComputeLocation,
) -> &'static str {
    if location.is_server() {
        recommended_llm_for_tier(DeviceTier::Low)
    } else {
        recommended_llm_for_tier(tier)
    }
}

/// 设备策略档的本机 LLM：在**已装清单**里挑一个，而不是报一个可能不在机器上的名字。
///
/// # 为什么设备策略必须看已装清单
///
/// 设备策略是「用户没有显式选过时，这个槽位落在哪」。它此前是一个常量（一个固定的 1.7B
/// 模型名），于是一台装了 4B、没装 1.7B 的机器上：
/// 分配槽解析出一个 `status = downloadable` 的目标 → 就绪判 `missing` → 槽位不进
/// `assigned_slots` → 对话顶栏显示「未配置本机模型」、状态栏显示「模型 未加载」，
/// **而这台机器明明有一个能用的对话模型**。同一个空目标还会被装进
/// `LlmHttpProvider`，于是每一个本机 LLM 任务都以「模型不存在」失败——知识管线的
/// 全库批处理因此连撞三篇即整批中止（ADR-059 的第二条根因）。
///
/// # 只对**设备策略**成立，不对用户的显式选择成立
///
/// 用户显式 pin 了一个没装的模型时**绝不暗换**（[TDD-04 §2.4]「未安装时保留目标并报告
/// `Missing`，不暗换模型」）——那时正确的动作是如实报缺、引导去装或去改，
/// 见 [`assess_llm_readiness`]。本函数只回答「没人选过时默认落在哪」。
///
/// # 挑选顺序
///
/// 默认只承担云端不可达时的可用性兜底，因此恒取**已安装、能力已确认的最小 Chat 模型**。
/// 设备档位不参与这条默认策略；用户显式选择时仍严格尊重其目标，不在这里暗换。
///
/// 专用翻译模型（Hy-MT2）不参选：它只会翻译，当成对话默认档就会答非所问。
///
/// `None` = 这台机器一个可用的本机对话模型都没有。那时「未配置」是**诚实**的。
#[must_use]
pub(crate) fn device_policy_llm(
    _tier: DeviceTier,
    installed: &[crate::local_engine::EngineModelRef],
) -> Option<String> {
    smallest(installed, |m| !m.translation_only)
}

/// 「翻译与实时档」的设备策略：已装的专用翻译模型里最小的一个；没有则 `None`
/// （调用方据此回落继承默认对话档）。
#[must_use]
pub(crate) fn device_policy_translation(
    installed: &[crate::local_engine::EngineModelRef],
) -> Option<String> {
    smallest(installed, |m| m.translation_only)
}

/// 已装对话模型里满足条件的最小一个。体积读不到（0）的排在最后——不冒充最小，否则
/// 一个读不到体积的大模型会永远胜出；同体积按 id 排，结果稳定。
fn smallest(
    installed: &[crate::local_engine::EngineModelRef],
    keep: impl Fn(&crate::local_engine::EngineModelRef) -> bool,
) -> Option<String> {
    installed
        .iter()
        .filter(|m| m.kind == crate::local_engine::EngineModelKind::Chat && keep(m))
        .min_by_key(|m| (m.size_bytes == 0, m.size_bytes, m.id.clone()))
        .map(|m| m.id.clone())
}

/// 启动 LLM 就绪核查结论（纯逻辑，可单测；PRD-01 §异常「配置的模型未安装须主动引导，
/// 不能等到首次生成才失败」）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmReadiness {
    /// 配置的默认模型已安装，可直接生成。
    Ready,
    /// 默认模型未安装，但本机已有其他可用模型 → 引导去模型页**切换**。
    MissingWithAlternatives {
        /// 当前配置（未安装）的模型标签。
        configured: String,
    },
    /// 本机没有任何已安装模型 → 引导去模型页**下载**。
    NothingInstalled {
        /// 当前配置（未安装）的模型标签。
        configured: String,
    },
}

/// 校验配置的默认 LLM 是否在已安装列表中（条目 id 精确匹配；`installed` 来自引擎编排时
/// 记下的已装清单，不做任何模型名硬编码）。
#[must_use]
pub fn assess_llm_readiness(configured: &str, installed: &[String]) -> LlmReadiness {
    if installed.iter().any(|name| name == configured) {
        return LlmReadiness::Ready;
    }
    if installed.is_empty() {
        LlmReadiness::NothingInstalled {
            configured: configured.to_owned(),
        }
    } else {
        LlmReadiness::MissingWithAlternatives {
            configured: configured.to_owned(),
        }
    }
}

/// 目录条目类型与随包目录的唯一定义在 `luna-model-catalog`（ADR-145 ②）；本模块只消费。
pub use luna_model_catalog::{Artifact, ModelEntry, Task};

/// 编码器目录名——whisper.cpp 按权重文件名派生：`weights.bin` → `weights-encoder.mlmodelc`
/// （并剥掉 `-qx_x` 后缀，故量化档与非量化档同名）。
pub const COREML_ENCODER_DIR: &str = "weights-encoder.mlmodelc";
/// 探针判定 CoreML 不安全时把目录挪到这个名字：留档但不再被 whisper.cpp 找到。
pub const COREML_ENCODER_DISABLED_DIR: &str = "weights-encoder.mlmodelc.disabled";

/// 权重目录里的编码器路径。
#[must_use]
pub fn coreml_encoder_path(root: &Path, model_id: &str) -> PathBuf {
    root.join(model_id).join(COREML_ENCODER_DIR)
}

/// 编码器是否已就位（目录在场即算——whisper.cpp 的判据也只是"能不能打开它"）。
#[must_use]
pub fn coreml_encoder_installed(root: &Path, model_id: &str) -> bool {
    coreml_encoder_path(root, model_id).is_dir()
}

/// 取一条 whisper 目录项（远程目录或随包 tiny）的 CoreML 编码器规格。
#[must_use]
pub fn coreml_encoder_spec(model_id: &str) -> Option<&'static Artifact> {
    remote_model_spec(model_id).and_then(|spec| spec.coreml_encoder.as_ref())
}

/// 解压预转换的 `*.mlmodelc.zip` 到 `dest_dir`（整目录）。
///
/// 发布物顶层是一个 `ggml-<model>-encoder.mlmodelc/` 目录，这里把它**剥掉**直接铺进
/// `dest_dir`——目标名由 whisper.cpp 派生规则决定（[`COREML_ENCODER_DIR`]），与发布物里
/// 的名字无关。zip-slip 一律拒绝（`enclosed_name`）。
pub fn extract_mlmodelc_zip(archive: &Path, dest_dir: &Path) -> Result<(), ModelError> {
    let install_err = |detail: String| {
        ModelError::new(
            "MODEL_INSTALL_FAILED",
            format!("CoreML 工件解压失败：{detail}"),
        )
    };
    let file = fs::File::open(archive).map_err(|e| install_err(e.to_string()))?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file))
        .map_err(|e| install_err(e.to_string()))?;
    if zip.is_empty() {
        return Err(install_err("归档为空".to_owned()));
    }
    // 顶层目录：所有条目共享的第一段（且以 .mlmodelc 结尾）才剥；否则原样铺。
    let mut top: Option<String> = None;
    let mut shared = true;
    for index in 0..zip.len() {
        let entry = zip
            .by_index(index)
            .map_err(|e| install_err(e.to_string()))?;
        let Some(path) = entry.enclosed_name() else {
            return Err(install_err(format!(
                "条目路径异常：{}",
                String::from_utf8_lossy(entry.name_raw())
            )));
        };
        let first = path
            .components()
            .next()
            .map(|c| c.as_os_str().to_string_lossy().into_owned());
        match (&top, first) {
            (None, Some(f)) => top = Some(f),
            (Some(t), Some(f)) if *t == f => {}
            _ => shared = false,
        }
    }
    let strip = shared && top.as_deref().is_some_and(|t| t.ends_with(".mlmodelc"));
    fs::create_dir_all(dest_dir).map_err(|e| install_err(e.to_string()))?;
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|e| install_err(e.to_string()))?;
        let Some(path) = entry.enclosed_name() else {
            return Err(install_err(format!(
                "条目路径异常：{}",
                String::from_utf8_lossy(entry.name_raw())
            )));
        };
        let rel: PathBuf = if strip {
            path.components().skip(1).collect()
        } else {
            path
        };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let out = dest_dir.join(&rel);
        if entry.is_dir() {
            fs::create_dir_all(&out).map_err(|e| install_err(e.to_string()))?;
            continue;
        }
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).map_err(|e| install_err(e.to_string()))?;
        }
        let mut file = fs::File::create(&out).map_err(|e| install_err(e.to_string()))?;
        std::io::copy(&mut entry, &mut file).map_err(|e| install_err(e.to_string()))?;
    }
    Ok(())
}

/// 随包转写档的文件名（`bundle.resources` 的源与落点共用这一个名字）。
///
/// **档位是量化的 tiny（q5_1）而不是全精度 tiny**（PRD-14 §6 的「档位待打包体积实测后
/// 微调」在此收口）：随包的全部理由是「零下载即可在引导内完成首个任务」，而 77.7 MB
/// 的全精度档要把这个代价压在每一个安装包上；q5_1 是 30.7 MB，同样的语种覆盖与接口，
/// 兜底与「试一次」这两件事上的质量差异不影响它要承担的职责。
pub const BUNDLED_WHISPER_FILE: &str = "ggml-tiny-q5_1.bin";

/// 随包转写档在应用资源目录里的相对路径（`tauri.conf.json` 的 `bundle.resources` 落点）。
pub const BUNDLED_WHISPER_RESOURCE_REL: &str = "models/ggml-tiny-q5_1.bin";

/// 随包转写档的目录条目（名称、许可、体积与恢复下载规格的唯一来源）。
///
/// # Panics
/// 随包目录里没有它——那是构建缺陷，目录测试会先红。
#[must_use]
pub fn bundled_tiny_spec() -> &'static ModelEntry {
    remote_model_spec(BUNDLED_WHISPER_ID).expect("随包目录里必须有随包转写档")
}

/// 这台机器、这个构建能不能看到这条目录条目。
///
/// GGUF 条目（随包 llama-server）的门：本平台有引擎包、且引擎版本够（ADR-145 ③）——条目
/// 要求的构建号高于随包引擎时不可见，装上也加载不了。
fn visible(entry: &ModelEntry) -> bool {
    // 版本区间（ADR-145 ③）：要求更新应用版本的条目，这个客户端看不到。
    if !entry.supports_client(env!("CARGO_PKG_VERSION")) {
        return false;
    }
    entry
        .gguf
        .as_ref()
        .is_none_or(|gguf| engine_build().is_some_and(|build| build >= gguf.engine_min_build))
}

/// 随包引擎的构建号——本平台有引擎包时为 `Some`，否则 `None`。GGUF 目录条目与导入条目
/// 共用这一道门（ADR-143）。
#[must_use]
pub fn engine_build() -> Option<u32> {
    luna_engine::llama::runtime::current()
        .and_then(|rt| rt.build.trim_start_matches('b').parse().ok())
}

/// 可下载的目录条目（随包档除外——它的下载只用于恢复，不出现在可下载列表里）。
///
/// 已验签的远程目录生效时以它为准，不在其中的条目撤出发现（ADR-145 ⑤）；没有远程时用随包目录。
pub fn remote_model_specs() -> impl Iterator<Item = &'static ModelEntry> {
    luna_model_catalog::discoverable()
        .into_iter()
        .filter(|entry| !entry.bundled && visible(entry))
}

/// 按 id 查目录条目：远程优先、随包兜底——撤出发现的条目对已装模型照常可查（PRD-13-035）。
/// 含随包档的恢复下载规格（包里那份不在时——从未跑过物化的旧安装、剥掉了资源的构建——
/// 靠它下回来）。
#[must_use]
pub fn remote_model_spec(id: &str) -> Option<&'static ModelEntry> {
    luna_model_catalog::lookup(id).filter(|entry| visible(entry))
}

/// 模型管理错误（带稳定 code，供前端做可执行引导 / i18n）。
#[derive(Debug, Clone)]
pub struct ModelError {
    code: &'static str,
    message: String,
}

impl ModelError {
    #[must_use]
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// 稳定错误码（如 `MODEL_BUNDLED_NOT_DELETABLE`）。
    #[must_use]
    pub fn code(&self) -> &str {
        self.code
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ModelError {}

/// 一条模型条目的 DTO（供设置 / 模型管理面板消费）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ModelEntryDto {
    /// 稳定模型 id。
    pub id: String,
    /// 展示名。
    pub name: String,
    /// 目录声明的能力（不由前端按名称猜测）。
    pub declared_capabilities: Vec<String>,
    /// 可安全分配的有效能力（= 目录 / 导入时声明的能力）。
    pub effective_capabilities: Vec<String>,
    /// 管理方：`LunaManaged`。
    pub managed_by: String,
    /// 来源：`bundled` / `downloaded` / `remote` / `imported`。
    pub origin: String,
    /// 许可标签（License 一等公民，ADR-005）。
    pub license_label: String,
    /// 状态：`bundled` / `installed` / `downloadable`。
    pub status: String,
    /// 标称体积（字节，可空）。以 f64 表达供 JS number 消费（字节数远不及 2^53）。
    pub size_bytes: Option<f64>,
    /// 预估磁盘占用（字节，可空）。
    pub est_on_disk_bytes: Option<f64>,
    /// 实际磁盘占用（字节，已安装时统计，可空）。
    pub on_disk_bytes: Option<f64>,
    /// 运行时 digest；重新拉取后变化，供索引身份与能力缓存失效使用。
    pub runtime_digest: Option<String>,
    /// 运行时/目录报告的量化级别；未知为空，绝不按模型名猜。
    pub quantization: Option<String>,
    /// 按权重体积估算的最低运行内存（含 25% 运行开销）；无体积事实时为空。
    pub estimated_memory_bytes: Option<f64>,
    /// `recommended` / `supported` / `insufficient` / `unknown`，由 IPC 按本机内存投影。
    pub device_fit: String,
    /// 当前被哪些模型分配槽使用（由 IPC 聚合的事实）。
    pub assigned_slots: Vec<String>,
    /// 暂存目录里已下载的部分字节（暂停 / 断网留下的 `.part`）；`None` = 没有断点。
    /// 只对 LunaManaged 直下条目有意义。
    pub partial_bytes: Option<f64>,
    /// 这一档有没有 CoreML 编码器可装（本构建编入了 CoreML **且**目录里登记了工件）。
    pub coreml_available: bool,
    /// 编码器已就位（whisper.cpp 加载时会自动用它跑 encoder）。
    pub coreml_installed: bool,
    /// 编码器 zip 体积（可装时才有）。
    pub coreml_size_bytes: Option<f64>,
    /// 编码器下载的断点字节（暂停 / 断网留下的 `.part`）。
    pub coreml_partial_bytes: Option<f64>,
    /// 有可用更新时新版本的下载体积（目录里这一条换了权重）；没有更新为空。更新从不自动发生，
    /// 由用户确认后走同一条安装编排（ADR-145 ④）。
    pub update_size_bytes: Option<f64>,
}

/// 模型存储占用汇总 DTO。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct StorageInfoDto {
    /// 模型存储根目录。
    pub models_root: String,
    /// 已统计模型实际磁盘占用合计（字节）。
    pub total_on_disk_bytes: f64,
    /// 已安装/内置模型数。
    pub installed_count: u32,
}

#[derive(Debug, Clone)]
struct ModelRecord {
    id: String,
    name: String,
    /// 内部资产类别；只用于 LunaManaged 生命周期和 Provider 装配，绝不下发前端决策。
    asset_kind: String,
    declared_capabilities: Vec<String>,
    runtime_digest: Option<String>,
    quantization: Option<String>,
    managed_by: String,
    origin: String,
    license_label: String,
    bundled: bool,
    installed: bool,
    size_bytes: Option<f64>,
    est_on_disk_bytes: Option<f64>,
    install_path: Option<PathBuf>,
    /// 已装、且目录里这一条换了权重（sha256 与安装时记下的不同）时，新版本的下载体积。
    update_size_bytes: Option<f64>,
}

impl ModelRecord {
    fn bundled_whisper(root: &Path) -> Self {
        let path = root.join(BUNDLED_WHISPER_ID);
        let installed = luna_managed_weight_path(root, BUNDLED_WHISPER_ID).is_file();
        // 名称 / 许可 / 体积全部取自目录条目：同一档模型写两遍名字，
        // 换档时必然只改到一处（这里此前写死 "Whisper Tiny"）。
        let spec = bundled_tiny_spec();
        Self {
            id: BUNDLED_WHISPER_ID.to_owned(),
            name: spec.name.clone(),
            asset_kind: "asr".to_owned(),
            declared_capabilities: vec!["transcribe".to_owned()],
            runtime_digest: None,
            quantization: quantization_from_catalog_id(BUNDLED_WHISPER_FILE).map(str::to_owned),
            managed_by: "LunaManaged".to_owned(),
            origin: "bundled".to_owned(),
            license_label: spec.license.label.clone(),
            bundled: true,
            installed,
            size_bytes: Some(spec.weights.size_bytes as f64),
            est_on_disk_bytes: Some(spec.est_on_disk_bytes as f64),
            install_path: installed.then_some(path),
            update_size_bytes: None,
        }
    }

    fn remote(root: &Path, spec: &ModelEntry) -> Self {
        let weight = spec_weight_path(root, spec);
        let installed = weight.is_file();
        // 有更新 = 装着的那份 sha256（安装时记在 manifest 里）与目录当前这条不同（ADR-145 ④）。
        // 没有 manifest（随包物化、手动放置）不算——无从比较，也就不提示。
        let update_size_bytes = installed
            .then(|| installed_sha256(root, spec))
            .flatten()
            .filter(|sha| *sha != spec.weights.sha256)
            .map(|_| spec.weights.size_bytes as f64);
        // 目录式（whisper 惯例）install_path = 模型目录；文件式（`weight_rel` 约定路径，
        // 如 diarize/ 下两模型共目录）= 权重文件本身——删除只移除该文件与其 manifest，
        // 不影响同目录的其他模型。
        let path = if spec.weight_rel.is_some() {
            weight
        } else {
            root.join(&spec.id)
        };
        Self {
            id: spec.id.clone(),
            name: spec.name.clone(),
            asset_kind: spec.task.as_str().to_owned(),
            declared_capabilities: capabilities_for_spec(spec),
            runtime_digest: None,
            quantization: spec
                .gguf
                .as_ref()
                .map(|gguf| gguf.quant.clone())
                .or_else(|| quantization_from_catalog_id(&spec.id).map(str::to_owned)),
            managed_by: "LunaManaged".to_owned(),
            origin: if installed { "downloaded" } else { "remote" }.to_owned(),
            license_label: spec.license.label.clone(),
            bundled: false,
            installed,
            size_bytes: Some(spec.weights.size_bytes as f64),
            est_on_disk_bytes: Some(spec.est_on_disk_bytes as f64),
            install_path: installed.then_some(path),
            update_size_bytes,
        }
    }

    fn status(&self) -> &'static str {
        if self.bundled && !self.installed {
            if remote_model_spec(&self.id).is_some() {
                "downloadable"
            } else {
                "unavailable"
            }
        } else if self.bundled {
            "bundled"
        } else if self.installed || self.install_path.is_some() {
            "installed"
        } else {
            "downloadable"
        }
    }

    fn to_dto(&self) -> ModelEntryDto {
        let on_disk_bytes = self.accounted_on_disk_bytes();
        ModelEntryDto {
            id: self.id.clone(),
            name: self.name.clone(),
            declared_capabilities: self.declared_capabilities.clone(),
            effective_capabilities: self.declared_capabilities.clone(),
            managed_by: self.managed_by.clone(),
            origin: self.origin.clone(),
            license_label: self.license_label.clone(),
            status: self.status().to_owned(),
            size_bytes: self.size_bytes,
            est_on_disk_bytes: self.est_on_disk_bytes,
            on_disk_bytes,
            runtime_digest: self.runtime_digest.clone(),
            quantization: self.quantization.clone(),
            estimated_memory_bytes: self
                .size_bytes
                .or(self.est_on_disk_bytes)
                .map(|bytes| bytes * 1.25),
            device_fit: "unknown".to_owned(),
            assigned_slots: Vec::new(),
            partial_bytes: None,
            coreml_available: false,
            coreml_installed: false,
            coreml_size_bytes: None,
            coreml_partial_bytes: None,
            update_size_bytes: self.update_size_bytes,
        }
    }

    fn accounted_on_disk_bytes(&self) -> Option<f64> {
        self.install_path
            .as_ref()
            .map(|path| dir_size_bytes(path) as f64)
            .or_else(|| {
                if self.installed {
                    self.est_on_disk_bytes.or(self.size_bytes)
                } else {
                    None
                }
            })
    }

    fn weight_path(&self) -> Option<PathBuf> {
        if self.asset_kind != "asr" {
            return None;
        }
        self.install_path
            .as_ref()
            .map(|path| path.join(LUNA_MANAGED_WEIGHT_FILE))
            .filter(|path| path.is_file())
    }
}

fn quantization_from_catalog_id(id: &str) -> Option<&'static str> {
    // LunaManaged 目录是权威元数据，量化级别只从目录 id 中已定义的有限标记投影；
    // 不把文件格式后缀（如 `-ggml`）误当成量化名称，也不对外部模型做名称猜测。
    ["q5_0", "q5_1"]
        .into_iter()
        .find(|quantization| id.contains(quantization))
}

fn capabilities_for_asset_kind(asset_kind: &str) -> Vec<String> {
    let names: &[&str] = match asset_kind {
        "asr" => &["transcribe"],
        "diarize" => &["diarize"],
        "vad" => &["vad"],
        // 随包引擎（ADR-143）：通用对话模型四种文本能力都承担；嵌入模型只嵌入。
        "llm" => &GENERAL_LLM_CAPABILITIES,
        "embed" => &["embed"],
        _ => &[],
    };
    names.iter().map(|name| (*name).to_owned()).collect()
}

/// 通用对话模型承担的文本能力。
const GENERAL_LLM_CAPABILITIES: [&str; 4] = ["chat", "translate", "summarize", "refine"];

/// 目录条目声明的能力：专用翻译模型（Hy-MT2）只声明「翻译」——它不是对话模型，进了
/// 对话 / 总结的候选就会被用户选上、然后答非所问。
fn capabilities_for_spec(spec: &ModelEntry) -> Vec<String> {
    if spec.gguf.as_ref().is_some_and(|gguf| gguf.translation_only) {
        return vec!["translate".to_owned()];
    }
    capabilities_for_asset_kind(spec.task.as_str())
}

/// 模型存储：内置 + 已下载（root 下子目录）+ 导入的 GGUF。
pub struct ModelStore {
    root: PathBuf,
    records: Vec<ModelRecord>,
}

impl ModelStore {
    /// 以模型根目录加载：内置条目 + 扫描 root 下已下载的 LunaManaged 模型目录。
    #[must_use]
    pub fn load(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let mut store = Self {
            records: vec![ModelRecord::bundled_whisper(&root)],
            root,
        };
        for spec in remote_model_specs() {
            store.records.push(ModelRecord::remote(&store.root, spec));
        }
        store.scan_downloaded();
        store.scan_imported();
        store
    }

    /// 导入的 GGUF 用户条目（ADR-144 ③）：与目录并列，按 manifest 登记；GGUF 不可用的构建里不出现。
    fn scan_imported(&mut self) {
        if engine_build().is_none() {
            return;
        }
        for manifest in crate::model_import::imported(&self.root) {
            if self.records.iter().any(|record| record.id == manifest.id) {
                continue;
            }
            let asset_kind = match manifest.role {
                crate::model_import::ImportedRole::Chat => "llm",
                crate::model_import::ImportedRole::Embed { .. } => "embed",
            };
            self.records.push(ModelRecord {
                id: manifest.id.clone(),
                name: manifest.name.clone(),
                asset_kind: asset_kind.to_owned(),
                declared_capabilities: capabilities_for_asset_kind(asset_kind),
                runtime_digest: None,
                quantization: None,
                managed_by: "LunaManaged".to_owned(),
                origin: "imported".to_owned(),
                license_label: manifest.license.clone().unwrap_or_default(),
                bundled: false,
                installed: true,
                size_bytes: Some(manifest.size_bytes as f64),
                est_on_disk_bytes: Some(manifest.size_bytes as f64),
                install_path: Some(self.root.join(&manifest.id)),
                update_size_bytes: None,
            });
        }
    }

    /// 扫描 root 下每个子目录为一条已下载的 LunaManaged 模型（whisper 权重等）。
    fn scan_downloaded(&mut self) {
        let Ok(entries) = fs::read_dir(&self.root) else {
            return;
        };
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            if !entry.path().join(LUNA_MANAGED_WEIGHT_FILE).is_file() {
                continue;
            }
            let id = entry.file_name().to_string_lossy().into_owned();
            if id.starts_with('.')
                || id == BUNDLED_WHISPER_ID
                || remote_model_spec(&id).is_some()
                || self.records.iter().any(|record| record.id == id)
            {
                continue;
            }
            self.records.push(ModelRecord {
                name: id.clone(),
                id,
                asset_kind: "asr".to_owned(),
                declared_capabilities: vec!["transcribe".to_owned()],
                runtime_digest: None,
                quantization: None,
                managed_by: "LunaManaged".to_owned(),
                origin: "downloaded".to_owned(),
                license_label: "MIT".to_owned(),
                bundled: false,
                installed: true,
                size_bytes: None,
                est_on_disk_bytes: None,
                install_path: Some(entry.path()),
                update_size_bytes: None,
            });
        }
    }

    /// 列出全部模型条目。
    #[must_use]
    pub fn list(&self) -> Vec<ModelEntryDto> {
        self.records
            .iter()
            .map(|record| self.entry_of(record))
            .collect()
    }

    /// 记录 → DTO，并补上只有知道 `root` 才答得出的那一格：暂存目录里的断点字节数。
    fn entry_of(&self, record: &ModelRecord) -> ModelEntryDto {
        let mut entry = record.to_dto();
        if record.managed_by == "LunaManaged" && !record.installed {
            entry.partial_bytes = partial_bytes_in(&install_staging_dir(&self.root, &record.id));
        }
        // CoreML 伴生工件（ADR-115）：只在本构建编入了 CoreML 时才对外声明"可装"，
        // 否则 Windows 上会出现一颗永远装不上的按钮。
        if luna_engine::COREML_COMPILED_IN {
            if let Some(spec) = coreml_encoder_spec(&record.id) {
                entry.coreml_available = true;
                entry.coreml_size_bytes = Some(spec.size_bytes as f64);
                entry.coreml_installed = coreml_encoder_installed(&self.root, &record.id);
                entry.coreml_partial_bytes =
                    partial_bytes_in(&coreml_staging_dir(&self.root, &record.id));
            }
        }
        entry
    }

    /// 列出目录并投影已生效分配槽。分配关系属于领域服务，此处仅做无判断的 DTO 合并。
    #[must_use]
    pub fn list_with_assignments(
        &self,
        assignment_slots: &BTreeMap<String, Vec<String>>,
    ) -> Vec<ModelEntryDto> {
        self.records
            .iter()
            .map(|record| {
                let mut entry = self.entry_of(record);
                entry.assigned_slots = assignment_slots.get(&entry.id).cloned().unwrap_or_default();
                entry
            })
            .collect()
    }

    /// 取单条模型 DTO。
    #[must_use]
    pub fn entry(&self, id: &str) -> Option<ModelEntryDto> {
        self.records
            .iter()
            .find(|record| record.id == id)
            .map(ModelRecord::to_dto)
    }

    /// 返回 LunaManaged ASR 权重位置。路径只在 Rust 内部解析，绝不进入设置或前端 DTO。
    #[must_use]
    pub fn weight_path_for_target(&self, id: &str) -> Option<PathBuf> {
        self.records
            .iter()
            .find(|record| record.id == id)
            .and_then(ModelRecord::weight_path)
    }

    /// 存储占用汇总。
    #[must_use]
    pub fn storage_info(&self) -> StorageInfoDto {
        let mut total = 0u64;
        let mut installed = 0u32;
        for record in &self.records {
            if let Some(bytes) = record.accounted_on_disk_bytes() {
                total += bytes.max(0.0) as u64;
                installed += 1;
            }
        }
        StorageInfoDto {
            models_root: self.root.to_string_lossy().into_owned(),
            total_on_disk_bytes: total as f64,
            installed_count: installed,
        }
    }

    /// 删除一个模型：内置不可删；其余清除磁盘 artifacts 并清空 install_path。
    ///
    /// # Errors
    /// 未知 id → `MODEL_NOT_FOUND`；内置 → `MODEL_BUNDLED_NOT_DELETABLE`；删除失败 → `MODEL_DELETE_FAILED`。
    pub fn delete(&mut self, id: &str) -> Result<(), ModelError> {
        let record = self
            .records
            .iter_mut()
            .find(|record| record.id == id)
            .ok_or_else(|| ModelError::new("MODEL_NOT_FOUND", format!("未知模型：{id}")))?;
        if record.bundled {
            return Err(ModelError::new(
                "MODEL_BUNDLED_NOT_DELETABLE",
                "内置模型不可删除",
            ));
        }
        if let Some(path) = record.install_path.take() {
            if path.is_file() {
                // 文件式安装（weight_rel 约定路径）：仅删权重与邻接 manifest，
                // 同目录其他模型（如 diarize 另一件）保持完好。
                fs::remove_file(&path).map_err(|e| {
                    ModelError::new("MODEL_DELETE_FAILED", format!("删除失败：{e}"))
                })?;
                let manifest = weight_manifest_path(&path);
                if manifest.is_file() {
                    let _ = fs::remove_file(manifest);
                }
            } else if path.exists() {
                fs::remove_dir_all(&path).map_err(|e| {
                    ModelError::new("MODEL_DELETE_FAILED", format!("删除失败：{e}"))
                })?;
            }
        }
        record.installed = false;
        Ok(())
    }

    /// 测试夹具：默认存储（仅内置条目，root 指向临时目录）。
    #[cfg(test)]
    #[must_use]
    pub fn for_test() -> Self {
        let root =
            std::env::temp_dir().join(format!("luna-model-store-test-{}", std::process::id()));
        Self {
            records: vec![ModelRecord::bundled_whisper(&root)],
            root,
        }
    }

    /// 测试夹具：指定 root（用于 add_downloaded_fixture / delete）。
    #[cfg(test)]
    #[must_use]
    pub fn for_test_root(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            records: vec![ModelRecord::bundled_whisper(&root)],
            root,
        }
    }

    /// 测试夹具：装一个随包引擎目录条目（占位 `weights.gguf` + 目录记录）。不经可见性门——
    /// 测试关心的是槽位与就绪逻辑，不是本平台有没有引擎包。
    #[cfg(test)]
    pub fn add_engine_fixture(&mut self, id: &str) {
        let spec = luna_model_catalog::find(id).expect("目录里得有这个条目");
        let dir = self.root.join(id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(LUNA_MANAGED_GGUF_FILE), b"fixture").unwrap();
        self.records.retain(|record| record.id != id);
        self.records.push(ModelRecord::remote(&self.root, spec));
    }

    /// 测试夹具：在 root 下伪造一个已下载模型（建目录 + 占位权重文件）。
    #[cfg(test)]
    pub fn add_downloaded_fixture(&mut self, id: &str) -> Result<(), ModelError> {
        let path = self.root.join(id);
        fs::create_dir_all(&path)
            .and_then(|()| fs::write(path.join("weights.bin"), b"fixture"))
            .map_err(|e| ModelError::new("MODEL_FIXTURE_FAILED", e.to_string()))?;
        self.records.push(ModelRecord {
            name: id.to_owned(),
            id: id.to_owned(),
            asset_kind: "asr".to_owned(),
            declared_capabilities: vec!["transcribe".to_owned()],
            runtime_digest: None,
            quantization: None,
            managed_by: "LunaManaged".to_owned(),
            origin: "downloaded".to_owned(),
            license_label: "MIT".to_owned(),
            bundled: false,
            installed: true,
            size_bytes: None,
            est_on_disk_bytes: None,
            install_path: Some(path),
            update_size_bytes: None,
        });
        Ok(())
    }
}

#[must_use]
pub fn luna_managed_weight_path(root: &Path, model_id: &str) -> PathBuf {
    root.join(model_id).join(LUNA_MANAGED_WEIGHT_FILE)
}

/// 一条远程目录项的权重最终落盘绝对路径（`weight_rel` 优先，否则 whisper 惯例
/// `<id>/weights.bin`）。下载器、安装态探测、Provider 注册共用此单一来源。
#[must_use]
pub fn spec_weight_path(root: &Path, spec: &ModelEntry) -> PathBuf {
    match &spec.weight_rel {
        Some(rel) => root.join(rel),
        None => root.join(&spec.id).join(weight_file_name(spec)),
    }
}

/// 目录式安装的权重文件名：GGUF 条目 `weights.gguf`，其余沿 whisper 惯例 `weights.bin`。
#[must_use]
pub fn weight_file_name(spec: &ModelEntry) -> &'static str {
    if spec.gguf.is_some() {
        LUNA_MANAGED_GGUF_FILE
    } else {
        LUNA_MANAGED_WEIGHT_FILE
    }
}

/// 安装前核对 GGUF 文件头与目录声明（ADR-144 ③；判据唯一定义在
/// [`luna_model_catalog::gguf::check_entry`]，服务端冻结前跑同一份）。
/// 不过就不落位：「安装损坏」好过「已安装却跑不起来」。
pub fn check_gguf_header(
    path: &std::path::Path,
    gguf: &luna_model_catalog::Gguf,
) -> Result<(), String> {
    use luna_model_catalog::gguf;
    let file = std::fs::File::open(path).map_err(|e| format!("MODEL_INSTALL_FAILED: {e}"))?;
    let header = gguf::read_header(&mut std::io::BufReader::new(file))
        .map_err(|e| format!("MODEL_INSTALL_FAILED: {e}"))?;
    gguf::check_entry(&header, gguf)
        .map_err(|why| format!("MODEL_INSTALL_FAILED: GGUF 文件头与目录不符：{why}"))
}

/// 把随包转写档**物化**到模型目录（幂等；返回 `true` = 这一次真的铺了一份）。
///
/// 资源目录里那一份是母版，模型目录下的是它的副本。之所以复制出来而不是就地读母版：
/// macOS 的 App Translocation 会把首次从下载目录启动的 .app 挂到一个随机只读路径上，
/// 那条路径在用户把应用拖进「应用程序」之后就不存在了；把字节复制出来一次，此后
/// 权重与 .app 再无关系（应用被删，已装好的模型仍在）。
///
/// 同时这也是**所有既有权重路径解析一行不改**的原因——`luna_managed_weight_path` 那条
/// 唯一约定继续成立，不必给随包档另开一条"去别处找"的分支，也就不会有第二处判断
/// "这个模型到底装没装"。
///
/// 代价：权重在包内与模型目录各占一份（约 31 MB × 2）。接受。
///
/// 不做 sha256 校验：母版来自本应用自己的签名包体，校验它等于用它自己证明它自己；
/// 真正需要校验的是**下载**那条路（[`bundled_tiny_spec`] 的恢复下载），那里照旧校验。
pub fn materialize_bundled_asr(resource_dir: &Path, models_root: &Path) -> std::io::Result<bool> {
    let target = luna_managed_weight_path(models_root, BUNDLED_WHISPER_ID);
    if target.is_file() {
        return Ok(false);
    }
    let master = resource_dir.join(BUNDLED_WHISPER_RESOURCE_REL);
    if !master.is_file() {
        return Ok(false);
    }
    if let Some(dir) = target.parent() {
        fs::create_dir_all(dir)?;
    }
    // 先落到同目录的临时名再改名：复制中途断电/退出不会留下半个权重文件，而
    // "文件在不在" 正是上面那条就绪判据的全部依据。
    let staging = target.with_extension("materializing");
    fs::copy(&master, &staging)?;
    fs::rename(&staging, &target)?;
    Ok(true)
}

/// 断线兜底用的本机 whisper 权重（ADR-038 ⑤「本地 whisper 恒为兜底」）。
///
/// **不是「只有随包 tiny 能当兜底」**：那句话此前被实现成了字面意思，于是装了
/// Whisper Small 却没装随包 tiny 的机器**根本没有兜底**——算力卡片写「这台设备还没有
/// 本机转写模型」，而同一时刻仪表盘写「ASR 转写引擎 Whisper Small (q5_1) 就绪」。
/// 两个界面对同一件事说不一样的话，且功能上也真的缺：服务端一断就没有字幕，
/// 而用户明明装了模型。真机坐实（2026-08-08）。
///
/// 顺序：随包最小档优先（零下载、加载最快），否则退到**已安装的最小 ASR 模型**——
/// 兜底的职责是断线那一刻立刻接上，越小越快接管；用户想要更好的兜底质量，
/// 装一个更小的档或在模型面板里另配即可。
#[must_use]
pub fn fallback_asr_weight(root: &Path) -> Option<PathBuf> {
    let bundled = luna_managed_weight_path(root, BUNDLED_WHISPER_ID);
    if bundled.is_file() {
        return Some(bundled);
    }
    remote_model_specs()
        .filter(|spec| spec.task == Task::Asr)
        .filter_map(|spec| {
            let weight = spec_weight_path(root, spec);
            weight
                .is_file()
                .then_some((spec.weights.size_bytes, weight))
        })
        .min_by_key(|(size, _)| *size)
        .map(|(_, weight)| weight)
}

/// 已装那份的 sha256（取自安装时写下的 manifest）；没有 manifest（随包物化、手动放置）为空。
#[must_use]
pub fn installed_sha256(root: &Path, spec: &ModelEntry) -> Option<String> {
    let manifest = if spec.weight_rel.is_some() {
        weight_manifest_path(&spec_weight_path(root, spec))
    } else {
        root.join(&spec.id).join(INSTALL_DIR_MANIFEST_FILE)
    };
    let value: serde_json::Value = serde_json::from_slice(&fs::read(manifest).ok()?).ok()?;
    value.get("sha256")?.as_str().map(str::to_owned)
}

/// 文件式安装的邻接 manifest 路径（`<weight>.manifest.json`；License 溯源，ADR-005）。
#[must_use]
pub fn weight_manifest_path(weight: &Path) -> PathBuf {
    let mut name = weight
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    name.push_str(".manifest.json");
    weight.with_file_name(name)
}

/// 安装暂存目录名前缀（`.` 开头 = 对目录扫描不可见，见
/// `scan_downloaded_ignores_temporary_hidden_directories`）。
const INSTALL_STAGING_PREFIX: &str = ".";
/// 安装暂存目录名后缀。
const INSTALL_STAGING_SUFFIX: &str = ".download";
/// 暂存目录内的下载中转文件（断点续传的落点；`ArtifactFetcher` 的 `dest`）。
pub const INSTALL_PART_FILE: &str = "weights.bin.part";
/// 暂存目录内的归档提取产物（`archive_member` 那条腿）。
pub const INSTALL_EXTRACTED_FILE: &str = "weight.extracted";
/// 目录式安装的目录内 manifest 文件名（`weight_rel` 为 `None` 时随整目录一并改名落地）。
pub const INSTALL_DIR_MANIFEST_FILE: &str = "manifest.json";

/// 一次安装的暂存目录绝对路径（`<root>/.<id>.download`）。
///
/// 名字由此单一来源生成而不是各处 `format!`：目录扫描要靠「`.` 开头」把它排除在模型
/// 列表之外（否则安装中途的半成品会作为一行「模型」出现在界面上），拼错一次就漏一次。
#[must_use]
pub fn install_staging_dir(root: &Path, model_id: &str) -> PathBuf {
    root.join(format!(
        "{INSTALL_STAGING_PREFIX}{model_id}{INSTALL_STAGING_SUFFIX}"
    ))
}

/// 安装编排逐阶段如实上报的进度事实（类型化事件，不是裸 `Fn(u64)`）。
///
/// **为什么是类型化事件**：编排真正经历的是「取字节 → 解压 → 落地 → 完成」四段，
/// 只有第一段有字节数。此前这条接缝是 `&tauri::AppHandle` + 一个字节回调，于是
/// **编排能说的只剩「下载了多少」**——解压一个 tar.bz2、把 1.6 GB 权重改名落地这些
/// 真实发生且可能很慢的阶段，在接缝上根本没有词可以表达，测试也就无从断言编排到底
/// 走了哪条腿。类型化之后，「走了归档腿还是直下腿」「终态恰好一次」都成为可断言的事实。
///
/// 事件里**不带 model_id**：一次安装 = 一个 sink = 一个模型，把同一个常量塞进每个变体
/// 只是把同一个事实重复四遍；模型身份由 sink 在构造时绑定（见 `TauriInstallProgressSink`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelInstallProgress {
    /// 幂等早退没有发生，确实要装：开始取字节。
    Started { total_bytes: u64 },
    /// 取字节中。`received_bytes` = 累计已下载（续传时含既有起点）。
    Downloading {
        received_bytes: u64,
        total_bytes: u64,
    },
    /// 归档提取中（仅 `archive_member` 那条腿会经过）。
    Extracting,
    /// 落地中：写 manifest、改名到最终路径。
    Finalizing,
    /// 安装完成（终态，一次安装恰好一次）。
    Installed { total_bytes: u64 },
}

/// 安装进度出口。
///
/// 生产实现把阶段翻译成既有的 `model://download-progress` 事件；测试实现只把阶段记进
/// 一个 `Vec`，于是**整条安装编排不需要 Tauri app 也能跑**——那正是本接缝存在的理由。
pub trait ModelInstallProgressSink: Send + Sync {
    /// 上报一个阶段。实现必须是非阻塞的：编排在下载热路径上逐 chunk 调用它。
    fn report(&self, progress: ModelInstallProgress);
}

/// 从 tar.bz2 归档提取单个成员到 `dest`（细分-分割模型官方发布物为归档，TDD-04 §2）。
///
/// 阻塞 IO/CPU：调用方须在阻塞线程（`spawn_blocking`）执行。
///
/// # Errors
/// 归档不可读/成员缺失/写出失败 → `MODEL_INSTALL_FAILED`。
pub fn extract_tar_bz2_member(archive: &Path, member: &str, dest: &Path) -> Result<(), ModelError> {
    let install_err =
        |detail: String| ModelError::new("MODEL_INSTALL_FAILED", format!("归档提取失败：{detail}"));
    let file = fs::File::open(archive).map_err(|e| install_err(e.to_string()))?;
    let mut tar = tar::Archive::new(bzip2::read::BzDecoder::new(file));
    let entries = tar.entries().map_err(|e| install_err(e.to_string()))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| install_err(e.to_string()))?;
        let matched = entry
            .path()
            .map(|p| p == Path::new(member))
            .unwrap_or(false);
        if !matched {
            continue;
        }
        let mut out = fs::File::create(dest).map_err(|e| install_err(e.to_string()))?;
        std::io::copy(&mut entry, &mut out).map_err(|e| install_err(e.to_string()))?;
        return Ok(());
    }
    Err(install_err(format!("归档缺少成员 {member}")))
}

/// 暂存目录里 `.part` 的字节数（>0 才算断点）。
fn partial_bytes_in(staging_dir: &Path) -> Option<f64> {
    std::fs::metadata(staging_dir.join(INSTALL_PART_FILE))
        .ok()
        .map(|meta| meta.len())
        .filter(|len| *len > 0)
        .map(|len| len as f64)
}

/// CoreML 编码器下载的暂存目录（与权重暂存分开：两者可并行、断点各自独立）。
#[must_use]
pub fn coreml_staging_dir(root: &Path, model_id: &str) -> PathBuf {
    install_staging_dir(root, &format!("{model_id}+coreml"))
}

/// 统计目录（或单文件）删掉后能腾出的字节（浅递归；模型目录通常扁平）。
///
/// 还有别的硬链接的文件不计：同卷导入的本地 `.gguf` 与原文件共用同一份数据（ADR-144 ③），
/// 删掉这一条什么也腾不出来，「占用」与删除计划的「预计释放」都不能把它算进去。
fn dir_size_bytes(path: &Path) -> u64 {
    if path.is_file() {
        return path
            .metadata()
            .map(|meta| exclusive_len(&meta))
            .unwrap_or(0);
    }
    let mut total = 0u64;
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            let entry_path = entry.path();
            if entry_path.is_dir() {
                total += dir_size_bytes(&entry_path);
            } else if let Ok(meta) = entry.metadata() {
                total += exclusive_len(&meta);
            }
        }
    }
    total
}

/// 只有这一个链接时才算它的字节。
fn exclusive_len(meta: &fs::Metadata) -> u64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.nlink() > 1 {
            return 0;
        }
    }
    meta.len()
}

#[cfg(test)]
mod tests {

    /// 与原文件共用的硬链接不算占用：删掉这一条腾不出空间，「预计释放」不能说能腾。
    #[cfg(unix)]
    #[test]
    fn shared_hardlinks_do_not_count_as_space_this_entry_frees() {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("blob");
        std::fs::write(&outside, vec![0u8; 4096]).unwrap();
        let entry = dir.path().join("gguf-x");
        std::fs::create_dir_all(&entry).unwrap();
        std::fs::hard_link(&outside, entry.join(LUNA_MANAGED_GGUF_FILE)).unwrap();
        std::fs::write(entry.join(INSTALL_DIR_MANIFEST_FILE), b"{}").unwrap();
        assert_eq!(super::dir_size_bytes(&entry), 2);

        std::fs::remove_file(&outside).unwrap();
        assert_eq!(super::dir_size_bytes(&entry), 4096 + 2);
    }

    /// 随包档的物化（PRD-14 §6）：包内那份是母版，模型目录下是它的副本。
    ///
    /// 这条守的是「零下载即可在引导内完成首个任务」那句承诺的最后一环——不铺，
    /// `ModelRecord::status()` 就把随包档判成 `downloadable`，而界面上只会看到一颗
    /// 「下载」按钮，看不出是接线断了。
    #[test]
    fn the_bundled_weight_is_materialized_once_and_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let resources = dir.path().join("Resources");
        let models = dir.path().join("models");
        let master = resources.join(BUNDLED_WHISPER_RESOURCE_REL);
        std::fs::create_dir_all(master.parent().unwrap()).unwrap();
        std::fs::write(&master, b"master-bytes").unwrap();

        let target = luna_managed_weight_path(&models, BUNDLED_WHISPER_ID);
        assert!(
            materialize_bundled_asr(&resources, &models).unwrap(),
            "第一次该铺"
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"master-bytes");

        // 已在就不动：用户后来换成了别的同名权重（或恢复下载过一份），不该被每次启动覆盖。
        std::fs::write(&target, b"user-replaced").unwrap();
        assert!(
            !materialize_bundled_asr(&resources, &models).unwrap(),
            "第二次该是空操作"
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"user-replaced");
    }

    /// 母版不在（剥掉资源的构建、或还没跑过取回脚本）：如实什么都不做，不报错。
    /// 那条路上随包档退回 [`bundled_tiny_spec`] 的恢复下载，应用照常起。
    #[test]
    fn a_missing_master_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let models = dir.path().join("models");
        assert!(!materialize_bundled_asr(&dir.path().join("empty"), &models).unwrap());
        assert!(!luna_managed_weight_path(&models, BUNDLED_WHISPER_ID).exists());
    }

    /// 物化完不留临时文件——「文件在不在」就是就绪判据的全部依据，目录里多一个
    /// 半成品会让后续的体积统计与清理各自猜一遍那是什么。
    #[test]
    fn materializing_leaves_no_staging_file_behind() {
        let dir = tempfile::tempdir().unwrap();
        let resources = dir.path().join("Resources");
        let models = dir.path().join("models");
        let master = resources.join(BUNDLED_WHISPER_RESOURCE_REL);
        std::fs::create_dir_all(master.parent().unwrap()).unwrap();
        std::fs::write(&master, b"x").unwrap();

        materialize_bundled_asr(&resources, &models).unwrap();
        let entries: Vec<_> = std::fs::read_dir(models.join(BUNDLED_WHISPER_ID))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(entries.len(), 1, "只该留下权重本身，实得 {entries:?}");
    }

    /// ADR-038 ⑤ 是「本地 whisper **恒为**兜底」，不是「只有随包 tiny 能当兜底」。
    ///
    /// 那句话此前被实现成字面意思：装了 Whisper Small 却没装随包 tiny 的机器根本没有兜底，
    /// 卡片写「这台设备还没有本机转写模型」，而同一时刻仪表盘写「Whisper Small (q5_1) 就绪」。
    /// 两个界面对同一件事说不一样的话，功能上也真的缺——服务端一断就没有字幕。
    /// 真机坐实（2026-08-08）。
    #[test]
    fn any_installed_asr_model_can_serve_as_the_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        assert_eq!(fallback_asr_weight(root), None, "一个都没装时如实说没有");

        // 只装了 small（随包 tiny 不在）——它就该是兜底，而不是「没有兜底」。
        let small = remote_model_spec(WHISPER_SMALL_Q5_REMOTE_ID).expect("目录里应有 small");
        let small_path = spec_weight_path(root, small);
        std::fs::create_dir_all(small_path.parent().unwrap()).unwrap();
        std::fs::write(&small_path, b"stub").unwrap();
        assert_eq!(
            fallback_asr_weight(root).as_deref(),
            Some(small_path.as_path()),
            "装了模型却报没有兜底——服务端一断就没有字幕，而用户明明装了"
        );

        // 又装了更大的一档：兜底仍取**最小**的那个（断线那一刻要立刻接上）。
        let large =
            remote_model_spec(WHISPER_LARGE_V3_TURBO_Q5_REMOTE_ID).expect("目录里应有 large");
        let large_path = spec_weight_path(root, large);
        std::fs::create_dir_all(large_path.parent().unwrap()).unwrap();
        std::fs::write(&large_path, b"stub").unwrap();
        assert_eq!(
            fallback_asr_weight(root).as_deref(),
            Some(small_path.as_path()),
            "兜底该取最小的那个：它要在断线那一刻立刻接管"
        );

        // 随包档一旦在场就优先：零下载、加载最快。
        let bundled = luna_managed_weight_path(root, BUNDLED_WHISPER_ID);
        std::fs::create_dir_all(bundled.parent().unwrap()).unwrap();
        std::fs::write(&bundled, b"stub").unwrap();
        assert_eq!(
            fallback_asr_weight(root).as_deref(),
            Some(bundled.as_path())
        );
    }

    use super::*;

    #[test]
    fn bundled_model_cannot_be_deleted() {
        let mut store = ModelStore::for_test();
        let err = store.delete(BUNDLED_WHISPER_ID).unwrap_err();
        assert_eq!(err.code(), "MODEL_BUNDLED_NOT_DELETABLE");
    }

    #[test]
    fn bundled_model_without_weight_is_unavailable_not_installed() {
        let temp = tempfile::tempdir().unwrap();
        let store = ModelStore::load(temp.path());
        let entry = store.entry(BUNDLED_WHISPER_ID).unwrap();

        assert_eq!(entry.status, "downloadable");
        assert_eq!(store.storage_info().installed_count, 0);
    }

    #[test]
    fn tier_recommendations_stay_within_catalogs() {
        for tier in [DeviceTier::Low, DeviceTier::Standard, DeviceTier::High] {
            let asr = recommended_asr_for_tier(tier);
            assert_eq!(asr.task, Task::Asr);
            assert!(remote_model_spec(&asr.id).is_some());
            // 推荐档必须是随包目录里真实存在、能力对得上的条目——指向退役条目的推荐曾让
            // 翻译首用即模型缺失。
            for (id, capability) in [
                (recommended_llm_for_tier(tier), "chat"),
                (recommended_translation_for_tier(tier), "translate"),
                (RECOMMENDED_EMBED, "embed"),
            ] {
                let entry = luna_model_catalog::find(id).unwrap_or_else(|| panic!("{id} 不在目录"));
                assert!(
                    capabilities_for_spec(entry).iter().any(|c| c == capability),
                    "{id}"
                );
            }
        }
    }

    /// 每一档 whisper 都登记了 CoreML 编码器（ADR-115），且 sha256 / 体积齐备、源固定官方仓库；
    /// 量化档复用同名非量化档的那一份；随包 tiny 也有。
    #[test]
    fn every_whisper_tier_has_a_coreml_encoder_artifact() {
        for spec in remote_model_specs().filter(|s| s.task == Task::Asr) {
            let enc = spec.coreml_encoder.as_ref().expect(&spec.id);
            assert_eq!(enc.sha256.len(), 64, "{} 编码器 sha256 缺失", spec.id);
            assert!(enc.size_bytes > 0);
            assert!(enc
                .primary_url()
                .starts_with("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-"));
            assert!(enc.primary_url().ends_with("-encoder.mlmodelc.zip"));
        }
        assert_eq!(
            coreml_encoder_spec(WHISPER_SMALL_Q5_REMOTE_ID),
            coreml_encoder_spec(WHISPER_SMALL_REMOTE_ID)
        );
        assert!(coreml_encoder_spec(BUNDLED_WHISPER_ID).is_some());
        assert!(coreml_encoder_spec(DIARIZE_SEGMENTATION_REMOTE_ID).is_none());
    }

    /// 解压：剥掉发布物的顶层 `xxx.mlmodelc/`，铺进目标目录；带 `..` 的条目拒绝。
    #[test]
    fn mlmodelc_zip_extraction_strips_the_top_level_dir() {
        use std::io::Write as _;
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("enc.zip");
        {
            let file = fs::File::create(&archive).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            zip.add_directory("ggml-base-encoder.mlmodelc/", opts)
                .unwrap();
            zip.start_file("ggml-base-encoder.mlmodelc/coremldata.bin", opts)
                .unwrap();
            zip.write_all(b"head").unwrap();
            zip.add_directory("ggml-base-encoder.mlmodelc/weights/", opts)
                .unwrap();
            zip.start_file("ggml-base-encoder.mlmodelc/weights/weight.bin", opts)
                .unwrap();
            zip.write_all(b"w").unwrap();
            zip.finish().unwrap();
        }
        let dest = dir.path().join(COREML_ENCODER_DIR);
        extract_mlmodelc_zip(&archive, &dest).unwrap();
        assert_eq!(fs::read(dest.join("coremldata.bin")).unwrap(), b"head");
        assert_eq!(fs::read(dest.join("weights/weight.bin")).unwrap(), b"w");
        assert!(!dest.join("ggml-base-encoder.mlmodelc").exists());

        let evil = dir.path().join("evil.zip");
        {
            let file = fs::File::create(&evil).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let opts = zip::write::SimpleFileOptions::default();
            zip.start_file("../escape.bin", opts).unwrap();
            zip.write_all(b"x").unwrap();
            zip.finish().unwrap();
        }
        assert!(extract_mlmodelc_zip(&evil, &dir.path().join("out")).is_err());
        assert!(!dir.path().join("escape.bin").exists());

        // Windows 分隔符的同一形状（`..\`）同样拒绝——`enclosed_name` 按 Windows 路径语义解析。
        let evil_win = dir.path().join("evil-win.zip");
        {
            let file = fs::File::create(&evil_win).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let opts = zip::write::SimpleFileOptions::default();
            zip.start_file("..\\escape-win.bin", opts).unwrap();
            zip.write_all(b"x").unwrap();
            zip.finish().unwrap();
        }
        assert!(extract_mlmodelc_zip(&evil_win, &dir.path().join("out-win")).is_err());
        assert!(!dir.path().join("escape-win.bin").exists());
    }

    #[test]
    fn remote_catalog_covers_speed_to_accuracy_tiers_with_integrity_specs() {
        let ids: Vec<&str> = remote_model_specs().map(|s| s.id.as_str()).collect();
        for id in [
            WHISPER_BASE_REMOTE_ID,
            WHISPER_SMALL_REMOTE_ID,
            WHISPER_SMALL_Q5_REMOTE_ID,
            WHISPER_MEDIUM_REMOTE_ID,
            WHISPER_LARGE_V3_TURBO_REMOTE_ID,
            WHISPER_LARGE_V3_TURBO_Q5_REMOTE_ID,
        ] {
            assert!(ids.contains(&id), "缺少远程档位：{id}");
        }
        for spec in remote_model_specs() {
            assert_eq!(spec.weights.sha256.len(), 64, "{} sha256 缺失", spec.id);
            assert!(spec.weights.size_bytes > 0, "{} 体积缺失", spec.id);
            assert!(
                !spec.license.label.is_empty(),
                "{} License 字段必填（ADR-005）",
                spec.id
            );
            match spec.task {
                Task::Asr => {
                    assert!(
                        spec.weights.primary_url().starts_with(
                            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/"
                        ),
                        "{} 下载源须为固定官方仓库",
                        spec.id
                    );
                    assert_eq!(spec.license.label, "MIT");
                    assert!(spec.weight_rel.is_none());
                }
                // 来源钉提交号与 ModelScope 兜底由目录 crate 的测试守着；这里只守「是 GGUF 条目」。
                Task::Llm | Task::Embed => assert!(spec.gguf.is_some(), "{}", spec.id),
                Task::Diarize | Task::Vad => {
                    assert!(
                        spec.weights
                            .primary_url()
                            .starts_with("https://github.com/k2-fsa/sherpa-onnx/releases/download/"),
                        "{} 下载源须为 sherpa-onnx 官方发布页（S12 结论：分割模型须 sherpa 转换版）",
                        spec.id
                    );
                }
            }
        }
    }

    #[test]
    fn diarize_catalog_targets_provider_convention_paths() {
        // 下载落盘路径与 SherpaDiarizeProvider 的约定路径必须同源（手动放置/下载互通）。
        let root = Path::new("/m");
        let (seg, emb) = crate::diarize_provider::SherpaDiarizeProvider::model_paths(root);
        let seg_spec = remote_model_spec(DIARIZE_SEGMENTATION_REMOTE_ID).unwrap();
        let emb_spec = remote_model_spec(DIARIZE_EMBEDDING_ZH_REMOTE_ID).unwrap();
        assert_eq!(spec_weight_path(root, seg_spec), seg);
        assert_eq!(spec_weight_path(root, emb_spec), emb);
        // 分割模型：归档 + 提取成员；嵌入模型：直下。
        assert!(seg_spec.weights.archive_member.is_some());
        assert!(emb_spec.weights.archive_member.is_none());
        assert_eq!(seg_spec.license.label, "MIT");
        assert_eq!(emb_spec.license.label, "Apache-2.0");
    }

    #[test]
    fn diarize_entry_installed_by_weight_file_and_deletes_only_itself() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("diarize");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("segmentation.onnx"), b"seg").unwrap();
        fs::write(dir.join("segmentation.onnx.manifest.json"), b"{}").unwrap();
        fs::write(dir.join("embedding.onnx"), b"emb").unwrap();

        let mut store = ModelStore::load(temp.path());
        assert_eq!(
            store.entry(DIARIZE_SEGMENTATION_REMOTE_ID).unwrap().status,
            "installed"
        );
        assert_eq!(
            store.entry(DIARIZE_EMBEDDING_ZH_REMOTE_ID).unwrap().status,
            "installed"
        );

        store.delete(DIARIZE_SEGMENTATION_REMOTE_ID).unwrap();
        assert!(!dir.join("segmentation.onnx").exists());
        assert!(!dir.join("segmentation.onnx.manifest.json").exists());
        // 同目录另一模型不受影响。
        assert!(dir.join("embedding.onnx").exists());
    }

    #[test]
    fn extract_tar_bz2_member_pulls_single_file() {
        let temp = tempfile::tempdir().unwrap();
        let archive_path = temp.path().join("a.tar.bz2");
        {
            let file = fs::File::create(&archive_path).unwrap();
            let enc = bzip2::write::BzEncoder::new(file, bzip2::Compression::fast());
            let mut builder = tar::Builder::new(enc);
            let data = b"onnx-bytes";
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, "pkg/model.onnx", data.as_slice())
                .unwrap();
            builder.into_inner().unwrap().finish().unwrap();
        }
        let dest = temp.path().join("out.onnx");
        extract_tar_bz2_member(&archive_path, "pkg/model.onnx", &dest).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"onnx-bytes");

        let missing = extract_tar_bz2_member(&archive_path, "pkg/nope.onnx", &dest).unwrap_err();
        assert_eq!(missing.code(), "MODEL_INSTALL_FAILED");
    }

    /// 目录里 Silero 的落点与运行期读取的约定路径同源（手动放置与下载互通）。
    #[test]
    fn silero_catalog_entry_targets_the_runtime_convention_path() {
        let spec = remote_model_spec(SILERO_VAD_REMOTE_ID).unwrap();
        assert_eq!(spec.task, Task::Vad);
        assert_eq!(
            spec.weight_rel.as_deref(),
            Some(crate::silero_vad::SILERO_WEIGHT_REL)
        );
    }

    /// 「有更新」只认 manifest 里记的 sha256 与目录当前这条不同（ADR-145 ④）；
    /// 相同不报，没有 manifest（随包物化、手动放置）也不报——无从比较。
    #[test]
    fn update_is_reported_only_when_the_installed_sha_differs() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let spec = remote_model_spec(WHISPER_BASE_REMOTE_ID).unwrap();
        let dir = root.join(&spec.id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(LUNA_MANAGED_WEIGHT_FILE), b"w").unwrap();
        let update_of = || {
            ModelStore::load(root.to_path_buf())
                .entry(&spec.id)
                .unwrap()
                .update_size_bytes
        };
        assert_eq!(update_of(), None, "no manifest = nothing to compare");

        let write = |sha: &str| {
            std::fs::write(
                dir.join(INSTALL_DIR_MANIFEST_FILE),
                serde_json::to_vec(&serde_json::json!({ "sha256": sha })).unwrap(),
            )
            .unwrap();
        };
        write(&spec.weights.sha256);
        assert_eq!(update_of(), None, "same sha = up to date");
        write(&"2".repeat(64));
        assert_eq!(update_of(), Some(spec.weights.size_bytes as f64));
    }

    #[test]
    fn bundled_tiny_has_recovery_download_spec() {
        let spec = remote_model_spec(BUNDLED_WHISPER_ID).unwrap();
        assert_eq!(spec.id, BUNDLED_WHISPER_ID);
        assert_eq!(spec.task, Task::Asr);
        assert!(spec.bundled);
        assert_eq!(spec.weights.sha256.len(), 64);
        // 可下载列表不含随包档（它的下载只用于恢复）
        assert!(remote_model_specs().all(|s| s.id != BUNDLED_WHISPER_ID));
    }

    #[test]
    fn delete_removes_artifacts_and_marks_model_downloadable() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = ModelStore::for_test_root(temp.path());
        store.add_downloaded_fixture("whisper-small").unwrap();
        store.delete("whisper-small").unwrap();
        assert!(!temp.path().join("whisper-small").exists());
        assert_eq!(store.entry("whisper-small").unwrap().status, "downloadable");
    }

    #[test]
    fn scan_downloaded_ignores_temporary_hidden_directories() {
        let temp = tempfile::tempdir().unwrap();
        // 名字取自安装编排用的同一个来源：拼第二遍就会在改名那天让这条守护空跑。
        let staging = install_staging_dir(temp.path(), WHISPER_BASE_REMOTE_ID);
        fs::create_dir_all(&staging).unwrap();

        let store = ModelStore::load(temp.path());

        let staging_name = staging.file_name().unwrap().to_string_lossy().into_owned();
        assert!(store.entry(&staging_name).is_none());
    }

    #[test]
    fn unknown_model_delete_reports_not_found() {
        let mut store = ModelStore::for_test();
        assert_eq!(store.delete("nope").unwrap_err().code(), "MODEL_NOT_FOUND");
    }

    /// 设备策略档必须落在**这台机器真有**的模型上。
    ///
    /// 反向验证过的形态：把本函数换回一个固定模型名常量，下面第二、第三条立刻红。
    /// 那正是真机上「装着 qwen3:4b，界面却说未配置本机模型 / 状态栏说模型未加载」的成因
    /// （ADR-059 第二条根因）。
    #[test]
    fn device_policy_llm_picks_something_actually_installed() {
        use crate::local_engine::{EngineModelKind, EngineModelRef};
        let model = |id: &str, kind, translation_only, size_bytes| EngineModelRef {
            id: id.to_owned(),
            kind,
            digest: None,
            translation_only,
            size_bytes,
        };
        let chat = |id: &str, size| model(id, EngineModelKind::Chat, false, size);

        // 默认兜底恒取已装最小的通用对话模型，档位不参与（兜底不按性能挑）。
        assert_eq!(
            device_policy_llm(
                DeviceTier::High,
                &[
                    chat("qwen3.5-2b-q8_0-gguf", 2_000),
                    chat("qwen3.5-0.8b-q8_0-gguf", 800),
                    chat("qwen3.5-4b-q4_k_m-gguf", 2_700),
                ]
            ),
            Some("qwen3.5-0.8b-q8_0-gguf".to_owned())
        );
        // 只有大模型时如实用它，不编造没装的小模型。
        assert_eq!(
            device_policy_llm(DeviceTier::Low, &[chat("qwen3.5-4b-q4_k_m-gguf", 2_700)]),
            Some("qwen3.5-4b-q4_k_m-gguf".to_owned())
        );
        // 专用翻译模型与嵌入模型不当对话默认档，哪怕它们更小。
        let translator = model("hy-mt2-1.8b-q4_k_m-gguf", EngineModelKind::Chat, true, 100);
        let embedder = model("bge-m3-q8_0-gguf", EngineModelKind::Embed, false, 50);
        assert_eq!(
            device_policy_llm(
                DeviceTier::Standard,
                &[
                    translator.clone(),
                    embedder.clone(),
                    chat("qwen3.5-2b-q8_0-gguf", 2_000)
                ]
            ),
            Some("qwen3.5-2b-q8_0-gguf".to_owned())
        );
        assert_eq!(
            device_policy_llm(DeviceTier::Standard, &[translator.clone(), embedder]),
            None
        );
        // 体积读不到的不冒充最小。
        assert_eq!(
            device_policy_llm(
                DeviceTier::Standard,
                &[chat("gguf-unknown", 0), chat("gguf-small", 900)]
            ),
            Some("gguf-small".to_owned())
        );
        // 一个都没装时如实说没有。
        assert_eq!(device_policy_llm(DeviceTier::High, &[]), None);

        // 翻译档：只认专用翻译模型。
        assert_eq!(
            device_policy_translation(&[chat("qwen3.5-0.8b-q8_0-gguf", 800), translator]),
            Some("hy-mt2-1.8b-q4_k_m-gguf".to_owned())
        );
        assert_eq!(
            device_policy_translation(&[chat("qwen3.5-0.8b-q8_0-gguf", 800)]),
            None
        );
    }

    #[test]
    fn llm_readiness_ready_when_configured_is_installed() {
        // gemma4:e4b 模拟用户自行拉取的受限许可模型：不在推荐目录，但已安装即可用
        //（ADR-005 只限"推荐"，不限"使用"）。
        let installed = vec!["qwen3:1.7b".to_owned(), "gemma4:e4b".to_owned()];
        assert_eq!(
            assess_llm_readiness("qwen3:1.7b", &installed),
            LlmReadiness::Ready
        );
    }

    #[test]
    fn llm_readiness_guides_switch_when_alternatives_exist() {
        let installed = vec!["qwen3:1.7b".to_owned()];
        assert_eq!(
            assess_llm_readiness("legacy-model:1b", &installed),
            LlmReadiness::MissingWithAlternatives {
                configured: "legacy-model:1b".to_owned()
            }
        );
    }

    #[test]
    fn llm_readiness_guides_download_when_nothing_installed() {
        assert_eq!(
            assess_llm_readiness("qwen3:1.7b", &[]),
            LlmReadiness::NothingInstalled {
                configured: "qwen3:1.7b".to_owned()
            }
        );
    }
}
