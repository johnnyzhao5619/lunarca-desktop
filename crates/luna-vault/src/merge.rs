//! 「多条 → 一篇」的**源侧语义**：谁够格当源、喂给模型的是哪一段字、以及这一篇在
//! 生成期间有没有被动过（PRD-06-047 / TDD-07 §2.1）。
//!
//! **为什么这些判定住在 Vault 层而不是命令层**：它们全都要回答「这个相对路径在这个
//! 库里到底指向什么」——路径安全、正文提取口径、成熟度与聚合身份。命令层拿不到这些的
//! 权威答案，只能各自猜一遍；而三个入口（速记多选 / 标签汇总 / 主题页初稿）共用同一个
//! 汇总内核，猜法一旦不同，"同样一篇笔记喂进去的字不一样"这种问题不会有任何东西变红。
//!
//! 命令层负责的是**候选集政策**（哪些路径进来、失败是整体拒绝还是跳过），那才是三个
//! 入口真正不同的地方。

use crate::frontmatter::body_of;
use crate::frontmatter::{NoteStage, NoteType};
use crate::hashing::ContentHash;
use crate::parse::{clean_for_fts, title_of};

/// 「多条 → 一篇」被挡住的原因——`None` = 这一篇可以作为汇总的源。
///
/// 判据只有这一份：`DraftNoteDto.can_merge` / `NoteTreeDto.can_merge`（下发给界面决定给不
/// 给这一项）与 `merge_notes` 的落盘前重验问的是同一个问题——用户选好之后、点确认之前把
/// 其中一篇改掉是完全可能的，故资格是**对象资格**，不是一次性的通行证。
///
/// # 2026-09-02：候选集从「待整理内容」放开到「任何非聚合页的笔记」（PRD-06-047 修订）
///
/// 原政策只收记录态。真机上的诉求是"把这三次会议录音合成一篇总纪要"——而汇总内核**本来
/// 就按来源类型选输入提取口径**（[`merge_input`]：资料剥时间码、笔记原样保留），标签汇总
/// 那条入口也一直在收任意类型。于是"只收记录态"既不是内核的限制、也与另一个入口的口径
/// 不一致，只是一条挡在半路的候选政策。放开之后**唯一剩下的挡板是聚合页**：主题页与知识
/// 地图已经是聚合层，把它们当碎片再合一次是让它吞自己（同 PRD-16-037 的理由）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MergeBlock {
    /// 主题页 / 知识地图：它们**已经是聚合层**，把聚合页当碎片再合一次是让它吞自己。
    AggregatePage,
}

/// 这一篇能不能作为「多条 → 一篇」的源——**唯一判定点**（见 [`MergeBlock`]）。
///
/// 入参是**已判定的事实**：调用方各自用手边最便宜的来源取（命令层读 frontmatter，收件箱
/// 查询直接读派生列），但"什么算能汇总"只在这里回答一次。
///
/// **成熟度不再参与判定**（2026-09-02，见 [`MergeBlock`] 的修订说明），故它不再是入参
/// ——留一个不影响结果的参数，下一个人会以为它还管着什么。
#[must_use]
pub fn merge_block(is_aggregate_page: bool) -> Option<MergeBlock> {
    if is_aggregate_page {
        return Some(MergeBlock::AggregatePage);
    }
    None
}

/// 取一篇笔记时被挡住的原因（[`crate::Vault::merge_source`]）。
///
/// **逐条给出原因而不是一个笼统的 `None`**：调用方要据此决定"整体拒绝还是跳过"，
/// 而用户要据此知道下一步做什么——"这条不见了"与"这条是空的"的出路完全不同。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MergeSourceError {
    /// 越出 Vault 根：`..`、绝对路径，或经符号链接指到库外。
    ///
    /// 符号链接单列在这里而不是靠 `Vault::resolve` 的词法检查：后者只看路径**字面**
    /// 有没有 `..`，而库内的一条软链可以字面完全合法、读到的却是库外的文件。汇总会把
    /// 读到的正文原样喂给模型并写进产物，那就是一条把库外内容搬进库里的通路。
    PathEscape,
    /// 不是 `.md`：附件没有可汇总的正文。
    NotMarkdown,
    /// 读不到（不存在 / 已被移走 / IO 失败）。
    Unreadable,
    /// 正文为空——空内容进汇总只会稀释输入，且产物的来源区会指向一篇没有内容的笔记。
    Empty,
}

