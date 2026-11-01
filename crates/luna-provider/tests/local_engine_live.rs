//! 实机：Provider ↔ 真 `llama-server` 路由器（ADR-143，P3a）。默认 `#[ignore]`。
//!
//! ```text
//! LUNA_LLAMA_BIN_DIR=~/Dev/lunarca-p0/bin/llama-b11429 \
//! LUNA_LLAMA_CHAT_GGUF=~/Dev/lunarca-p0/models/qwen3.5-0.8b-q8_0.gguf \
//! LUNA_LLAMA_EMBED_GGUF=~/Dev/lunarca-p0/models/bge-m3-q8_0.gguf \
//! cargo test -p luna-provider --test local_engine_live -- --ignored --test-threads=1
//! ```
//!
//! 窗口故意给小（2048）：超窗两条路径才能便宜地撞上。

use futures::StreamExt as _;
use luna_core::{
    AIProvider, CancelToken, Job, JobError, JobEvent, JobKind, JobOrigin, Lang, ResultPayload,
    TextRef,
};
use luna_provider::{EmbedHttpConfig, EmbedHttpProvider, LlmHttpConfig, LlmHttpProvider};

mod support;

fn start() -> std::sync::Arc<support::Router> {
    support::start(
        &format!(
            "[chat]\nmodel = {}\nctx-size = 2048\n\n\
             [embed]\nmodel = {}\nctx-size = 2048\nembeddings = true\npooling = cls\n\
             batch-size = 2048\nubatch-size = 2048\n",
            support::path("LUNA_LLAMA_CHAT_GGUF").display(),
            support::path("LUNA_LLAMA_EMBED_GGUF").display(),
        ),
        2,
    )
}

async fn terminal(provider: &dyn AIProvider, job: Job) -> JobEvent {
    let events: Vec<JobEvent> = provider
        .execute(job, CancelToken::new())
        .await
        .collect()
        .await;
    events.last().cloned().unwrap()
}

fn translate(text: String) -> Job {
    Job::new(
        JobKind::Translate {
            text: TextRef::Inline(text),
            source: None,
            target: Lang::from("zh"),
            instruction: None,
            template: None,
        },
        JobOrigin::PopupTranslate,
    )
}

fn embed(texts: Vec<String>) -> Job {
    Job::new(
        JobKind::Embed {
            texts,
            purpose: luna_core::EmbeddingPurpose::Document,
        },
        JobOrigin::Recording,
    )
}

#[tokio::test]
#[ignore = "需要本机 llama-server 与 GGUF"]
async fn provider_round_trips_against_a_real_router() {
    let router = start();
    let chat = LlmHttpProvider::new(LlmHttpConfig::local_engine(router.clone(), "chat"));

    // 正常翻译：有正文（思考已关）、有自报用量。
    let JobEvent::Finished(result) = terminal(&chat, translate("Good morning.".into())).await
    else {
        panic!("翻译应成功");
    };
    let ResultPayload::Text(text) = result.payload else {
        panic!()
    };
    assert!(!text.trim().is_empty(), "思考没关掉时正文为空");
    assert!(result.usage.tokens_in.is_some() && result.usage.tokens_out.is_some());

    // 超窗如实拒绝。
    let long = "This sentence fills the context window. ".repeat(400);
    assert!(matches!(
        terminal(&chat, translate(long)).await,
        JobEvent::Failed(JobError::InputTooLong {
            window: Some(2048),
            ..
        })
    ));

    // 未知模型。
    let ghost = LlmHttpProvider::new(LlmHttpConfig::local_engine(router.clone(), "ghost"));
    assert!(matches!(
        terminal(&ghost, translate("hi".into())).await,
        JobEvent::Failed(JobError::ModelMissing { .. })
    ));

    // 16 × 1800 字的嵌入批（请求体远超 32 KB，路由器转发要扛得住）。
    let embedder = EmbedHttpProvider::new(EmbedHttpConfig::local_engine(router.clone(), "embed"));
    let batch: Vec<String> = (0..16)
        .map(|i| format!("第{i}段。") + &"嵌入测试文字，".repeat(120))
        .collect();
    let JobEvent::Finished(result) = terminal(&embedder, embed(batch)).await else {
        panic!("嵌入批应成功");
    };
    let ResultPayload::Embeddings(vectors) = result.payload else {
        panic!()
    };
    assert_eq!(vectors.len(), 16);
    for v in &vectors {
        assert_eq!(v.len(), 1024);
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-3, "向量须出厂归一化：{norm}");
    }

    // 编码器超批：500 也要归成超窗。
    assert!(matches!(
        terminal(&embedder, embed(vec!["超长文字。".repeat(1200)])).await,
        JobEvent::Failed(JobError::InputTooLong { .. })
    ));
}
