//! 语义检索的**纯计算内核**（ADR-027 / TDD-06 §3.5）：
//! 暴力余弦、RRF 融合、近重复检测。零 IO、可无头单测。
//!
//! ## 为什么是暴力余弦，而不是向量库
//!
//! S16 实测：1 万分片 × 1024 维 f32 = **39 MB**，10 万片 = 391 MB。这个量级下**精确**暴力
//! 点积比 ANN 近似更快也更简单（一次查询就是几百万次乘加，纯 Rust 毫秒级）。
//! 故 **`sqlite-vec` 刻意不引**：它解决的是"内存装不下 / 必须近似"的问题，而 Lunarca 在
//! 这个规模上没有那个问题；引它要付一个 pre-v1 的 C 扩展依赖（ADR-005 License 面 + 构建面
//! 都是净负担），换来的是更慢和更近似。10 万片是这条裁决的重估点。
//!
//! 向量**出厂即 L2 归一化**（S16 实测 bge-m3 范数 1.000000），故余弦 = 点积，此处不做归一化。
//!
//! ## 为什么融合走 RRF，而不是加权分数
//!
//! 关键词路给的是 bm25（**越小越相关**、量纲随语料变），语义路给的是余弦（0–1）。
//! 两者的分数**尺度本就不可比**，任何 `α·bm25 + β·cosine` 都要先各自归一化，而归一化的
//! 基准（本轮的 min/max）会随查询漂移——同一篇笔记在两次查询里能拿到完全不同的融合分。
//!
//! RRF 只用**排名**：`score = Σ 1/(k + rank)`。它对两路的分数尺度免疫，是混合检索的行业
//! 默认解。S16 还给了一条更硬的依据：负对照（库里没有答案）的余弦 top1 = 0.415，而真实
//! 次位答案 0.538——**只差 0.12**，任何绝对阈值都会在某类查询上误杀。用排名就绕开了
//! "阈值定在哪"这个无解的问题。

use std::collections::HashMap;

/// RRF 的平滑常数。60 是原论文与主流实现（Elasticsearch / Weaviate）的默认值：
/// 它让第 1 名与第 2 名的差距（1/61 vs 1/62）远小于"两路都进前十"带来的加成，
/// 即**一致性胜过单路的极端自信**——这正是混合检索想要的行为。
pub const RRF_K: f64 = 60.0;

/// 一条语义命中（分片粒度）。
#[derive(Clone, Debug, PartialEq)]
pub struct SemanticHit {
    /// 笔记相对路径。
    pub path: String,
    /// 篇内分片序号。
    pub chunk_seq: u32,
    /// 该分片的起始行（1-based，相对整篇文件）——跳转直接用它。
    pub line_start: u32,
    /// 余弦相似度（向量已归一化 → 即点积）。
    pub score: f32,
}

/// 常驻内存的向量矩阵（AppState 懒加载持有；切库即整体丢弃重建，ADR-025）。
///
/// 扁平存 `f32` 而不是 `Vec<Vec<f32>>`：后者每片一次堆分配 + 一层间接寻址，在"一次查询扫
/// 全部分片"这个访问模式下是纯损失。一块连续内存让点积循环走在 cache line 上。
#[derive(Debug, Default)]
pub struct VectorMatrix {
    dim: usize,
    /// 逐分片的身份（与 `data` 的第 i 块一一对应）。
    rows: Vec<VectorRow>,
    /// 扁平向量数据（`rows.len() * dim` 个 f32）。
    data: Vec<f32>,
}

/// 矩阵里一行的身份。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VectorRow {
    /// 笔记相对路径。
    pub path: String,
    /// 篇内分片序号。
    pub chunk_seq: u32,
    /// 起始行号（1-based，相对整篇文件）。
    pub line_start: u32,
}

