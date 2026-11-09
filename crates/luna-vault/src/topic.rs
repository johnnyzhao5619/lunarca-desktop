//! 主题页与归入的纯逻辑（PRD-16 §3.4、
//! ADR-026 ①④）。
//!
//! 本模块是 ADR-026 ④ 那条裁决的技术承载：**模型只出片段与锚点，插入由这里确定性完成**。
//!
//! ## 为什么插入必须是纯函数
//!
//! 被否决的方案是「旧全文 + 新材料 → 新全文」。S17 spike 在本机模型上实测（见
//! S17 归入 prompt 保真度 spike，内部记录）：即便 prompt 里明写
//! 「保持原有内容一字不改」，8B 模型仍**整行删掉了一条风险项**、2B 删了两条还擅自加注解。
//! 那是一次「看起来成功了」的归入，没有任何报错——用户的内容被静默删除。
//!
//! 而在这里，既有正文**只被读取、从不被重新生成**：插入 = 在字符串的某个位置塞进一段新
//! 文本，其余字节逐字不动。于是「绝不覆盖用户资产」不再依赖模型的自律，而是结构上不可能。
//!
//! ## 所有权（ADR-026 ①）
//!
//! AI 维护页带 `luna.synth_body`（AI 上次写入时的正文指纹）：一致 = AI 所有（自动档可直接
//! 更新）；不一致 = **用户已接管**，此后一切更新恒走差异预览。判定复用 `distill_body` 的
//! 同一套指纹机制，不写第二份。

use crate::frontmatter::{body_of, read_luna, NoteStage, NoteType};
use luna_sync_core::hashing::ContentHash;

/// 一页 AI 维护笔记的所有权状态（ADR-026 ①）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TopicOwnership {
    /// 不是主题页。
    NotTopic,
    /// **AI 所有**：`synth_body` 与此刻正文一致，自动档可直接更新。
    AiOwned,
    /// **用户已接管**：正文被编辑过，此后一切更新恒走差异预览。
    ///
    /// 缺失 `synth_body` 也归此档（而非 `AiOwned`）：存量或外部工具建的页没有指纹，
    /// 无从证明「AI 写的还原封不动」，此时唯一安全的假设是它属于用户。
    UserOwned,
}

impl TopicOwnership {
    /// 自动路径可否直接写入（`false` = 必须走差异预览让用户过目）。
    #[must_use]
    pub fn allows_direct_write(self) -> bool {
        matches!(self, Self::AiOwned)
    }

    /// 是否是主题页（两种所有权都是）。
    #[must_use]
    pub fn is_topic(self) -> bool {
        !matches!(self, Self::NotTopic)
    }
}

/// 这一篇**该不该被我们替用户挑成候选**——合成主题页的源集、主题候选、「值得回看」
/// 都问它。
///
/// 它与新证据提取共用 [`crate::evidence_eligibility`]：待整理或空白笔记不算
/// （ADR-086 成熟度轴）。
///
/// # 为什么非要有这一格（真机 2026-08-31）
///
/// 这个判据此前在四处各写各的：归入候选的 SQL、`revisit_candidate` 的 SQL、
/// 「立即归入」检索路的三条 Rust `filter`，以及标签合成源集的 `Query`——而**最后那一份
/// 漏了产物轴**（只有 `type = note` 与成熟度，没有 `is_topic = 0`）。三个主题页在索引里
/// 恰好都是 `type = 'note', is_topic = 1`，于是它们全部通过，被当成源笔记喂进了合成：
/// `Knowledge/计划.md` 的「来源」区里逐字列着 `Datadog.md` 与 `I2X.md`。**AI 产物回流
/// 进了 AI 产物**，正是 ADR-096 只读产物层要防的。
/// 更糟的是当时已经有一句注释写着「这三条与共享标签那一路的 SQL 判据**必须同义**」——
/// 纪律在、机器判定不在，第四份副本就这么漏了一格。
///
/// SQLite 候选查询通过连接上的 `luna_ingest_candidate` 函数调用本判据；
/// `the_sql_and_the_rust_predicate_agree_on_every_shape` 再用真实文件语料锁住接线。
#[must_use]
pub fn is_ingest_candidate(
    is_markdown: bool,
    note_type: NoteType,
    stage: NoteStage,
    is_aggregate: bool,
    has_authoritative_text: bool,
) -> bool {
    crate::evidence_eligibility(
        is_markdown,
        note_type,
        stage,
        is_aggregate,
        has_authoritative_text,
        crate::SourcePolicy::FORMAL_ONLY,
        false,
    )
    .suggestible
}

/// 按 frontmatter 与此刻正文判定所有权。
#[must_use]
pub fn ownership_of(content: &str) -> TopicOwnership {
    let meta = read_luna(content);
    if !meta.topic && !meta.knowledge_map {
        return TopicOwnership::NotTopic;
    }
    let current = ContentHash::of(body_of(content).as_bytes()).to_string();
    match meta.synth_body {
        Some(recorded) if recorded == current => TopicOwnership::AiOwned,
        _ => TopicOwnership::UserOwned,
    }
}

/// 主题页正文里已引用的源笔记链接目标（用于 ADR-026 ②(a) 的「必须含源链接」校验）。
///
/// 判定复用 [`crate::parse::extract_links`]——与 links 表、反链、rename 重构同一口径，
/// 故「算不算引用了这一篇」在全仓只有一个答案。
#[must_use]
pub fn cites_source(body: &str, source_rel: &str) -> bool {
    cites_any_of(&cited_targets(body), source_rel)
}

