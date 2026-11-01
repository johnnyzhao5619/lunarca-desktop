//! 知识地图的确定性 Markdown 生成（PRD-16-023）。
//!
//! 本模块不调用模型、不读设置：相同主题页集合必得相同字节。地图只做路由，摘要取主题页
//! 首个有意义段落；问答检索不得把地图本身作为事实来源。

use crate::{body_of, markdown_link, title_of};

pub const SUMMARY_CHARS: usize = 120;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KnowledgeMapTopic<'a> {
    pub path: &'a str,
    pub content: &'a str,
    /// 系统最近一次写入这一页的时刻（系统底版的时间，Unix 毫秒）；没有底版为 `None`。
    /// 只用于地图与目录的「最近更新」——按先后取前几页，不按「距今几天」开窗，地图才不会随
    /// 时间流逝自己变。
    pub updated_ms: Option<u64>,
}

/// 地图「最近更新」列几页。
const RECENT_TOPICS: usize = 5;
/// 每页列几条相关主题。
const RELATED_TOPICS: usize = 3;

/// 地图的固定文案（调用方按界面语言给）。
#[derive(Clone, Copy, Debug)]
pub struct MapLabels<'a> {
    pub heading: &'a str,
    pub empty: &'a str,
    /// 没有已确认标签的主题页所在的分组标题。
    pub other: &'a str,
    /// 「最近更新」小节标题。
    pub recent: &'a str,
    /// 相关主题那一行的前缀（如「相关：」）。
    pub related: &'a str,
}

/// 一组主题：`name` 为 `None` 即「其他主题」（没有已确认标签的页）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopicGroup<'a> {
    pub name: Option<String>,
    pub topics: Vec<KnowledgeMapTopic<'a>>,
}

/// 按已确认的标签层级把主题页分组（PRD-16-050）——知识地图与主题目录**共用这一个函数**，
/// 分组与顺序只有一个来源：组按名称排序（小写后逐码位）、「其他主题」最后；组内按标题。
/// 一页可属多组（每组各列一次），计数由调用方按页去重。
#[must_use]
pub fn group_topics<'a>(topics: &[KnowledgeMapTopic<'a>]) -> Vec<TopicGroup<'a>> {
    let mut groups: std::collections::BTreeMap<
        (bool, String),
        (Option<String>, Vec<KnowledgeMapTopic<'a>>),
    > = std::collections::BTreeMap::new();
    for topic in topics {
        let names = topic_groups(topic.content);
        if names.is_empty() {
            groups
                .entry((true, String::new()))
                .or_insert_with(|| (None, Vec::new()))
                .1
                .push(*topic);
        }
        for name in names {
            groups
                .entry((false, name.to_lowercase()))
                .or_insert_with(|| (Some(name.clone()), Vec::new()))
                .1
                .push(*topic);
        }
    }
    groups
        .into_values()
        .map(|(name, mut members)| {
            members.sort_by(|a, b| {
                let (ta, tb) = (title_of(a.path), title_of(b.path));
                ta.to_lowercase()
                    .cmp(&tb.to_lowercase())
                    .then_with(|| a.path.to_lowercase().cmp(&b.path.to_lowercase()))
            });
            TopicGroup {
                name,
                topics: members,
            }
        })
        .collect()
}