impl VectorMatrix {
    /// 从「身份 + 向量」序列构建。维度不一致的行**如实丢弃**（返回值只含被采纳的行）——
    /// 换过模型而索引里混着旧维度的向量时，硬塞进同一个矩阵会算出毫无意义的相似度。
    #[must_use]
    pub fn build(entries: Vec<(VectorRow, Vec<f32>)>) -> Self {
        let dim = entries.first().map_or(0, |(_, v)| v.len());
        let mut rows = Vec::with_capacity(entries.len());
        let mut data = Vec::with_capacity(entries.len() * dim);
        for (row, vector) in entries {
            if vector.len() != dim || dim == 0 {
                continue;
            }
            rows.push(row);
            data.extend_from_slice(&vector);
        }
        Self { dim, rows, data }
    }

    /// 向量维度（`0` = 空矩阵）。
    #[must_use]
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// 行数（= 已嵌入的分片数）。
    #[must_use]
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// 是否为空。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// 常驻内存字节数（设置页如实告知代价用；不含 `path` 字符串本身）。
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.data.len() * std::mem::size_of::<f32>()
    }

    /// 暴力余弦检索：返回前 `limit` 条（降序）。
    ///
    /// 查询向量维度不符即返回空——那说明查询用的模型与建库用的不是同一个，此时给出任何
    /// 结果都是编的（调用方据此走"重建索引"引导，而不是拿到一串随机相似度）。
    #[must_use]
    pub fn search(&self, query: &[f32], limit: usize) -> Vec<SemanticHit> {
        if self.dim == 0 || query.len() != self.dim || limit == 0 {
            return Vec::new();
        }
        let mut scored: Vec<SemanticHit> = self
            .rows
            .iter()
            .enumerate()
            .map(|(i, row)| {
                let start = i * self.dim;
                let score = dot(query, &self.data[start..start + self.dim]);
                SemanticHit {
                    path: row.path.clone(),
                    chunk_seq: row.chunk_seq,
                    line_start: row.line_start,
                    score,
                }
            })
            .collect();
        // 稳定次序：分数相同时按路径/序号定序，否则同一查询两次跑出不同顺序（不可测试）。
        scored.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.path.cmp(&b.path))
                .then_with(|| a.chunk_seq.cmp(&b.chunk_seq))
        });
        scored.truncate(limit);
        scored
    }

    /// 语义近邻：与 `path` 最像的**其它笔记**（PRD-16-029 「相关」页签的静默升级）。
    ///
    /// 拿这一篇自己的全部分片各查一次，按「最相似的那一对分片」给每篇邻居定分——与
    /// [`near_duplicates`] 同一口径，理由也同：一篇长笔记里只要有一段与另一篇很像，那就是
    /// 值得摆出来的相关，要求"整篇都像"会漏掉最常见的形态。
    ///
    /// `path` 不在矩阵里（那篇还没嵌入）即返回空——降级为标签共现，界面无变化。
    #[must_use]
    pub fn neighbours(&self, path: &str, limit: usize) -> Vec<(String, f32)> {
        if self.dim == 0 || limit == 0 {
            return Vec::new();
        }
        let mut best: HashMap<&str, f32> = HashMap::new();
        for (i, row) in self.rows.iter().enumerate() {
            if row.path != path {
                continue;
            }
            let vi = &self.data[i * self.dim..(i + 1) * self.dim];
            for (j, other) in self.rows.iter().enumerate() {
                if other.path == path {
                    continue;
                }
                let score = dot(vi, &self.data[j * self.dim..(j + 1) * self.dim]);
                let slot = best.entry(other.path.as_str()).or_insert(score);
                if score > *slot {
                    *slot = score;
                }
            }
        }
        let mut out: Vec<(String, f32)> =
            best.into_iter().map(|(p, s)| (p.to_string(), s)).collect();
        out.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        out.truncate(limit);
        out
    }
}

