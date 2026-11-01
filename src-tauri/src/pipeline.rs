//! 转写 / 翻译 / 说话人细分的**执行核心**（PRD-04-030 / PRD-04-032 / PRD-05，TDD-02 §3.7、TDD-08 §4）。
//!
//! **四处复用同一条 batch 主干**（DRY，ADR-001 单一 AI 主干）：
//! 1. 停录后台**权威重转写**（[`crate::recording`] 停止即触发）；
//! 2. 手动**二次转写**（回放/转写笔记动作）；
//! 3. **导入音/视频**首次转写（转写视图，PRD-05）；
//! 4. **二次翻译 / 说话人细分**（对已归档段的再处理）。
//!
//! 命名口径：本模块是「执行核心」而非「重转写」——导入首转与二次处理共用同一批构件，
//! 故不以任一调用方命名（任务编排在 [`crate::ipc::processing`]，队列在 [`crate::processing_queue`]）。
//!
//! 流程：`decode_media_to_pcm`（symphonia，音/视频音轨 → 16k mono）→ 经 JobQueue 提交
//! `Transcribe{Pcm}` 定向到**批量档 whisper Provider**（`routing.preferred_provider`，独立模型，
//! 与实时档并存）→ 段偏移到会话绝对时钟 → 可选 `translate_segments_batch` 批量转译 →
//! `Vault::rewrite_recording_transcript` 权威写回（正文哨兵块 + 边车 + 标记一致）。
//!
//! 隐私铁律：进度事件仅计数/错误码/路径，绝不含转写或译文正文。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use luna_audio::{decode_media_to_pcm, decode_media_to_stereo_tracks};
use luna_core::{
    AudioRef, Job, JobKind, JobOrigin, Lang, PcmBuffer, Priority, ProviderId, RoutingHint, Segment,
};
use luna_jobqueue::{
    translate_segments_batch, BatchTranslateOutcome, JobQueue, JobSink, TranslationConfig,
};
use luna_vault::Vault;

use crate::recording::{drain_job_segments, drain_job_segments_with_executor};
use luna_core::cancel::TaskCancel;
use luna_core::glossary::{apply_glossary_to_segments, glossary_initial_prompt, GlossaryEntry};

/// 骑跨切开触发线（TDD-03 §10.3）：**次强**簇的重叠占段长比例达此值才值得按词切开——
/// 低于此为句尾/句首蹭到邻簇的正常抖动，切开只会产生碎段。
const SPLIT_SECONDARY_OVERLAP_RATIO: f64 = 0.30;

/// 骑跨切开的最短段长（ms）：短段（一句话/一个短语）本质单说话人，词级切开是噪声放大。
const SPLIT_MIN_SEGMENT_MS: u64 = 1_000;

/// 按簇重叠时长汇总一个时间区间与全部细分段的交叠（簇号 → 重叠时长）。
fn cluster_overlaps(start: Duration, end: Duration, diar: &[Segment]) -> Vec<(String, Duration)> {
    let mut acc: std::collections::BTreeMap<&str, Duration> = std::collections::BTreeMap::new();
    for d in diar {
        let overlap_start = start.max(d.start);
        let overlap_end = end.min(d.end);
        if overlap_end <= overlap_start {
            continue;
        }
        let cluster = d.speaker.as_deref().unwrap_or("");
        if cluster.is_empty() {
            continue;
        }
        *acc.entry(cluster).or_default() += overlap_end - overlap_start;
    }
    let mut v: Vec<(String, Duration)> = acc
        .into_iter()
        .map(|(c, dur)| (c.to_owned(), dur))
        .collect();
    v.sort_by_key(|(_, dur)| std::cmp::Reverse(*dur));
    v
}