/// 按已确认的标签层级分组的知识地图（PRD-16-050）：每组一个二级标题；分组只取已有标签的
/// 顶层段，不让模型编排学科树；同一输入必得同一字节。
#[must_use]
pub fn render_knowledge_map(
    map_rel: &str,
    topics: &[KnowledgeMapTopic<'_>],
    labels: MapLabels<'_>,
) -> String {
    let mut out = format!("# {}\n\n", labels.heading.trim());
    if topics.is_empty() {
        out.push_str(labels.empty.trim());
        out.push('\n');
        return out;
    }
    let recent = recent_topics(topics);
    let mut first = true;
    if !recent.is_empty() {
        first = false;
        out.push_str(&format!("## {}\n\n", labels.recent.trim()));
        for path in recent {
            out.push_str(&format!(
                "- {}\n",
                markdown_link(map_rel, path, &title_of(path))
            ));
        }
    }
    let related = related_topics(topics);
    for group in group_topics(topics) {
        if !first {
            out.push('\n');
        }
        first = false;
        let name = group.name.as_deref().unwrap_or(labels.other).trim();
        out.push_str(&format!("## {name}\n\n"));
        for topic in group.topics {
            let title = title_of(topic.path);
            let summary = topic_summary(topic.content);
            let link = markdown_link(map_rel, topic.path, &title);
            if summary.is_empty() {
                out.push_str(&format!("- {link}\n"));
            } else {
                out.push_str(&format!("- {link} — {summary}\n"));
            }
            if let Some(others) = related.get(topic.path).filter(|others| !others.is_empty()) {
                let links: Vec<String> = others
                    .iter()
                    .map(|other| markdown_link(map_rel, other, &title_of(other)))
                    .collect();
                out.push_str(&format!("  - {}{}\n", labels.related, links.join("、")));
            }
        }
    }
    out
}

/// 「最近更新」的那几页（PRD-16-050）：按系统最近写入时刻降序、同刻按路径，至多
/// [`RECENT_TOPICS`] 页；没有底版的页不列。知识地图与主题目录（PRD-16-061）共用这一个函数。
#[must_use]
pub fn recent_topics<'a>(topics: &[KnowledgeMapTopic<'a>]) -> Vec<&'a str> {
    let mut recent: Vec<(u64, &'a str)> = topics
        .iter()
        .filter_map(|topic| topic.updated_ms.map(|at| (at, topic.path)))
        .collect();
    recent.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    recent
        .into_iter()
        .take(RECENT_TOPICS)
        .map(|(_, path)| path)
        .collect()
}

/// 两页引用了同一篇来源即相关（PRD-16-050「相关主题链接」）：按 [`topic_source_links`] 的关联强度
/// 降序、同强度按路径，每页至多 [`RELATED_TOPICS`] 条。知识地图与主题目录共用。
#[must_use]
pub fn related_topics<'a>(
    topics: &[KnowledgeMapTopic<'a>],
) -> std::collections::HashMap<&'a str, Vec<&'a str>> {
    let links = topic_source_links(topics);
    let mut out = std::collections::HashMap::new();
    for topic in topics {
        // `links` 已按强度降序、两端路径排好，按序取即是本页的相关顺序。
        let mut others: Vec<(&TopicLink<'a>, &'a str)> = links
            .iter()
            .filter_map(|link| {
                if link.a == topic.path {
                    Some((link, link.b))
                } else if link.b == topic.path {
                    Some((link, link.a))
                } else {
                    None
                }
            })
            .collect();
        others.sort_by(|x, y| y.0.weight.total_cmp(&x.0.weight).then_with(|| x.1.cmp(y.1)));
        out.insert(
            topic.path,
            others
                .into_iter()
                .take(RELATED_TOPICS)
                .map(|(_, other)| other)
                .collect(),
        );
    }
    out
}

/// 每页引用的来源（库内路径）；指向这批主题页自己的链接不算来源。
fn cited_sources<'a>(
    topics: &[KnowledgeMapTopic<'a>],
) -> Vec<(&'a str, std::collections::BTreeSet<String>)> {
    let pages: std::collections::HashSet<&str> = topics.iter().map(|topic| topic.path).collect();
    topics
        .iter()
        .map(|topic| {
            let cited = crate::parse::extract_links(body_of(topic.content))
                .iter()
                .map(|link| crate::pathutil::canonical_link_target(topic.path, link))
                .filter(|target| !pages.contains(target.as_str()))
                .collect();
            (topic.path, cited)
        })
        .collect()
}

