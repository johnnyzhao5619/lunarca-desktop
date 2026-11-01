//! 同步回放 · 转写编辑 · 导出 命令域（PRD-04 §3.4 / PRD-11，IPC 层，从 `ipc/mod.rs` 纵切）。
//!
//! 职责边界：**已归档转写产物的读取、就地编辑与导出**——回放聚合包（分片表 + cue +
//! 书签 + 说话人名册）、cue/整篇校对写回、说话人指派与改名、SRT/VTT/ASS/LRC/文本导出，
//! 以及媒体分片的类型判定与可拼接性推导。录音与导入两类产物共用同一时间轴文本模型
//! （PRD-05-022，不为批量另写一套回放）。
//!
//! 事实源：笔记 frontmatter `luna.media[]` + 耐久边车（TDD-06 §2.1）；写回单一入口
//! `Vault::rewrite_recording_transcript`。
//!
//! 跨模块可见性：命令与本域 DTO `pub(crate)`；共享 helper 经 `use super::*` 从 `mod.rs` 取。

use super::*;

/// 加载同步回放聚合包（TDD-06 §2.1 / PRD-04 §3.4）。
///
/// 从笔记 frontmatter `luna.media[]` 取真实分片表（替代任何前端路径推导），
/// 合并各分片耐久边车为统一 cue 列表（会话绝对时钟升序），并注册媒体服务器
/// 虚拟拼接资源——多分片对前端呈现为单一连续 WAV（PRD-04 §5「一条连续录音」）。
///
/// `media_kind` 告知前端本笔记该以**音频条**还是**视频舞台**呈现（PRD-04-058）：
/// 导入的视频以原容器登记为唯一分片（画面与音轨同源），故视频恒走单分片直链。
///
/// 降级链：边车缺失/损坏 → 该分片 0 cue；拼接注册失败 → 回退首分片直链。
///
/// # Errors
/// Vault 未打开或笔记不存在时返回错误。
#[tauri::command]
#[specta::specta]
pub(crate) async fn load_sync_playback(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    note_rel: String,
) -> Result<SyncPlaybackDto, String> {
    let (root, meta, bookmarks, cloud_only) = {
        let vault_guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = vault_guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        let doc = vault.read(&note_rel).map_err(vault_err)?;
        let bookmarks = luna_vault::bookmarks_of(&doc.meta.bookmarks, &doc.content);
        // 只在云端的录音（按需下载，ADR-156）：先取到本机再装载。
        let cloud_only: Vec<String> = doc
            .meta
            .media
            .iter()
            .map(|m| m.path.clone())
            .filter(|path| vault.is_sync_cloud_only(path))
            .collect();
        (vault.root().to_path_buf(), doc.meta, bookmarks, cloud_only)
    };
    // 锁外下（几百 MB 的录音要一会儿，进度经 `sync://download-progress`）。没下成就照常往下走：
    // 播放器对「媒体不在」已有如实的说法。
    if !cloud_only.is_empty() {
        if let Some(service) =
            tauri::Manager::try_state::<std::sync::Arc<crate::sync_service::SyncService>>(&app)
        {
            for path in &cloud_only {
                let _ = service.on_demand(&app, path, false).await;
            }
        }
    }
    let media = meta.media.clone();
    let speaker_names: std::collections::HashMap<String, String> =
        meta.speakers.clone().into_iter().collect();
    let source_url = meta.source_url.clone();

    // 1. 分片表：仅可播放媒体（音频 + 视频），按会话区间升序；单分片缺省区间 = [0, duration*1000)。
    let media_parts = sorted_media_parts(&media);
    let parts = derive_playback_parts(&media_parts);
    let total_duration_ms = parts.last().map_or(0.0, |p| p.end_ms);
    let media_kind = parts_media_kind(&media_parts);

    // 2. cue 合并：逐分片读耐久边车（时间为会话绝对毫秒，无需偏移换算）。
    // 隐私铁律：CuePoint 携带转写文本仅供编辑器只读渲染（已落盘），不经敏感路径。
    let segments = collect_note_segments(&root, &meta);
    // 说话人名册 + 下一个可用 id（PRD-04-056）：与 cue 同一份段派生，前端零业务。
    let speakers: Vec<SpeakerStatDto> = luna_core::speaker_roster(&segments)
        .into_iter()
        .map(|stat| SpeakerStatDto {
            id: stat.id,
            segment_count: stat.segments,
            duration_ms: stat.duration.as_millis() as f64,
        })
        .collect();
    let next_speaker_id = luna_core::next_speaker_id(&segments);
    // 字幕规范阈值按**整篇语系**选档（PRD-05-033）：与导出重切分同一套数，避免出现
    // "校验说超标、导出却已自动修好"的自相矛盾提示。
    let constraints = luna_core::CueConstraints::for_segments(&segments);
    let cues: Vec<CuePointDto> = segments
        .into_iter()
        .map(|seg| {
            let duration = seg.end.saturating_sub(seg.start);
            CuePointDto {
                start_ms: seg.start.as_millis() as f64,
                end_ms: seg.end.as_millis() as f64,
                issues: luna_core::subtitle_issues(&seg.text, duration, &constraints)
                    .into_iter()
                    .map(SubtitleIssueDto::from)
                    .collect(),
                cps: luna_core::subtitle_cps(&seg.text, duration).map(f64::from),
                text: seg.text,
                translation: seg.translation,
                speaker: seg.speaker,
            }
        })
        .collect();

    // 3. 虚拟拼接注册：仅 WAV 分片满足 PCM 直拼前提（视频/压缩容器不进这条路，
    //    否则每次加载都要先失败一次才降级）；路径先过 Vault 越界校验再解析 RIFF 头。
    let composite_id = if parts_are_concatenable(&media_parts) {
        let resolved: Result<Vec<PathBuf>, String> = media_parts
            .iter()
            .map(|m| crate::media_server::resolve_vault_path(&root, &m.path))
            .collect();
        resolved
            .and_then(|paths| state.media_server.register_composite(paths))
            .ok()
    } else {
        None
    };
    let fallback_media_rel = if composite_id.is_none() {
        media_parts.first().map(|m| m.path.clone())
    } else {
        None
    };

    Ok(SyncPlaybackDto {
        parts,
        cues,
        bookmarks: bookmarks
            .into_iter()
            .map(|b| BookmarkDto {
                ms: b.ms as f64,
                note: b.note,
            })
            .collect(),
        composite_id,
        fallback_media_rel,
        media_kind,
        total_duration_ms,
        speaker_names,
        speakers,
        next_speaker_id,
        source_url,
    })
}

