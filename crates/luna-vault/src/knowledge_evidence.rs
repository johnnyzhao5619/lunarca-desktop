//! 主题范围和逐条证据的文件契约（ADR-134 / TDD-06 §3.9）。
//!
//! 这里只解释、验证与渲染 Markdown 中的系统声明；是否写盘由 Vault 的原子写、快照和
//! `synth_body` 守卫决定。旧主题没有此格式时返回明确错误，不把它解释成一篇空的新主题。

use crate::frontmatter::{body_of, read_luna, NoteStage, NoteType};
use crate::parse::extract_link_occurrences;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

pub const KNOWLEDGE_SCHEMA: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TopicScope {
    pub title: String,
    pub description: String,
    pub includes: Vec<String>,
    #[serde(default)]
    pub excludes: Vec<String>,
}

impl TopicScope {
    /// 使用完整标签路径保留语境；末级标签只用于现有文件名，不再充当生成范围。
    #[must_use]
    pub fn from_tag(tag: &str) -> Self {
        Self {
            title: tag.split('/').collect::<Vec<_>>().join(" · "),
            description: String::new(),
            includes: vec![tag.to_owned()],
            excludes: Vec::new(),
        }
    }

    /// 「正文写到了本主题」这一路候选用的名字（PRD-16-063，ADR-136 修订）：范围标题、包含
    /// 标签的整条路径拼接（「项目/甲」→「项目甲」）与三字以上的末段。名字去掉空白与「·」
    /// 分隔后再比较；短于三字的一律不用——两字词（「发布」「会议」）在日常文字里到处都是，
    /// 用它圈候选等于把全库都圈进来，trigram 全文索引也匹配不了短于三字的词。
    ///
    /// 纯语义相似度**不**作候选扩展：固定评测集上，名字只差一个字的主题（项目甲 / 项目乙）
    /// 语义上几乎分不开，召回七成时就混进 36/480 条别的主题的来源；「写到名字」召回全部、
    /// 且仅有的命中都是确实写到了该主题的多主题来源（2026-09-30 检索层实验，TDD-06 §3.9.2）。
    #[must_use]
    pub fn mention_names(&self) -> Vec<String> {
        const MIN_CHARS: usize = 3;
        let compact = |text: &str| -> String {
            text.chars()
                .filter(|c| !c.is_whitespace() && *c != '·' && *c != '/')
                .collect()
        };
        let mut names: Vec<String> = Vec::new();
        let mut push = |name: String| {
            if name.chars().count() >= MIN_CHARS
                && !names.iter().any(|known| known.eq_ignore_ascii_case(&name))
            {
                names.push(name);
            }
        };
        // 由标签生成的标题带「 · 」分隔（「项目 · 甲」），原样拿去匹配正文找不到任何东西；
        // 只有用户写的自然标题才原样用。
        if !self.title.contains('·') {
            push(self.title.trim().to_owned());
        }
        push(compact(&self.title));
        for tag in &self.includes {
            push(compact(tag));
            if let Some(leaf) = tag.rsplit('/').next() {
                push(leaf.trim().to_owned());
            }
        }
        names
    }

