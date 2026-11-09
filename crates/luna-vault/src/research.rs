//! 项目研究（ADR-167 ⑥，PRD-19-046）：来源指南与研究简报的纯函数，以及来源指南的缓存。
//!
//! - **来源指南**：一条来源的摘要与要点。模型回 JSON（`{"summary","points"}`），这里解析；结果按
//!   「路径 + 正文指纹」缓存在库里（`.luna/creator_source_guides.json`，随库同步），正文没变就不再
//!   调模型。
//! - **研究简报**：选中的几条来源编号喂给模型，模型回 JSON（`{"overview","findings","questions"}`，
//!   每条发现带来源编号），这里解析并渲染成一篇 Markdown——每条发现后面链回它出自的来源，链接由
//!   Rust 按真实路径生成，模型给不出、也编不出链接。
//! - **从简报出发**：读最新一份研究简报，提几个一句话选题（`{"topics"}`），或起草一版脚本（模型回
//!   Markdown，按 `##` 分段）；脚本只在项目脚本还是空骨架时写进去，否则另起一篇「脚本初稿」。
//!
//! 调模型、读写文件在调用方（`AIProvider + JobQueue + ResultSchema`，ADR-001）；这里没有 IO，缓存
//! 除外（读写库里的一份元数据）。

use serde::{Deserialize, Serialize};

use crate::pathutil::markdown_link;
use crate::vault::Vault;
use luna_sync_core::hashing::ContentHash;

/// 来源指南的缓存（库元数据名）。
pub(crate) const GUIDES_META: &str = "creator_source_guides";

/// 缓存最多记多少条（新的在前；超了丢最旧的）。
const GUIDES_MAX: usize = 300;

/// 一条来源的指南。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceGuide {
    pub summary: String,
    pub points: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CachedGuide {
    path: String,
    /// 生成时那份正文的指纹：正文变了，缓存就作废。
    hash: String,
    summary: String,
    points: Vec<String>,
}

/// 每条要点、摘要最多留多少字（模型偶尔写成一大段；超长的截断，不拒收整份）。
const GUIDE_TEXT_MAX_CHARS: usize = 600;

fn clean_line(text: &str) -> String {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    joined.chars().take(GUIDE_TEXT_MAX_CHARS).collect()
}

/// 截出模型回复里那一段 JSON 对象（前后可能有说明文字或代码围栏）。
fn json_object(raw: &str) -> Option<&str> {
    let begin = raw.find('{')?;
    let end = raw.rfind('}')?;
    (end > begin).then(|| &raw[begin..=end])
}

#[derive(Deserialize)]
struct ModelGuide {
    #[serde(default)]
    summary: String,
    #[serde(default)]
    points: Vec<String>,
}

/// 解析模型给的来源指南；摘要为空且一条要点都没有时为 `None`（等于没生成出来）。
#[must_use]
pub fn parse_source_guide(raw: &str) -> Option<SourceGuide> {
    let parsed: ModelGuide = serde_json::from_str(json_object(raw)?).ok()?;
    let summary = clean_line(&parsed.summary);
    let points: Vec<String> = parsed
        .points
        .iter()
        .map(|point| clean_line(point))
        .filter(|point| !point.is_empty())
        .take(8)
        .collect();
    (!summary.is_empty() || !points.is_empty()).then_some(SourceGuide { summary, points })
}

/// 研究简报里的一条发现：一句话与它出自的来源（0 起的下标）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BriefFinding {
    pub point: String,
    pub sources: Vec<usize>,
}

/// 模型写出的研究简报（已校验：来源编号都在范围内）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResearchBrief {
    pub overview: String,
    pub findings: Vec<BriefFinding>,
    pub questions: Vec<String>,
}

#[derive(Deserialize)]
struct ModelBrief {
    #[serde(default)]
    overview: String,
    #[serde(default)]
    findings: Vec<ModelFinding>,
    #[serde(default)]
    questions: Vec<String>,
}

#[derive(Deserialize)]
struct ModelFinding {
    #[serde(default)]
    point: String,
    #[serde(default)]
    sources: Vec<serde_json::Value>,
}