/// 说话人改名（PRD-04-054）：显示名写录音笔记 frontmatter `luna.speakers`（笔记即文件），
/// 回放徽标与导出前缀同源生效。`name` 为空 = 恢复默认「我/对方/对方N」标签。
#[tauri::command]
#[specta::specta]
pub(crate) fn set_speaker_name(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    note_rel: String,
    speaker: String,
    name: String,
) -> Result<(), String> {
    let content = {
        let vault_guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = vault_guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        vault
            .set_speaker_name(&note_rel, &speaker, &name)
            .map_err(vault_err)?;
        vault.read(&note_rel).map_err(vault_err)?.content
    };
    // frontmatter 已变：走 `note://written` 广播（多 pane/独立窗同文件同步的单一通道，
    // 与 write_note 同语义——接收端按 savedContentRef 判自写/增量/冲突）。
    let _ = (NoteWritten {
        path: note_rel,
        content,
        origin: "main".to_owned(),
    })
    .emit(&app);
    Ok(())
}

/// 更新同步回放中的单条 cue（PRD-04-031）。
///
/// 前端仅回传用户选中的 cue 区间与新文本；匹配、sidecar 写入、Markdown 时间戳行同步
/// 统一由 Vault 层完成，避免前端绕过笔记即文件的事实源。
/// 取笔记正文里全部带时间戳的行（PRD-04-066，ADR-118）。
///
/// 服务于「在笔记里就地听」：编辑面的时间戳行可点即跳、当前句随播放高亮。**只读**，
/// 不改任何东西；行号与编辑器所见一致（见 [`TimecodeLineDto`]）。
///
/// 与 [`load_sync_playback`] 的 cue 表刻意分开：那份是**重切分后的可读 cue**（与导出
/// 字幕同源），行数与正文行数不必相等；这份回答的是另一个问题——"正文第几行对应第几毫秒"。
/// 把两者混成一张表，任一侧的断行规则一变，另一侧就静默错位。
///
/// # Errors
/// Vault 未打开或笔记不存在时返回错误。
#[tauri::command]
#[specta::specta]
pub(crate) fn playback_timecode_lines(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    note_rel: String,
) -> Result<Vec<TimecodeLineDto>, String> {
    // 按窗口身份解析落点库（副库窗口读它自己的库，同 `read_note`）。
    let content = state.with_vault(window.label(), |vault| {
        vault
            .read(&note_rel)
            .map(|doc| doc.content)
            .map_err(vault_err)
    })?;
    Ok(content
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            luna_vault::timecode::parse_timecode(line).map(|(duration, _)| TimecodeLineDto {
                line: u32::try_from(index + 1).unwrap_or(u32::MAX),
                start_ms: duration.as_millis() as f64,
            })
        })
        .collect())
}

/// 上次听到的位置（PRD-04-068）；没记过返回 `None`。
///
/// 键 = 当前库根路径 + 库内相对路径，落**设备级**用量库：它是这台设备上的阅读进度，
/// 不是笔记内容——写进 frontmatter 或随库的那份，两台机器会互相覆盖。
///
/// # Errors
/// Vault 未打开或设备用量库不可读时返回错误。
#[tauri::command]
#[specta::specta]
pub(crate) fn playback_position_load(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    note_rel: String,
) -> Result<Option<f64>, String> {
    let vault_key = state.with_vault(window.label(), |vault| {
        Ok(vault.root().to_string_lossy().into_owned())
    })?;
    state
        .device_usage_store()
        .load_playback_position(&vault_key, &note_rel)
        .map(|ms| ms.map(|v| v as f64))
        .map_err(vault_err)
}

/// 记住这一篇听到哪了（PRD-04-068）。前端按节流调用；`0` = 从头（播完即归零）。
///
/// # Errors
/// Vault 未打开或设备用量库不可写时返回错误。
#[tauri::command]
#[specta::specta]
pub(crate) fn playback_position_save(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    note_rel: String,
    position_ms: f64,
) -> Result<(), String> {
    let vault_key = state.with_vault(window.label(), |vault| {
        Ok(vault.root().to_string_lossy().into_owned())
    })?;
    state
        .device_usage_store()
        .save_playback_position(&vault_key, &note_rel, position_ms.max(0.0) as i64)
        .map_err(vault_err)
}

#[tauri::command]
#[specta::specta]
pub(crate) fn update_playback_cue(
    state: tauri::State<'_, AppState>,
    note_rel: String,
    start_ms: f64,
    end_ms: f64,
    text: String,
    translation: Option<String>,
) -> Result<(), String> {
    let start = cue_ms(start_ms, "start_ms")?;
    let end = cue_ms(end_ms, "end_ms")?;
    if end <= start {
        return Err("cue 结束时间必须晚于开始时间".into());
    }

    let vault_guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = vault_guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    vault
        .update_playback_cue(&note_rel, start, end, &text, translation.as_deref())
        .map_err(vault_err)
}

/// 整篇转写编辑的一条 cue 文本改动（`update_playback_transcript` 入参）。
#[derive(Debug, Clone, Deserialize, Type)]
pub struct CueTextEditDto {
    /// 段起点（会话绝对毫秒，匹配键）。
    pub start_ms: f64,
    /// 段止点（会话绝对毫秒，匹配键）。
    pub end_ms: f64,
    /// 新文本。
    pub text: String,
}

/// 整篇转写编辑批量写回（PRD-04-031「文本编辑模式」的原子保存）：把用户整篇校对改过的多条
/// cue 文本一次性权威落盘（正文哨兵块整块重写 + 各分片边车 + 标记一致），保留译文/说话人结构。
///
/// 前端只回传 `(start_ms, end_ms, text)`；匹配段更新、sidecar 写入、正文重写统一由 Vault 层完成，
/// 不绕过「笔记即文件」事实源。为自写静默改（不发 `vault://changed`），前端保存成功后主动重载。
///
/// # Errors
/// Vault 未打开、笔记不存在、时间戳非法或任一文本为空时返回错误。
#[tauri::command]
#[specta::specta]
pub(crate) fn update_playback_transcript(
    state: tauri::State<'_, AppState>,
    note_rel: String,
    edits: Vec<CueTextEditDto>,
) -> Result<(), String> {
    let edits: Vec<luna_vault::CueTextEdit> = edits
        .into_iter()
        .map(|e| {
            Ok(luna_vault::CueTextEdit {
                start_ms: cue_ms(e.start_ms, "start_ms")?,
                end_ms: cue_ms(e.end_ms, "end_ms")?,
                text: e.text,
            })
        })
        .collect::<Result<_, String>>()?;

    let vault_guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = vault_guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    vault
        .update_playback_transcript(&note_rel, &edits)
        .map_err(vault_err)
}

/// 重指派作用范围（DTO ↔ [`luna_core::AssignScope`]，PRD-04-056）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SpeakerScopeDto {
    /// 仅这一条。
    Single,
    /// 这一条 + 其后连续同原说话人的段。
    Following,
    /// 全篇所有原说话人相同的段（= 说话人合并）。
    All,
}

impl From<SpeakerScopeDto> for luna_core::AssignScope {
    fn from(value: SpeakerScopeDto) -> Self {
        match value {
            SpeakerScopeDto::Single => Self::Single,
            SpeakerScopeDto::Following => Self::Following,
            SpeakerScopeDto::All => Self::All,
        }
    }
}

