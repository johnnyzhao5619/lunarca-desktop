//! 逐主题生成的封闭模型输出协议（ADR-134 / TDD-06 §3.9.4）。
//!
//! 候选片段必须先从已打开的 Vault 来源快照构造；模型只看片段 ID，不提供路径、锚点或
//! Markdown。此处做结构与证据集合校验，语义是否属于范围仍须模型评测及人工验收。

use crate::frontmatter::{body_of, read_luna};
use crate::frontmatter::{NoteStage, NoteType};
use crate::{
    evidence_eligibility, ClaimKind, ContentHash, EvidenceLocator, EvidenceReason, PlanEvidence,
    SourcePolicy, TopicScope, Vault, VaultError, VaultOpenError,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
use uuid::Uuid;

const MAX_EVIDENCE: usize = 64;
const MAX_CLAIMS: usize = 24;
const MAX_REFERENCES_PER_CLAIM: usize = 8;
const MAX_EXCERPT_CHARS: usize = 4_096;
const MAX_CLAIM_CHARS: usize = 4_096;
const MAX_OUTPUT_BYTES: usize = 128 * 1_024;
const MAX_REQUEST_BYTES: usize = 128 * 1_024;
const MAX_SOURCE_BYTES: usize = 1024 * 1024;
const MAX_BATCH_EXCERPT_CHARS: usize = 12_000;

/// 初稿本轮只用正式知识笔记。来源资料要在权威完成与设备级范围授权接线后另行加入。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopicEvidenceBatch {
    pub evidence: Vec<TopicEvidenceInput>,
    pub input_complete: bool,
    pub omitted_sources: u32,
    /// 这一批实际读到的每篇来源：读的是哪一版、读到了第几个片段（按来源在批中的顺序）。
    pub coverage: Vec<SourceCoverage>,
}

/// 一篇来源这次被读到哪里。`next_fragment = Some(k)` 表示这一版只读了前 k 个片段、
/// 下一轮从第 k 个接着读；`None` 表示这一版已读完（单行过长被截的仍算读完，出处标部分输入）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceCoverage {
    pub path: String,
    pub version: String,
    pub next_fragment: Option<u32>,
}

impl SourceCoverage {
    /// 这一版整篇读完。
    #[must_use]
    pub fn whole(path: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            version: version.into(),
            next_fragment: None,
        }
    }
}

/// Rust 从来源快照提取的有界片段。`id` 只在本次生成请求中有效。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TopicEvidenceInput {
    pub id: String,
    pub excerpt: String,
    pub kind: TopicEvidenceKind,
    pub source: PlanEvidence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TopicEvidenceKind {
    FormalNote,
    AuthoritativeTranscript,
    /// 速记（ADR-136 ①）：用户之言、未整理。引用它的声明身份仍按「是否引用转写」推导，
    /// 「只由速记支撑」由预览按来源类型如实标出，不写进声明元数据。
    QuickNote,
}

impl TopicEvidenceKind {
    /// 交给模型的类型：模型协议只区分「用户写下的笔记」与「转写原文」两种——速记也是用户
    /// 自己写的，按前者呈现。给模型一个协议里没定义的新取值，要先改提示词并跑评测对照
    /// （提示词纪律），而身份标注本就由 Rust 负责，模型无须知道这一层。
    #[must_use]
    pub fn model_facing(self) -> Self {
        match self {
            Self::QuickNote => Self::FormalNote,
            other => other,
        }
    }
}

impl Vault {
    /// 预览返回前统一复核所有输入来源；无新增也要过这道闸，否则模型等待期间来源变了，
    /// 旧 no-op 会被误当成当前事实。按路径只读一次，继续沿同句柄的字节上限与逃逸守卫。
    pub fn topic_evidence_sources_current(
        &self,
        evidence: &[TopicEvidenceInput],
    ) -> crate::Result<bool> {
        let mut checked = HashMap::new();
        for item in evidence {
            let path = item.source.source_path.as_str();
            let expected = item.source.source_hash.as_str();
            if let Some(previous) = checked.get(path) {
                if *previous != expected {
                    return Ok(false);
                }
                continue;
            }
            let opened = match self.open_in_vault_limited(path, MAX_SOURCE_BYTES) {
                Ok(opened) => opened,
                Err(VaultOpenError::PathEscape) => {
                    return Err(VaultError::PathEscape(path.into()));
                }
                Err(VaultOpenError::Unreadable) => return Ok(false),
            };
            if ContentHash::of(body_of(&opened.content).as_bytes()).as_str() != expected {
                return Ok(false);
            }
            checked.insert(path, expected);
        }
        Ok(true)
    }

    /// 从安全打开的来源中提取有界证据。正式笔记按行；录音/转写只在 `policy` 授权且为
    /// 已完成权威转写时，按带时间码的源文行提取并标为「转写记载」证据（PRD-16-047）。
    /// 删除、草稿、未授权资料和聚合页不混入；路径逃逸直接失败，普通不可读来源计入遗漏，
    /// 避免把剩余输入称为全量。
    pub fn topic_evidence(
        &self,
        notes: &[String],
        policy: SourcePolicy,
    ) -> crate::Result<TopicEvidenceBatch> {
        self.topic_evidence_resuming(notes, policy, &std::collections::BTreeMap::new())
    }

    /// 同 [`Self::topic_evidence`]，但上一轮只读了前半的来源从断点接着读：`resume` 是
    /// 路径 →（那一版的正文版本，下一个片段序号）。版本对不上（来源改过）就从头读——
    /// 断点只对同一版正文有意义。单篇超长来源因此能分几轮读完，而不是永远只读开头。
    pub fn topic_evidence_resuming(
        &self,
        notes: &[String],
        policy: SourcePolicy,
        resume: &std::collections::BTreeMap<String, (String, u32)>,
    ) -> crate::Result<TopicEvidenceBatch> {
        let mut evidence = Vec::new();
        let mut coverage = Vec::new();
        let mut seen = HashSet::new();
        let mut omitted_sources = 0u32;
        let mut input_complete = true;
        let mut total_chars = 0usize;
        let mut processed_sources = 0usize;
        for rel in notes.iter().take(MAX_EVIDENCE) {
            if evidence.len() >= MAX_EVIDENCE || total_chars >= MAX_BATCH_EXCERPT_CHARS {
                break;
            }
            processed_sources += 1;
            if !rel.to_ascii_lowercase().ends_with(".md") {
                omitted_sources = omitted_sources.saturating_add(1);
                input_complete = false;
                continue;
            }
            let opened = match self.open_in_vault_limited(rel, MAX_SOURCE_BYTES) {
                Ok(opened) => opened,
                Err(VaultOpenError::PathEscape) => {
                    return Err(VaultError::PathEscape(rel.into()));
                }
                Err(VaultOpenError::Unreadable) => {
                    omitted_sources = omitted_sources.saturating_add(1);
                    input_complete = false;
                    continue;
                }
            };
            if !seen.insert(opened.id) {
                continue;
            }
            let meta = read_luna(&opened.content);
            let body = body_of(&opened.content);
            let is_source_material = meta.stage() == NoteStage::SourceMaterial;
            let is_quick_note = meta.note_type == NoteType::QuickNote;
            let transcript = if is_source_material {
                authoritative_transcript_lines(&opened.content)
            } else {
                Vec::new()
            };
            let has_text = if is_source_material {
                !transcript.is_empty()
            } else {
                !body.trim().is_empty()
            };
            let eligibility = evidence_eligibility(
                true,
                meta.note_type,
                meta.stage(),
                meta.topic || meta.knowledge_map,
                has_text,
                policy,
                false,
            );
            if eligibility.reason != EvidenceReason::Eligible {
                omitted_sources = omitted_sources.saturating_add(1);
                input_complete = false;
                continue;
            }
            let (fragments, kind) = if is_source_material {
                (transcript, TopicEvidenceKind::AuthoritativeTranscript)
            } else if is_quick_note {
                (
                    extract_formal_lines(&opened.content),
                    TopicEvidenceKind::QuickNote,
                )
            } else {
                (
                    extract_formal_lines(&opened.content),
                    TopicEvidenceKind::FormalNote,
                )
            };
            if fragments.is_empty() {
                omitted_sources = omitted_sources.saturating_add(1);
                input_complete = false;
                continue;
            }
            let source_hash = ContentHash::of(body.as_bytes()).to_string();
            let start = resume
                .get(rel)
                .filter(|(version, _)| *version == source_hash)
                .map_or(0, |(_, next)| *next as usize);
            let start = if start < fragments.len() { start } else { 0 };
            let mut accepted = 0usize;
            for fragment in &fragments[start..] {
                let chars = fragment.quote.chars().count();
                if evidence.len() + accepted >= MAX_EVIDENCE
                    || total_chars + chars > MAX_BATCH_EXCERPT_CHARS
                {
                    break;
                }
                total_chars += chars;
                accepted += 1;
            }
            let read_to = start + accepted;
            let source_complete = start == 0
                && read_to == fragments.len()
                && fragments.iter().all(|fragment| !fragment.clipped);
            if !source_complete {
                input_complete = false;
            }
            if accepted == 0 {
                omitted_sources = omitted_sources.saturating_add(1);
                continue;
            }
            coverage.push(SourceCoverage {
                path: opened.rel.clone(),
                version: source_hash.clone(),
                next_fragment: (read_to < fragments.len())
                    .then(|| u32::try_from(read_to).unwrap_or(u32::MAX)),
            });
            let title = crate::title_of(&opened.rel).to_owned();
            for fragment in fragments.into_iter().skip(start).take(accepted) {
                evidence.push(TopicEvidenceInput {
                    id: format!("e{:04}", evidence.len()),
                    excerpt: fragment.quote.clone(),
                    kind,
                    source: PlanEvidence {
                        source_path: opened.rel.clone(),
                        source_title: title.clone(),
                        source_hash: source_hash.clone(),
                        locator: EvidenceLocator {
                            heading_path: fragment.heading_path,
                            quote: fragment.quote,
                            context: String::new(),
                            line_hint: Some(fragment.line_hint),
                            time_start_ms: fragment.time.map(|(start, _)| start),
                            time_end_ms: fragment.time.map(|(_, end)| end),
                        },
                        complete_input: source_complete,
                    },
                });
            }
        }
        if processed_sources < notes.len() {
            omitted_sources = omitted_sources
                .saturating_add(u32::try_from(notes.len() - processed_sources).unwrap_or(u32::MAX));
            input_complete = false;
        }
        Ok(TopicEvidenceBatch {
            evidence,
            input_complete,
            omitted_sources,
            coverage,
        })
    }

