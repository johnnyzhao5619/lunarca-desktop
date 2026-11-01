//! `luna` CLI——与 GUI **平级**的前端，复用同一 `JobQueue` 执行主干（ADR-003、TDD-09）。
//!
//! 已有：`luna translate`（Phase 2）、`luna export`（2026-09-06，PRD-11-006）、`luna bench-asr`
//! （2026-09-06，ADR-115 收益量化）、只读的 `luna vault` / `luna note`（2026-10-05，ADR-141 ④，
//! 见 [`vault_cmd`]）。其余命令
//! （transcribe/batch/model/...，TDD-09 §2）为 P1。脚本约定：结果走 stdout、诊断/错误走
//! stderr、退出码语义化（TDD-09 §3）。
#![forbid(unsafe_code)]

mod local_router;
mod model_eval;
mod vault_cmd;

use clap::{Parser, Subcommand, ValueEnum};
use luna_core::{
    AudioRef, CancelToken, Job, JobEvent, JobKind, JobOrigin, Lang, PartialPayload, ResultPayload,
    TextRef,
};
use luna_jobqueue::{AdmissionCapacity, JobQueue, JobSink};
use luna_provider::{LlmHttpConfig, LlmHttpProvider, ProviderRegistry};
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

/// Lunarca 命令行。
#[derive(Parser)]
#[command(
    name = "luna",
    version,
    about = "Lunarca CLI（与 GUI 共享同一执行主干）"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 翻译一段文本（经 JobQueue → 随包推理引擎，本命令自己起一个路由器）。
    Translate {
        /// 待翻译文本。
        text: String,
        /// 目标语言（BCP-47，如 zh / en / ja）。
        #[arg(long, default_value = "zh")]
        target: String,
        /// 对话模型 GGUF 文件。
        #[arg(long)]
        gguf: PathBuf,
        /// 引擎资源根（其下 `llama/` 为引擎包；缺省工作树的 `src-tauri/resources`）。
        #[arg(long)]
        resources: Option<PathBuf>,
    },
    /// 转写基准：同一段音频跑 N 次，报模型加载、每次转写耗时与 RTF（CoreML 收益量化，ADR-115）。
    BenchAsr {
        /// 音频 / 视频文件（wav / mp3 / m4a / flac / … 由内置解码器解到 16k 单声道）。
        audio: PathBuf,
        /// whisper ggml 模型文件路径（旁边若有 `<名>-encoder.mlmodelc` 即走 CoreML 编码器）。
        #[arg(long)]
        model: PathBuf,
        /// 运行次数（第一次含模型加载后的首推理预热，全部逐次列出）。
        #[arg(long, default_value_t = 3)]
        runs: u32,
        /// 后端：cpu / metal（macOS 缺省 metal，其余平台 cpu）。
        #[arg(long, value_enum)]
        backend: Option<BackendArg>,
        /// 语种提示（缺省自动检测）。
        #[arg(long)]
        lang: Option<String>,
        /// 参考文本文件（UTF-8）。给了就按首次转写结果报 CER（按字）与 WER（按空白分词）；
        /// 归一化：小写、去标点与空白（CER）/ 折叠空白（WER）。
        #[arg(long)]
        reference: Option<PathBuf>,
    },
    /// 嵌入基准（PRD-10-017 门禁）：用应用同一张 profile 表与分块策略对一个语料建库，报
    /// 建库时间 / 逐批延迟 / 引擎内存增量 / 索引占用；给评测文件时再报召回与近重复准确率。
    BenchEmbed {
        /// profile 键（bge-m3 / qwen3-embedding:0.6b）；须对应一个目录里的嵌入 GGUF 条目。
        #[arg(long)]
        model: String,
        /// 该模型的 GGUF 文件（随包推理引擎加载它；本命令自己起一个路由器）。
        #[arg(long)]
        gguf: PathBuf,
        /// 引擎资源根（其下 `llama/` 为引擎包；缺省工作树的 `src-tauri/resources`）。
        #[arg(long)]
        resources: Option<PathBuf>,
        /// 语料目录（递归取全部 .md）。
        #[arg(long)]
        corpus: PathBuf,
        /// 评测文件（JSON：`queries[{text, expect}]` + `duplicates[{a, b, duplicate}]`）。
        #[arg(long)]
        eval: Option<PathBuf>,
        /// 覆盖分块目标字数（profile 没登记分块基准的模型必须给；这就是基准要定下来的数）。
        #[arg(long)]
        target_chars: Option<usize>,
        /// 覆盖分块硬上限字数（缺省 = 目标 × 1.5）。
        #[arg(long)]
        hard_chars: Option<usize>,
        /// 每批送进引擎的分片数。
        #[arg(long, default_value_t = 16)]
        batch: usize,
        /// 以 JSON 一行输出汇总（便于记进基准表）。
        #[arg(long)]
        json: bool,
    },
    /// 目录条目的评测证据（ADR-145 ⑤）：用随包引擎与应用真实的提示词跑固定评测集，输出报告
    /// JSON（不含模型输出原文），在管理台上传给本地模型目录的生效闸。服务端用同一判据重算。
    ModelEval {
        /// 目录条目 id（如 qwen3.5-2b-q8_0-gguf）。
        catalog_id: String,
        /// 被测权重 GGUF（sha256 必须与条目一致）。
        #[arg(long)]
        gguf: PathBuf,
        /// 目录外的条目：给出它的 JSON（一条 `ModelEntry`）。
        #[arg(long)]
        entry: Option<PathBuf>,
        /// 嵌入条目替换同一 profile 时的参照模型 GGUF（报逐条最小余弦）。
        #[arg(long)]
        reference: Option<PathBuf>,
        /// 引擎资源根（其下 `llama/` 为引擎包；缺省工作树的 `src-tauri/resources`）。
        #[arg(long)]
        resources: Option<PathBuf>,
        /// 报告写到这个文件（缺省打到标准输出）。
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// 库：搜索、更新索引（只读打开，不写库）。
    Vault {
        #[command(subcommand)]
        command: vault_cmd::VaultCommand,
    },
    /// 笔记：列清单、看正文；新建交给应用确认。
    Note {
        #[command(subcommand)]
        command: vault_cmd::NoteCommand,
    },
    /// 导出一篇笔记的转写为字幕 / 文本，或导出笔记正文（与 GUI 同一份导出器）。
    Export {
        /// 笔记在 Vault 内的相对路径（如 `Recordings/会议.md`）。
        note: String,
        /// Vault 根目录；缺省依次取 `LUNA_VAULT_ROOT`、GUI 配置里的 `storage.vault_root`。
        #[arg(long)]
        vault: Option<PathBuf>,
        /// 目标格式。
        #[arg(long, value_enum, default_value_t = ExportFormatArg::Srt)]
        format: ExportFormatArg,
        /// 内容：仅原文 / 仅译文（缺译回退原文）/ 双语对照。
        #[arg(long, value_enum, default_value_t = ContentArg::Source)]
        content: ContentArg,
        /// 整体时移（毫秒，可负；结果钳在 0）。
        #[arg(long, default_value_t = 0)]
        offset_ms: i64,
        /// SRT / VTT 写 UTF-8 BOM。
        #[arg(long)]
        bom: bool,
        /// 每行最大字数覆盖（缺省按语言自动；0 = 不限）。
        #[arg(long)]
        line_len: Option<usize>,
        /// 每条最大行数覆盖（缺省 2；LRC 恒 1）。
        #[arg(long)]
        lines: Option<u8>,
        /// 不输出「名字: 」说话人前缀。
        #[arg(long)]
        no_speaker: bool,
        /// 中文字形转换：keep 不转 / simplified 简体 / tw 繁体·台湾 / hk 繁体·香港。
        #[arg(long, value_enum, default_value_t = HanArg::Keep)]
        han: HanArg,
        /// 写到文件；缺省写 stdout。
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
}

/// `luna export --format`。`txt-timestamped` = 行首带 `[HH:MM:SS.mmm]` 的文本；`md` = 笔记正文。
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum ExportFormatArg {
    Srt,
    Vtt,
    Ass,
    Lrc,
    Bcc,
    Txt,
    TxtTimestamped,
    Md,
}

/// `luna export --han`（PRD-11-013）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum HanArg {
    Keep,
    Simplified,
    Tw,
    Hk,
}

