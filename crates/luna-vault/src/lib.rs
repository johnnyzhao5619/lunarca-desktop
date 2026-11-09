//! `luna-vault` — 本地 Markdown Vault（**本地文件是唯一事实源**，TDD-06）。
//!
//! Vault = 磁盘上一个文件夹，里面是普通 `.md` 与媒体文件。Lunarca 不发明私有数据库格式；
//! SQLite 只是派生索引/缓存（删了能从文件重建）。本 crate 承载：
//!
//! - **数据安全四件套（C1，本提交）**：①原子写（[`atomic`]，§4.3）②frontmatter 局部
//!   round-trip（[`frontmatter`]，§4.4）③自写回声抑制 + 外部编辑判定（[`suppress`]，§4）
//!   ④版本快照（[`snapshot`]，§4.7，内容寻址去重）。均为可 headless 重测的纯逻辑/确定性 IO。
//! - 派生索引/搜索（trigram FTS）、耐久边车、模板渲染器、回收站、`notify` 监听与 `Vault`
//!   trait 生命周期为后续提交（C2–C4）。
//!
//! 依赖方向：仅依赖 `luna-core`（与 engine/audio 同层），不依赖 jobqueue/provider，保持
//! 纯逻辑可独立测试（[`TDD-00`] §3）。
#![forbid(unsafe_code)]

pub mod absence;
pub mod atomic;
pub mod bookmarks;
pub mod channel;
pub mod checkup;
pub mod chunk;
pub mod conflicts;
pub mod creator;
pub mod diskusage;
pub mod duplicate;
pub mod embed;
pub mod embedding_profile;
pub mod error;
pub mod exclusions;
pub mod external_rename;
pub mod file_import;
pub mod frontmatter;
pub mod gfm_table;
pub mod graph;
pub mod html_paste;
pub mod icloud;
pub mod identity;
pub mod index;
pub mod knowledge_draft;
pub mod knowledge_evidence;
pub mod knowledge_generation;
pub mod knowledge_map;
pub mod knowledge_plan;
pub mod knowledge_reading;
pub mod knowledge_upgrade;
pub mod merge;
pub mod meta;
pub mod obsidian;
pub mod open;
pub mod orphans;
pub mod parse;
pub mod pathutil;
pub mod pinyin_keys;
mod presets;
pub mod quote;
pub mod registry;
mod relink;
pub mod research;
pub mod retrieve;
pub mod scan;
pub mod script;
pub mod semantic;
pub mod sidecar;
pub mod snapshot;
pub mod speaking_rate;
pub mod suppress;
pub mod swipe;
pub mod switcher;
pub mod tag_edit;
pub mod template;
pub mod tidy;
pub mod timecode;
pub mod topic;
pub mod uri;
pub mod usage;
pub mod vault;
pub mod watcher;