    /// 把一次已校验模型结果转成同一份可发布预览。无新增不造空主题；
    /// 预览仍重验来源版本，最终发布还须再过专用发布闸门。
    pub fn scoped_topic_preview_from_generation(
        &self,
        target: &str,
        topic_src: Option<&str>,
        scope: TopicScope,
        evidence: &[TopicEvidenceInput],
        generation: &TopicGeneration,
    ) -> crate::Result<Option<String>> {
        let TopicGeneration::Claims { claims } = generation else {
            return Ok(None);
        };
        validate_topic_generation_input(&scope, evidence)
            .map_err(|error| VaultError::InvalidReplace(error.to_string()))?;
        let mut sources_by_claim = Vec::with_capacity(claims.len());
        for claim in claims {
            if claim.id.is_nil() || claim.evidence_indices.is_empty() {
                return Err(VaultError::InvalidReplace(
                    "invalid generated claim references".into(),
                ));
            }
            let mut seen = BTreeSet::new();
            let mut sources = Vec::with_capacity(claim.evidence_indices.len());
            for index in &claim.evidence_indices {
                let item = evidence.get(*index).ok_or_else(|| {
                    VaultError::InvalidReplace("generated evidence ID is out of range".into())
                })?;
                if !seen.insert(index) {
                    return Err(VaultError::InvalidReplace(
                        "generated evidence ID was repeated".into(),
                    ));
                }
                sources.push(crate::ClaimSource {
                    path: &item.source.source_path,
                    title: &item.source.source_title,
                    source_hash: &item.source.source_hash,
                    locator: item.source.locator.clone(),
                    complete_input: item.source.complete_input,
                });
            }
            sources_by_claim.push(sources);
        }
        let prepared: Vec<crate::NewClaim<'_>> = claims
            .iter()
            .zip(&sources_by_claim)
            .map(|(claim, sources)| crate::NewClaim {
                id: claim.id,
                text: &claim.text,
                kind: claim.kind,
                sources,
            })
            .collect();
        self.scoped_topic_content(target, topic_src, scope, &prepared)
            .map(Some)
    }
}

struct FormalLine {
    quote: String,
    heading_path: Vec<String>,
    line_hint: u32,
    clipped: bool,
    /// 转写行的起止（会话绝对毫秒）；正文行为 `None`。
    time: Option<(u64, u64)>,
}

/// 已完成权威转写的源文行：转写块内以时间码开头、非书签的行，止点取下一行起点。
/// 非权威（实时预览、旧文件）返回空——没有文本可作证据，与「没有转写」同样对待。
fn authoritative_transcript_lines(content: &str) -> Vec<FormalLine> {
    if !read_luna(content).transcript_authoritative {
        return Vec::new();
    }
    transcript_source_lines(content)
}

/// 转写块内的源文行，不论是否权威（实时预览稿也在内）。
fn transcript_source_lines(content: &str) -> Vec<FormalLine> {
    let body = body_of(content);
    let prefix_lines = content[..content.len() - body.len()]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count();
    let mut inside = false;
    let mut lines: Vec<(u64, String, u32, bool)> = Vec::new();
    for (index, line) in body.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed == crate::vault::TRANSCRIPT_BEGIN {
            inside = true;
            continue;
        }
        if trimmed == crate::vault::TRANSCRIPT_END {
            inside = false;
            continue;
        }
        if !inside {
            continue;
        }
        let Some((at, rest)) = crate::timecode::parse_timecode(trimmed) else {
            continue;
        };
        let text = rest.trim();
        if text.is_empty() || text.starts_with('\u{1F516}') {
            continue;
        }
        let clipped = text.chars().count() > MAX_EXCERPT_CHARS;
        lines.push((
            u64::try_from(at.as_millis()).unwrap_or(u64::MAX),
            text.chars().take(MAX_EXCERPT_CHARS).collect(),
            u32::try_from(prefix_lines + index + 1).unwrap_or(u32::MAX),
            clipped,
        ));
    }
    let starts: Vec<u64> = lines.iter().map(|(start, ..)| *start).collect();
    lines
        .into_iter()
        .enumerate()
        .map(|(index, (start, quote, line_hint, clipped))| FormalLine {
            quote,
            heading_path: Vec::new(),
            line_hint,
            clipped,
            time: Some((
                start,
                starts.get(index + 1).copied().unwrap_or(start).max(start),
            )),
        })
        .collect()
}

/// 这篇录音/转写是否有可作证据的权威转写文本（候选判据与证据提取同一口径）。
#[must_use]
pub fn has_authoritative_transcript(content: &str) -> bool {
    !authoritative_transcript_lines(content).is_empty()
}

/// 这篇录音/转写有没有任何转写文本（含录音时的实时预览稿）。与
/// [`has_authoritative_transcript`] 一起区分「只有实时稿」与「还什么都没有」（PRD-16-065）。
#[must_use]
pub fn has_transcript_text(content: &str) -> bool {
    !transcript_source_lines(content).is_empty()
}

fn extract_formal_lines(content: &str) -> Vec<FormalLine> {
    let body = body_of(content);
    let prefix_lines = content[..content.len() - body.len()]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count();
    let mut headings: Vec<(usize, String)> = Vec::new();
    let mut fragments = Vec::new();
    for (index, line) in body.lines().enumerate() {
        let line = line.trim();
        if let Some((level, title)) = markdown_heading(line) {
            while headings
                .last()
                .is_some_and(|(current, _)| *current >= level)
            {
                headings.pop();
            }
            headings.push((level, title.to_owned()));
            continue;
        }
        if line.is_empty() || line.starts_with("<!--") {
            continue;
        }
        let clipped = line.chars().count() > MAX_EXCERPT_CHARS;
        let quote: String = line.chars().take(MAX_EXCERPT_CHARS).collect();
        fragments.push(FormalLine {
            quote,
            heading_path: headings.iter().map(|(_, title)| title.clone()).collect(),
            line_hint: u32::try_from(prefix_lines + index + 1).unwrap_or(u32::MAX),
            clipped,
            time: None,
        });
    }
    fragments
}

