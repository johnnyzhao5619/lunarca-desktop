//! 耐久边车 `<media>.segments.json`（TDD-06 §2.1）。
//!
//! 精确分段（毫秒级 `start/end`、词级、置信度）是 ASR 一次性产物，**无法从 md 秒级行标记反推**，
//! 故边车属 **durable·不可重建** 事实源（清缓存绝不动它）；SQLite `segments` 表才是其可重建派生。
//!
//! 三项纪律（与其它不可重建数据同级）：
//! - **带 `schema` 版本**：读到落后版本按注册迁移升级；读到未知更高版本 / 迁移失败 → **保守降级为
//!   行标记级**（[`SidecarLoad::Degraded`]），绝不丢弃或强改。
//! - **原子写**（§4.3，复用 [`crate::atomic::write_atomic`]）：与 md 同为事实源，绝不写半截。
//! - **降级链**：边车存在 → 精确高亮 + 端点 seek；缺失 / 降级 → 仅凭 md 行标记退化为行级 seek，
//!   功能可用不报错（[`SidecarLoad::Missing`]）。

use crate::atomic::write_atomic;
use crate::error::Result;
use crate::hashing::ContentHash;
use luna_core::Segment;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// 当前边车结构版本。
pub const SIDECAR_SCHEMA: u32 = 1;

/// 边车内容（durable 事实源）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sidecar {
    /// 结构版本（顶层，迁移用）。
    pub schema: u32,
    /// 完整分段（精确区间 + 富信息）。
    pub segments: Vec<Segment>,
}

impl Sidecar {
    /// 以当前 schema 包装一组分段。
    ///
    /// **词级时间戳随段持久化（TDD-03 §10.4，2026-07-24 修订）**：早先「写盘统一剥离
    /// `Segment.words`」的落盘卫生假设了「words 只服务转写当场的骑跨切开」——该假设已被
    /// 会后编辑推翻：① 手动「说话人细分」重跑读的是**边车**段，剥掉 words 后骑跨切开
    /// 永远退化为整段归属（用户看到的正是「明明识别出第二个说话人却拆不开」）；
    /// ② 人工「在光标处拆分」要把字符位置映射回时间，无 words 只能按字数线性内插。
    /// 因此 words 成为盘上一等公民；文本被改写的段由改写方清空 words（词表描述的是
    /// 「那段文本」的时间，文本一变即失效，见 `Vault::update_playback_*`）。
    #[must_use]
    pub fn new(segments: Vec<Segment>) -> Self {
        Self {
            schema: SIDECAR_SCHEMA,
            segments,
        }
    }
}

/// 读取边车的结果（降级链，§2.1）。
#[derive(Clone, Debug, PartialEq)]
pub enum SidecarLoad {
    /// 成功（已迁移到当前 schema）。
    Ok(Sidecar),
    /// 降级：未知更高版本 / 解析失败 / 迁移失败 → 退化为行标记级（不丢弃、不强改）。
    Degraded {
        /// 降级原因（诊断用，不含正文内容）。
        reason: String,
    },
    /// 边车缺失（旧笔记 / 外部新建）→ 退化为行标记级。
    Missing,
}

/// 解析边车字节（**纯函数**，headless 重测）。处理迁移与降级，绝不 panic。
#[must_use]
pub fn parse_sidecar(bytes: &[u8]) -> SidecarLoad {
    // 先只取顶层 schema，避免低版本结构与当前 struct 不兼容导致整体解析失败。
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return SidecarLoad::Degraded {
            reason: "JSON 解析失败".into(),
        };
    };
    let schema = value
        .get("schema")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as u32;

    if schema != SIDECAR_SCHEMA {
        // 不读取或迁移旧写入格式；非当前 Lunarca schema 统一保守降级。
        return SidecarLoad::Degraded {
            reason: format!("边车 schema {schema} 不等于当前 {SIDECAR_SCHEMA}"),
        };
    }

    match serde_json::from_value::<Sidecar>(value) {
        Ok(sidecar) => SidecarLoad::Ok(sidecar),
        Err(_) => SidecarLoad::Degraded {
            reason: format!("边车 schema {schema} 解析失败"),
        },
    }
}