#[derive(Clone, Copy, ValueEnum)]
enum BackendArg {
    Cpu,
    Metal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum ContentArg {
    Source,
    Translation,
    Bilingual,
}

/// 退出码（TDD-09 §3）：0 成功；1 写出失败；2 用法 / Vault 无法确定；3 笔记读不出；
/// 4 没有内容（无可导出内容 / 搜索无命中）；5 库没有索引或索引读不出；6 应用在跑，拒绝更新索引。
const EXIT_USAGE: u8 = 2;
const EXIT_NOTE: u8 = 3;
const EXIT_EMPTY: u8 = 4;
const EXIT_INDEX: u8 = 5;
const EXIT_APP_RUNNING: u8 = 6;

/// CLI 参数 → 导出器选项（纯映射，可单测）。
#[allow(clippy::too_many_arguments)]
fn export_options(
    content: ContentArg,
    format: ExportFormatArg,
    offset_ms: i64,
    bom: bool,
    line_len: Option<usize>,
    lines: Option<u8>,
    speaker_prefix: bool,
    han: HanArg,
) -> luna_export::ExportOptions {
    luna_export::ExportOptions {
        bilingual: match content {
            ContentArg::Source => luna_export::Bilingual::SourceOnly,
            ContentArg::Translation => luna_export::Bilingual::TargetOnly,
            ContentArg::Bilingual => luna_export::Bilingual::Both,
        },
        include_timestamps: format == ExportFormatArg::TxtTimestamped,
        speaker_prefix,
        time_offset_ms: offset_ms,
        utf8_bom: bom,
        max_line_len: line_len,
        max_lines: lines,
        han_variant: match han {
            HanArg::Keep => luna_export::HanVariant::Keep,
            HanArg::Simplified => luna_export::HanVariant::Simplified,
            HanArg::Tw => luna_export::HanVariant::TraditionalTaiwan,
            HanArg::Hk => luna_export::HanVariant::TraditionalHongKong,
        },
    }
}

fn export_format(format: ExportFormatArg) -> luna_export::ExportFormat {
    match format {
        ExportFormatArg::Srt => luna_export::ExportFormat::Srt,
        ExportFormatArg::Vtt => luna_export::ExportFormat::Vtt,
        ExportFormatArg::Ass => luna_export::ExportFormat::Ass,
        ExportFormatArg::Lrc => luna_export::ExportFormat::Lrc,
        ExportFormatArg::Bcc => luna_export::ExportFormat::Bcc,
        ExportFormatArg::Txt | ExportFormatArg::TxtTimestamped | ExportFormatArg::Md => {
            luna_export::ExportFormat::Txt
        }
    }
}

/// 应用的配置目录：与 Tauri `app_config_dir()` 同口径（`<config_dir>/<identifier>`）。设备配置、
/// 库注册表与「应用在跑」的锁都在这里。
fn app_config_dir() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join(luna_config::identity::APP_IDENTIFIER))
}

