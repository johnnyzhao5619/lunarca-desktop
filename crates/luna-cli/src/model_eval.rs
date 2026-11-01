//! `luna model-eval`：给一个目录条目跑评测证据（ADR-145 ⑤ B1）。
//!
//! 评测集、用例与判据全在 `luna_model_catalog::eval`——这里只负责把模型跑起来：随包推理
//! 引擎（与应用同一个路由器、同一份 preset 渲染）+ 应用真实的提示词合成（经 Provider 主干，
//! 不手写 prompt），产出报告 JSON 交给管理台上传。服务端会用同一个 `verdict` 重算，不信这里
//! 的自报结论；这里先算一遍只是让运营当场知道过不过。
//!
//! 报告**不含模型输出原文**（只记判定与字数）：它要上传、要进审计。

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use luna_core::{
    ChatMessage, ChatRole, EmbeddingPurpose, Job, JobEvent, JobKind, JobOrigin, Lang,
    ResultPayload, SummaryStyle, TextRef,
};
use luna_jobqueue::{AdmissionCapacity, JobQueue, JobSink as _};
use luna_model_catalog::eval::{self, CaseResult, EmbedMetrics, EvalReport, Suite};
use luna_model_catalog::{GgufRole, ModelEntry};
use luna_provider::{
    EmbedHttpConfig, EmbedHttpProvider, LlmHttpConfig, LlmHttpProvider, ProviderRegistry,
};

use crate::local_router;

/// 对话评测的上下文窗：用例都很短，8k 足够且与低档机一致。
const CHAT_EVAL_CTX: u32 = 8192;

pub struct Args {
    pub catalog_id: String,
    pub gguf: PathBuf,
    pub entry: Option<PathBuf>,
    pub reference: Option<PathBuf>,
    pub resources: Option<PathBuf>,
    pub out: Option<PathBuf>,
}

/// 退出码：0 达标；1 跑完但不达标；2 输入不对（条目 / 文件 / sha256）；3 引擎起不来。
pub async fn run(args: Args) -> ExitCode {
    let entry = match load_entry(&args.catalog_id, args.entry.as_deref()) {
        Ok(entry) => entry,
        Err(e) => {
            eprintln!("luna: {e}");
            return ExitCode::from(2);
        }
    };
    let Some(suite) = Suite::for_entry(&entry) else {
        eprintln!(
            "luna: {} 不是 GGUF 对话 / 嵌入条目，不需要评测证据",
            entry.id
        );
        return ExitCode::from(2);
    };
    let sha = match sha256_file(&args.gguf) {
        Ok(sha) => sha,
        Err(e) => {
            eprintln!("luna: 读不了 {}：{e}", args.gguf.display());
            return ExitCode::from(2);
        }
    };
    if sha != entry.weights.sha256 {
        eprintln!(
            "luna: {} 的 sha256 与条目不符（{sha} ≠ {}）——证据只能对目录登记的那份权重出",
            args.gguf.display(),
            entry.weights.sha256
        );
        return ExitCode::from(2);
    }
    let resources = args
        .resources
        .clone()
        .unwrap_or_else(local_router::default_resources);
    let Some(engine_build) = luna_engine::llama::runtime::current()
        .and_then(|r| r.build.trim_start_matches('b').parse::<u32>().ok())
    else {
        eprintln!("luna: 本平台没有随包推理引擎");
        return ExitCode::from(3);
    };

    let mut report = EvalReport {
        schema: eval::REPORT_SCHEMA.to_owned(),
        catalog_id: entry.id.clone(),
        sha256: sha,
        suite,
        suite_version: suite.version(),
        engine_build,
        tool: format!("luna {}", env!("CARGO_PKG_VERSION")),
        produced_at: chrono::Utc::now().to_rfc3339(),
        cases: Vec::new(),
        embed: None,
    };
    let outcome = match suite {
        Suite::ChatSmoke | Suite::TranslateSmoke => run_chat(&entry, &args.gguf, &resources, suite)
            .await
            .map(|cases| {
                report.cases = cases;
            }),
        Suite::EmbedSanity => run_embed(&entry, &args.gguf, args.reference.as_deref(), &resources)
            .await
            .map(|metrics| report.embed = Some(metrics)),
    };
    if let Err(e) = outcome {
        eprintln!("luna: {e}");
        return ExitCode::from(3);
    }

    let dims = luna_vault::engine_entry_profile_key(&entry.id)
        .and_then(luna_vault::embedding_profile)
        .and_then(|p| u32::try_from(p.native_dimension).ok());
    let verdict = eval::verdict(&report, &entry, dims);
    let json = serde_json::to_string_pretty(&report).expect("report serializes");
    match &args.out {
        Some(path) => {
            if let Err(e) = std::fs::write(path, &json) {
                eprintln!("luna: 写不了 {}：{e}", path.display());
                return ExitCode::from(2);
            }
            eprintln!("报告已写到 {}", path.display());
        }
        None => println!("{json}"),
    }
    for case in &report.cases {
        eprintln!("  {} {}", if case.passed { "✓" } else { "✗" }, case.id);
    }
    if let Some(m) = &report.embed {
        eprintln!(
            "  维度 {} · 范数偏差 {:.1e} · 无关余弦 {:.3} · top-1 {:.3}{}",
            m.dimensions,
            m.norm_max_deviation,
            m.unrelated_cosine,
            m.hit_at_1,
            m.reference_cosine_min
                .map(|c| format!(" · 对参照最小余弦 {c:.5}"))
                .unwrap_or_default()
        );
    }
    match verdict {
        Ok(()) => {
            eprintln!("✓ 达标（服务端上传后会用同一判据重算）");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("✗ 不达标：{e}");
            ExitCode::FAILURE
        }
    }
}