/// 两页主题之间经共同来源的关联（[`topic_source_links`]）。
#[derive(Clone, Debug, PartialEq)]
pub struct TopicLink<'a> {
    pub a: &'a str,
    pub b: &'a str,
    /// 共同来源的篇数。
    pub shared: usize,
    /// 关联强度：每篇共同来源按「它被几页主题引用」折算——只被这两页引用的算 1，被 k 页
    /// 引用的算 1/(k−1)。几乎所有主题都引用的枢纽笔记（如一篇总周会）因此只给每对一点点，
    /// 网络与「相关主题」才分得出远近（真机 2026-10-01：7 页全引用同一篇周会，图成了全连通）。
    pub weight: f64,
}

/// 主题级知识网络的边（PRD-16-062）：两页引用了同一篇来源即连一条——与 [`related_topics`]
/// 同一判据，只是不截断。每对只出现一次（按输入顺序前者在前），按强度降序、同强度按两端路径。
/// 确定性：只看这批主题页的正文。
#[must_use]
pub fn topic_source_links<'a>(topics: &[KnowledgeMapTopic<'a>]) -> Vec<TopicLink<'a>> {
    let sources = cited_sources(topics);
    let mut cited_by: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for (_, cited) in &sources {
        for source in cited {
            *cited_by.entry(source.as_str()).or_default() += 1;
        }
    }
    let mut out = Vec::new();
    for (index, (path, cited)) in sources.iter().enumerate() {
        for (other, theirs) in &sources[index + 1..] {
            let shared: Vec<&String> = cited.intersection(theirs).collect();
            if shared.is_empty() {
                continue;
            }
            // 共同来源至少被这两页引用，`k − 1 ≥ 1`。
            #[allow(clippy::cast_precision_loss)]
            let weight = shared
                .iter()
                .map(|source| 1.0 / (cited_by[source.as_str()] - 1) as f64)
                .sum();
            out.push(TopicLink {
                a: path,
                b: other,
                shared: shared.len(),
                weight,
            });
        }
    }
    out.sort_by(|x, y| {
        y.weight
            .total_cmp(&x.weight)
            .then_with(|| x.a.cmp(y.a))
            .then_with(|| x.b.cmp(y.b))
    });
    out
}

/// 一页主题页所属的分组（PRD-16-050）——知识地图与主题目录共用这一条规则。
///
/// 只看**已确认**的标签：证据格式页取声明范围的包含标签，旧格式页取出身标签；系统自动打的
/// AI 标签不算。分组名是标签的顶层段（`项目/C` → `项目`），去重后按名称排序；返回空即归
/// 「其他主题」。
#[must_use]
pub fn topic_groups(content: &str) -> Vec<String> {
    let tags: Vec<String> = match crate::validate_topic_document(content) {
        Ok(knowledge) => knowledge.scope.includes,
        Err(_) => crate::read_luna(content).topic_src.into_iter().collect(),
    };
    let mut groups: Vec<String> = tags
        .iter()
        .filter_map(|tag| tag.split('/').next())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect();
    groups.sort_by(|a, b| {
        a.to_lowercase()
            .cmp(&b.to_lowercase())
            .then_with(|| a.cmp(b))
    });
    groups.dedup_by(|a, b| a.to_lowercase() == b.to_lowercase());
    groups
}

/// 一页主题页的**确定性一句摘要**——知识地图与「知识库 → 主题页」目录共用这一条规则
/// （PRD-16-023 / ADR-088 ②）。
///
/// **同一输入必得同一字节**：不调模型、不读设置、不看时间。两处各写一份实现的表现是
/// 同一页在地图里一句话、在目录里另一句话，而两处都自称是这一页的摘要——故目录**不**
/// 另写摘要、也**不**落第二份缓存，它就调这个函数（TDD-07 §4.1d）。
#[must_use]
pub fn topic_summary(content: &str) -> String {
    first_meaningful_paragraph(body_of(content))
}