/// 原子写边车（§4.3）。父目录须已存在（调用方负责）。
pub fn write_sidecar(path: &Path, sidecar: &Sidecar) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(sidecar)
        .map_err(|e| crate::error::VaultError::Decode(e.to_string()))?;
    write_atomic(path, &bytes)
}

/// 读取边车文件，套用降级链：文件不存在 → [`SidecarLoad::Missing`]；存在则交 [`parse_sidecar`]。
#[must_use]
pub fn read_sidecar(path: &Path) -> SidecarLoad {
    match std::fs::read(path) {
        Ok(bytes) => parse_sidecar(&bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => SidecarLoad::Missing,
        Err(e) => SidecarLoad::Degraded {
            reason: format!("读取失败：{e}"),
        },
    }
}

/// 边车存储目录前缀（TDD-06 §2.1：`.luna/sidecars/`，durable·设备本地）。
const SIDECARS_DIR: &str = ".luna/sidecars";

/// 删除一个边车文件（不存在即空操作——目标状态已达成）。
///
/// 维护「**边车文件与段落一一对应**」这条不变量的另一半：读取端只认文件在不在
/// （无媒体槽没有 `media.segments` 那样的标记可查），故"逻辑上删掉段落"必须落到物理删除，
/// 否则下次加载会把删掉的段落读回来。
///
/// # Errors
/// 删除失败（权限等）时返回 IO 错误；文件不存在**不算失败**。
pub fn remove_sidecar_file(path: &Path) -> crate::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// 从媒体相对路径派生边车存储路径（`.luna/sidecars/<hash8>.segments.json`）。
///
/// hash8 = `ContentHash::of(media_rel.as_bytes())` 前 8 字符，避免同 stem 不同目录碰撞。
/// 读写都只认这一条 Lunarca 路径；不存在即按正常降级链返回 [`SidecarLoad::Missing`]。
#[must_use]
pub fn sidecar_path_for(media_rel: &str) -> String {
    let hash8: String = ContentHash::of(media_rel.as_bytes())
        .as_str()
        .chars()
        .take(8)
        .collect();
    format!("{SIDECARS_DIR}/{hash8}.segments.json")
}

/// 按媒体相对路径读取 Lunarca 边车。调用方须先用 frontmatter `media[].segments` 判断
/// `Some`/`None`；字段值不参与路径决策，路径由 `media_rel` 确定性推导。
#[must_use]
pub fn read_sidecar_for_media(root: &Path, media_rel: &str) -> SidecarLoad {
    read_sidecar(&root.join(sidecar_path_for(media_rel)))
}

/// 转写段的**挂载槽**：一篇笔记的段落分别存在哪些边车里（读写的单一口径）。
///
/// 为什么需要这个抽象：转写段此前一律"挂在媒体分片上"（多分片录音天然多个边车），
/// 于是**没有本地媒体就没有地方放段落**。而「仅存链接」的转写（PRD-05-006 的省磁盘档）
/// 恰恰是"有时间轴文稿、没有本地媒体"——它的时间轴锚定的是**源视频**，那个身份就是 URL。
///
/// 边车寻址本就只是"把一个字符串哈希成路径"（[`sidecar_path_for`]），故用来源地址作
/// 逻辑身份天然成立，无需为它另造一套存储。
///
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranscriptSlot {
    /// 边车寻址键：本地媒体的相对路径，或（无本地媒体时）内容来源地址。
    pub key: String,
    /// 该槽在会话时间轴上的起点（多分片录音才有；`None` = 整篇归属本槽）。
    pub part_start_ms: Option<u64>,
    /// 该槽在会话时间轴上的止点。
    pub part_end_ms: Option<u64>,
}

/// 求一篇笔记的转写段挂载槽（**读写共用**，绝不各自推导一遍）。
///
/// - 有本地媒体分片 → 每个分片一个槽（保持多分片录音的既有行为不变）；
/// - 无本地媒体但有 `luna.source_url` → **单槽挂在来源地址上**（仅存链接的转写）；
/// - 两者皆无 → 空（这篇笔记没有可归属的转写段，调用方据此如实为空）。
#[must_use]
pub fn transcript_slots(meta: &crate::LunaMeta) -> Vec<TranscriptSlot> {
    if !meta.media.is_empty() {
        return meta
            .media
            .iter()
            .map(|media| TranscriptSlot {
                key: media.path.clone(),
                part_start_ms: media.part_start_ms,
                part_end_ms: media.part_end_ms,
            })
            .collect();
    }
    meta.source_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(|url| {
            vec![TranscriptSlot {
                key: url.to_owned(),
                part_start_ms: None,
                part_end_ms: None,
            }]
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LunaMeta, MediaRef};
    use luna_core::SegmentId;
    use std::time::Duration;

    /// 有本地媒体 → 每个分片一个槽（多分片录音的既有行为不得变）。
    #[test]
    fn slots_follow_media_parts_when_present() {
        let meta = LunaMeta {
            media: vec![
                MediaRef {
                    path: "R/a-p01.wav".into(),
                    part_start_ms: Some(0),
                    part_end_ms: Some(1000),
                    ..Default::default()
                },
                MediaRef {
                    path: "R/a-p02.wav".into(),
                    part_start_ms: Some(1000),
                    part_end_ms: Some(2000),
                    ..Default::default()
                },
            ],
            source_url: Some("https://a.com/v".into()),
            ..Default::default()
        };
        let slots = transcript_slots(&meta);
        assert_eq!(slots.len(), 2);
        assert_eq!(slots[0].key, "R/a-p01.wav");
        assert_eq!(slots[1].part_start_ms, Some(1000));
    }

    /// **无本地媒体但有来源地址** → 单槽挂来源（仅存链接的转写，PRD-05-006）。
    /// 时间轴锚定的是源视频，故整篇归属这一个槽（无分片区间）。
    #[test]
    fn slots_fall_back_to_source_url_without_media() {
        let meta = LunaMeta {
            source_url: Some("  https://www.youtube.com/watch?v=abc  ".into()),
            ..Default::default()
        };
        let slots = transcript_slots(&meta);
        assert_eq!(slots.len(), 1);
        assert_eq!(
            slots[0].key, "https://www.youtube.com/watch?v=abc",
            "键取去空白后的地址——它必须与写入时完全一致，否则读不回来"
        );
        assert_eq!(slots[0].part_start_ms, None);
    }

    /// 两者皆无 → 空槽（如实为空，而不是造一个假键把段落写到无人找得到的地方）。
    #[test]
    fn slots_are_empty_without_media_or_source() {
        assert!(transcript_slots(&LunaMeta::default()).is_empty());
        let blank = LunaMeta {
            source_url: Some("   ".into()),
            ..Default::default()
        };
        assert!(transcript_slots(&blank).is_empty());
    }

    /// 同一个键读写必须落到同一个边车文件（URL 与本地路径在寻址上无差别）。
    #[test]
    fn url_keys_address_sidecars_like_paths_do() {
        let url = "https://a.com/v?id=1";
        assert_eq!(sidecar_path_for(url), sidecar_path_for(url));
        assert_ne!(
            sidecar_path_for(url),
            sidecar_path_for("https://a.com/v?id=2")
        );
        assert!(sidecar_path_for(url).starts_with(SIDECARS_DIR));
    }

    fn seg(start_ms: u64, end_ms: u64, text: &str) -> Segment {
        Segment {
            id: SegmentId::new(),
            start: Duration::from_millis(start_ms),
            end: Duration::from_millis(end_ms),
            text: text.into(),
            translation: None,
            speaker: None,
            confidence: None,
            words: None,
        }
    }

    #[test]
    fn new_keeps_word_timings_for_post_hoc_editing() {
        // TDD-03 §10.4：words 是会后编辑（重跑细分的骑跨切开 / 光标处拆分定时）的输入，
        // 必须随段落盘——剥离即让这两件能力永久退化。
        let mut s = seg(0, 1_000, "hello world");
        s.words = Some(vec![luna_core::WordTiming {
            text: " hello".into(),
            start: Duration::ZERO,
            end: Duration::from_millis(500),
        }]);
        let sc = Sidecar::new(vec![s]);
        assert_eq!(
            sc.segments[0].words.as_ref().map(Vec::len),
            Some(1),
            "词级时间戳须持久化"
        );
    }

    #[test]
    fn round_trip_current_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rec.segments.json");
        let sc = Sidecar::new(vec![seg(0, 1200, "大家好"), seg(1200, 3400, "开始评审")]);
        write_sidecar(&path, &sc).unwrap();
        match read_sidecar(&path) {
            SidecarLoad::Ok(loaded) => assert_eq!(loaded, sc),
            other => panic!("期望 Ok，得 {other:?}"),
        }
    }

    #[test]
    fn missing_file_degrades_to_line_level() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            read_sidecar(&dir.path().join("none.segments.json")),
            SidecarLoad::Missing
        );
    }

    #[test]
    fn higher_schema_degrades_not_destroyed() {
        // 用户用更新版 Lunarca 写过（schema 99）→ 只读降级，不强改。
        let bytes = br#"{"schema":99,"segments":[],"future_field":1}"#;
        match parse_sidecar(bytes) {
            SidecarLoad::Degraded { reason } => assert!(reason.contains("99")),
            other => panic!("期望 Degraded，得 {other:?}"),
        }
    }

    #[test]
    fn schemaless_sidecar_is_not_migrated() {
        // 清洁切换：最初无 schema 的旧边车不读取、不迁移。
        let s = seg(0, 500, "x");
        let arr = serde_json::to_vec(&vec![s]).unwrap();
        assert!(matches!(parse_sidecar(&arr), SidecarLoad::Degraded { .. }));
    }

    #[test]
    fn garbage_degrades() {
        assert!(matches!(
            parse_sidecar(b"not json at all"),
            SidecarLoad::Degraded { .. }
        ));
    }

    #[test]
    fn reads_hash_named_luna_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let media_rel = "Recordings/meeting.wav";
        let sc = Sidecar::new(vec![seg(0, 800, "hash 主路径命中")]);

        let primary = root.join(sidecar_path_for(media_rel));
        std::fs::create_dir_all(primary.parent().unwrap()).unwrap();
        write_sidecar(&primary, &sc).unwrap();

        match read_sidecar_for_media(root, media_rel) {
            SidecarLoad::Ok(loaded) => assert_eq!(loaded, sc),
            other => panic!("期望 Ok（hash 主路径），得 {other:?}"),
        }
    }

    #[test]
    fn old_luna_sidecar_is_not_used_when_luna_file_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let media_rel = "Recordings/meeting.wav";
        let sc = Sidecar::new(vec![seg(0, 1500, "stem 降级兜底命中")]);

        let legacy = root.join(".luna/sidecars/meeting.segments.json");
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        write_sidecar(&legacy, &sc).unwrap();

        assert_eq!(
            read_sidecar_for_media(root, media_rel),
            SidecarLoad::Missing
        );
    }

    #[test]
    fn returns_missing_when_luna_path_is_absent() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let media_rel = "Recordings/silent.wav";

        std::fs::create_dir_all(root.join(SIDECARS_DIR)).unwrap();

        assert_eq!(
            read_sidecar_for_media(root, media_rel),
            SidecarLoad::Missing
        );
    }

    #[test]
    fn luna_path_degradation_does_not_probe_legacy_sidecar() {
        // 主路径 Degraded（文件存在但 schema 异常）不应换路径走 stem——Degraded
        // 表示"用户用更新版 Lunarca 写过"，不应静默降级到旧 stem 文件。
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let media_rel = "Recordings/upgraded.wav";

        let primary = root.join(sidecar_path_for(media_rel));
        std::fs::create_dir_all(primary.parent().unwrap()).unwrap();
        std::fs::write(&primary, br#"{"schema":99,"segments":[]}"#).unwrap();

        let legacy = root.join(".luna/sidecars/upgraded.segments.json");
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        let sc = Sidecar::new(vec![seg(0, 500, "stem 文件应被忽略")]);
        write_sidecar(&legacy, &sc).unwrap();

        match read_sidecar_for_media(root, media_rel) {
            SidecarLoad::Degraded { reason } => assert!(reason.contains("99")),
            other => panic!("期望 Degraded（主路径优先、不走 stem），得 {other:?}"),
        }
    }
}