/// 点积（向量已 L2 归一化 → 即余弦）。
#[must_use]
pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// RRF 融合：把若干条**已按相关度降序排好**的路径序列融合成一个排序。
///
/// 入参是「每一路的路径序列」而非「路径 + 分数」——这正是 RRF 的要义：它只看排名。
/// 同一路里重复出现的路径只算它**最好的那一次**（一篇笔记有三个分片都命中，不该因此
/// 获得三倍加成——那会让长笔记系统性压过短笔记）。
#[must_use]
pub fn rrf_fuse(rankings: &[Vec<String>], limit: usize) -> Vec<String> {
    let mut scores: HashMap<&str, f64> = HashMap::new();
    let mut first_seen: HashMap<&str, usize> = HashMap::new();
    for ranking in rankings {
        let mut seen_in_this = HashMap::new();
        for (rank, path) in ranking.iter().enumerate() {
            if seen_in_this.insert(path.as_str(), ()).is_some() {
                continue; // 同一路内的重复只计最佳排名
            }
            *scores.entry(path.as_str()).or_insert(0.0) += 1.0 / (RRF_K + rank as f64 + 1.0);
            first_seen.entry(path.as_str()).or_insert(rank);
        }
    }
    let mut fused: Vec<(&str, f64)> = scores.into_iter().collect();
    fused.sort_by(|a, b| {
        b.1.total_cmp(&a.1)
            // 平分时按"在某一路里的最好排名"定序，再按路径——保证确定性。
            .then_with(|| first_seen[a.0].cmp(&first_seen[b.0]))
            .then_with(|| a.0.cmp(b.0))
    });
    fused
        .into_iter()
        .take(limit)
        .map(|(path, _)| path.to_string())
        .collect()
}

/// 近重复检测的相似度门槛。0.93 不是拍的：S16 实测**真实同主题但不同内容**的笔记对
/// 余弦落在 0.5–0.75，而 0.93 以上基本只有"同一份内容的两个版本"。定得再低就会把
/// 「项目 X 的四月纪要」与「项目 X 的五月纪要」判成重复——那是体检报告里最讨人厌的误报。
pub const NEAR_DUPLICATE_THRESHOLD: f32 = 0.93;

/// 一对近重复。
#[derive(Clone, Debug, PartialEq)]
pub struct NearDuplicate {
    /// 两篇笔记的相对路径（字典序，保证同一对只出现一次且顺序稳定）。
    pub left: String,
    pub right: String,
    /// 两篇之间最相似的一对分片的余弦。
    pub score: f32,
}

/// 小语料直接全对比的上限（分片数）。低于它时 LSH 的签名开销反而是纯损失，
/// 且全对比零漏报——预筛只该在它真能省下数量级工作量时介入。
const BRUTE_FORCE_CHUNK_LIMIT: usize = 512;

/// LSH 分带参数（服务 ADR-027 的适用包络：万片级；10 万片是整套裁决的重估点）。
///
/// 数学：余弦 ≥0.93 的一对向量，随机超平面两侧同号的概率 p = 1 − θ/π ≈ 0.88。
/// 一条带 = [`SIG_BAND_BITS`] 位全同才碰撞（p⁸ ≈ 0.36），[`SIG_BANDS`] 条带里至少
/// 一条碰撞的概率 = 1 − (1 − 0.36)¹⁶ ≈ **99.9%**——预筛的漏报率被压到千分之一以下，
/// 而无关向量对（中心化后近似均匀，p ≈ 0.5）的碰撞率只有 16/256 ≈ 6%。
const SIG_BANDS: usize = 16;
/// 每条带的位数。
const SIG_BAND_BITS: usize = 8;