/// Vault 根：`--vault` → `LUNA_VAULT_ROOT` → GUI 设备配置 `storage.vault_root`。
fn resolve_vault_root(flag: Option<PathBuf>) -> Option<PathBuf> {
    if let Some(root) = flag {
        return Some(root);
    }
    if let Some(env) = std::env::var_os("LUNA_VAULT_ROOT").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(env));
    }
    let config = app_config_dir()?.join(luna_config::DEVICE_CONFIG_FILENAME);
    let (settings, _) = luna_config::Settings::load_path(&config);
    settings
        .get_str("storage.vault_root")
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
}

#[allow(clippy::too_many_arguments)]
fn export(
    note: String,
    vault: Option<PathBuf>,
    format: ExportFormatArg,
    content: ContentArg,
    offset_ms: i64,
    bom: bool,
    line_len: Option<usize>,
    lines: Option<u8>,
    no_speaker: bool,
    han: HanArg,
    output: Option<PathBuf>,
) -> ExitCode {
    let Some(root) = resolve_vault_root(vault) else {
        eprintln!(
            "luna: 无法确定 Vault——请传 --vault，或设置 LUNA_VAULT_ROOT，或先在 GUI 里选一个库"
        );
        return ExitCode::from(EXIT_USAGE);
    };
    let vault = match luna_vault::Vault::open(&root) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("luna: 打开 Vault 失败（{}）：{e}", root.display());
            return ExitCode::from(EXIT_USAGE);
        }
    };
    let doc = match vault.read(&note) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("luna: 读取笔记失败（{note}）：{e}");
            return ExitCode::from(EXIT_NOTE);
        }
    };
    let exportable = if format == ExportFormatArg::Md {
        luna_export::Exportable::Text(luna_vault::body_of(&doc.content).to_string())
    } else {
        let mut segments = luna_vault::note_segments(vault.root(), &doc.meta);
        if segments.is_empty() {
            eprintln!("luna: 这篇笔记没有转写段，字幕 / 文本导出无内容（--format md 可导正文）");
            return ExitCode::from(EXIT_EMPTY);
        }
        // CLI 没有文案表：改名映射优先，其余原样 id（GUI 走后端 i18n 给「我 / 对方」）。
        // 无说话人时前缀选项自然无效，导出器按段上有没有 speaker 决定。
        luna_vault::resolve_segment_speakers(&mut segments, &doc.meta.speakers, &|_| None);
        luna_export::Exportable::Segments(segments)
    };
    let opts = export_options(
        content,
        format,
        offset_ms,
        bom,
        line_len,
        lines,
        !no_speaker,
        han,
    );
    let rendered = match luna_export::ExportRegistry::with_builtin().export(
        export_format(format),
        &exportable,
        &opts,
    ) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("luna: 导出失败：{e}");
            return ExitCode::from(EXIT_EMPTY);
        }
    };
    match output {
        Some(path) => {
            if let Err(e) = std::fs::write(&path, rendered) {
                eprintln!("luna: 写出失败（{}）：{e}", path.display());
                return ExitCode::FAILURE;
            }
            eprintln!("luna: 已写出 {}", path.display());
        }
        None => {
            let mut stdout = std::io::stdout();
            let _ = stdout.write_all(rendered.as_bytes());
            let _ = stdout.flush();
        }
    }
    ExitCode::SUCCESS
}

#[tokio::main]
async fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Translate {
            text,
            target,
            gguf,
            resources,
        } => translate(text, &target, gguf, resources).await,
        Command::BenchAsr {
            audio,
            model,
            runs,
            backend,
            lang,
            reference,
        } => bench_asr(audio, model, runs, backend, lang, reference).await,
        Command::BenchEmbed {
            model,
            gguf,
            resources,
            corpus,
            eval,
            target_chars,
            hard_chars,
            batch,
            json,
        } => {
            bench_embed(
                BenchEmbedModel {
                    profile_key: model,
                    gguf,
                    resources,
                },
                corpus,
                eval,
                target_chars,
                hard_chars,
                batch,
                json,
            )
            .await
        }
        Command::ModelEval {
            catalog_id,
            gguf,
            entry,
            reference,
            resources,
            out,
        } => {
            model_eval::run(model_eval::Args {
                catalog_id,
                gguf,
                entry,
                reference,
                resources,
                out,
            })
            .await
        }
        Command::Vault { command } => vault_cmd::run_vault(command),
        Command::Note { command } => vault_cmd::run_note(command),
        Command::Export {
            note,
            vault,
            format,
            content,
            offset_ms,
            bom,
            line_len,
            lines,
            no_speaker,
            han,
            output,
        } => export(
            note, vault, format, content, offset_ms, bom, line_len, lines, no_speaker, han, output,
        ),
    }
}

