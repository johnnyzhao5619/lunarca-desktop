//! 关系图谱域（PRD-06-034/035）：全库图谱数据命令 + AI 关系标签生成链路。
//!
//! - `vault_graph`：把 `luna_vault::graph` 构建的图谱投影为前端 DTO（渲染零业务，ADR-003）。
//! - AI 标签：`JobKind::ExtractTags` 走单一 AI 主干（ADR-001，路由 `Capability::Custom`
//!   命中 LLM Provider）；结果整体替换 `luna.ai_tags`（用户顶层 `tags` 只读不写，
//!   PRD-06-032），`luna.ai_tags_src` 记生成时正文指纹做幂等门控。
//! - 三触发同一核心（DRY）：① `write_note` 保存后自动（设置 `vault.graph_ai_tags`
//!   门控 + 指纹去抖）；② 编辑器手动入口 `note_graph_tags`（强制重生成）；
//!   ③ 二次转写完成后自动（正文被权威改写，指纹变化自然触发）。
//! - 脱敏纪律（TDD-12）：日志只含路径/错误码，绝不含笔记正文或 LLM 输出。

use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

/// 喂给 LLM 的笔记文本上限（字符）。超长按字符边界截断——标签提炼对开头的标题/
/// 主题信息最敏感，尾部截断可接受（与悬浮菜单 `FLOATING_MENU_INPUT_MAX_CHARS` 同口径值）。
const GRAPH_TAGS_INPUT_MAX_CHARS: usize = 12_000;
/// 词表注入上限：把库内最常用标签提供给 LLM 复用，收敛词汇、提升图谱连通性。
const GRAPH_TAGS_VOCAB_LIMIT: usize = 40;
/// 单篇笔记 AI 标签数上限（Obsidian 心智：标签是主题聚类，不是关键词云）。
const GRAPH_TAGS_MAX: usize = 8;
/// 单个标签长度上限（字符）。超长多为 LLM 跑偏的句子，丢弃。
const GRAPH_TAG_MAX_CHARS: usize = 48;

// ── DTO ─────────────────────────────────────────────────────────────────────

/// 图谱节点 DTO（`luna_vault::graph::GraphNode` 的前端投影）。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct GraphNodeDto {
    /// 稳定 id：笔记与主题 = 相对路径；标签 = `tag:<tag>`；幽灵 = `ghost:<目标小写>`。
    pub id: String,
    /// 展示名。
    pub label: String,
    /// 节点类别：`note` / `tag` / `ghost`；主题级网络（`topic_network`）里恒为 `topic`。
    pub kind: String,
    /// 笔记类型串（仅笔记节点，供着色分组）。
    pub note_type: Option<String>,
    /// 这一篇是**记录态**吗（ADR-086；标签/幽灵节点恒 `false`）。
    ///
    /// 只是**事实**：记录态「默认隐藏、可开」是前端过滤器区的视图态（与资料的类型
    /// 过滤器同形态，PRD-06-052），构图与索引口径不变——孤立判定仍按全图度数
    /// （PRD-06-043 同口径）。
    pub draft: bool,
    /// 连接度（节点尺寸映射）。
    pub degree: u32,
}

/// 图谱边 DTO。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct GraphEdgeDto {
    pub source: String,
    pub target: String,
    /// 边类别：`link` / `tag`；主题级网络里恒为 `source`（两页引用了同一篇来源）。
    pub kind: String,
    /// 关联强度，1–100：主题级网络按 `luna_vault::topic_source_links` 的强度占全图最强一条的
    /// 百分比（画线粗细与布局拉力用，取整数免得前端去猜空值）；笔记级网络恒为 100。
    pub strength: u32,
}

/// 全库关系图谱 DTO。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct VaultGraphDto {
    pub nodes: Vec<GraphNodeDto>,
    pub edges: Vec<GraphEdgeDto>,
}

fn node_kind_str(kind: luna_vault::GraphNodeKind) -> &'static str {
    match kind {
        luna_vault::GraphNodeKind::Note => "note",
        luna_vault::GraphNodeKind::Tag => "tag",
        luna_vault::GraphNodeKind::Ghost => "ghost",
    }
}

impl From<luna_vault::VaultGraph> for VaultGraphDto {
    fn from(graph: luna_vault::VaultGraph) -> Self {
        Self {
            nodes: graph
                .nodes
                .into_iter()
                .map(|n| GraphNodeDto {
                    id: n.id,
                    label: n.label,
                    kind: node_kind_str(n.kind).to_owned(),
                    note_type: n.note_type,
                    draft: n.draft,
                    degree: n.degree,
                })
                .collect(),
            edges: graph
                .edges
                .into_iter()
                .map(|e| GraphEdgeDto {
                    source: e.source,
                    target: e.target,
                    kind: match e.kind {
                        luna_vault::GraphEdgeKind::Link => "link".to_owned(),
                        luna_vault::GraphEdgeKind::Tag => "tag".to_owned(),
                    },
                    strength: 100,
                })
                .collect(),
        }
    }
}

/// 拉取全库关系图谱（PRD-06-034）：节点（笔记/标签/未创建）+ 边（链接/标签归属）。
#[tauri::command]
#[specta::specta]
pub(crate) fn vault_graph(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
) -> Result<VaultGraphDto, String> {
    // 窗口感知（PRD-06-050）：副库窗口看它自己的图谱（只读；索引在开窗时已扫描）。
    state.with_vault(window.label(), |vault| {
        vault.graph().map(Into::into).map_err(vault_err)
    })
}

/// 主题级知识网络（PRD-16-062 / ADR-136 ④）：主题页为节点、共享来源为边（与目录「相关主题」
/// 同一判据 `luna_vault::topic_source_links`，只是不截断）。同一个 DTO 交给同一张画布，节点
/// 类别 `topic`、边类别 `source`。窗口感知同 [`vault_graph`]。只读，零 LLM。
#[tauri::command]
#[specta::specta]
pub(crate) fn topic_network(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
) -> Result<VaultGraphDto, String> {
    state.with_vault(window.label(), |vault| {
        topic_network_of(vault).map_err(vault_err)
    })
}