/// 找出**跨笔记**的近重复（供批次 5 的知识体检消费；PRD-16-020 的防膨胀同一判据）。
///
/// 逐分片比对，取每对笔记的**最大**分片相似度：一篇长笔记里只要有一段与另一篇几乎
/// 一样，那就是值得报的重复；要求"整篇都像"会漏掉最常见的形态（复制粘贴了一节）。
/// 同一篇内部的分片不比（一篇笔记里两段相似是正常的行文）。
///
/// ## 规模化：分带签名预筛 + 精确复核（都在本函数内，调用方无须知道）
///
/// 朴素全对比是 O(片数² × 维度)——万片就是五百亿次乘加。故大语料走两段式：
/// 1. **种子化随机超平面签名**（128 位/片）按带分桶，只有同桶的分片对进入复核——
///    这是候选生成，**不是判定**；
/// 2. **候选一律精确余弦复核**：门槛判定永远发生在真实向量上，LSH 只可能漏
///    （概率 <0.1%，见 [`SIG_BANDS`]），**绝不可能多**——报出来的每一对都是真的。
///
/// **签名前先做语料中心化**（减全库均值）：同一个库的嵌入天然同域相关（两两余弦
/// 普遍 0.3–0.5），不中心化会让无关向量也频繁同号，预筛退化成"几乎全过"。中心化只
/// 影响候选生成，复核用的仍是原始向量。超平面由固定种子生成——同一份输入永远同一份输出。
#[must_use]
pub fn near_duplicates(matrix: &VectorMatrix, threshold: f32, limit: usize) -> Vec<NearDuplicate> {
    if matrix.dim == 0 || limit == 0 {
        return Vec::new();
    }
    let mut best: HashMap<(&str, &str), f32> = HashMap::new();
    if matrix.rows.len() <= BRUTE_FORCE_CHUNK_LIMIT {
        for i in 0..matrix.rows.len() {
            for j in (i + 1)..matrix.rows.len() {
                verify_pair(matrix, threshold, i, j, &mut best);
            }
        }
    } else {
        for (i, j) in candidate_pairs(matrix) {
            verify_pair(matrix, threshold, i, j, &mut best);
        }
    }

    let mut out: Vec<NearDuplicate> = best
        .into_iter()
        .map(|((left, right), score)| NearDuplicate {
            left: left.to_string(),
            right: right.to_string(),
            score,
        })
        .collect();
    out.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.left.cmp(&b.left))
            .then_with(|| a.right.cmp(&b.right))
    });
    out.truncate(limit);
    out
}

/// 精确复核一对分片：真实余弦过门槛才计入（每对笔记取最大分片相似度）。
fn verify_pair<'a>(
    matrix: &'a VectorMatrix,
    threshold: f32,
    i: usize,
    j: usize,
    best: &mut HashMap<(&'a str, &'a str), f32>,
) {
    let (pi, pj) = (matrix.rows[i].path.as_str(), matrix.rows[j].path.as_str());
    if pi == pj {
        return;
    }
    let vi = &matrix.data[i * matrix.dim..(i + 1) * matrix.dim];
    let vj = &matrix.data[j * matrix.dim..(j + 1) * matrix.dim];
    let score = dot(vi, vj);
    if score < threshold {
        return;
    }
    let key = if pi < pj { (pi, pj) } else { (pj, pi) };
    let slot = best.entry(key).or_insert(score);
    if score > *slot {
        *slot = score;
    }
}

/// 候选分片对（LSH 分带碰撞；去重、i < j）。
fn candidate_pairs(matrix: &VectorMatrix) -> Vec<(usize, usize)> {
    let dim = matrix.dim;
    let rows = matrix.rows.len();

    // ① 语料均值（中心化基准）。
    let mut mean = vec![0.0f32; dim];
    for i in 0..rows {
        for (m, v) in mean.iter_mut().zip(&matrix.data[i * dim..(i + 1) * dim]) {
            *m += v;
        }
    }
    for m in &mut mean {
        *m /= rows as f32;
    }

    // ② 固定种子的随机超平面（确定性：同一份输入永远同一份输出，测试可复现）。
    let bits = SIG_BANDS * SIG_BAND_BITS;
    let mut rng = SplitMix64(0x005E_ED10_24AD_E127_u64);
    let planes: Vec<f32> = (0..bits * dim).map(|_| rng.next_unit()).collect();

    // ③ 逐片签名（中心化后投影取符号），按带分桶。
    let mut buckets: HashMap<(usize, u8), Vec<usize>> = HashMap::new();
    let mut centered = vec![0.0f32; dim];
    for i in 0..rows {
        for ((c, v), m) in centered
            .iter_mut()
            .zip(&matrix.data[i * dim..(i + 1) * dim])
            .zip(&mean)
        {
            *c = v - m;
        }
        for band in 0..SIG_BANDS {
            let mut value = 0u8;
            for bit in 0..SIG_BAND_BITS {
                let plane = &planes[(band * SIG_BAND_BITS + bit) * dim..][..dim];
                if dot(&centered, plane) >= 0.0 {
                    value |= 1 << bit;
                }
            }
            buckets.entry((band, value)).or_default().push(i);
        }
    }

    // ④ 同桶成对（HashSet 去重；跨带重复碰撞只算一次）。
    let mut pairs: std::collections::HashSet<(usize, usize)> = std::collections::HashSet::new();
    for members in buckets.values() {
        for (a, i) in members.iter().enumerate() {
            for j in &members[a + 1..] {
                pairs.insert((*i, *j));
            }
        }
    }
    let mut out: Vec<(usize, usize)> = pairs.into_iter().collect();
    out.sort_unstable();
    out
}