#[must_use]
pub fn first_meaningful_paragraph(body: &str) -> String {
    for paragraph in body.split("\n\n") {
        let lines = paragraph
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            // 机器边界注释（证据主题的 `<!-- luna:claim:… -->` 等）不是给人读的内容：
            // 证据格式页的正文一开头就是声明块，不跳过它，摘要就成了一串注释（真机 2026-09-25）。
            .filter(|line| !line.starts_with("<!--"))
            .filter(|line| !is_link_only(line))
            // 摘要是给人读的：把 `- ` / `* ` / `> ` / `1. ` 这些**块级**标记也去掉。
            // 只压平 inline 语法而把列表符号原样摆出来，是第六轮已经修过的同一类问题
            //（"给解析器看的东西给人看"）。
            .map(strip_block_marker)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>();
        if lines.is_empty() {
            continue;
        }
        let flattened = inline_markdown_to_text(&lines.join(" "));
        if !flattened.is_empty() {
            let mut summary = flattened.chars().take(SUMMARY_CHARS).collect::<String>();
            if flattened.chars().count() > SUMMARY_CHARS {
                summary.push('…');
            }
            return summary;
        }
    }
    String::new()
}

/// 去掉一行开头的块级 Markdown 标记（无序列表 / 引用 / 有序列表）。
fn strip_block_marker(line: &str) -> &str {
    // 引用套列表（`> 1. …`）是常见形态，故逐层剥到不再变化为止。
    let mut cur = line.trim_start();
    loop {
        let stripped = cur.trim_start_matches(['-', '*', '+', '>']).trim_start();
        if stripped.len() < cur.len() {
            cur = stripped;
            continue;
        }
        // `1. ` / `12) ` 这类有序列表：只在真的是「数字 + 分隔符 + 空格」时才剥。
        let digits = cur.len() - cur.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        if digits > 0 {
            let after = &cur[digits..];
            if let Some(tail) = after
                .strip_prefix(". ")
                .or_else(|| after.strip_prefix(") "))
            {
                cur = tail.trim_start();
                continue;
            }
        }
        return cur;
    }
}

fn is_link_only(line: &str) -> bool {
    let trimmed = line
        .trim_start_matches(|c: char| c == '-' || c.is_whitespace())
        .trim();
    trimmed.starts_with('[') && trimmed.ends_with(')') && trimmed.contains("](")
}