/// 翻译用的对话上下文：一段命令行文本远到不了，小窗也省得内存。
const TRANSLATE_CTX: u32 = 4096;

async fn translate(
    text: String,
    target: &str,
    gguf: PathBuf,
    resources: Option<PathBuf>,
) -> ExitCode {
    let resources = resources.unwrap_or_else(local_router::default_resources);
    let router = match local_router::LocalRouter::start(
        &resources,
        "chat",
        &gguf,
        // 按思考模型对待：关思考对不思考的模板无副作用，反过来正文会是空的（P0-1）。
        luna_engine::llama::ModelRole::Chat { thinking: true },
        TRANSLATE_CTX,
    )
    .await
    {
        Ok(router) => router,
        Err(e) => {
            eprintln!("luna: {e}");
            return ExitCode::from(3);
        }
    };
    let code = 'run: {
        let registry = ProviderRegistry::new();
        registry.register(Arc::new(LlmHttpProvider::new(LlmHttpConfig::local_engine(
            router.source(),
            "chat",
        ))));
        let queue = JobQueue::new(Arc::new(registry), AdmissionCapacity::for_capture_tracks(2));

        let job = Job::new(
            JobKind::Translate {
                text: TextRef::Inline(text),
                source: None,
                target: Lang::from(target),
                instruction: None,
                template: None,
            },
            JobOrigin::Cli,
        );

        let mut handle = queue.submit(job);
        let mut stdout = std::io::stdout();
        let mut streamed = false;
        while let Some(event) = handle.recv().await {
            match event {
                // 流式 token → stdout（即结果）。
                JobEvent::Partial(PartialPayload::Token(token)) => {
                    streamed = true;
                    print!("{token}");
                    let _ = stdout.flush();
                }
                JobEvent::Finished(result) => {
                    // 非流式 Provider（一次性产出）时，补打最终文本。
                    if !streamed {
                        if let ResultPayload::Text(t) = &result.payload {
                            print!("{t}");
                        }
                    }
                    println!();
                    break 'run ExitCode::SUCCESS;
                }
                JobEvent::Failed(err) => {
                    eprintln!("\nluna: 失败 [{}] {err}", err.code());
                    break 'run ExitCode::FAILURE;
                }
                JobEvent::Cancelled => {
                    eprintln!("\nluna: 已取消");
                    break 'run ExitCode::from(130);
                }
                // 其它（Queued/Started/Progress/Usage）走诊断，翻译场景静默。
                _ => {}
            }
        }
        ExitCode::SUCCESS
    };
    router.stop().await;
    code
}