/// 一条边的强度占最强一条的百分比，落在 1–100（再弱也还连着）。
fn strength_percent(weight: f64, strongest: f64) -> u32 {
    if strongest <= 0.0 {
        return 100;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let percent = (weight / strongest * 100.0).round() as u32;
    percent.clamp(1, 100)
}

fn topic_network_of(vault: &luna_vault::Vault) -> luna_vault::Result<VaultGraphDto> {
    let docs = vault
        .topic_pages()?
        .iter()
        .map(|path| vault.read(path))
        .collect::<luna_vault::Result<Vec<_>>>()?;
    let topics: Vec<luna_vault::KnowledgeMapTopic<'_>> = docs
        .iter()
        .map(|doc| luna_vault::KnowledgeMapTopic {
            path: &doc.path,
            content: &doc.content,
            updated_ms: None,
        })
        .collect();
    let links = luna_vault::topic_source_links(&topics);
    let degree = |path: &str| {
        let count = links
            .iter()
            .filter(|link| link.a == path || link.b == path)
            .count();
        u32::try_from(count).unwrap_or(u32::MAX)
    };
    // 已按强度降序：第一条就是全图最强的那条。
    let strongest = links.first().map_or(1.0, |link| link.weight);
    Ok(VaultGraphDto {
        nodes: docs
            .iter()
            .map(|doc| GraphNodeDto {
                id: doc.path.clone(),
                label: luna_sync_core::links::title_of(&doc.path).to_owned(),
                kind: "topic".to_owned(),
                note_type: None,
                draft: false,
                degree: degree(&doc.path),
            })
            .collect(),
        edges: links
            .iter()
            .map(|link| GraphEdgeDto {
                source: link.a.to_owned(),
                target: link.b.to_owned(),
                kind: "source".to_owned(),
                strength: strength_percent(link.weight, strongest),
            })
            .collect(),
    })
}

// ── AI 关系标签 ─────────────────────────────────────────────────────────────

/// 单篇标签生成的触发模式：三个触发面在「指纹门控 / 队列优先级 / 写回竞态处置」上
/// 各需不同组合，收敛为一个枚举而不是散落的布尔参数（`force` 一个布尔已经装不下
/// 「强制重生成但走后台优先级」这第三种组合）。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TagRefreshMode {
    /// 自动触发（保存 / 二次转写后）：指纹未变即跳过；后台优先级；生成期间正文又变
    /// 则放弃本次（下次保存自然重触发）。
    Auto,
    /// 编辑器手动入口：强制重生成；交互优先级（用户在等）；生成期间被修改如实报错。
    Manual,
    /// 全库重扫「全部重新生成」档：强制重生成但走后台优先级（批量任务不与交互抢队），
    /// 生成期间被修改则放弃本次，与自动路径同处置。
    RescanForce,
}

/// 注入词表的语言过滤（PRD-06-035「标签是库级词汇表」）。
///
/// **词表的作用是示范正确的形状，那就只能放形状正确的**。真机第三轮坐实了不过滤的
/// 代价：库当时中英分裂，而提示词一边说「用中文」、一边把 `ops`、`cutover-migration`
/// 作为「优先复用的既有标签」喂进去——于是**同一篇笔记的同一次生成里同时吐出
/// `运维` 和 `ops`**（5 篇笔记里 `#运维` 挂 5 篇、`#ops` 挂 4 篇）。指令与范例互相矛盾时
/// 模型跟范例走，而那正是我们自己给的。
///
/// **专名放行**（真机第四轮的教训，判定在 [`luna_vault::tag_fits_language`]，与语言
/// 盘点同一份）：第一版把 `Datadog`、`I2X`、`AWS` 也滤出了词表——语言分裂确实堵住了，
/// 但失去"优先复用"的锚之后每篇各自发挥，标签总数 8 → 22，单篇碎标签横生。专名与缩写
/// 在中文库里本就保留原样（提示词明写），它们是最该被复用的一批词，不是错语言。
fn filter_vocabulary(tags: Vec<String>, zh: bool) -> Vec<String> {
    tags.into_iter()
        .filter(|tag| luna_vault::tag_fits_language(tag, zh))
        .collect()
}

/// 合成标签提取指令（system prompt）。调用方决策、引擎只渲染（TDD-03 §3.3 同哲学）：
/// 注入库内既有词表促进复用（共享标签 = 图谱连通边），输出契约为纯 JSON 数组。
fn extract_tags_instruction(vocabulary: &[String], zh: bool) -> luna_core::Instruction {
    // 措辞归注册表 `tags` 域（ADR-050 修订 2026-09-05）；语言那一句按 locale 变体取
    // （`zh` = 界面语言是中文，与 `is_zh` 同源），显式指定而不读注册表当前 locale，
    // 是为了让英文进程里的单测也能造中文样本。
    let registry = luna_core::prompt::registry();
    let locale = if zh { Some("zh-CN") } else { None };
    let tags = |key: &str, vars: &[(&str, &str)]| {
        registry.render_for_locale(luna_core::PromptScope::Tags, key, locale, vars)
    };
    let vocab = if vocabulary.is_empty() {
        String::new()
    } else {
        tags("vocabulary", &[("tags", &vocabulary.join(", "))])
    };
    // **标签语言跟随界面语言，而不是跟随笔记内容**（2026-07-29 真机第一轮的裁决）。
    // 此前写的是"用与笔记内容相同的语言"，于是中英混排的库必然分裂：同一个概念在中文笔记
    // 上叫「趋势分析」、在英文笔记上叫 `Trend_Analysis`，两个标签各挂一篇——而主题页建议的
    // 判据是「同标签 ≥3 篇」，簇永远凑不够（真机库里 15 条标签**无一跨篇重合**）。
    // 标签是**库级词汇表**而不是笔记的属性：一个库一种语言，才谈得上"共享标签"。
    // **层级标签的每一段各自遵守同一条语言规则**（2026-08-30 真机第三轮）。上面那条只说
    // 了「所有标签用 X 语言」，没说层级的各段——于是模型产出了 `architecture/三层架构`、
    // `data-integration/real-time-data`、`ops/planning` 这种**一个标签内部就混着两种语言**
    // 的东西。后果与库级分裂同源：父级 `architecture` 与 `架构` 各挂一半笔记，标签作为
    // 「库级词汇表」的收敛作用再打一次折。产品名与缩写仍然保留原样，但那是**按词**豁免
    // （Datadog、AWS、iTWOx），不是按位置豁免——不能因为它在父级上就整段不翻。
    let language = tags("language", &[]);
    registry.instruction_for_locale(
        luna_core::PromptScope::Tags,
        "system",
        locale,
        &[("vocabulary", &vocab), ("language", &language)],
    )
}