    pub(crate) fn valid(&self) -> bool {
        !self.title.trim().is_empty()
            && !self.title.chars().any(char::is_control)
            && !self.description.chars().any(char::is_control)
            && !self.includes.is_empty()
            && self
                .includes
                .iter()
                .all(|item| !item.trim().is_empty() && !item.chars().any(char::is_control))
            && self
                .excludes
                .iter()
                .all(|item| !item.trim().is_empty() && !item.chars().any(char::is_control))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimKind {
    Synthesis,
    TranscriptRecord,
    Conflict,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceLocator {
    pub heading_path: Vec<String>,
    pub quote: String,
    pub context: String,
    pub line_hint: Option<u32>,
    pub time_start_ms: Option<u64>,
    pub time_end_ms: Option<u64>,
}

impl EvidenceLocator {
    pub(crate) fn valid(&self) -> bool {
        !self.quote.trim().is_empty()
            && match (self.time_start_ms, self.time_end_ms) {
                (Some(start), Some(end)) => start <= end,
                (None, None) => true,
                _ => false,
            }
    }
}

/// 同版本仍要验证复合定位：重复引文只接受确实落在原行的行提示，不能随意选第一处。
pub(crate) fn citation_quote_is_unique_or_hinted(content: &str, locator: &EvidenceLocator) -> bool {
    let quote = locator.quote.trim();
    if quote.is_empty() {
        return false;
    }
    let matches = body_of(content)
        .lines()
        .filter(|line| line.contains(quote))
        .count();
    if matches == 1 {
        return true;
    }
    matches > 1
        && locator.line_hint.is_some_and(|hint| {
            content
                .lines()
                .nth(hint.saturating_sub(1) as usize)
                .is_some_and(|line| line.contains(quote))
        })
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CitationMeta {
    pub source_hash: String,
    pub locator: EvidenceLocator,
    /// `false` 表示生成时只覆盖来源的一部分，不能把结论呈现为整篇已处理。
    pub complete_input: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimMeta {
    pub kind: ClaimKind,
    /// 只在「分歧」声明上：与它并列的那条既有声明（同页 claim ID）。两边各留出处，
    /// 不判定谁对，也不按新旧取舍（TDD-06 §3.9.1）。其余声明不写这个键，字节不变。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflicts_with: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeMeta {
    pub schema: u32,
    pub scope: TopicScope,
    #[serde(default)]
    pub claims: BTreeMap<String, ClaimMeta>,
    /// `claim_uuid:source_link_ordinal`；路径只在该声明的 Markdown 行内链接中。
    #[serde(default)]
    pub citations: BTreeMap<String, CitationMeta>,
}

/// 文件中一条出处的只读投影；路径由声明块的链接解析，不在元数据里复制。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopicCitation {
    pub id: String,
    pub source_path: Option<String>,
    pub meta: CitationMeta,
}

/// 出处状态由当前 Vault 文件派生，不是持久化的所有权标记。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceState {
    Current,
    SourceChanged,
    Missing,
    Ambiguous,
    Partial,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CitationEvidence {
    pub id: String,
    pub source_path: Option<String>,
    pub state: EvidenceState,
}

/// 单页来源状态查询的有界结果；`revision` 锁定分页期间的主题文件版本。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CitationEvidencePage {
    pub revision: String,
    pub total: u32,
    pub items: Vec<CitationEvidence>,
    pub next_offset: Option<u32>,
}

/// 新格式主题的维护闸门；与用户可读/可检索、正文所有权分别判断。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TopicMaintenanceState {
    Ready,
    NotTopic,
    NeedsUpgrade,
    UnknownSchema,
    InvalidEvidence,
    BodyChanged,
    MetadataChanged,
    MissingBaseline,
}

impl KnowledgeMeta {
    #[must_use]
    pub fn new(scope: TopicScope) -> Self {
        Self {
            schema: KNOWLEDGE_SCHEMA,
            scope,
            claims: BTreeMap::new(),
            citations: BTreeMap::new(),
        }
    }
}

/// 调用方已经在 Vault 内确认过身份与正文版本的来源快照。
pub struct ClaimSource<'a> {
    pub path: &'a str,
    pub title: &'a str,
    pub source_hash: &'a str,
    pub locator: EvidenceLocator,
    pub complete_input: bool,
}

/// 已由调用方核验来源版本的声明输入；模型只提供 `text`，不提供链接或文件内容。
pub struct NewClaim<'a> {
    pub id: Uuid,
    pub text: &'a str,
    pub kind: ClaimKind,
    pub sources: &'a [ClaimSource<'a>],
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum KnowledgeError {
    #[error("not a topic page")]
    NotTopic,
    #[error("topic needs an explicit evidence-format upgrade")]
    NeedsUpgrade,
    #[error("unknown knowledge schema: {0}")]
    UnknownSchema(u32),
    #[error("invalid topic scope")]
    InvalidScope,
    #[error("invalid knowledge metadata")]
    InvalidMetadata,
    #[error("invalid claim boundaries or citations")]
    InvalidClaims,
}

