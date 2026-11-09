//! 语义检索的分块器（ADR-027 / TDD-06 §3.5）。
//!
//! **纯函数**：一篇笔记的完整文件内容 → 若干带**行映射**的分片。可无头单测，与嵌入
//! Provider、SQLite、AppState 全部无关。
//!
//! ## 三条口径都是刻意的
//!
//! 1. **禁区与索引同源**：时间码标记与结构脚手架行（转写哨兵 / `## User` 之类的对话角色
//!    标题）经 [`crate::parse::clean_for_fts`] 剥除。复用那一个函数而不是再写一遍判定：
//!    ADR-024 定下的禁区在全仓只能有一个答案，否则「索引剥了、嵌入没剥」会让向量把
//!    `[00:12:34.567]` 这种噪声当成语义信号。
//! 2. **按标题与段落切，不按固定字符数硬切**：切在句子中间的分片，向量表示的是一句残句。
//!    故边界优先级 = 标题行 > 空行（段落）> 换行 > 硬切（只在单段超预算时兜底）。
//! 3. **每片带 `line_start`（1-based，相对整篇文件）**：命中一个分片之后要能跳回去，
//!    而那个行号必须是**原文的**行号——分片文本已经剥过时间码、丢过脚手架行，用分片
//!    内部的偏移去反推原文位置必然错位。故行号在切分时就记下来，不事后推算。
//!
//! ## 分片预算的来历（S16 实测，不是拍的）
//!
//! S16 实测时 bge-m3 的窗按 **2048 token 硬顶**计，而多样中文实测
//! **0.994 token/字符**——几乎 1:1。故：
//! - 正文预算 [`CHUNK_TARGET_CHARS`] = 1200 → 约 1190 token ≈ 窗的 58%，加上标题前缀仍有
//!   真实余量；
//! - **刻意不取 1500**：1500 字符中文实测 1491 token = 窗的 73%，加前缀就贴边了；
//! - 硬上界 [`CHUNK_HARD_MAX_CHARS`] = 1800 只在"一段话本身就超预算"时生效，仍远低于实测
//!   的 2015 字符崩溃线。
//!
//! 随包引擎（ADR-143）给嵌入的窗更大，预算仍沿用这组数：分块口径一变，既有索引与检索基准
//! 都要跟着重来，而更大的片未必更准（未验）。超窗的兜底是引擎**如实拒绝**
//! （`INPUT_TOO_LONG`）——宁可失败，不要一个只代表半片内容的向量。

use luna_sync_core::scaffold::{is_scaffold_line, timecode_spans};

/// 一个分片的目标正文字符数（S16 实测定值，见模块文档）。
pub const CHUNK_TARGET_CHARS: usize = 1_200;
/// 单个分片的硬上界（超出即在词界/字界硬切）。
pub const CHUNK_HARD_MAX_CHARS: usize = 1_800;
/// 短于此的分片丢弃。
///
/// **门槛刻意压得很低（8 字符）**：它要挡的只是「切分后剩下的一两个字」这类真噪声。
/// 定得高一点（比如 24）看着更干净，代价是**整篇短笔记直接搜不到**——而速记、
/// 「开会改到下周三」这类一句话笔记本来就短，让它们在语义检索里彻底消失是明显的退步。
/// 四个汉字（8 字符量级的中文）已经足够构成一个可检索的意思。
pub const CHUNK_MIN_CHARS: usize = 8;

/// 嵌入分块契约。它属于向量空间身份，不能由调用方临时拼常量。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EmbeddingChunkPolicy {
    pub id: &'static str,
    pub target_chars: usize,
    pub hard_chars: usize,
    pub min_chars: usize,
}

pub const BGE_M3_CHUNK_POLICY: EmbeddingChunkPolicy = EmbeddingChunkPolicy {
    id: "bge-m3-v1",
    target_chars: CHUNK_TARGET_CHARS,
    hard_chars: CHUNK_HARD_MAX_CHARS,
    min_chars: CHUNK_MIN_CHARS,
};

/// 一个可嵌入的分片。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    /// 篇内序号（0-based，稳定：同一份正文切出来的序号恒定，故可作主键的一半）。
    pub seq: u32,
    /// 起始行号（**1-based，相对整篇文件**，含 frontmatter；跳转直接用它）。
    pub line_start: u32,
    /// 供嵌入的文本（已剥时间码与脚手架行；不含标题前缀——前缀由调用方按需拼）。
    pub text: String,
}