/// 去掉 LLM 输出中的 `<think>…</think>` 推理块（小模型常见），避免其内部括号干扰
/// JSON 数组定位。未闭合的 think 块丢弃其后全部内容（推理未完成即无正式输出）。
pub(crate) fn strip_think_blocks(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(start) = rest.find("<think>") {
        out.push_str(&rest[..start]);
        match rest[start..].find("</think>") {
            Some(end) => rest = &rest[start + end + "</think>".len()..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// 单个标签清洗：去 `#`/引号/反引号包裹、空白折叠为 `-`、剔除标签体外字符、
/// 修剪边缘分隔符。空/超长/纯数字返回 `None`（与 `luna_vault::parse` 内联标签
/// 字符集同构：字母数字 / `_` / `-` / `/`）。
fn sanitize_tag(raw: &str) -> Option<String> {
    let trimmed = raw
        .trim()
        .trim_matches(|c: char| c == '"' || c == '\'' || c == '`')
        .trim_start_matches('#')
        .trim();
    let mut out = String::with_capacity(trimmed.len());
    let mut last_dash = false;
    for ch in trimmed.chars() {
        let mapped = if ch.is_whitespace() { '-' } else { ch };
        let allowed = mapped.is_alphanumeric() || mapped == '_' || mapped == '-' || mapped == '/';
        if !allowed {
            continue;
        }
        if mapped == '-' && last_dash {
            continue; // 折叠连续 '-'（多空白/混合分隔产生）。
        }
        last_dash = mapped == '-';
        out.push(mapped);
    }
    let out = out.trim_matches(['-', '/', '_']).to_string();
    if out.is_empty()
        || out.chars().count() > GRAPH_TAG_MAX_CHARS
        || out.chars().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    Some(out)
}

/// 解析 LLM 输出为标签列表：优先 JSON 数组（容忍前后杂文），退化按行/逗号切分；
/// 逐项清洗 + 大小写不敏感去重 + 上限截断。解析不出返回空（调用方诚实跳过，不猜）。
fn parse_tag_list(raw: &str) -> Vec<String> {
    let cleaned = strip_think_blocks(raw);
    let json_slice = cleaned.find('[').and_then(|start| {
        cleaned[start..]
            .rfind(']')
            .map(|end| &cleaned[start..=start + end])
    });
    let candidates: Vec<String> = json_slice
        .and_then(|slice| serde_json::from_str::<Vec<String>>(slice).ok())
        .unwrap_or_else(|| {
            // 退化路径（行/逗号列表）：只接受「像标签」的短条目——含句子标点或词数过多的
            // 是模型在说话（如 "I could not find any tags."），不是标签，整条丢弃。
            cleaned
                .lines()
                .flat_map(|line| line.split(','))
                .map(|item| {
                    item.trim()
                        .trim_start_matches(['-', '*', '•'])
                        .trim()
                        .to_owned()
                })
                .filter(|item| {
                    !item.contains(['.', '。', '!', '！', '?', '？', ':', '：', ';', '；'])
                        && item.split_whitespace().count() <= 3
                })
                .collect()
        });
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out = Vec::new();
    for candidate in candidates {
        let Some(tag) = sanitize_tag(&candidate) else {
            continue;
        };
        if seen.insert(tag.to_lowercase()) {
            out.push(tag);
            if out.len() == GRAPH_TAGS_MAX {
                break;
            }
        }
    }
    out
}

/// 按字符边界截断输入文本。
fn truncate_chars(text: &str, max_chars: usize) -> &str {
    match text.char_indices().nth(max_chars) {
        Some((idx, _)) => &text[..idx],
        None => text,
    }
}

/// 稳定错误码是不是「同一笔记正在生成中」——它不是事故：撞忙的那次触发已经把
/// 「事后补跑」旗标置上（见 [`generate_and_apply`] 的合并语义），日志与连败计数
/// 都不该把它当失败记账（重扫侧同一裁决，见本文件连败判定处）。
pub(crate) fn is_busy_code(code: &str) -> bool {
    code.starts_with("GRAPH_TAGS_BUSY")
}

/// 生成入口：跑一次核心，随后**补跑合并的期间保存**（PRD-06-035，2026-08-24 修订）。
///
/// 本地模型一次生成要几十秒，而编辑器打字期间每次自动保存都会再触发一次——旧实现逐次
/// 撞上 in-flight、逐条刷 `GRAPH_TAGS_BUSY` 日志，且**最后一次保存的内容拿不到标签**
/// （生成结果基于开跑那一刻的旧快照，陈旧到下一次保存为止）。现在撞忙只置旗标；
/// 持有 in-flight 名额的那次运行结束后检查旗标，被合并过就以自动档**补跑一次**——
/// 指纹门控（`ai_tags_src`）保证内容其实没变时补跑零成本，打字停下后旗标不再置上、
/// 循环自然收敛。
///
/// 返回值恒为**第一次运行**的结果（调用方的契约不变：手动档的 BUSY 仍如实报给用户）；
/// 补跑属于被合并的自动保存，失败只留脱敏日志。
pub(crate) async fn generate_and_apply(
    app: &tauri::AppHandle,
    note_rel: &str,
    mode: TagRefreshMode,
) -> Result<Vec<String>, String> {
    let first = generate_and_apply_once(app, note_rel, mode).await;
    if matches!(&first, Err(code) if is_busy_code(code)) {
        // 没拿到名额的出口：旗标已在核心里置上，补跑归持有名额的那一次负责。
        return first;
    }
    loop {
        let rerun = app
            .state::<AppState>()
            .graph_tags_pending
            .lock()
            .ok()
            .map(|mut pending| pending.remove(note_rel))
            .unwrap_or(false);
        if !rerun {
            break;
        }
        if let Err(code) = generate_and_apply_once(app, note_rel, TagRefreshMode::Auto).await {
            if !is_busy_code(code.as_str()) {
                eprintln!("[graph-tags] 合并补跑失败：{code}");
            }
        }
    }
    first
}

/// 生成核心：读笔记 → 合成 Job（单一 AI 主干）→ 解析 → 幂等写回 → 广播。
/// 指纹门控 / 优先级 / 竞态处置由 [`TagRefreshMode`] 决定（三个触发面 DRY 同一核心）。
///
/// 返回应用后的标签列表。错误为稳定错误码前缀的脱敏消息。
async fn generate_and_apply_once(
    app: &tauri::AppHandle,
    note_rel: &str,
    mode: TagRefreshMode,
) -> Result<Vec<String>, String> {
    let state = app.state::<AppState>();

    // 发起时的库代际（ADR-025）：写回前校验，期间切过库即放弃——`note_rel` 是库相对路径，
    // 写进新库会改错文件。
    let generation = state.vault_generation();

    // 阶段 1：锁内快照输入（不跨 await 持锁）。
    // 标签语言跟随界面语言（见 `extract_tags_instruction`）：库级词汇表要统一，
    // 否则同一个概念在中英笔记上各长一个标签，簇永远凑不够。
    // 提到读笔记之前：注入词表也要按它过滤——词表是"正确形状的范例"，混着另一种语言
    // 的范例会把指令本身抵消掉（见 `filter_vocabulary`）。
    let zh = super::knowledge::is_zh(app);
    let (title, body, src_hash, prev_tags, prev_src, vocabulary) = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        let doc = vault.read(note_rel).map_err(vault_err)?;
        // 知识地图不打标——**任何模式**（PRD-16-023 修订）。手动入口对类型/成熟度
        // 白名单的绕过理由是"显式动作是明确意图"，但那放行的是**噪声取舍**；地图的标签
        // 是**类别错误**（导航页链接所有主题，打标只会给库级词汇表灌水），没有哪种意图
        // 能让它变对。看门放在三种模式共用的这一个入口，不在三个触发面各写一遍。
        if doc.meta.knowledge_map {
            return Err(coded_error(
                app,
                "GRAPH_TAGS_KNOWLEDGE_MAP",
                "graph-tags-knowledge-map",
            ));
        }
        let body = luna_vault::body_of(&doc.content).to_owned();
        let hash = luna_sync_core::hashing::ContentHash::of(body.as_bytes())
            .as_str()
            .to_owned();
        // **先按语言过滤再取上限**，不是先截断再过滤：反过来的话，一个中英分裂的库
        // 里前 40 条可能大半是另一种语言，过滤完只剩几条，词表就等于没注入。
        let vocabulary: Vec<String> = filter_vocabulary(
            vault
                .list_tags()
                .map_err(vault_err)?
                .into_iter()
                .map(|t| t.tag)
                .collect(),
            zh,
        )
        .into_iter()
        .take(GRAPH_TAGS_VOCAB_LIMIT)
        .collect();
        (
            luna_sync_core::links::title_of(&doc.path),
            body,
            hash,
            doc.meta.ai_tags.clone(),
            doc.meta.ai_tags_src.clone(),
            vocabulary,
        )
    };
    if body.trim().is_empty() {
        return Err(coded_error(app, "GRAPH_TAGS_EMPTY", "graph-tags-empty"));
    }
    if mode == TagRefreshMode::Auto && prev_src.as_deref() == Some(src_hash.as_str()) {
        return Ok(prev_tags); // 正文未变：幂等跳过（不重复调用 LLM）。
    }

    // 阶段 2：in-flight 防重（同一笔记并发生成只留一个）。撞忙不是丢弃：置上合并旗标，
    // 持有名额的那次运行结束后按最新内容补跑一次（[`generate_and_apply`]）。
    {
        let mut inflight = state
            .graph_tags_inflight
            .lock()
            .map_err(|_| "图谱标签锁中毒".to_string())?;
        if !inflight.insert(note_rel.to_owned()) {
            if let Ok(mut pending) = state.graph_tags_pending.lock() {
                pending.insert(note_rel.to_owned());
            }
            return Err(coded_error(app, "GRAPH_TAGS_BUSY", "graph-tags-busy"));
        }
    }
    let outcome = run_extract_job(
        app,
        &state,
        note_rel,
        &title,
        &body,
        &src_hash,
        &vocabulary,
        zh,
        mode,
        generation,
    )
    .await;
    if let Ok(mut inflight) = state.graph_tags_inflight.lock() {
        inflight.remove(note_rel);
    }
    outcome
}

/// 生成核心的 Job 段（in-flight 已登记后进入；任何出口由调用方清理登记）。
#[allow(clippy::too_many_arguments)]
async fn run_extract_job(
    app: &tauri::AppHandle,
    state: &AppState,
    note_rel: &str,
    title: &str,
    body: &str,
    src_hash: &str,
    vocabulary: &[String],
    // 界面语言是否为中文。**由调用方传入而不是这里再读一次**：词表过滤与指令语言必须
    // 出自同一个判断，两处各读一次的话，语言在生成途中被改会让范例与指令说两种话。
    zh: bool,
    mode: TagRefreshMode,
    generation: u64,
) -> Result<Vec<String>, String> {
    let input = format!(
        "Title: {title}\n\n{}",
        truncate_chars(body, GRAPH_TAGS_INPUT_MAX_CHARS)
    );
    let mut job = Job::new(
        JobKind::ExtractTags {
            text: TextRef::Inline(input),
            instruction: extract_tags_instruction(vocabulary, zh),
        },
        JobOrigin::GraphTags,
    );
    // 手动入口用户在等 → 交互式；自动触发与全库重扫走后台，不与交互任务抢队。
    job.priority = if mode == TagRefreshMode::Manual {
        Priority::Interactive
    } else {
        Priority::Background
    };
    job.routing.local_only = state.strict_local();

    let mut handle = state.queue.submit(job);
    let mut final_text: Option<String> = None;
    while let Some(event) = handle.recv().await {
        match event {
            JobEvent::Finished(result) => {
                if let ResultPayload::Text(text) = result.payload {
                    final_text = Some(text);
                }
                break;
            }
            JobEvent::Failed(err) => {
                return Err(format!("GRAPH_TAGS_JOB: {}", err.code()));
            }
            JobEvent::Cancelled => {
                return Err(coded_error(
                    app,
                    "GRAPH_TAGS_CANCELLED",
                    "graph-tags-cancelled",
                ));
            }
            _ => {}
        }
    }
    let raw =
        final_text.ok_or_else(|| coded_error(app, "GRAPH_TAGS_JOB", "graph-tags-no-result"))?;
    let tags = parse_tag_list(&raw);
    if tags.is_empty() {
        // 诚实失败：解析不出标签绝不清空既有 AI 标签（LLM 输出跑偏时保留现状）。
        return Err(coded_error(app, "GRAPH_TAGS_PARSE", "graph-tags-parse"));
    }

    // 阶段 3：写回。两重校验缺一不可——**库**还是不是那个库（切库后写回会改错库的
    // 同名笔记，ADR-025），以及**正文**有没有被并发改过（防把旧标签盖到新内容上）。
    if !state.still_current_vault(generation) {
        return Err(coded_error(
            app,
            "GRAPH_TAGS_VAULT_SWITCHED",
            "graph-tags-vault-switched",
        ));
    }
    let written = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        let doc = vault.read(note_rel).map_err(vault_err)?;
        let now_hash =
            luna_sync_core::hashing::ContentHash::of(luna_vault::body_of(&doc.content).as_bytes());
        if now_hash.as_str() != src_hash {
            if mode == TagRefreshMode::Manual {
                return Err(coded_error(app, "GRAPH_TAGS_STALE", "graph-tags-stale"));
            }
            // 自动/重扫路径：内容又变了，放弃本次——下次保存自然重新触发。
            return Ok(doc.meta.ai_tags);
        }
        vault
            .set_ai_tags(note_rel, &tags, src_hash)
            .map_err(vault_err)?
    };
    if let Some(content) = written {
        // 同一笔记的打开编辑器增量跟进（note://written）；标签面板/图谱视图重载（vault://changed）。
        let _ = (NoteWritten {
            path: note_rel.to_owned(),
            content,
            origin: "main".to_owned(),
        })
        .emit(app);
        emit_vault_changed(app, note_rel, false);
    }
    Ok(tags)
}

/// 自动触发入口（保存 / 二次转写完成，DRY 单一函数）：设置 `vault.graph_ai_tags` 门控 +
/// 排除文件夹判据（PRD-06-035c，与重扫候选筛选共用 `luna_vault::is_tag_excluded`），
/// 后台生成，失败只留脱敏日志（错误码，无正文）不打扰用户。
pub(crate) fn spawn_graph_tag_refresh(app: &tauri::AppHandle, note_rel: &str) {
    let state = app.state::<AppState>();
    let enabled = state
        .settings_snapshot()
        .get_bool("vault.graph_ai_tags")
        .unwrap_or(true);
    if !enabled || !note_rel.ends_with(".md") {
        return;
    }
    // 两道并集判据（ADR-024 / ADR-086）：白名单答「哪类内容值得打标」（`note` 且非记录态
    // ——判据本体在 `NoteStage::allows_auto_ai_tags`，与重扫盘点共用同一个函数），
    // 文件夹排除答「这片区域不要打标」。任一命中即跳过；两者都不是故障，故静默
    // （用户划出的边界不该产生日志噪声）。
    let skip = state
        .vault
        .lock()
        .ok()
        .and_then(|guard| {
            let vault = guard.as_ref()?;
            let meta = vault.read(note_rel).map(|doc| doc.meta).unwrap_or_default();
            // 知识地图豁免与重扫盘点同一条（理由见 `generate_and_apply_once` 的看门）。
            let ineligible =
                meta.knowledge_map || !meta.stage().allows_auto_ai_tags(meta.note_type);
            let folders = vault.graph_tag_exclusions().ok()?;
            Some(ineligible || luna_vault::is_tag_excluded(note_rel, &folders))
        })
        .unwrap_or(false);
    if skip {
        return;
    }
    let app = app.clone();
    let note_rel = note_rel.to_owned();
    tauri::async_runtime::spawn(async move {
        if let Err(code) = generate_and_apply(&app, &note_rel, TagRefreshMode::Auto).await {
            // 幂等跳过/空正文等非事故路径也走 Err 之外的 Ok 分支；此处仅记录错误码（脱敏）。
            // BUSY 静默：撞忙 = 已合并进在跑的那一次（结束后补跑），不是失败——打字期间
            // 每次自动保存刷一行「正在生成中」正是 2026-08-24 真机反馈的日志噪声。
            if !is_busy_code(code.as_str()) {
                eprintln!("[graph-tags] 自动生成跳过或失败：{code}");
            }
        }
    });
}

// ── 生成范围：排除文件夹（PRD-06-035c）────────────────────────────────────────

/// 读取排除文件夹清单（文件树右键菜单勾选态 + 确认卡呈现共用）。
#[tauri::command]
#[specta::specta]
pub(crate) fn graph_tag_exclusions(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<String>, String> {
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    vault.graph_tag_exclusions().map_err(vault_err)
}

/// 增删一条排除文件夹（文件树右键「停用/启用 AI 关系标签」）。返回更新后的完整清单；
/// 广播 `vault://changed`（`.luna/graph_scope.json`）让各处菜单勾选态同步刷新。
#[tauri::command]
#[specta::specta]
pub(crate) fn set_graph_tag_exclusion(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    folder: String,
    excluded: bool,
) -> Result<Vec<String>, String> {
    let list = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        vault
            .set_graph_tag_exclusion(&folder, excluded)
            .map_err(vault_err)?
    };
    emit_vault_changed(&app, ".luna/graph_scope.json", false);
    Ok(list)
}

/// 手动生成/更新关系标签（编辑器入口，PRD-06-035）：绕过设置与指纹门控强制重生成，
/// 返回应用后的标签列表供前端即时反馈。
#[tauri::command]
#[specta::specta]
pub(crate) async fn note_graph_tags(
    app: tauri::AppHandle,
    path: String,
) -> Result<Vec<String>, String> {
    let tags = generate_and_apply(&app, &path, TagRefreshMode::Manual).await?;
    // 文件照写（AI 标签归 `luna:` 块），但全库忽略的标签不回给界面——与标签树、图谱同一事实。
    let ignored = {
        let state = app.state::<AppState>();
        let guard = state.vault.lock().map_err(|_| state.vault_not_open())?;
        guard
            .as_ref()
            .and_then(|vault| vault.ai_tag_ignores().ok())
            .unwrap_or_default()
    };
    Ok(tags
        .into_iter()
        .filter(|tag| !ignored.iter().any(|i| i.eq_ignore_ascii_case(tag)))
        .collect())
}

/// 一个被全库忽略的 AI 关系标签与它此刻压下了几篇笔记（PRD-16-051）。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct IgnoredAiTagDto {
    pub tag: String,
    pub note_count: u32,
}

/// 已忽略的 AI 关系标签（「已忽略」清单与恢复入口用）。
#[tauri::command]
#[specta::specta]
pub(crate) fn ignored_ai_tags(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<IgnoredAiTagDto>, String> {
    let guard = state.vault.lock().map_err(|_| state.vault_not_open())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    Ok(vault
        .ignored_ai_tags()
        .map_err(vault_err)?
        .into_iter()
        .map(|(tag, note_count)| IgnoredAiTagDto { tag, note_count })
        .collect())
}

/// 全库忽略或恢复一个 AI 关系标签（PRD-16-051）：只改随库清单与派生索引，笔记文件一字
/// 不动；标签树、图谱、主题候选与检索过滤同时生效。改完广播库变化，各视图按既有链路重取。
#[tauri::command]
#[specta::specta]
pub(crate) fn set_ai_tag_ignored(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    tag: String,
    ignored: bool,
) -> Result<(), String> {
    {
        let guard = state.vault.lock().map_err(|_| state.vault_not_open())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        vault.set_ai_tag_ignored(&tag, ignored).map_err(vault_err)?;
    }
    // 不对应某一篇文件（空路径）：只为让各视图按既有链路重取，并顺带排一次地图刷新。
    emit_vault_changed(&app, "", false);
    Ok(())
}

// ── 全库重扫（PRD-06-035 全库重扫入口，图谱视图承载） ──────────────────────────

/// 连续失败即中止的阈值：引擎不可用时每一篇都会失败，让批量任务把几十篇错误磨完
/// 既浪费时间也给不出新信息——连续 3 篇失败视为环境性故障，提前诚实中止并告知。
const RESCAN_HALT_AFTER_CONSECUTIVE_FAILURES: u32 = 3;

/// 全库重扫运行态（`AppState` 持有）：取消令牌在场 = 有一次重扫在跑（单例，杜绝并行
/// 重扫互相踩踏）；进度快照供图谱视图重挂载时恢复显示（事件是流、快照是状态，缺一不可）。
#[derive(Default)]
pub(crate) struct GraphRescan {
    pub(crate) cancel: Option<Arc<AtomicBool>>,
    progress: Option<GraphRescanProgress>,
}

/// 重扫进度事件（`graph://rescan`）：逐篇低频（每篇一次 LLM 生成，秒级间隔），
/// 无需节流。终态帧 `finished = true` 且三种收场互斥可辨（跑完 / 取消 / 中止）。
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[tauri_specta(event_name = "graph://rescan")]
pub struct GraphRescanProgress {
    /// 已处理篇数（含失败）。
    pub done: u32,
    /// 本次重扫总篇数。
    pub total: u32,
    /// 失败篇数（解析失败等单篇故障；不含「另一触发面正在生成」这类良性竞争）。
    pub failed: u32,
    /// 重扫范围：`None` = 整个库，`Some(文件夹)` = 定向重扫（进度条据此标注对象）。
    pub scope: Option<String>,
    /// 终态标记：跑完 / 被取消 / 连续失败中止均置位。
    pub finished: bool,
    /// 用户主动取消。
    pub cancelled: bool,
    /// 连续失败提前中止（引擎不可用的典型指纹）——前端据此引导检查 AI 引擎。
    pub halted: bool,
}

/// 重扫预检 DTO：确认卡据此陈述代价（扫多少篇、跳过多少篇、多少篇被排除规则挡在
/// 范围外）——数字在点开始之前给，而不是开始之后才发现。
#[derive(Debug, Clone, Serialize, Type)]
pub(crate) struct GraphRescanPreflightDto {
    /// 指纹已过期（含从未生成）的笔记数——「只补缺口」将扫这些。
    pub stale: u32,
    /// 指纹仍最新的笔记数——只有「全部重新生成」会连这些一起重扫。
    pub fresh: u32,
    /// 正文为空、无从提炼的笔记数（不参与重扫）。
    pub empty: u32,
    /// **类型白名单外**的篇数（资料区与速记，ADR-024）。这一档是**设计**，用户无须处置。
    pub excluded_type: u32,
    /// 落在**用户停用的文件夹**内的篇数（PRD-06-035c）。
    ///
    /// 与上一档**分开报**（2026-07-30 的 D7）：合成一个"27 篇在排除范围"时，用户无从
    /// 知道其中有 15 篇是**他自己某次误点停用了整个文件夹**——而那一下让整个知识面静默
    /// 失效（标签长不出来 → 主题页建议永不触发 → 归入链路整条够不着，真机连撞三轮）。
    pub excluded_folder: u32,
    /// 此刻被停用的文件夹清单（确认卡据此**列出规则本身**并给就地取消）。
    ///
    /// 规则的**结果**可见还不够：只报数字时，用户看得见"被挡住了"却看不见"是什么挡的"，
    /// 更没有入口去改。这一项与 `excluded_folder` 成对存在。
    pub excluded_folders: Vec<String>,
    /// 是否已有一次重扫在进行中（是则确认卡改呈现进行中状态，不给第二个开始按钮）。
    pub running: bool,
    /// 生成此刻**实际**在哪跑：知识管线与对话走同一条 LLM 主干（ADR-001），落点判据同一个
    /// （ADR-059 ③，与中止通知的 `executed_at` 同源）。确认卡据此陈述执行位置——此前那句
    /// 固定的「生成在本地逐篇进行」在连着服务端时是假话（2026-10-08 文案审查 F02 / C17）。
    pub execution: crate::cloud::ComputeLocation,
}

fn clamp_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// 重扫预检（确认卡）：盘点范围内各篇标签新鲜度 + 是否已在跑。只读不写。
/// `scope = None` 为整库，`Some(文件夹)` 为定向（文件树右键入口，PRD-06-035c）。
#[tauri::command]
#[specta::specta]
pub(crate) fn graph_rescan_preflight(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    scope: Option<String>,
) -> Result<GraphRescanPreflightDto, String> {
    let (scan, exclusions) = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        (
            vault.graph_tag_scan(scope.as_deref()).map_err(vault_err)?,
            // 规则本身随预检一起下发：确认卡要能**列出**被停用的文件夹并给就地取消，
            // 而不是只报一个"N 篇被挡住了"的数字（D7）。
            vault.graph_tag_exclusions().map_err(vault_err)?,
        )
    };
    let running = state
        .graph_rescan
        .lock()
        .map_err(|_| "重扫状态锁中毒".to_string())?
        .cancel
        .is_some();
    Ok(GraphRescanPreflightDto {
        stale: clamp_u32(scan.stale.len()),
        fresh: clamp_u32(scan.fresh.len()),
        empty: clamp_u32(scan.empty),
        excluded_type: clamp_u32(scan.excluded_type),
        excluded_folder: clamp_u32(scan.excluded_folder),
        excluded_folders: exclusions,
        running,
        execution: super::current_chat_compute_location(&app, &state),
    })
}

/// 启动重扫：`force = false` 只补缺口（指纹过期/从未生成），`force = true` 全部
/// 重新生成；`scope` 圈定文件夹（`None` = 整库）。返回本次重扫总篇数；单例——
/// 已有重扫在跑则拒绝第二次（整库与定向共用同一单例，杜绝互相踩踏）。
///
/// 批量任务逐篇顺序执行（单一 AI 主干后台优先级，不与交互任务抢队；也不并发轰炸
/// 本地 LLM），每篇完成即发 `graph://rescan` 进度；图谱本身随每篇写回的
/// `vault://changed` 增量变密，不必等整批结束。
#[tauri::command]
#[specta::specta]
pub(crate) fn graph_rescan_start(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    force: bool,
    scope: Option<String>,
) -> Result<u32, String> {
    let scan = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        vault.graph_tag_scan(scope.as_deref()).map_err(vault_err)?
    };
    // 缺口恒在扫描范围且排在前面（先补从未生成的，再刷新已有的）；force 才纳入已最新。
    let mut targets = scan.stale;
    if force {
        targets.extend(scan.fresh);
    }
    if targets.is_empty() {
        return Err(coded_error(
            &app,
            "GRAPH_RESCAN_EMPTY",
            "graph-rescan-empty",
        ));
    }
    let total = clamp_u32(targets.len());
    let cancel = Arc::new(AtomicBool::new(false));
    let initial = GraphRescanProgress {
        done: 0,
        total,
        failed: 0,
        scope: scope.clone(),
        finished: false,
        cancelled: false,
        halted: false,
    };
    {
        let mut guard = state
            .graph_rescan
            .lock()
            .map_err(|_| "重扫状态锁中毒".to_string())?;
        if guard.cancel.is_some() {
            return Err(coded_error(&app, "GRAPH_RESCAN_BUSY", "graph-rescan-busy"));
        }
        guard.cancel = Some(Arc::clone(&cancel));
        guard.progress = Some(initial.clone());
    }
    let _ = initial.emit(&app);
    let mode = if force {
        TagRefreshMode::RescanForce
    } else {
        TagRefreshMode::Auto
    };
    tauri::async_runtime::spawn(async move {
        run_rescan(app, targets, mode, scope, cancel).await;
    });
    Ok(total)
}

