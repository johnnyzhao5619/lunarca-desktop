//! 实时冒烟：连随包推理引擎（真 `llama-server` 路由器，ADR-143）。默认 `#[ignore]`。
//!
//! 跑法（本机对话模型的那几条）：
//! ```sh
//! LUNA_LLAMA_BIN_DIR=~/Dev/lunarca-p0/bin/llama-b11429 \
//! LUNA_LLAMA_CHAT_GGUF=~/Dev/lunarca-p0/models/qwen3.5-0.8b-q8_0.gguf \
//! cargo test -p luna-provider --test llm_live -- --ignored
//! ```
//! 主题生成 / 更新探针按 16k 窗设计，换 4B 时一并给 `LUNA_LLAMA_CTX=16384`。

mod support;

/// 本机对话 Provider：起一个只带 `LUNA_LLAMA_CHAT_GGUF` 的路由器。路由器活多久由调用方
/// 持有的那个 `Arc` 决定，故两样一起返回。
fn local_chat() -> (std::sync::Arc<support::Router>, LlmHttpProvider, String) {
    let router = support::start_chat();
    let model = support::path("LUNA_LLAMA_CHAT_GGUF")
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "chat".to_owned());
    let provider = LlmHttpProvider::new(LlmHttpConfig::local_engine(router.clone(), "chat"));
    (router, provider, model)
}

use futures::StreamExt as _;
use luna_core::{
    AIProvider, CancelToken, Job, JobEvent, JobKind, JobOrigin, Lang, PromptScope, ResultPayload,
    SummaryStyle, TextRef,
};
use luna_provider::{check_invariants, LlmHttpConfig, LlmHttpProvider, TerminalKind};
use luna_vault::{
    parse_topic_generation, parse_topic_update, topic_generation_payload, topic_update_payload,
    TopicGeneration, TopicScope, TopicUpdate, Vault,
};

#[tokio::test]
#[ignore = "需要本机 llama-server 与 GGUF"]
async fn local_engine_translate_smoke() {
    let (_router, provider, _model) = local_chat();
    let job = Job::new(
        JobKind::Translate {
            text: TextRef::Inline("Hello, world.".into()),
            source: Some(Lang::from("en")),
            target: Lang::from("zh"),
            instruction: None,
            template: None,
        },
        JobOrigin::PopupTranslate,
    );

    let summary = check_invariants(provider.execute(job, CancelToken::new()).await).await;
    assert_eq!(summary.terminal, Some(TerminalKind::Finished));
    assert!(summary.events > 2, "应有 Started + 若干 token + Finished");
}