/// SplitMix64（超平面生成专用）：够随机、零依赖、逐字确定。
struct SplitMix64(u64);

impl SplitMix64 {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// 均匀 [-1, 1)。超平面符号判定只看方向，分量分布均匀即可。
    fn next_unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 23) as f32 * 2.0 - 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(path: &str, seq: u32, line: u32) -> VectorRow {
        VectorRow {
            path: path.to_string(),
            chunk_seq: seq,
            line_start: line,
        }
    }

    /// 归一化向量的点积就是余弦；命中按分数降序且**次序确定**。
    #[test]
    fn search_ranks_by_cosine_with_deterministic_ties() {
        let matrix = VectorMatrix::build(vec![
            (row("a.md", 0, 3), vec![1.0, 0.0]),
            (row("b.md", 0, 5), vec![0.0, 1.0]),
            (row("c.md", 0, 7), vec![1.0, 0.0]),
        ]);
        let hits = matrix.search(&[1.0, 0.0], 3);
        assert_eq!(hits[0].path, "a.md");
        assert_eq!(hits[1].path, "c.md", "平分时按路径定序（次序必须可复现）");
        assert!((hits[0].score - 1.0).abs() < 1e-6);
        assert!(hits[2].score.abs() < 1e-6);
        // 行号随命中一起回传——跳转直接用它，不必回头再算。
        assert_eq!(hits[0].line_start, 3);
    }

    /// 查询向量维度不符 → 空结果，绝不编一串相似度。
    ///
    /// 这一幕的真实成因是"换了嵌入模型但没重建索引"。给出结果等于拿随机数当语义。
    #[test]
    fn dimension_mismatch_returns_nothing_instead_of_garbage() {
        let matrix = VectorMatrix::build(vec![(row("a.md", 0, 1), vec![1.0, 0.0])]);
        assert!(matrix.search(&[1.0, 0.0, 0.0], 5).is_empty());
        assert!(matrix.search(&[], 5).is_empty());
        assert!(matrix.search(&[1.0, 0.0], 0).is_empty());
        // 建库时混入的异维行也被丢掉（矩阵只留能算的那些）。
        let mixed = VectorMatrix::build(vec![
            (row("a.md", 0, 1), vec![1.0, 0.0]),
            (row("b.md", 0, 1), vec![1.0, 0.0, 0.0]),
        ]);
        assert_eq!(mixed.len(), 1);
        assert_eq!(mixed.dim(), 2);
    }

    /// RRF 只看排名：两路都进前列的胜过任一路的第一名。
    #[test]
    fn rrf_prefers_agreement_over_single_path_confidence() {
        let keyword = vec!["only-kw.md".into(), "both.md".into(), "x.md".into()];
        let semantic = vec!["only-sem.md".into(), "both.md".into(), "y.md".into()];
        let fused = rrf_fuse(&[keyword, semantic], 5);
        assert_eq!(
            fused[0], "both.md",
            "两路都排第 2 的应压过各自的第 1（1/62+1/62 > 1/61）：{fused:?}"
        );
        // 两个单路第一名之间按"最好排名 → 路径"定序，结果确定。
        assert_eq!(
            &fused[1..3],
            &["only-kw.md".to_string(), "only-sem.md".to_string()]
        );
    }

    /// 同一路里的重复只计最佳排名——否则长笔记会因分片多而系统性占优。
    #[test]
    fn rrf_counts_a_path_once_per_ranking() {
        let many_chunks = vec![
            "long.md".into(),
            "long.md".into(),
            "long.md".into(),
            "short.md".into(),
        ];
        let other = vec!["short.md".into()];
        let fused = rrf_fuse(&[many_chunks, other], 5);
        assert_eq!(
            fused[0], "short.md",
            "short 在两路各得一次（1/64+1/61）应胜过 long 的单路第一（1/61）：{fused:?}"
        );
    }

    /// 单路输入时 RRF 退化为原序（降级路径的行为必须与升级前完全一致）。
    #[test]
    fn rrf_with_one_ranking_preserves_original_order() {
        let only = vec!["a.md".to_string(), "b.md".to_string(), "c.md".to_string()];
        assert_eq!(rrf_fuse(std::slice::from_ref(&only), 10), only);
        assert!(rrf_fuse(&[], 10).is_empty());
        assert!(rrf_fuse(&[only], 0).is_empty());
    }

    /// 近重复：只报跨笔记的、取最相似的那一对分片。
    #[test]
    fn near_duplicates_are_cross_note_and_take_the_best_pair() {
        let same = vec![1.0, 0.0, 0.0];
        let almost = vec![0.99, 0.141, 0.0]; // 与 same 的余弦 ≈ 0.99
        let different = vec![0.0, 1.0, 0.0];
        let matrix = VectorMatrix::build(vec![
            (row("a.md", 0, 1), same.clone()),
            (row("a.md", 1, 9), same.clone()), // 同一篇内部不比
            (row("b.md", 0, 1), almost),
            (row("c.md", 0, 1), different),
        ]);
        let dups = near_duplicates(&matrix, NEAR_DUPLICATE_THRESHOLD, 10);
        assert_eq!(dups.len(), 1, "同篇内部不算重复，且不相似的不报：{dups:?}");
        assert_eq!(
            (dups[0].left.as_str(), dups[0].right.as_str()),
            ("a.md", "b.md")
        );
        assert!(dups[0].score > NEAR_DUPLICATE_THRESHOLD);
        // 门槛之上才报：把门槛抬到 1.0 之上即一条都不报。
        assert!(near_duplicates(&matrix, 1.01, 10).is_empty());
        assert!(near_duplicates(&VectorMatrix::default(), 0.5, 10).is_empty());
    }

    /// 门槛不能低到把「同主题不同内容」判成重复（S16 实测那类对子落在 0.5–0.75）。
    ///
    /// 编译期断言：调低门槛的人立刻撞墙，不必等跑测试（同 `chunk.rs` 的预算断言）。
    const _THRESHOLD_ABOVE_SAME_TOPIC: () = {
        assert!(
            NEAR_DUPLICATE_THRESHOLD > 0.8,
            "0.8 以下会把「四月纪要 vs 五月纪要」报成重复——体检里最讨人厌的误报"
        );
        assert!(NEAR_DUPLICATE_THRESHOLD < 1.0);
    };

    /// LSH 预筛路径与全对比**结果完全一致**（L1 遗留的机器防线）。
    ///
    /// 语料 600 片（> `BRUTE_FORCE_CHUNK_LIMIT`，强制走 LSH 路径），刻意模拟真实库的
    /// 三个难点：① 全库共享一个"域向量"分量（同域嵌入两两余弦普遍 0.3–0.5——不做
    /// 中心化时预筛在这种语料上退化）；② 近重复对**不是完全相同**（0.93+ 而非 1.0）；
    /// ③ 噪声向量确定性生成（固定种子），测试永远可复现。
    ///
    /// 断言"与 brute 参照**相等**"而不是"找到了已知的那几对"：预筛只可能漏不可能多
    /// （复核是精确的），相等断言同时锁住漏报（缺对）与实现漂移（分数不同）。
    #[test]
    fn lsh_candidate_path_matches_brute_force_exactly() {
        const DIM: usize = 32;
        let mut rng = SplitMix64(0x7E57_C0DE);
        let normalize = |v: &mut Vec<f32>| {
            let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
            v.iter_mut().for_each(|x| *x /= norm);
        };
        // 同域基底：每个向量都混入 60% 的公共分量（模拟同一个库的嵌入相关性）。
        let mut domain: Vec<f32> = (0..DIM).map(|_| rng.next_unit()).collect();
        normalize(&mut domain);
        let mut entries: Vec<(VectorRow, Vec<f32>)> = Vec::new();
        let make = |rng: &mut SplitMix64, base: Option<&[f32]>, jitter: f32| -> Vec<f32> {
            let mut v: Vec<f32> = match base {
                Some(b) => b.iter().map(|x| x + jitter * rng.next_unit()).collect(),
                None => (0..DIM)
                    .map(|i| 0.6 * domain[i] + 0.4 * rng.next_unit())
                    .collect(),
            };
            normalize(&mut v);
            v
        };
        // 580 片噪声（跨 60 篇笔记）。
        for i in 0..580 {
            let vector = make(&mut rng, None, 0.0);
            entries.push((row(&format!("n{}.md", i % 60), (i / 60) as u32, 1), vector));
        }
        // 10 组跨笔记近重复：副本 = 原向量 + 微扰（余弦 ≈ 0.97–0.999）。
        for g in 0..10 {
            let original = make(&mut rng, None, 0.0);
            let copy = make(&mut rng, Some(&original), 0.05);
            entries.push((row(&format!("dup-a{g}.md"), 0, 1), original));
            entries.push((row(&format!("dup-b{g}.md"), 0, 1), copy));
        }
        let matrix = VectorMatrix::build(entries);
        assert!(matrix.len() > BRUTE_FORCE_CHUNK_LIMIT, "必须走 LSH 路径");

        // 中心化的可证伪断言：候选对必须远小于全对（约 18 万对）。这份语料 60% 是
        // 公共域分量——不中心化时无关向量也频繁同号，候选会膨胀到 ~29%（5 万对上下），
        // 预筛名存实亡；中心化后应压在 ~6%（1.1 万对上下）。25,000 取两者中点，
        // 两边各留约 2 倍余量。
        let candidates = candidate_pairs(&matrix);
        assert!(
            candidates.len() < 25_000,
            "候选对 {} 超界——预筛退化（中心化失效的指纹）",
            candidates.len()
        );

        // brute 参照：在测试里独立写一遍全对比（不调被测函数的任何内部）。
        let mut reference: HashMap<(String, String), f32> = HashMap::new();
        for i in 0..matrix.len() {
            for j in (i + 1)..matrix.len() {
                let (pi, pj) = (&matrix.rows[i].path, &matrix.rows[j].path);
                if pi == pj {
                    continue;
                }
                let s = dot(
                    &matrix.data[i * DIM..(i + 1) * DIM],
                    &matrix.data[j * DIM..(j + 1) * DIM],
                );
                if s < NEAR_DUPLICATE_THRESHOLD {
                    continue;
                }
                let key = if pi < pj {
                    (pi.clone(), pj.clone())
                } else {
                    (pj.clone(), pi.clone())
                };
                let slot = reference.entry(key).or_insert(s);
                if s > *slot {
                    *slot = s;
                }
            }
        }
        assert!(
            reference.len() >= 8,
            "参照至少该有 8 组真重复（构造了 10 组，微扰可能压掉一两组）：{}",
            reference.len()
        );

        let found = near_duplicates(&matrix, NEAR_DUPLICATE_THRESHOLD, usize::MAX);
        let found_map: HashMap<(String, String), f32> = found
            .iter()
            .map(|d| ((d.left.clone(), d.right.clone()), d.score))
            .collect();
        assert_eq!(
            found_map, reference,
            "LSH 预筛 + 精确复核必须与全对比一字不差（预筛只可能漏不可能多，漏也在此现形）"
        );
        // 确定性：同一份输入第二次跑结果逐位相同（固定种子超平面）。
        assert_eq!(
            near_duplicates(&matrix, NEAR_DUPLICATE_THRESHOLD, usize::MAX),
            found
        );
    }

    /// 内存账与 S16 的估算对得上（这条断言把"不引向量库"的依据钉在代码上）。
    #[test]
    fn memory_accounting_matches_the_spike_estimate() {
        let dim = 1024;
        let matrix = VectorMatrix::build(
            (0..100)
                .map(|i| (row(&format!("n{i}.md"), 0, 1), vec![0.0; dim]))
                .collect(),
        );
        assert_eq!(matrix.bytes(), 100 * dim * 4);
        // 1 万片 × 1024 维 × f32 = 39.1 MB（S16 §4 ②）。
        assert_eq!(10_000 * dim * 4 / 1024 / 1024, 39);
    }
}