pub use bookmarks::VaultBookmark;
pub use checkup::{
    claims_of, sort_findings, suppress_superseded, t0_findings, without_ignored, CheckupNote,
    CheckupRows, CheckupState, Claim, Finding, FindingKind, SETTLE_BACKLOG_STALE_MS,
};
pub use chunk::{
    chunk_note, Chunk, EmbeddingChunkPolicy, BGE_M3_CHUNK_POLICY, CHUNK_HARD_MAX_CHARS,
    CHUNK_MIN_CHARS, CHUNK_TARGET_CHARS,
};
pub use conflicts::{ConflictCopy, ConflictKind};
pub use creator::{
    display_column, first_visible_stage, next_action, resolve_publish_date, BoardLayout,
    NextAction, OverviewGroup, OverviewItem, ProjectCard, ProjectChild, ProjectDeletion,
    ProjectLayout, ProjectOverview, ProjectStage, PublishSource, SourceKind, PUBLISH_DATE_FORMAT,
};
pub use embedding_profile::{
    embedding_profile, engine_entry_profile_key, normalize_embedding_vectors, profile_texts,
    EmbeddingProfile, VectorContract, BGE_M3_PROFILE, QWEN3_EMBED_06_PROFILE,
    QWEN3_EMBED_4_PROFILE, QWEN3_EMBED_8_PROFILE, QWEN3_EMBED_QUERY_INSTRUCTION,
};
pub use error::{Result, VaultError};
pub use exclusions::{Exclusions, EXCLUDED_PATHS_META};
pub use external_rename::{pair_external_renames, ExternalRename, ExternalRepair};
pub use file_import::{ImportReport, ImportSkip};
pub use frontmatter::{
    body_of, knowledge_body_hash, read_luna, update_luna, DesktopNote, LunaMeta, MediaRef,
    NoteStage, NoteType, OriginalFile, WebClip, LUNA_SCHEMA, STAGE_DRAFT,
};
pub use graph::{
    build_graph, is_tag_excluded, scope_covers, GraphEdge, GraphEdgeKind, GraphNode, GraphNodeKind,
    VaultGraph,
};
pub use identity::VaultId;
pub use index::{
    DraftFilter, DraftInboxEntry, DraftInboxQuery, EmbeddingIndexIdentity, EmbeddingRow, Index,
    KnowledgeMetricsFacts, ListPage, ListSort, NoteListEntry, Query, RecentSource, SearchHit,
    TagCount, TagNeighbor, TagSummary, TopicCitationDependency, LIST_PAGE_SIZE,
};
pub use knowledge_draft::{DraftClaim, TopicDraft, TopicDraftPublished};
pub use knowledge_evidence::{
    evidence_eligibility, render_generated_claim, render_scoped_topic, topic_citation_at_link,
    topic_claim_statements, validate_topic_document, CitationEvidencePage, CitationMeta, ClaimKind,
    ClaimMeta, ClaimSource, EvidenceEligibility, EvidenceLocator, EvidenceReason, EvidenceState,
    KnowledgeError, KnowledgeMeta, NewClaim, SourcePolicy, TopicClaimStatement,
    TopicMaintenanceState, TopicScope, KNOWLEDGE_SCHEMA,
};
pub use knowledge_generation::{
    has_authoritative_transcript, has_transcript_text, parse_topic_generation, parse_topic_update,
    topic_generation_payload, topic_update_payload, validate_topic_generation_input,
    DiscardedModelId, GroundedClaim, GroundedUpdate, SourceCoverage, TopicEvidenceBatch,
    TopicEvidenceInput, TopicEvidenceKind, TopicGeneratedClaim, TopicGeneration,
    TopicGenerationError, TopicModelOutput, TopicUpdate, TopicUpdateClaim,
};
pub use knowledge_map::{
    first_meaningful_paragraph, group_topics, recent_topics, related_topics, render_knowledge_map,
    topic_groups, topic_source_links, topic_summary, KnowledgeMapTopic, MapLabels, TopicGroup,
};
pub use knowledge_plan::{
    IngestAction, IngestDecision, IngestDecisionKind, IngestPlan, PlanApplyOutcome, PlanBaseline,
    PlanClaim, PlanEvidence, PlanRejection, PlanRejectionCode, PlanRevisions, PlanUndoOutcome,
    TopicUpdateContext,
};
pub use knowledge_reading::{
    ClaimCitationView, ClaimFilter, ConflictPeerView, EvidenceTally, SourceAttention,
    TopicClaimView, TopicClaimsAdded, TopicReadingSummary, MAX_ATTENTION_SOURCES,
    MAX_FILTERED_CLAIMS,
};
pub use knowledge_upgrade::{
    TopicUpgradeOutcome, TopicUpgradePreview, TopicUpgradeRejection, TopicUpgradeRejectionCode,
};
pub use merge::{
    dedup_by_object, merge_block, merge_input, MergeBlock, MergeFingerprint, MergeSource,
    MergeSourceError,
};
pub use meta::{read_meta, write_meta};
pub use obsidian::{AttachmentSetting, ObsidianSuggestion};
pub use open::{ObjectId, VaultFile, VaultOpenError};
pub use orphans::{OrphanAsset, OrphanRemoval};
pub use parse::{
    clean_for_fts, due_state, extract_links, link_spans, todo_stats, DueState, TodoStats,
};
pub use pathutil::{encode_link_target, markdown_link};
pub use pinyin_keys::pinyin_keys;
pub use presets::{parse_pack_preset, AiPreset};
pub use retrieve::{
    allows_grounding, in_scope, pack_passages, question_terms, resolve_anchor, score_note, Anchor,
    AnchorHit, ClaimNotices, Passage, QueryTerms, Retrieval, RetrieveOptions, RetrieveScope,
};
pub use script::{
    broll_shots, insert_retake_marks, move_section, normalize_pasted_script, parse_cues,
    prompter_blocks, relocate_line, retake_notes, script_language, script_outline, script_title_of,
    section_body, set_broll_done, spoken_millis, upsert_section, BrollShot, PrompterBlock,
    PrompterLine, PrompterSpan, RetakeMark, RetakeNote, ScriptOutline, ScriptSection, SpeakingRate,
    CUE_CALLOUT, PAUSE_MILLIS, RETAKE_CALLOUT,
};
pub use semantic::{
    dot, near_duplicates, rrf_fuse, NearDuplicate, SemanticHit, VectorMatrix, VectorRow,
    NEAR_DUPLICATE_THRESHOLD, RRF_K,
};
pub use sidecar::{note_segments, resolve_segment_speakers, speaker_display_name, SpeakerLabel};
pub use sidecar::{
    parse_sidecar, read_sidecar, read_sidecar_for_media, remove_sidecar_file, sidecar_path_for,
    transcript_slots, write_sidecar, Sidecar, SidecarLoad, TranscriptSlot, SIDECAR_SCHEMA,
};
pub use snapshot::{snapshots_root, SnapshotMeta, SnapshotStore, DEFAULT_MAX_VERSIONS};
pub use speaking_rate::{RateSample, SavedSpeakingRate};
pub use suppress::{decide_external_edit, ExternalEditOutcome, WriteGuard};
pub use switcher::SwitcherEntry;
pub use template::{
    list_builtin, render as render_template, render_user as render_user_template,
    BuiltinTemplateInfo, RenderedUserTemplate, TemplateVars, UserTemplateVars,
};
pub use tidy::{tidy_list_levels, ListTidy, ListTidyChange};
pub use timecode::{format_timecode, parse_timecode};
pub use topic::{
    cited_targets, cites_any_of, cites_source, is_ingest_candidate, ownership_of, TopicOwnership,
};
pub use usage::{
    AiOutcome, AiOutcomeSummary, QueueOutcome, UsageRange, UsageStore, UsageSummary,
    QUEUE_OUTCOME_COLUMNS,
};
pub use vault::{
    bookmarks_of, chunk_fingerprint, parse_bookmark_lines, quick_note_products, read_sync_file,
    sticky_text, sync_scope_bytes, sync_scope_files, tag_fits_language, AdoptedVersion, Bookmark,
    CueTextEdit, DeleteMode, DistillScan, DistillState, EmbedScan, GraphTagScan, MentionKind,
    MentionScope, NoteDoc, NoteFacts, NoteSave, NoteTree, PurgeOutcome, QuickNoteProducts,
    RecordingPart, RelatedNotes, RelocationNoteChange, RelocationReceipt, RelocationUndo,
    RestoreConflict, SegmentEdit, TopicSuggestion, TranscriptAuthority, TrashEntry, TrashListEntry,
    TrashPreview, TrashPreviewKind, UnlinkedMention, UserTemplateInfo, Vault, WikiLinkHit,
    QUICK_NOTE_RAW_HEADINGS, QUICK_NOTE_REFINED_HEADINGS, TOPIC_SUGGESTION_MIN_NOTES,
};
pub use watcher::{watch, AssetChange, VaultChange, VaultWatcher};