/// 正文里全部链接目标的**归一化**形式（小写 + 百分号解码），供多次判定复用。
///
/// **一篇正文只解析一次**：[`cites_source`] 每调一次就重新 `extract_links` 整篇正文，
/// 「这一页还差哪几篇没归入」要对 C 条候选各问一次，于是同一篇正文被扫 C 遍。主题页多、
/// 正文长的库上，这是目录刷新的主要开销（目录还会随 `vault://changed` 每 400ms 重取一次）。
#[must_use]
pub fn cited_targets(body: &str) -> Vec<String> {
    crate::parse::extract_links(body)
        .iter()
        .map(|link| luna_sync_core::links::percent_decode(link).to_lowercase())
        .collect()
}

/// 归一化后的链接集合里，有没有一条指向 `source_rel`。
///
/// 判定规则与 [`cites_source`] 是同一份——两者都走这里，故「算不算引用了这一篇」在全仓
/// 仍然只有一个答案。
#[must_use]
pub fn cites_any_of(cited: &[String], source_rel: &str) -> bool {
    let stem = luna_sync_core::links::title_of(source_rel).to_lowercase();
    let target = source_rel.to_lowercase();
    cited.iter().any(|link| {
        // 链接在 `cited_targets` 里**已经百分号解码**：写入侧的 `markdown_link` 恒转义
        // （`洞见%202026…md`），而这里比的是未转义的路径与 stem——不解码，凡是文件名带空格
        // 的源笔记这道校验就**永远不成立**，于是 `materialize` 把每一项都当成「不含源链接」
        // 丢掉，归入在真实库上结构性失效（真机第七轮实测：三个目标全被静默丢弃、建议条与
        // 确认卡都空）。ADR-026 ②a 要挡的是「没有出处的声明」，不是「文件名有空格」。
        let link = link.as_str();
        link == target
            || link == stem
            || link.trim_end_matches(".md") == target.trim_end_matches(".md")
            || link
                .rsplit('/')
                .next()
                .is_some_and(|name| name.trim_end_matches(".md") == stem.trim_end_matches(".md"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cites_source_accepts_the_escaped_link_the_writer_actually_produces() {
        // 写入侧恒经 `markdown_link` 转义，校验侧却比未转义路径——真机上 Lunarca 自己的
        // 命名全都带空格，于是每一条归入都被判成「不含源链接」而静默丢弃。
        // 反向验证：去掉 `percent_decode` 这一条即红。
        let source = "Notes/洞见 2026-07-30 双均线.md";
        let link = crate::markdown_link("Notes/交易计划.md", source, "洞见");
        assert!(link.contains("%20"), "写入侧确实会转义：{link}");
        assert!(cites_source(&format!("- 一条要点。（{link}）"), source));
        // 未转义的写法（用户手写）照旧成立。
        assert!(cites_source(
            "- 一条要点。（[洞见](洞见 2026-07-30 双均线.md)）",
            source
        ));
        // 指向别处的链接仍然不算数。
        assert!(!cites_source(
            "- 一条要点。（[别的](Notes/%E5%88%AB%E7%9A%84.md)）",
            source
        ));
    }

    #[test]
    fn ownership_flips_when_the_user_edits() {
        let body = "# 主题\n\n## 概要\n\n内容。\n";
        let hash = ContentHash::of(body.as_bytes()).to_string();
        let ai_owned = format!(
            "---\nluna:\n  schema: 1\n  type: note\n  topic: true\n  synth_body: {hash}\n---\n{body}"
        );
        assert_eq!(ownership_of(&ai_owned), TopicOwnership::AiOwned);
        assert!(ownership_of(&ai_owned).allows_direct_write());

        // 用户改了一个字 → 指纹破 → 所有权翻转，自动路径一律止步。
        let edited = ai_owned.replace("内容。", "内容（我补充的）。");
        assert_eq!(ownership_of(&edited), TopicOwnership::UserOwned);
        assert!(!ownership_of(&edited).allows_direct_write());
    }

    #[test]
    fn missing_fingerprint_is_treated_as_user_owned() {
        // 存量页或外部工具建的页没有指纹——无从证明「AI 写的还原封不动」，
        // 唯一安全的假设是它属于用户。默认成 AiOwned 会让第一次自动归入就盲写用户的页。
        let no_fp = "---\nluna:\n  schema: 1\n  type: note\n  topic: true\n---\n# 主题\n\n## 概要\n\n内容。\n";
        assert_eq!(ownership_of(no_fp), TopicOwnership::UserOwned);
        // 非主题页则两种写入路径都不适用。
        let plain = "---\nluna:\n  schema: 1\n  type: note\n---\n正文\n";
        assert_eq!(ownership_of(plain), TopicOwnership::NotTopic);
        assert!(!ownership_of(plain).is_topic());
    }

    #[test]
    fn cites_source_accepts_both_link_syntaxes() {
        // 校验口径必须与 links 表同源：用户手写 wikilink 也算引用了这一篇。
        assert!(cites_source(
            "见 [纪要](../Notes/四月纪要.md)",
            "Notes/四月纪要.md"
        ));
        assert!(cites_source("见 [[四月纪要]]", "Notes/四月纪要.md"));
        assert!(!cites_source(
            "见 [别的](../Notes/五月纪要.md)",
            "Notes/四月纪要.md"
        ));
        assert!(!cites_source("没有任何链接", "Notes/四月纪要.md"));
    }
}