fn markdown_heading(line: &str) -> Option<(usize, &str)> {
    let level = line.bytes().take_while(|byte| *byte == b'#').count();
    if (1..=6).contains(&level) && line[level..].starts_with(' ') {
        let title = line[level..].trim();
        if !title.is_empty() {
            return Some((level, title));
        }
    }
    None
}

#[derive(Serialize)]
struct ModelEvidence<'a> {
    id: &'a str,
    source_title: &'a str,
    heading_path: &'a [String],
    excerpt: &'a str,
    kind: TopicEvidenceKind,
    complete_input: bool,
}

#[derive(Serialize)]
struct ModelRequest<'a> {
    scope: &'a TopicScope,
    input_complete: bool,
    evidence: Vec<ModelEvidence<'a>>,
}

/// 模型只能从请求中的 ID 集合选择；声明 ID、出处路径及 Markdown 由 Rust 生成。
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TopicGeneratedClaim {
    pub text: String,
    pub kind: ClaimKind,
    pub evidence_ids: Vec<String>,
    /// 见 [`DiscardedModelId`]。
    #[serde(default, rename = "id")]
    pub discarded_id: Option<DiscardedModelId>,
}

/// 模型照着输入里既有声明 `{id, text}` 的形状，给新声明也带上的 `id`——只认字符串、值丢弃。
///
/// 声明 ID 一律由 Rust 生成（[`GroundedClaim::id`]），模型给的这个值从不被读取，拒收它
/// 只是白白消耗重试。实测（2026-09-28，60 组评测集）：生产模型 `gpt-oss-20b` 三轮 10 份
/// 格式失败里 7 份只多这一个键，本机 `qwen3.5:4b` 同样常见；在提示词里明令不带 `id` 两轮
/// 对照无改善。只放过这**一个**键：其余未知键、重复键、非字符串的 `id` 照旧整份拒绝。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiscardedModelId;

impl<'de> Deserialize<'de> for DiscardedModelId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(|_| Self)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum TopicModelOutput {
    NoOp { reason: String },
    Claims { claims: Vec<TopicGeneratedClaim> },
}

/// 已封闭引用的结果；`evidence_indices` 与本次输入的顺序一一对应。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TopicGeneration {
    NoOp { reason: String },
    Claims { claims: Vec<GroundedClaim> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroundedClaim {
    /// 模型不提供此 ID；解析成功时由 Rust 生成，预览与发布复用同一个值。
    pub id: Uuid,
    pub text: String,
    pub kind: ClaimKind,
    pub evidence_indices: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TopicGenerationError {
    #[error("invalid topic scope or evidence input")]
    InvalidInput,
    #[error("model output was truncated or did not reach a complete terminal state")]
    IncompleteOutput,
    #[error("invalid model output structure")]
    InvalidOutput,
    #[error("model output cites an unknown or repeated evidence ID")]
    InvalidEvidenceId,
    #[error("topic generation exceeded its budget")]
    BudgetExceeded,
}

/// 在模型调用前检查范围和封闭证据列表；不把无效输入交给模型猜。
pub fn validate_topic_generation_input(
    scope: &TopicScope,
    evidence: &[TopicEvidenceInput],
) -> Result<(), TopicGenerationError> {
    if !scope.valid() || evidence.is_empty() {
        return Err(TopicGenerationError::InvalidInput);
    }
    if evidence.len() > MAX_EVIDENCE {
        return Err(TopicGenerationError::BudgetExceeded);
    }
    let mut ids = BTreeSet::new();
    for item in evidence {
        if item.excerpt.chars().count() > MAX_EXCERPT_CHARS {
            return Err(TopicGenerationError::BudgetExceeded);
        }
        if item.id.is_empty()
            || item.id.len() > 64
            || !item
                .id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
            || !ids.insert(item.id.as_str())
            || item.excerpt.trim().is_empty()
            || item.source.source_path.trim().is_empty()
            || item.source.source_title.trim().is_empty()
            || item.source.source_hash.trim().is_empty()
            || !item.source.locator.valid()
            || !item.excerpt.contains(item.source.locator.quote.trim())
        {
            return Err(TopicGenerationError::InvalidInput);
        }
    }
    Ok(())
}

/// 生成调用的用户消息载荷。完整范围和覆盖事实一次传入；来源路径、hash 与锚点留在
/// Rust，模型只需返回本次封闭 ID。调用方须把该 JSON 当资料传入专用能力提示词。
pub fn topic_generation_payload(
    scope: &TopicScope,
    evidence: &[TopicEvidenceInput],
    input_complete: bool,
) -> Result<String, TopicGenerationError> {
    validate_topic_generation_input(scope, evidence)?;
    if input_complete && evidence.iter().any(|item| !item.source.complete_input) {
        return Err(TopicGenerationError::InvalidInput);
    }
    let request = ModelRequest {
        scope,
        input_complete,
        evidence: evidence
            .iter()
            .map(|item| ModelEvidence {
                id: &item.id,
                source_title: &item.source.source_title,
                heading_path: &item.source.locator.heading_path,
                excerpt: &item.excerpt,
                kind: item.kind.model_facing(),
                complete_input: item.source.complete_input,
            })
            .collect(),
    };
    let payload =
        serde_json::to_string(&request).map_err(|_| TopicGenerationError::InvalidInput)?;
    if payload.len() > MAX_REQUEST_BYTES {
        return Err(TopicGenerationError::BudgetExceeded);
    }
    Ok(payload)
}

/// `output_complete` 必须取自 Provider 的实际终态，不能由 JSON 自称完整。
/// JSON 有效不代表语义支持；后续仍须重新验证盘上来源版本及范围。
pub fn parse_topic_generation(
    scope: &TopicScope,
    evidence: &[TopicEvidenceInput],
    raw: &str,
    output_complete: bool,
) -> Result<TopicGeneration, TopicGenerationError> {
    validate_topic_generation_input(scope, evidence)?;
    if !output_complete {
        return Err(TopicGenerationError::IncompleteOutput);
    }
    if raw.len() > MAX_OUTPUT_BYTES {
        return Err(TopicGenerationError::BudgetExceeded);
    }
    let output: TopicModelOutput = serde_json::from_str(json_envelope(raw))
        .map_err(|_| TopicGenerationError::InvalidOutput)?;
    match output {
        TopicModelOutput::NoOp { reason } => {
            if reason.trim().is_empty() || reason.chars().any(char::is_control) {
                return Err(TopicGenerationError::InvalidOutput);
            }
            Ok(TopicGeneration::NoOp { reason })
        }
        TopicModelOutput::Claims { claims } => {
            if claims.is_empty() {
                return Err(TopicGenerationError::InvalidOutput);
            }
            if claims.len() > MAX_CLAIMS {
                return Err(TopicGenerationError::BudgetExceeded);
            }
            let mut grounded = Vec::with_capacity(claims.len());
            let mut texts = BTreeSet::new();
            for claim in claims {
                grounded.push(ground_claim(
                    claim.text,
                    claim.kind,
                    &claim.evidence_ids,
                    evidence,
                    &mut texts,
                )?);
            }
            Ok(TopicGeneration::Claims { claims: grounded })
        }
    }
}

/// 单条模型声明 → 封闭引用。预算、控制字符、输出内重复、未知/重复 ID 与声明类别
/// 对证据类型的约束在初稿与增量两条协议里只写这一份。
fn ground_claim(
    text: String,
    kind: ClaimKind,
    evidence_ids: &[String],
    evidence: &[TopicEvidenceInput],
    texts: &mut BTreeSet<String>,
) -> Result<GroundedClaim, TopicGenerationError> {
    if text.chars().count() > MAX_CLAIM_CHARS || evidence_ids.len() > MAX_REFERENCES_PER_CLAIM {
        return Err(TopicGenerationError::BudgetExceeded);
    }
    if text.trim().is_empty()
        || text.chars().any(char::is_control)
        || evidence_ids.is_empty()
        || !texts.insert(normalized_claim_text(&text))
    {
        return Err(TopicGenerationError::InvalidOutput);
    }
    let mut seen = BTreeSet::new();
    let mut evidence_indices = Vec::with_capacity(evidence_ids.len());
    for id in evidence_ids {
        if !seen.insert(id) {
            return Err(TopicGenerationError::InvalidEvidenceId);
        }
        let index = evidence
            .iter()
            .position(|item| item.id == *id)
            .ok_or(TopicGenerationError::InvalidEvidenceId)?;
        evidence_indices.push(index);
    }
    // 证据身份由 Rust 按已核验的来源类型推导，不交给模型贴标签：模型只决定一条声明是不是
    // 分歧；其余引用了转写的即「转写记载」，否则为综合。真机 2026-09-25 生产模型把引用转写的
    // 声明标成 synthesis，旧协议据此整份拒绝——而这个标签本来就不该由模型决定。
    let cites_transcript = evidence_indices
        .iter()
        .any(|index| evidence[*index].kind == TopicEvidenceKind::AuthoritativeTranscript);
    let kind = match kind {
        ClaimKind::Conflict => ClaimKind::Conflict,
        ClaimKind::Synthesis | ClaimKind::TranscriptRecord if cites_transcript => {
            ClaimKind::TranscriptRecord
        }
        ClaimKind::Synthesis | ClaimKind::TranscriptRecord => ClaimKind::Synthesis,
    };
    Ok(GroundedClaim {
        id: Uuid::now_v7(),
        text,
        kind,
        evidence_indices,
    })
}

/// 重复判定只做确定性的规范化（去首尾空白、折叠空白），不做语义相似度：
/// 换一种说法的重复由模型在提示词约束下自行排除，Rust 只拦得住逐字重复。
fn normalized_claim_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

// 增量请求携带的既有声明上限；更大的主题须先分块，不能静默只给模型前一部分。
const MAX_EXISTING_CLAIMS: usize = 64;

#[derive(Serialize)]
struct ModelExistingClaim<'a> {
    id: String,
    text: &'a str,
    kind: ClaimKind,
}

#[derive(Serialize)]
struct UpdateRequest<'a> {
    scope: &'a TopicScope,
    input_complete: bool,
    existing_claims: Vec<ModelExistingClaim<'a>>,
    evidence: Vec<ModelEvidence<'a>>,
}