/// 转写段人工校正操作（`edit_playback_segment` 入参，PRD-04-031 / PRD-04-056）。段以
/// `(start_ms, end_ms)` 会话绝对毫秒定位（与整篇编辑同一匹配键；前端不持有段 id）。
#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "snake_case", tag = "op", content = "data")]
pub enum SegmentEditDto {
    /// 重指派说话人。
    Assign {
        /// 段起点（匹配键）。
        start_ms: f64,
        /// 段止点（匹配键）。
        end_ms: f64,
        /// 目标 speaker id（取自名册或 `next_speaker_id`）。
        speaker: String,
        /// 作用范围。
        scope: SpeakerScopeDto,
    },
    /// 在光标处拆成两段（右片可同时指派说话人）。
    Split {
        /// 段起点（匹配键）。
        start_ms: f64,
        /// 段止点（匹配键）。
        end_ms: f64,
        /// 拆分点 = `textarea.selectionStart`（UTF-16 码元偏移，Rust 侧同口径换算）。
        char_offset: u32,
        /// 右片说话人（`null` = 沿用左片，纯断句拆分）。
        speaker: Option<String>,
    },
    /// 与上一条合并（拆分的逆操作）。
    Merge {
        /// 段起点（匹配键）。
        start_ms: f64,
        /// 段止点（匹配键）。
        end_ms: f64,
    },
    /// 整条删除（PRD-04-031：片头广告 / 纯口水 / 静音段的凭空识别）。
    Delete {
        /// 段起点（匹配键）。
        start_ms: f64,
        /// 段止点（匹配键）。
        end_ms: f64,
    },
}

/// 转写段人工校正（PRD-04-031 删除 / PRD-04-056 重指派·拆分·合并）。
///
/// 机器产出必有错判（相近音色并簇、抢话段整段归主导者、句界抖动、静音段凭空识别出文本），
/// 行业形态一致提供人工兜底；本命令是四件兜底的唯一入口。算法在 [`luna_core::segment`]
/// （纯函数），落盘经 `Vault::edit_transcript_segment` 走权威写回单一点（正文哨兵块 + 边车 +
/// 标记一致）。为自写静默改（不发 `vault://changed`），前端成功后主动重载回放包。
///
/// # Errors
/// Vault 未打开、笔记不存在、段未匹配、拆分点切出空片或首段无「上一条」时返回错误。
#[tauri::command]
#[specta::specta]
pub(crate) fn edit_playback_segment(
    state: tauri::State<'_, AppState>,
    note_rel: String,
    edit: SegmentEditDto,
) -> Result<(), String> {
    let edit = match edit {
        SegmentEditDto::Assign {
            start_ms,
            end_ms,
            speaker,
            scope,
        } => luna_vault::SegmentEdit::Assign {
            start_ms: cue_ms(start_ms, "start_ms")?,
            end_ms: cue_ms(end_ms, "end_ms")?,
            speaker,
            scope: scope.into(),
        },
        SegmentEditDto::Split {
            start_ms,
            end_ms,
            char_offset,
            speaker,
        } => luna_vault::SegmentEdit::Split {
            start_ms: cue_ms(start_ms, "start_ms")?,
            end_ms: cue_ms(end_ms, "end_ms")?,
            char_offset: char_offset as usize,
            speaker,
        },
        SegmentEditDto::Merge { start_ms, end_ms } => luna_vault::SegmentEdit::Merge {
            start_ms: cue_ms(start_ms, "start_ms")?,
            end_ms: cue_ms(end_ms, "end_ms")?,
        },
        SegmentEditDto::Delete { start_ms, end_ms } => luna_vault::SegmentEdit::Delete {
            start_ms: cue_ms(start_ms, "start_ms")?,
            end_ms: cue_ms(end_ms, "end_ms")?,
        },
    };
    let vault_guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = vault_guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    vault
        .edit_transcript_segment(&note_rel, &edit)
        .map_err(vault_err)
}

pub(crate) fn cue_ms(value: f64, field: &str) -> Result<u64, String> {
    if !value.is_finite() || value < 0.0 {
        return Err(format!("{field} 非法"));
    }
    Ok(value.round() as u64)
}

/// 逐分片读耐久边车并合并为绝对时钟升序段列表（回放聚合与导出共用，DRY）。
///
/// 降级链与回放一致：`segments: None` 分片跳过（诚实元数据），边车缺失/损坏 → 该分片 0 段。
pub(crate) fn collect_note_segments(
    root: &Path,
    meta: &luna_vault::LunaMeta,
) -> Vec<luna_core::Segment> {
    // 实现下沉到 luna-vault（`note_segments`）：CLI `luna export` 与 GUI 同一份（PRD-11-006）。
    luna_vault::note_segments(root, meta)
}

/// 转写导出格式（`export_transcript` 入参；映射 `luna-export` 的格式 + 选项，TDD-10）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptExportFormat {
    /// SubRip 字幕。
    Srt,
    /// WebVTT 字幕。
    Vtt,
    /// ASS 字幕（Advanced SubStation Alpha）。
    Ass,
    /// LRC 歌词（双语 = 同时间戳双行）。
    Lrc,
    /// B 站 CC 字幕（JSON，PRD-11-014）。
    Bcc,
    /// 逐行带时间戳文本。
    TimestampedTxt,
    /// 纯文本。
    PlainTxt,
}

impl TranscriptExportFormat {
    /// DTO → 导出管线的 `(格式, 是否带行首时间标记)`（时间标记是 txt 的选项而非独立格式）。
    fn to_export(self) -> (luna_export::ExportFormat, bool) {
        match self {
            Self::Srt => (luna_export::ExportFormat::Srt, false),
            Self::Vtt => (luna_export::ExportFormat::Vtt, false),
            Self::Ass => (luna_export::ExportFormat::Ass, false),
            Self::Lrc => (luna_export::ExportFormat::Lrc, false),
            Self::Bcc => (luna_export::ExportFormat::Bcc, false),
            Self::TimestampedTxt => (luna_export::ExportFormat::Txt, true),
            Self::PlainTxt => (luna_export::ExportFormat::Txt, false),
        }
    }
}

/// 转写导出的可调选项（PRD-11-004/007）：时移、BOM、折行覆盖、说话人前缀。
/// 一份 DTO 同时服务单篇与批量，前端的「导出选项」对话框只维护这一份。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
pub struct TranscriptExportOptionsDto {
    /// 整体时移（毫秒，可负；结果钳在 0）。
    pub time_offset_ms: i32,
    /// SRT / VTT 加 UTF-8 BOM。
    pub utf8_bom: bool,
    /// 每行最大字符数覆盖；`None` = 按语系档自动；`0` = 不限。
    pub max_line_len: Option<u32>,
    /// 每条最大行数覆盖；`None` = 按语系档（2）。
    pub max_lines: Option<u32>,
    /// 段带说话人时输出「名字: 」前缀。
    pub speaker_prefix: bool,
    /// 中文字形转换（PRD-11-013）。
    pub han_variant: HanVariantDto,
}

