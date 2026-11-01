//! 二次分发（ADR-165 ⑧，PRD-19-024 / 025）：把一期视频改写成博客文章、邮件简报、推文串，
//! 或从成片转写里挑出几段短视频（给入点、出点与短视频脚本，不剪视频）。
//!
//! 每一份都落成项目文件夹里的一篇笔记，开头一行链回项目主页；名字是 `<项目>-<渠道>`（通用的
//! 一份是 `<项目>-<种类>`），「分发」页签按这个名字列出已有的（与发布笔记 `<项目>-发布` 同一种
//! 查询投影，不加新字段）。
//!
//! **按目标渠道写**（ADR-166 ④ S3）：项目里每个文章 / 帖子 / 简报 / 短视频渠道各一个目标，没有
//! 那种形态的渠道时给一个通用目标；提示词带上渠道名、上限句与风格句。已有的那份每次列出时按
//! 渠道的上限重查一遍（只查有出处的上限；X 按它自己的加权字数）。
//! AI 走单一主干（`Summarize` + 注册表提示词，ADR-001）；短视频片段的入点出点由这里对齐到
//! 转写行首并校验时长——模型给的时间点只是候选（ADR-003：判定在 Rust）。

use super::creator_publish::{
    channel_label, channel_of, channel_style, load_source_transcript, project_folder,
    run_publish_job_with, run_summarize_job_with, CreatorNotes,
};
use super::*;
use luna_core::chapters::{format_timestamp, parse_timestamp, snap_to_nearest};
use luna_vault::channel::{Channel, ChannelFormat};

/// 短视频片段的时长下限与上限（毫秒）：短于此不成一条，长于此就不是短视频了。
const CLIP_MIN_MS: u64 = 10_000;
const CLIP_MAX_MS: u64 = 180_000;
/// 不指定渠道的推文串每条多长（此前通用推文串一直按 X 的 280 写）。
const GENERIC_POST_MAX: u32 = 280;
/// 一次最多留几段（提示词要 2–5 段）。
const CLIPS_MAX: usize = 5;

/// 分发的种类（顺序即「分发」页签上的顺序）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum DistributionKindDto {
    Blog,
    Newsletter,
    Thread,
    Shorts,
}

impl DistributionKindDto {
    const ALL: [Self; 4] = [Self::Blog, Self::Newsletter, Self::Thread, Self::Shorts];

    /// 注册表里的提示词。
    const fn template(self) -> &'static str {
        match self {
            Self::Blog => "creator.distribute.blog",
            Self::Newsletter => "creator.distribute.newsletter",
            Self::Thread => "creator.distribute.thread",
            Self::Shorts => "creator.distribute.shorts",
        }
    }

    /// 笔记名里的那个词（随界面语言）。
    fn label(self, copy: BackendCopy<'_>) -> String {
        let (key, fallback) = match self {
            Self::Blog => ("creator-distribute-blog", "博客"),
            Self::Newsletter => ("creator-distribute-newsletter", "简报"),
            Self::Thread => ("creator-distribute-thread", "推文串"),
            Self::Shorts => ("creator-distribute-shorts", "短视频"),
        };
        backend_text_with(copy, key, fallback)
    }

    /// 渠道形态 → 改写成哪一种；长视频与音频走发布页签，不在这里。
    const fn of_format(format: ChannelFormat) -> Option<Self> {
        match format {
            ChannelFormat::Article => Some(Self::Blog),
            ChannelFormat::Newsletter => Some(Self::Newsletter),
            ChannelFormat::Post => Some(Self::Thread),
            ChannelFormat::ShortVideo => Some(Self::Shorts),
            ChannelFormat::LongVideo | ChannelFormat::Audio => None,
        }
    }

    /// 输出预算（博客最长，推文串最短）。
    const fn output_tokens(self) -> u32 {
        match self {
            Self::Blog => 4096,
            Self::Newsletter => 2048,
            Self::Thread => 1536,
            Self::Shorts => 3072,
        }
    }
}

/// 改写的来源：项目的脚本，或项目里的一篇成片转写。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DistributionSourceDto {
    Script,
    Transcript { path: String },
}