/// 增量模型输出的单条声明。`conflicts_with` 只能指向本次请求里的既有声明短 ID。
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TopicUpdateClaim {
    pub text: String,
    pub kind: ClaimKind,
    pub evidence_ids: Vec<String>,
    #[serde(default)]
    pub conflicts_with: Option<String>,
    /// 这条是某条既有声明的更新版（来源明确说那件事变了）：只能指向本次请求里的既有
    /// 非分歧声明短 ID，不能与 `conflicts_with` 同时出现，也不能是分歧。
    #[serde(default)]
    pub replaces: Option<String>,
    /// 见 [`DiscardedModelId`]。
    #[serde(default, rename = "id")]
    pub discarded_id: Option<DiscardedModelId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
enum TopicUpdateOutput {
    NoOp { reason: String },
    Claims { claims: Vec<TopicUpdateClaim> },
}

/// 已封闭引用的一条增量；`conflicts_with` / `replaces` 已映射回主题中的真实声明 ID。
/// 替换时 `claim.id` 沿用被替换那条的 ID——指向它的分歧关系因此仍然成立。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroundedUpdate {
    pub claim: GroundedClaim,
    pub conflicts_with: Option<Uuid>,
    pub replaces: Option<Uuid>,
}

/// 增量结果的三种显式终态：模型判无新增、全部与既有声明逐字重复、或有可计划的声明。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TopicUpdate {
    NoOp {
        reason: String,
    },
    AllDuplicates {
        duplicates: u32,
    },
    Claims {
        claims: Vec<GroundedUpdate>,
        duplicates: u32,
    },
}

/// 模型输出的**外壳**容错，内容不容错：只剥掉整段 Markdown 代码围栏，以及一个完整 JSON
/// 值之后至多三个零星收尾符（引号、反引号、多余的右括号）；剥完仍按封闭协议逐字段严格
/// 解析——未知键、缺引用、裸标识符照样拒绝，不修补内容。固定评测集（2026-09-28，
/// `qwen3.5:4b-mlx`）一轮 15 份无效输出里 9 份只是这层外壳：`}]}"`、`}]}'`、`` }]}` ``、多一个 `}`
/// 或 ```` ```json ```` 围栏，内容本身合规，却要白白消耗一次重试预算。同类做法即 LangChain
/// `parse_json_markdown` 先剥围栏再校验模式。
fn json_envelope(raw: &str) -> &str {
    let mut text = raw.trim();
    if let Some(rest) = text.strip_prefix("```") {
        let rest = rest.strip_prefix("json").unwrap_or(rest);
        if let Some(inner) = rest.trim_end().strip_suffix("```") {
            text = inner.trim();
        }
    }
    let mut values = serde_json::Deserializer::from_str(text).into_iter::<serde::de::IgnoredAny>();
    if let Some(Ok(_)) = values.next() {
        let end = values.byte_offset();
        let tail = text[end..].trim();
        if tail.chars().count() <= 3
            && tail
                .chars()
                .all(|c| matches!(c, '"' | '\'' | '`' | '}' | ']'))
        {
            return &text[..end];
        }
    }
    text
}

fn existing_claim_ref(index: usize) -> String {
    format!("c{index:04}")
}

/// 增量调用的用户消息载荷：完整范围、既有声明纯文本（短 ID，不含路径与链接）与封闭证据。
pub fn topic_update_payload(
    scope: &TopicScope,
    existing: &[crate::TopicClaimStatement],
    evidence: &[TopicEvidenceInput],
    input_complete: bool,
) -> Result<String, TopicGenerationError> {
    validate_topic_generation_input(scope, evidence)?;
    if input_complete && evidence.iter().any(|item| !item.source.complete_input) {
        return Err(TopicGenerationError::InvalidInput);
    }
    if existing.len() > MAX_EXISTING_CLAIMS {
        return Err(TopicGenerationError::BudgetExceeded);
    }
    let request = UpdateRequest {
        scope,
        input_complete,
        existing_claims: existing
            .iter()
            .enumerate()
            .map(|(index, claim)| ModelExistingClaim {
                id: existing_claim_ref(index),
                text: &claim.text,
                kind: claim.kind,
            })
            .collect(),
        evidence: evidence
            .iter()
            .map(|item| ModelEvidence {
                id: &item.id,
                source_title: &item.source.source_title,
                heading_path: &item.source.locator.heading_path,
                excerpt: &item.excerpt,
                kind: item.kind.model_facing(),
                complete_input: item.source.complete_input,
            })
            .collect(),
    };
    let payload =
        serde_json::to_string(&request).map_err(|_| TopicGenerationError::InvalidInput)?;
    if payload.len() > MAX_REQUEST_BYTES {
        return Err(TopicGenerationError::BudgetExceeded);
    }
    Ok(payload)
}