/// 中文字形变体（PRD-11-013；映射 `luna_export::HanVariant`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum HanVariantDto {
    /// 不转换。
    Keep,
    /// 简体。
    Simplified,
    /// 繁体（台湾）。
    TraditionalTaiwan,
    /// 繁体（香港）。
    TraditionalHongKong,
}

impl From<HanVariantDto> for luna_export::HanVariant {
    fn from(value: HanVariantDto) -> Self {
        match value {
            HanVariantDto::Keep => Self::Keep,
            HanVariantDto::Simplified => Self::Simplified,
            HanVariantDto::TraditionalTaiwan => Self::TraditionalTaiwan,
            HanVariantDto::TraditionalHongKong => Self::TraditionalHongKong,
        }
    }
}

impl TranscriptExportOptionsDto {
    fn apply(
        self,
        base: luna_export::ExportOptions,
        has_speakers: bool,
    ) -> luna_export::ExportOptions {
        luna_export::ExportOptions {
            speaker_prefix: self.speaker_prefix && has_speakers,
            time_offset_ms: i64::from(self.time_offset_ms),
            utf8_bom: self.utf8_bom,
            max_line_len: self.max_line_len.map(|n| n as usize),
            max_lines: self
                .max_lines
                .map(|n| u8::try_from(n.clamp(1, 8)).unwrap_or(2)),
            han_variant: self.han_variant.into(),
            ..base
        }
    }
}

/// 转写导出内容（与回放视图双语三态对齐，PRD-04-025 / PRD-11-003）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptExportContent {
    /// 仅原文。
    Source,
    /// 仅译文（缺译回退原文）。
    Translation,
    /// 双语对照。
    Bilingual,
}

impl From<TranscriptExportContent> for luna_export::Bilingual {
    fn from(value: TranscriptExportContent) -> Self {
        match value {
            TranscriptExportContent::Source => Self::SourceOnly,
            TranscriptExportContent::Translation => Self::TargetOnly,
            TranscriptExportContent::Bilingual => Self::Both,
        }
    }
}

/// 渲染一篇转写为目标格式（单篇导出与批量导出共用，DRY）。
///
/// 段收集 + 说话人显示名解析 + `luna-export` 注册管线，纯同步、不跨 await 持锁。
/// 无转写段的笔记返回 `Ok(None)`——批量场景据此**跳过**而非中断整批（PRD-05 §5）。
fn render_transcript(
    app: &tauri::AppHandle,
    vault: &Vault,
    note_rel: &str,
    export_format: luna_export::ExportFormat,
    include_timestamps: bool,
    content: TranscriptExportContent,
    options: TranscriptExportOptionsDto,
) -> Result<Option<String>, String> {
    let doc = vault.read(note_rel).map_err(vault_err)?;
    let mut segments = collect_note_segments(vault.root(), &doc.meta);
    if segments.is_empty() {
        return Ok(None);
    }
    // 说话人前缀（PRD-04-054）：speaker id → 显示名（改名映射优先，与回放徽标同源），
    // 有任何说话人即开前缀（"张三: …"）。
    let i18n_state = backend_copy_arc(app);
    let has_speakers =
        resolve_segment_speakers(&mut segments, &doc.meta.speakers, i18n_state.as_deref());
    let base = luna_export::ExportOptions {
        bilingual: content.into(),
        include_timestamps,
        ..luna_export::ExportOptions::default()
    };
    luna_export::ExportRegistry::with_builtin()
        .export(
            export_format,
            &luna_export::Exportable::Segments(segments),
            &options.apply(base, has_speakers),
        )
        .map(Some)
        .map_err(|e| format!("EXPORT_FAILED: {e}"))
}

/// 弹出保存对话框（Rust 侧，前端零业务），返回用户选择的目标路径；取消返回 `None`。
pub(crate) async fn pick_export_destination(
    app: &tauri::AppHandle,
    file_name: &str,
    extension: &'static str,
) -> Result<Option<PathBuf>, String> {
    use tauri_plugin_dialog::DialogExt as _;

    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_file_name(file_name)
        .add_filter(extension.to_ascii_uppercase(), &[extension])
        .save_file(move |path| {
            let result = path
                .map(|path| path.into_path().map_err(|e| e.to_string()))
                .transpose();
            let _ = tx.send(result);
        });
    rx.await
        .map_err(|_| coded_error(app, "DIALOG_CANCELLED", "dialog-cancelled"))?
}

/// 导出录音转写（PRD-11-001/003：SRT / 带时间戳文本 / 纯文本；内容三态与回放显示一致）。
///
/// 渲染走 `luna-export` 注册管线（纯函数，TDD-10 §1/§4）；边车段收集、保存对话框与
/// 写盘在此层——前端只发命令（ADR-003 前端零业务）。字幕类只取时间轴段（PRD-11-012）。
/// 返回写入的绝对路径；用户取消对话框返回 `None`。
///
/// # Errors
/// Vault 未打开、笔记不存在、无可导出转写段或写盘失败时返回错误。
#[tauri::command]
#[specta::specta]
pub(crate) async fn export_transcript(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    note_rel: String,
    format: TranscriptExportFormat,
    content: TranscriptExportContent,
    options: TranscriptExportOptionsDto,
) -> Result<Option<String>, String> {
    let (export_format, include_timestamps) = format.to_export();
    // 先收集并渲染（不跨 await 持锁），再弹保存对话框。
    let (rendered, note_stem) = {
        let vault_guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = vault_guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        let rendered = render_transcript(
            &app,
            vault,
            &note_rel,
            export_format,
            include_timestamps,
            content,
            options,
        )?
        .ok_or_else(|| coded_error(&app, "EXPORT_NO_TRANSCRIPT", "export-no-transcript-single"))?;
        (
            rendered,
            export_stem(
                &state.settings_snapshot(),
                &note_stem_of(&note_rel),
                export_format.extension(),
                content_label(content),
            ),
        )
    };

    let Some(dest) = pick_export_destination(&app, &note_stem, export_format.extension()).await?
    else {
        return Ok(None);
    };
    std::fs::write(&dest, rendered).map_err(|e| format!("EXPORT_WRITE_FAILED: {e}"))?;
    Ok(Some(dest.to_string_lossy().into_owned()))
}

/// 选择导出目标**文件夹**（批量导出用）；用户取消返回 `None`。
pub(crate) async fn pick_export_dir(app: &tauri::AppHandle) -> Result<Option<PathBuf>, String> {
    use tauri_plugin_dialog::DialogExt as _;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_folder(move |dir| {
        let result = dir
            .map(|dir| dir.into_path().map_err(|e| e.to_string()))
            .transpose();
        let _ = tx.send(result);
    });
    rx.await
        .map_err(|_| coded_error(app, "DIALOG_CANCELLED", "dialog-cancelled"))?
}