/// 说话人细分段（Diarize Job 产物）归属到转写段（TDD-03 §10.3）：
///
/// - 基线：按**时间重叠最大**原则整段归属；无重叠的段保持原 speaker（粗轨标签兜底）；
/// - **骑跨切开（WhisperX 系标准做法）**：段与 ≥2 个簇重叠、次强簇重叠占比 ≥
///   [`SPLIT_SECONDARY_OVERLAP_RATIO`]、段长 ≥ [`SPLIT_MIN_SEGMENT_MS`] 且 `words` 可用 →
///   按词时间戳（词中点落在哪个簇 = 归哪簇，无簇词随前词）在簇边界切成多段——文本按词
///   分组直拼 trim 重组（词文本保留原始拼写含拉丁前导空格，`aggregate_word_timings` 契约），
///   时间取词界；`words` 不可用/不满足条件 → 回退整段最大重叠归属。
///
/// `label` 把簇号（"1"/"2"/…）映射为最终 speaker 值（如 `remote:1`）。纯函数，headless 单测。
pub(crate) fn assign_speakers(
    transcript: &mut Vec<Segment>,
    diar: &[Segment],
    label: impl Fn(&str) -> String,
) {
    let mut out: Vec<Segment> = Vec::with_capacity(transcript.len());
    for seg in transcript.drain(..) {
        let overlaps = cluster_overlaps(seg.start, seg.end, diar);
        let Some((best_cluster, _)) = overlaps.first() else {
            out.push(seg); // 无重叠：保持原 speaker（轨标签兜底）
            continue;
        };
        let seg_len = seg.end.saturating_sub(seg.start);
        let straddles = overlaps.len() >= 2
            && seg_len >= Duration::from_millis(SPLIT_MIN_SEGMENT_MS)
            && overlaps[1].1.as_secs_f64() >= seg_len.as_secs_f64() * SPLIT_SECONDARY_OVERLAP_RATIO;
        if straddles {
            if let Some(pieces) = split_segment_by_words(&seg, diar, &label) {
                out.extend(pieces);
                continue;
            }
        }
        let mut seg = seg;
        seg.speaker = Some(label(best_cluster));
        out.push(seg);
    }
    *transcript = out;
}

/// 按词时间戳把骑跨段切成多段（`None` = 无 words / 词组不足 2 组，调用方回退整段归属）。
fn split_segment_by_words(
    seg: &Segment,
    diar: &[Segment],
    label: &impl Fn(&str) -> String,
) -> Option<Vec<Segment>> {
    let words = seg.words.as_ref()?;
    if words.is_empty() {
        return None;
    }
    // 逐词归簇：词中点重叠最大者；无簇词随前词（None 时归 pending 首词组）。
    let mut groups: Vec<(String, Vec<&luna_core::WordTiming>)> = Vec::new();
    for w in words {
        let cluster = cluster_overlaps(w.start, w.end, diar)
            .first()
            .map(|(c, _)| c.clone());
        match (cluster, groups.last_mut()) {
            (Some(c), Some((gc, gw))) if *gc == c => gw.push(w),
            (Some(c), _) => groups.push((c, vec![w])),
            (None, Some((_, gw))) => gw.push(w), // 无簇词（静音边缘）随前词组
            (None, None) => groups.push((String::new(), vec![w])),
        }
    }
    // 首组若为无簇占位，并入次组（若有）。
    if groups.first().is_some_and(|(c, _)| c.is_empty()) {
        if groups.len() < 2 {
            return None;
        }
        let (_, head) = groups.remove(0);
        let (_, second) = &mut groups[0];
        let mut merged = head;
        merged.extend(second.iter().copied());
        *second = merged;
    }
    if groups.len() < 2 {
        return None; // 词级归属未形成 ≥2 组：不切（避免与整段归属打架）
    }
    let pieces = groups
        .into_iter()
        .map(|(cluster, gw)| Segment {
            id: luna_core::SegmentId::new(),
            start: gw.first().map_or(seg.start, |w| w.start),
            end: gw.last().map_or(seg.end, |w| w.end),
            text: gw
                .iter()
                .map(|w| w.text.as_str())
                .collect::<String>()
                .trim()
                .to_string(),
            translation: None, // 段界已变，原整段译文不再对位（权威重转写批量转译在归属之后）
            speaker: Some(label(&cluster)),
            confidence: seg.confidence,
            words: Some(gw.into_iter().cloned().collect()),
        })
        .collect();
    Some(pieces)
}

/// 对一路 16k mono PCM 提交 `Diarize` Job 并取回细分段。
/// `lang` = 会话语言（与转写 `lang_hint` 同源）：嵌入模型按语言选档（TDD-03 §10.2）。
/// 无 Provider（模型未装）/失败 → `Err`（调用方按「跳过细分、保留粗轨标签」降级）。
/// 双轨重转写搭车与 mono 手动细分入口（统一处理队列 worker）共用。
pub(crate) async fn diarize_pcm(
    queue: &Arc<JobQueue>,
    pcm: PcmBuffer,
    lang: Option<Lang>,
    cancel: &TaskCancel,
) -> Result<Vec<Segment>, String> {
    cancel.bail_if_cancelled()?;
    let job = Job::new(
        JobKind::Diarize {
            audio: AudioRef::Pcm(pcm),
            lang,
        },
        JobOrigin::SecondPass,
    );
    let handle = queue.submit(job);
    cancel.arm(handle.cancel_token());
    drain_job_segments(handle).await
}

