//! 进程内说话人细分 Provider（`sherpa-diarize`，sherpa-onnx 离线管线）。
//! 事实源：ADR-001（单一 AI 主干）、ADR-020（本地 ONNX 管线 + sherpa-onnx 首选，
//! S12 spike 裁决 S12 本地 diarization spike，内部记录）、TDD-03 §10。
//!
//! 说话人细分是继 ASR/LLM/OCR 之后第四类 AI 能力，同实现 [`AIProvider`] 接入 `JobQueue`：
//! 权威重转写经 `JobKind::Diarize` 提交、按 `Capability::Diarize` 路由到本 Provider。
//! sherpa 推理为 CPU 密集阻塞调用 → 专用阻塞线程（ADR-010 纪律）；oneshot 桥回异步流。
//!
//! **模型约定（v1，手动放置）**：`<models_root>/diarize/segmentation.onnx`（pyannote
//! segmentation-3.0 **sherpa 转换版**，MIT）+ `<models_root>/diarize/embedding.onnx`
//! （3D-Speaker CAM++ zh-cn，Apache-2.0，中文会议主场景）。两文件齐备才注册本 Provider
//! （启动时探测；下载 UI 集成与按语言选档为后续批次，TDD-03 §10.2）。
//!
//! 输出：`ResultPayload::Segments`——段 `text` 恒空、`speaker=Some("1"/"2"/…)`（簇号，
//! 1-based），调用方按时间重叠归属到转写段（`pipeline::assign_speakers`）。
//! 隐私：输入为用户音频（不出进程）；日志仅段数/耗时，绝不含音频内容。

use async_trait::async_trait;
use luna_core::{
    AIProvider, AudioRef, CancelToken, Capability, CapabilitySet, Job, JobError, JobEvent,
    JobEventStream, JobKind, JobResult, ProviderDescriptor, ProviderHealth, ProviderId,
    ProviderLocation, ResultPayload, Segment, SegmentId, Timestamp, Timings, Usage, UsageEstimate,
};
use std::path::PathBuf;
use std::time::Duration;

/// `sherpa-diarize` 稳定 Provider id。
pub const PROVIDER_ID: &str = "sherpa-diarize";

/// 聚类阈值默认值（S12 实测：真人多说话人 0.7 恰分；设置项
/// `diarize.cluster_threshold` ×100 存储，装配点读取后传入，S12 结论 #5）。
pub const DEFAULT_CLUSTER_THRESHOLD: f32 = 0.7;

/// 期望输入采样率（pyannote segmentation-3.0 = 16k，与管线 `TARGET_SAMPLE_RATE` 一致）。
const EXPECTED_SAMPLE_RATE: u32 = 16_000;

/// onnxruntime intra-op 线程数：按核数取但设上限——细分是会后批量（数十倍实时，S12），
/// 超过 4 线程收益递减，且须给 ASR 专用线程池/UI 留余量（ADR-010 资源分治）。
fn diarize_num_threads() -> i32 {
    std::thread::available_parallelism()
        .map(|n| n.get().min(4))
        .unwrap_or(1) as i32
}

/// 本地说话人细分 Provider（Device 级；模型文件齐备才注册，见 `models_present`）。
pub struct SherpaDiarizeProvider {
    segmentation_model: PathBuf,
    /// zh 基础嵌入档（注册门槛，恒在）。
    embedding_model: PathBuf,
    /// 英文嵌入档（可选增量；job 语言 = en 且文件在场时启用，否则回退 zh）。
    embedding_model_en: PathBuf,
    cluster_threshold: f32,
}

impl SherpaDiarizeProvider {
    /// 模型约定路径（`<models_root>/diarize/…`）。嵌入为 zh 基础档（注册门槛）。
    #[must_use]
    pub fn model_paths(models_root: &std::path::Path) -> (PathBuf, PathBuf) {
        let dir = models_root.join("diarize");
        (dir.join("segmentation.onnx"), dir.join("embedding.onnx"))
    }

    /// 英文嵌入档约定路径（wespeaker EN，TDD-03 §10.2 按语言选档；与 zh 件并存，
    /// 下载/删除互不影响——缺档时细分回退 zh 基础档）。
    #[must_use]
    pub fn embedding_en_path(models_root: &std::path::Path) -> PathBuf {
        models_root.join("diarize").join("embedding-en.onnx")
    }

    /// 两模型文件是否齐备（启动注册探测；不加载模型）。
    #[must_use]
    pub fn models_present(models_root: &std::path::Path) -> bool {
        let (seg, emb) = Self::model_paths(models_root);
        seg.is_file() && emb.is_file()
    }

