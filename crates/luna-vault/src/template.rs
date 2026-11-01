//! 模板渲染器（TDD-06 §10，归档骨架 = 内置模板）。
//!
//! 第一性原理：**"自动归档产物的 frontmatter+正文骨架" 与 "用户笔记模板" 是同一抽象**——都是
//! "把变量填进一份 md 骨架"。分两套必 DRY 违规，故统一为一个渲染器。
//!
//! - 内置模板（MVP 必有，归档要用）：`quick-note` / `recording` / `transcript` / `chat` / `note`；
//!   创作空间的 `video-project` / `script` / `swipe`（ADR-162 ②）。
//! - **`note` = 最小骨架，默认不含 `luna:` 块**（§5 lazy 策略，保 interop 干净）——普通新建笔记
//!   走它，仅当后续附加 media/归档元数据才注入 `luna:`。
//! - 带 `luna:` 块的模板**复用 [`crate::frontmatter::update_luna`]** 注入（DRY + YAML 安全转义，
//!   避免破坏 YAML/Markdown 结构，§10 变量与安全）；正文占位逐字写入。
//! - 用户自定义模板（P1）走**同一渲染器**，此处只内置默认。

use crate::error::Result;
use crate::frontmatter::{update_luna, LunaMeta, MediaRef, NoteType};
use serde::Serialize;

/// 渲染变量（来自 Job 结果 ResultSchema 与系统）。
#[derive(Clone, Debug, Default)]
pub struct TemplateVars {
    /// 创建时间（RFC3339 字符串）。
    pub created: Option<String>,
    /// 媒体映射（录音/批量归档用）。
    pub media: Vec<MediaRef>,
    /// 正文（转录正文 / 对话存档 / 用户初始内容的插入点）。
    pub body: String,
}

/// 用户模板在实例化时可使用的公开变量。
///
/// 这套小而明确的语法是 Lunarca 自己的 Markdown 模板契约；不猜测或半实现第三方
/// 插件语法。未知占位符保持原样，方便用户在模板中发现拼写错误，也不会静默丢失内容。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserTemplateVars {
    /// 笔记标题（新建时 = 最终文件名，插入时 = 当前那一篇的文件名；不含 `.md`）。
    pub title: String,
    /// 此刻（带时区的本地时间）：`{{date}}`、`{{time}}`、`{{datetime}}` 与带格式的
    /// `{{date:…}}` / `{{time:…}}` 都从它取。
    pub now: chrono::DateTime<chrono::FixedOffset>,
}

/// 渲染结果：正文与 `{{cursor}}` 所在的位置（字节偏移；模板里没写就是 `None`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderedUserTemplate {
    pub text: String,
    pub cursor: Option<usize>,
}

/// 光标占位：渲染后删掉，记下第一处的位置（插入模板后光标落在那里）。
const CURSOR: &str = "{{cursor}}";

/// 渲染用户 Markdown 模板。
///
/// 支持 `{{title}}`、`{{date}}`、`{{time}}`、`{{datetime}}`，带格式的 `{{date:YYYY-MM-DD}}`
/// / `{{time:HH:mm}}`（[`format_with_tokens`]，Obsidian 核心模板同一写法），以及
/// `{{cursor}}`（插入后光标的位置）。此函数不解析 Markdown、不触碰 frontmatter，确保用户
/// 的模板内容逐字保留，只有显式变量被替换。
#[must_use]
pub fn render_user(source: &str, vars: &UserTemplateVars) -> RenderedUserTemplate {
    let text = replace_formatted(source, "{{date:", vars);
    let text = replace_formatted(&text, "{{time:", vars);
    let text = text
        .replace("{{title}}", &vars.title)
        .replace("{{date}}", &vars.now.format("%Y-%m-%d").to_string())
        .replace("{{time}}", &vars.now.format("%H:%M").to_string())
        .replace("{{datetime}}", &vars.now.to_rfc3339());
    let cursor = text.find(CURSOR);
    RenderedUserTemplate {
        text: text.replace(CURSOR, ""),
        cursor,
    }
}