/// 段是否属 remote 轨（粗标签 `remote` 或已细分的 `remote:N`）。
fn is_remote_speaker(speaker: Option<&str>) -> bool {
    speaker.is_some_and(|s| s == "remote" || s.starts_with("remote:"))
}

/// 手动细分标签写入（纯函数，TDD-03 §10.1/§10.3）。按录音**结构**路由：
///
/// - **存在 remote 轨段**（双轨立体声的 R 声道内容 / 单系统源整轨对方）：diar 只作用于
///   remote 侧段——self 段**绝不被误标**（R 声道细分与 L 声道时间天然重叠，不分侧即误标）；
///   单簇也写 `remote:1`（与权威重转写自动细分同口径，手动重跑幂等——服务「调过聚类
///   阈值 / 下载 EN 嵌入档后重细分」）；
/// - **纯现场单麦克风**（无 remote 段）：整轨细分，检出 **≥2 簇才写** `speaker:N`
///   （防自言自语被误标 Speaker 1）。
///
/// 返回 `(检出簇数, 是否已写标签)`，调用方据此决定是否权威写回与如何反馈。
pub(crate) fn apply_manual_diarization(
    transcript: &mut Vec<Segment>,
    diar: &[Segment],
) -> (u32, bool) {
    let clusters: std::collections::BTreeSet<&str> = diar
        .iter()
        .filter_map(|d| d.speaker.as_deref())
        .filter(|s| !s.is_empty())
        .collect();
    let count = clusters.len() as u32;
    if count == 0 {
        return (0, false);
    }
    let has_remote = transcript
        .iter()
        .any(|s| is_remote_speaker(s.speaker.as_deref()));
    if has_remote {
        // 只重标 remote 侧：抽出 remote 段归属（可能被骑跨切开改变段数）后按时间归并。
        let mut remote: Vec<Segment> = Vec::new();
        let mut others: Vec<Segment> = Vec::new();
        for seg in transcript.drain(..) {
            if is_remote_speaker(seg.speaker.as_deref()) {
                remote.push(seg);
            } else {
                others.push(seg);
            }
        }
        assign_speakers(&mut remote, diar, |c| format!("remote:{c}"));
        others.extend(remote);
        others.sort_by_key(|s| s.start);
        *transcript = others;
        (count, true)
    } else {
        if count < 2 {
            return (count, false);
        }
        assign_speakers(transcript, diar, |c| format!("speaker:{c}"));
        (count, true)
    }
}

/// 一个待重转写分片（媒体相对路径 + 会话绝对偏移 + 磁盘绝对路径）。
pub struct RetranscribePart {
    /// Vault 相对媒体路径（`media[].path`）。
    pub media_rel: String,
    /// 该分片在会话时间轴的绝对起点（ms；段时间加此偏移）。
    pub part_start_ms: u64,
    /// 磁盘绝对路径（解码输入）。
    pub audio_abs: PathBuf,
    /// Lunarca 双轨立体声标记（`MediaRef.tracks == "mic_system"`，PRD-04-052）：
    /// 权威重转写按声道分别转写并归属 `speaker=self/remote`（L=我，R=对方）。
    pub dual_track: bool,
}