/// 把一篇笔记切成分片。
///
/// `content` 是**此刻磁盘上的完整文件内容**（含 frontmatter）——行号据此计算，与
/// [`crate::retrieve::score_note`] 同一口径。
#[must_use]
pub fn chunk_note(content: &str, policy: EmbeddingChunkPolicy) -> Vec<Chunk> {
    assert!(policy.min_chars < policy.target_chars);
    assert!(policy.target_chars < policy.hard_chars);
    let body_start_line = frontmatter_line_offset(content);
    let body = crate::frontmatter::body_of(content);

    // 逐行清洗：保留「原文行号 → 清洗后文本」的对应，禁区行产出空文本但**占位**
    // （不占位就会让后面每一行的行号都错位）。
    let cleaned: Vec<(u32, String)> = body
        .split('\n')
        .enumerate()
        .map(|(i, line)| {
            let no = body_start_line + i as u32 + 1;
            (no, clean_line(line))
        })
        .collect();

    let mut acc = Accumulator::new(policy);
    for (no, text) in &cleaned {
        let trimmed = text.trim();
        // 标题行 = 硬边界：一个小节自成语义单元，把两个小节混进一片会让向量指向"两件事"。
        if is_heading(trimmed) && !acc.buf.trim().is_empty() {
            acc.flush();
        }
        if trimmed.is_empty() {
            // 段落边界：预算已过半即在此收口（宁可短一点，也不要跨段拼到超预算再硬切）。
            if acc.buf.chars().count() >= policy.target_chars / 2 {
                acc.flush();
            } else if !acc.buf.is_empty() {
                acc.buf.push('\n');
            }
            continue;
        }
        acc.push_line(trimmed, *no);
    }
    acc.flush();
    acc.chunks
}

/// 切分过程的累积状态（把 `flush`/硬切收在一处，避免闭包与 `chunks` 争借用）。
struct Accumulator {
    policy: EmbeddingChunkPolicy,
    chunks: Vec<Chunk>,
    buf: String,
    buf_line: Option<u32>,
    seq: u32,
}

impl Accumulator {
    fn new(policy: EmbeddingChunkPolicy) -> Self {
        Self {
            policy,
            chunks: Vec::new(),
            buf: String::new(),
            buf_line: None,
            seq: 0,
        }
    }

    /// 收口当前缓冲为一个分片（太短即丢弃）。
    fn flush(&mut self) {
        let text = self.buf.trim();
        if text.chars().count() >= self.policy.min_chars {
            if let Some(line) = self.buf_line {
                self.chunks.push(Chunk {
                    seq: self.seq,
                    line_start: line,
                    text: text.to_string(),
                });
                self.seq += 1;
            }
        }
        self.buf.clear();
        self.buf_line = None;
    }

    fn push_line(&mut self, trimmed: &str, no: u32) {
        if self.buf_line.is_none() {
            self.buf_line = Some(no);
        }
        if !self.buf.is_empty() {
            self.buf.push('\n');
        }
        self.buf.push_str(trimmed);

        // 单段就超硬上界：只有此时才硬切。按 `chars()` 切，故**绝不切在多字节字符中间**。
        while self.buf.chars().count() > self.policy.hard_chars {
            let head: String = self.buf.chars().take(self.policy.hard_chars).collect();
            let rest: String = self.buf.chars().skip(self.policy.hard_chars).collect();
            let line = self.buf_line.unwrap_or(no);
            self.chunks.push(Chunk {
                seq: self.seq,
                line_start: line,
                text: head,
            });
            self.seq += 1;
            self.buf = rest;
            // 硬切出来的后半段仍属**同一行**（那一行本身就超长），故行号不变。
            self.buf_line = Some(line);
        }
        if self.buf.chars().count() >= self.policy.target_chars {
            self.flush();
        }
    }
}