/// 虚构 E05 夹具经真实 `AIProvider` 走一次双项目/无关主题的初稿预览。
/// 只读、不发布；`JobQueue`、应用库代际、增量计划及所有者语义验收仍须另验。
///
/// ```sh
/// LUNA_LLAMA_BIN_DIR=… LUNA_LLAMA_CHAT_GGUF=…/qwen3.5-4b-q4_k_m.gguf LUNA_LLAMA_CTX=16384 \
///   cargo test -p luna-provider --test llm_live topic_generation_provider_smoke -- --ignored --nocapture
/// ```
#[tokio::test]
#[ignore = "需要本机 llama-server 与 GGUF；小模型输出可能不达标"]
async fn topic_generation_provider_smoke() {
    let (_router, provider, _model) = local_chat();
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../evals/fixtures/topic_generation_model.json"
    ))
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::open(dir.path()).unwrap();
    vault
        .write(
            "Notes/同一份周会.md",
            fixture["source_markdown"].as_str().unwrap(),
        )
        .unwrap();
    let batch = vault
        .topic_evidence(
            &["Notes/同一份周会.md".to_owned()],
            luna_vault::SourcePolicy::FORMAL_ONLY,
        )
        .unwrap();
    assert!(batch.input_complete);
    assert_eq!(batch.evidence.len(), 2);

    for case in fixture["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let scope: TopicScope = serde_json::from_value(case["scope"].clone()).unwrap();
        let allowed_ids = case["allowed_ids"].as_array().unwrap();
        let expected_index = allowed_ids.first().map(|id| {
            batch
                .evidence
                .iter()
                .position(|item| item.id == id.as_str().unwrap())
                .expect("夹具 ID 必须来自 Vault 提取结果")
        });
        assert_eq!(
            expected_index.is_some(),
            case["expected_status"] == "claims",
            "{name}: 夹具状态与允许引用不一致"
        );
        let payload = topic_generation_payload(&scope, &batch.evidence, batch.input_complete)
            .expect("Vault 片段应组成封闭输入");
        let payload_value: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(payload_value["evidence"], case["evidence"]);
        assert_eq!(payload_value["input_complete"], case["input_complete"]);
        let instruction = luna_core::prompt::registry().instruction_for_locale(
            PromptScope::Ingest,
            "topic_generation",
            None,
            &[],
        );
        let mut job = Job::new(
            JobKind::Summarize {
                text: TextRef::Inline(payload),
                style: SummaryStyle::Faithful,
                instruction: Some(instruction),
            },
            JobOrigin::Ingest,
        );
        job.options.output_budget = Some(2_048);

        let mut stream = provider.execute(job, CancelToken::new()).await;
        let mut terminal = None;
        while let Some(event) = stream.next().await {
            match event {
                JobEvent::Finished(result) => {
                    assert!(
                        result.output_truncated.is_none(),
                        "{name}: Provider 输出触顶"
                    );
                    let ResultPayload::Text(raw) = result.payload else {
                        panic!("{name}: Provider 未返回 Text 载荷");
                    };
                    terminal = Some(raw);
                }
                JobEvent::Failed(error) => panic!("{name}: Provider 失败 {}", error.code()),
                JobEvent::Cancelled => panic!("{name}: Provider 意外取消"),
                _ => {}
            }
        }
        let raw = terminal.expect("Provider 流必须有完成终态");
        let generation = parse_topic_generation(&scope, &batch.evidence, &raw, true)
            .unwrap_or_else(|error| panic!("{name}: 模型输出未通过封闭协议：{error}"));
        let topic_src = scope.includes[0].clone();
        match (expected_index, &generation) {
            (Some(index), TopicGeneration::Claims { claims }) => {
                assert!(
                    claims.iter().all(|claim| claim.evidence_indices == [index]),
                    "{name}: 引用了其他项目的小节"
                );
                let preview = vault
                    .scoped_topic_preview_from_generation(
                        &format!("Knowledge/{name}.md"),
                        Some(&topic_src),
                        scope,
                        &batch.evidence,
                        &generation,
                    )
                    .expect("预览应在原始来源版本上成功");
                assert!(preview.is_some(), "{name}: 有据声明应能只读预览");
            }
            (None, TopicGeneration::NoOp { .. }) => {
                assert!(
                    vault
                        .scoped_topic_preview_from_generation(
                            "Knowledge/无关项目.md",
                            Some(&topic_src),
                            scope,
                            &batch.evidence,
                            &generation,
                        )
                        .unwrap()
                        .is_none(),
                    "无关主题不应生成空主题页"
                );
            }
            _ => panic!("{name}: 模型返回了错误的生成状态"),
        }
    }
}