/// `luna bench-asr`：解码 → 加载模型（计时，含 CoreML 首次编译）→ 经 JobQueue 跑 N 次批量转写。
///
/// 报的是**应用里那条路径**的数字（同一个 Provider、同一个解码器、同一条队列），不是
/// 另写一段直接调 whisper.cpp 的测量。RTF = 转写墙钟 / 音频时长，越小越快。
/// 退出码：0 成功；2 音频解不出；3 模型加载失败；1 某一次转写失败。
async fn bench_asr(
    audio: PathBuf,
    model: PathBuf,
    runs: u32,
    backend: Option<BackendArg>,
    lang: Option<String>,
    reference: Option<PathBuf>,
) -> ExitCode {
    use luna_engine::{WhisperBackend, WhisperConfig, WhisperProvider, COREML_COMPILED_IN};
    let reference = match reference {
        Some(path) => match std::fs::read_to_string(&path) {
            Ok(text) => Some(text),
            Err(e) => {
                eprintln!("REFERENCE_UNREADABLE: {} ({e})", path.display());
                return ExitCode::from(2);
            }
        },
        None => None,
    };
    use std::time::Instant;

    let pcm = match luna_audio::decoder::decode_media_to_pcm(&audio, &CancelToken::new()) {
        Ok(pcm) => pcm,
        Err(e) => {
            eprintln!("DECODE_FAILED: {e}");
            return ExitCode::from(2);
        }
    };
    let audio_seconds = pcm.samples.len() as f64 / f64::from(pcm.sample_rate.max(1));
    let backend = match backend {
        Some(BackendArg::Cpu) => WhisperBackend::Cpu,
        Some(BackendArg::Metal) => WhisperBackend::Metal,
        None if cfg!(target_os = "macos") => WhisperBackend::Metal,
        None => WhisperBackend::Cpu,
    };
    let encoder = model
        .file_stem()
        .map(|stem| {
            let mut name = stem.to_os_string();
            name.push("-encoder.mlmodelc");
            model.with_file_name(name)
        })
        .filter(|path| path.is_dir());

    println!("model            {}", model.display());
    println!(
        "audio            {} ({audio_seconds:.2}s @ {}Hz)",
        audio.display(),
        pcm.sample_rate
    );
    println!("backend          {backend:?}");
    println!("coreml_compiled  {COREML_COMPILED_IN}");
    println!(
        "coreml_encoder   {}",
        encoder
            .as_ref()
            .map_or("absent".to_owned(), |p| p.display().to_string())
    );

    let t_load = Instant::now();
    let provider = match WhisperProvider::load(
        WhisperConfig::new(&model)
            .with_backend(backend)
            .with_default_lang(lang.clone()),
    ) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("MODEL_LOAD_FAILED: {e}");
            return ExitCode::from(3);
        }
    };
    println!("load_ms          {}", t_load.elapsed().as_millis());

    let registry = ProviderRegistry::new();
    registry.register(Arc::new(provider));
    let queue = JobQueue::new(Arc::new(registry), AdmissionCapacity::for_capture_tracks(1));

    let mut wall_ms: Vec<u128> = Vec::with_capacity(runs as usize);
    for run in 1..=runs {
        let job = Job::new(
            JobKind::Transcribe {
                audio: AudioRef::Pcm(pcm.clone()),
                lang_hint: lang.as_deref().map(Lang::from),
                initial_prompt: None,
            },
            JobOrigin::Cli,
        );
        let t_run = Instant::now();
        let mut handle = queue.submit(job);
        let mut segments = 0usize;
        let mut hypothesis = String::new();
        loop {
            match handle.recv().await {
                Some(JobEvent::Finished(result)) => {
                    if let ResultPayload::Segments(list) = &result.payload {
                        segments = list.len();
                        hypothesis = list
                            .iter()
                            .map(|s| s.text.trim())
                            .collect::<Vec<_>>()
                            .join(" ");
                    }
                    break;
                }
                Some(JobEvent::Failed(err)) => {
                    eprintln!("TRANSCRIBE_FAILED: {err}");
                    return ExitCode::FAILURE;
                }
                Some(JobEvent::Cancelled) | None => {
                    eprintln!("TRANSCRIBE_CANCELLED");
                    return ExitCode::FAILURE;
                }
                Some(_) => {}
            }
        }
        let ms = t_run.elapsed().as_millis();
        let rtf = ms as f64 / 1000.0 / audio_seconds.max(f64::EPSILON);
        println!("run {run:<3} {ms:>8} ms  rtf {rtf:.3}  segments {segments}");
        wall_ms.push(ms);
        // 准确率只报一次：同一模型对同一音频是确定性的，逐次重复只是噪声。
        if run == 1 {
            if let Some(reference) = reference.as_deref() {
                let score = asr_accuracy::score(reference, &hypothesis);
                println!(
                    "cer              {:.3}  ({} ref chars)",
                    score.cer, score.ref_chars
                );
                println!(
                    "wer              {:.3}  ({} ref words)",
                    score.wer, score.ref_words
                );
                println!("hypothesis       {hypothesis}");
            }
        }
    }
    if !wall_ms.is_empty() {
        let mut sorted = wall_ms.clone();
        sorted.sort_unstable();
        let median = sorted[sorted.len() / 2];
        println!(
            "median           {median} ms  rtf {:.3}",
            median as f64 / 1000.0 / audio_seconds.max(f64::EPSILON)
        );
    }
    ExitCode::SUCCESS
}

/// `luna bench-embed` 的评测文件形状。
#[derive(serde::Deserialize, Default)]
struct EmbedEval {
    #[serde(default)]
    queries: Vec<EvalQuery>,
    #[serde(default)]
    duplicates: Vec<EvalPair>,
}

#[derive(serde::Deserialize)]
struct EvalQuery {
    text: String,
    /// 期望命中的笔记相对路径（相对语料目录）；前 5 名里出现任一即算召回。
    expect: Vec<String>,
}

#[derive(serde::Deserialize)]
struct EvalPair {
    a: String,
    b: String,
    duplicate: bool,
}

fn collect_markdown(
    root: &std::path::Path,
    dir: &std::path::Path,
    out: &mut Vec<(String, String)>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            if path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with('.'))
            {
                continue;
            }
            collect_markdown(root, &path, out);
        } else if path.extension().is_some_and(|e| e == "md") {
            if let (Ok(rel), Ok(text)) = (path.strip_prefix(root), std::fs::read_to_string(&path)) {
                out.push((rel.to_string_lossy().replace('\\', "/"), text));
            }
        }
    }
}

/// 嵌入一批文本（经 JobQueue → EmbedHttpProvider，与应用同一条路）。
async fn embed_batch(
    queue: &JobQueue,
    profile: luna_vault::EmbeddingProfile,
    purpose: luna_core::EmbeddingPurpose,
    texts: Vec<String>,
) -> Result<Vec<Vec<f32>>, String> {
    let expected = texts.len();
    let texts = luna_vault::profile_texts(profile, purpose, texts);
    let job = Job::new(JobKind::Embed { purpose, texts }, JobOrigin::Cli);
    let mut handle = queue.submit(job);
    while let Some(event) = handle.recv().await {
        match event {
            JobEvent::Finished(result) => {
                return match result.payload {
                    ResultPayload::Embeddings(vectors) if vectors.len() == expected => {
                        luna_vault::normalize_embedding_vectors(vectors, profile)
                            .map_err(|v| format!("{}: {}", v.code(), v.detail()))
                    }
                    ResultPayload::Embeddings(vectors) => Err(format!(
                        "EMBED_SHAPE: count expected={expected} got={}",
                        vectors.len()
                    )),
                    _ => Err("EMBED_SHAPE: payload is not embeddings".to_owned()),
                };
            }
            JobEvent::Failed(err) => return Err(err.to_string()),
            JobEvent::Cancelled => return Err("CANCELLED".to_owned()),
            _ => {}
        }
    }
    Err("JOB_STREAM_ENDED".to_owned())
}