/// 分发的一个目标：一种形态，发到项目里的哪个渠道（`None` = 不指定渠道的通用一份）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct DistributionTargetDto {
    pub kind: DistributionKindDto,
    pub channel: Option<String>,
}

/// 已生成的一份超出了目标渠道的上限（序号从 1 起；`max` 是这个渠道的上限）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DistributionIssueDto {
    /// 第几条帖子太长（`weighted`：这个渠道把中日韩文字算两个，提示里要说明）。
    Post {
        post: u32,
        over: u32,
        max: u32,
        weighted: bool,
    },
    /// 标题（第一个一级标题）太长。
    Title { over: u32, max: u32 },
    /// 全文太长。
    Body { over: u32, max: u32 },
}

/// 已经生成过的一份。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct DistributionOutputDto {
    pub kind: DistributionKindDto,
    /// 写给哪个渠道的（通用的一份为 `None`）。
    pub channel: Option<String>,
    pub path: String,
    /// 按那个渠道的上限重查的结果（通用的一份不查）。
    pub issues: Vec<DistributionIssueDto>,
}

/// 「分发」页签要的东西：有没有脚本、有哪些目标、已经生成过哪些。
#[derive(Debug, Clone, Serialize, Type)]
pub struct DistributionDto {
    /// 项目的脚本（没有为 `None`，界面上「从脚本改写」不可选）。
    pub script: Option<String>,
    /// 按种类排，同种类按项目渠道的顺序。
    pub targets: Vec<DistributionTargetDto>,
    pub outputs: Vec<DistributionOutputDto>,
}

/// 项目的渠道 → 分发目标：每种形态，项目里有那种形态的渠道就各一个，没有就一个通用的。
fn distribution_targets(channels: &[Channel]) -> Vec<DistributionTargetDto> {
    DistributionKindDto::ALL
        .into_iter()
        .flat_map(|kind| {
            let matching: Vec<DistributionTargetDto> = channels
                .iter()
                .filter(|channel| DistributionKindDto::of_format(channel.format) == Some(kind))
                .map(|channel| DistributionTargetDto {
                    kind,
                    channel: Some(channel.id.clone()),
                })
                .collect();
            if matching.is_empty() {
                vec![DistributionTargetDto {
                    kind,
                    channel: None,
                }]
            } else {
                matching
            }
        })
        .collect()
}

/// 笔记名里的一种叫法：某个渠道（`<项目>-<渠道名>`）或通用的一种（`<项目>-<种类>`）。
struct OutputLabel {
    kind: DistributionKindDto,
    channel: Option<Channel>,
    label: String,
}

/// 目录里每个能分发的渠道一种叫法，再加四种通用的（渠道在前：重名时认作渠道）。
fn output_labels(catalog: &[Channel], copy: BackendCopy<'_>) -> Vec<OutputLabel> {
    catalog
        .iter()
        .filter_map(|channel| {
            Some(OutputLabel {
                kind: DistributionKindDto::of_format(channel.format)?,
                channel: Some(channel.clone()),
                label: channel_label(channel, copy),
            })
        })
        .chain(
            DistributionKindDto::ALL
                .into_iter()
                .map(|kind| OutputLabel {
                    kind,
                    channel: None,
                    label: kind.label(copy),
                }),
        )
        .collect()
}

/// `<项目>-<叫法>` 或 `<项目>-<叫法>-<序号>`。
fn output_label<'a>(
    stem: &str,
    project: &str,
    labels: &'a [OutputLabel],
) -> Option<&'a OutputLabel> {
    let rest = stem.strip_prefix(project)?.strip_prefix('-')?;
    labels.iter().find(|entry| {
        rest.strip_prefix(entry.label.as_str()).is_some_and(|tail| {
            tail.is_empty()
                || tail
                    .strip_prefix('-')
                    .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        })
    })
}

/// 分发笔记的正文：去掉 frontmatter 与开头那行「由……改写」的引用。
fn draft_body(content: &str) -> &str {
    let mut body = luna_vault::frontmatter::body_of(content).trim_start();
    while body.starts_with('>') {
        body = body
            .split_once('\n')
            .map_or("", |(_, rest)| rest)
            .trim_start();
    }
    body.trim_end()
}