    /// 以约定路径构造（调用方先经 [`Self::models_present`] 探测）。
    /// `cluster_threshold` 由装配点从设置解析（`diarize.cluster_threshold`，改设置即
    /// 重建 Provider 热生效）。
    #[must_use]
    pub fn new(models_root: &std::path::Path, cluster_threshold: f32) -> Self {
        let (segmentation_model, embedding_model) = Self::model_paths(models_root);
        Self {
            segmentation_model,
            embedding_model,
            embedding_model_en: Self::embedding_en_path(models_root),
            cluster_threshold,
        }
    }

    /// 按 job 语言选嵌入档（TDD-03 §10.2）：`en` 且英文件在场 → EN；否则 zh 基础档
    /// （en 请求但缺档 → 脱敏日志一次，诚实回退）。纯路径决策，可单测。
    fn select_embedding(&self, lang: Option<&str>) -> PathBuf {
        if lang == Some("en") {
            if self.embedding_model_en.is_file() {
                return self.embedding_model_en.clone();
            }
            eprintln!("[diarize] 英文嵌入档缺失，回退 zh 基础档（可在模型面板下载 EN 档）。");
        }
        self.embedding_model.clone()
    }
}

#[async_trait]
impl AIProvider for SherpaDiarizeProvider {
    fn id(&self) -> ProviderId {
        ProviderId::from(PROVIDER_ID)
    }

    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: self.id(),
            display_name: "本地说话人细分（sherpa-onnx）".into(),
            location: ProviderLocation::Device,
            model: None,
            license_summary: Some(
                "sherpa-onnx Apache-2.0；segmentation-3.0 MIT；CAM++ Apache-2.0".into(),
            ),
        }
    }

    fn capabilities(&self) -> CapabilitySet {
        let mut capabilities = std::collections::BTreeSet::new();
        capabilities.insert(Capability::Diarize);
        CapabilitySet {
            capabilities,
            streaming: false,
            location: ProviderLocation::Device,
            explicit_only: false,
        }
    }

    async fn health(&self) -> ProviderHealth {
        if self.segmentation_model.is_file() && self.embedding_model.is_file() {
            ProviderHealth::ready()
        } else {
            ProviderHealth {
                status: luna_core::HealthStatus::Unavailable,
                detail: Some("细分模型文件缺失".into()),
            }
        }
    }

    async fn execute(&self, job: Job, cancel: CancelToken) -> JobEventStream {
        let seg_model = self.segmentation_model.clone();
        // 嵌入档按 job 语言选择（TDD-03 §10.2；execute 时点探测文件，下载即生效无需重注册）。
        let job_lang = match &job.kind {
            JobKind::Diarize { lang, .. } => lang.as_ref().map(|l| l.as_str().to_owned()),
            _ => None,
        };
        let emb_model = self.select_embedding(job_lang.as_deref());
        let threshold = self.cluster_threshold;
        Box::pin(async_stream::stream! {
            yield JobEvent::Started;

            let JobKind::Diarize { audio: AudioRef::Pcm(pcm), .. } = job.kind else {
                yield JobEvent::Failed(JobError::InvalidInput(
                    "非 Diarize Job / 仅支持 Pcm 音频来源".into(),
                ));
                return;
            };
            if pcm.sample_rate != EXPECTED_SAMPLE_RATE || pcm.channels != 1 {
                yield JobEvent::Failed(JobError::InvalidInput(format!(
                    "细分输入须 16k mono（实际 {}Hz/{}ch）",
                    pcm.sample_rate, pcm.channels
                )));
                return;
            }
            if cancel.is_cancelled() {
                yield JobEvent::Cancelled;
                return;
            }

            // sherpa 推理阻塞 CPU 密集 → 专用阻塞线程（ADR-010）；
            // 注：sherpa process 不可中途打断，cancel 仅在边界生效（离线批量可接受）。
            let (tx, rx) = tokio::sync::oneshot::channel();
            let spawn = std::thread::Builder::new()
                .name("luna-sherpa-diarize".into())
                .spawn(move || {
                    let _ = tx.send(run_diarize(&seg_model, &emb_model, threshold, &pcm.samples));
                });
            if spawn.is_err() {
                yield JobEvent::Failed(JobError::ResourceExhausted {
                    detail: "无法启动细分线程".into(),
                });
                return;
            }

            match rx.await {
                Ok(Ok(segments)) => yield JobEvent::Finished(JobResult {
                    job_id: job.id,
                    provider_id: ProviderId::from(PROVIDER_ID),
                    model_id: None,
                    payload: ResultPayload::Segments(segments),
                    usage: Usage::default(),
                    timings: Timings::default(),
                    output_truncated: None,
                    finished_at: Timestamp::ZERO,
                }),
                Ok(Err(e)) => yield JobEvent::Failed(JobError::InvalidInput(e)),
                Err(_) => yield JobEvent::Failed(JobError::ResourceExhausted {
                    detail: "细分线程异常退出".into(),
                }),
            }
        })
    }

    fn estimate(&self, _job: &Job) -> Option<UsageEstimate> {
        None
    }
}