/// 一篇源笔记在**发起那一刻**的快照。
///
/// `fingerprint` 存在的理由是汇总要跨分钟级的模型等待：等待期间用户完全可能把其中一篇
/// 改了、移走、或者沉淀掉。拿等待前的正文生成、再按等待后的世界写盘，产出的就是一篇
/// 与来源对不上的笔记，而来源区的链接还言之凿凿。
#[derive(Clone, Debug)]
pub struct MergeSource {
    /// Vault 相对路径（归一后）。
    pub rel: String,
    /// **对象身份**（取自打开这一篇时的那个句柄，见 [`crate::ObjectId`]）。
    ///
    /// **去重按它、不按 [`MergeSource::rel`]**：`rel` 只经斜杠归一，库内一条软链别名、
    /// 以及大小写不敏感卷上的 `Note.md` / `note.md` 会被算成两条来源——条数与来源区同时
    /// 多出一份，而"至少选 2 条"这条规则就此形同虚设（勾同一篇两次即可绕过）。
    ///
    /// 反过来，**落盘前的复验不比它、只比 [`MergeSource::fingerprint`]**：身份回答的是
    /// "这两条是不是同一篇"（一瞬间的事实），而原子写会换掉 inode，任何一次保存都让身份
    /// 改变。两者管的跨度不同，理由写在 [`crate::Vault::merge_sources_unchanged`]。
    pub id: crate::ObjectId,
    /// 标题（= 文件名 stem）——逐条带进输入，"什么时候写的"是碎片的主要线索。
    pub title: String,
    /// 喂给模型的那一段字（已按来源类型选好提取口径，见 [`merge_input`]）。
    pub input: String,
    /// 发起那一刻的指纹：正文 + 成熟度 + 聚合身份。
    pub fingerprint: MergeFingerprint,
}

/// 源笔记的指纹：落盘前逐条重算并比对，任一项变了都视为"源已改变"。
///
/// **不只比正文**：一篇在等待期间被沉淀掉的草稿，正文可以一个字节没动（沉淀只去掉
/// frontmatter 里的一行），而它已经不该出现在这次汇总里了。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeFingerprint {
    body: String,
    stage: NoteStage,
    aggregate: bool,
}

impl MergeFingerprint {
    /// 成熟度（调用方据此重验自己的候选集资格）。
    #[must_use]
    pub fn stage(&self) -> NoteStage {
        self.stage
    }

    /// 它是不是主题页 / 知识地图。
    #[must_use]
    pub fn is_aggregate_page(&self) -> bool {
        self.aggregate
    }
}

/// 喂给模型的输入取哪一段字——**按来源类型**（PRD-06-044/051 与 PRD-06-054 的既有口径）。
///
/// - **资料**（录音 / 转写 / 对话）：`clean_for_fts`，剥掉时间码与脚手架行。把
///   `[00:01:23]` 逐行喂进去只是浪费上下文，并诱导模型把它抄进产出。
/// - **笔记 / 速记**：`body_of`，**原样保留用户写下的字**。它们本来就是散文，没有边车、
///   没有时间码；对它们跑 `clean_for_fts` 会把用户自己敲的时间码、角色标题（"张三：
///   …"）当成脚手架剥掉——那不是清洗，那是改用户的稿子。
#[must_use]
pub fn merge_input(note_type: NoteType, content: &str) -> String {
    if note_type.is_source_material() {
        clean_for_fts(content).trim().to_owned()
    } else {
        body_of(content).trim().to_owned()
    }
}