/// **批量导出**多篇转写到一个文件夹（PRD-05-021）：每篇写 `<笔记名>.<扩展名>`。
///
/// 服务字幕译者的真实闭环——导入 10 个文件后一次拿到 10 份 SRT，而不是逐篇点保存。
/// 一篇失败/无转写段**不中断整批**（PRD-05 §5「单个文件失败不影响其余」），返回实际写出
/// 的份数；同名文件追加序号，绝不静默覆盖用户已有文件。
///
/// # Errors
/// Vault 未打开、用户取消对话框以外的写盘失败（目标目录不可写）时返回错误。
#[tauri::command]
#[specta::specta]
pub(crate) async fn export_transcripts(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    note_rels: Vec<String>,
    format: TranscriptExportFormat,
    content: TranscriptExportContent,
    options: TranscriptExportOptionsDto,
) -> Result<u32, String> {
    let (export_format, include_timestamps) = format.to_export();
    let settings_for_names = state.settings_snapshot();
    // 先全部渲染（不跨 await 持锁），再弹目录选择——避免对话框期间长时间持 Vault 锁。
    let rendered: Vec<(String, String)> = {
        let vault_guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = vault_guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        note_rels
            .iter()
            .filter_map(|note_rel| {
                match render_transcript(
                    &app,
                    vault,
                    note_rel,
                    export_format,
                    include_timestamps,
                    content,
                    options,
                ) {
                    Ok(Some(text)) => Some((
                        export_stem(
                            &settings_for_names,
                            &note_stem_of(note_rel),
                            export_format.extension(),
                            content_label(content),
                        ),
                        text,
                    )),
                    // 无转写段 / 读取失败：跳过该篇，其余照常导出（脱敏日志仅路径与错误）。
                    Ok(None) => None,
                    Err(e) => {
                        eprintln!("[export] 跳过一篇（{note_rel}）：{e}");
                        None
                    }
                }
            })
            .collect()
    };
    if rendered.is_empty() {
        return Err(coded_error(
            &app,
            "EXPORT_NO_TRANSCRIPT",
            "export-no-transcript-batch",
        ));
    }
    let Some(dir) = pick_export_dir(&app).await? else {
        return Ok(0);
    };
    let ext = export_format.extension();
    let mut written = 0u32;
    for (stem, text) in rendered {
        let dest = unique_export_path(&dir, &stem, ext);
        match std::fs::write(&dest, text) {
            Ok(()) => written += 1,
            Err(e) => eprintln!("[export] 写出失败（{}）：{e}", dest.display()),
        }
    }
    if written == 0 {
        return Err(coded_error(
            &app,
            "EXPORT_WRITE_FAILED",
            "export-write-failed",
        ));
    }
    Ok(written)
}