/// 把 `{{date:格式}}` / `{{time:格式}}` 换成按格式写出的此刻；没闭合的原样留下。
fn replace_formatted(source: &str, open: &str, vars: &UserTemplateVars) -> String {
    let mut out = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(at) = rest.find(open) {
        let after = &rest[at + open.len()..];
        let Some(close) = after.find("}}") else {
            break;
        };
        out.push_str(&rest[..at]);
        out.push_str(&format_with_tokens(&vars.now, &after[..close]));
        rest = &after[close + 2..];
    }
    out.push_str(rest);
    out
}

/// 按 Moment 风格的记号写日期时间（Obsidian 模板的通行写法）：`YYYY` `YY` `MM` `M` `DD` `D`
/// `HH` `H` `mm` `m` `ss` `s`；`[文字]` 原样输出；其余字符照抄。只认这十二个记号——够写日期
/// 与时间，又不至于把模板里的普通字母当成记号吃掉。
#[must_use]
pub fn format_with_tokens(now: &chrono::DateTime<chrono::FixedOffset>, pattern: &str) -> String {
    use chrono::{Datelike, Timelike};
    const TOKENS: [&str; 12] = [
        "YYYY", "YY", "MM", "M", "DD", "D", "HH", "H", "mm", "m", "ss", "s",
    ];
    let mut out = String::new();
    let mut rest = pattern;
    while !rest.is_empty() {
        if let Some(literal) = rest.strip_prefix('[') {
            if let Some(end) = literal.find(']') {
                out.push_str(&literal[..end]);
                rest = &literal[end + 1..];
                continue;
            }
        }
        let Some(token) = TOKENS.iter().find(|token| rest.starts_with(**token)) else {
            let ch = rest.chars().next().expect("非空");
            out.push(ch);
            rest = &rest[ch.len_utf8()..];
            continue;
        };
        out.push_str(&match *token {
            "YYYY" => format!("{:04}", now.year()),
            "YY" => format!("{:02}", now.year().rem_euclid(100)),
            "MM" => format!("{:02}", now.month()),
            "M" => now.month().to_string(),
            "DD" => format!("{:02}", now.day()),
            "D" => now.day().to_string(),
            "HH" => format!("{:02}", now.hour()),
            "H" => now.hour().to_string(),
            "mm" => format!("{:02}", now.minute()),
            "m" => now.minute().to_string(),
            "ss" => format!("{:02}", now.second()),
            _ => now.second().to_string(),
        });
        rest = &rest[token.len()..];
    }
    out
}

/// 按功能类型渲染一份归档/新建笔记的完整 md 内容。
///
/// - [`NoteType::Note`]：**不注入 `luna:` 块**（lazy），直接返回正文（普通新建笔记）。
/// - 其它类型：构造 `luna:` 元数据并经 [`update_luna`] 注入到正文之上（YAML 安全）。
pub fn render(kind: NoteType, vars: &TemplateVars) -> Result<String> {
    if matches!(kind, NoteType::Note) {
        // 最小骨架：标题即文件名（不写 frontmatter title，§3），正文原样。
        return Ok(vars.body.clone());
    }
    let meta = LunaMeta {
        schema: crate::frontmatter::LUNA_SCHEMA,
        note_type: kind,
        created: vars.created.clone(),
        media: vars.media.clone(),
        bookmarks: Vec::new(),
        speakers: std::collections::BTreeMap::new(),
        ai_tags: Vec::new(),
        ai_tags_src: None,
        // 来源在建笔记时尚未确定（抓取任务建好笔记后经 `set_source_url` 补写），
        // 且本地文件导入永远没有来源——故模板不接受它作入参。
        source_url: None,
        // 蒸馏指针由 `Vault::write_distilled` 在生成时写入，模板阶段一律为空
        // （新建的资料笔记天然还没有蒸馏笔记，PRD-06-044）。
        // 新建的视频项目一律从第一列「选题」开始（ADR-162 ②）；别的类型没有这个键。
        project_stage: matches!(kind, NoteType::VideoProject)
            .then(|| crate::ProjectStage::Idea.key().to_owned()),
        ..LunaMeta::default()
    };
    // 复用 §4.4 局部改写：无 frontmatter 的正文 → 生成 `---\nluna:…\n---\n` 前缀 + 正文。
    update_luna(&vars.body, &meta)
}