/// 解析模型给的研究简报。来源编号是 1 起的、可以是数字或数字字符串；越界、重复的丢掉；一条
/// 发现一个有效来源都没有就不收（没有出处的结论正是简报要避免的）。概述与发现都没有时为 `None`。
#[must_use]
pub fn parse_research_brief(raw: &str, source_count: usize) -> Option<ResearchBrief> {
    let parsed: ModelBrief = serde_json::from_str(json_object(raw)?).ok()?;
    let findings: Vec<BriefFinding> = parsed
        .findings
        .iter()
        .filter_map(|finding| {
            let point = clean_line(&finding.point);
            let mut sources: Vec<usize> = Vec::new();
            for value in &finding.sources {
                let number = value.as_u64().or_else(|| {
                    value
                        .as_str()
                        .and_then(|s| s.trim().trim_matches(['[', ']']).parse().ok())
                });
                let Some(index) = number
                    .and_then(|n| usize::try_from(n).ok())
                    .and_then(|n| n.checked_sub(1))
                else {
                    continue;
                };
                if index < source_count && !sources.contains(&index) {
                    sources.push(index);
                }
            }
            (!point.is_empty() && !sources.is_empty()).then_some(BriefFinding { point, sources })
        })
        .take(12)
        .collect();
    let overview = clean_line(&parsed.overview);
    let questions: Vec<String> = parsed
        .questions
        .iter()
        .map(|question| clean_line(question))
        .filter(|question| !question.is_empty())
        .take(6)
        .collect();
    (!overview.is_empty() || !findings.is_empty()).then_some(ResearchBrief {
        overview,
        findings,
        questions,
    })
}

/// 研究简报一篇笔记里的固定文字（随界面语言，由调用方给）。
#[derive(Clone, Copy, Debug)]
pub struct BriefCopy<'a> {
    /// 标题（`# …`），已带项目名。
    pub title: &'a str,
    /// 「依据：」那一行的引导词。
    pub based_on: &'a str,
    /// 列表分隔（`、` / `, `）。
    pub separator: &'a str,
    pub overview: &'a str,
    pub findings: &'a str,
    pub questions: &'a str,
}

/// 渲染研究简报：`note_rel` 是简报笔记自己的路径（链接按它算相对路径），`sources` 是喂给模型的
/// 那几条来源（顺序即编号）的 `(路径, 标题)`。
#[must_use]
pub fn render_research_brief(
    brief: &ResearchBrief,
    note_rel: &str,
    sources: &[(String, String)],
    copy: BriefCopy<'_>,
) -> String {
    let link = |index: usize| {
        let (path, title) = &sources[index];
        markdown_link(note_rel, path, title)
    };
    let mut out = format!("# {}\n\n", copy.title);
    let all: Vec<String> = (0..sources.len()).map(link).collect();
    out.push_str(&format!(
        "> {}{}\n\n",
        copy.based_on,
        all.join(copy.separator)
    ));
    if !brief.overview.is_empty() {
        out.push_str(&format!("## {}\n\n{}\n\n", copy.overview, brief.overview));
    }
    if !brief.findings.is_empty() {
        out.push_str(&format!("## {}\n\n", copy.findings));
        for finding in &brief.findings {
            let refs: Vec<String> = finding.sources.iter().map(|&index| link(index)).collect();
            out.push_str(&format!(
                "- {}（{}）\n",
                finding.point,
                refs.join(copy.separator)
            ));
        }
        out.push('\n');
    }
    if !brief.questions.is_empty() {
        out.push_str(&format!("## {}\n\n", copy.questions));
        for question in &brief.questions {
            out.push_str(&format!("- {question}\n"));
        }
        out.push('\n');
    }
    format!("{}\n", out.trim_end())
}

/// 模型提的一个选题：一句话（讲什么、讲给谁）与为什么这样切。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopicIdea {
    pub title: String,
    pub angle: String,
}

#[derive(Deserialize)]
struct ModelTopics {
    #[serde(default)]
    topics: Vec<ModelTopic>,
}

#[derive(Deserialize)]
struct ModelTopic {
    #[serde(default)]
    title: String,
    #[serde(default)]
    angle: String,
}

/// 解析模型提的选题（至多五个，标题为空的丢掉）；一个都没有为 `None`。
#[must_use]
pub fn parse_topics(raw: &str) -> Option<Vec<TopicIdea>> {
    let parsed: ModelTopics = serde_json::from_str(json_object(raw)?).ok()?;
    let topics: Vec<TopicIdea> = parsed
        .topics
        .iter()
        .map(|topic| TopicIdea {
            title: clean_line(&topic.title),
            angle: clean_line(&topic.angle),
        })
        .filter(|topic| !topic.title.is_empty())
        .take(5)
        .collect();
    (!topics.is_empty()).then_some(topics)
}