/// 目标目录内的唯一文件名：`stem.ext` 被占用则 `stem-2.ext`、`stem-3.ext`…（绝不覆盖）。
pub(crate) fn unique_export_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return first;
    }
    for n in 2..1000u32 {
        let candidate = dir.join(format!("{stem}-{n}.{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    first
}

/// 导出笔记正文为纯文本（速记等无时间轴笔记的导出入口）。
///
/// 剥离 frontmatter 后写出正文（`luna_vault::body_of`）；返回写入的绝对路径，
/// 用户取消对话框返回 `None`。
///
/// # Errors
/// Vault 未打开、笔记不存在、正文为空或写盘失败时返回错误。
#[tauri::command]
#[specta::specta]
pub(crate) async fn export_note_text(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    note_rel: String,
) -> Result<Option<String>, String> {
    let (rendered, note_stem) = {
        let vault_guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = vault_guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        let doc = vault.read(&note_rel).map_err(vault_err)?;
        let body = luna_vault::body_of(&doc.content).to_string();
        let rendered = luna_export::ExportRegistry::with_builtin()
            .export(
                luna_export::ExportFormat::Txt,
                &luna_export::Exportable::Text(body),
                &luna_export::ExportOptions::default(),
            )
            .map_err(|e| format!("EXPORT_FAILED: {e}"))?;
        (
            rendered,
            export_stem(
                &state.settings_snapshot(),
                &note_stem_of(&note_rel),
                luna_export::ExportFormat::Txt.extension(),
                "text",
            ),
        )
    };

    let Some(dest) =
        pick_export_destination(&app, &note_stem, luna_export::ExportFormat::Txt.extension())
            .await?
    else {
        return Ok(None);
    };
    std::fs::write(&dest, rendered).map_err(|e| format!("EXPORT_WRITE_FAILED: {e}"))?;
    Ok(Some(dest.to_string_lossy().into_owned()))
}

/// **批量导出**多篇笔记正文为纯文本（多选「导出为文本」，PRD-11-001）：每篇写
/// `<笔记名>.txt` 到用户选的文件夹，同名追加序号绝不覆盖。返回实际写出的份数；
/// 用户取消目录选择返回 `0`。
///
/// 与单篇 [`export_note_text`] 的落点形态不同（保存对话框 vs 目录选择），故是两条命令
/// ——与字幕导出的单篇/批量分工逐字一致。一篇读不出/正文为空**不中断整批**。
///
/// # Errors
/// Vault 未打开、无一篇有可导出正文，或全部写盘失败时返回错误。
#[tauri::command]
#[specta::specta]
pub(crate) async fn export_notes_text(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    note_rels: Vec<String>,
) -> Result<u32, String> {
    let settings_for_names = state.settings_snapshot();
    let registry = luna_export::ExportRegistry::with_builtin();
    let rendered: Vec<(String, String)> = {
        let vault_guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = vault_guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        note_rels
            .iter()
            .filter_map(|note_rel| {
                let doc = vault.read(note_rel).ok()?;
                let body = luna_vault::body_of(&doc.content).to_string();
                registry
                    .export(
                        luna_export::ExportFormat::Txt,
                        &luna_export::Exportable::Text(body),
                        &luna_export::ExportOptions::default(),
                    )
                    .ok()
                    .map(|text| {
                        (
                            export_stem(
                                &settings_for_names,
                                &note_stem_of(note_rel),
                                luna_export::ExportFormat::Txt.extension(),
                                "text",
                            ),
                            text,
                        )
                    })
            })
            .collect()
    };
    if rendered.is_empty() {
        return Err(coded_error(&app, "EXPORT_FAILED", "export-no-text-batch"));
    }
    let Some(dir) = pick_export_dir(&app).await? else {
        return Ok(0);
    };
    let ext = luna_export::ExportFormat::Txt.extension();
    let mut written = 0u32;
    for (stem, text) in rendered {
        let dest = unique_export_path(&dir, &stem, ext);
        match std::fs::write(&dest, text) {
            Ok(()) => written += 1,
            Err(e) => eprintln!("[export] 写出失败（{}）：{e}", dest.display()),
        }
    }
    if written == 0 {
        return Err(coded_error(
            &app,
            "EXPORT_WRITE_FAILED",
            "export-write-failed",
        ));
    }
    Ok(written)
}

/// 说话人结构 → 后端 i18n 文案（「我」「对方」「对方 N」「说话人 N」）。
fn speaker_label_copy(
    i18n: Option<&crate::i18n::BackendI18n>,
    label: luna_vault::SpeakerLabel<'_>,
) -> Option<String> {
    let i18n = i18n?;
    match label {
        luna_vault::SpeakerLabel::SelfTrack => i18n.translate("track-self"),
        luna_vault::SpeakerLabel::Remote => i18n.translate("track-remote"),
        luna_vault::SpeakerLabel::RemoteN(n) => {
            i18n.translate_args("track-remote-n", &[("n", n.to_owned())])
        }
        luna_vault::SpeakerLabel::SpeakerN(n) => {
            i18n.translate_args("track-speaker-n", &[("n", n.to_owned())])
        }
        luna_vault::SpeakerLabel::Other(_) => None,
    }
}

/// 把段列表的 speaker id 原地解析为显示名（导出/纪要装配前调用；返回是否存在任何说话人）。
pub(crate) fn resolve_segment_speakers(
    segments: &mut [luna_core::Segment],
    names: &std::collections::BTreeMap<String, String>,
    i18n: Option<&crate::i18n::BackendI18n>,
) -> bool {
    luna_vault::resolve_segment_speakers(segments, names, &|label| speaker_label_copy(i18n, label))
}

/// 录音笔记的「说话人显名转写」文本（会议纪要输入装配，PRD-04-054 / TDD-03 §10.3）。
///
/// 仅当目标是 `type: recording` 笔记、有转写段、且**任一段带说话人**时返回逐行
/// `显示名: 文本`（显示名解析与回放徽标/导出前缀同源，DRY）；否则 `None`——调用方
/// 沿用原始正文（普通笔记总结路径零改变）。纯装配，不发 Job（ADR-001 主干不变）。
pub(crate) fn speaker_labeled_transcript(
    state: &AppState,
    i18n: Option<&crate::i18n::BackendI18n>,
    note_rel: &str,
) -> Option<String> {
    let guard = state.vault.lock().ok()?;
    let vault = guard.as_ref()?;
    let doc = vault.read(note_rel).ok()?;
    if doc.meta.note_type != NoteType::Recording {
        return None;
    }
    let mut segments = collect_note_segments(vault.root(), &doc.meta);
    if !resolve_segment_speakers(&mut segments, &doc.meta.speakers, i18n) {
        return None;
    }
    let mut out = String::new();
    for seg in &segments {
        let text = seg.text.trim();
        if text.is_empty() {
            continue;
        }
        if let Some(name) = seg.speaker.as_deref() {
            out.push_str(name);
            out.push_str(": ");
        }
        out.push_str(text);
        out.push('\n');
    }
    if out.trim().is_empty() {
        None
    } else {
        Some(out)
    }
}

/// 笔记相对路径 → 导出默认文件名主干（去目录、去 `.md`）。
/// 按设置 `export.filename_template` 渲染导出文件名 stem（PRD-11-007）：`{name}` = 笔记名，
/// `{date}` / `{time}` = 导出此刻，`{format}` = 扩展名，`{content}` = 内容口径。
pub(crate) fn export_stem(settings: &Settings, name: &str, format: &str, content: &str) -> String {
    let template = settings
        .get_str("export.filename_template")
        .unwrap_or_else(|| luna_core::filename_template::DEFAULT_TEMPLATE.to_owned());
    let now = chrono::Local::now();
    let date = now.format("%Y-%m-%d").to_string();
    let time = now.format("%H%M").to_string();
    luna_core::filename_template::render(
        &template,
        luna_core::filename_template::FilenameContext {
            name,
            date: &date,
            time: &time,
            format,
            content,
        },
    )
}

/// **导出文件名预览**（PRD-11-007，2026-10-08 C47）：用设置页上**正在编辑的那一份**模板，
/// 按导出时同一个渲染函数（[`luna_core::filename_template::render`]）、此刻的日期时间，渲染出
/// 一个示例文件名（示例笔记名由界面给、字幕格式与原文口径固定）。模板不合法时返回 `None`——
/// 不合法的原因由设置写入那条路径说，这里不另造一份错误。
///
/// 替代的是一段语法说明：用户看见「会议记录 2026-10-08.srt」，比读懂五个占位符的含义快。
/// 重名追加序号、不覆盖已有文件是导出时的行为，预览只给一个文件名，不预言磁盘上的冲突。
#[tauri::command]
#[specta::specta]
pub(crate) fn preview_export_filename(template: String, sample_name: String) -> Option<String> {
    luna_core::filename_template::validate(&template).ok()?;
    let now = chrono::Local::now();
    let date = now.format("%Y-%m-%d").to_string();
    let time = now.format("%H%M").to_string();
    let stem = luna_core::filename_template::render(
        &template,
        luna_core::filename_template::FilenameContext {
            name: &sample_name,
            date: &date,
            time: &time,
            format: "srt",
            content: "source",
        },
    );
    Some(format!("{stem}.srt"))
}

/// 内容口径在文件名里的写法。
pub(crate) fn content_label(content: TranscriptExportContent) -> &'static str {
    match content {
        TranscriptExportContent::Source => "source",
        TranscriptExportContent::Translation => "translation",
        TranscriptExportContent::Bilingual => "bilingual",
    }
}

pub(crate) fn note_stem_of(note_rel: &str) -> String {
    Path::new(note_rel)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("transcript")
        .to_string()
}

/// 可播放媒体扩展名（**单一来源**，PRD-04-058）：导入选择框过滤、回放分片识别、
/// 导入落地策略三处共用，绝不各自维护一份列表。
///
/// 音频档：symphonia 可解（[TDD-02 §3.7]）且媒体服务可流式提供。
pub(crate) const AUDIO_MEDIA_EXTS: &[&str] = &["wav", "mp3", "m4a", "aac", "flac", "ogg", "opus"];
/// 视频档：须**同时**满足「symphonia 可解其音轨」与「Webview 可解码其画面」——
/// 二者的交集即 isomp4 家族（mp4 / mov / m4v）。mkv/avi 任一侧不成立故不收（诚实拒绝，
/// 不做「能导入却放不出画面」的半成品，PRD-05-002 的 FFmpeg 路线未落地前维持此边界）。
pub(crate) const VIDEO_MEDIA_EXTS: &[&str] = &["mp4", "mov", "m4v"];

/// 可播放媒体的类型（回放前端据此决定音频条 or 视频舞台，PRD-04-058）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackMediaKind {
    /// 纯音频分片（录音会话产物 / 导入音频转码的 WAV）。
    Audio,
    /// 带画面的视频容器（导入视频原件，画面与音轨同源单文件）。
    Video,
}