/// 请求取消进行中的重扫。当前那一篇的生成照常收尾（LLM Job 已在途），下一篇前停下——
/// 前端文案如实说「当前篇完成后停止」。无重扫在跑时为 no-op（幂等）。
#[tauri::command]
#[specta::specta]
pub(crate) fn graph_rescan_cancel(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let guard = state
        .graph_rescan
        .lock()
        .map_err(|_| "重扫状态锁中毒".to_string())?;
    if let Some(cancel) = guard.cancel.as_ref() {
        cancel.store(true, Ordering::Relaxed);
    }
    Ok(())
}

/// 查询重扫进度快照（图谱视图挂载时恢复进行中的进度条）；`None` = 无重扫在跑。
#[tauri::command]
#[specta::specta]
pub(crate) fn graph_rescan_status(
    state: tauri::State<'_, AppState>,
) -> Result<Option<GraphRescanProgress>, String> {
    Ok(state
        .graph_rescan
        .lock()
        .map_err(|_| "重扫状态锁中毒".to_string())?
        .progress
        .clone())
}

/// 重扫执行体：逐篇生成 → 逐篇报进度 → 终态收尾（清运行态 + 完成/中止通知）。
async fn run_rescan(
    app: tauri::AppHandle,
    targets: Vec<String>,
    mode: TagRefreshMode,
    scope: Option<String>,
    cancel: Arc<AtomicBool>,
) {
    let total = clamp_u32(targets.len());
    let mut progress = GraphRescanProgress {
        done: 0,
        total,
        failed: 0,
        scope,
        finished: false,
        cancelled: false,
        halted: false,
    };
    let mut consecutive_failures: u32 = 0;
    for rel in &targets {
        if cancel.load(Ordering::Relaxed) {
            progress.cancelled = true;
            break;
        }
        match generate_and_apply(&app, rel, mode).await {
            Ok(_) => {
                consecutive_failures = 0;
            }
            Err(code) => {
                // BUSY = 同一笔记正被别的触发面生成（保存自动触发等）——结果一样会来，
                // 不算失败也不计入连败。
                if !is_busy_code(code.as_str()) {
                    progress.failed += 1;
                    consecutive_failures += 1;
                    // 脱敏纪律（TDD-12）：只留错误码与路径，绝不含正文或 LLM 输出。
                    eprintln!("[graph-rescan] {rel} 生成失败：{code}");
                }
            }
        }
        progress.done += 1;
        if consecutive_failures >= RESCAN_HALT_AFTER_CONSECUTIVE_FAILURES {
            progress.halted = true;
            break;
        }
        if !progress.cancelled && !progress.halted && progress.done < total {
            publish_rescan_progress(&app, progress.clone());
        }
    }
    progress.finished = true;
    // 先清运行态再发终态帧：订阅方收到 finished 后再查 status 必然是「无重扫在跑」。
    {
        let state = app.state::<AppState>();
        if let Ok(mut guard) = state.graph_rescan.lock() {
            guard.cancel = None;
            guard.progress = None;
        };
    }
    let _ = progress.clone().emit(&app);
    notify_rescan_outcome(&app, &progress);
}