/// 模型起草的脚本：去掉代码围栏与围栏外的说明，至少要有一个 `## ` 段落标题才算（脚本按二级标题
/// 分段，ADR-162 ③）；否则为 `None`。
#[must_use]
pub fn clean_script_draft(raw: &str) -> Option<String> {
    let start = raw.find("## ")?;
    let rest = &raw[start..];
    // 围栏外的结束说明：截到第一个收尾的 ``` 为止。
    let body = rest.find("```").map_or(rest, |end| &rest[..end]).trim();
    (!body.is_empty()).then(|| format!("{body}\n"))
}

/// 脚本是不是还只是空骨架：正文除了标题行全是空白（新建项目时的「开头 / 节拍 1 / 结尾」）。
#[must_use]
pub fn script_is_blank(content: &str) -> bool {
    crate::frontmatter::body_of(content)
        .lines()
        .all(|line| line.trim().is_empty() || line.trim_start().starts_with('#'))
}

/// 喂给模型的来源文字：去掉 frontmatter，截到 `limit_chars` 字。
#[must_use]
pub fn source_text(content: &str, limit_chars: usize) -> String {
    crate::frontmatter::body_of(content)
        .trim()
        .chars()
        .take(limit_chars)
        .collect()
}

impl Vault {
    /// 项目文件夹里最新的一份研究简报（`<项目>-<研究简报>` 及其带序号的那些，按修改时间）。
    ///
    /// # Errors
    /// 索引查询失败时返回错误。
    pub fn latest_research_brief(
        &self,
        folder: &str,
        stem: &str,
    ) -> crate::error::Result<Option<String>> {
        let prefix = if folder.is_empty() {
            stem.to_owned()
        } else {
            format!("{folder}/{stem}")
        };
        let mut briefs: Vec<(i64, String)> = self
            .index()
            .notes_by_path()?
            .into_iter()
            .filter(|entry| {
                entry.path.strip_prefix(&prefix).is_some_and(|rest| {
                    rest == ".md"
                        || (rest.starts_with('-')
                            && rest.ends_with(".md")
                            && !rest[1..].contains('/'))
                })
            })
            .map(|entry| (entry.modified_ms, entry.path))
            .collect();
        briefs.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
        Ok(briefs.into_iter().next().map(|(_, path)| path))
    }

    /// 这条来源有没有现成的指南（正文没变才算）。
    #[must_use]
    pub fn cached_source_guide(&self, rel: &str, content: &str) -> Option<SourceGuide> {
        let hash = ContentHash::of(content.as_bytes()).to_string();
        crate::meta::read_meta::<CachedGuide>(self.root(), GUIDES_META)
            .unwrap_or_default()
            .into_iter()
            .find(|entry| entry.path == rel && entry.hash == hash)
            .map(|entry| SourceGuide {
                summary: entry.summary,
                points: entry.points,
            })
    }

    /// 记下这条来源的指南（同一路径只留最新的一份；总数超了丢最旧的）。
    ///
    /// # Errors
    /// 写库元数据失败时返回错误。
    pub fn remember_source_guide(
        &self,
        rel: &str,
        content: &str,
        guide: &SourceGuide,
    ) -> crate::error::Result<()> {
        let mut entries: Vec<CachedGuide> =
            crate::meta::read_meta(self.root(), GUIDES_META).unwrap_or_default();
        entries.retain(|entry| entry.path != rel);
        entries.insert(
            0,
            CachedGuide {
                path: rel.to_owned(),
                hash: ContentHash::of(content.as_bytes()).to_string(),
                summary: guide.summary.clone(),
                points: guide.points.clone(),
            },
        );
        entries.truncate(GUIDES_MAX);
        crate::meta::write_meta(self.root(), GUIDES_META, &entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_guide_survives_chatter_around_the_json_and_drops_empty_points() {
        let raw = "好的：\n```json\n{\"summary\": \" 一篇访谈。 \", \"points\": [\"要点一\", \"\", \"要点  二\"]}\n```";
        let guide = parse_source_guide(raw).unwrap();
        assert_eq!(guide.summary, "一篇访谈。");
        assert_eq!(guide.points, ["要点一", "要点 二"]);
        assert_eq!(
            parse_source_guide("{\"summary\": \"\", \"points\": []}"),
            None
        );
        assert_eq!(parse_source_guide("不是 JSON"), None);
    }

    #[test]
    fn brief_findings_keep_only_real_sources_and_drop_unsourced_claims() {
        let raw = r#"{"overview": "三条来源都说……", "findings": [
            {"point": "结论甲", "sources": [1, "2", 2, 9]},
            {"point": "没有出处的结论", "sources": []},
            {"point": "结论乙", "sources": ["[3]"]}
        ], "questions": ["还要查什么？", ""]}"#;
        let brief = parse_research_brief(raw, 3).unwrap();
        assert_eq!(
            brief.findings,
            [
                BriefFinding {
                    point: "结论甲".into(),
                    sources: vec![0, 1]
                },
                BriefFinding {
                    point: "结论乙".into(),
                    sources: vec![2]
                },
            ]
        );
        assert_eq!(brief.questions, ["还要查什么？"]);
    }