/// 按**对象身份**去重，保留第一次出现的位置。
///
/// **顺序即用户在列表里看到的顺序**（PRD-06-047）：勾选顺序是他理解这次汇总的线索，
/// 去重不该顺手把它打乱。同一篇被勾两次（列表刷新期间的重复提交、软链别名、大小写变体）
/// 只保留第一次那一条。
#[must_use]
pub fn dedup_by_object(sources: Vec<MergeSource>) -> Vec<MergeSource> {
    let mut seen: std::collections::HashSet<crate::ObjectId> = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(sources.len());
    for source in sources {
        if seen.insert(source.id.clone()) {
            out.push(source);
        }
    }
    out
}

pub(crate) fn source_from_doc(
    rel: &str,
    content: &str,
    meta: &crate::LunaMeta,
    id: crate::ObjectId,
) -> Option<MergeSource> {
    let input = merge_input(meta.note_type, content);
    if input.is_empty() {
        return None;
    }
    Some(MergeSource {
        rel: rel.to_owned(),
        id,
        title: title_of(rel).to_owned(),
        fingerprint: MergeFingerprint {
            // 正文原文（不是提取后的输入）：提取口径将来若变，指纹也不该因此把"没动过"
            // 判成"动过"。
            body: ContentHash::of(body_of(content).as_bytes()).to_string(),
            stage: meta.stage(),
            aggregate: crate::ownership_of(content).is_topic(),
        },
        input,
    })
}

#[cfg(test)]
mod merge_block_tests {
    use super::*;
    use crate::NoteType;

    /// **候选集只挡聚合页**（2026-09-02 修订，PRD-06-047）：资料、记录、定稿的知识
    /// 笔记都可以当碎片合成新的一篇——"把这三次会议录音合成一篇总纪要"是真机诉求，而
    /// 内核本来就按来源类型选输入口径（`merge_input`），标签汇总那条入口也一直收任意类型。
    #[test]
    fn only_an_aggregate_page_is_blocked_from_being_a_merge_source() {
        assert_eq!(merge_block(false), None);
        assert_eq!(
            merge_block(true),
            Some(MergeBlock::AggregatePage),
            "主题页已经是聚合层，再合一次是让它吞自己"
        );
    }

    /// 输入口径按来源类型分，这才是资料与笔记真正不同的地方（判定资格时它们一视同仁）。
    #[test]
    fn the_input_is_cleaned_for_source_material_and_kept_verbatim_for_notes() {
        let recording = "---\n---\n[00:01:23.400] 大家好\n";
        assert!(!merge_input(NoteType::Recording, recording).contains("00:01:23"));
        let note = "---\n---\n张三：我在 00:01:23 提过\n";
        assert!(merge_input(NoteType::Note, note).contains("00:01:23"));
    }
}

#[cfg(test)]
mod merge_source_tests {
    use crate::{MergeSourceError, Vault};

    fn open_vault() -> (tempfile::TempDir, Vault) {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        (dir, vault)
    }

    #[test]
    fn a_plain_note_keeps_the_words_the_user_actually_typed() {
        // 笔记 / 速记走 `body_of` 而**不是** `clean_for_fts`：后者是给资料剥时间码与
        // 脚手架用的，对着用户手写的稿子跑一遍，会把他自己敲的字当噪音删掉——那不是
        // 清洗，那是改稿。
        //
        // 两处样本**刻意挑成 `clean_for_fts` 真会动手的形状**：`[00:01:23]` 是它认得
        // 的时间轴标记，`## User` 是它整行丢弃的对话脚手架。拿 `[00:12]` 这种它本来就
        // 不认的写法做样本，这条守护在两种实现下都会通过——那是一条恒绿的假守护。
        let (_dir, v) = open_vault();
        let body = "[00:01:23] 这是我自己敲的时间码\n\n## User\n\n- 要点一\n";
        v.write(
            "速记.md",
            &format!("---\nluna:\n  type: quick-note\n---\n\n{body}"),
        )
        .unwrap();

        let src = v.merge_source("速记.md").unwrap();
        assert!(
            src.input.contains("[00:01:23]"),
            "用户写的时间码必须留着：{}",
            src.input
        );
        assert!(
            src.input.contains("## User"),
            "用户写的标题必须留着：{}",
            src.input
        );
        assert_eq!(src.title, "速记");
    }