/// 解码 + 转写单个媒体文件 → 段（**分片本地 0-based 时钟**，调用方再加偏移）。
///
/// 解码在阻塞线程池（symphonia CPU 密集，绝不占用 async worker）；转写经 JobQueue 定向到批量档。
#[allow(clippy::too_many_arguments)]
async fn transcribe_file(
    queue: &Arc<JobQueue>,
    provider_id: &ProviderId,
    audio_abs: PathBuf,
    lang: Option<Lang>,
    dual_track: bool,
    glossary: &[GlossaryEntry],
    cancel: &TaskCancel,
    on_queue: QueueObserver<'_>,
) -> Result<(Vec<Segment>, Option<luna_core::ExecutorInfo>), String> {
    if dual_track {
        // 双轨立体声（声道即轨）：分声道解码 + 各自权威转写 + 轨级标签（PRD-04-053），
        // 按起点排序合并为单一时间线——实时期的 self/remote 标签经重转写**不再丢失**。
        let (left, right) = {
            let flag = cancel.flag();
            tokio::task::spawn_blocking(move || decode_media_to_stereo_tracks(&audio_abs, &flag))
        }
        .await
        .map_err(|e| format!("DECODE_TASK_PANIC: {e}"))?
        .map_err(|e| format!("DECODE_FAILED: {e}"))?;
        // 双声道各转一次；执行者取第一次的（同一 Provider，两次必然一致）。
        let (mut segs, executor) = transcribe_pcm(
            queue,
            provider_id,
            left,
            lang.clone(),
            glossary,
            cancel,
            on_queue,
        )
        .await?;
        for s in &mut segs {
            s.speaker = Some("self".into());
        }
        // L1 说话人细分（PRD-04-054 / TDD-03 §10）：对 R（对方）声道跑本地 diarization，
        // 把粗粒度 remote 细分为 remote:1/2/…；无模型/失败 → 保留粗轨标签（诚实降级）。
        let diar_input = right.clone();
        let (mut remote, _) = transcribe_pcm(
            queue,
            provider_id,
            right,
            lang.clone(),
            glossary,
            cancel,
            on_queue,
        )
        .await?;
        for s in &mut remote {
            s.speaker = Some("remote".into());
        }
        // 嵌入按会话语言选档（TDD-03 §10.2）：与转写 lang_hint 同源。
        match diarize_pcm(queue, diar_input, lang, cancel).await {
            Ok(diar) if !diar.is_empty() => {
                assign_speakers(&mut remote, &diar, |cluster| format!("remote:{cluster}"));
            }
            Ok(_) => {}
            Err(e) => {
                // 无 Provider（模型未装）属常态：仅结构化日志，不打扰用户。
                eprintln!("[F4] 说话人细分跳过（{e}）——保留轨级标签。");
            }
        }
        segs.extend(remote);
        segs.sort_by_key(|s| s.start);
        // 外放回声去重（TDD-02 §3.1-I 层②）：mic 轨拾到扬声器里对方声音的段
        // （与 system 轨时间重叠 + 文本相似）判为回声移除，根治「对方话被标『我』+
        // 重复转写」。纯函数与实时 finalize 共用（DRY）；日志仅计数（脱敏）。
        let echo_dropped = luna_core::drop_cross_track_echo(&mut segs);
        if echo_dropped > 0 {
            eprintln!("[F4] 跨轨回声去重：移除 {echo_dropped} 个 mic 轨回声段（重转写）。");
        }
        return Ok((segs, executor));
    }
    let pcm = {
        let flag = cancel.flag();
        tokio::task::spawn_blocking(move || decode_media_to_pcm(&audio_abs, &flag))
    }
    .await
    .map_err(|e| format!("DECODE_TASK_PANIC: {e}"))?
    .map_err(|e| format!("DECODE_FAILED: {e}"))?;
    transcribe_pcm(queue, provider_id, pcm, lang, glossary, cancel, on_queue).await
}

/// 「服务端队列排位变了」的观察者（ADR-049 ⑧）。`None` = 已经不在服务端队列里。
///
/// 做成一个**回调**而不是把排位并进返回值：它在任务跑完之前会反复变化，而返回值
/// 只在跑完时才有——那时排位早就没有意义了。形状与 `on_progress` 同构，理由也同构。
pub(crate) type QueueObserver<'a> = &'a (dyn Fn(Option<luna_core::RemoteQueueStatus>) + Sync);

/// 提交 `Transcribe{Pcm}` 到批量档 Provider 并 drain 出段。
pub async fn transcribe_pcm(
    queue: &Arc<JobQueue>,
    provider_id: &ProviderId,
    pcm: PcmBuffer,
    lang: Option<Lang>,
    glossary: &[GlossaryEntry],
    cancel: &TaskCancel,
    on_queue: QueueObserver<'_>,
) -> Result<(Vec<Segment>, Option<luna_core::ExecutorInfo>), String> {
    cancel.bail_if_cancelled()?;
    let mut job = Job::new(
        JobKind::Transcribe {
            audio: AudioRef::Pcm(pcm),
            lang_hint: lang,
            // 术语表注入（PRD-05-003 / PRD-01-017）：让 whisper 倾向识别出正确的专有名词。
            // 与实时录音同一合成函数（`luna_core::glossary`），离线路径此前恒为 None——
            // 用户维护的词典对导入与二次转写完全不生效。
            initial_prompt: glossary_initial_prompt(glossary),
        },
        JobOrigin::SecondPass,
    );
    job.priority = Priority::Normal;
    job.routing = RoutingHint {
        preferred_provider: Some(provider_id.clone()),
        ..RoutingHint::default()
    };
    let handle = queue.submit(job);
    cancel.arm(handle.cancel_token());
    let (segments, executor) = drain_job_segments_with_executor(handle, on_queue).await?;
    // ASR 后 canonical 化兜底：模型没听对也能纠回规范形（长词优先，保留时间戳）。
    Ok((apply_glossary_to_segments(segments, glossary), executor))
}