// ─── 转写段的跨消费点共享（回放 / 导出 / 蒸馏 / CLI，TDD-09 §1 复用核心） ────────────

/// 逐槽读耐久边车并合并为绝对时钟升序段列表。
///
/// 降级链与回放一致：边车缺失 / 损坏 → 该槽 0 段（诚实元数据）。挂载槽 = 有本地媒体走
/// 各分片、无本地媒体走来源地址（[`transcript_slots`] 是读写单一口径），于是「仅存链接」
/// 的转写在所有消费点自动取得段落。此前它住在 Tauri 层，CLI 复用不了（PRD-11-006）。
#[must_use]
pub fn note_segments(root: &Path, meta: &crate::LunaMeta) -> Vec<luna_core::Segment> {
    let mut segments: Vec<luna_core::Segment> = Vec::new();
    for slot in transcript_slots(meta) {
        if let SidecarLoad::Ok(sidecar) = read_sidecar_for_media(root, &slot.key) {
            segments.extend(sidecar.segments);
        }
    }
    segments.sort_by_key(|s| s.start);
    segments
}

/// 说话人 id 的**结构**（`self` / `remote` / `remote:N` / `speaker:N` / 其它）。
/// 解析只在这里做一次；把它翻成人话（「我」「对方 2」）的文案归调用方——GUI 走后端
/// i18n，CLI 没有文案表就原样输出 id。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpeakerLabel<'a> {
    SelfTrack,
    Remote,
    RemoteN(&'a str),
    SpeakerN(&'a str),
    Other(&'a str),
}

impl<'a> SpeakerLabel<'a> {
    #[must_use]
    pub fn parse(speaker: &'a str) -> Self {
        let numbered = |prefix: &str| {
            speaker
                .strip_prefix(prefix)
                .filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        };
        match speaker {
            "self" => Self::SelfTrack,
            "remote" => Self::Remote,
            other => {
                if let Some(n) = numbered("remote:") {
                    Self::RemoteN(n)
                } else if let Some(n) = numbered("speaker:") {
                    Self::SpeakerN(n)
                } else {
                    Self::Other(other)
                }
            }
        }
    }
}

/// 说话人显示名：改名映射优先 → 调用方文案（`label`）→ 原样 id。
#[must_use]
pub fn speaker_display_name(
    speaker: &str,
    names: &std::collections::BTreeMap<String, String>,
    label: &dyn Fn(SpeakerLabel<'_>) -> Option<String>,
) -> String {
    if let Some(name) = names.get(speaker).filter(|n| !n.trim().is_empty()) {
        return name.clone();
    }
    label(SpeakerLabel::parse(speaker)).unwrap_or_else(|| speaker.to_owned())
}

/// 把段上的说话人 id 就地换成显示名；返回是否有任何说话人（导出据此决定要不要前缀）。
pub fn resolve_segment_speakers(
    segments: &mut [luna_core::Segment],
    names: &std::collections::BTreeMap<String, String>,
    label: &dyn Fn(SpeakerLabel<'_>) -> Option<String>,
) -> bool {
    let mut any = false;
    for seg in segments.iter_mut() {
        if let Some(speaker) = seg.speaker.take() {
            let trimmed = speaker.trim();
            if trimmed.is_empty() {
                continue;
            }
            seg.speaker = Some(speaker_display_name(trimmed, names, label));
            any = true;
        }
    }
    any
}

#[cfg(test)]
mod speaker_tests {
    use super::*;

    #[test]
    fn speaker_label_parses_the_four_shapes() {
        assert_eq!(SpeakerLabel::parse("self"), SpeakerLabel::SelfTrack);
        assert_eq!(SpeakerLabel::parse("remote"), SpeakerLabel::Remote);
        assert_eq!(SpeakerLabel::parse("remote:3"), SpeakerLabel::RemoteN("3"));
        assert_eq!(
            SpeakerLabel::parse("speaker:12"),
            SpeakerLabel::SpeakerN("12")
        );
        assert_eq!(
            SpeakerLabel::parse("remote:x"),
            SpeakerLabel::Other("remote:x")
        );
    }

    #[test]
    fn display_name_prefers_mapping_then_label_then_raw() {
        let mut names = std::collections::BTreeMap::new();
        names.insert("remote:1".to_owned(), "张三".to_owned());
        let label = |l: SpeakerLabel<'_>| match l {
            SpeakerLabel::SelfTrack => Some("我".to_owned()),
            _ => None,
        };
        assert_eq!(speaker_display_name("remote:1", &names, &label), "张三");
        assert_eq!(speaker_display_name("self", &names, &label), "我");
        assert_eq!(speaker_display_name("remote:2", &names, &label), "remote:2");
    }
}