/// 取小写扩展名（无扩展 → 空串）。
fn lower_ext(path: &str) -> String {
    Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// 媒体路径 → 可播放类型；`None` = 非可播放媒体（图片/文档等，不入回放分片表）。
pub(crate) fn media_kind(path: &str) -> Option<PlaybackMediaKind> {
    let ext = lower_ext(path);
    if AUDIO_MEDIA_EXTS.contains(&ext.as_str()) {
        Some(PlaybackMediaKind::Audio)
    } else if VIDEO_MEDIA_EXTS.contains(&ext.as_str()) {
        Some(PlaybackMediaKind::Video)
    } else {
        None
    }
}

/// 过滤可播放媒体（音频 + 视频）并按会话区间升序排序（纯逻辑，headless 可测）。
pub(crate) fn sorted_media_parts(media: &[MediaRef]) -> Vec<&MediaRef> {
    let mut parts: Vec<&MediaRef> = media
        .iter()
        .filter(|m| media_kind(&m.path).is_some())
        .collect();
    parts.sort_by_key(|m| m.part_start_ms.unwrap_or(0));
    parts
}

/// 分片表整体的呈现类型：任一分片是视频即视频（导入视频恒单分片，PRD-04-058）。
pub(crate) fn parts_media_kind(parts: &[&MediaRef]) -> PlaybackMediaKind {
    if parts
        .iter()
        .any(|m| media_kind(&m.path) == Some(PlaybackMediaKind::Video))
    {
        PlaybackMediaKind::Video
    } else {
        PlaybackMediaKind::Audio
    }
}

/// 分片是否可做**虚拟拼接**（媒体服务的 PCM 直拼前提，TDD-12 §4）：仅自家 WAV 分片成立。
/// 视频/压缩音频容器一律走单分片直链（导入产物恒单分片，不损失连续时间轴语义）。
pub(crate) fn parts_are_concatenable(parts: &[&MediaRef]) -> bool {
    !parts.is_empty() && parts.iter().all(|m| lower_ext(&m.path) == "wav")
}

/// 由媒体映射推导分片区间表（纯逻辑，headless 可测）。
///
/// 多 part 录音 frontmatter 携带显式 `part_start_ms/part_end_ms`；
/// 单 part 省略区间字段 → 缺省 `[0, duration*1000)`；二者皆缺 → `[start, start)`（未知）。
pub(crate) fn derive_playback_parts(media_parts: &[&MediaRef]) -> Vec<PlaybackPartDto> {
    media_parts
        .iter()
        .map(|m| {
            let start_ms = m.part_start_ms.unwrap_or(0) as f64;
            let end_ms = m
                .part_end_ms
                .map(|v| v as f64)
                .or_else(|| m.duration.map(|secs| start_ms + secs * 1000.0))
                .unwrap_or(start_ms);
            PlaybackPartDto {
                media_rel: m.path.clone(),
                start_ms,
                end_ms,
            }
        })
        .collect()
}
// ─── 回放 DTO ──────────────────────────────────────────────────────────────────

/// 字幕规范问题（DTO ↔ [`luna_core::SubtitleIssue`]，PRD-05-033）。
///
/// **只含重切分修不掉、必须人工处理的项**——行长超限与时长过短由导出前的
/// `segments_to_cues` 自动修（拆分 / 合并），报出来只会制造"我改不掉"的噪声。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(rename_all = "kebab-case")]
pub enum SubtitleIssueDto {
    /// 阅读速度超限：这段时间内的字太多，观众读不完（精简文本或延长显示时间）。
    Cps,
    /// 单条显示过久：在该处拆成两条即可。
    TooLong,
}

impl From<luna_core::SubtitleIssue> for SubtitleIssueDto {
    fn from(value: luna_core::SubtitleIssue) -> Self {
        match value {
            luna_core::SubtitleIssue::Cps => Self::Cps,
            luna_core::SubtitleIssue::TooLong => Self::TooLong,
        }
    }
}

/// 转写 Cue 点（前端时间轴跳转，TDD-06 §2.1）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct CuePointDto {
    /// 绝对起点（ms）。TS `number`（f64）。
    pub start_ms: f64,
    /// 绝对止点（ms）。TS `number`（f64）。
    pub end_ms: f64,
    /// 转写文本（已落盘，供只读展示；前端不向后端回传）。
    pub text: String,
    /// 译文（实时转译/二次转译产物；`None` = 该段无译文，PRD-04-025 双语回放）。
    pub translation: Option<String>,
    /// 说话人标签（PRD-04-053/054：`"self"`=我 / `"remote"`=对方 / `"remote:N"`=对方 N；
    /// `None` = 单麦克风源无标签）。前端映射显示名（common:track.*）。
    pub speaker: Option<String>,
    /// 字幕规范问题（PRD-05-033）；空 = 合规。阈值与导出重切分同源（`CueConstraints`），
    /// 判定在 Rust——前端零业务只渲染徽标（ADR-003）。
    pub issues: Vec<SubtitleIssueDto>,
    /// 阅读速度（字符/秒）；`None` = 时长未知。供徽标显示"N 字/秒"，前端不自己算。
    pub cps: Option<f64>,
}

/// 录音分片（会话绝对时间轴上的区间，PRD-04 §5）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct PlaybackPartDto {
    /// 分片媒体相对路径。
    pub media_rel: String,
    /// 分片在会话时间轴的绝对起点（ms）。
    pub start_ms: f64,
    /// 分片在会话时间轴的绝对止点（ms；未知时等于 start_ms）。
    pub end_ms: f64,
}

/// 同步回放聚合包（`load_sync_playback` 返回值）。
/// 一条书签（PRD-04-005）：毫秒来自 frontmatter 精确层，备注来自正文 🔖 行。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct BookmarkDto {
    pub ms: f64,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SyncPlaybackDto {
    /// 分片表（会话时间轴升序）。
    pub parts: Vec<PlaybackPartDto>,
    /// 全分片合并 cue 列表（绝对时钟升序）。
    pub cues: Vec<CuePointDto>,
    /// 录音书签（会话绝对 ms + 可选备注，PRD-04-005）。
    pub bookmarks: Vec<BookmarkDto>,
    /// 虚拟拼接资源 id（`/stream?composite=<id>`）；`None` = 走 fallback 直链。
    pub composite_id: Option<String>,
    /// 拼接不可用时的首分片直链回退（`/stream?path=<rel>`）。
    pub fallback_media_rel: Option<String>,
    /// 本笔记媒体的呈现类型（PRD-04-058）：`video` = 回放视图挂视频舞台并叠字幕，
    /// `audio` = 纯音频条。由分片扩展名派生，前端零业务不猜。
    pub media_kind: PlaybackMediaKind,
    /// 会话总时长（ms；0 = 未知，前端以 `<audio>.duration` 兜底）。
    pub total_duration_ms: f64,
    /// 说话人显示名映射（PRD-04-054：speaker id → 用户改名，如 `remote:1`→「张三」；
    /// 事实源 = 笔记 frontmatter `luna.speakers`，导出前缀同源）。
    pub speaker_names: std::collections::HashMap<String, String>,
    /// 说话人名册（PRD-04-056：按首次出现顺序，含段数/时长）——回放说话人条与指派菜单
    /// 的数据源；顺序即配色槽位顺序（稳定不跳色）。
    pub speakers: Vec<SpeakerStatDto>,
    /// 「新建说话人」的下一个可用 id（沿用本录音已有命名家族；前端零业务只呈现）。
    pub next_speaker_id: String,
    /// 内容来源地址（PRD-05-006「从链接导入」的溯源；`None` = 录音或本地文件导入）。
    /// 回放页头据此给一枚可点的「来源」——三个月后回看，用户要能一键回到原视频核对上下文。
    pub source_url: Option<String>,
}