/// 段序列**权威写回**（正文哨兵块 + 边车 + 标记一致）——阻塞线程池持 Vault 锁改写。
/// 二次转写、二次翻译、整篇编辑、手动细分共用此单一写回点（DRY）。`model=None` 保留原模型标记。
pub(crate) async fn write_recording_segments(
    vault_arc: &Arc<Mutex<Option<Vault>>>,
    note_rel: &str,
    segments: Vec<Segment>,
    model: Option<String>,
    authority: luna_vault::TranscriptAuthority,
) -> Result<(), String> {
    let vault_arc = Arc::clone(vault_arc);
    let note_rel = note_rel.to_string();
    tokio::task::spawn_blocking(move || {
        let guard = vault_arc.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| "Vault 未打开".to_string())?;
        vault
            .rewrite_recording_transcript(&note_rel, &segments, model, authority)
            .map_err(|e| format!("WRITE_FAILED: {e}"))
    })
    .await
    .map_err(|e| format!("WRITE_TASK_PANIC: {e}"))?
}

/// 可选批量转译 + 权威写回（正文哨兵块 + 边车 + 标记一致）。纯逻辑无事件——事件由入口包装。
///
/// **转译整批停时照常写回，并把结局交回调用方**（ADR-083 ⑧）：转写正文是这条路的主产物
/// （二次转写 / 导入首转），它已经好了，不写回等于把一段跑完的转写丢掉；已译出的句子同样
/// 保留（它们是真的译出来了）。而「译文一句都没有」这件事必须说出来，不能吞成一次「成功」。
///
/// **本函数刻意不把「整批停」自己变成 `Err`**：`Err` 在导入那条路上会触发
/// [`crate::ipc::processing`] 的回滚——**永久删除**刚建的笔记。用一次翻译失败去销毁一段
/// 已经跑完的转写，比没有信号贵得多。`Err` 在这里只留给真正的写回失败；档位那一层的结局
/// 由 [`BatchTranslateOutcome`] 表达，`#[must_use]` 保证调用方必须表态。
pub(crate) async fn translate_and_write(
    queue: &Arc<JobQueue>,
    vault_arc: &Arc<Mutex<Option<Vault>>>,
    note_rel: &str,
    mut segments: Vec<Segment>,
    translate_config: Option<TranslationConfig>,
    model: Option<String>,
    cancel: &TaskCancel,
) -> Result<BatchTranslateOutcome, String> {
    // 本函数只服务「导入首转 / 停录后重转写」：批量转写已完整跑完，写回即权威转写
    // （PRD-16-047）。翻译失败不影响转写本身的权威性。
    let mut outcome = BatchTranslateOutcome::Completed;
    if let Some(cfg) = translate_config.filter(|_| !cancel.is_cancelled()) {
        let sink: Arc<dyn JobSink + Send + Sync> = Arc::clone(queue) as Arc<_>;
        // 二次转写路径进度由分片计数承载，逐句转译进度不投影（本路径无译文进度事件）。
        outcome = translate_segments_batch(&sink, &cfg, &mut segments, cancel, |_, _| {}).await;
    }
    write_recording_segments(
        vault_arc,
        note_rel,
        segments,
        model,
        luna_vault::TranscriptAuthority::Authoritative,
    )
    .await?;
    Ok(outcome)
}