/// 虚构增量夹具经真实 `AIProvider` 逐次打分：协议、范围、终态与分歧指向分别计数。
/// 它是诊断探针而不是语义验收——不 panic 于语义失误，只在协议之外的环境故障时失败；
/// 结果按行打印，供实施计划留证。`JobQueue`、应用库代际与所有者语义验收仍须另验。
///
/// ```sh
/// LUNA_LLAMA_BIN_DIR=… LUNA_LLAMA_CHAT_GGUF=…/qwen3.5-4b-q4_k_m.gguf LUNA_LLAMA_CTX=16384 \
///   LUNA_PROBE_REPEATS=3 cargo test -p luna-provider --test llm_live topic_update_provider_probe \
///   -- --ignored --nocapture
/// ```
#[tokio::test]
#[ignore = "需要本机 llama-server 与 GGUF；小模型输出可能不达标"]
async fn topic_update_provider_probe() {
    let repeats: usize = std::env::var("LUNA_PROBE_REPEATS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(3);
    let (_router, provider, model) = local_chat();
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../evals/fixtures/topic_update_model.json"
    ))
    .unwrap();
    let instruction = luna_core::prompt::registry().instruction_for_locale(
        PromptScope::Ingest,
        "topic_update",
        None,
        &[],
    );
    let (mut runs, mut passed) = (0usize, 0usize);
    for case in fixture["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        let baseline_path = fixture["baseline_source_path"].as_str().unwrap();
        vault
            .write(
                baseline_path,
                fixture["baseline_source_markdown"].as_str().unwrap(),
            )
            .unwrap();
        let baseline_hash = vault.body_fingerprint(baseline_path).unwrap();
        let locators: Vec<luna_vault::EvidenceLocator> = fixture["existing_claims"]
            .as_array()
            .unwrap()
            .iter()
            .map(|claim| luna_vault::EvidenceLocator {
                heading_path: vec!["旧计划".into(), "项目 A".into()],
                quote: claim["quote"].as_str().unwrap().into(),
                context: String::new(),
                line_hint: None,
                time_start_ms: None,
                time_end_ms: None,
            })
            .collect();
        let sources: Vec<[luna_vault::ClaimSource<'_>; 1]> = locators
            .iter()
            .map(|locator| {
                [luna_vault::ClaimSource {
                    path: baseline_path,
                    title: "旧计划",
                    source_hash: &baseline_hash,
                    locator: locator.clone(),
                    complete_input: true,
                }]
            })
            .collect();
        let claims: Vec<luna_vault::NewClaim<'_>> = fixture["existing_claims"]
            .as_array()
            .unwrap()
            .iter()
            .zip(&sources)
            .map(|(claim, sources)| luna_vault::NewClaim {
                id: uuid::Uuid::now_v7(),
                text: claim["text"].as_str().unwrap(),
                kind: luna_vault::ClaimKind::Synthesis,
                sources,
            })
            .collect();
        let scope: TopicScope = serde_json::from_value(case["scope"].clone()).unwrap();
        let target = "Knowledge/项目 A.md";
        let content = vault
            .scoped_topic_content(target, Some("项目/A"), scope.clone(), &claims)
            .unwrap();
        vault.publish_scoped_topic(target, &content).unwrap();
        let source_path = case["source_path"].as_str().unwrap();
        vault
            .write(source_path, case["source_markdown"].as_str().unwrap())
            .unwrap();
        let context = vault.topic_update_context(target).unwrap();
        let batch = vault
            .topic_evidence(
                &[source_path.to_owned()],
                luna_vault::SourcePolicy::FORMAL_ONLY,
            )
            .unwrap();
        let allowed: Vec<usize> = batch
            .evidence
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                case["allowed_quotes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|quote| quote.as_str() == Some(item.excerpt.as_str()))
            })
            .map(|(index, _)| index)
            .collect();
        let payload = topic_update_payload(
            &context.scope,
            &context.existing,
            &batch.evidence,
            batch.input_complete,
        )
        .expect("夹具应组成封闭输入");
        for attempt in 0..repeats {
            runs += 1;
            let mut job = Job::new(
                JobKind::Summarize {
                    text: TextRef::Inline(payload.clone()),
                    style: SummaryStyle::Faithful,
                    instruction: Some(instruction.clone()),
                },
                JobOrigin::Ingest,
            );
            job.options.output_budget = Some(2_048);
            let mut stream = provider.execute(job, CancelToken::new()).await;
            let mut terminal = None;
            while let Some(event) = stream.next().await {
                match event {
                    JobEvent::Finished(result) => terminal = Some(result),
                    JobEvent::Failed(error) => panic!("{name}: Provider 失败 {}", error.code()),
                    JobEvent::Cancelled => panic!("{name}: Provider 意外取消"),
                    _ => {}
                }
            }
            let result = terminal.expect("Provider 流必须有完成终态");
            let complete = result.output_truncated.is_none();
            let ResultPayload::Text(raw) = result.payload else {
                panic!("{name}: Provider 未返回 Text 载荷");
            };
            let parsed = parse_topic_update(
                &context.scope,
                &context.existing,
                &batch.evidence,
                &raw,
                complete,
            );
            let (verdict, detail) = match (&parsed, case["expected_status"].as_str().unwrap()) {
                // 夹具全为虚构内容；失败时留原始输出前缀，便于区分围栏、多余键与类别错误。
                (Err(error), _) => (
                    false,
                    format!(
                        "protocol_error={error} raw={:?}",
                        raw.chars().take(400).collect::<String>()
                    ),
                ),
                (Ok(TopicUpdate::NoOp { .. } | TopicUpdate::AllDuplicates { .. }), "no_new") => {
                    (true, "no_new".into())
                }
                (Ok(TopicUpdate::Claims { claims, duplicates }), "claims") => {
                    let in_scope = claims.iter().all(|update| {
                        update
                            .claim
                            .evidence_indices
                            .iter()
                            .all(|index| allowed.contains(index))
                    });
                    let conflict_ok = case["expect_conflict_with"].as_str().is_none_or(|_| {
                        claims
                            .iter()
                            .any(|update| update.conflicts_with == Some(context.existing[0].id))
                    });
                    let texts: Vec<&str> = claims
                        .iter()
                        .map(|update| update.claim.text.as_str())
                        .collect();
                    (
                        in_scope && conflict_ok,
                        format!(
                            "in_scope={in_scope} conflict_ok={conflict_ok} verbatim_duplicates={duplicates} claims={texts:?}"
                        ),
                    )
                }
                (Ok(TopicUpdate::Claims { claims, .. }), _) => {
                    let texts: Vec<&str> = claims
                        .iter()
                        .map(|update| update.claim.text.as_str())
                        .collect();
                    (false, format!("unexpected_claims={texts:?}"))
                }
                (Ok(_), _) => (false, "unexpected_no_new".into()),
            };
            if verdict {
                passed += 1;
            }
            println!(
                "PROBE model={model} case={name} attempt={} pass={verdict} {detail}",
                attempt + 1
            );
        }
    }
    println!("PROBE_SUMMARY model={model} passed={passed}/{runs}");
}