/// `luna bench-embed` 测哪个模型：profile 键 + 它的 GGUF。
struct BenchEmbedModel {
    profile_key: String,
    gguf: PathBuf,
    resources: Option<PathBuf>,
}

/// profile 键 → 目录里的嵌入 GGUF 条目（取它的角色与上下文；映射与应用同一处）。
fn embed_catalog_entry(profile_key: &str) -> Option<&'static luna_model_catalog::ModelEntry> {
    luna_model_catalog::bundled().iter().find(|entry| {
        entry.gguf.is_some() && luna_vault::engine_entry_profile_key(&entry.id) == Some(profile_key)
    })
}

/// `luna bench-embed`：语料 → 分块 → 批量嵌入 → 向量矩阵，报六项基准；评测文件可选。
/// 退出码：0 成功；2 语料 / 评测文件读不出；3 模型没有 profile 或引擎起不来；1 嵌入失败。
#[allow(clippy::too_many_lines)]
async fn bench_embed(
    target: BenchEmbedModel,
    corpus: PathBuf,
    eval: Option<PathBuf>,
    target_chars: Option<usize>,
    hard_chars: Option<usize>,
    batch: usize,
    json: bool,
) -> ExitCode {
    use luna_core::EmbeddingPurpose;
    use luna_provider::{EmbedHttpConfig, EmbedHttpProvider};
    use luna_vault::{VectorMatrix, VectorRow};
    use std::time::Instant;

    let model = target.profile_key;
    let Some(profile) = luna_vault::embedding_profile(&model) else {
        eprintln!("EMBED_PROFILE_MISSING: {model}（只认目录里登记了 profile 的模型）");
        return ExitCode::from(3);
    };
    let policy = match (profile.chunk_policy, target_chars) {
        (_, Some(target)) => luna_vault::EmbeddingChunkPolicy {
            id: "bench-override",
            target_chars: target,
            hard_chars: hard_chars.unwrap_or(target + target / 2),
            min_chars: luna_vault::CHUNK_MIN_CHARS,
        },
        (Some(policy), None) => policy,
        (None, None) => {
            eprintln!(
                "CHUNK_POLICY_MISSING: {model} 还没有通过分块基准，请用 --target-chars 给一个候选值来测"
            );
            return ExitCode::from(3);
        }
    };
    let mut notes = Vec::new();
    collect_markdown(&corpus, &corpus, &mut notes);
    if notes.is_empty() {
        eprintln!("CORPUS_EMPTY: {} 下没有 .md", corpus.display());
        return ExitCode::from(2);
    }
    let eval: EmbedEval = match eval {
        Some(path) => match std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|text| serde_json::from_str(&text).map_err(|e| e.to_string()))
        {
            Ok(eval) => eval,
            Err(e) => {
                eprintln!("EVAL_UNREADABLE: {e}");
                return ExitCode::from(2);
            }
        },
        None => EmbedEval::default(),
    };

    let Some((entry, gguf)) = embed_catalog_entry(&model).and_then(|e| Some((e, e.gguf.as_ref()?)))
    else {
        eprintln!("EMBED_ENTRY_MISSING: {model} 在目录里没有嵌入 GGUF 条目");
        return ExitCode::from(3);
    };
    let resources = target
        .resources
        .unwrap_or_else(local_router::default_resources);
    let router = match local_router::LocalRouter::start(
        &resources,
        &entry.id,
        &target.gguf,
        gguf.role.into(),
        gguf.max_context,
    )
    .await
    {
        Ok(router) => router,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(3);
        }
    };
    // 起了路由器之后的每一条出口都要经过 `stop`（`Drop` 只是 panic 时的兜底：被外部
    // 杀掉的路由器会被监管器当成意外退出而重启）。
    let code = 'run: {
        let registry = ProviderRegistry::new();
        registry.register(Arc::new(EmbedHttpProvider::new(
            EmbedHttpConfig::local_engine(router.source(), entry.id.clone()),
        )));
        let queue = JobQueue::new(Arc::new(registry), AdmissionCapacity::for_capture_tracks(1));

        // ── 建库 ──
        let mut rows: Vec<(VectorRow, String)> = Vec::new();
        for (rel, text) in &notes {
            for chunk in luna_vault::chunk_note(text, policy) {
                rows.push((
                    VectorRow {
                        path: rel.clone(),
                        chunk_seq: chunk.seq,
                        line_start: chunk.line_start,
                    },
                    chunk.text,
                ));
            }
        }
        let rss_before = router.rss_kb();
        let t_build = Instant::now();
        let mut batch_ms: Vec<u128> = Vec::new();
        let mut entries: Vec<(VectorRow, Vec<f32>)> = Vec::with_capacity(rows.len());
        for group in rows.chunks(batch.max(1)) {
            let texts: Vec<String> = group.iter().map(|(_, text)| text.clone()).collect();
            let t = Instant::now();
            let vectors =
                match embed_batch(&queue, profile, EmbeddingPurpose::Document, texts).await {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("EMBED_FAILED: {e}");
                        break 'run ExitCode::FAILURE;
                    }
                };
            batch_ms.push(t.elapsed().as_millis());
            for ((row, _), vector) in group.iter().zip(vectors) {
                entries.push((row.clone(), vector));
            }
        }
        let build_ms = t_build.elapsed().as_millis();
        let rss_after = router.rss_kb();
        let vectors_by_row: std::collections::HashMap<(String, u32), Vec<f32>> = entries
            .iter()
            .map(|(row, v)| ((row.path.clone(), row.chunk_seq), v.clone()))
            .collect();
        let matrix = VectorMatrix::build(entries);
        let mut sorted = batch_ms.clone();
        sorted.sort_unstable();
        let p50 = sorted.get(sorted.len() / 2).copied().unwrap_or(0);
        let p95 = sorted
            .get((sorted.len() * 95 / 100).min(sorted.len().saturating_sub(1)))
            .copied()
            .unwrap_or(0);

        // ── 召回（recall@5 / MRR）──
        let mut recall_hits = 0usize;
        let mut mrr = 0.0f64;
        for query in &eval.queries {
            let Ok(mut vectors) = embed_batch(
                &queue,
                profile,
                EmbeddingPurpose::Query,
                vec![query.text.clone()],
            )
            .await
            else {
                continue;
            };
            let Some(vector) = vectors.pop() else {
                continue;
            };
            let hits = matrix.search(&vector, 5);
            let mut seen: Vec<&str> = Vec::new();
            for hit in &hits {
                if !seen.contains(&hit.path.as_str()) {
                    seen.push(hit.path.as_str());
                }
            }
            if let Some(rank) = seen
                .iter()
                .position(|path| query.expect.iter().any(|e| e == path))
            {
                recall_hits += 1;
                mrr += 1.0 / (rank as f64 + 1.0);
            }
        }
        let recall_at_5 =
            (!eval.queries.is_empty()).then(|| recall_hits as f64 / eval.queries.len() as f64);
        let mrr = (!eval.queries.is_empty()).then(|| mrr / eval.queries.len() as f64);

        // ── 近重复准确率（两篇之间最相似分片的余弦 ≥ 阈值 = 判重）──
        let threshold = profile
            .near_duplicate_threshold
            .unwrap_or(luna_vault::NEAR_DUPLICATE_THRESHOLD);
        let mut dup_correct = 0usize;
        for pair in &eval.duplicates {
            let best = vectors_by_row
                .iter()
                .filter(|((path, _), _)| path == &pair.a)
                .flat_map(|(_, va)| {
                    vectors_by_row
                        .iter()
                        .filter(|((path, _), _)| path == &pair.b)
                        .map(move |(_, vb)| luna_vault::dot(va, vb))
                })
                .fold(f32::MIN, f32::max);
            if (best >= threshold) == pair.duplicate {
                dup_correct += 1;
            }
        }
        let dup_accuracy = (!eval.duplicates.is_empty())
            .then(|| dup_correct as f64 / eval.duplicates.len() as f64);

        let rss_delta_mb = match (rss_before, rss_after) {
            (Some(a), Some(b)) => Some(b.saturating_sub(a) as f64 / 1024.0),
            _ => None,
        };
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "model": model,
                    "chunk_policy": policy.id,
                    "target_chars": policy.target_chars,
                    "notes": notes.len(),
                    "chunks": matrix.len(),
                    "dimension": matrix.dim(),
                    "build_ms": build_ms,
                    "batch_p50_ms": p50,
                    "batch_p95_ms": p95,
                    "engine_rss_delta_mb": rss_delta_mb,
                    "index_bytes": matrix.bytes(),
                    "recall_at_5": recall_at_5,
                    "mrr": mrr,
                    "near_duplicate_accuracy": dup_accuracy,
                    "near_duplicate_threshold": threshold,
                })
            );
            break 'run ExitCode::SUCCESS;
        }
        println!("model              {model}");
        println!(
            "chunk_policy       {} (target {} / hard {})",
            policy.id, policy.target_chars, policy.hard_chars
        );
        println!("notes / chunks     {} / {}", notes.len(), matrix.len());
        println!("dimension          {}", matrix.dim());
        println!("build_ms           {build_ms}");
        println!("batch_ms p50/p95   {p50} / {p95}  (batch={batch})");
        println!(
            "engine_rss_delta   {}",
            rss_delta_mb.map_or("n/a".to_owned(), |mb| format!("{mb:.0} MB"))
        );
        println!("index_bytes        {}", matrix.bytes());
        println!(
            "recall@5           {}",
            recall_at_5.map_or("n/a（未给评测文件）".to_owned(), |r| format!(
                "{r:.3}"
            ))
        );
        println!(
            "mrr                {}",
            mrr.map_or("n/a".to_owned(), |m| format!("{m:.3}"))
        );
        println!(
            "near_dup_accuracy  {} (threshold {threshold})",
            dup_accuracy.map_or("n/a".to_owned(), |a| format!("{a:.3}"))
        );
        ExitCode::SUCCESS
    };
    router.stop().await;
    code
}