    #[test]
    fn a_transcript_still_gets_its_timecodes_stripped() {
        // 资料侧的清洗口径不变（PRD-06-044/051）：把 `[00:01:23]` 逐行喂给模型只是浪费
        // 上下文，并诱导它抄进产出。
        let (_dir, v) = open_vault();
        v.write(
            "转写.md",
            "---\nluna:\n  type: transcript\n---\n\n[00:01:23] 会议开场\n",
        )
        .unwrap();

        let src = v.merge_source("转写.md").unwrap();
        assert!(!src.input.contains("[00:01:23]"), "{}", src.input);
        assert!(src.input.contains("会议开场"));
    }

    #[test]
    fn every_rejection_says_which_one_it_is() {
        let (_dir, v) = open_vault();
        v.write("空的.md", "---\nluna:\n  type: quick-note\n---\n\n   \n")
            .unwrap();
        std::fs::write(v.root().join("图片.png"), b"not markdown").unwrap();

        assert_eq!(
            v.merge_source("空的.md").unwrap_err(),
            MergeSourceError::Empty
        );
        assert_eq!(
            v.merge_source("图片.png").unwrap_err(),
            MergeSourceError::NotMarkdown
        );
        assert_eq!(
            v.merge_source("不存在.md").unwrap_err(),
            MergeSourceError::Unreadable
        );
        for escape in ["../外面.md", "/etc/passwd.md", "a/../../外面.md"] {
            assert_eq!(
                v.merge_source(escape).unwrap_err(),
                MergeSourceError::PathEscape,
                "{escape}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_pointing_out_of_the_vault_is_an_escape_not_a_source() {
        // 词法检查看不见它：`外链.md` 字面上完完全全是一条库内相对路径，读到的却是
        // 库外的文件。汇总会把读到的正文原样喂给模型并写进产物——那是一条把库外内容
        // 搬进库里的通路。
        let (_dir, v) = open_vault();
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("库外.md");
        std::fs::write(&secret, "---\nluna:\n  type: note\n---\n\n库外的内容\n").unwrap();
        std::os::unix::fs::symlink(&secret, v.root().join("外链.md")).unwrap();

        assert_eq!(
            v.merge_source("外链.md").unwrap_err(),
            MergeSourceError::PathEscape
        );
    }

    #[test]
    fn the_fingerprint_notices_a_settled_draft_even_when_the_body_is_byte_identical() {
        // 沉淀只去掉 frontmatter 里的一行——正文可以一个字节没动，而这一篇已经不该
        // 出现在这次汇总里了。只比正文的指纹会放它过去。
        let (_dir, v) = open_vault();
        v.write(
            "草稿.md",
            "---\nluna:\n  type: note\n  stage: draft\n---\n\n同一段正文\n",
        )
        .unwrap();
        let before = v.merge_source("草稿.md").unwrap();
        assert!(before.fingerprint.stage().is_draft());

        v.promote_to_knowledge("草稿.md").unwrap();
        let after = v.merge_source("草稿.md").unwrap();
        assert_eq!(before.input, after.input, "正文确实一个字节没动");
        assert_ne!(before.fingerprint, after.fingerprint, "指纹必须察觉");
        assert!(!after.fingerprint.stage().is_draft());
    }

    #[test]
    fn the_fingerprint_notices_an_edited_body() {
        let (_dir, v) = open_vault();
        v.write("草稿.md", "---\nluna:\n  type: quick-note\n---\n\n原文\n")
            .unwrap();
        let before = v.merge_source("草稿.md").unwrap();
        v.write("草稿.md", "---\nluna:\n  type: quick-note\n---\n\n改过了\n")
            .unwrap();
        assert_ne!(
            before.fingerprint,
            v.merge_source("草稿.md").unwrap().fingerprint
        );
    }

    /// **按对象去重、保留第一次选择的位置**（PRD-06-047）。
    ///
    /// 修复前是 `HashSet<String>` 装 `rel`，而 `rel` 只做过斜杠归一：库内一条软链别名、
    /// 大小写不敏感卷上的 `Note.md` / `note.md` 都会被算成两条来源，同时把条数与来源区
    /// 各多算一份——而"至少选 2 条"这条规则就此形同虚设（勾同一篇两次即可绕过）。
    #[test]
    fn dedup_collapses_aliases_and_keeps_the_order_the_user_saw() {
        let (_dir, v) = open_vault();
        for (name, body) in [("甲", "甲的正文"), ("乙", "乙的正文"), ("丙", "丙的正文")]
        {
            v.write(
                &format!("{name}.md"),
                &format!("---\nluna:\n  type: note\n---\n\n{body}\n"),
            )
            .unwrap();
        }
        let src = |rel: &str| v.merge_source(rel).unwrap();

        // ① 真实不同的对象：各算一条，顺序原样。
        let kept = crate::dedup_by_object(vec![src("丙.md"), src("甲.md"), src("乙.md")]);
        assert_eq!(
            kept.iter().map(|s| s.rel.as_str()).collect::<Vec<_>>(),
            ["丙.md", "甲.md", "乙.md"],
            "顺序即用户在列表里看到的顺序"
        );

        // ② 同一篇被勾两次：算一条，且留下的是**第一次**那个位置。
        let kept = crate::dedup_by_object(vec![src("丙.md"), src("甲.md"), src("丙.md")]);
        assert_eq!(
            kept.iter().map(|s| s.rel.as_str()).collect::<Vec<_>>(),
            ["丙.md", "甲.md"]
        );
    }

    /// 库内软链别名指向同一个对象 → 算一条（路径字符串完全不同，对象只有一个）。
    #[cfg(unix)]
    #[test]
    fn an_in_vault_alias_counts_once() {
        let (_dir, v) = open_vault();
        v.write("真身.md", "---\nluna:\n  type: note\n---\n\n正文\n")
            .unwrap();
        std::os::unix::fs::symlink(v.root().join("真身.md"), v.root().join("别名.md")).unwrap();
        let kept = crate::dedup_by_object(vec![
            v.merge_source("真身.md").unwrap(),
            v.merge_source("别名.md").unwrap(),
        ]);
        assert_eq!(kept.len(), 1, "同一个对象的两个名字被算成了两条来源");
        assert_eq!(kept[0].rel, "真身.md", "保留第一次选择的那一条");
    }

    /// 大小写变体在大小写不敏感卷上同样只算一条（敏感卷上第二条本就打不开）。
    #[test]
    fn a_case_variant_counts_once_where_the_filesystem_says_it_is_one_file() {
        let (_dir, v) = open_vault();
        v.write("Note.md", "---\nluna:\n  type: note\n---\n\n正文\n")
            .unwrap();
        let exact = v.merge_source("Note.md").unwrap();
        let Ok(variant) = v.merge_source("note.md") else {
            return; // 大小写敏感卷：没有第二条可选，本条无从谈起。
        };
        assert_eq!(crate::dedup_by_object(vec![exact, variant]).len(), 1);
    }

    #[test]
    fn a_topic_page_is_recognised_as_an_aggregate_in_the_fingerprint() {
        let (_dir, v) = open_vault();
        v.write(
            "主题页.md",
            "---\nluna:\n  type: note\n  topic: true\n  stage: draft\n---\n\n综述\n",
        )
        .unwrap();
        let src = v.merge_source("主题页.md").unwrap();
        assert!(src.fingerprint.is_aggregate_page());
        assert_eq!(
            crate::merge_block(src.fingerprint.is_aggregate_page()),
            Some(crate::MergeBlock::AggregatePage),
            "主题页即使被手工标成待整理也不能当碎片——它已经是聚合层"
        );
    }
}