/// 只渲染一块；是否允许应用、如何原子写盘和留快照由上层统一处理。
/// 模型文本按纯文本转义，来源链接只由 Rust 从已校验的来源路径生成。
pub fn render_generated_claim(
    id: Uuid,
    text: &str,
    kind: ClaimKind,
    target_path: &str,
    sources: &[ClaimSource<'_>],
) -> Result<(String, ClaimMeta, BTreeMap<String, CitationMeta>), KnowledgeError> {
    if text.trim().is_empty() || sources.is_empty() || text.chars().any(char::is_control) {
        return Err(KnowledgeError::InvalidClaims);
    }
    let mut citations = BTreeMap::new();
    let mut links = Vec::with_capacity(sources.len());
    // 同一条声明多次引用同一篇来源时给标签编号（·1、·2），读者一眼看出是同一篇的不同两处，
    // 而不是重复出处（同 Wikipedia 以 {{rp}} 标同一文献不同页）。每处仍是独立链接、独立
    // 定位锚；标签只供阅读，不被解析回来。
    let mut seen_per_path: BTreeMap<&str, usize> = BTreeMap::new();
    for (ordinal, source) in sources.iter().enumerate() {
        if source.path.trim().is_empty()
            || source.path.chars().any(char::is_control)
            || source.title.trim().is_empty()
            || source.title.chars().any(char::is_control)
            || source.source_hash.trim().is_empty()
            || !source.locator.valid()
        {
            return Err(KnowledgeError::InvalidClaims);
        }
        let repeats = sources
            .iter()
            .filter(|other| other.path == source.path)
            .count();
        let nth = seen_per_path.entry(source.path).or_insert(0);
        *nth += 1;
        let label = if repeats > 1 {
            format!("{}·{nth}", escape_markdown_text(source.title))
        } else {
            escape_markdown_text(source.title)
        };
        links.push(crate::markdown_link(target_path, source.path, &label));
        citations.insert(
            format!("{id}:{ordinal}"),
            CitationMeta {
                source_hash: source.source_hash.to_owned(),
                locator: source.locator.clone(),
                complete_input: source.complete_input,
            },
        );
    }
    let statement = escape_markdown_text(text.trim());
    let block = format!(
        "<!-- luna:claim:{id} -->\n- {statement}（{}）\n<!-- /luna:claim -->",
        links.join("；")
    );
    Ok((
        block,
        ClaimMeta {
            kind,
            conflicts_with: None,
        },
        citations,
    ))
}

/// 从封闭的逐条声明构造正文和元数据；不接受模型产出的整页 Markdown。
pub fn render_scoped_topic(
    target_path: &str,
    scope: TopicScope,
    claims: &[NewClaim<'_>],
) -> Result<(String, KnowledgeMeta), KnowledgeError> {
    if !scope.valid() {
        return Err(KnowledgeError::InvalidScope);
    }
    if claims.is_empty() {
        return Err(KnowledgeError::InvalidClaims);
    }
    let mut knowledge = KnowledgeMeta::new(scope);
    let mut blocks = Vec::with_capacity(claims.len());
    for claim in claims {
        let (block, meta, citations) =
            render_generated_claim(claim.id, claim.text, claim.kind, target_path, claim.sources)?;
        if knowledge
            .claims
            .insert(claim.id.to_string(), meta)
            .is_some()
        {
            return Err(KnowledgeError::InvalidClaims);
        }
        knowledge.citations.extend(citations);
        blocks.push(block);
    }
    let title = escape_markdown_text(&knowledge.scope.title);
    Ok((format!("# {title}\n\n{}\n", blocks.join("\n\n")), knowledge))
}

fn escape_markdown_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for char in text.chars() {
        if matches!(
            char,
            '\\' | '*' | '_' | '`' | '[' | ']' | '<' | '>' | '(' | ')'
        ) {
            out.push('\\');
        }
        out.push(char);
    }
    out
}

/// 前台可以只读旧主题；写入前必须得到严格验证过的新版契约。
pub fn validate_topic_document(content: &str) -> Result<KnowledgeMeta, KnowledgeError> {
    let meta = read_luna(content);
    if !meta.topic || meta.knowledge_map {
        return Err(KnowledgeError::NotTopic);
    }
    let raw = meta.knowledge.ok_or(KnowledgeError::NeedsUpgrade)?;
    let knowledge: KnowledgeMeta =
        serde_yaml_ng::from_value(raw).map_err(|_| KnowledgeError::InvalidMetadata)?;
    if knowledge.schema != KNOWLEDGE_SCHEMA {
        return Err(KnowledgeError::UnknownSchema(knowledge.schema));
    }
    if !knowledge.scope.valid() {
        return Err(KnowledgeError::InvalidScope);
    }
    validate_claims(body_of(content), &knowledge)?;
    Ok(knowledge)
}

/// 按声明块中的链接次序恢复出处；同一文件被引用两次仍是两个 citation ID。
pub fn topic_citations(
    topic_rel: &str,
    content: &str,
) -> Result<Vec<TopicCitation>, KnowledgeError> {
    let knowledge = validate_topic_document(content)?;
    let mut current_id: Option<String> = None;
    let mut citations = Vec::with_capacity(knowledge.citations.len());
    for line in body_of(content).lines() {
        if let Some(id) = marker_id(line, "<!-- luna:claim:") {
            current_id = Some(id);
        } else if line == "<!-- /luna:claim -->" {
            current_id = None;
        } else if line.starts_with("- ") {
            if let Some(id) = current_id.as_deref() {
                for (ordinal, link) in extract_link_occurrences(line).iter().enumerate() {
                    let key = format!("{id}:{ordinal}");
                    let meta = knowledge
                        .citations
                        .get(&key)
                        .ok_or(KnowledgeError::InvalidClaims)?
                        .clone();
                    citations.push(TopicCitation {
                        id: key,
                        source_path: citation_source_path(topic_rel, link),
                        meta,
                    });
                }
            }
        }
    }
    Ok(citations)
}

/// 已有系统声明的纯文本投影，供增量生成避免重复和标注分歧；不含来源路径或链接。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopicClaimStatement {
    pub id: Uuid,
    pub text: String,
    pub kind: ClaimKind,
}

/// 只从严格验证过的新格式主题读取；声明行由 [`render_generated_claim`] 渲染，
/// 故末尾的「（来源链接）」与反斜杠转义都可逆。无法还原即视为格式错误，不猜。
pub fn topic_claim_statements(content: &str) -> Result<Vec<TopicClaimStatement>, KnowledgeError> {
    let knowledge = validate_topic_document(content)?;
    let mut current: Option<Uuid> = None;
    let mut statements = Vec::with_capacity(knowledge.claims.len());
    for line in body_of(content).lines() {
        if let Some(id) = marker_id(line, "<!-- luna:claim:") {
            current = Uuid::parse_str(&id).ok();
        } else if line == "<!-- /luna:claim -->" {
            current = None;
        } else if let (Some(id), Some(item)) = (current, line.strip_prefix("- ")) {
            let body = item
                .strip_suffix('）')
                .and_then(|rest| rest.rfind("（[").map(|at| &rest[..at]))
                .ok_or(KnowledgeError::InvalidClaims)?;
            let text = unescape_markdown_text(body);
            if text.trim().is_empty() {
                return Err(KnowledgeError::InvalidClaims);
            }
            let kind = knowledge
                .claims
                .get(&id.to_string())
                .ok_or(KnowledgeError::InvalidClaims)?
                .kind;
            statements.push(TopicClaimStatement { id, text, kind });
        }
    }
    if statements.len() != knowledge.claims.len() {
        return Err(KnowledgeError::InvalidClaims);
    }
    Ok(statements)
}