/// 组织服务端**原生**推理面的跨端冒烟：`translate` 的两条泳道各打一次。
///
/// **它守的不是「服务端此刻好不好」，而是「客户端把服务端说的话翻成了哪一句」**。
/// 真机 2026-08-31：同一台服务端、同一把凭据、同一秒——`translate/batch` 200 正常，
/// `translate/realtime` 回 `502 {"error":"upstream_unavailable"}`（服务端按
/// `(capability, class)` 取链，而 `translate` 是今天唯一真的分了两条泳道的能力）。
/// 客户端当时把它压成 `UPSTREAM`，界面显示「AI 引擎返回错误，请重试」，用户手上
/// 没有任何一个能改变结果的旋钮，而屏幕让他重试。
///
/// 故断言是**契约**而不是状态：不论这一刻哪条泳道健康，原生端点上的失败都不许落进
/// `UPSTREAM`——那一档只属于「我们直连的那个上游回了错」（ADR-097 ②）。
///
/// 跑法：
/// ```sh
/// LUNA_NATIVE_BASE_URL=https://10.0.0.62:8443 \
/// LUNA_NATIVE_BEARER=$(security find-generic-password -s lunarca-cloud \
///     -a '10.0.0.62:8443/access-token' -w) \
///   cargo test -p luna-provider --test llm_live native_endpoint_lane -- --ignored --nocapture
/// ```
#[tokio::test]
#[ignore = "需要可达的 Lunarca 原生服务端与有效会话凭据"]
async fn native_endpoint_lane_failures_are_server_side_never_bare_upstream() {
    use luna_core::JobError;

    struct StaticBearer(String);
    #[async_trait::async_trait]
    impl luna_provider::asr_remote::BearerSource for StaticBearer {
        async fn bearer(&self) -> Option<String> {
            Some(self.0.clone())
        }
        async fn refresh(&self, _rejected: &str) -> Option<String> {
            None // 冒烟不做轮换：401 就该以 UNAUTHORIZED 收尾，那本身也是被断言的形态之一
        }
    }

    let base = std::env::var("LUNA_NATIVE_BASE_URL").expect("缺 LUNA_NATIVE_BASE_URL");
    let bearer = std::env::var("LUNA_NATIVE_BEARER").expect("缺 LUNA_NATIVE_BEARER");

    // 两条泳道由 `JobOrigin` 推导（ADR-083 ③），测试里一个字节的档位判断都不写：
    // 弹窗翻译 → 实时档，批量转译 → 批量档。
    for (origin, lane) in [
        (JobOrigin::PopupTranslate, "realtime"),
        (JobOrigin::Batch, "batch"),
    ] {
        let provider = luna_provider::CloudLlmProvider::new(luna_provider::CloudLlmConfig {
            api_base: base.clone(),
            display_name: "组织服务端".into(),
            bearer: std::sync::Arc::new(StaticBearer(bearer.clone())),
            gate: std::sync::Arc::new(luna_core::AllowOutbound),
            allowed: None,
        });
        let job = Job::new(
            JobKind::Translate {
                text: TextRef::Inline("Production".into()),
                source: None,
                target: Lang::from("zh"),
                instruction: None,
                template: None,
            },
            origin,
        );

        let mut stream = provider.execute(job, CancelToken::new()).await;
        let mut finished = None;
        let mut failure = None;
        while let Some(ev) = stream.next().await {
            match ev {
                JobEvent::Finished(result) => {
                    finished = match result.payload {
                        ResultPayload::Text(text) => Some(text),
                        other => panic!("翻译应回 Text 载荷，实得 {other:?}"),
                    };
                }
                JobEvent::Failed(err) => failure = Some(err),
                _ => {}
            }
        }

        match (finished, failure) {
            (Some(text), None) => println!("[{lane}] 正常：{}", text.trim()),
            (_, Some(err)) => {
                println!("[{lane}] 失败：{} / {err:?}", err.code());
                assert_ne!(
                    err.code(),
                    "UPSTREAM",
                    "[{lane}] 原生服务端的失败落进了 UPSTREAM——那一档只属于「我们直连的\
                     那个上游回了错」。落在这里的话，界面给的是「AI 引擎返回错误，请重试」，\
                     而用户手上一个能改变结果的旋钮都没有（ADR-097 ②）"
                );
                assert!(
                    matches!(
                        err,
                        JobError::ServerSideFailure { .. }
                            | JobError::ServerMisconfigured { .. }
                            | JobError::RouteLaneEmpty { .. }
                            | JobError::Unauthorized { .. }
                            | JobError::QuotaExhausted { .. }
                            | JobError::RateLimited { .. }
                            | JobError::ProviderUnavailable { .. }
                            | JobError::Timeout
                    ),
                    "[{lane}] 落进了一个没有可执行动作的码"
                );
            }
            (None, None) => panic!("[{lane}] 流结束却既无终态结果也无失败"),
        }
    }
}