/// 解析增量输出。分歧必须指向一条既有声明、非分歧不得带指向；与既有声明逐字重复的
/// 条目被剔除并计数，全部重复时返回显式终态而不是空计划。
pub fn parse_topic_update(
    scope: &TopicScope,
    existing: &[crate::TopicClaimStatement],
    evidence: &[TopicEvidenceInput],
    raw: &str,
    output_complete: bool,
) -> Result<TopicUpdate, TopicGenerationError> {
    validate_topic_generation_input(scope, evidence)?;
    if existing.len() > MAX_EXISTING_CLAIMS {
        return Err(TopicGenerationError::BudgetExceeded);
    }
    if !output_complete {
        return Err(TopicGenerationError::IncompleteOutput);
    }
    if raw.len() > MAX_OUTPUT_BYTES {
        return Err(TopicGenerationError::BudgetExceeded);
    }
    let output: TopicUpdateOutput = serde_json::from_str(json_envelope(raw))
        .map_err(|_| TopicGenerationError::InvalidOutput)?;
    let claims = match output {
        TopicUpdateOutput::NoOp { reason } => {
            if reason.trim().is_empty() || reason.chars().any(char::is_control) {
                return Err(TopicGenerationError::InvalidOutput);
            }
            return Ok(TopicUpdate::NoOp { reason });
        }
        TopicUpdateOutput::Claims { claims } => claims,
    };
    if claims.is_empty() {
        return Err(TopicGenerationError::InvalidOutput);
    }
    if claims.len() > MAX_CLAIMS {
        return Err(TopicGenerationError::BudgetExceeded);
    }
    let known: BTreeSet<String> = existing
        .iter()
        .map(|claim| normalized_claim_text(&claim.text))
        .collect();
    let mut texts = BTreeSet::new();
    let mut grounded = Vec::with_capacity(claims.len());
    let mut duplicates = 0u32;
    let find_existing = |reference: &str| {
        existing
            .iter()
            .enumerate()
            .find(|(index, _)| existing_claim_ref(*index) == reference)
            .map(|(_, claim)| claim)
            .ok_or(TopicGenerationError::InvalidEvidenceId)
    };
    // 同一批里每条既有声明至多被替换一次，且不能同时被替换又被标分歧——两种说法互相矛盾。
    let mut replaced = BTreeSet::new();
    let mut disputed = BTreeSet::new();
    for claim in claims {
        let conflicts_with = match (claim.kind, claim.conflicts_with.as_deref()) {
            (ClaimKind::Conflict, Some(reference)) => Some(
                existing
                    .iter()
                    .enumerate()
                    .find(|(index, _)| existing_claim_ref(*index) == reference)
                    .map(|(_, claim)| claim.id)
                    .ok_or(TopicGenerationError::InvalidEvidenceId)?,
            ),
            (ClaimKind::Conflict, None) | (_, Some(_)) => {
                return Err(TopicGenerationError::InvalidOutput);
            }
            (_, None) => None,
        };
        let replaces = match (claim.kind, claim.replaces.as_deref(), conflicts_with) {
            (_, None, _) => None,
            (ClaimKind::Conflict, Some(_), _) | (_, Some(_), Some(_)) => {
                return Err(TopicGenerationError::InvalidOutput);
            }
            (_, Some(reference), None) => {
                let target = find_existing(reference)?;
                // 分歧是两种说法并存的记录，不能被一边「更新」掉；要更新就新增一条。
                if target.kind == ClaimKind::Conflict || !replaced.insert(target.id) {
                    return Err(TopicGenerationError::InvalidOutput);
                }
                Some(target.id)
            }
        };
        if let Some(related) = conflicts_with {
            disputed.insert(related);
        }
        let is_duplicate = known.contains(&normalized_claim_text(&claim.text));
        let mut claim = ground_claim(
            claim.text,
            claim.kind,
            &claim.evidence_ids,
            evidence,
            &mut texts,
        )?;
        if is_duplicate {
            duplicates = duplicates.saturating_add(1);
            continue;
        }
        if let Some(original) = replaces {
            claim.id = original;
        }
        grounded.push(GroundedUpdate {
            claim,
            conflicts_with,
            replaces,
        });
    }
    if replaced.iter().any(|id| disputed.contains(id)) {
        return Err(TopicGenerationError::InvalidOutput);
    }
    if grounded.is_empty() {
        return Ok(TopicUpdate::AllDuplicates { duplicates });
    }
    Ok(TopicUpdate::Claims {
        claims: grounded,
        duplicates,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EvidenceLocator;

    fn evidence() -> Vec<TopicEvidenceInput> {
        [
            ("e-project-a", "项目 A 决定本周发布。", "a.md"),
            ("e-project-b", "项目 B 决定下月发布。", "b.md"),
        ]
        .into_iter()
        .map(|(id, excerpt, path)| TopicEvidenceInput {
            id: id.to_owned(),
            excerpt: excerpt.to_owned(),
            kind: TopicEvidenceKind::FormalNote,
            source: PlanEvidence {
                source_path: path.to_owned(),
                source_title: path.to_owned(),
                source_hash: "source-v1".to_owned(),
                locator: EvidenceLocator {
                    heading_path: vec![],
                    quote: excerpt.to_owned(),
                    context: String::new(),
                    line_hint: Some(1),
                    time_start_ms: None,
                    time_end_ms: None,
                },
                complete_input: true,
            },
        })
        .collect()
    }

    /// 「只有实时稿」与「还什么都没有」「已完成权威转写」三种情形分得开（PRD-16-065）。
    #[test]
    fn transcript_text_is_told_apart_from_authoritative_text() {
        let note = |authoritative: bool, line: &str| {
            format!(
                "---\nluna:\n  schema: 1\n  type: recording\n{}---\n# 周会\n\n\
                 <!-- luna:transcript -->\n{line}<!-- /luna:transcript -->\n",
                if authoritative {
                    "  transcript_authoritative: true\n"
                } else {
                    ""
                }
            )
        };
        let preview = note(false, "[00:00:01.000] 实时稿里的一句。\n");
        assert!(has_transcript_text(&preview));
        assert!(!has_authoritative_transcript(&preview));
        let empty = note(false, "");
        assert!(!has_transcript_text(&empty));
        let done = note(true, "[00:00:01.000] 批量转写的一句。\n");
        assert!(has_transcript_text(&done) && has_authoritative_transcript(&done));
    }

    #[test]
    fn closed_ids_keep_the_two_projects_separate_structurally() {
        let scope = TopicScope::from_tag("项目/A");
        let input = evidence();
        let output = r#"{"status":"claims","claims":[{"text":"项目 A 本周发布","kind":"synthesis","evidence_ids":["e-project-a"]}]}"#;
        let TopicGeneration::Claims { claims } =
            parse_topic_generation(&scope, &input, output, true).unwrap()
        else {
            panic!("expected claims");
        };
        assert_eq!(claims[0].evidence_indices, vec![0]);
        // 结构校验不能判断 B 的语义是否属于 A；跨主题错误仍需真实模型评测。
    }

    #[test]
    fn invalid_reference_and_truncated_output_never_become_claims() {
        let scope = TopicScope::from_tag("项目/A");
        let input = evidence();
        let foreign = r#"{"status":"claims","claims":[{"text":"发布","kind":"synthesis","evidence_ids":["invented"]}]}"#;
        assert_eq!(
            parse_topic_generation(&scope, &input, foreign, true),
            Err(TopicGenerationError::InvalidEvidenceId)
        );
        let valid = r#"{"status":"claims","claims":[{"text":"发布","kind":"synthesis","evidence_ids":["e-project-a"]}]}"#;
        assert_eq!(
            parse_topic_generation(&scope, &input, valid, false),
            Err(TopicGenerationError::IncompleteOutput)
        );
        assert_eq!(
            parse_topic_generation(&scope, &input, "```json\n{}\n```", true),
            Err(TopicGenerationError::InvalidOutput)
        );
        // 本机模型探针曾把 no_op 值输出为裸标识符；不得修补后继续写盘。
        assert_eq!(
            parse_topic_generation(
                &scope,
                &input,
                r#"{"status":no_op,"reason":"无范围内证据"}"#,
                true,
            ),
            Err(TopicGenerationError::InvalidOutput)
        );
        // 多出的顶层解释字段也说明结果没有遵守封闭协议。
        assert_eq!(
            parse_topic_generation(
                &scope,
                &input,
                r#"{"status":"claims","reason":"额外解释","claims":[{"text":"发布","kind":"synthesis","evidence_ids":["e-project-a"]}]}"#,
                true,
            ),
            Err(TopicGenerationError::InvalidOutput)
        );
    }

    #[test]
    fn no_op_is_explicit_and_empty_claims_are_invalid() {
        let scope = TopicScope::from_tag("项目/A");
        let input = evidence();
        let no_op = r#"{"status":"no_op","reason":"没有范围内的新证据"}"#;
        assert!(matches!(
            parse_topic_generation(&scope, &input, no_op, true),
            Ok(TopicGeneration::NoOp { .. })
        ));
        assert_eq!(
            parse_topic_generation(&scope, &input, r#"{"status":"claims","claims":[]}"#, true),
            Err(TopicGenerationError::InvalidOutput)
        );
    }

    #[test]
    fn input_requires_unique_ids_and_anchor_inside_excerpt() {
        let scope = TopicScope::from_tag("项目/A");
        let mut input = evidence();
        input[1].id = input[0].id.clone();
        assert_eq!(
            validate_topic_generation_input(&scope, &input),
            Err(TopicGenerationError::InvalidInput)
        );
        input[1].id = "e-project-b".to_owned();
        input[1].source.locator.quote = "别的片段".to_owned();
        assert_eq!(
            validate_topic_generation_input(&scope, &input),
            Err(TopicGenerationError::InvalidInput)
        );
    }

    #[test]
    fn payload_carries_full_scope_without_exposing_source_paths() {
        let scope = TopicScope {
            title: "项目 A".to_owned(),
            description: "只讨论发布时间".to_owned(),
            includes: vec!["项目/A".to_owned()],
            excludes: vec!["项目/B".to_owned()],
        };
        let input = evidence();
        let payload = topic_generation_payload(&scope, &input, true).unwrap();
        let value: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(value["scope"]["includes"][0], "项目/A");
        assert_eq!(value["scope"]["excludes"][0], "项目/B");
        assert_eq!(value["scope"]["description"], "只讨论发布时间");
        assert_eq!(value["evidence"][0]["id"], "e-project-a");
        assert!(value["evidence"][0].get("source_path").is_none());
        assert!(value["evidence"][0].get("source_hash").is_none());
        assert!(value["evidence"][0].get("locator").is_none());
        assert!(value["evidence"][0].get("heading_path").is_some());
    }

    #[test]
    fn payload_cannot_claim_full_coverage_of_partial_evidence() {
        let scope = TopicScope::from_tag("项目/A");
        let mut input = evidence();
        input[0].source.complete_input = false;
        assert_eq!(
            topic_generation_payload(&scope, &input, true),
            Err(TopicGenerationError::InvalidInput)
        );
        let payload = topic_generation_payload(&scope, &input, false).unwrap();
        let value: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(value["input_complete"], false);
        assert_eq!(value["evidence"][0]["complete_input"], false);
    }

    fn only_kind(output: Result<TopicGeneration, TopicGenerationError>) -> ClaimKind {
        match output {
            Ok(TopicGeneration::Claims { claims }) if claims.len() == 1 => claims[0].kind,
            other => panic!("expected one grounded claim: {other:?}"),
        }
    }

    #[test]
    fn transcript_evidence_is_always_recorded_as_a_transcript_record() {
        let scope = TopicScope::from_tag("项目/A");
        let mut input = evidence();
        input[0].kind = TopicEvidenceKind::AuthoritativeTranscript;
        // 模型标成 synthesis 也不会把录音里说的话呈现为已核实的综合结论。
        let synthesis = r#"{"status":"claims","claims":[{"text":"本周发布","kind":"synthesis","evidence_ids":["e-project-a"]}]}"#;
        assert_eq!(
            only_kind(parse_topic_generation(&scope, &input, synthesis, true)),
            ClaimKind::TranscriptRecord
        );
        let record = r#"{"status":"claims","claims":[{"text":"转写记载本周发布","kind":"transcript_record","evidence_ids":["e-project-a"]}]}"#;
        assert_eq!(
            only_kind(parse_topic_generation(&scope, &input, record, true)),
            ClaimKind::TranscriptRecord
        );
    }

    #[test]
    fn formal_note_evidence_is_never_labeled_as_transcript() {
        let scope = TopicScope::from_tag("项目/A");
        let input = evidence();
        let output = r#"{"status":"claims","claims":[{"text":"本周发布","kind":"transcript_record","evidence_ids":["e-project-a"]}]}"#;
        assert_eq!(
            only_kind(parse_topic_generation(&scope, &input, output, true)),
            ClaimKind::Synthesis
        );
    }

    #[test]
    fn repeated_statement_is_not_an_additional_claim() {
        let scope = TopicScope::from_tag("项目/A");
        let input = evidence();
        let duplicate = r#"{"status":"claims","claims":[{"text":"本周发布","kind":"synthesis","evidence_ids":["e-project-a"]},{"text":"本周发布","kind":"synthesis","evidence_ids":["e-project-a"]}]}"#;
        assert_eq!(
            parse_topic_generation(&scope, &input, duplicate, true),
            Err(TopicGenerationError::InvalidOutput)
        );
    }

    #[test]
    fn formal_extraction_preserves_two_project_sections_and_exact_lines() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        vault
            .write(
                "Notes/共享.md",
                "# 周会\n\n## 项目 A\n\nA 本周发布。\n\n## 项目 B\n\nB 下月发布。\n",
            )
            .unwrap();
        let batch = vault
            .topic_evidence(&["Notes/共享.md".to_owned()], SourcePolicy::FORMAL_ONLY)
            .unwrap();
        assert!(batch.input_complete);
        assert_eq!(batch.omitted_sources, 0);
        assert_eq!(batch.evidence.len(), 2);
        assert_eq!(
            batch.evidence[0].source.locator.heading_path,
            ["周会", "项目 A"]
        );
        assert_eq!(
            batch.evidence[1].source.locator.heading_path,
            ["周会", "项目 B"]
        );
        assert_eq!(batch.evidence[0].source.locator.quote, "A 本周发布。");
        let source = vault.read("Notes/共享.md").unwrap();
        let hint = batch.evidence[1].source.locator.line_hint.unwrap() as usize;
        assert!(source
            .content
            .lines()
            .nth(hint - 1)
            .unwrap()
            .contains("B 下月发布。"));
        assert_eq!(
            batch.evidence[0].source.source_hash,
            vault.body_fingerprint("Notes/共享.md").unwrap()
        );
        let payload =
            topic_generation_payload(&TopicScope::from_tag("项目/A"), &batch.evidence, true)
                .unwrap();
        assert!(payload.contains("项目/A"));
        assert!(payload.contains("A 本周发布"));
        assert!(payload.contains("B 下月发布"));
    }

    #[test]
    fn no_op_input_versions_must_still_match_current_sources() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        vault
            .write("Notes/A.md", "# 项目 A\n\n本周发布。\n")
            .unwrap();
        let batch = vault
            .topic_evidence(&["Notes/A.md".into()], SourcePolicy::FORMAL_ONLY)
            .unwrap();
        assert!(vault
            .topic_evidence_sources_current(&batch.evidence)
            .unwrap());

        let mut conflicting = batch.evidence.clone();
        let mut old = batch.evidence[0].clone();
        old.source.source_hash = "sha256:old".into();
        conflicting.push(old);
        assert!(!vault.topic_evidence_sources_current(&conflicting).unwrap());

        vault
            .write("Notes/A.md", "# 项目 A\n\n发布计划延期。\n")
            .unwrap();
        assert!(!vault
            .topic_evidence_sources_current(&batch.evidence)
            .unwrap());
        std::fs::remove_file(dir.path().join("Notes/A.md")).unwrap();
        assert!(!vault
            .topic_evidence_sources_current(&batch.evidence)
            .unwrap());
    }

    #[test]
    fn payload_disambiguates_identical_lines_by_project_heading() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        vault
            .write(
                "Notes/共享.md",
                "# 周会\n\n## 项目 A\n\n本周发布。\n\n## 项目 B\n\n本周发布。\n",
            )
            .unwrap();
        let batch = vault
            .topic_evidence(&["Notes/共享.md".to_owned()], SourcePolicy::FORMAL_ONLY)
            .unwrap();
        let payload = topic_generation_payload(
            &TopicScope::from_tag("项目/A"),
            &batch.evidence,
            batch.input_complete,
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(
            value["evidence"][0]["excerpt"],
            value["evidence"][1]["excerpt"]
        );
        assert_eq!(value["evidence"][0]["heading_path"][1], "项目 A");
        assert_eq!(value["evidence"][1]["heading_path"][1], "项目 B");
    }

    #[test]
    fn oversized_source_is_omitted_before_model_input() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        vault
            .write("Notes/过大.md", &"A".repeat(MAX_SOURCE_BYTES + 1))
            .unwrap();
        let batch = vault
            .topic_evidence(&["Notes/过大.md".to_owned()], SourcePolicy::FORMAL_ONLY)
            .unwrap();
        assert!(batch.evidence.is_empty());
        assert_eq!(batch.omitted_sources, 1);
        assert!(!batch.input_complete);
    }

    #[test]
    fn formal_extraction_skips_drafts_material_and_rejects_path_escape() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        vault.write("正式.md", "已定稿。\n").unwrap();
        vault
            .write("草稿.md", "---\nluna:\n  stage: draft\n---\n\n尚未整理。\n")
            .unwrap();
        vault
            .write(
                "转写.md",
                "---\nluna:\n  type: transcript\n---\n\n[00:01:00] 未获范围授权。\n",
            )
            .unwrap();
        let batch = vault
            .topic_evidence(
                &[
                    "正式.md".to_owned(),
                    "草稿.md".to_owned(),
                    "转写.md".to_owned(),
                    "正式.md".to_owned(),
                ],
                SourcePolicy::FORMAL_ONLY,
            )
            .unwrap();
        assert_eq!(batch.evidence.len(), 1);
        assert_eq!(batch.omitted_sources, 2);
        assert!(!batch.input_complete);
        assert_eq!(
            vault
                .topic_evidence(&["../库外.md".to_owned()], SourcePolicy::FORMAL_ONLY)
                .unwrap_err()
                .code(),
            "VAULT_PATH_ESCAPE"
        );
    }

    #[test]
    fn formal_extraction_qualification_matrix_excludes_derived_and_missing_sources() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        let corpus = [
            ("Notes/正式.md", "已定稿。\n"),
            (
                "Notes/速记.md",
                "---\nluna:\n  type: quick-note\n---\n\n尚未整理。\n",
            ),
            (
                "Notes/草稿.md",
                "---\nluna:\n  stage: draft\n---\n\n仍是草稿。\n",
            ),
            (
                "Transcripts/来源.md",
                "---\nluna:\n  type: transcript\n---\n\n未经范围授权。\n",
            ),
            (
                "Chats/对话.md",
                "---\nluna:\n  type: chat\n---\n\n对话正文。\n",
            ),
            (
                "Knowledge/主题.md",
                "---\nluna:\n  topic: true\n---\n\n主题正文。\n",
            ),
            (
                "Knowledge/地图.md",
                "---\nluna:\n  knowledge_map: true\n---\n\n地图正文。\n",
            ),
            ("Notes/空白.md", "\n"),
        ];
        for (path, content) in corpus {
            vault.write(path, content).unwrap();
        }
        let mut paths = corpus
            .iter()
            .map(|(path, _)| (*path).to_owned())
            .collect::<Vec<_>>();
        paths.push("Notes/已删除.md".into());
        paths.push("Notes/不是 Markdown.txt".into());
        let batch = vault
            .topic_evidence(&paths, SourcePolicy::FORMAL_ONLY)
            .unwrap();
        assert_eq!(batch.evidence.len(), 1);
        assert_eq!(batch.evidence[0].source.source_path, "Notes/正式.md");
        assert_eq!(batch.omitted_sources, 9);
        assert!(!batch.input_complete);
    }

    #[test]
    fn evidence_budget_marks_every_included_fragment_as_partial() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        let body = (0..80)
            .map(|index| format!("第 {index} 条可核查的来源内容。"))
            .collect::<Vec<_>>()
            .join("\n");
        vault.write("许多行.md", &body).unwrap();
        let batch = vault
            .topic_evidence(&["许多行.md".to_owned()], SourcePolicy::FORMAL_ONLY)
            .unwrap();
        assert_eq!(batch.evidence.len(), MAX_EVIDENCE);
        assert!(!batch.input_complete);
        assert!(batch
            .evidence
            .iter()
            .all(|item| !item.source.complete_input));
    }

    #[test]
    fn closed_generation_previews_and_publishes_only_its_cited_claim() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        vault
            .write("Notes/共享.md", "# 项目\n\nA 本周发布。\n\nB 下月发布。\n")
            .unwrap();
        let batch = vault
            .topic_evidence(&["Notes/共享.md".to_owned()], SourcePolicy::FORMAL_ONLY)
            .unwrap();
        let scope = TopicScope::from_tag("项目/A");
        let raw = format!(
            r#"{{"status":"claims","claims":[{{"text":"A 本周发布","kind":"synthesis","evidence_ids":["{}"]}}]}}"#,
            batch.evidence[0].id
        );
        let generation = parse_topic_generation(&scope, &batch.evidence, &raw, true).unwrap();
        let target = "Knowledge/项目 A.md";
        let preview = vault
            .scoped_topic_preview_from_generation(
                target,
                Some("项目/A"),
                scope.clone(),
                &batch.evidence,
                &generation,
            )
            .unwrap()
            .unwrap();
        let checked = crate::validate_topic_document(&preview).unwrap();
        assert_eq!(checked.scope, scope);
        assert_eq!(checked.claims.len(), 1);
        assert_eq!(checked.citations.len(), 1);
        assert!(preview.contains("A 本周发布"));
        assert!(!preview.contains("B 下月发布"));
        vault.publish_scoped_topic(target, &preview).unwrap();
        assert_eq!(vault.read(target).unwrap().content, preview);
    }

    #[test]
    fn preview_rejects_a_source_edit_after_generation() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        vault.write("Notes/A.md", "A 本周发布。\n").unwrap();
        let batch = vault
            .topic_evidence(&["Notes/A.md".to_owned()], SourcePolicy::FORMAL_ONLY)
            .unwrap();
        let scope = TopicScope::from_tag("项目/A");
        let raw = r#"{"status":"claims","claims":[{"text":"A 本周发布","kind":"synthesis","evidence_ids":["e0000"]}]}"#;
        let generation = parse_topic_generation(&scope, &batch.evidence, raw, true).unwrap();
        vault.write("Notes/A.md", "A 延后发布。\n").unwrap();
        assert!(vault
            .scoped_topic_preview_from_generation(
                "Knowledge/项目 A.md",
                Some("项目/A"),
                scope,
                &batch.evidence,
                &generation,
            )
            .is_err());
    }

    fn existing() -> Vec<crate::TopicClaimStatement> {
        vec![crate::TopicClaimStatement {
            id: Uuid::parse_str("019d1a08-7cf3-72d7-90fa-382ef03524ad").unwrap(),
            text: "项目 A 决定本周发布。".into(),
            kind: ClaimKind::Synthesis,
        }]
    }

    #[test]
    fn update_payload_shows_existing_text_but_no_paths() {
        let scope = TopicScope::from_tag("项目/A");
        let payload = topic_update_payload(&scope, &existing(), &evidence(), true).unwrap();
        assert!(payload.contains("\"existing_claims\""));
        assert!(payload.contains("\"c0000\""));
        assert!(payload.contains("项目 A 决定本周发布。"));
        assert!(!payload.contains("source_path") && !payload.contains("source_hash"));
        assert!(!payload.contains("019d1a08"), "真实声明 ID 不交给模型");
        let many = vec![existing()[0].clone(); MAX_EXISTING_CLAIMS + 1];
        assert_eq!(
            topic_update_payload(&scope, &many, &evidence(), true),
            Err(TopicGenerationError::BudgetExceeded)
        );
    }

    #[test]
    fn update_filters_verbatim_duplicates_and_maps_conflicts() {
        let scope = TopicScope::from_tag("项目/A");
        let input = evidence();
        let known = existing();
        let output = r#"{"status":"claims","claims":[
            {"text":"  项目 A 决定本周发布。 ","kind":"synthesis","evidence_ids":["e-project-a"]},
            {"text":"项目 A 改为下周发布","kind":"conflict","evidence_ids":["e-project-a"],"conflicts_with":"c0000"}
        ]}"#;
        match parse_topic_update(&scope, &known, &input, output, true).unwrap() {
            TopicUpdate::Claims { claims, duplicates } => {
                assert_eq!(duplicates, 1);
                assert_eq!(claims.len(), 1);
                assert_eq!(claims[0].conflicts_with, Some(known[0].id));
                assert_eq!(claims[0].claim.kind, ClaimKind::Conflict);
            }
            other => panic!("unexpected update: {other:?}"),
        }
        let only_duplicate = r#"{"status":"claims","claims":[{"text":"项目 A 决定本周发布。","kind":"synthesis","evidence_ids":["e-project-a"]}]}"#;
        assert_eq!(
            parse_topic_update(&scope, &known, &input, only_duplicate, true),
            Ok(TopicUpdate::AllDuplicates { duplicates: 1 })
        );
        let no_op = r#"{"status":"no_op","reason":"没有新增"}"#;
        assert!(matches!(
            parse_topic_update(&scope, &known, &input, no_op, true),
            Ok(TopicUpdate::NoOp { .. })
        ));
        assert_eq!(
            parse_topic_update(&scope, &known, &input, no_op, false),
            Err(TopicGenerationError::IncompleteOutput)
        );
    }

    #[test]
    fn update_replacement_keeps_the_original_id_and_rejects_ambiguous_forms() {
        let scope = TopicScope::from_tag("项目/A");
        let input = evidence();
        let mut known = existing();
        known.push(crate::TopicClaimStatement {
            id: Uuid::parse_str("019d1a08-7cf3-72d7-90fa-382ef0350000").unwrap(),
            text: "另一份记录称推迟。".into(),
            kind: ClaimKind::Conflict,
        });
        let replace = r#"{"status":"claims","claims":[
            {"text":"项目 A 改为下周发布","kind":"synthesis","evidence_ids":["e-project-a"],"replaces":"c0000"}
        ]}"#;
        match parse_topic_update(&scope, &known, &input, replace, true).unwrap() {
            TopicUpdate::Claims { claims, .. } => {
                assert_eq!(claims[0].replaces, Some(known[0].id));
                assert_eq!(claims[0].claim.id, known[0].id, "更新版沿用原声明 ID");
                assert_eq!(claims[0].conflicts_with, None);
            }
            other => panic!("unexpected update: {other:?}"),
        }
        for (output, expected) in [
            // 分歧不能是「更新」；也不能既更新又标分歧。
            (
                r#"{"status":"claims","claims":[{"text":"改","kind":"conflict","evidence_ids":["e-project-a"],"conflicts_with":"c0000","replaces":"c0000"}]}"#,
                TopicGenerationError::InvalidOutput,
            ),
            (
                r#"{"status":"claims","claims":[{"text":"改","kind":"synthesis","evidence_ids":["e-project-a"],"replaces":"c0009"}]}"#,
                TopicGenerationError::InvalidEvidenceId,
            ),
            // 既有分歧不能被一边「更新」掉。
            (
                r#"{"status":"claims","claims":[{"text":"改","kind":"synthesis","evidence_ids":["e-project-a"],"replaces":"c0001"}]}"#,
                TopicGenerationError::InvalidOutput,
            ),
            // 同一条被替换两次、或同一批里既被替换又被标分歧：说法互相矛盾。
            (
                r#"{"status":"claims","claims":[{"text":"改一","kind":"synthesis","evidence_ids":["e-project-a"],"replaces":"c0000"},{"text":"改二","kind":"synthesis","evidence_ids":["e-project-a"],"replaces":"c0000"}]}"#,
                TopicGenerationError::InvalidOutput,
            ),
            (
                r#"{"status":"claims","claims":[{"text":"改一","kind":"synthesis","evidence_ids":["e-project-a"],"replaces":"c0000"},{"text":"不同说法","kind":"conflict","evidence_ids":["e-project-a"],"conflicts_with":"c0000"}]}"#,
                TopicGenerationError::InvalidOutput,
            ),
        ] {
            assert_eq!(
                parse_topic_update(&scope, &known, &input, output, true),
                Err(expected),
                "{output}"
            );
        }
    }

    #[test]
    fn model_output_envelope_is_stripped_but_content_stays_strict() {
        let scope = TopicScope::from_tag("项目/A");
        let input = evidence();
        let valid = r#"{"status":"claims","claims":[{"text":"项目 A 本周发布","kind":"synthesis","evidence_ids":["e-project-a"]}]}"#;
        // 评测集首跑真见过的外壳：围栏、收尾多一个引号/反引号/右括号。
        for wrapped in [
            format!("```json\n{valid}\n```"),
            format!("```\n{valid}\n```"),
            format!("{valid}\""),
            format!("{valid}'"),
            format!("{valid}`"),
            format!("{valid}}}"),
            format!("  {valid}\n"),
        ] {
            assert!(
                matches!(
                    parse_topic_generation(&scope, &input, &wrapped, true),
                    Ok(TopicGeneration::Claims { .. })
                ),
                "{wrapped}"
            );
            assert!(
                matches!(
                    parse_topic_update(&scope, &existing(), &input, &wrapped, true),
                    Ok(TopicUpdate::Claims { .. })
                ),
                "{wrapped}"
            );
        }
        // 外壳之外的东西一律不收：第二个 JSON、收尾文字、未知键、没闭合的围栏。
        for rejected in [
            format!("{valid}{valid}"),
            format!("{valid} 以上是结果"),
            format!("{valid}\"\"\"\""),
            format!("```json\n{valid}"),
            format!("说明：{valid}"),
            r#"{"status":"claims","claims":[{"text":"项目 A 本周发布","kind":"synthesis","evidence_ids":["e-project-a"],"source_path":"a.md"}]}"#.to_owned(),
        ] {
            assert_eq!(
                parse_topic_generation(&scope, &input, &rejected, true),
                Err(TopicGenerationError::InvalidOutput),
                "{rejected}"
            );
        }
    }

    #[test]
    fn a_model_supplied_claim_id_is_discarded_but_nothing_else_is_tolerated() {
        let scope = TopicScope::from_tag("项目/A");
        let input = evidence();
        let known = existing();
        // 生产模型真见过的形状：新声明照抄既有声明的 `{id, text}`，id 是编的短号。
        let with_id = r#"{"status":"claims","claims":[{"id":"c0001","text":"项目 A 本周发布","kind":"synthesis","evidence_ids":["e-project-a"]}]}"#;
        let Ok(TopicUpdate::Claims { claims, .. }) =
            parse_topic_update(&scope, &known, &input, with_id, true)
        else {
            panic!("只多一个 id 的输出应当可用");
        };
        assert_ne!(
            claims[0].claim.id, known[0].id,
            "声明 ID 仍由 Rust 生成，不采用模型给的"
        );
        assert!(matches!(
            parse_topic_generation(&scope, &input, with_id, true),
            Ok(TopicGeneration::Claims { .. })
        ));
        for rejected in [
            // 非字符串的 id、重复的 id、多出别的键——都不是「照抄形状」，照旧拒绝。
            r#"{"status":"claims","claims":[{"id":1,"text":"项目 A 本周发布","kind":"synthesis","evidence_ids":["e-project-a"]}]}"#,
            r#"{"status":"claims","claims":[{"id":"c1","id":"c2","text":"项目 A 本周发布","kind":"synthesis","evidence_ids":["e-project-a"]}]}"#,
            r#"{"status":"claims","claims":[{"id":"c1","text":"项目 A 本周发布","kind":"synthesis","evidence_ids":["e-project-a"],"replaced_text":"旧"}]}"#,
            r#"{"status":"claims","id":"x","claims":[{"text":"项目 A 本周发布","kind":"synthesis","evidence_ids":["e-project-a"]}]}"#,
        ] {
            assert_eq!(
                parse_topic_update(&scope, &known, &input, rejected, true),
                Err(TopicGenerationError::InvalidOutput),
                "{rejected}"
            );
        }
    }

    #[test]
    fn update_rejects_unanchored_or_misplaced_conflicts() {
        let scope = TopicScope::from_tag("项目/A");
        let input = evidence();
        let known = existing();
        for (output, expected) in [
            (
                r#"{"status":"claims","claims":[{"text":"冲突","kind":"conflict","evidence_ids":["e-project-a"]}]}"#,
                TopicGenerationError::InvalidOutput,
            ),
            (
                r#"{"status":"claims","claims":[{"text":"冲突","kind":"conflict","evidence_ids":["e-project-a"],"conflicts_with":"c0009"}]}"#,
                TopicGenerationError::InvalidEvidenceId,
            ),
            (
                r#"{"status":"claims","claims":[{"text":"新增","kind":"synthesis","evidence_ids":["e-project-a"],"conflicts_with":"c0000"}]}"#,
                TopicGenerationError::InvalidOutput,
            ),
            (
                r#"{"status":"claims","claims":[{"text":"新增","kind":"synthesis","evidence_ids":["c0000"]}]}"#,
                TopicGenerationError::InvalidEvidenceId,
            ),
        ] {
            assert_eq!(
                parse_topic_update(&scope, &known, &input, output, true),
                Err(expected),
                "{output}"
            );
        }
    }
}