/// 推文串按编号（行首 `1/`、`2/`……）切成一条条；没有编号就整段是一条（领英、小红书图文）。
fn split_posts(body: &str) -> Vec<String> {
    let numbered = |line: &str| {
        let line = line.trim_start();
        let digits = line.bytes().take_while(u8::is_ascii_digit).count();
        digits > 0 && line[digits..].starts_with('/')
    };
    let mut posts: Vec<Vec<&str>> = Vec::new();
    for line in body.lines() {
        if numbered(line) || posts.is_empty() {
            posts.push(Vec::new());
        }
        if let Some(post) = posts.last_mut() {
            post.push(line);
        }
    }
    posts
        .into_iter()
        .map(|lines| lines.join("\n").trim().to_owned())
        .filter(|post| !post.is_empty())
        .collect()
}

fn over(length: usize, max: u32) -> Option<u32> {
    u32::try_from(length)
        .unwrap_or(u32::MAX)
        .checked_sub(max)
        .filter(|over| *over > 0)
}

/// 按渠道的上限重查一份（纯函数）：推文串查每一条；文章与简报查标题与全文；短视频的片段时长
/// 在生成时已经筛过。没有的上限不查。
fn check_output(
    kind: DistributionKindDto,
    channel: &Channel,
    content: &str,
) -> Vec<DistributionIssueDto> {
    let body = draft_body(content);
    let limits = channel.limits;
    let mut issues = Vec::new();
    match kind {
        DistributionKindDto::Thread => {
            if let Some(max) = limits.body_max {
                for (index, post) in split_posts(body).iter().enumerate() {
                    if let Some(over) = over(channel.text_length(post), max) {
                        issues.push(DistributionIssueDto::Post {
                            post: u32::try_from(index + 1).unwrap_or(u32::MAX),
                            over,
                            max,
                            weighted: channel.counts_weighted(),
                        });
                    }
                }
            }
        }
        DistributionKindDto::Blog | DistributionKindDto::Newsletter => {
            let title = body
                .lines()
                .find_map(|line| line.strip_prefix("# "))
                .map(str::trim);
            if let (Some(max), Some(title)) = (limits.title_max, title) {
                if let Some(over) = over(channel.text_length(title), max) {
                    issues.push(DistributionIssueDto::Title { over, max });
                }
            }
            if let Some(max) = limits.body_max {
                if let Some(over) = over(channel.text_length(body), max) {
                    issues.push(DistributionIssueDto::Body { over, max });
                }
            }
        }
        DistributionKindDto::Shorts => {}
    }
    issues
}