/// 阻塞执行细分（专用线程内）：模型加载 + process + 段转换。
/// 错误串仅含 IO/配置语义，绝不含音频内容（日志脱敏铁律）。
fn run_diarize(
    seg_model: &std::path::Path,
    emb_model: &std::path::Path,
    threshold: f32,
    samples: &[f32],
) -> Result<Vec<Segment>, String> {
    use sherpa_onnx::{
        FastClusteringConfig, OfflineSpeakerDiarization, OfflineSpeakerDiarizationConfig,
        OfflineSpeakerSegmentationModelConfig, OfflineSpeakerSegmentationPyannoteModelConfig,
        SpeakerEmbeddingExtractorConfig,
    };
    let num_threads = diarize_num_threads();
    let cfg = OfflineSpeakerDiarizationConfig {
        segmentation: OfflineSpeakerSegmentationModelConfig {
            pyannote: OfflineSpeakerSegmentationPyannoteModelConfig {
                model: Some(seg_model.to_string_lossy().into_owned()),
            },
            num_threads,
            ..Default::default()
        },
        embedding: SpeakerEmbeddingExtractorConfig {
            model: Some(emb_model.to_string_lossy().into_owned()),
            num_threads,
            ..Default::default()
        },
        clustering: FastClusteringConfig {
            num_clusters: -1,
            threshold,
        },
        ..Default::default()
    };
    let sd = OfflineSpeakerDiarization::create(&cfg)
        .ok_or_else(|| "细分器初始化失败（模型文件损坏/格式不符）".to_string())?;
    let result = sd
        .process(samples)
        .ok_or_else(|| "细分推理失败".to_string())?;
    let segments = result
        .sort_by_start_time()
        .into_iter()
        .map(|s| Segment {
            id: SegmentId::new(),
            start: Duration::from_secs_f64(f64::from(s.start)),
            end: Duration::from_secs_f64(f64::from(s.end)),
            text: String::new(),
            translation: None,
            speaker: Some((s.speaker + 1).to_string()), // 1-based 簇号
            confidence: None,
            words: None,
        })
        .collect();
    Ok(segments)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 契约锁定：Provider 宣告 `Capability::Diarize` 且 id 稳定（Router 按能力匹配，ADR-001）。
    #[test]
    fn advertises_diarize_capability_on_device() {
        let p = SherpaDiarizeProvider::new(
            std::path::Path::new("/nonexistent"),
            DEFAULT_CLUSTER_THRESHOLD,
        );
        assert_eq!(p.id().as_str(), PROVIDER_ID);
        let caps = p.capabilities();
        assert!(caps.capabilities.contains(&Capability::Diarize));
        assert!(!caps.streaming);
        assert_eq!(caps.location, ProviderLocation::Device);
        // 细分为设备内处理，绝不外发（隐私边界）。
        assert!(!p.descriptor().location.is_external());
    }

    #[test]
    fn num_threads_bounded_by_cores_and_cap() {
        let n = diarize_num_threads();
        assert!((1..=4).contains(&n), "线程数须在 [1,4]（实得 {n}）");
    }

    #[test]
    fn model_paths_follow_convention() {
        let (seg, emb) = SherpaDiarizeProvider::model_paths(std::path::Path::new("/m"));
        assert_eq!(
            seg,
            std::path::PathBuf::from("/m/diarize/segmentation.onnx")
        );
        assert_eq!(emb, std::path::PathBuf::from("/m/diarize/embedding.onnx"));
        assert_eq!(
            SherpaDiarizeProvider::embedding_en_path(std::path::Path::new("/m")),
            std::path::PathBuf::from("/m/diarize/embedding-en.onnx")
        );
        assert!(!SherpaDiarizeProvider::models_present(
            std::path::Path::new("/nonexistent")
        ));
    }

    /// 嵌入按语言选档（TDD-03 §10.2）：en 且英文件在场 → EN；en 缺档/其余语种/None → zh。
    #[test]
    fn embedding_tier_selection_by_lang_with_fallback() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir_all(root.join("diarize")).unwrap();
        let p = SherpaDiarizeProvider::new(root, DEFAULT_CLUSTER_THRESHOLD);

        // en 请求但英文件缺失 → 回退 zh 基础档。
        assert_eq!(p.select_embedding(Some("en")), p.embedding_model);
        // 英文件落盘后（下载即生效，无需重建 Provider）→ 选 EN。
        std::fs::write(root.join("diarize/embedding-en.onnx"), b"en").unwrap();
        assert_eq!(p.select_embedding(Some("en")), p.embedding_model_en);
        // 中文/未知语种/None 恒 zh 基础档。
        assert_eq!(p.select_embedding(Some("zh")), p.embedding_model);
        assert_eq!(p.select_embedding(Some("ja")), p.embedding_model);
        assert_eq!(p.select_embedding(None), p.embedding_model);
    }
}