fn unescape_markdown_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(char) = chars.next() {
        if char == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
                continue;
            }
        }
        out.push(char);
    }
    out
}

/// 声明块里一条出处链接指向的库内来源；越界、锚点或非 Markdown 目标不算。
fn citation_source_path(topic_rel: &str, link: &str) -> Option<String> {
    let decoded = luna_sync_core::links::percent_decode(link);
    (decoded.ends_with(".md") && !decoded.starts_with('/') && !decoded.contains(['#', '?', '\\']))
        .then(|| {
            luna_sync_core::links::normalize_vault_rel(
                luna_sync_core::links::parent_dir(topic_rel),
                &decoded,
            )
        })
        .flatten()
}

/// 阅读器里「正文中第 `occurrence` 个（0 起）指向 `href` 的库内链接」是哪一条出处。
///
/// 阅读器只知道被点的链接写的是什么、它是同一 href 的第几次出现；声明边界与出处编号
/// 只在这里判定。落在声明块外（人工正文、升级前的旧行）返回 `None`，由调用方按普通
/// 链接打开——不把块外链接冒充成有定位锚的出处。
pub fn topic_citation_at_link(
    topic_rel: &str,
    content: &str,
    href: &str,
    occurrence: usize,
) -> Result<Option<TopicCitation>, KnowledgeError> {
    let knowledge = validate_topic_document(content)?;
    let mut current: Option<String> = None;
    let mut seen = 0usize;
    for line in body_of(content).lines() {
        if let Some(id) = marker_id(line, "<!-- luna:claim:") {
            current = Some(id);
            continue;
        }
        if line == "<!-- /luna:claim -->" {
            current = None;
            continue;
        }
        for (ordinal, link) in extract_link_occurrences(line).iter().enumerate() {
            if link != href {
                continue;
            }
            if seen != occurrence {
                seen += 1;
                continue;
            }
            let Some(id) = current.as_deref().filter(|_| line.starts_with("- ")) else {
                return Ok(None);
            };
            let key = format!("{id}:{ordinal}");
            let meta = knowledge
                .citations
                .get(&key)
                .ok_or(KnowledgeError::InvalidClaims)?
                .clone();
            return Ok(Some(TopicCitation {
                id: key,
                source_path: citation_source_path(topic_rel, link),
                meta,
            }));
        }
    }
    Ok(None)
}

/// 正文里「没有逐条出处」的内容行数：系统声明块之外、非标题的非空行。
///
/// 升级旧页时预览它、升级后阅读页头照同一判据标「未核验」——两处各数一遍，就会出现
/// 预览说 3 行、页头说 1 行的矛盾。标题只是结构，不是待核验的说法，不计入。
#[must_use]
pub fn unverified_content_lines(body: &str) -> usize {
    let mut inside_claim = false;
    let mut count = 0;
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("<!-- luna:claim:") {
            inside_claim = true;
        } else if trimmed == "<!-- /luna:claim -->" {
            inside_claim = false;
        } else if !inside_claim && !trimmed.is_empty() && !trimmed.starts_with('#') {
            count += 1;
        }
    }
    count
}

fn marker_id(line: &str, prefix: &str) -> Option<String> {
    let id = line.strip_prefix(prefix)?.strip_suffix(" -->")?;
    Uuid::parse_str(id).ok().map(|parsed| parsed.to_string())
}

