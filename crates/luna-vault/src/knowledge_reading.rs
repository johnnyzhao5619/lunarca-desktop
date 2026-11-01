//! 主题阅读页头的只读汇总（ADR-134 / TDD-07 §4.1e）。
//!
//! 阅读顺序是「范围 → 摘要 → 声明与出处」，分歧与待核对只在有内容时出现。这些判断
//! （哪条声明是分歧、哪条出处的来源已变或已不在）全部从主题 Markdown 与此刻的来源文件
//! 派生，一个字节都不落盘；前端只渲染。

use crate::knowledge_evidence::{
    topic_citations, topic_claim_statements, validate_topic_document, ClaimKind, EvidenceState,
    TopicScope,
};
use crate::{Result, Vault, VaultError};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const READING_META: &str = "knowledge_reading";

/// 最近打开的一页知识产物；只存路径。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct KnowledgeVisit {
    path: String,
}

/// 页头逐项列出的需关注来源上限；其余只计数，不让一页坏出处把页头撑成清单。
pub const MAX_ATTENTION_SOURCES: usize = 20;

/// 各状态的出处条数。同一声明引用同一来源两次仍是两条出处。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EvidenceTally {
    pub current: u32,
    pub source_changed: u32,
    pub missing: u32,
    pub ambiguous: u32,
    pub partial: u32,
}

/// 一个来源在一种非当前状态下影响了几条声明。`source_path` 为空 = 出处链接无法解析。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceAttention {
    pub source_path: Option<String>,
    pub state: EvidenceState,
    pub claims: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopicReadingSummary {
    pub scope: TopicScope,
    pub claims: u32,
    pub conflicts: u32,
    pub evidence: EvidenceTally,
    /// 按「已不存在 → 来源已变 → 引文不唯一 → 部分输入」、再按路径排序，至多
    /// [`MAX_ATTENTION_SOURCES`] 组。
    pub attention: Vec<SourceAttention>,
    pub attention_omitted: u32,
    /// 至少有一条出处不是「当前」的声明数（去重）；「只看需核对」的计数。
    pub attention_claims: u32,
    /// 声明块之外、没有逐条出处的内容行数（升级前的旧内容或人工补写）：页头标「未核验」。
    pub unverified_lines: u32,
    /// 「转写记载」声明数。
    pub transcript_records: u32,
}

/// 阅读视图的声明筛选（TDD-07 §4.1e「分歧/待更新可筛选」）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaimFilter {
    /// 标为分歧的声明。
    Conflicts,
    /// 至少有一条出处不是「当前」的声明。
    NeedsCheck,
    /// 「转写记载」声明：依据是录音里说过的话（PRD-16-047）。
    TranscriptRecords,
    /// 站得住的结论：不是分歧，且每条出处都是「当前」——知识网络预览卡只摆这类（PRD-16-062）。
    Settled,
}

/// 筛选结果里的一条出处。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaimCitationView {
    pub citation_id: String,
    pub source_path: Option<String>,
    pub state: EvidenceState,
    /// 生成时记录的原文引句（定位锚里的那一句）；来源已改时它是当初的版本。
    pub quote: String,
    /// 来源笔记「录于何时」（`luna.created`，与列表「按录于时间」同一口径）。没记就是
    /// `None`——不拿修改时间冒充：改个标签就会动它，却与这句话何时说的无关。
    pub recorded_ms: Option<i64>,
}

/// 分歧声明所并列的那条既有声明：两边各带出处与时间并排给读者，不判定谁对。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConflictPeerView {
    pub id: String,
    pub text: String,
    pub citations: Vec<ClaimCitationView>,
}

/// 筛选结果里的一条声明（文本即页面上那句，已去掉行内来源链接）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopicClaimView {
    pub id: String,
    pub text: String,
    pub kind: ClaimKind,
    pub citations: Vec<ClaimCitationView>,
    /// 只在分歧声明上：它与哪条并列（`ClaimMeta.conflicts_with`）。
    pub conflicts_with: Option<ConflictPeerView>,
}