    #[test]
    fn the_rendered_brief_links_every_finding_back_to_its_sources() {
        let brief = ResearchBrief {
            overview: "概述。".into(),
            findings: vec![BriefFinding {
                point: "结论甲".into(),
                sources: vec![1],
            }],
            questions: vec!["待查？".into()],
        };
        let sources = vec![
            ("Creator/P/素材/访谈.md".to_owned(), "访谈".to_owned()),
            ("Notes/外部资料.md".to_owned(), "外部资料".to_owned()),
        ];
        let copy = BriefCopy {
            title: "P 研究简报",
            based_on: "依据：",
            separator: "、",
            overview: "概述",
            findings: "要点",
            questions: "待查的问题",
        };
        let note = render_research_brief(&brief, "Creator/P/P-研究简报.md", &sources, copy);
        assert!(
            note.starts_with("# P 研究简报\n\n> 依据：[访谈](素材/访谈.md)、[外部资料]"),
            "{note}"
        );
        assert!(note.contains("- 结论甲（[外部资料](../../Notes/"), "{note}");
        assert!(note.contains("## 待查的问题\n\n- 待查？\n"), "{note}");
    }

    #[test]
    fn a_cached_guide_is_only_reused_while_the_source_text_is_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        let guide = SourceGuide {
            summary: "摘要".into(),
            points: vec!["点".into()],
        };
        vault
            .remember_source_guide("Notes/a.md", "正文一", &guide)
            .unwrap();
        assert_eq!(
            vault.cached_source_guide("Notes/a.md", "正文一"),
            Some(guide.clone())
        );
        assert_eq!(
            vault.cached_source_guide("Notes/a.md", "正文二"),
            None,
            "正文变了就作废"
        );
        assert_eq!(vault.cached_source_guide("Notes/b.md", "正文一"), None);
    }

    #[test]
    fn topics_and_script_drafts_are_parsed_defensively() {
        let topics = parse_topics(r#"{"topics": [{"title": "一周发一次版", "angle": "从访谈切入"}, {"title": "", "angle": "x"}]}"#).unwrap();
        assert_eq!(
            topics,
            [TopicIdea {
                title: "一周发一次版".into(),
                angle: "从访谈切入".into()
            }]
        );
        assert_eq!(parse_topics(r#"{"topics": []}"#), None);
        let draft = clean_script_draft(
            "好的，这是初稿：\n```markdown\n## 开头\n大家好。\n\n## 结尾\n再见。\n```",
        )
        .unwrap();
        assert_eq!(draft, "## 开头\n大家好。\n\n## 结尾\n再见。\n");
        assert_eq!(clean_script_draft("没有分段的一大段话"), None);
    }

    #[test]
    fn only_a_skeleton_script_counts_as_blank() {
        assert!(script_is_blank(
            "---\nluna:\n  type: script\n---\n## 开头\n\n## 节拍 1\n\n## 结尾\n"
        ));
        assert!(!script_is_blank("## 开头\n我写了一句。\n"));
    }

    #[test]
    fn the_latest_brief_is_picked_by_modification_time_and_name_shape() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        vault.write("Creator/P/P-研究简报.md", "# 旧\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        vault.write("Creator/P/P-研究简报-1.md", "# 新\n").unwrap();
        vault
            .write("Creator/P/P-研究简报草稿/x.md", "# 子文件夹里的不算\n")
            .unwrap();
        vault.rebuild_index().unwrap();
        assert_eq!(
            vault
                .latest_research_brief("Creator/P", "P-研究简报")
                .unwrap()
                .as_deref(),
            Some("Creator/P/P-研究简报-1.md")
        );
        assert_eq!(
            vault
                .latest_research_brief("Creator/Q", "Q-研究简报")
                .unwrap(),
            None
        );
    }

    #[test]
    fn source_text_drops_frontmatter_and_respects_the_budget() {
        let content = "---\nluna:\n  type: transcript\n---\n[0:01] 你好世界\n";
        assert_eq!(source_text(content, 100), "[0:01] 你好世界");
        assert_eq!(source_text(content, 4), "[0:0");
    }
}