fn inline_markdown_to_text(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '`' | '*' | '_' => {}
            '[' => {
                for next in chars.by_ref() {
                    if next == ']' {
                        break;
                    }
                    out.push(next);
                }
                if chars.peek() == Some(&'(') {
                    chars.next();
                    for next in chars.by_ref() {
                        if next == ')' {
                            break;
                        }
                    }
                }
            }
            _ => out.push(ch),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const LABELS_EN: MapLabels<'static> = MapLabels {
        heading: "Knowledge Map",
        empty: "Empty",
        other: "Other topics",
        recent: "Recently updated",
        related: "Related: ",
    };
    const LABELS_ZH: MapLabels<'static> = MapLabels {
        heading: "知识地图",
        empty: "空",
        other: "其他主题",
        recent: "最近更新",
        related: "相关：",
    };

    fn legacy(tag: &str, body: &str) -> String {
        format!("---\nluna:\n  topic: true\n  topic_src: {tag}\n---\n{body}")
    }

    /// 分组只取已确认标签的顶层段（PRD-16-050）：同顶层的主题同组、组按名排序、组内按标题；
    /// 没有已确认标签的在「其他主题」最后；同一输入同一字节。
    #[test]
    fn the_map_groups_topics_by_the_top_of_their_confirmed_tags() {
        let a = legacy("项目/A", "# A\n\n项目 A 的进展。");
        let c = legacy("项目/C", "# C\n\n项目 C 的进展。");
        let reading = legacy("阅读", "# 阅读\n\n读书笔记汇总。");
        let loose = "# 散页\n\n没有出身标签。".to_owned();
        let topics = [
            KnowledgeMapTopic {
                path: "Knowledge/C.md",
                content: &c,
                updated_ms: None,
            },
            KnowledgeMapTopic {
                path: "Knowledge/散页.md",
                content: &loose,
                updated_ms: None,
            },
            KnowledgeMapTopic {
                path: "Knowledge/阅读.md",
                content: &reading,
                updated_ms: None,
            },
            KnowledgeMapTopic {
                path: "Knowledge/A.md",
                content: &a,
                updated_ms: None,
            },
        ];
        let map = render_knowledge_map("Knowledge/知识地图.md", &topics, LABELS_ZH);
        assert_eq!(
            map,
            render_knowledge_map("Knowledge/知识地图.md", &topics, LABELS_ZH)
        );
        let at = |needle: &str| {
            map.find(needle)
                .unwrap_or_else(|| panic!("缺 {needle}:\n{map}"))
        };
        assert!(at("## 项目") < at("[A]") && at("[A]") < at("[C]"), "{map}");
        // 组按名称（小写后逐码位）排序：「阅」U+9605 在「项」U+9879 之前。
        assert!(at("## 阅读") < at("## 项目"), "组按名称排序：{map}");
        assert!(
            at("## 项目") < at("## 其他主题") && at("## 其他主题") < at("[散页]"),
            "{map}"
        );
        assert_eq!(topic_groups(&a), vec!["项目".to_owned()]);
        assert!(topic_groups(&loose).is_empty());
    }

    /// 「最近更新」按系统写入先后取前几页；相关主题按共同来源数，指向主题页自己的链接不算来源。
    #[test]
    fn the_map_lists_recent_updates_and_related_topics_deterministically() {
        let a = legacy(
            "项目/A",
            "# A\n\n进展见[会议](../Notes/会议.md)与[周报](../Notes/周报.md)。",
        );
        let b = legacy(
            "项目/B",
            "# B\n\n也见[会议](../Notes/会议.md)，另见[C](C.md)。",
        );
        let c = legacy("阅读", "# C\n\n读书笔记，见[周报](../Notes/周报.md)。");
        let topics = [
            KnowledgeMapTopic {
                path: "Knowledge/A.md",
                content: &a,
                updated_ms: Some(10),
            },
            KnowledgeMapTopic {
                path: "Knowledge/B.md",
                content: &b,
                updated_ms: Some(30),
            },
            KnowledgeMapTopic {
                path: "Knowledge/C.md",
                content: &c,
                updated_ms: None,
            },
        ];
        let map = render_knowledge_map("Knowledge/知识地图.md", &topics, LABELS_ZH);
        assert_eq!(
            map,
            render_knowledge_map("Knowledge/知识地图.md", &topics, LABELS_ZH)
        );
        let recent = &map[map.find("## 最近更新").unwrap()..map.find("## 阅读").unwrap()];
        assert!(
            recent.find("[B]").unwrap() < recent.find("[A]").unwrap(),
            "{map}"
        );
        assert!(!recent.contains("[C]"), "没有底版的页不进最近更新");
        // 分组里的那一行（最近更新里也有一行同样的链接）。
        let a_line = map
            .lines()
            .collect::<Vec<_>>()
            .iter()
            .rposition(|l| l.starts_with("- [A](A.md)"))
            .unwrap();
        let related = map.lines().nth(a_line + 1).unwrap();
        assert_eq!(related, "  - 相关：[B](B.md)、[C](C.md)", "{map}");
        // 分组里的那一行（最近更新里也有一行同样的链接）。
        let b_line = map
            .lines()
            .collect::<Vec<_>>()
            .iter()
            .rposition(|l| l.starts_with("- [B](B.md)"))
            .unwrap();
        assert_eq!(
            map.lines().nth(b_line + 1).unwrap(),
            "  - 相关：[A](A.md)",
            "B 链向 C 的主题页链接不算共同来源"
        );
    }

    /// 主题级网络的边与「相关主题」同一判据，只是不截断：每对一次，按共同来源数降序。
    #[test]
    fn topic_source_links_pair_topics_that_cite_the_same_source() {
        let a = legacy(
            "项目/A",
            "# A\n\n见[会议](../Notes/会议.md)与[周报](../Notes/周报.md)。",
        );
        let b = legacy(
            "项目/B",
            "# B\n\n见[会议](../Notes/会议.md)、[周报](../Notes/周报.md)，另见[C](C.md)。",
        );
        let c = legacy("阅读", "# C\n\n见[周报](../Notes/周报.md)。");
        let d = legacy("杂项", "# D\n\n与别处都不相干。");
        let topic = |path, content| KnowledgeMapTopic {
            path,
            content,
            updated_ms: None,
        };
        let topics = [
            topic("Knowledge/A.md", &a),
            topic("Knowledge/B.md", &b),
            topic("Knowledge/C.md", &c),
            topic("Knowledge/D.md", &d),
        ];
        let links: Vec<(&str, &str, usize, f64)> = topic_source_links(&topics)
            .into_iter()
            .map(|link| (link.a, link.b, link.shared, link.weight))
            .collect();
        // 会议只被 A、B 引用（各算 1）；周报被三页引用（每对算 1/2）。
        assert_eq!(
            links,
            vec![
                ("Knowledge/A.md", "Knowledge/B.md", 2, 1.5),
                ("Knowledge/A.md", "Knowledge/C.md", 1, 0.5),
                ("Knowledge/B.md", "Knowledge/C.md", 1, 0.5),
            ],
            "B 链向 C 的主题页链接不算共同来源；D 不连任何页"
        );
    }

    /// 枢纽来源（几乎每页都引用）只给每对一点点：两页独有的共同来源决定谁和谁最近。
    #[test]
    fn a_hub_source_cited_by_every_topic_does_not_decide_who_is_related() {
        let hub = "[周会](../Notes/周会.md)";
        let pages: Vec<(String, String)> = [
            (
                "Knowledge/甲.md",
                "甲",
                format!("# 甲\n\n见{hub}与[评审](../Notes/评审.md)。"),
            ),
            (
                "Knowledge/乙.md",
                "乙",
                format!("# 乙\n\n见{hub}与[评审](../Notes/评审.md)。"),
            ),
            ("Knowledge/丙.md", "丙", format!("# 丙\n\n见{hub}。")),
            ("Knowledge/丁.md", "丁", format!("# 丁\n\n见{hub}。")),
        ]
        .into_iter()
        .map(|(path, tag, body)| (path.to_owned(), legacy(tag, &body)))
        .collect();
        let topics: Vec<KnowledgeMapTopic<'_>> = pages
            .iter()
            .map(|(path, content)| KnowledgeMapTopic {
                path,
                content,
                updated_ms: None,
            })
            .collect();
        let links = topic_source_links(&topics);
        assert_eq!(links.len(), 6, "枢纽仍让每对都连着，只是很轻");
        assert_eq!(
            (links[0].a, links[0].b),
            ("Knowledge/甲.md", "Knowledge/乙.md")
        );
        assert!(links[0].weight > 3.0 * links[1].weight);
        assert_eq!(
            related_topics(&topics)["Knowledge/甲.md"][0],
            "Knowledge/乙.md"
        );
    }

    /// 系统自动打在主题页上的 AI 标签不参与分组——它们不是用户确认过的范围。
    #[test]
    fn ai_tags_on_a_topic_page_do_not_create_groups() {
        let page = "---\nluna:\n  topic: true\n  topic_src: 项目/C\n  ai_tags:\n  - 功能/网页端\n---\n# C\n\n正文。";
        assert_eq!(topic_groups(page), vec!["项目".to_owned()]);
    }

    #[test]
    fn evidence_topic_summary_skips_claim_markers() {
        let body = "# 项目 · C\n\n<!-- luna:claim:01a0d707-0000-7000-8000-000000000000 -->\n\
                    - 项目 C 计划在十一月底完成内测。（[C-排期](../Notes/C-排期.md)）\n\
                    <!-- /luna:claim -->\n";
        let summary = first_meaningful_paragraph(body);
        assert!(
            summary.starts_with("项目 C 计划在十一月底完成内测。"),
            "{summary}"
        );
        assert!(!summary.contains("<!--"));
    }

    #[test]
    fn map_is_sorted_stable_and_uses_first_meaningful_paragraph() {
        let topics = [
            KnowledgeMapTopic {
                path: "Notes/Z.md",
                content: "# Z\n\n[only](x.md)\n\n**First** useful paragraph.\n\nLater",
                updated_ms: None,
            },
            KnowledgeMapTopic {
                path: "Notes/A.md",
                content: "# A\n\nAlpha",
                updated_ms: None,
            },
        ];
        let one = render_knowledge_map("Notes/Map.md", &topics, LABELS_EN);
        let two = render_knowledge_map("Notes/Map.md", &topics, LABELS_EN);
        assert_eq!(one, two);
        assert!(one.find("[A]").unwrap() < one.find("[Z]").unwrap());
        assert!(one.contains("First useful paragraph."));
        assert!(!one.contains("only"));
    }

    /// **地图与主题页目录读同一条摘要规则**（ADR-088 ②）：同一输入 → 同一**字节**。
    ///
    /// 反向验证：把 `topic_summary` 换成任何"给目录另写一份"的实现（哪怕只是多截一个字
    /// 或少剥一层列表符号），下面 `contains` 那一条当场红。这条用例存在的理由是那种分叉
    /// **在界面上看起来完全正常**——两处各自都像一句合理的摘要，只是不是同一句。
    #[test]
    fn the_directory_and_the_map_read_the_same_summary_byte_for_byte() {
        let content =
            "# 交易计划\n\n- **背景**：围绕 [双均线](x.md) 的一轮复盘。\n\n后面还有别的段落。";
        let summary = topic_summary(content);
        assert_eq!(summary, topic_summary(content), "同一输入必须得同一字节");
        assert!(!summary.is_empty());

        let map = render_knowledge_map(
            "Notes/知识地图.md",
            &[KnowledgeMapTopic {
                path: "Notes/交易计划.md",
                content,
                updated_ms: None,
            }],
            LABELS_ZH,
        );
        // 地图里那一行的摘要段与目录行拿到的**是同一串字节**（不是"看起来差不多"）。
        assert!(
            map.contains(&format!(" — {summary}\n")),
            "地图里的摘要与目录不同源：\n{map}\n目录：{summary}"
        );
    }

    #[test]
    fn summary_drops_block_markers_so_humans_read_prose() {
        // 真机上主题页正文首段常常是一串要点，摘要里原样露出 `- ` 就是把 Markdown 语法
        // 摆给人看。反向验证：去掉 `strip_block_marker` 即红。
        let topics = [KnowledgeMapTopic {
            path: "Notes/P.md",
            content: "# P\n\n## 主题归纳\n\n- 会议背景：围绕切换计划。\n- 当前状态：环境为空。",
            updated_ms: None,
        }];
        let out = render_knowledge_map("Notes/Map.md", &topics, LABELS_ZH);
        assert!(out.contains("会议背景：围绕切换计划。"));
        assert!(!out.contains("— - "), "摘要不得以列表符号开头：{out}");
        assert_eq!(
            first_meaningful_paragraph("> 1. 第一条\n> 2. 第二条"),
            "第一条 第二条"
        );
    }
}