/// 内置模板信息。
///
/// `id` 是 `NoteType` 而不是 `String`：它本来就恒等于某个笔记类型，写成字符串等于
/// 把这个事实降级成散文（此前正是靠一行注释说「其 `id` 对应 `NoteType` 的 kebab 形式」
/// 来维持的），前端拿到 `string` 只能 `as NoteTypeDto` 蒙过去——那个 `as` 一旦蒙错，
/// 编译期什么都不查，运行时是「新建这个模板永远失败」。类型化之后 wire 取值由 serde
/// 产生，两侧再没有第二处需要手工对齐的 kebab 形式。
#[derive(Debug, Clone, Serialize)]
pub struct BuiltinTemplateInfo {
    pub id: NoteType,
    pub description: String,
}

/// 列出所有内置模板（每个笔记类型一个，与 `vault_create` 的 `noteType` 参数同一词表）。
#[must_use]
pub fn list_builtin() -> Vec<BuiltinTemplateInfo> {
    vec![
        BuiltinTemplateInfo {
            id: NoteType::Note,
            description: "空白笔记".into(),
        },
        BuiltinTemplateInfo {
            id: NoteType::QuickNote,
            description: "语音速记".into(),
        },
        BuiltinTemplateInfo {
            id: NoteType::Recording,
            description: "录音归档".into(),
        },
        BuiltinTemplateInfo {
            id: NoteType::Transcript,
            description: "媒体转写".into(),
        },
        BuiltinTemplateInfo {
            id: NoteType::Chat,
            description: "对话存档".into(),
        },
        BuiltinTemplateInfo {
            id: NoteType::Document,
            description: "导入文档".into(),
        },
        BuiltinTemplateInfo {
            id: NoteType::VideoProject,
            description: "视频项目".into(),
        },
        BuiltinTemplateInfo {
            id: NoteType::Script,
            description: "视频脚本".into(),
        },
        BuiltinTemplateInfo {
            id: NoteType::Swipe,
            description: "灵感条目".into(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontmatter::read_luna;

    #[test]
    fn note_template_has_no_luna_block() {
        let out = render(
            NoteType::Note,
            &TemplateVars {
                body: "# 未命名\n\n".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!out.contains("luna:"), "普通笔记不应注入 luna: 块：{out}");
        assert!(!out.starts_with("---"));
        assert_eq!(read_luna(&out).note_type, NoteType::Note);
    }

    #[test]
    fn recording_template_carries_luna_and_media() {
        let vars = TemplateVars {
            created: Some("2026-05-28T10:00:00+08:00".into()),
            media: vec![MediaRef {
                path: "Recordings/会议-2026-05-28.wav".into(),
                duration: Some(3600.0),
                transcript: true,
                segments: Some("Recordings/会议-2026-05-28.segments.json".into()),
                ..Default::default()
            }],
            body: "# 会议纪要\n\n[00:00:12.480] 大家好".into(),
        };
        let out = render(NoteType::Recording, &vars).unwrap();
        let meta = read_luna(&out);
        assert_eq!(meta.note_type, NoteType::Recording);
        assert_eq!(meta.created.as_deref(), Some("2026-05-28T10:00:00+08:00"));
        assert_eq!(meta.media.len(), 1);
        assert!(meta.media[0].transcript);
        assert!(
            meta.media[0].segments.is_some(),
            "recording 模板应含 segments 边车路径"
        );
        // 正文占位逐字保留。
        assert!(out.contains("# 会议纪要\n\n[00:00:12.480] 大家好"));
    }

    #[test]
    fn recording_template_multi_part_interval_roundtrip() {
        // 多 part 录音：part_start_ms/part_end_ms 经 frontmatter round-trip 后与写入值一致。
        let vars = TemplateVars {
            created: Some("2026-06-08T09:00:00+08:00".into()),
            media: vec![
                MediaRef {
                    path: "Recordings/session-001.wav".into(),
                    transcript: true,
                    segments: Some(".luna/sidecars/session-001.segments.json".into()),
                    part_start_ms: Some(0),
                    part_end_ms: Some(5000),
                    ..Default::default()
                },
                MediaRef {
                    path: "Recordings/session-002.wav".into(),
                    transcript: true,
                    segments: Some(".luna/sidecars/session-002.segments.json".into()),
                    part_start_ms: Some(5000),
                    part_end_ms: Some(10000),
                    ..Default::default()
                },
            ],
            body: "# 录音\n\n[00:00:01.000] 第一段".into(),
        };
        let out = render(NoteType::Recording, &vars).unwrap();
        let meta = read_luna(&out);
        assert_eq!(meta.media.len(), 2);

        // 分片 1 区间与边车路径 round-trip。
        assert_eq!(meta.media[0].part_start_ms, Some(0));
        assert_eq!(meta.media[0].part_end_ms, Some(5000));
        assert_eq!(
            meta.media[0].segments.as_deref(),
            Some(".luna/sidecars/session-001.segments.json")
        );

        // 分片 2 区间与边车路径 round-trip。
        assert_eq!(meta.media[1].part_start_ms, Some(5000));
        assert_eq!(meta.media[1].part_end_ms, Some(10000));
        assert_eq!(
            meta.media[1].segments.as_deref(),
            Some(".luna/sidecars/session-002.segments.json")
        );

        // 正文逐字保留。
        assert!(out.contains("[00:00:01.000] 第一段"));
    }

    #[test]
    fn chat_template_type() {
        let out = render(
            NoteType::Chat,
            &TemplateVars {
                body: "## 用户\n问题\n\n## 助手\n回答".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(read_luna(&out).note_type, NoteType::Chat);
        assert!(out.contains("## 助手\n回答"));
    }

    #[test]
    fn yaml_special_chars_in_media_path_are_escaped() {
        // 含冒号/引号的路径不得破坏 YAML——经 serde_yaml 序列化自动转义。
        let vars = TemplateVars {
            media: vec![MediaRef {
                path: "录音: \"特殊\".wav".into(),
                ..Default::default()
            }],
            body: "正文".into(),
            ..Default::default()
        };
        let out = render(NoteType::QuickNote, &vars).unwrap();
        // 回读应还原出原始路径（转义正确）。
        let meta = read_luna(&out);
        assert_eq!(meta.media[0].path, "录音: \"特殊\".wav");
    }

    /// 每个笔记类型**恰好**一个内置模板，判据取自 `NoteType::ALL` 而不是一份手抄的
    /// kebab 字符串清单：此前这条用例逐个 `assert!(ids.contains("quick-note"))`，于是新增
    /// 一个笔记类型时它照样全绿——而那个类型在「新建笔记」的模板列表里根本不出现。
    #[test]
    fn every_note_type_has_exactly_one_builtin_template() {
        let ids: Vec<NoteType> = list_builtin().iter().map(|t| t.id).collect();
        assert_eq!(ids.len(), NoteType::ALL.len(), "模板数与笔记类型数对不上");
        for note_type in NoteType::ALL {
            assert_eq!(
                ids.iter().filter(|id| **id == note_type).count(),
                1,
                "{note_type:?} 的内置模板不是恰好一个"
            );
        }
    }

    #[test]
    fn every_builtin_template_says_what_it_is() {
        for template in list_builtin() {
            assert!(
                !template.description.trim().is_empty(),
                "{:?} 的模板没有说明——模板选择器里它就是一行空白",
                template.id
            );
        }
    }

    fn vars() -> UserTemplateVars {
        UserTemplateVars {
            title: "周会".into(),
            now: chrono::DateTime::parse_from_rfc3339("2026-07-08T09:05:03-04:00").unwrap(),
        }
    }

    #[test]
    fn user_template_replaces_only_documented_variables() {
        let rendered = render_user(
            "# {{title}}\n\n{{date}} {{time}} {{datetime}}\n{{unknown}}",
            &vars(),
        );
        assert!(rendered.text.contains("# 周会"));
        assert!(rendered
            .text
            .contains("2026-07-08 09:05 2026-07-08T09:05:03-04:00"));
        assert!(rendered.text.ends_with("{{unknown}}"));
        assert_eq!(rendered.cursor, None);
    }

    #[test]
    fn formatted_dates_and_the_cursor_marker() {
        let rendered = render_user(
            "{{date:YYYY年M月D日}} {{time:HH:mm:ss}} {{date:[第]YY[期]}}\n- {{cursor}}\n{{date:没闭合",
            &vars(),
        );
        assert_eq!(
            rendered.text,
            "2026年7月8日 09:05:03 第26期\n- \n{{date:没闭合"
        );
        assert_eq!(
            rendered.cursor,
            Some("2026年7月8日 09:05:03 第26期\n- ".len())
        );
    }
}