/// 一行的清洗：脚手架整行丢弃（返回空串占位），其余剥掉时间码标记。
///
/// 判定两处都委托 [`luna_sync_core::scaffold`]——与 `clean_for_fts` / `SearchableLine` 同一个答案。
fn clean_line(line: &str) -> String {
    if is_scaffold_line(line) {
        return String::new();
    }
    let spans = timecode_spans(line);
    if spans.is_empty() {
        return line.to_string();
    }
    let mut out = String::with_capacity(line.len());
    let mut cursor = 0usize;
    for (start, end) in spans {
        out.push_str(&line[cursor..start]);
        cursor = end;
    }
    out.push_str(&line[cursor..]);
    out.trim_start().to_string()
}

fn is_heading(trimmed: &str) -> bool {
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ')
}

/// frontmatter 占了多少行（分片行号要加上它——`body_of` 切掉的那一段仍在文件里）。
fn frontmatter_line_offset(content: &str) -> u32 {
    let body = crate::frontmatter::body_of(content);
    let prefix_len = content.len() - body.len();
    content[..prefix_len]
        .bytes()
        .filter(|b| *b == b'\n')
        .count() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunks(content: &str) -> Vec<Chunk> {
        chunk_note(content, BGE_M3_CHUNK_POLICY)
    }

    /// 行映射准确：分片的 `line_start` 必须指向**原文**那一行（含 frontmatter 偏移）。
    /// 错了就是"点了引用跳到别处"，而那是批次 1 明令禁止的静默失败。
    #[test]
    fn line_start_points_at_the_real_file_line() {
        let content = "---\nluna:\n  schema: 1\n  type: note\n---\n# 标题\n\n第一段正文。\n\n## 小节\n\n第二段正文。\n";
        let chunks = chunks(content);
        // 文件行：1 `---`、2-4 frontmatter、5 `---`、6 `# 标题`、7 空、8 第一段、9 空、
        // 10 `## 小节`、11 空、12 第二段。
        for chunk in &chunks {
            let line = content
                .lines()
                .nth(chunk.line_start as usize - 1)
                .expect("行号必须落在文件内");
            // 分片的第一行文本必须真的出现在它声称的那一行上。
            let first = chunk.text.lines().next().unwrap();
            assert!(
                line.contains(first.trim()),
                "分片 {} 声称起于第 {} 行（{line:?}），但它的首行是 {first:?}",
                chunk.seq,
                chunk.line_start
            );
        }
        assert!(!chunks.is_empty());
    }

    /// 标题是硬边界：两个小节不混进同一片（否则那片向量指向"两件事"）。
    #[test]
    fn headings_split_chunks() {
        let content = "# 页\n\n## 甲\n\n甲的内容足够长以越过最短分片门槛，这里再补一些字。\n\n## 乙\n\n乙的内容同样足够长以越过最短分片门槛，也补一些字。\n";
        let chunks = chunks(content);
        assert!(chunks.len() >= 2, "两个小节应至少切成两片：{chunks:?}");
        for chunk in &chunks {
            let heads = chunk.text.matches("## ").count();
            assert!(heads <= 1, "一片里出现了两个小节标题：{chunk:?}");
        }
    }

    /// 禁区与索引同源：时间码被剥、脚手架行整行丢弃。
    ///
    /// 这两类内容进了向量就是把噪声当语义信号（而 `clean_for_fts` 早已把它们剥出索引）。
    #[test]
    fn timecodes_and_scaffold_lines_never_reach_the_vector() {
        let content = "# 转写\n\n<!-- luna:transcript -->\n[00:12:34.567] 这是一句被时间码标记的转写内容，长度足够。\n[00:12:40.000] 第二句也足够长，用来撑过最短分片门槛。\n";
        let chunks = chunks(content);
        let all: String = chunks.iter().map(|c| c.text.as_str()).collect();
        assert!(!all.contains("00:12:34"), "时间码泄进了向量：{all}");
        assert!(
            !all.contains("luna:transcript"),
            "脚手架行泄进了向量：{all}"
        );
        assert!(
            all.contains("被时间码标记的转写内容"),
            "正文本身必须留下：{all}"
        );
    }

    /// 剥掉脚手架行之后，**后续行的行号不能错位**——逐行占位就是为了这个。
    ///
    /// 造一篇「小节标题 + 长段 → 分片边界 → 脚手架行 → 又一长段」的笔记：第二个分片的
    /// 行号必须跨过被丢弃的那一行如实落在它自己那一行上。若清洗时不占位，这里会偏 1 行，
    /// 而症状是"点引用跳到上一行"——批次 1 明令禁止的静默失败。
    #[test]
    fn dropping_scaffold_lines_does_not_shift_later_line_numbers() {
        let first = "甲段内容".repeat(200); // 越过目标预算，强制在此收口
        let second = "乙段内容需要足够长以自成一片。".repeat(20);
        let content = format!("# 对话\n\n{first}\n\n## User\n\n{second}\n");
        let chunks = chunks(&content);
        assert!(chunks.len() >= 2, "应至少切成两片：{}", chunks.len());
        let target = chunks
            .iter()
            .find(|c| c.text.starts_with("乙段内容"))
            .expect("第二段自成一片");
        assert_eq!(
            content.lines().nth(target.line_start as usize - 1),
            Some(second.as_str()),
            "角色标题被丢弃后，后一段的行号仍须指向它自己那一行"
        );
    }

    /// CJK 边界：硬切**绝不切在多字节字符中间**（切了就是乱码进向量）。
    #[test]
    fn hard_split_respects_char_boundaries_for_cjk() {
        let long = "中".repeat(CHUNK_HARD_MAX_CHARS * 2 + 37);
        let chunks = chunks(&format!("# 长\n\n{long}\n"));
        assert!(chunks.len() >= 2);
        for chunk in &chunks {
            // 能取到 chars 就说明没有半个字符（Rust 的 String 本身保证 UTF-8 合法，
            // 真正要断言的是每片都非空且不超硬上界）。
            assert!(
                chunk.text.chars().count() <= CHUNK_HARD_MAX_CHARS,
                "{chunk:?}"
            );
            assert!(chunk.text.chars().count() > 0);
        }
        // 拼回去应覆盖原文（硬切不丢字）。
        let joined: String = chunks.iter().map(|c| c.text.as_str()).collect::<String>();
        assert!(joined.contains(&"中".repeat(100)));
        assert_eq!(joined.matches('中').count(), long.chars().count());
    }

    /// 分片预算落在 S16 定的窗内（把 spike 的结论钉在代码上）。
    ///
    /// 写成 `const {}` 断言而不是运行期 `assert!`：这些全是编译期已知的常量关系，
    /// 做成**编译错误**比做成测试失败更早、更硬——改预算的人立刻撞墙，不必等跑测试。
    /// S16 实测：中文最坏 0.994 token/字符、窗 2048 硬顶、崩溃线 2015 字符。
    const _BUDGET_FITS_MEASURED_WINDOW: () = {
        assert!(CHUNK_TARGET_CHARS < CHUNK_HARD_MAX_CHARS);
        assert!(
            CHUNK_HARD_MAX_CHARS < 2015,
            "硬上界必须低于 S16 实测的崩溃线"
        );
        assert!(CHUNK_MIN_CHARS < CHUNK_TARGET_CHARS);
    };

    /// 太短的正文不产分片（一两个字的残片对检索只是噪声，还白占一次嵌入调用）。
    #[test]
    fn tiny_notes_produce_no_chunks() {
        assert!(chunks("---\nluna:\n  schema: 1\n---\n短\n").is_empty());
        assert!(chunks("").is_empty());
        assert!(chunks("# 标题\n").is_empty());
        // 但**一句话的短笔记必须产出分片**——否则速记在语义检索里整篇消失。
        let quick =
            chunks("---\nluna:\n  schema: 1\n  type: quick-note\n---\n开会改到下周三下午三点。\n");
        assert_eq!(quick.len(), 1, "一句话笔记也要能被语义检索到");
        assert_eq!(quick[0].text, "开会改到下周三下午三点。");
    }

    /// 序号稳定且连续（它是 `embeddings` 表主键的一半）。
    #[test]
    fn seq_is_dense_and_stable() {
        let content = format!("# 页\n\n{}\n", "内容一二三四五六七八九十。".repeat(400));
        let a = chunks(&content);
        let b = chunks(&content);
        assert_eq!(a, b, "同一份正文必须切出完全相同的分片");
        assert!(a.len() > 2);
        for (i, chunk) in a.iter().enumerate() {
            assert_eq!(chunk.seq, i as u32);
        }
    }
}