/// 进度帧：事件（流）与快照（状态）同步更新，视图重挂载靠快照恢复。
fn publish_rescan_progress(app: &tauri::AppHandle, progress: GraphRescanProgress) {
    let state = app.state::<AppState>();
    if let Ok(mut guard) = state.graph_rescan.lock() {
        guard.progress = Some(progress.clone());
    }
    let _ = progress.emit(app);
}

/// 终态通知：重扫是典型的「点了开始就切去做别的」长任务（PRD-15 Completion 语义），
/// 跑完/中止发通知把用户带回图谱；用户主动取消不发（他知道自己做了什么）。
fn notify_rescan_outcome(app: &tauri::AppHandle, progress: &GraphRescanProgress) {
    if progress.cancelled {
        return;
    }
    let Some(nc) = app.try_state::<crate::notifications::NotificationCenter>() else {
        return;
    };
    let target = crate::notifications::NotificationTarget::Route("graph://".to_owned());
    let event = if progress.halted {
        // 中止文案必须指向**真正的执行落点**：知识管线走的是与对话同一条 LLM 主干
        // （ADR-001），故落点判据也用同一个（ADR-059 ③）。
        let location = super::current_chat_compute_location(app, &app.state::<AppState>());
        crate::notifications::NotificationEvent::failure("graph_tags_rescan_halted", target)
            .executed_at(location)
    } else {
        crate::notifications::NotificationEvent::completion(
            "graph_tags_rescanned",
            target,
            crate::notifications::PrivatePayload::new(String::new()),
        )
    };
    nc.publish(event, app);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_strength_is_a_percent_of_the_strongest_and_never_zero() {
        assert_eq!(strength_percent(1.5, 1.5), 100);
        assert_eq!(strength_percent(0.5, 1.5), 33);
        assert_eq!(strength_percent(0.001, 1.5), 1, "再弱也还连着");
        assert_eq!(strength_percent(1.0, 0.0), 100);
    }

    /// 主题级网络（PRD-16-062）：节点只有主题页，边只连引用了同一篇来源的两页。
    #[test]
    fn topic_network_links_topics_through_shared_sources_only() {
        let dir = tempfile::tempdir().unwrap();
        let vault = luna_vault::Vault::open(dir.path()).unwrap();
        vault
            .write("Notes/周会.md", "# 周会\n\n原型已完成。\n")
            .unwrap();
        for path in ["Notes/项目甲.md", "Notes/项目乙.md"] {
            vault
                .write_topic(path, "# 题\n\n要点见 [周会](周会.md)。\n", Some("项目"))
                .unwrap();
        }
        vault
            .write_topic("Notes/无关.md", "# 无关\n\n不相干。\n", Some("杂项"))
            .unwrap();
        let network = topic_network_of(&vault).unwrap();
        assert_eq!(network.nodes.len(), 3, "周会是来源笔记，不是节点");
        assert!(network.nodes.iter().all(|node| node.kind == "topic"));
        assert_eq!(network.edges.len(), 1);
        assert_eq!(network.edges[0].kind, "source");
        assert_eq!(network.edges[0].strength, 100, "唯一一条即最强");
        let lonely = network
            .nodes
            .iter()
            .find(|node| node.id == "Notes/无关.md")
            .unwrap();
        assert_eq!(lonely.degree, 0);
    }

    /// BUSY 的三个消费面（自动保存日志 / 重扫连败 / 处理链降级）共用这一个谓词：
    /// 它答的是「结果一样会来，别当失败记账」。前缀匹配是因为错误码后面跟着中文说明。
    #[test]
    fn busy_code_is_not_a_failure() {
        assert!(is_busy_code(
            "GRAPH_TAGS_BUSY: 该笔记的关系标签正在生成中。"
        ));
        assert!(!is_busy_code("GRAPH_TAGS_JOB: QUOTA_EXHAUSTED"));
        assert!(!is_busy_code(
            "GRAPH_TAGS_EMPTY: 笔记正文为空，无从提炼标签。"
        ));
    }

    #[test]
    fn parse_tag_list_accepts_json_with_noise() {
        let raw = "Sure! Here are the tags:\n[\"会议纪要\", \"project/luna\", \"AI 语音\"]\nDone.";
        assert_eq!(
            parse_tag_list(raw),
            vec!["会议纪要", "project/luna", "AI-语音"]
        );
    }

    #[test]
    fn parse_tag_list_strips_think_blocks_and_fences() {
        let raw = "<think>maybe [1,2,3] hmm</think>```json\n[\"图谱\", \"#标签\"]\n```";
        assert_eq!(parse_tag_list(raw), vec!["图谱", "标签"]);
    }

    #[test]
    fn parse_tag_list_falls_back_to_lines_and_commas() {
        let raw = "- 产品评审\n- 路线图, 规划";
        assert_eq!(parse_tag_list(raw), vec!["产品评审", "路线图", "规划"]);
    }

    #[test]
    fn parse_tag_list_sanitizes_dedupes_and_caps() {
        let raw = r#"["Work", "work", "  ", "123", "a b  c", "x/y/", "很长的标签一二三四五六七八九十很长的标签一二三四五六七八九十很长的标签一二三四五六七八九十", "t1","t2","t3","t4","t5","t6","t7"]"#;
        let tags = parse_tag_list(raw);
        assert_eq!(tags.len(), GRAPH_TAGS_MAX, "上限截断：{tags:?}");
        assert_eq!(tags[0], "Work");
        assert!(!tags.contains(&"work".to_string()), "大小写不敏感去重");
        assert!(tags.contains(&"a-b-c".to_string()), "空白折叠为 '-'");
        assert!(tags.contains(&"x/y".to_string()), "边缘 '/' 修剪");
        assert!(!tags.iter().any(|t| t == "123"), "纯数字剔除");
    }

    #[test]
    fn parse_tag_list_unparsable_returns_empty() {
        assert!(parse_tag_list("I could not find any tags.").is_empty());
        assert!(parse_tag_list("<think>never closed [\"x\"]").is_empty());
    }

    #[test]
    fn instruction_embeds_vocabulary_and_contract() {
        let inst = extract_tags_instruction(&["工作".into(), "project/luna".into()], true);
        assert!(inst.contains("工作, project/luna"));
        assert!(inst.contains("JSON array"));
        // 空词表不带复用子句。
        assert!(!extract_tags_instruction(&[], true).contains("existing vault tags"));
        // 标签语言跟随**界面语言**而不是笔记内容：中英混排的库若各写各的语言，
        // 同一个概念会长出两个标签，而主题页建议的判据是「同标签 ≥3 篇」——簇永远凑不够
        // （真机第一轮：15 条标签无一跨篇重合）。
        assert!(extract_tags_instruction(&[], true).contains("Simplified Chinese"));
        assert!(extract_tags_instruction(&[], false).contains("in English"));
        assert!(
            !extract_tags_instruction(&[], true).contains("same language as the note"),
            "旧规则必须彻底消失，否则两条指令自相矛盾"
        );
    }

    /// 层级标签的**每一段**都要遵守语言规则（2026-08-30 真机第三轮）。
    ///
    /// 只说「所有标签用 X 语言」不够——模型据此产出过 `architecture/三层架构`、
    /// `data-integration/real-time-data`、`ops/planning`，一个标签内部就混着两种语言。
    /// 后果与库级分裂同源：父级 `architecture` 与 `架构` 各挂一半笔记。
    /// 注入词表必须按目标语言过滤（2026-08-30 真机第三轮）。
    ///
    /// 不过滤的代价是真机坐实过的：库当时中英分裂，提示词一边说「用中文」、一边把 `ops`、
    /// `cutover-migration` 作为「优先复用的既有标签」喂进去——**同一篇笔记的同一次生成里
    /// 同时吐出 `运维` 和 `ops`**（5 篇笔记里 `#运维` 挂 5 篇、`#ops` 挂 4 篇）。指令与
    /// 范例互相矛盾时模型跟范例走，而那正是我们自己给的。
    #[test]
    fn vocabulary_is_filtered_to_the_target_language() {
        let mixed = || {
            vec![
                "运维".to_string(),
                "ops".to_string(),
                "运维/计划".to_string(),
                "ops/计划".to_string(),
                "architecture/三层架构".to_string(),
                "datadog/metrics-monitoring".to_string(),
                "Datadog".to_string(),
                "I2X".to_string(),
                "MIO-One".to_string(),
            ]
        };

        let zh = filter_vocabulary(mixed(), true);
        assert_eq!(
            zh,
            vec![
                "运维".to_string(),
                "运维/计划".to_string(),
                // 专名放行（真机第四轮）：把复用锚滤掉的代价是标签总数 8 → 22。
                "Datadog".to_string(),
                "I2X".to_string(),
                "MIO-One".to_string(),
            ],
            "中文库拿到中文范例 + 语言中性的专名"
        );
        // 逐段判而不是整串判：`ops/计划` 含 CJK，但父级仍是英文——把它当范例喂回去
        // 等于继续教模型分裂。全小写的 `datadog/metrics-monitoring` 不享受专名豁免：
        // 专名形状认的是大写/数字这个**书写信号**，小写化之后它就是普通英文词组。
        assert!(!zh.contains(&"ops/计划".to_string()));
        assert!(!zh.contains(&"architecture/三层架构".to_string()));
        assert!(!zh.contains(&"datadog/metrics-monitoring".to_string()));

        let en = filter_vocabulary(mixed(), false);
        assert_eq!(
            en,
            vec![
                "ops".to_string(),
                "datadog/metrics-monitoring".to_string(),
                "Datadog".to_string(),
                "I2X".to_string(),
                "MIO-One".to_string(),
            ],
            "英文库拿全部非 CJK 的（专名在英文库本来就站得住）"
        );
    }

    #[test]
    fn instruction_requires_every_hierarchy_segment_in_one_language() {
        let zh = extract_tags_instruction(&[], true);
        assert!(zh.contains("EVERY segment"), "必须点名层级的每一段");
        assert!(
            zh.contains("architecture/三层架构"),
            "给反例比讲道理管用——这正是真机上产出过的那一个"
        );
        // 产品名仍按**词**豁免，不是按位置豁免：不能因为它在父级上就整段不翻。
        assert!(zh.contains("product names"));

        let en = extract_tags_instruction(&[], false);
        assert!(en.contains("EVERY segment"));
    }
}