fn load_entry(id: &str, file: Option<&Path>) -> Result<ModelEntry, String> {
    if let Some(path) = file {
        let text = std::fs::read_to_string(path).map_err(|e| format!("读不了条目文件：{e}"))?;
        let entry: ModelEntry =
            serde_json::from_str(&text).map_err(|e| format!("条目文件不是一条目录条目：{e}"))?;
        if entry.id != id {
            return Err(format!("条目文件里是 {}，不是 {id}", entry.id));
        }
        return Ok(entry);
    }
    luna_model_catalog::bundled()
        .iter()
        .find(|entry| entry.id == id)
        .cloned()
        .ok_or_else(|| format!("随包目录里没有 {id}；目录外的条目用 --entry 给出它的 JSON"))
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read as _;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

async fn final_text(queue: &JobQueue, kind: JobKind) -> Result<String, String> {
    let mut handle = queue.submit(Job::new(kind, JobOrigin::Cli));
    while let Some(event) = handle.recv().await {
        match event {
            JobEvent::Finished(result) => {
                return match result.payload {
                    ResultPayload::Text(text) => Ok(text),
                    other => Err(format!("unexpected payload {other:?}")),
                };
            }
            JobEvent::Failed(err) => return Err(format!("[{}] {err}", err.code())),
            JobEvent::Cancelled => return Err("cancelled".to_owned()),
            _ => {}
        }
    }
    Err("job ended without a result".to_owned())
}

async fn run_chat(
    entry: &ModelEntry,
    gguf: &Path,
    resources: &Path,
    suite: Suite,
) -> Result<Vec<CaseResult>, String> {
    let max_context = entry.gguf.as_ref().map_or(CHAT_EVAL_CTX, |g| g.max_context);
    let role = entry
        .gguf
        .as_ref()
        .map(|g| g.role.into())
        .ok_or("not a GGUF entry")?;
    let router = local_router::LocalRouter::start(
        resources,
        &entry.id,
        gguf,
        role,
        max_context.min(CHAT_EVAL_CTX),
    )
    .await?;
    let registry = ProviderRegistry::new();
    registry.register(Arc::new(LlmHttpProvider::new(LlmHttpConfig::local_engine(
        router.source(),
        entry.id.clone(),
    ))));
    let queue = JobQueue::new(Arc::new(registry), AdmissionCapacity::for_capture_tracks(1));
    let translate = |text: &str, target: &str| JobKind::Translate {
        text: TextRef::Inline(text.to_owned()),
        source: None,
        target: Lang::from(target),
        instruction: None,
        template: None,
    };
    let chat = |prompt: &str| JobKind::Chat {
        messages: vec![ChatMessage {
            role: ChatRole::User,
            content: prompt.to_owned(),
        }],
        tools: Vec::new(),
    };
    let mut results = Vec::new();
    for case in suite.required_cases() {
        let kind = match *case {
            eval::CASE_ZH_TO_EN => translate(eval::ZH_TO_EN_INPUT, "en"),
            eval::CASE_EN_TO_ZH => translate(eval::EN_TO_ZH_INPUT, "zh"),
            eval::CASE_QUESTION_NOT_ANSWERED => translate(eval::QUESTION_INPUT, "zh"),
            eval::CASE_SUMMARY_BRIEF => JobKind::Summarize {
                text: TextRef::Inline(eval::SUMMARY_INPUT.to_owned()),
                style: SummaryStyle::OneLine,
                instruction: None,
            },
            eval::CASE_JSON_ONLY => chat(eval::JSON_PROMPT),
            eval::CASE_NONEMPTY_REPLY => chat(eval::NONEMPTY_PROMPT),
            other => return Err(format!("unknown case {other}")),
        };
        let output = final_text(&queue, kind).await.unwrap_or_default();
        results.push(CaseResult {
            id: (*case).to_owned(),
            passed: eval::case_passes(case, &output),
            output_chars: u32::try_from(output.trim().chars().count()).unwrap_or(u32::MAX),
        });
    }
    drop(queue);
    router.stop().await;
    Ok(results)
}

async fn embed_all(
    entry: &ModelEntry,
    gguf: &Path,
    resources: &Path,
    batches: &[(EmbeddingPurpose, Vec<String>)],
) -> Result<Vec<Vec<Vec<f32>>>, String> {
    let g = entry.gguf.as_ref().ok_or("not a GGUF entry")?;
    if !matches!(g.role, GgufRole::Embed { .. }) {
        return Err(format!("{} is not an embedding entry", entry.id));
    }
    let profile =
        luna_vault::engine_entry_profile_key(&entry.id).and_then(luna_vault::embedding_profile);
    let router =
        local_router::LocalRouter::start(resources, &entry.id, gguf, g.role.into(), g.max_context)
            .await?;
    let registry = ProviderRegistry::new();
    registry.register(Arc::new(EmbedHttpProvider::new(
        EmbedHttpConfig::local_engine(router.source(), entry.id.clone()),
    )));
    let queue = JobQueue::new(Arc::new(registry), AdmissionCapacity::for_capture_tracks(1));
    let mut out = Vec::new();
    for (purpose, texts) in batches {
        let texts = match profile {
            Some(profile) => luna_vault::profile_texts(profile, *purpose, texts.clone()),
            None => texts.clone(),
        };
        let mut handle = queue.submit(Job::new(
            JobKind::Embed {
                purpose: *purpose,
                texts,
            },
            JobOrigin::Cli,
        ));
        let mut vectors = None;
        while let Some(event) = handle.recv().await {
            match event {
                JobEvent::Finished(result) => {
                    if let ResultPayload::Embeddings(v) = result.payload {
                        vectors = Some(v);
                    }
                    break;
                }
                JobEvent::Failed(err) => {
                    drop(queue);
                    router.stop().await;
                    return Err(format!("embedding failed [{}] {err}", err.code()));
                }
                _ => {}
            }
        }
        out.push(vectors.ok_or("embedding produced no vectors")?);
    }
    drop(queue);
    router.stop().await;
    Ok(out)
}

fn cosine(a: &[f32], b: &[f32]) -> f64 {
    let dot: f64 = a
        .iter()
        .zip(b)
        .map(|(x, y)| f64::from(*x) * f64::from(*y))
        .sum();
    let na: f64 = a.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>().sqrt();
    let nb: f64 = b.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>().sqrt();
    dot / (na * nb)
}

async fn run_embed(
    entry: &ModelEntry,
    gguf: &Path,
    reference: Option<&Path>,
    resources: &Path,
) -> Result<EmbedMetrics, String> {
    let owned = |items: &[&str]| items.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    let docs = owned(eval::EMBED_DOCS);
    let queries = owned(eval::EMBED_QUERIES);
    let unrelated = owned(&eval::EMBED_UNRELATED);
    let batches = [
        (EmbeddingPurpose::Document, docs.clone()),
        (EmbeddingPurpose::Query, queries),
        (EmbeddingPurpose::Document, unrelated),
    ];
    let vectors = embed_all(entry, gguf, resources, &batches).await?;
    let (doc_v, query_v, unrelated_v) = (&vectors[0], &vectors[1], &vectors[2]);
    let dimensions = u32::try_from(doc_v.first().map_or(0, Vec::len)).unwrap_or(0);
    let norm_max_deviation = vectors
        .iter()
        .flatten()
        .map(|v| (v.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>().sqrt() - 1.0).abs())
        .fold(0.0, f64::max);
    let hits = query_v
        .iter()
        .enumerate()
        .filter(|(i, q)| {
            let best = doc_v
                .iter()
                .enumerate()
                .max_by(|a, b| cosine(q, a.1).total_cmp(&cosine(q, b.1)))
                .map(|(j, _)| j);
            best == Some(*i)
        })
        .count();
    #[allow(clippy::cast_precision_loss)]
    let hit_at_1 = hits as f64 / query_v.len().max(1) as f64;
    let unrelated_cosine = cosine(&unrelated_v[0], &unrelated_v[1]);

    let (reference_sha256, reference_cosine_min) = match reference {
        None => (None, None),
        Some(path) => {
            let reference_sha = sha256_file(path).map_err(|e| format!("参照模型读不了：{e}"))?;
            let reference_vectors = embed_all(
                entry,
                path,
                resources,
                &[(EmbeddingPurpose::Document, docs)],
            )
            .await?;
            let min = reference_vectors[0]
                .iter()
                .zip(doc_v)
                .map(|(a, b)| cosine(a, b))
                .fold(f64::INFINITY, f64::min);
            (Some(reference_sha), Some(min))
        }
    };
    Ok(EmbedMetrics {
        dimensions,
        norm_max_deviation,
        unrelated_cosine,
        hit_at_1,
        reference_sha256,
        reference_cosine_min,
    })
}