/// 把一次批量转译的结局折成任务终态：`Stopped` → 带稳定码的失败，其余 → 成功。
///
/// **调用点必须先把「转写已经落定」的后续都做完再折**（刷新关系标签 / 语义索引 /
/// 通知前端刷新…）：那些跟着的是**转写**的落定，与译文出没出来无关，用一个 `?` 顺手跳过
/// 它们，表现是笔记正文更新了而界面不知道、索引也不跟。
pub(crate) fn translate_outcome_into_result(outcome: &BatchTranslateOutcome) -> Result<(), String> {
    match outcome {
        BatchTranslateOutcome::Stopped { code } => Err(format!(
            "{code}: 转写已写回，但本批转译因该错误整批停止（已译出的部分已保留）。"
        )),
        BatchTranslateOutcome::Completed | BatchTranslateOutcome::Cancelled => Ok(()),
    }
}

/// 逐分片解码+转写并偏移累积到会话绝对时钟（`on_progress` 每分片回调一次 `(done, total)`）。
/// 统一处理队列 worker 的二次转写任务据此投影分片进度（[`crate::processing_queue`]）。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn transcribe_all_parts(
    queue: &Arc<JobQueue>,
    provider_id: &ProviderId,
    parts: Vec<RetranscribePart>,
    lang: Option<Lang>,
    glossary: &[GlossaryEntry],
    cancel: &TaskCancel,
    mut on_progress: impl FnMut(u32, u32),
    on_queue: QueueObserver<'_>,
) -> Result<(Vec<Segment>, Option<luna_core::ExecutorInfo>), String> {
    let total = parts.len() as u32;
    let mut all_segments: Vec<Segment> = Vec::new();
    // 多分片共用同一个 Provider，执行者取首片的即可。
    let mut executor: Option<luna_core::ExecutorInfo> = None;
    for (done, part) in (1u32..).zip(parts) {
        cancel.bail_if_cancelled()?;
        on_progress(done, total);
        let (mut segs, part_executor) = transcribe_file(
            queue,
            provider_id,
            part.audio_abs,
            lang.clone(),
            part.dual_track,
            glossary,
            cancel,
            on_queue,
        )
        .await?;
        executor = executor.or(part_executor);
        let offset = Duration::from_millis(part.part_start_ms);
        for s in &mut segs {
            s.start += offset;
            s.end += offset;
        }
        all_segments.extend(segs);
    }
    Ok((all_segments, executor))
}

// 四类录音处理（导入 / 二次转写 / 二次翻译 / 说话人细分）不再各自 spawn + Channel；改由
// 统一处理队列 worker（[`crate::processing_queue`] + `ipc::processing` 的 `process_*`）顺序编排，
// 复用本模块的 [`transcribe_all_parts`] / [`translate_and_write`] / [`write_recording_segments`]
// / [`diarize_pcm`] / [`apply_manual_diarization`] 等构件（DRY，PRD-04-032/055）。

#[cfg(test)]
mod tests {
    use super::*;
    use luna_core::SegmentId;

    fn seg(start_ms: u64, end_ms: u64, speaker: Option<&str>) -> Segment {
        Segment {
            id: SegmentId::new(),
            start: Duration::from_millis(start_ms),
            end: Duration::from_millis(end_ms),
            text: String::new(),
            translation: None,
            speaker: speaker.map(str::to_string),
            confidence: None,
            words: None,
        }
    }

    // ── 说话人细分归属（TDD-03 §10.3：重叠时长最大原则）────────────────────────

    #[test]
    fn assign_speakers_picks_max_overlap_cluster() {
        let mut transcript = vec![
            seg(0, 2_000, Some("remote")),
            seg(2_500, 5_000, Some("remote")),
        ];
        let diar = vec![seg(0, 2_200, Some("1")), seg(2_200, 6_000, Some("2"))];
        assign_speakers(&mut transcript, &diar, |c| format!("remote:{c}"));
        assert_eq!(transcript[0].speaker.as_deref(), Some("remote:1"));
        assert_eq!(transcript[1].speaker.as_deref(), Some("remote:2"));
    }

    #[test]
    fn assign_speakers_keeps_track_label_when_no_overlap() {
        // 细分只覆盖 0-1s；2-3s 的转写段无重叠 → 保留粗粒度轨标签（诚实降级）。
        let mut transcript = vec![seg(2_000, 3_000, Some("remote"))];
        let diar = vec![seg(0, 1_000, Some("1"))];
        assign_speakers(&mut transcript, &diar, |c| format!("remote:{c}"));
        assert_eq!(transcript[0].speaker.as_deref(), Some("remote"));
    }