/// 项目文件夹里已有的分发笔记（按种类、再按名字排；写给某个渠道的那份顺带重查）。
fn outputs_of(vault: &Vault, home: &str, copy: BackendCopy<'_>) -> Vec<DistributionOutputDto> {
    let folder = project_folder(home);
    let project = note_stem_of(home);
    let labels = output_labels(&vault.channel_catalog(), copy);
    let mut outputs: Vec<DistributionOutputDto> = std::fs::read_dir(vault.root().join(folder))
        .map(|dir| {
            dir.filter_map(std::result::Result::ok)
                .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
                .filter_map(|name| {
                    let stem = name.strip_suffix(".md")?;
                    let entry = output_label(stem, &project, &labels)?;
                    let path = if folder.is_empty() {
                        name.clone()
                    } else {
                        format!("{folder}/{name}")
                    };
                    let issues = entry.channel.as_ref().map_or_else(Vec::new, |channel| {
                        vault
                            .read(&path)
                            .map(|doc| check_output(entry.kind, channel, &doc.content))
                            .unwrap_or_default()
                    });
                    Some(DistributionOutputDto {
                        kind: entry.kind,
                        channel: entry.channel.as_ref().map(|channel| channel.id.clone()),
                        path,
                        issues,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    outputs.sort_by(|a, b| {
        let rank =
            |kind: DistributionKindDto| DistributionKindDto::ALL.iter().position(|k| *k == kind);
        rank(a.kind)
            .cmp(&rank(b.kind))
            .then_with(|| a.path.cmp(&b.path))
    });
    outputs
}

/// 提示词变量：渠道名、上限句、风格句（通用目标：「任何平台」、推文串按 280 写）。
fn distribution_prompt_vars(
    kind: DistributionKindDto,
    channel: Option<&Channel>,
    copy: BackendCopy<'_>,
) -> (String, String, String) {
    let registry = luna_core::prompt::registry();
    let scope = luna_core::PromptScope::Summarize;
    let Some(channel) = channel else {
        let limits = if kind == DistributionKindDto::Thread {
            registry.render(
                scope,
                "creator.distribute.limit.post",
                &[("n", &GENERIC_POST_MAX.to_string())],
            )
        } else {
            registry.render(scope, "creator.distribute.limit.none", &[])
        };
        return (
            registry.render(scope, "creator.distribute.channel.none", &[]),
            limits,
            registry.render(scope, "creator.distribute.style.none", &[]),
        );
    };
    let limits = channel.limits;
    let wanted: &[(Option<u32>, &str)] = match kind {
        DistributionKindDto::Blog | DistributionKindDto::Newsletter => &[
            (limits.title_max, "creator.distribute.limit.title"),
            (limits.body_max, "creator.distribute.limit.body"),
        ],
        DistributionKindDto::Thread => &[(limits.body_max, "creator.distribute.limit.post")],
        DistributionKindDto::Shorts => &[(limits.clip_max_secs, "creator.distribute.limit.clip")],
    };
    let mut sentences: Vec<String> = wanted
        .iter()
        .filter_map(|(value, key)| {
            value.map(|n| registry.render(scope, key, &[("n", &n.to_string())]))
        })
        .collect();
    if sentences.is_empty() {
        sentences.push(registry.render(scope, "creator.distribute.limit.none", &[]));
    }
    (
        channel_label(channel, copy),
        sentences.join(" "),
        channel_style(channel, "creator.distribute.style"),
    )
}

/// 「分发」页签的数据（只读）。
///
/// # Errors
/// 库未打开时返回错误。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn creator_distribution(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    home: String,
) -> Result<DistributionDto, String> {
    let i18n = backend_copy_arc(&app);
    let copy = i18n.as_deref();
    let script_rel = super::creator::project_child_rel(&home, copy, "creator-note-script", "脚本");
    state.with_vault(window.label(), |vault| {
        let catalog = vault.channel_catalog();
        let channels: Vec<Channel> = vault
            .project_channels(&home)
            .map_err(vault_err)?
            .iter()
            .filter_map(|id| catalog.iter().find(|channel| &channel.id == id).cloned())
            .collect();
        Ok(DistributionDto {
            script: vault
                .project_script(&home, &script_rel)
                .map_err(vault_err)?,
            targets: distribution_targets(&channels),
            outputs: outputs_of(vault, &home, copy),
        })
    })
}

/// 模型给的一段短视频（ResultSchema）。
#[derive(Debug, serde::Deserialize)]
struct ModelClip {
    start: String,
    end: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    script: String,
}

#[derive(Debug, serde::Deserialize)]
struct ModelClips {
    #[serde(default)]
    clips: Vec<ModelClip>,
}

/// 对齐、校验过的一段。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Clip {
    start_ms: u64,
    end_ms: u64,
    title: String,
    script: String,
}

/// 把模型给的片段对齐到转写行首、按时长筛（纯函数）：入点出点都吸到最近的行首；出点不晚于
/// 片尾；时长不在 10 秒到 `max_ms`（目标渠道的上限，没有就 3 分钟）之间、标题或脚本为空、
/// 与前一段重叠的一律丢掉；最多五段，按入点排序。
fn align_clips(
    raw: Vec<ModelClip>,
    starts: &[u64],
    duration_ms: Option<u64>,
    max_ms: u64,
) -> Vec<Clip> {
    let mut clips: Vec<Clip> = raw
        .into_iter()
        .filter_map(|clip| {
            let start_ms = snap_to_nearest(starts, parse_timestamp(&clip.start)?);
            let mut end_ms = snap_to_nearest(starts, parse_timestamp(&clip.end)?);
            if let Some(duration) = duration_ms {
                end_ms = end_ms.min(duration);
            }
            let (title, script) = (clip.title.trim().to_owned(), clip.script.trim().to_owned());
            let length = end_ms.checked_sub(start_ms)?;
            ((CLIP_MIN_MS..=max_ms).contains(&length) && !title.is_empty() && !script.is_empty())
                .then_some(Clip {
                    start_ms,
                    end_ms,
                    title,
                    script,
                })
        })
        .collect();
    clips.sort_by_key(|clip| clip.start_ms);
    let mut kept: Vec<Clip> = Vec::new();
    for clip in clips {
        if kept.last().is_none_or(|last| clip.start_ms >= last.end_ms) {
            kept.push(clip);
        }
    }
    kept.truncate(CLIPS_MAX);
    kept
}

/// 短视频笔记的正文：一段一节，节下第一行写入点出点。
fn clips_markdown(clips: &[Clip], copy: BackendCopy<'_>) -> String {
    let with_hours = clips.iter().any(|clip| clip.end_ms >= 3_600_000);
    clips
        .iter()
        .enumerate()
        .map(|(index, clip)| {
            let range = copy
                .and_then(|i18n| {
                    i18n.translate_args(
                        "creator-distribute-clip-range",
                        &[
                            ("start", format_timestamp(clip.start_ms, with_hours)),
                            ("end", format_timestamp(clip.end_ms, with_hours)),
                        ],
                    )
                })
                .unwrap_or_else(|| {
                    format!(
                        "入点 {} · 出点 {}",
                        format_timestamp(clip.start_ms, with_hours),
                        format_timestamp(clip.end_ms, with_hours)
                    )
                });
            format!(
                "## {}. {}\n\n{range}\n\n{}",
                index + 1,
                clip.title,
                clip.script
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// 生成一份分发，写成项目文件夹里的一篇笔记，返回它的库相对路径（界面随即在项目里打开它）。
/// `channel` 是目标渠道（`None` = 通用的一份）：提示词按它的名字、上限与风格写，笔记名用它的
/// 名字，短视频片段按它的时长上限筛。`extra` 是「这次的额外要求」（与设置里的频道风格一起作为
/// 内容带给模型，ADR-166 ⑤）。
///
/// # Errors
/// 创作未启用、目标渠道不在目录里或形态不对、项目没有脚本 / 成片不在项目里、短视频没给成片
/// 转写、模型不可用或回得不像样、写盘失败时返回错误。
#[tauri::command]
#[specta::specta]
pub(crate) async fn creator_distribute(
    app: tauri::AppHandle,
    window: tauri::Window,
    home: String,
    kind: DistributionKindDto,
    channel: Option<String>,
    source: DistributionSourceDto,
    extra: Option<String>,
) -> Result<String, String> {
    {
        let state = app.state::<AppState>();
        super::require_app(&app, &state, luna_config::apps::CREATOR_APP.id)?;
    }
    let i18n = backend_copy_arc(&app);
    let copy = i18n.as_deref();
    let project = note_stem_of(&home).to_owned();
    let target = match &channel {
        Some(id) => {
            let state = app.state::<AppState>();
            let channel = state.with_vault(window.label(), |vault| channel_of(vault, id, &app))?;
            if DistributionKindDto::of_format(channel.format) != Some(kind) {
                return Err(coded_error(
                    &app,
                    "CREATOR_CHANNEL_UNKNOWN",
                    "creator-channel-unknown",
                ));
            }
            Some(channel)
        }
        None => None,
    };
    let (channel_name, limits, style) = distribution_prompt_vars(kind, target.as_ref(), copy);
    let vars = [
        ("channel", channel_name.as_str()),
        ("limits", limits.as_str()),
        ("style", style.as_str()),
    ];
    let notes = CreatorNotes::of(&app, extra.as_deref());
    let clip_max_ms = target
        .as_ref()
        .and_then(|channel| channel.limits.clip_max_secs)
        .map_or(CLIP_MAX_MS, |secs| u64::from(secs) * 1000)
        .max(CLIP_MIN_MS);

    let (body, source_label) = match (&source, kind) {
        (DistributionSourceDto::Script, DistributionKindDto::Shorts) => {
            return Err(coded_error(
                &app,
                "CREATOR_DISTRIBUTE_NEEDS_CUT",
                "creator-distribute-needs-cut",
            ));
        }
        (DistributionSourceDto::Script, _) => {
            let script_rel =
                super::creator::project_child_rel(&home, copy, "creator-note-script", "脚本");
            let state = app.state::<AppState>();
            let script = state.with_vault(window.label(), |vault| {
                let path = vault
                    .project_script(&home, &script_rel)
                    .map_err(vault_err)?
                    .ok_or_else(|| {
                        coded_error(
                            &app,
                            "CREATOR_DISTRIBUTE_NO_SCRIPT",
                            "creator-distribute-no-script",
                        )
                    })?;
                vault.read(&path).map(|doc| doc.content).map_err(vault_err)
            })?;
            let text = luna_vault::frontmatter::body_of(&script).trim().to_owned();
            if text.is_empty() {
                return Err(coded_error(
                    &app,
                    "CREATOR_DISTRIBUTE_NO_SCRIPT",
                    "creator-distribute-no-script",
                ));
            }
            super::chat::ensure_chat_context_window(&app).await;
            let limit =
                super::translate::floating_input_limit(app.state::<AppState>().chat_ctx_window());
            let text: String = text.chars().take(limit).collect();
            let output = run_summarize_job_with(
                &app,
                kind.template(),
                &vars,
                &notes,
                text,
                kind.output_tokens(),
            )
            .await?;
            (
                output,
                backend_text_with(copy, "creator-distribute-from-script", "脚本"),
            )
        }
        (DistributionSourceDto::Transcript { path }, _) => {
            let transcript = load_source_transcript(&app, window.label(), &home, path)?;
            let output = run_publish_job_with(
                &app,
                kind.template(),
                &vars,
                &notes,
                &transcript.lines,
                kind.output_tokens(),
            )
            .await?;
            let body = if kind == DistributionKindDto::Shorts {
                let parsed: ModelClips = serde_json::from_str(&output).map_err(|_| {
                    coded_error(&app, "CREATOR_PUBLISH_PARSE", "creator-publish-parse")
                })?;
                let starts: Vec<u64> = transcript.lines.iter().map(|(ms, _)| *ms).collect();
                let clips = align_clips(parsed.clips, &starts, transcript.duration_ms, clip_max_ms);
                if clips.is_empty() {
                    return Err(coded_error(
                        &app,
                        "CREATOR_PUBLISH_PARSE",
                        "creator-publish-parse",
                    ));
                }
                clips_markdown(&clips, copy)
            } else {
                output
            };
            let label = copy
                .and_then(|i18n| {
                    i18n.translate_args(
                        "creator-distribute-from-transcript",
                        &[("title", note_stem_of(path).to_owned())],
                    )
                })
                .unwrap_or_else(|| format!("成片「{}」的转写", note_stem_of(path)));
            (body, label)
        }
    };
    let body = body.trim();
    if body.is_empty() {
        return Err(coded_error(
            &app,
            "CREATOR_PUBLISH_PARSE",
            "creator-publish-parse",
        ));
    }
    let derived = copy
        .and_then(|i18n| {
            i18n.translate_args(
                "creator-distribute-derived",
                &[
                    ("project", format!("[[{project}]]")),
                    ("source", source_label.clone()),
                ],
            )
        })
        .unwrap_or_else(|| {
            format!("由「[[{project}]]」的{source_label}改写的 AI 草稿，发布前请自己过一遍。")
        });
    let content = format!("> {derived}\n\n{body}\n");

    let folder = project_folder(&home).to_owned();
    let label = target
        .as_ref()
        .map_or_else(|| kind.label(copy), |channel| channel_label(channel, copy));
    let base = format!("{project}-{label}");
    let state = app.state::<AppState>();
    let rel = state.with_vault(window.label(), |vault| {
        for index in 0..100 {
            let stem = if index == 0 {
                base.clone()
            } else {
                format!("{base}-{index}")
            };
            let rel = if folder.is_empty() {
                format!("{stem}.md")
            } else {
                format!("{folder}/{stem}.md")
            };
            if vault.note_exists(&rel) {
                continue;
            }
            vault.write(&rel, &content).map_err(vault_err)?;
            return Ok(rel);
        }
        Err(coded_error_with(
            copy,
            "VAULT_CREATE_EXHAUSTED",
            "vault-create-exhausted",
        ))
    })?;
    emit_vault_changed_in_scope(
        &app,
        &rel,
        false,
        VaultChangeScope::for_window(window.label()),
    );
    Ok(rel)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(start: &str, end: &str) -> ModelClip {
        ModelClip {
            start: start.to_owned(),
            end: end.to_owned(),
            title: "标题".to_owned(),
            script: "脚本".to_owned(),
        }
    }

    #[test]
    fn clips_snap_to_line_starts_and_bad_ones_are_dropped() {
        let starts = [0, 12_000, 30_000, 61_000, 95_000, 140_000];
        let clips = align_clips(
            vec![
                clip("1:01", "1:35"), // 对齐 61s–95s
                clip("0:11", "0:31"), // 吸到 12s–30s
                clip("0:00", "0:05"), // 太短
                clip("1:10", "1:50"), // 吸到 61s–95s，与第一段重叠
                clip("x", "0:30"),    // 认不出
                ModelClip {
                    title: String::new(),
                    ..clip("2:20", "2:40")
                }, // 没标题
            ],
            &starts,
            Some(150_000),
            CLIP_MAX_MS,
        );
        let ranges: Vec<(u64, u64)> = clips.iter().map(|c| (c.start_ms, c.end_ms)).collect();
        assert_eq!(ranges, [(12_000, 30_000), (61_000, 95_000)]);
    }

    fn channel(
        id: &str,
        format: ChannelFormat,
        limits: luna_vault::channel::ChannelLimits,
    ) -> Channel {
        Channel {
            id: id.to_owned(),
            name: None,
            format,
            limits,
            style: None,
            builtin: true,
            ai_notice: false,
        }
    }

    fn post_limit(n: u32) -> luna_vault::channel::ChannelLimits {
        luna_vault::channel::ChannelLimits {
            body_max: Some(n),
            ..Default::default()
        }
    }

    #[test]
    fn output_names_are_project_dash_channel_or_kind_with_optional_number() {
        let x = channel("x", ChannelFormat::Post, post_limit(280));
        let labels = vec![
            OutputLabel {
                kind: DistributionKindDto::Thread,
                channel: Some(x),
                label: "X".to_owned(),
            },
            OutputLabel {
                kind: DistributionKindDto::Blog,
                channel: None,
                label: "博客".to_owned(),
            },
            OutputLabel {
                kind: DistributionKindDto::Shorts,
                channel: None,
                label: "短视频".to_owned(),
            },
        ];
        let found = |stem: &str| {
            output_label(stem, "P", &labels)
                .map(|entry| (entry.kind, entry.channel.as_ref().map(|c| c.id.clone())))
        };
        assert_eq!(found("P-博客"), Some((DistributionKindDto::Blog, None)));
        assert_eq!(found("P-博客-2"), Some((DistributionKindDto::Blog, None)));
        assert_eq!(found("P-短视频"), Some((DistributionKindDto::Shorts, None)));
        assert_eq!(
            found("P-X-3"),
            Some((DistributionKindDto::Thread, Some("x".to_owned())))
        );
        assert_eq!(found("P-博客草稿"), None);
        assert_eq!(found("P-博客-x"), None);
        assert_eq!(found("Q-博客"), None);
    }

    #[test]
    fn targets_are_one_per_project_channel_of_each_kind_or_one_generic() {
        let channels = [
            channel("youtube", ChannelFormat::LongVideo, Default::default()),
            channel("x", ChannelFormat::Post, post_limit(280)),
            channel("douyin", ChannelFormat::ShortVideo, Default::default()),
            channel("weibo", ChannelFormat::Post, Default::default()),
        ];
        let targets: Vec<(DistributionKindDto, Option<&str>)> = distribution_targets(&channels)
            .iter()
            .map(|t| (t.kind, t.channel.as_deref()))
            .map(|(kind, channel)| {
                (
                    kind,
                    channel.map(|id| {
                        if id == "x" {
                            "x"
                        } else if id == "weibo" {
                            "weibo"
                        } else {
                            "douyin"
                        }
                    }),
                )
            })
            .collect();
        assert_eq!(
            targets,
            [
                (DistributionKindDto::Blog, None),
                (DistributionKindDto::Newsletter, None),
                (DistributionKindDto::Thread, Some("x")),
                (DistributionKindDto::Thread, Some("weibo")),
                (DistributionKindDto::Shorts, Some("douyin")),
            ]
        );
    }

    #[test]
    fn a_thread_is_checked_post_by_post_with_the_channels_own_count() {
        let x = channel("x", ChannelFormat::Post, post_limit(20));
        let note = "> 由「[[P]]」的脚本改写的 AI 草稿。\n\n1/ short\n\n2/ 这一条有十二个汉字那么长\n\n3/ ok";
        assert_eq!(
            check_output(DistributionKindDto::Thread, &x, note),
            [DistributionIssueDto::Post {
                post: 2,
                over: 7,
                max: 20,
                weighted: true
            }],
            "X 里汉字算两个：2/ 加空格 3 + 12 字 × 2 = 27"
        );
        let linkedin = channel("linkedin", ChannelFormat::Post, post_limit(10));
        assert_eq!(
            check_output(
                DistributionKindDto::Thread,
                &linkedin,
                "> 引\n\n一整条没有编号\n\n第二段"
            ),
            [DistributionIssueDto::Post {
                post: 1,
                over: 2,
                max: 10,
                weighted: false
            }],
            "没有编号就整段算一条"
        );
        let weibo = channel("weibo", ChannelFormat::Post, Default::default());
        assert!(
            check_output(DistributionKindDto::Thread, &weibo, note).is_empty(),
            "没有上限不查"
        );
    }

    #[test]
    fn an_article_checks_its_title_and_length() {
        let article = channel(
            "custom-1",
            ChannelFormat::Article,
            luna_vault::channel::ChannelLimits {
                title_max: Some(4),
                body_max: Some(12),
                ..Default::default()
            },
        );
        let issues = check_output(
            DistributionKindDto::Blog,
            &article,
            "> 引\n\n# 一个很长的标题\n\n正文",
        );
        assert_eq!(
            issues,
            [
                DistributionIssueDto::Title { over: 3, max: 4 },
                DistributionIssueDto::Body { over: 1, max: 12 },
            ]
        );
    }

    #[test]
    fn prompt_vars_follow_the_target_and_the_generic_thread_keeps_280() {
        let (name, limits, style) =
            distribution_prompt_vars(DistributionKindDto::Thread, None, None);
        assert!(!name.is_empty() && !style.is_empty());
        assert!(limits.contains("280"));
        let shorts = channel(
            "youtube_shorts",
            ChannelFormat::ShortVideo,
            luna_vault::channel::ChannelLimits {
                title_max: Some(100),
                clip_max_secs: Some(180),
                ..Default::default()
            },
        );
        let (name, limits, style) =
            distribution_prompt_vars(DistributionKindDto::Shorts, Some(&shorts), None);
        assert_eq!(name, "youtube_shorts", "没有界面语言时退回 id");
        assert!(
            limits.contains("180") && !limits.contains("100"),
            "短视频只带时长上限"
        );
        assert!(style.contains("Shorts"));
    }

    #[test]
    fn clips_markdown_numbers_sections_and_writes_the_range() {
        let text = clips_markdown(
            &[Clip {
                start_ms: 12_000,
                end_ms: 30_000,
                title: "开头".to_owned(),
                script: "第一句。".to_owned(),
            }],
            None,
        );
        assert_eq!(text, "## 1. 开头\n\n入点 0:12 · 出点 0:30\n\n第一句。");
    }
}