/// 转写准确率（TDD-13 §2 转写侧半边）：CER 按字符、WER 按空白分词，编辑距离 / 参考长度。
///
/// 归一化刻意保守：只做小写、去标点与（CER 下）去空白——不做数字归一、不做简繁转换，
/// 那些会把「模型错了」与「口径不同」混在一起。
mod asr_accuracy {
    pub struct Score {
        pub cer: f64,
        pub wer: f64,
        pub ref_chars: usize,
        pub ref_words: usize,
    }

    fn normalize_chars(text: &str) -> Vec<char> {
        text.chars()
            .filter(|c| !c.is_whitespace() && !c.is_ascii_punctuation() && !is_cjk_punct(*c))
            .flat_map(char::to_lowercase)
            .collect()
    }

    fn normalize_words(text: &str) -> Vec<String> {
        text.split_whitespace()
            .map(|w| {
                w.chars()
                    .filter(|c| !c.is_ascii_punctuation() && !is_cjk_punct(*c))
                    .flat_map(char::to_lowercase)
                    .collect::<String>()
            })
            .filter(|w| !w.is_empty())
            .collect()
    }

    fn is_cjk_punct(c: char) -> bool {
        matches!(
            c,
            '，' | '。'
                | '、'
                | '；'
                | '：'
                | '？'
                | '！'
                | '「'
                | '」'
                | '『'
                | '』'
                | '（'
                | '）'
                | '《'
                | '》'
                | '“'
                | '”'
                | '‘'
                | '’'
                | '…'
                | '—'
                | '·'
        )
    }