    #[test]
    fn assign_speakers_straddling_segment_takes_dominant_side() {
        // 骑跨两簇但无 words（实时段/边车段）→ 回退整段最大重叠归属（簇2）。
        let mut transcript = vec![seg(0, 3_000, Some("remote"))];
        let diar = vec![seg(0, 1_000, Some("1")), seg(1_000, 3_000, Some("2"))];
        assign_speakers(&mut transcript, &diar, |c| format!("remote:{c}"));
        assert_eq!(transcript.len(), 1, "无 words 不切开");
        assert_eq!(transcript[0].speaker.as_deref(), Some("remote:2"));
    }

    /// 造带词级时间戳的段（词文本带拉丁前导空格，aggregate_word_timings 契约）。
    fn seg_with_words(start_ms: u64, end_ms: u64, words: &[(&str, u64, u64)]) -> Segment {
        let mut s = seg(start_ms, end_ms, Some("remote"));
        s.text = words
            .iter()
            .map(|(t, _, _)| *t)
            .collect::<String>()
            .trim()
            .to_string();
        s.words = Some(
            words
                .iter()
                .map(|(t, a, b)| luna_core::WordTiming {
                    text: (*t).to_string(),
                    start: Duration::from_millis(*a),
                    end: Duration::from_millis(*b),
                })
                .collect(),
        );
        s
    }

    #[test]
    fn assign_speakers_splits_straddling_segment_at_cluster_boundary() {
        // 骑跨切开（TDD-03 §10.3）：0-3s 段，簇1 占 0-1.2s、簇2 占 1.2-3s（次强 40%≥30%），
        // words 可用 → 按词界切成两段，文本按词重组、时间取词界、各归各簇。
        let mut transcript = vec![seg_with_words(
            0,
            3_000,
            &[
                (" hello", 0, 500),
                (" there", 500, 1_100),
                (" yes", 1_300, 2_000),
                (" indeed", 2_000, 2_900),
            ],
        )];
        let diar = vec![seg(0, 1_200, Some("1")), seg(1_200, 3_000, Some("2"))];
        assign_speakers(&mut transcript, &diar, |c| format!("remote:{c}"));
        assert_eq!(transcript.len(), 2, "骑跨段应切成两段");
        assert_eq!(transcript[0].speaker.as_deref(), Some("remote:1"));
        assert_eq!(transcript[0].text, "hello there");
        assert_eq!(transcript[0].start, Duration::ZERO);
        assert_eq!(transcript[0].end, Duration::from_millis(1_100));
        assert_eq!(transcript[1].speaker.as_deref(), Some("remote:2"));
        assert_eq!(transcript[1].text, "yes indeed");
        assert_eq!(transcript[1].start, Duration::from_millis(1_300));
        assert_eq!(transcript[1].end, Duration::from_millis(2_900));
    }

    #[test]
    fn assign_speakers_short_segment_never_splits() {
        // 短段（<1s）本质单说话人：即便次强重叠超比例、words 在场也不切。
        let mut transcript = vec![seg_with_words(0, 800, &[(" a", 0, 400), (" b", 400, 800)])];
        let diar = vec![seg(0, 400, Some("1")), seg(400, 800, Some("2"))];
        assign_speakers(&mut transcript, &diar, |c| format!("remote:{c}"));
        assert_eq!(transcript.len(), 1, "短段不切");
    }

    #[test]
    fn assign_speakers_minor_secondary_overlap_does_not_split() {
        // 次强重叠 <30%（句尾蹭到邻簇的正常抖动）：不切，整段归主簇。
        let mut transcript = vec![seg_with_words(
            0,
            4_000,
            &[(" x", 0, 3_500), (" y", 3_500, 4_000)],
        )];
        // 簇2 只占尾部 0.5s（12.5% < 30%）。
        let diar = vec![seg(0, 3_500, Some("1")), seg(3_500, 4_000, Some("2"))];
        assign_speakers(&mut transcript, &diar, |c| format!("remote:{c}"));
        assert_eq!(transcript.len(), 1);
        assert_eq!(transcript[0].speaker.as_deref(), Some("remote:1"));
    }

    // ── 手动细分（TDD-03 §10.1/§10.3 结构路由）──────────────────────────────────

    #[test]
    fn manual_diarization_mic_single_cluster_writes_nothing() {
        // 现场单麦克风 + 自言自语（1 簇）：不写标签，原字段原样（防误标）。
        let mut transcript = vec![seg(0, 2_000, None)];
        let diar = vec![seg(0, 2_000, Some("1"))];
        assert_eq!(apply_manual_diarization(&mut transcript, &diar), (1, false));
        assert_eq!(transcript[0].speaker, None);
    }