/// 笔记正文里一条带时间戳的行（PRD-04-066）。
///
/// 行号口径 = **读到的文件内容的 1-based 行号**，与编辑器所见完全一致（`Vault::read`
/// 返回的 `content` 含 frontmatter，编辑器渲染的也是这一份，故两边行号天然对齐——
/// 前端不需要、也不允许自己数行或解析时间戳格式，那是 Rust 的事，ADR-003）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct TimecodeLineDto {
    /// 1-based 行号。
    pub line: u32,
    /// 该行时间戳对应的会话绝对毫秒。
    pub start_ms: f64,
}

/// 说话人名册项（PRD-04-056）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SpeakerStatDto {
    /// speaker id（`self` / `remote` / `remote:N` / `speaker:N`）。
    pub id: String,
    /// 该说话人的段数。
    pub segment_count: u32,
    /// 该说话人的累计时长（ms）。
    pub duration_ms: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 预览与导出走同一个渲染：示例名进得去、扩展名固定为 .srt；不合法的模板不给预览
    /// （原因由设置写入那条路径说），也不回落成默认模板去冒充预览。
    #[test]
    fn export_filename_preview_renders_or_declines() {
        let preview = preview_export_filename("{name} {format}".to_owned(), "周会".to_owned())
            .expect("合法模板有预览");
        assert_eq!(preview, "周会 srt.srt");
        assert_eq!(
            preview_export_filename("{nope}".to_owned(), "周会".to_owned()),
            None
        );
        assert_eq!(
            preview_export_filename("a/b".to_owned(), "周会".to_owned()),
            None
        );
    }

    /// 可播放媒体分类（PRD-04-058）：音频/视频白名单 + 大小写不敏感；其余一律非可播放。
    #[test]
    fn media_kind_classifies_audio_video_and_rejects_others() {
        assert_eq!(
            media_kind("录音/assets/a.wav"),
            Some(PlaybackMediaKind::Audio)
        );
        assert_eq!(media_kind("a.MP3"), Some(PlaybackMediaKind::Audio));
        assert_eq!(
            media_kind("录音/assets/talk.mp4"),
            Some(PlaybackMediaKind::Video)
        );
        assert_eq!(media_kind("talk.MOV"), Some(PlaybackMediaKind::Video));
        assert_eq!(media_kind("clip.m4v"), Some(PlaybackMediaKind::Video));
        // Webview 放不出画面 / symphonia 解不了容器 → 不假装支持。
        assert_eq!(media_kind("clip.mkv"), None);
        assert_eq!(media_kind("assets/cover.png"), None);
        assert_eq!(media_kind("noext"), None);
    }

    /// 分片表：视频与音频同为可播放分片；类型由分片派生；拼接仅 WAV 成立。
    #[test]
    fn media_parts_derive_kind_and_concatenable() {
        let wav0 = MediaRef {
            path: "录音/assets/s-p00.wav".into(),
            part_start_ms: Some(0),
            ..Default::default()
        };
        let wav1 = MediaRef {
            path: "录音/assets/s-p01.wav".into(),
            part_start_ms: Some(5_000),
            ..Default::default()
        };
        let video = MediaRef {
            path: "录音/assets/talk.mp4".into(),
            ..Default::default()
        };
        let image = MediaRef {
            path: "assets/cover.png".into(),
            ..Default::default()
        };

        let wav_parts = vec![wav1.clone(), wav0.clone()];
        let sorted = sorted_media_parts(&wav_parts);
        assert_eq!(sorted.len(), 2);
        assert_eq!(parts_media_kind(&sorted), PlaybackMediaKind::Audio);
        assert!(parts_are_concatenable(&sorted));

        let video_parts = vec![video, image];
        let sorted = sorted_media_parts(&video_parts);
        assert_eq!(sorted.len(), 1, "图片不入回放分片表");
        assert_eq!(parts_media_kind(&sorted), PlaybackMediaKind::Video);
        assert!(
            !parts_are_concatenable(&sorted),
            "视频不走 PCM 直拼（该前提只对自家 WAV 成立）"
        );
        assert!(!parts_are_concatenable(&[]), "空分片表无可拼接资源");
    }
    fn names(pairs: &[(&str, &str)]) -> std::collections::BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    /// 显示名解析（PRD-04-054）：改名映射优先 → 默认轨标签（后端 i18n）→ 原 id 兜底。
    #[test]
    fn speaker_display_name_prefers_mapping_then_i18n_then_raw() {
        let i18n = crate::i18n::BackendI18n::new("zh-CN");
        let map = names(&[("remote:1", "张三")]);
        let name = |speaker: &str,
                    map: &std::collections::BTreeMap<String, String>,
                    i18n: Option<&crate::i18n::BackendI18n>| {
            luna_vault::speaker_display_name(speaker, map, &|label| speaker_label_copy(i18n, label))
        };
        assert_eq!(name("remote:1", &map, Some(&i18n)), "张三");
        assert_eq!(name("remote:2", &map, Some(&i18n)), "对方2");
        assert_eq!(name("self", &map, Some(&i18n)), "我");
        assert_eq!(name("remote", &map, Some(&i18n)), "对方");
        // 未知格式回退原 id；无 i18n 也回退原 id。
        assert_eq!(name("alien", &map, Some(&i18n)), "alien");
        assert_eq!(name("self", &names(&[]), None), "self");
    }

    #[test]
    fn resolve_segment_speakers_rewrites_ids_and_reports_presence() {
        let i18n = crate::i18n::BackendI18n::new("en-US");
        let map = names(&[("remote:1", "Alice")]);
        let mut segs = vec![
            luna_core::Segment {
                id: luna_core::SegmentId::new(),
                start: std::time::Duration::ZERO,
                end: std::time::Duration::from_secs(1),
                text: "hi".into(),
                translation: None,
                speaker: Some("remote:1".into()),
                confidence: None,
                words: None,
            },
            luna_core::Segment {
                id: luna_core::SegmentId::new(),
                start: std::time::Duration::from_secs(1),
                end: std::time::Duration::from_secs(2),
                text: "yo".into(),
                translation: None,
                speaker: Some("self".into()),
                confidence: None,
                words: None,
            },
        ];
        assert!(resolve_segment_speakers(&mut segs, &map, Some(&i18n)));
        assert_eq!(segs[0].speaker.as_deref(), Some("Alice"));
        assert_eq!(segs[1].speaker.as_deref(), Some("Me"));

        let mut none: Vec<luna_core::Segment> = Vec::new();
        assert!(!resolve_segment_speakers(&mut none, &map, Some(&i18n)));
    }
}
