//! 笔记库规模基准（TDD-06 §4.1 增量扫描，笔记库对标轮 V1）。
//!
//! 合成一个 N 篇的库（固定种子，可复现），量五件事：首次接入、再次打开（= 每次启动的代价）、
//! 改名一篇被 500 篇引用的笔记、全文搜索 P50 / P95、开一次快速切换器（含拼音键现算）。结果打印一行 JSON，写进 TDD-06 §8。
//!
//! 用法：`cargo run --release -p luna-vault --example vault_bench -- <N> <工作目录>`

use luna_vault::search::SearchQuery;
use luna_vault::Vault;
use std::path::Path;
use std::time::Instant;

/// 线性同余伪随机：固定种子，结果逐次可复现，不为基准引入依赖。
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const WORDS: &[&str] = &[
    "会议", "项目", "排期", "预算", "客户", "需求", "评审", "上线", "复盘", "风险", "进度", "设计",
    "接口", "测试", "发布", "季度", "目标", "指标", "增长", "留存", "roadmap", "release", "sprint",
    "design", "review", "metric", "pipeline", "budget", "launch", "retro",
];

const HUB_REFERRERS: usize = 500;

fn note_path(index: usize) -> String {
    format!("区{:03}/笔记{index:06}.md", index / 100)
}

fn note_body(rng: &mut Lcg, index: usize, total: usize) -> String {
    let mut out = format!(
        "---\ntags: [{}]\n---\n# 笔记 {index}\n\n",
        WORDS[rng.below(WORDS.len())]
    );
    for line in 0..30 {
        let mut sentence = String::new();
        for _ in 0..12 {
            sentence.push_str(WORDS[rng.below(WORDS.len())]);
            sentence.push(' ');
        }
        if line % 10 == 0 {
            let target = rng.below(total);
            sentence.push_str(&format!("见 [[笔记{target:06}]] "));
        }
        out.push_str(sentence.trim_end());
        out.push('\n');
    }
    if index < HUB_REFERRERS {
        out.push_str("\n关联：[[枢纽]]\n");
    }
    out
}

fn generate(root: &Path, total: usize) -> std::io::Result<()> {
    let mut rng = Lcg(0x5EED_1234);
    std::fs::create_dir_all(root.join("hub"))?;
    std::fs::write(
        root.join("hub/枢纽.md"),
        "# 枢纽\n\n被很多笔记引用的一篇。\n",
    )?;
    for index in 0..total {
        let path = root.join(note_path(index));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, note_body(&mut rng, index, total))?;
    }
    Ok(())
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    let rank = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[rank.min(sorted.len() - 1)]
}

fn main() {
    let mut args = std::env::args().skip(1);
    let total: usize = args.next().and_then(|n| n.parse().ok()).unwrap_or(10_000);
    let workdir = args
        .next()
        .unwrap_or_else(|| "target/vault-bench".to_owned());
    let root = Path::new(&workdir).join(format!("vault-{total}"));
    let _ = std::fs::remove_dir_all(&root);

    let started = Instant::now();
    generate(&root, total).expect("生成合成库");
    let generate_ms = started.elapsed().as_millis();

    let started = Instant::now();
    let vault = Vault::open(&root).expect("打开库");
    let cold_count = vault.scan().expect("首次扫描");
    let cold_ms = started.elapsed().as_millis();
    drop(vault);

    let started = Instant::now();
    let vault = Vault::open(&root).expect("再次打开");
    let warm_count = vault.scan().expect("再次扫描");
    let reopen_ms = started.elapsed().as_millis();

    let started = Instant::now();
    let receipt = vault
        .move_with_report("hub/枢纽.md", "hub/枢纽-改名.md")
        .expect("改名枢纽");
    let rename_ms = started.elapsed().as_millis();

    // 快速切换器条目（含拼音键现算，PRD-07-040 ④）：开一次切换器的代价。
    let started = Instant::now();
    let switcher_count = vault.switcher_entries().expect("切换器条目").len();
    let switcher_ms = started.elapsed().as_millis();

    let mut rng = Lcg(0xBEEF);
    let mut latencies: Vec<f64> = (0..40)
        .map(|_| {
            let query = format!(
                "{} {}",
                WORDS[rng.below(WORDS.len())],
                WORDS[rng.below(WORDS.len())]
            );
            let parsed = SearchQuery::parse(&query, false);
            let started = Instant::now();
            let _ = vault.search_matches(&parsed).expect("搜索");
            started.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    latencies.sort_by(f64::total_cmp);

    println!(
        "{{\"notes\":{total},\"generate_ms\":{generate_ms},\"cold_scan_ms\":{cold_ms},\"cold_indexed\":{cold_count},\
         \"reopen_scan_ms\":{reopen_ms},\"reopen_indexed\":{warm_count},\"rename_hub_ms\":{rename_ms},\
         \"rename_notes\":{},\"rename_links\":{},\"search_p50_ms\":{:.1},\"search_p95_ms\":{:.1},\
         \"switcher_entries\":{switcher_count},\"switcher_ms\":{switcher_ms}}}",
        receipt.rewritten.len(),
        receipt.links,
        percentile(&latencies, 0.5),
        percentile(&latencies, 0.95),
    );
}