    #[test]
    fn manual_diarization_mic_track_labels_speaker_n() {
        // 现场单麦克风（无轨标签）≥2 簇 → speaker:N。
        let mut transcript = vec![seg(0, 2_000, None), seg(2_000, 4_000, None)];
        let diar = vec![seg(0, 2_000, Some("1")), seg(2_000, 4_000, Some("2"))];
        assert_eq!(apply_manual_diarization(&mut transcript, &diar), (2, true));
        assert_eq!(transcript[0].speaker.as_deref(), Some("speaker:1"));
        assert_eq!(transcript[1].speaker.as_deref(), Some("speaker:2"));
    }

    #[test]
    fn manual_diarization_remote_track_labels_remote_n() {
        // 单系统源（整轨 remote）→ remote:N（对方N）。
        let mut transcript = vec![
            seg(0, 2_000, Some("remote")),
            seg(2_000, 4_000, Some("remote")),
        ];
        let diar = vec![seg(0, 2_000, Some("1")), seg(2_000, 4_000, Some("2"))];
        assert_eq!(apply_manual_diarization(&mut transcript, &diar), (2, true));
        assert_eq!(transcript[0].speaker.as_deref(), Some("remote:1"));
        assert_eq!(transcript[1].speaker.as_deref(), Some("remote:2"));
    }

    #[test]
    fn manual_diarization_dual_track_relabels_remote_side_only() {
        // 双轨（self/remote 交错）：diar 来自 R 声道，与 self 段时间天然重叠——
        // self 段绝不被误标，仅 remote 侧改 remote:N（含已细分 remote:N 的幂等重跑）。
        let mut transcript = vec![
            seg(0, 2_000, Some("self")),
            seg(500, 2_500, Some("remote")),
            seg(2_500, 4_000, Some("remote:9")), // 上一轮细分残留：重跑应按新聚类改写
            seg(3_000, 5_000, Some("self")),
        ];
        let diar = vec![seg(0, 2_600, Some("1")), seg(2_600, 5_000, Some("2"))];
        assert_eq!(apply_manual_diarization(&mut transcript, &diar), (2, true));
        let speakers: Vec<Option<&str>> = transcript.iter().map(|s| s.speaker.as_deref()).collect();
        assert_eq!(
            speakers,
            vec![
                Some("self"),
                Some("remote:1"),
                Some("remote:2"),
                Some("self")
            ]
        );
    }

    #[test]
    fn manual_diarization_splits_straddling_segment_when_words_persisted() {
        // 真机症状「明明识别出第二个说话人却拆不开」的回归锁：手动细分读的是**边车**段，
        // 词级时间戳一旦不落盘（旧行为），此路径的骑跨切开永远退化为整段归属。
        // 边车现随段持久化 words（TDD-03 §10.4），故同一段应被按词界切开成两位说话人。
        let mut straddling = seg_with_words(
            0,
            3_000,
            &[
                (" 我先说", 0, 900),
                (" 两句", 900, 1_100),
                (" 换我", 1_300, 2_000),
                (" 补充一点", 2_000, 2_900),
            ],
        );
        straddling.speaker = Some("remote".into());
        let mut transcript = vec![straddling];
        let diar = vec![seg(0, 1_200, Some("1")), seg(1_200, 3_000, Some("2"))];
        assert_eq!(apply_manual_diarization(&mut transcript, &diar), (2, true));
        assert_eq!(transcript.len(), 2, "骑跨段应被切成两位说话人");
        assert_eq!(transcript[0].speaker.as_deref(), Some("remote:1"));
        assert_eq!(transcript[1].speaker.as_deref(), Some("remote:2"));
    }

    #[test]
    fn manual_diarization_dual_track_single_cluster_still_applies() {
        // 双轨 + 对方仅 1 人：remote → remote:1（与权威重转写自动细分同口径，重跑幂等）。
        let mut transcript = vec![seg(0, 2_000, Some("self")), seg(0, 2_000, Some("remote"))];
        let diar = vec![seg(0, 2_000, Some("1"))];
        assert_eq!(apply_manual_diarization(&mut transcript, &diar), (1, true));
        assert_eq!(transcript[0].speaker.as_deref(), Some("self"));
        assert_eq!(transcript[1].speaker.as_deref(), Some("remote:1"));
    }
}