/// 人工验收用的固定评测集（`evals/fixtures/topic_review_set.json`，60 组虚构样本）经真实
/// `AIProvider` 逐组跑一次增量，导出评审 CSV：每条新声明一行，带所引片段与三项机器初筛
/// （引用是否落在本主题范围、是否出现范围外字样、确定程度/否定词是否保留）；`relevant` /
/// `supported` / `certainty_kept` / `conflict_kept` 四列留空给人工（标注办法见
/// `evals/README.md`）。只做导出、不做语义断言——机器初筛不替代评审。
///
/// 默认连随包推理引擎（`LUNA_LLAMA_BIN_DIR` + `LUNA_LLAMA_CHAT_GGUF`）；设了 `LUNA_COMPUTE_BASE_URL` 则连该端点
/// （`LUNA_COMPUTE_API_KEY` / `LUNA_COMPUTE_MODEL`）。`LUNA_REVIEW_OUT` 覆盖输出路径，
/// 缺省写到 `evals/review/out/<模型>.csv`（已忽略，不入库）。
///
/// ```sh
/// LUNA_LLAMA_BIN_DIR=… LUNA_LLAMA_CHAT_GGUF=…/qwen3.5-4b-q4_k_m.gguf LUNA_LLAMA_CTX=16384 \
///   cargo test -p luna-provider --test llm_live topic_review_set_export -- --ignored --nocapture
/// ```
#[tokio::test]
#[ignore = "需要可用的模型端点；产出供人工评审，不做语义断言"]
async fn topic_review_set_export() {
    // 只评本机随包引擎（第三方端点已移除，ADR-155）。
    let (_router, provider, model) = local_chat();
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../evals/fixtures/topic_review_set.json"
    ))
    .unwrap();
    let instruction = luna_core::prompt::registry().instruction_for_locale(
        PromptScope::Ingest,
        "topic_update",
        None,
        &[],
    );
    let prompt_revision =
        luna_sync_core::hashing::ContentHash::of(instruction.as_bytes()).to_string();
    let prompt_revision = &prompt_revision[..prompt_revision.len().min(16)];
    let strings = |value: &serde_json::Value| -> Vec<String> {
        value
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };

    let mut rows = vec![REVIEW_HEADER.join(",")];
    let (mut groups, mut status_ok, mut action_ok, mut scope_ok, mut failed) = (0, 0, 0, 0, 0);
    // 开头连续几组都是端点拒绝（地址或密钥不对）就停下，不把整批跑成一张失败表。
    let mut answered = false;
    let mut endpoint_failures = 0usize;
    for group in fixture["groups"].as_array().unwrap() {
        groups += 1;
        let id = group["id"].as_str().unwrap();
        let kind = group["kind"].as_str().unwrap();
        let traits = strings(&group["traits"]).join("|");
        let topic = group["topic"].as_str().unwrap();
        let tag = group["tag"].as_str().unwrap();
        let allowed_quotes = strings(&group["allowed_quotes"]);
        let must_not = strings(&group["expect"]["must_not"]);
        let keep_words = strings(&group["expect"]["keep_words"]);
        let expected_status = group["expect"]["status"].as_str().unwrap();
        let expected_action = group["expect"]["action"].as_str().unwrap();

        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        let baseline_path = "Notes/基线.md";
        let existing = group["existing"].as_array().unwrap();
        let baseline: String = existing
            .iter()
            .map(|claim| format!("{}\n\n", claim["quote"].as_str().unwrap()))
            .collect();
        vault
            .write(
                baseline_path,
                &format!("# 基线\n\n## {topic}\n\n{baseline}"),
            )
            .unwrap();
        let baseline_hash = vault.body_fingerprint(baseline_path).unwrap();
        let locators: Vec<luna_vault::EvidenceLocator> = existing
            .iter()
            .map(|claim| luna_vault::EvidenceLocator {
                heading_path: vec!["基线".into(), topic.into()],
                quote: claim["quote"].as_str().unwrap().into(),
                context: String::new(),
                line_hint: None,
                time_start_ms: None,
                time_end_ms: None,
            })
            .collect();
        let sources: Vec<[luna_vault::ClaimSource<'_>; 1]> = locators
            .iter()
            .map(|locator| {
                [luna_vault::ClaimSource {
                    path: baseline_path,
                    title: "基线",
                    source_hash: &baseline_hash,
                    locator: locator.clone(),
                    complete_input: true,
                }]
            })
            .collect();
        let claims: Vec<luna_vault::NewClaim<'_>> = existing
            .iter()
            .zip(&sources)
            .map(|(claim, sources)| luna_vault::NewClaim {
                id: uuid::Uuid::now_v7(),
                text: claim["text"].as_str().unwrap(),
                kind: luna_vault::ClaimKind::Synthesis,
                sources,
            })
            .collect();
        let target = format!("Knowledge/{topic}.md");
        let content = vault
            .scoped_topic_content(&target, Some(tag), TopicScope::from_tag(tag), &claims)
            .unwrap();
        vault.publish_scoped_topic(&target, &content).unwrap();

        // 速记只有整理成知识笔记后才进主题（缺省记录态不合成）；录音与导入转写走权威转写外壳，
        // 并按用户已授权资料来源的口径取证（PRD-16 来源授权）。
        let (folder, policy) = match kind {
            "note" => ("Notes", luna_vault::SourcePolicy::FORMAL_ONLY),
            "quick_note" => ("QuickNotes", luna_vault::SourcePolicy::FORMAL_ONLY),
            "recording" => (
                "Recordings",
                luna_vault::SourcePolicy {
                    transcripts: true,
                    quick_notes: false,
                },
            ),
            "transcript" => (
                "Transcripts",
                luna_vault::SourcePolicy {
                    transcripts: true,
                    quick_notes: false,
                },
            ),
            other => panic!("{id}: 未知来源类型 {other}"),
        };
        let mut paths = Vec::new();
        for (n, source) in strings(&group["sources"]).iter().enumerate() {
            let path = format!("{folder}/{id}-{}.md", n + 1);
            let markdown = match kind {
                "note" => source.clone(),
                "quick_note" => format!("---\nluna:\n  schema: 1\n  type: note\n---\n{source}"),
                _ => format!(
                    "---\nluna:\n  schema: 1\n  type: {}\n  transcript_authoritative: true\n---\n\
                     # {id}\n\n<!-- luna:transcript -->\n{source}\n<!-- /luna:transcript -->\n",
                    if kind == "recording" {
                        "recording"
                    } else {
                        "transcript"
                    }
                ),
            };
            vault.write(&path, &markdown).unwrap();
            paths.push(path);
        }

        let context = vault.topic_update_context(&target).unwrap();
        let batch = match vault.topic_evidence(&paths, policy) {
            Ok(batch) => batch,
            Err(error) => panic!("{id}: 夹具来源未通过取证 {error:?}"),
        };
        let payload = topic_update_payload(
            &context.scope,
            &context.existing,
            &batch.evidence,
            batch.input_complete,
        )
        .expect("夹具应组成封闭输入");
        let mut job = Job::new(
            JobKind::Summarize {
                text: TextRef::Inline(payload),
                style: SummaryStyle::Faithful,
                instruction: Some(instruction.clone()),
            },
            JobOrigin::Ingest,
        );
        job.options.output_budget = Some(2_048);
        let mut stream = provider.execute(job, CancelToken::new()).await;
        let mut outcome = Err("no_terminal".to_owned());
        while let Some(event) = stream.next().await {
            match event {
                JobEvent::Finished(result) => {
                    let complete = result.output_truncated.is_none();
                    outcome = match result.payload {
                        ResultPayload::Text(raw) => Ok((raw, complete)),
                        _ => Err("non_text_payload".into()),
                    };
                }
                JobEvent::Failed(error) => outcome = Err(format!("provider_{}", error.code())),
                JobEvent::Cancelled => outcome = Err("cancelled".into()),
                _ => {}
            }
        }

        let row = |status: &str, action: &str, claim: &str, cited: &[String], auto: [&str; 3]| {
            let mut fields = vec![
                id.to_owned(),
                kind.to_owned(),
                traits.clone(),
                model.clone(),
                prompt_revision.to_owned(),
                expected_status.to_owned(),
                expected_action.to_owned(),
                status.to_owned(),
                action.to_owned(),
                claim.to_owned(),
                cited.join(" ‖ "),
            ];
            fields.extend(auto.iter().map(|value| (*value).to_owned()));
            fields.extend(std::iter::repeat_n(String::new(), 6));
            fields
                .iter()
                .map(|field| csv_field(field))
                .collect::<Vec<_>>()
                .join(",")
        };
        let parsed = match outcome {
            Err(code) => {
                failed += 1;
                rows.push(row(&code, "", "", &[], ["", "", ""]));
                println!("REVIEW {id} status={code}");
                endpoint_failures += 1;
                assert!(
                    answered || endpoint_failures < 3,
                    "前 3 组都被端点拒绝（{code}）：先核对 LUNA_COMPUTE_BASE_URL 与密钥是否配套"
                );
                continue;
            }
            Ok((raw, complete)) => (
                parse_topic_update(
                    &context.scope,
                    &context.existing,
                    &batch.evidence,
                    &raw,
                    complete,
                ),
                raw,
            ),
        };
        answered = true;
        match parsed {
            // 样本全为虚构内容；留原始输出前缀，便于区分围栏、多余键与类别错误。
            (Err(error), raw) => {
                failed += 1;
                let prefix: String = raw.chars().take(400).collect();
                rows.push(row(
                    "protocol_error",
                    "",
                    &format!("{error}: {prefix}"),
                    &[],
                    ["", "", ""],
                ));
                println!("REVIEW {id} status=protocol_error {error} raw={prefix:?}");
            }
            (Ok(TopicUpdate::NoOp { .. } | TopicUpdate::AllDuplicates { .. }), _) => {
                let hit = expected_status == "no_new";
                status_ok += usize::from(hit);
                action_ok += usize::from(hit);
                scope_ok += 1;
                rows.push(row("no_new", "none", "", &[], ["yes", "", ""]));
                println!("REVIEW {id} status=no_new expected={expected_status}");
            }
            (Ok(TopicUpdate::Claims { claims, .. }), _) => {
                status_ok += usize::from(expected_status == "claims");
                let actions: Vec<&str> = claims
                    .iter()
                    .map(|update| {
                        if update.replaces.is_some() {
                            "replace"
                        } else if update.conflicts_with.is_some() {
                            "conflict"
                        } else {
                            "append"
                        }
                    })
                    .collect();
                action_ok += usize::from(actions.contains(&expected_action));
                let mut group_in_scope = true;
                for (update, action) in claims.iter().zip(&actions) {
                    let cited: Vec<String> = update
                        .claim
                        .evidence_indices
                        .iter()
                        .map(|&index| batch.evidence[index].excerpt.clone())
                        .collect();
                    let in_scope = cited.iter().all(|excerpt| {
                        allowed_quotes.iter().any(|quote| {
                            excerpt.contains(quote.as_str()) || quote.contains(excerpt.as_str())
                        })
                    });
                    group_in_scope &= in_scope;
                    let text = &update.claim.text;
                    let hits: Vec<&str> = must_not
                        .iter()
                        .filter(|word| text.contains(word.as_str()))
                        .map(String::as_str)
                        .collect();
                    let missing: Vec<&str> = keep_words
                        .iter()
                        .filter(|word| !text.contains(word.as_str()))
                        .map(String::as_str)
                        .collect();
                    rows.push(row(
                        "claims",
                        action,
                        text,
                        &cited,
                        [
                            if in_scope { "yes" } else { "no" },
                            &hits.join("|"),
                            &missing.join("|"),
                        ],
                    ));
                }
                scope_ok += usize::from(group_in_scope);
                println!(
                    "REVIEW {id} status=claims expected={expected_status} actions={actions:?} \
                     expected_action={expected_action} in_scope={group_in_scope}"
                );
            }
        }
    }

    let out = std::env::var("LUNA_REVIEW_OUT").map_or_else(
        |_| {
            let name: String = model
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                .collect();
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../evals/review/out")
                .join(format!("{name}.csv"))
        },
        // 相对路径按仓库根解析：`cargo test` 的工作目录是本 crate，照字面写会落进
        // `crates/luna-provider/evals/…`——不在忽略规则里，下一次 `git add` 就进仓了。
        |path| {
            let path = std::path::PathBuf::from(path);
            if path.is_absolute() {
                path
            } else {
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../..")
                    .join(path)
            }
        },
    );
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    std::fs::write(&out, rows.join("\n") + "\n").unwrap();
    println!(
        "REVIEW_SUMMARY model={model} prompt={prompt_revision} groups={groups} \
         status_match={status_ok} action_match={action_ok} in_scope={scope_ok} failed={failed} \
         out={}",
        out.display()
    );
}

/// 评审 CSV 列：前半为样本与模型输出、三列机器初筛，末六列与
/// `evals/review/annotation-template.csv` 的人工列同名同序。
const REVIEW_HEADER: [&str; 20] = [
    "sample_id",
    "kind",
    "traits",
    "model_revision",
    "prompt_revision",
    "expected_status",
    "expected_action",
    "status",
    "action",
    "claim_text",
    "cited_excerpts",
    "auto_in_scope",
    "auto_must_not_hits",
    "auto_keep_words_missing",
    "relevant",
    "supported",
    "certainty_kept",
    "conflict_kept",
    "reason",
    "reviewer",
];

fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}