/// 一次筛选至多返回的声明数；其余只计数。
pub const MAX_FILTERED_CLAIMS: usize = 200;

/// 一页主题在一段时间内新写入的结论（PRD-16-067「近 7 天」）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopicClaimsAdded {
    pub path: String,
    pub claims: u32,
    /// 其中标为分歧的条数。
    pub conflicts: u32,
}

/// 声明 ID（UUIDv7）里编着的生成时刻（Unix 毫秒）；不是 v7 的 ID 返回 `None`。
fn claim_created_ms(id: &str) -> Option<u64> {
    let (secs, nanos) = uuid::Uuid::parse_str(id).ok()?.get_timestamp()?.to_unix();
    Some(secs.saturating_mul(1000) + u64::from(nanos / 1_000_000))
}

fn severity(state: EvidenceState) -> u8 {
    match state {
        EvidenceState::Missing => 0,
        EvidenceState::SourceChanged => 1,
        EvidenceState::Ambiguous => 2,
        EvidenceState::Partial => 3,
        EvidenceState::Current => 4,
    }
}

fn count(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

impl Vault {
    fn is_knowledge_product(&self, rel: &str) -> bool {
        self.read(rel)
            .is_ok_and(|doc| doc.meta.topic || doc.meta.knowledge_map)
    }

    /// 阅读视图打开了一页知识产物：记为工作台「继续阅读」的目标。非产物路径静默忽略。
    pub fn record_knowledge_visit(&self, rel: &str) -> Result<()> {
        if !self.is_knowledge_product(rel) {
            return Ok(());
        }
        crate::meta::write_meta(
            self.root(),
            READING_META,
            &[KnowledgeVisit {
                path: rel.to_owned(),
            }],
        )
    }

    /// 「继续阅读」：最近打开且此刻仍是知识产物的那一页；被删、被移走或已转为普通笔记时为空。
    /// 记录读不出（未知版本、损坏）同样为空——这只是一个便捷入口，不值得报错。
    #[must_use]
    pub fn last_knowledge_visit(&self) -> Option<String> {
        let visits: Vec<KnowledgeVisit> = crate::meta::read_meta(self.root(), READING_META).ok()?;
        let path = visits.into_iter().next()?.path;
        self.is_knowledge_product(&path).then_some(path)
    }

    /// 某时刻以来各主题页新写入的结论（PRD-16-067）：证据格式页上此刻仍在的声明里，ID
    /// （UUIDv7）的生成时刻不早于 `since_ms` 的那些——即这段时间里整理出、此刻仍在页上的结论；
    /// 替换一条旧说法也会生成新 ID，同样算。不看快照、不看修改时间（人工改正文不算整理），
    /// 旧格式页没有声明 ID，不计。只列有新结论的页，按条数降序、同数按路径。
    pub fn topic_claims_added_since(&self, since_ms: u64) -> Result<Vec<TopicClaimsAdded>> {
        let mut out = Vec::new();
        for path in self.topic_pages()? {
            let Ok(doc) = self.read(&path) else {
                continue;
            };
            let Ok(knowledge) = validate_topic_document(&doc.content) else {
                continue;
            };
            let mut claims = 0u32;
            let mut conflicts = 0u32;
            for (id, meta) in &knowledge.claims {
                if claim_created_ms(id).is_some_and(|at| at >= since_ms) {
                    claims += 1;
                    if meta.kind == ClaimKind::Conflict {
                        conflicts += 1;
                    }
                }
            }
            if claims > 0 {
                out.push(TopicClaimsAdded {
                    path,
                    claims,
                    conflicts,
                });
            }
        }
        out.sort_by(|a, b| b.claims.cmp(&a.claims).then_with(|| a.path.cmp(&b.path)));
        Ok(out)
    }

    /// 新格式主题的阅读汇总；旧格式、未知 schema 与坏证据返回格式错误，由调用方呈现
    /// 为「没有汇总」而不是「零条声明」。
    pub fn topic_reading_summary(&self, topic_rel: &str) -> Result<TopicReadingSummary> {
        let topic = self.read(topic_rel)?;
        let invalid = |error: crate::KnowledgeError| VaultError::InvalidReplace(error.to_string());
        let knowledge = validate_topic_document(&topic.content).map_err(invalid)?;
        let citations = topic_citations(topic_rel, &topic.content).map_err(invalid)?;
        let states = self.evidence_states_of(topic_rel, &topic.content)?;

        let mut evidence = EvidenceTally::default();
        let mut groups: BTreeMap<(u8, Option<String>), (EvidenceState, BTreeSet<String>)> =
            BTreeMap::new();
        for (citation, state) in citations.iter().zip(&states) {
            let slot = match state.state {
                EvidenceState::Current => &mut evidence.current,
                EvidenceState::SourceChanged => &mut evidence.source_changed,
                EvidenceState::Missing => &mut evidence.missing,
                EvidenceState::Ambiguous => &mut evidence.ambiguous,
                EvidenceState::Partial => &mut evidence.partial,
            };
            *slot = slot.saturating_add(1);
            if state.state == EvidenceState::Current {
                continue;
            }
            let claim = citation
                .id
                .split_once(':')
                .map_or(citation.id.as_str(), |(claim, _)| claim)
                .to_owned();
            groups
                .entry((severity(state.state), state.source_path.clone()))
                .or_insert_with(|| (state.state, BTreeSet::new()))
                .1
                .insert(claim);
        }
        let attention_claims = count(
            groups
                .values()
                .flat_map(|(_, claims)| claims.iter())
                .collect::<BTreeSet<_>>()
                .len(),
        );
        let total_groups = groups.len();
        let attention = groups
            .into_iter()
            .take(MAX_ATTENTION_SOURCES)
            .map(|((_, source_path), (state, claims))| SourceAttention {
                source_path,
                state,
                claims: count(claims.len()),
            })
            .collect();
        Ok(TopicReadingSummary {
            scope: knowledge.scope,
            claims: count(knowledge.claims.len()),
            conflicts: count(
                knowledge
                    .claims
                    .values()
                    .filter(|claim| claim.kind == ClaimKind::Conflict)
                    .count(),
            ),
            evidence,
            attention,
            attention_omitted: count(total_groups.saturating_sub(MAX_ATTENTION_SOURCES)),
            attention_claims,
            transcript_records: count(
                knowledge
                    .claims
                    .values()
                    .filter(|claim| claim.kind == ClaimKind::TranscriptRecord)
                    .count(),
            ),
            unverified_lines: count(crate::knowledge_evidence::unverified_content_lines(
                crate::frontmatter::body_of(&topic.content),
            )),
        })
    }

    /// 按筛选列出声明及其出处状态，按页面顺序；第二个值是超出上限未返回的条数。
    /// 旧格式、未知 schema 与坏证据返回格式错误（调用方呈现为「没有可筛选的声明」）。
    pub fn topic_claims_filtered(
        &self,
        topic_rel: &str,
        filter: ClaimFilter,
    ) -> Result<(Vec<TopicClaimView>, u32)> {
        let topic = self.read(topic_rel)?;
        let invalid = |error: crate::KnowledgeError| VaultError::InvalidReplace(error.to_string());
        let knowledge = crate::validate_topic_document(&topic.content).map_err(invalid)?;
        let statements = topic_claim_statements(&topic.content).map_err(invalid)?;
        let citations = topic_citations(topic_rel, &topic.content).map_err(invalid)?;
        let states = self.evidence_states_of(topic_rel, &topic.content)?;
        let mut recorded: BTreeMap<String, Option<i64>> = BTreeMap::new();
        let mut by_claim: BTreeMap<String, Vec<ClaimCitationView>> = BTreeMap::new();
        for (citation, state) in citations.iter().zip(&states) {
            let recorded_ms = state.source_path.as_ref().and_then(|path| {
                *recorded.entry(path.clone()).or_insert_with(|| {
                    self.read(path)
                        .ok()
                        .and_then(|note| crate::index::created_ms_of(note.meta.created.as_deref()))
                })
            });
            let claim = citation
                .id
                .split_once(':')
                .map_or(citation.id.as_str(), |(claim, _)| claim);
            by_claim
                .entry(claim.to_owned())
                .or_default()
                .push(ClaimCitationView {
                    citation_id: citation.id.clone(),
                    source_path: state.source_path.clone(),
                    state: state.state,
                    quote: citation.meta.locator.quote.clone(),
                    recorded_ms,
                });
        }
        let statements: Vec<_> = statements
            .into_iter()
            .map(|statement| {
                let citations = by_claim
                    .remove(&statement.id.to_string())
                    .unwrap_or_default();
                (statement, citations)
            })
            .collect();
        let peer_of = |id: &str| {
            let peer = knowledge.claims.get(id)?.conflicts_with.as_deref()?;
            statements
                .iter()
                .find(|(statement, _)| statement.id.to_string() == peer)
                .map(|(statement, citations)| ConflictPeerView {
                    id: peer.to_owned(),
                    text: statement.text.clone(),
                    citations: citations.clone(),
                })
        };
        let mut matched = Vec::new();
        for (statement, citations) in &statements {
            let id = statement.id.to_string();
            let keep = match filter {
                ClaimFilter::Conflicts => statement.kind == ClaimKind::Conflict,
                ClaimFilter::NeedsCheck => citations
                    .iter()
                    .any(|citation| citation.state != EvidenceState::Current),
                ClaimFilter::TranscriptRecords => statement.kind == ClaimKind::TranscriptRecord,
                ClaimFilter::Settled => {
                    statement.kind != ClaimKind::Conflict
                        && !citations.is_empty()
                        && citations
                            .iter()
                            .all(|citation| citation.state == EvidenceState::Current)
                }
            };
            if keep {
                matched.push(TopicClaimView {
                    conflicts_with: peer_of(&id),
                    id,
                    text: statement.text.clone(),
                    kind: statement.kind,
                    citations: citations.clone(),
                });
            }
        }
        let omitted = count(matched.len().saturating_sub(MAX_FILTERED_CLAIMS));
        matched.truncate(MAX_FILTERED_CLAIMS);
        Ok((matched, omitted))
    }

    /// 按出处 ID 取回一条出处（筛选清单里点出处时用）；不在此页或格式无效时为 `None`。
    pub fn topic_citation_by_id(
        &self,
        topic_rel: &str,
        citation_id: &str,
    ) -> Result<Option<crate::knowledge_evidence::TopicCitation>> {
        let topic = self.read(topic_rel)?;
        Ok(topic_citations(topic_rel, &topic.content)
            .ok()
            .and_then(|citations| citations.into_iter().find(|c| c.id == citation_id)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge_evidence::{ClaimSource, EvidenceLocator, NewClaim};
    use uuid::Uuid;

    const TARGET: &str = "Knowledge/项目.md";

    fn locator(quote: &str, line: u32) -> EvidenceLocator {
        EvidenceLocator {
            heading_path: vec![],
            quote: quote.into(),
            context: String::new(),
            line_hint: Some(line),
            time_start_ms: None,
            time_end_ms: None,
        }
    }

    #[test]
    fn summary_counts_claims_conflicts_and_groups_sources_needing_attention() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        vault
            .write(
                "Notes/周会.md",
                "# 周会\n\n原型已完成。\n发布推迟到下月。\n",
            )
            .unwrap();
        vault
            .write("Notes/站会.md", "# 站会\n\n灰度扩大到五百人。\n")
            .unwrap();
        let weekly = vault.body_fingerprint("Notes/周会.md").unwrap();
        let standup = vault.body_fingerprint("Notes/站会.md").unwrap();
        let from_weekly = |quote: &str, line: u32| ClaimSource {
            path: "Notes/周会.md",
            title: "周会",
            source_hash: &weekly,
            locator: locator(quote, line),
            complete_input: true,
        };
        let a = [from_weekly("原型已完成", 3)];
        let b = [from_weekly("发布推迟到下月", 4)];
        let c = [ClaimSource {
            path: "Notes/站会.md",
            title: "站会",
            source_hash: &standup,
            locator: locator("灰度扩大到五百人", 3),
            complete_input: true,
        }];
        let claims = [
            NewClaim {
                id: Uuid::now_v7(),
                text: "原型已完成",
                kind: ClaimKind::Synthesis,
                sources: &a,
            },
            NewClaim {
                id: Uuid::now_v7(),
                text: "发布推迟到下月",
                kind: ClaimKind::Conflict,
                sources: &b,
            },
            NewClaim {
                id: Uuid::now_v7(),
                text: "灰度扩大",
                kind: ClaimKind::Synthesis,
                sources: &c,
            },
        ];
        let content = vault
            .scoped_topic_content(TARGET, Some("项目"), TopicScope::from_tag("项目"), &claims)
            .unwrap();
        vault.publish_scoped_topic(TARGET, &content).unwrap();

        let fresh = vault.topic_reading_summary(TARGET).unwrap();
        assert_eq!(fresh.scope.includes, vec!["项目".to_owned()]);
        assert_eq!((fresh.claims, fresh.conflicts), (3, 1));
        assert_eq!(fresh.unverified_lines, 0, "系统生成的页只有标题与声明块");
        assert_eq!(fresh.evidence.current, 3);
        assert!(fresh.attention.is_empty());

        // 周会被改、站会被删：两条周会声明归成一组，缺失排在前面。
        vault
            .write("Notes/周会.md", "# 周会\n\n原型已完成。\n发布改到本月。\n")
            .unwrap();
        vault
            .delete("Notes/站会.md", crate::DeleteMode::VaultTrash)
            .unwrap();
        let stale = vault.topic_reading_summary(TARGET).unwrap();
        assert_eq!(
            (stale.evidence.source_changed, stale.evidence.missing),
            (2, 1)
        );
        assert_eq!(
            stale.attention,
            vec![
                SourceAttention {
                    source_path: Some("Notes/站会.md".into()),
                    state: EvidenceState::Missing,
                    claims: 1,
                },
                SourceAttention {
                    source_path: Some("Notes/周会.md".into()),
                    state: EvidenceState::SourceChanged,
                    claims: 2,
                },
            ]
        );
        assert_eq!(stale.attention_omitted, 0);
        assert_eq!(stale.attention_claims, 3);

        // 筛选：分歧只有一条；需核对是三条（两条周会已变 + 一条站会已删），按页面顺序。
        let (conflicts, omitted) = vault
            .topic_claims_filtered(TARGET, ClaimFilter::Conflicts)
            .unwrap();
        assert_eq!(omitted, 0);
        assert_eq!(
            conflicts
                .iter()
                .map(|c| c.text.as_str())
                .collect::<Vec<_>>(),
            vec!["发布推迟到下月"]
        );
        let (check, _) = vault
            .topic_claims_filtered(TARGET, ClaimFilter::NeedsCheck)
            .unwrap();
        assert_eq!(check.len(), 3);
        assert_eq!(check[2].citations[0].state, EvidenceState::Missing);
        // 引句是生成时记录的那一句：来源被改、被删后仍可对照。
        assert_eq!(check[2].citations[0].quote, "灰度扩大到五百人");
        let id = &check[0].citations[0].citation_id;
        let citation = vault.topic_citation_by_id(TARGET, id).unwrap().unwrap();
        assert_eq!(citation.source_path.as_deref(), Some("Notes/周会.md"));
        assert!(vault
            .topic_citation_by_id(TARGET, "nope:0")
            .unwrap()
            .is_none());
    }

    #[test]
    fn conflicts_carry_the_peer_claim_and_when_each_source_was_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        vault
            .write(
                "Notes/旧.md",
                "---\nluna:\n  created: 2026-09-01T09:00:00+08:00\n---\n# 旧\n\n原型已完成。\n",
            )
            .unwrap();
        vault
            .write("Notes/新.md", "# 新\n\n原型尚未完成。\n")
            .unwrap();
        let (old_hash, new_hash) = (
            vault.body_fingerprint("Notes/旧.md").unwrap(),
            vault.body_fingerprint("Notes/新.md").unwrap(),
        );
        let old = [ClaimSource {
            path: "Notes/旧.md",
            title: "旧",
            source_hash: &old_hash,
            locator: locator("原型已完成", 6),
            complete_input: true,
        }];
        let new = [ClaimSource {
            path: "Notes/新.md",
            title: "新",
            source_hash: &new_hash,
            locator: locator("原型尚未完成", 3),
            complete_input: true,
        }];
        let (old_id, new_id) = (Uuid::now_v7(), Uuid::now_v7());
        let claims = [
            NewClaim {
                id: old_id,
                text: "原型已完成",
                kind: ClaimKind::Synthesis,
                sources: &old,
            },
            NewClaim {
                id: new_id,
                text: "原型尚未完成",
                kind: ClaimKind::Conflict,
                sources: &new,
            },
        ];
        let content = vault
            .scoped_topic_content(TARGET, Some("项目"), TopicScope::from_tag("项目"), &claims)
            .unwrap();
        // 增量写入会记下分歧指向谁（见 knowledge_plan 用例）；这里直接写进元数据。
        let mut knowledge = crate::validate_topic_document(&content).unwrap();
        knowledge
            .claims
            .get_mut(&new_id.to_string())
            .unwrap()
            .conflicts_with = Some(old_id.to_string());
        let mut meta = crate::read_luna(&content);
        meta.knowledge = Some(serde_yaml_ng::to_value(&knowledge).unwrap());
        let content = crate::update_luna(&content, &meta).unwrap();
        vault.publish_scoped_topic(TARGET, &content).unwrap();

        let (conflicts, _) = vault
            .topic_claims_filtered(TARGET, ClaimFilter::Conflicts)
            .unwrap();
        assert_eq!(conflicts.len(), 1);
        let conflict = &conflicts[0];
        assert_eq!(conflict.text, "原型尚未完成");
        assert_eq!(
            conflict.citations[0].recorded_ms, None,
            "没记录于何时就不给时间"
        );
        let peer = conflict.conflicts_with.as_ref().expect("并列的原说法");
        assert_eq!(
            (peer.id.as_str(), peer.text.as_str()),
            (old_id.to_string().as_str(), "原型已完成")
        );
        assert_eq!(
            peer.citations[0].source_path.as_deref(),
            Some("Notes/旧.md")
        );
        assert_eq!(peer.citations[0].recorded_ms, Some(1_788_224_400_000));

        // 改来源的标签会动修改时间，但不改「录于何时」。
        vault
            .write(
                "Notes/旧.md",
                "---\ntags: [x]\nluna:\n  created: 2026-09-01T09:00:00+08:00\n---\n# 旧\n\n原型已完成。\n",
            )
            .unwrap();
        let (conflicts, _) = vault
            .topic_claims_filtered(TARGET, ClaimFilter::Conflicts)
            .unwrap();
        let peer = conflicts[0].conflicts_with.as_ref().unwrap();
        assert_eq!(peer.citations[0].recorded_ms, Some(1_788_224_400_000));
    }

    #[test]
    fn claims_added_since_counts_claims_generated_in_the_window_only() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        vault
            .write("Notes/周会.md", "# 周会\n\n原型已完成。\n内测推迟。\n")
            .unwrap();
        let hash = vault.body_fingerprint("Notes/周会.md").unwrap();
        let source = |quote: &str, line: u32| ClaimSource {
            path: "Notes/周会.md",
            title: "周会",
            source_hash: &hash,
            locator: locator(quote, line),
            complete_input: true,
        };
        let (done, delayed) = ([source("原型已完成", 3)], [source("内测推迟", 4)]);
        // 一条「很久以前」生成的结论（v7 时间戳取 2020 年），一条刚生成的分歧。
        let old = Uuid::new_v7(uuid::Timestamp::from_unix(
            uuid::NoContext,
            1_577_836_800,
            0,
        ));
        let claims = [
            NewClaim {
                id: old,
                text: "原型已完成",
                kind: ClaimKind::Synthesis,
                sources: &done,
            },
            NewClaim {
                id: Uuid::now_v7(),
                text: "内测推迟",
                kind: ClaimKind::Conflict,
                sources: &delayed,
            },
        ];
        let content = vault
            .scoped_topic_content(TARGET, Some("项目"), TopicScope::from_tag("项目"), &claims)
            .unwrap();
        vault.publish_scoped_topic(TARGET, &content).unwrap();
        vault
            .write(
                "Knowledge/旧页.md",
                "---\nluna:\n  topic: true\n---\n# 旧页\n",
            )
            .unwrap();

        let since = claim_created_ms(&Uuid::now_v7().to_string()).unwrap() - 60_000;
        assert_eq!(
            vault.topic_claims_added_since(since).unwrap(),
            vec![TopicClaimsAdded {
                path: TARGET.to_owned(),
                claims: 1,
                conflicts: 1,
            }],
            "旧结论不算；旧格式页没有声明 ID"
        );
        assert_eq!(vault.topic_claims_added_since(u64::MAX).unwrap(), vec![]);
    }

    #[test]
    fn last_visit_survives_reopen_and_disappears_with_the_page() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        vault
            .write(TARGET, "---\nluna:\n  topic: true\n---\n# 项目\n")
            .unwrap();
        vault.write("Notes/普通.md", "# 普通\n").unwrap();
        assert_eq!(vault.last_knowledge_visit(), None);
        vault.record_knowledge_visit(TARGET).unwrap();
        // 普通笔记不是知识产物：不覆盖已记下的那一页。
        vault.record_knowledge_visit("Notes/普通.md").unwrap();
        drop(vault);
        let vault = Vault::open(dir.path()).unwrap();
        assert_eq!(vault.last_knowledge_visit().as_deref(), Some(TARGET));
        vault.delete(TARGET, crate::DeleteMode::VaultTrash).unwrap();
        assert_eq!(vault.last_knowledge_visit(), None);
    }

    #[test]
    fn transcript_records_are_counted_and_filterable() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        vault
            .write("Transcripts/周会.md", "# 周会\n\n灰度十月结束。\n")
            .unwrap();
        let hash = vault.body_fingerprint("Transcripts/周会.md").unwrap();
        let sources = [ClaimSource {
            path: "Transcripts/周会.md",
            title: "周会",
            source_hash: &hash,
            locator: locator("灰度十月结束", 3),
            complete_input: true,
        }];
        let claims = [NewClaim {
            id: Uuid::now_v7(),
            text: "灰度十月结束",
            kind: ClaimKind::TranscriptRecord,
            sources: &sources,
        }];
        let content = vault
            .scoped_topic_content(TARGET, Some("项目"), TopicScope::from_tag("项目"), &claims)
            .unwrap();
        vault.publish_scoped_topic(TARGET, &content).unwrap();
        assert_eq!(
            vault
                .topic_reading_summary(TARGET)
                .unwrap()
                .transcript_records,
            1
        );
        let (records, _) = vault
            .topic_claims_filtered(TARGET, ClaimFilter::TranscriptRecords)
            .unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].kind, ClaimKind::TranscriptRecord);
    }

    #[test]
    fn legacy_topic_has_no_summary() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        vault
            .write(
                TARGET,
                "---\nluna:\n  topic: true\n---\n# 项目\n\n旧内容。\n",
            )
            .unwrap();
        assert!(vault.topic_reading_summary(TARGET).is_err());
    }
}
