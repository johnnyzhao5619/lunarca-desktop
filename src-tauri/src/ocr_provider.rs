//! 进程内 OCR Provider（`vision-ocr`，macOS Vision framework）。
//! 事实源：ADR-001（一切 AI 动作走单一主干）、PRD-01-007 截屏翻译。
//!
//! OCR 是继 ASR/LLM 之后第三类 AI 能力，同实现 [`AIProvider`] 接入 `JobQueue`：截屏翻译经
//! `JobKind::Ocr` 提交、按 `Capability::Ocr` 路由到本 Provider，与 whisper / 随包引擎等价治理
//! （用量记账 / 取消传播 / 路由）。Vision 同步识别为 CPU 密集阻塞调用 → 专用阻塞线程
//! （ADR-010 纪律：绝不占用 tokio async executor）；结果经 oneshot 桥回异步 `execute` 流。
//!
//! **平台边界**：本 Provider 仅 macOS 注册；其它平台不注册 OCR Provider → OCR Job 按能力
//! 路由失败（诚实的「该平台暂无 OCR」，而非静默兜底），为 Windows OCR / 本地 PaddleOCR /
//! 云 OCR API 留出唯一插槽（架构评估缺口 A 收口）。

use async_trait::async_trait;
use luna_core::{
    AIProvider, CancelToken, Capability, CapabilitySet, ImageRef, Job, JobError, JobEvent,
    JobEventStream, JobKind, JobResult, ProviderDescriptor, ProviderHealth, ProviderId,
    ProviderLocation, ResultPayload, Timestamp, Timings, Usage, UsageEstimate,
};

/// `vision-ocr` 稳定 Provider id。
pub const PROVIDER_ID: &str = "vision-ocr";

/// macOS Vision OCR Provider（Device 级；无模型权重、无预热，随系统框架可用）。
pub struct VisionOcrProvider;

#[async_trait]
impl AIProvider for VisionOcrProvider {
    fn id(&self) -> ProviderId {
        ProviderId::from(PROVIDER_ID)
    }

    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: self.id(),
            display_name: "macOS Vision OCR".into(),
            location: ProviderLocation::Device,
            model: None,
            license_summary: Some("Apple Vision（系统框架，随 OS）".into()),
        }
    }

    fn capabilities(&self) -> CapabilitySet {
        let mut capabilities = std::collections::BTreeSet::new();
        capabilities.insert(Capability::Ocr);
        CapabilitySet {
            capabilities,
            streaming: false,
            location: ProviderLocation::Device,
            explicit_only: false,
        }
    }

    async fn health(&self) -> ProviderHealth {
        ProviderHealth::ready()
    }

    async fn execute(&self, job: Job, cancel: CancelToken) -> JobEventStream {
        Box::pin(async_stream::stream! {
            yield JobEvent::Started;

            let JobKind::Ocr { image: ImageRef::Path(path) } = job.kind else {
                yield JobEvent::Failed(JobError::InvalidInput(
                    "非 OCR Job / 不支持的图像来源".into(),
                ));
                return;
            };

            if cancel.is_cancelled() {
                yield JobEvent::Cancelled;
                return;
            }

            // Vision 识别 CPU 密集阻塞 → 专用阻塞线程（ADR-010）；oneshot 桥回异步流。
            let (tx, rx) = tokio::sync::oneshot::channel();
            let spawn = std::thread::Builder::new()
                .name("luna-vision-ocr".into())
                .spawn(move || {
                    let _ = tx.send(crate::platform::macos::ocr::recognize_text_at_path(&path));
                });
            if spawn.is_err() {
                yield JobEvent::Failed(JobError::ResourceExhausted {
                    detail: "无法启动 OCR 线程".into(),
                });
                return;
            }

            match rx.await {
                Ok(Ok(text)) => yield JobEvent::Finished(JobResult {
                    job_id: job.id,
                    provider_id: ProviderId::from(PROVIDER_ID),
                    model_id: None,
                    payload: ResultPayload::Text(text),
                    usage: Usage::default(),
                    timings: Timings::default(),
                    output_truncated: None,
                    finished_at: Timestamp::ZERO,
                }),
                Ok(Err(e)) => yield JobEvent::Failed(JobError::InvalidInput(e)),
                Err(_) => yield JobEvent::Failed(JobError::ResourceExhausted {
                    detail: "OCR 线程异常退出".into(),
                }),
            }
        })
    }

    fn estimate(&self, _job: &Job) -> Option<UsageEstimate> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 契约锁定：Provider 宣告 `Capability::Ocr` 且 id 稳定——Router 按能力匹配（ADR-001）依赖此。
    #[test]
    fn advertises_ocr_capability_on_device() {
        let p = VisionOcrProvider;
        assert_eq!(p.id().as_str(), PROVIDER_ID);
        let caps = p.capabilities();
        assert!(caps.capabilities.contains(&Capability::Ocr));
        assert!(!caps.streaming);
        assert_eq!(caps.location, ProviderLocation::Device);
        // OCR 为设备内处理，绝不外发（隐私边界）。
        assert!(!p.descriptor().location.is_external());
    }
}