fn validate_claims(body: &str, knowledge: &KnowledgeMeta) -> Result<(), KnowledgeError> {
    let mut seen = BTreeSet::new();
    let mut referenced = BTreeSet::new();
    let mut open: Option<(String, String)> = None;
    for line in body.lines() {
        if let Some(id) = marker_id(line, "<!-- luna:claim:") {
            if open.is_some() || !seen.insert(id.clone()) || !knowledge.claims.contains_key(&id) {
                return Err(KnowledgeError::InvalidClaims);
            }
            open = Some((id, String::new()));
        } else if line == "<!-- /luna:claim -->" {
            let (id, text) = open.take().ok_or(KnowledgeError::InvalidClaims)?;
            let line = text.trim();
            if !line.starts_with("- ") || line.lines().count() != 1 {
                return Err(KnowledgeError::InvalidClaims);
            }
            let links = extract_link_occurrences(line);
            if links.is_empty() {
                return Err(KnowledgeError::InvalidClaims);
            }
            for (ordinal, _) in links.iter().enumerate() {
                let key = format!("{id}:{ordinal}");
                let citation = knowledge
                    .citations
                    .get(&key)
                    .ok_or(KnowledgeError::InvalidClaims)?;
                if citation.source_hash.trim().is_empty() || !citation.locator.valid() {
                    return Err(KnowledgeError::InvalidClaims);
                }
                referenced.insert(key);
            }
        } else if let Some((_, text)) = open.as_mut() {
            if !text.is_empty() && !line.is_empty() {
                return Err(KnowledgeError::InvalidClaims);
            }
            text.push_str(line);
        } else if line.starts_with("<!-- luna:claim:") || line.starts_with("<!-- /luna:claim") {
            // 拼错的边界不能退化为用户普通 Markdown 后继续自动写入。
            return Err(KnowledgeError::InvalidClaims);
        }
    }
    if open.is_some()
        || seen.len() != knowledge.claims.len()
        || referenced.len() != knowledge.citations.len()
    {
        return Err(KnowledgeError::InvalidClaims);
    }
    // 分歧关系只能由分歧声明指向本页另一条声明；指空或指自己都不是可信的并列。
    for (id, claim) in &knowledge.claims {
        if let Some(peer) = &claim.conflicts_with {
            if claim.kind != ClaimKind::Conflict
                || peer == id
                || !knowledge.claims.contains_key(peer)
            {
                return Err(KnowledgeError::InvalidClaims);
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvidenceReason {
    Eligible,
    NotMarkdown,
    Excluded,
    Aggregate,
    Draft,
    IncompleteSource,
    SourceNeedsAuthorization,
    ChatArchive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EvidenceEligibility {
    pub searchable: bool,
    pub suggestible: bool,
    pub auto_applicable: bool,
    pub reason: EvidenceReason,
}

/// 新证据路径的来源范围（PRD-16-047/048/057）：正式笔记恒在；已完成权威转写的录音/转写
/// 只有在本设备对本库授权后才进入；速记是用户之言，以「速记记载」身份进入，不需授权
/// （ADR-136 ①）——被引用不等于转正，它仍是待整理内容。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct SourcePolicy {
    pub transcripts: bool,
    pub quick_notes: bool,
}

impl SourcePolicy {
    pub const FORMAL_ONLY: Self = Self {
        transcripts: false,
        quick_notes: false,
    };
}

/// 只接受已在 Vault 中判定过的类型与成熟度；检索可见不代表可自动写主题。
///
/// 待整理内容里**只有速记**可凭 `policy.quick_notes` 成为证据（ADR-136 ①）；普通笔记标成
/// 待整理是用户明说「还没定稿」，仍只有完成整理后才参与（PRD-16-047）。
#[must_use]
pub fn evidence_eligibility(
    is_markdown: bool,
    note_type: NoteType,
    stage: NoteStage,
    is_aggregate: bool,
    has_authoritative_text: bool,
    policy: SourcePolicy,
    excluded_from_topic: bool,
) -> EvidenceEligibility {
    let searchable = is_markdown;
    let reason = if !is_markdown {
        EvidenceReason::NotMarkdown
    } else if excluded_from_topic {
        EvidenceReason::Excluded
    } else if is_aggregate {
        EvidenceReason::Aggregate
    } else if note_type == NoteType::Chat {
        EvidenceReason::ChatArchive
    } else if stage == NoteStage::Draft && !(note_type == NoteType::QuickNote && policy.quick_notes)
    {
        EvidenceReason::Draft
    } else if !has_authoritative_text {
        EvidenceReason::IncompleteSource
    } else if stage == NoteStage::SourceMaterial && !policy.transcripts {
        EvidenceReason::SourceNeedsAuthorization
    } else {
        EvidenceReason::Eligible
    };
    EvidenceEligibility {
        searchable,
        suggestible: reason == EvidenceReason::Eligible,
        auto_applicable: reason == EvidenceReason::Eligible,
        reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontmatter::{update_luna, LunaMeta};

    const ID: &str = "019d1a08-7cf3-72d7-90fa-382ef03524ad";

    /// 「写到名字」候选的名字：标签路径拼接与三字以上的末段；两字词与带分隔的生成标题不用。
    #[test]
    fn mention_names_skip_short_words_and_generated_separators() {
        assert_eq!(
            TopicScope::from_tag("项目/甲").mention_names(),
            vec!["项目甲"]
        );
        assert_eq!(
            TopicScope::from_tag("流程/发布").mention_names(),
            vec!["流程发布"]
        );
        assert!(TopicScope::from_tag("会议").mention_names().is_empty());
        let named = TopicScope {
            title: "Lunarca 桌面端".into(),
            description: String::new(),
            includes: vec!["产品/桌面端".into(), "Lunarca".into()],
            excludes: Vec::new(),
        };
        assert_eq!(
            named.mention_names(),
            vec![
                "Lunarca 桌面端",
                "Lunarca桌面端",
                "产品桌面端",
                "桌面端",
                "Lunarca"
            ]
        );
    }

    #[test]
    fn scoped_claim_round_trips_and_invalid_evidence_fails_closed() {
        let mut meta = LunaMeta {
            topic: true,
            ..LunaMeta::default()
        };
        let mut knowledge = KnowledgeMeta::new(TopicScope::from_tag("项目/I2X"));
        knowledge.claims.insert(
            ID.into(),
            ClaimMeta {
                kind: ClaimKind::Synthesis,
                conflicts_with: None,
            },
        );
        knowledge.citations.insert(
            format!("{ID}:0"),
            CitationMeta {
                source_hash: "sha256:known".into(),
                locator: EvidenceLocator {
                    heading_path: vec!["进展".into()],
                    quote: "已完成原型".into(),
                    context: String::new(),
                    line_hint: Some(8),
                    time_start_ms: None,
                    time_end_ms: None,
                },
                complete_input: true,
            },
        );
        meta.knowledge = Some(serde_yaml_ng::to_value(&knowledge).unwrap());
        let body = format!(
            "# 项目 · I2X\n\n<!-- luna:claim:{ID} -->\n- 已完成原型（[会议](../Notes/会议.md)）\n<!-- /luna:claim -->\n"
        );
        let doc = update_luna(&body, &meta).unwrap();
        assert_eq!(validate_topic_document(&doc).unwrap(), knowledge);
        assert_eq!(
            validate_topic_document(&doc.replace("[会议](../Notes/会议.md)", "会议")),
            Err(KnowledgeError::InvalidClaims)
        );
        assert_eq!(
            validate_topic_document(&doc.replace("<!-- /luna:claim -->", "")),
            Err(KnowledgeError::InvalidClaims)
        );
        // 分歧关系只能由分歧声明指向本页另一条声明（正向用例见 knowledge_plan 的增量写入）。
        let with_peer = |kind: ClaimKind, peer: &str| {
            let mut changed = knowledge.clone();
            let claim = changed.claims.get_mut(ID).unwrap();
            claim.kind = kind;
            claim.conflicts_with = Some(peer.into());
            let mut meta = meta.clone();
            meta.knowledge = Some(serde_yaml_ng::to_value(&changed).unwrap());
            validate_topic_document(&update_luna(&body, &meta).unwrap())
        };
        for (kind, peer) in [
            (ClaimKind::Conflict, ID),
            (ClaimKind::Conflict, "019d1a08-0000-7000-8000-000000000000"),
            (ClaimKind::Synthesis, "019d1a08-0000-7000-8000-000000000000"),
        ] {
            assert_eq!(with_peer(kind, peer), Err(KnowledgeError::InvalidClaims));
        }
    }

    #[test]
    fn repeated_sources_in_one_claim_get_numbered_labels() {
        let locator = |quote: &str, line: u32| EvidenceLocator {
            heading_path: vec![],
            quote: quote.into(),
            context: String::new(),
            line_hint: Some(line),
            time_start_ms: None,
            time_end_ms: None,
        };
        let source =
            |path: &'static str, title: &'static str, quote: &str, line: u32| ClaimSource {
                path,
                title,
                source_hash: "sha256:a",
                locator: locator(quote, line),
                complete_input: true,
            };
        let sources = [
            source("Notes/反馈.md", "反馈", "希望支持导入", 3),
            source("Notes/周会.md", "周会", "已排期", 4),
            source("Notes/反馈.md", "反馈", "十二月前上线", 5),
        ];
        let id = Uuid::parse_str(ID).unwrap();
        let (body, knowledge) = render_scoped_topic(
            "Knowledge/项目.md",
            TopicScope::from_tag("项目"),
            &[NewClaim {
                id,
                text: "客户希望十二月前能导入",
                kind: ClaimKind::Synthesis,
                sources: &sources,
            }],
        )
        .unwrap();
        assert!(
            body.contains(
                "[反馈·1](../Notes/反馈.md)；[周会](../Notes/周会.md)；[反馈·2](../Notes/反馈.md)"
            ),
            "{body}"
        );
        let meta = LunaMeta {
            topic: true,
            knowledge: Some(serde_yaml_ng::to_value(&knowledge).unwrap()),
            ..LunaMeta::default()
        };
        let doc = update_luna(&body, &meta).unwrap();
        assert_eq!(
            topic_claim_statements(&doc).unwrap()[0].text,
            "客户希望十二月前能导入"
        );
        let citations = topic_citations("Knowledge/项目.md", &doc).unwrap();
        assert_eq!(citations.len(), 3);
        assert_eq!(citations[2].meta.locator.quote, "十二月前上线");
    }

    #[test]
    fn claim_statements_recover_plain_text_without_links() {
        let locator = EvidenceLocator {
            heading_path: vec![],
            quote: "预算".into(),
            context: String::new(),
            line_hint: Some(3),
            time_start_ms: None,
            time_end_ms: None,
        };
        let sources = [
            ClaimSource {
                path: "Notes/会议.md",
                title: "会议 [周一]",
                source_hash: "sha256:a",
                locator: locator.clone(),
                complete_input: true,
            },
            ClaimSource {
                path: "Notes/复盘.md",
                title: "复盘",
                source_hash: "sha256:b",
                locator,
                complete_input: false,
            },
        ];
        let id = Uuid::parse_str(ID).unwrap();
        let text = "预算（含 A*B_C）已批准 [待定]";
        let (body, knowledge) = render_scoped_topic(
            "Knowledge/项目.md",
            TopicScope::from_tag("项目"),
            &[NewClaim {
                id,
                text,
                kind: ClaimKind::Conflict,
                sources: &sources,
            }],
        )
        .unwrap();
        let meta = LunaMeta {
            topic: true,
            knowledge: Some(serde_yaml_ng::to_value(&knowledge).unwrap()),
            ..LunaMeta::default()
        };
        let doc = update_luna(&body, &meta).unwrap();
        assert_eq!(
            topic_claim_statements(&doc).unwrap(),
            vec![TopicClaimStatement {
                id,
                text: text.to_owned(),
                kind: ClaimKind::Conflict,
            }]
        );
        assert!(topic_claim_statements(&doc.replace("（[", "([")).is_err());
    }

    #[test]
    fn link_occurrence_maps_to_the_right_citation_and_ignores_plain_lines() {
        let locator = |quote: &str| EvidenceLocator {
            heading_path: vec![],
            quote: quote.into(),
            context: String::new(),
            line_hint: Some(3),
            time_start_ms: None,
            time_end_ms: None,
        };
        let first = [ClaimSource {
            path: "Notes/周会.md",
            title: "周会",
            source_hash: "sha256:a",
            locator: locator("发布推迟"),
            complete_input: true,
        }];
        let second = [ClaimSource {
            path: "Notes/周会.md",
            title: "周会",
            source_hash: "sha256:a",
            locator: locator("体积降到"),
            complete_input: true,
        }];
        let a = Uuid::now_v7();
        let b = Uuid::now_v7();
        let (body, knowledge) = render_scoped_topic(
            "Knowledge/项目.md",
            TopicScope::from_tag("项目"),
            &[
                NewClaim {
                    id: a,
                    text: "发布推迟",
                    kind: ClaimKind::Synthesis,
                    sources: &first,
                },
                NewClaim {
                    id: b,
                    text: "体积降低",
                    kind: ClaimKind::Synthesis,
                    sources: &second,
                },
            ],
        )
        .unwrap();
        // 块外再放一行指向同一来源的人工链接，排在两个声明之前。
        let body = body.replacen("\n\n", "\n\n- 人工备注（[周会](../Notes/周会.md)）\n\n", 1);
        let meta = LunaMeta {
            topic: true,
            knowledge: Some(serde_yaml_ng::to_value(&knowledge).unwrap()),
            ..LunaMeta::default()
        };
        let doc = update_luna(&body, &meta).unwrap();
        let href = "../Notes/周会.md";
        assert_eq!(
            topic_citation_at_link("Knowledge/项目.md", &doc, href, 0).unwrap(),
            None,
            "块外的人工链接不冒充出处"
        );
        let hit = topic_citation_at_link("Knowledge/项目.md", &doc, href, 2)
            .unwrap()
            .unwrap();
        assert_eq!(hit.id, format!("{b}:0"));
        assert_eq!(hit.meta.locator.quote, "体积降到");
        assert_eq!(hit.source_path.as_deref(), Some("Notes/周会.md"));
        assert_eq!(
            topic_citation_at_link("Knowledge/项目.md", &doc, href, 3).unwrap(),
            None
        );
    }

    #[test]
    fn source_qualification_keeps_search_separate_from_topic_writes() {
        let draft = evidence_eligibility(
            true,
            NoteType::QuickNote,
            NoteStage::Draft,
            false,
            true,
            SourcePolicy {
                transcripts: true,
                quick_notes: false,
            },
            false,
        );
        assert!(draft.searchable);
        assert!(!draft.auto_applicable);
        assert_eq!(draft.reason, EvidenceReason::Draft);
        let transcript = evidence_eligibility(
            true,
            NoteType::Transcript,
            NoteStage::SourceMaterial,
            false,
            true,
            SourcePolicy {
                transcripts: false,
                quick_notes: false,
            },
            false,
        );
        assert_eq!(transcript.reason, EvidenceReason::SourceNeedsAuthorization);
        let authorized = evidence_eligibility(
            true,
            NoteType::Transcript,
            NoteStage::SourceMaterial,
            false,
            true,
            SourcePolicy {
                transcripts: true,
                quick_notes: false,
            },
            false,
        );
        assert!(authorized.auto_applicable);
        let partial = evidence_eligibility(
            true,
            NoteType::Recording,
            NoteStage::SourceMaterial,
            false,
            false,
            SourcePolicy {
                transcripts: true,
                quick_notes: false,
            },
            false,
        );
        assert_eq!(partial.reason, EvidenceReason::IncompleteSource);
        let empty_note = evidence_eligibility(
            true,
            NoteType::Note,
            NoteStage::Knowledge,
            false,
            false,
            SourcePolicy {
                transcripts: true,
                quick_notes: false,
            },
            false,
        );
        assert!(empty_note.searchable);
        assert!(!empty_note.auto_applicable);
        // 速记：凭来源范围以「速记记载」进入；普通待整理笔记仍挡在外面。
        let quick = SourcePolicy {
            transcripts: false,
            quick_notes: true,
        };
        let quick_note = evidence_eligibility(
            true,
            NoteType::QuickNote,
            NoteStage::Draft,
            false,
            true,
            quick,
            false,
        );
        assert_eq!(quick_note.reason, EvidenceReason::Eligible);
        let empty_quick_note = evidence_eligibility(
            true,
            NoteType::QuickNote,
            NoteStage::Draft,
            false,
            false,
            quick,
            false,
        );
        assert_eq!(empty_quick_note.reason, EvidenceReason::IncompleteSource);
        let draft_note = evidence_eligibility(
            true,
            NoteType::Note,
            NoteStage::Draft,
            false,
            true,
            quick,
            false,
        );
        assert_eq!(draft_note.reason, EvidenceReason::Draft);
        let excluded_quick_note = evidence_eligibility(
            true,
            NoteType::QuickNote,
            NoteStage::Draft,
            false,
            true,
            quick,
            true,
        );
        assert_eq!(excluded_quick_note.reason, EvidenceReason::Excluded);
    }

    #[test]
    fn writer_links_only_rust_supplied_sources() {
        let id = Uuid::parse_str(ID).unwrap();
        let source = ClaimSource {
            path: "Notes/会议 2026.md",
            title: "会议 [原文]",
            source_hash: "sha256:known",
            locator: EvidenceLocator {
                heading_path: vec![],
                quote: "已完成原型".into(),
                context: String::new(),
                line_hint: Some(8),
                time_start_ms: None,
                time_end_ms: None,
            },
            complete_input: true,
        };
        let (block, kind, citations) = render_generated_claim(
            id,
            "完成原型 [冒充来源](https://example.com)",
            ClaimKind::Synthesis,
            "Knowledge/项目.md",
            &[source],
        )
        .unwrap();
        assert_eq!(kind.kind, ClaimKind::Synthesis);
        assert_eq!(citations.len(), 1);
        assert!(block.contains("[会议 \\[原文\\]](../Notes/会议%202026.md)"));
        assert_eq!(extract_link_occurrences(&block).len(), 1);
    }

    #[test]
    fn scoped_writer_rejects_duplicate_claims_and_empty_topics() {
        let id = Uuid::parse_str(ID).unwrap();
        let sources = [ClaimSource {
            path: "Notes/会议.md",
            title: "会议",
            source_hash: "source-v1",
            locator: EvidenceLocator {
                heading_path: vec![],
                quote: "已完成原型".into(),
                context: String::new(),
                line_hint: None,
                time_start_ms: None,
                time_end_ms: None,
            },
            complete_input: true,
        }];
        let claim = || NewClaim {
            id,
            text: "已完成原型",
            kind: ClaimKind::Synthesis,
            sources: &sources,
        };
        let scope = TopicScope::from_tag("项目");
        assert_eq!(
            render_scoped_topic("Knowledge/项目.md", scope.clone(), &[]),
            Err(KnowledgeError::InvalidClaims)
        );
        assert_eq!(
            render_scoped_topic("Knowledge/项目.md", scope, &[claim(), claim()]),
            Err(KnowledgeError::InvalidClaims)
        );
    }

    #[test]
    fn invalid_nested_metadata_keeps_topic_identity_readable() {
        let doc = "---\nluna:\n  schema: 1\n  type: note\n  topic: true\n  knowledge:\n    schema: future\n---\n# 原文\n";
        assert!(read_luna(doc).topic);
        assert_eq!(
            validate_topic_document(doc),
            Err(KnowledgeError::InvalidMetadata)
        );
    }

    #[test]
    fn repeated_quote_needs_a_matching_line_hint() {
        let content = "# 记录\n\n同一句话。\n\n同一句话。\n";
        let mut locator = EvidenceLocator {
            heading_path: vec!["记录".into()],
            quote: "同一句话".into(),
            context: String::new(),
            line_hint: None,
            time_start_ms: None,
            time_end_ms: None,
        };
        assert!(!citation_quote_is_unique_or_hinted(content, &locator));
        locator.line_hint = Some(5);
        assert!(citation_quote_is_unique_or_hinted(content, &locator));
        locator.line_hint = Some(4);
        assert!(!citation_quote_is_unique_or_hinted(content, &locator));
    }

    #[test]
    fn evidence_page_caps_items_and_rejects_stale_revision() {
        let dir = tempfile::tempdir().unwrap();
        let vault = crate::Vault::open(dir.path()).unwrap();
        let source = "Notes/来源.md";
        let topic = "Knowledge/项目.md";
        vault.write(source, "# 来源\n\n原型已完成。\n").unwrap();
        let hash = vault.body_fingerprint(source).unwrap();
        let sources = [ClaimSource {
            path: source,
            title: "来源",
            source_hash: &hash,
            locator: EvidenceLocator {
                heading_path: vec!["来源".into()],
                quote: "原型已完成".into(),
                context: String::new(),
                line_hint: Some(3),
                time_start_ms: None,
                time_end_ms: None,
            },
            complete_input: true,
        }];
        let claims = (0..101)
            .map(|_| NewClaim {
                id: Uuid::now_v7(),
                text: "原型已完成",
                kind: ClaimKind::Synthesis,
                sources: &sources,
            })
            .collect::<Vec<_>>();
        let content = vault
            .scoped_topic_content(topic, Some("项目"), TopicScope::from_tag("项目"), &claims)
            .unwrap();
        vault.publish_scoped_topic(topic, &content).unwrap();

        let first = vault.topic_evidence_page(topic, 0, u32::MAX, None).unwrap();
        assert_eq!(first.total, 101);
        assert_eq!(first.items.len(), 100);
        assert_eq!(first.next_offset, Some(100));
        let last = vault
            .topic_evidence_page(topic, 100, 100, Some(&first.revision))
            .unwrap();
        assert_eq!(last.items.len(), 1);
        assert_eq!(last.next_offset, None);
        vault.write(topic, &format!("{content}\n")).unwrap();
        assert!(vault
            .topic_evidence_page(topic, 100, 100, Some(&first.revision))
            .is_err());
    }
}