    /// Levenshtein 编辑距离（两行滚动，O(min) 内存）。
    pub fn edit_distance<T: PartialEq>(a: &[T], b: &[T]) -> usize {
        if a.is_empty() {
            return b.len();
        }
        if b.is_empty() {
            return a.len();
        }
        let mut prev: Vec<usize> = (0..=b.len()).collect();
        let mut cur = vec![0usize; b.len() + 1];
        for (i, x) in a.iter().enumerate() {
            cur[0] = i + 1;
            for (j, y) in b.iter().enumerate() {
                let cost = usize::from(x != y);
                cur[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1);
            }
            std::mem::swap(&mut prev, &mut cur);
        }
        prev[b.len()]
    }

    pub fn score(reference: &str, hypothesis: &str) -> Score {
        let ref_chars = normalize_chars(reference);
        let hyp_chars = normalize_chars(hypothesis);
        let ref_words = normalize_words(reference);
        let hyp_words = normalize_words(hypothesis);
        let rate = |d: usize, n: usize| if n == 0 { 0.0 } else { d as f64 / n as f64 };
        Score {
            cer: rate(edit_distance(&ref_chars, &hyp_chars), ref_chars.len()),
            wer: rate(edit_distance(&ref_words, &hyp_words), ref_words.len()),
            ref_chars: ref_chars.len(),
            ref_words: ref_words.len(),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn identical_text_scores_zero() {
            let s = score("今天下午三点开会。", "今天下午三点开会");
            assert_eq!(s.cer, 0.0);
            assert_eq!(s.ref_chars, 8);
        }

        #[test]
        fn cer_counts_substitutions_over_reference_length() {
            let s = score("今天下午三点开会", "今天下午四点开会");
            assert!((s.cer - 0.125).abs() < 1e-9);
        }

        #[test]
        fn wer_ignores_case_and_punctuation() {
            let s = score("The quick, brown fox.", "the quick brown fox");
            assert_eq!(s.wer, 0.0);
            assert_eq!(s.ref_words, 4);
            let s = score("the quick brown fox", "the quick fox");
            assert!((s.wer - 0.25).abs() < 1e-9);
        }

        #[test]
        fn empty_reference_does_not_divide_by_zero() {
            let s = score("", "anything");
            assert_eq!(s.cer, 0.0);
            assert_eq!(s.wer, 0.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_options_map_every_flag() {
        let opts = export_options(
            ContentArg::Bilingual,
            ExportFormatArg::TxtTimestamped,
            -250,
            true,
            Some(20),
            Some(1),
            false,
            HanArg::Tw,
        );
        assert_eq!(opts.bilingual, luna_export::Bilingual::Both);
        assert_eq!(opts.han_variant, luna_export::HanVariant::TraditionalTaiwan);
        assert!(opts.include_timestamps);
        assert!(!opts.speaker_prefix);
        assert_eq!(opts.time_offset_ms, -250);
        assert!(opts.utf8_bom);
        assert_eq!(opts.max_line_len, Some(20));
        assert_eq!(opts.max_lines, Some(1));
        assert_eq!(
            export_format(ExportFormatArg::Md),
            luna_export::ExportFormat::Txt
        );
        assert_eq!(
            export_format(ExportFormatArg::Vtt),
            luna_export::ExportFormat::Vtt
        );
    }

    #[test]
    fn vault_flag_wins_over_environment() {
        std::env::set_var("LUNA_VAULT_ROOT", "/from/env");
        assert_eq!(
            resolve_vault_root(Some(PathBuf::from("/flag"))),
            Some(PathBuf::from("/flag"))
        );
        assert_eq!(resolve_vault_root(None), Some(PathBuf::from("/from/env")));
        std::env::remove_var("LUNA_VAULT_ROOT");
    }
}
