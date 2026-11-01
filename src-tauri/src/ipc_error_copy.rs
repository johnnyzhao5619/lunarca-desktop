//! IPC 错误说明的本地化门禁：**带稳定机器前缀的错误串，冒号后那半句不许出现 CJK**。
//!
//! # 为什么需要它（而不是靠纪律）
//!
//! `ipc/distill.rs` 的 `backend_text` 头注早就写死了这条纪律——「产物默认名、来源区标题、
//! 覆盖说明与全部错误说明都走既有 Fluent 资源……漏改的那一处会给英文界面的用户一段中文」。
//! 纪律在，门禁不在，于是 `DISTILL_QUOTA_SHORT` 就在那段头注的正下方被 `format!` 拼成中文
//! 上线，**全套门禁全绿**。
//!
//! 姊妹门禁 [`crate::i18n`] 的 `both_ftl_bundles_declare_exactly_the_same_message_ids` 守的是
//! 「两份 `.ftl` 的 key 集合相等」。它天然看不见本模块要抓的东西：**那些串压根没进过任何
//! bundle**。两条判据互补，缺一条就等于「凡是没打算翻译的，都不会被发现没翻译」。
//!
//! # 判据面（诚实边界，收窄了就在这里写清楚）
//!
//! 只认**字符串字面量本身以机器前缀开头**这一种形状：`"^[A-Z][A-Z0-9_]{2,}:"`。
//! 因此以下两类**抓不到**，是已知且刻意的边界：
//!
//! - 码与文案分开写：`format!("{}: {}", CODE, "中文")`——字面量不带前缀。
//! - 前缀在 `format!` 的参数里：`format!("{code}: {text}")`——本来就没有 CJK 字面量。
//!
//! 收窄的理由是误报的代价：拿「任何含中文的字面量」当判据，会把日志 target、单测夹具、
//! frontmatter 键名一并扫红，而**放宽过的门禁是最贵的一种——它还在跑，只是不再拦得住**。
//! 机器前缀是契约（调用方按它分支、日志按它聚类），以它为锚点既准又稳。
//!
//! # 扫源码必须剥注释
//!
//! 本文件的扫描器是一个真正的 Rust 词法游标，不是正则：注释里逐字写着被扫的字面量时，
//! 正则会把注释当代码扫红；而人一旦被注释扫红过，下一步就是去放宽规则。反过来更要命——
//! 若只用 `grep`，把违规串**搬进注释**也算「修好了」，守护形同虚设。故本模块把
//! 「注释里的中文不算数」做成了一条**反向自测**（见 `chinese_inside_comments_is_not_a_violation`）。
//!
//! # 守护自身不在扫描面内
//!
//! 本文件住在 `src/`（不是 `src/ipc/`），自测样本因此不可能扫到自己。这是结构保证而不是
//! 纪律——`ipc/mod.rs` 的 `source_without_tests` 头注记着同一类事故：三条扫源码的守护
//! 里有一条**一路全绿而什么都没验**，因为它找的东西就写在它自己那行字面量里。
//!
//! # 无豁免（欠账已清零，2026-08-31 批次 J′）
//!
//! 这道门禁曾带着一份欠账台账（`ipc_error_copy_debt.txt` + `MAX_DEBT`），**只减不增**：
//! 93 → 36（批次 J）→ 0（批次 J′）。归零的那一刻台账与常量**一并删除**，判据转为
//! 「一处都不许有」——而**不是**留一个空清单当过渡（ADR-046 不留兼容层）。留着它的代价
//! 很具体：一个恒空的豁免口是在邀请下一个人往里加第一行，而那一行不会有人反对，因为
//! 「反正机制本来就在」。
//!
//! 台账清零靠的是把「够不着文案取用来源」这件事根治掉（ADR-107：`AppState` 持
//! `Arc<BackendI18n>`），而不是靠纪律——最后那 36 条全部是同一个成因。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// 被扫描的源码面。
const IPC_DIR: &str = "src/ipc";

/// 冒号后那半句里出现任一即判红。
///
/// 范围比「基本区汉字」宽一档：`「」。、` 与全角 `！？（）` 同样是只有中文界面才该出现的
/// 字形，只锁汉字会让 `"CODE: 「」"` 这种**残留了中文排版的英文串**从判定面下溜走。
fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{3000}'..='\u{303F}'   // CJK 标点：。、「」《》
        | '\u{3400}'..='\u{4DBF}' // 汉字扩展 A
        | '\u{4E00}'..='\u{9FFF}' // 汉字基本区
        | '\u{F900}'..='\u{FAFF}' // 兼容汉字
        | '\u{FF00}'..='\u{FFEF}' // 全角形式：！？（）
    )
}

/// 机器前缀的长度（不含冒号）；不以前缀开头则 `None`。
///
/// 前缀是**契约**，本门禁只管冒号后那半句——不校验、更不改动前缀本身。
fn machine_prefix_len(text: &str) -> Option<usize> {
    let mut chars = text.chars();
    if !chars.next()?.is_ascii_uppercase() {
        return None;
    }
    let mut len = 1;
    for c in chars {
        if c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_' {
            len += 1;
        } else {
            return (len >= 3 && c == ':').then_some(len);
        }
    }
    None
}

/// `#[cfg(test)]` 之后正在跳过的东西。
enum Skip {
    /// 还没遇到 item 的 `{`（也可能是 `use x;` 这种没有块的 item）。
    Await,
    /// 已进入块，值是花括号深度。
    Block(usize),
}

/// 取出**生产代码**里的字符串字面量内容（已解转义，`#[cfg(test)]` 块内的不算）。
///
/// 一个字符游标同时管注释、字符串、原始字符串、字符字面量与生命周期——**必须是同一遍**：
/// `//` 在字符串里不是注释、`"` 在注释里不是字符串，分两遍做必然在其中一个方向上错。
///
/// **`#[cfg(test)]` 按花括号配对整块跳过，而不是 `split("\n#[cfg(test)]")`**：
/// `ipc/transcribe.rs` 在第一个 `#[cfg(test)] mod fetch_tests` **之后**还有 7 条
/// `#[tauri::command]`，按文本切会把这些生产代码一并切掉——扫描面凭空少一块，且不会有
/// 任何东西变红。
///
/// 写法上认死 `#[cfg(test)]` 这一种形态。若将来出现 `#[cfg(all(test, …))]`，本函数不跳过它，
/// 于是夹具串会被当生产代码扫红——**失败方向是对的**：多红一次有人来看，静默少扫没人知道。
fn production_string_literals(src: &str) -> Vec<String> {
    const CFG_TEST: &str = "#[cfg(test)]";
    let c: Vec<char> = src.chars().collect();
    let n = c.len();
    let mut out = Vec::new();
    let mut skip: Option<Skip> = None;
    let mut i = 0;

    while i < n {
        // 行注释（`//`、`///`、`//!` 同一形状）。
        if c[i] == '/' && c.get(i + 1) == Some(&'/') {
            while i < n && c[i] != '\n' {
                i += 1;
            }
            continue;
        }
        // 块注释；Rust 的块注释可嵌套。
        if c[i] == '/' && c.get(i + 1) == Some(&'*') {
            let mut depth = 1usize;
            i += 2;
            while i < n && depth > 0 {
                if c[i] == '/' && c.get(i + 1) == Some(&'*') {
                    depth += 1;
                    i += 2;
                } else if c[i] == '*' && c.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            continue;
        }
        // 原始字符串 `r"…"` / `r#"…"#` / `br#"…"#`：内部无转义，按 `#` 个数配对收尾。
        if let Some((start, hashes)) = raw_string_opening(&c, i) {
            let (text, next) = read_raw_string(&c, start, hashes);
            if skip.is_none() {
                out.push(text);
            }
            i = next;
            continue;
        }
        // 普通字符串（含 `b"…"`）。
        if c[i] == '"' {
            let (text, next) = read_string(&c, i + 1);
            if skip.is_none() {
                out.push(text);
            }
            i = next;
            continue;
        }
        // 字符字面量 vs 生命周期：`'a` 只是标注，吞掉它会把后面的代码错位成字符串内容。
        if c[i] == '\'' {
            i = skip_char_or_lifetime(&c, i);
            continue;
        }
        // `#[cfg(test)]`：进入跳过态。嵌套的内层属性不必再认，花括号自然配平。
        if skip.is_none() && c[i] == '#' && slice_starts_with(&c, i, CFG_TEST) {
            skip = Some(Skip::Await);
            i += CFG_TEST.chars().count();
            continue;
        }
        match (&skip, c[i]) {
            (Some(Skip::Await), '{') => skip = Some(Skip::Block(1)),
            // `#[cfg(test)] use super::*;` 这类无块 item，到分号就结束。
            (Some(Skip::Await), ';') => skip = None,
            (Some(Skip::Block(d)), '{') => skip = Some(Skip::Block(d + 1)),
            (Some(Skip::Block(d)), '}') => {
                skip = (*d > 1).then(|| Skip::Block(d - 1));
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// 认原始字符串的开头，返回（`"` 之后的下标，`#` 个数）。
fn raw_string_opening(c: &[char], i: usize) -> Option<(usize, usize)> {
    let mut j = i;
    if c.get(j) == Some(&'b') {
        j += 1;
    }
    if c.get(j) != Some(&'r') {
        return None;
    }
    // 前一个字符是标识符字符时，这个 `r` 只是名字的尾巴，不是原始串的开头。
    if i > 0 && (c[i - 1].is_alphanumeric() || c[i - 1] == '_') {
        return None;
    }
    j += 1;
    let hashes = c[j..].iter().take_while(|&&x| x == '#').count();
    (c.get(j + hashes) == Some(&'"')).then_some((j + hashes + 1, hashes))
}

fn read_raw_string(c: &[char], start: usize, hashes: usize) -> (String, usize) {
    let mut i = start;
    let mut text = String::new();
    while i < c.len() {
        if c[i] == '"' && c[i + 1..].iter().take_while(|&&x| x == '#').count() >= hashes {
            return (text, i + 1 + hashes);
        }
        text.push(c[i]);
        i += 1;
    }
    (text, i)
}

/// 读普通字符串，返回（解转义后的内容，收尾引号之后的下标）。
///
/// `\` + 换行是**续行**：Rust 会把换行与其后的前导空白一并吃掉，两行拼成一句。本仓
/// `ipc/providers.rs` 就有三处这么写的错误串——按行做的正则一律漏掉它们。
fn read_string(c: &[char], start: usize) -> (String, usize) {
    let mut i = start;
    let mut text = String::new();
    while i < c.len() {
        match c[i] {
            '"' => return (text, i + 1),
            '\\' => {
                i += 1;
                match c.get(i) {
                    Some('n') => text.push('\n'),
                    Some('t') => text.push('\t'),
                    Some('r') => text.push('\r'),
                    Some('0') => text.push('\0'),
                    Some('\n') => {
                        // 续行：吃掉换行与下一行的前导空白。
                        i += 1;
                        while c.get(i).is_some_and(|x| x.is_whitespace()) {
                            i += 1;
                        }
                        continue;
                    }
                    // `\x41` / `\u{4e00}`：本门禁只问「有没有中文」，逐字节还原没有意义，
                    // 但必须把它们**吃掉**，否则 `}` 会被误当作块结束。
                    Some('x') => i += 2,
                    Some('u') => {
                        while c.get(i).is_some_and(|&x| x != '}') {
                            i += 1;
                        }
                    }
                    Some(&other) => text.push(other),
                    None => break,
                }
                i += 1;
            }
            other => {
                text.push(other);
                i += 1;
            }
        }
    }
    (text, i)
}

/// `'` 开头的可能是字符字面量，也可能是生命周期；返回下一个待处理下标。
fn skip_char_or_lifetime(c: &[char], i: usize) -> usize {
    if c.get(i + 1) == Some(&'\\') {
        // `'\n'` / `'\u{1F600}'`：扫到收尾引号。
        let mut j = i + 2;
        while j < c.len() && c[j] != '\'' {
            j += 1;
        }
        return j + 1;
    }
    if c.get(i + 2) == Some(&'\'') {
        return i + 3; // `'x'`
    }
    i + 1 // `'a` 生命周期
}

fn slice_starts_with(c: &[char], i: usize, needle: &str) -> bool {
    needle
        .chars()
        .enumerate()
        .all(|(k, ch)| c.get(i + k) == Some(&ch))
}

/// 台账/报错里的单行表示：不能带真换行或制表符，否则一行一处的格式就散了。
fn escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\t', "\\t")
        .replace('\r', "\\r")
}

fn unescape(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// 一份源码里所有「前缀 + CJK 说明」的错误串，以及它一共出现过多少条带前缀的串
/// （后者只用来证明扫描器不是在空跑）。
fn violations(src: &str) -> (Vec<String>, usize) {
    let mut found = Vec::new();
    let mut prefixed = 0;
    for text in production_string_literals(src) {
        let Some(len) = machine_prefix_len(&text) else {
            continue;
        };
        prefixed += 1;
        if text.chars().skip(len + 1).any(is_cjk) {
            found.push(text);
        }
    }
    (found, prefixed)
}

struct Scan {
    /// `(文件名, 错误串) -> 出现次数`。
    tally: BTreeMap<(String, String), usize>,
    files: usize,
    prefixed: usize,
}

fn scan_ipc() -> Scan {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(IPC_DIR);
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("读得到 ipc 目录")
        .map(|e| e.expect("目录项可读").path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "rs"))
        .collect();
    paths.sort();

    let mut scan = Scan {
        tally: BTreeMap::new(),
        files: 0,
        prefixed: 0,
    };
    for path in paths {
        let src = std::fs::read_to_string(&path).expect("源码可读");
        let name = path
            .file_name()
            .expect("有文件名")
            .to_string_lossy()
            .into_owned();
        let (found, prefixed) = violations(&src);
        scan.files += 1;
        scan.prefixed += prefixed;
        for text in found {
            *scan.tally.entry((name.clone(), text)).or_insert(0) += 1;
        }
    }
    scan
}

fn ledger_lines(tally: &BTreeMap<(String, String), usize>) -> Vec<String> {
    let mut lines: Vec<String> = tally
        .iter()
        .flat_map(|((file, text), n)| std::iter::repeat_n(format!("{file}\t{}", escape(text)), *n))
        .collect();
    lines.sort();
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **IPC 错误说明不许把中文发给英文界面**（TDD-07 §2.1，`ipc/distill.rs` 的
    /// `backend_text` 头注）。
    ///
    /// **无豁免**：判据是「一处都不许有」。它曾带着一份只减不增的欠账台账，
    /// 于 2026-08-31（批次 J′）清零，台账与 `MAX_DEBT` **一并删除**——留一个恒空的
    /// 豁免口是在邀请下一个人往里加第一行（见模块头注）。
    ///
    /// 报错信息直接给出**文件 + 那一串**，照着改一处即可，不必回来读这段。
    #[test]
    fn ipc_error_copy_never_hands_chinese_to_an_english_ui() {
        let scan = scan_ipc();

        // 空跑防护：扫不到文件、或一条带前缀的串都认不出来时，下面的断言会恒真。
        assert!(scan.files > 1, "只扫到 {} 个文件——判据在空跑", scan.files);
        assert!(
            scan.prefixed > 50,
            "只认出 {} 条带机器前缀的错误串——词法游标坏了，判据在空跑",
            scan.prefixed
        );

        assert!(
            scan.tally.is_empty(),
            "中文错误说明进了 IPC 层——英文界面的用户会读到它。\n\
             改法：`coded_error(&app, \"<CODE>\", \"<fluent-key>\")`；拿不到 `AppHandle` 时用\
             `coded_error_with(state.copy(), …)`（ADR-107），文案两份 .ftl 各加一条。\n\
             **这道门禁没有豁免口**，欠账已清零。发现的：\n{}",
            ledger_lines(&scan.tally).join("\n")
        );
    }

    /// **凡是 `coded_error` / `coded_error_args` / `backend_text` 要的 key，两份 .ftl
    /// 里都必须真的有**——拼错一个，用户拿到的是 `CODE: `（说明是空的）或一段中文兜底串，
    /// 而**没有任何东西会因此变红**：`coded_error_args` 取不到消息时 `unwrap_or_default()`，
    /// `backend_text` 则静默回落到那个中文 `fallback` 参数。
    ///
    /// # 为什么必须是扫源码，而不是一张手写清单
    ///
    /// 这道判据此前只存在于 `ipc/distill.rs` 的一个**手抄 key 列表**里，而那个模块的头注
    /// 自己记着同一个教训的上一次发作：门禁曾写成 `filter(|id| id.starts_with("merge-"))`，
    /// 于是后加的 key 一个都不在判定面上，`DISTILL_QUOTA_SHORT` 被 `format!` 拼成中文
    /// 一路绿灯上线。key 从 3 个模块扩散到全 IPC 层之后，手抄清单必然漏——漏的方式还是
    /// 「新加的那一条没人想起回来加」。故判定面由源码给出，不由人给出。
    ///
    /// **只判 id 在不在**，不判能不能格式化：带参数的消息（`{ $min }`）不给参数取不到，
    /// 而参数是不是齐由调用点的类型与用例管。中英两份的 key 集合相等由
    /// [`crate::i18n`] 的 `both_ftl_bundles_declare_exactly_the_same_message_ids` 守，
    /// 故这里只对着 zh-CN 判一次即可——两道判据合起来才覆盖「两边都有」。
    #[test]
    fn every_backend_copy_key_used_in_ipc_exists_in_the_bundle() {
        let declared = bundle_message_ids(include_str!("../resources/i18n/zh-CN.ftl"));
        assert!(
            declared.len() > 20,
            "只解析出 {} 个消息 id——判据在空跑",
            declared.len()
        );

        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(IPC_DIR);
        let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
            .expect("读得到 ipc 目录")
            .map(|e| e.expect("目录项可读").path())
            .filter(|p| p.extension().is_some_and(|ext| ext == "rs"))
            .collect();
        paths.sort();

        let mut checked = 0usize;
        let mut missing: Vec<String> = Vec::new();
        for path in &paths {
            let src =
                crate::ipc::source_without_tests(&std::fs::read_to_string(path).expect("源码可读"));
            let name = path.file_name().expect("有文件名").to_string_lossy();
            for key in backend_copy_keys(&src) {
                checked += 1;
                if !declared.contains(&key) {
                    missing.push(format!("{name}: {key}"));
                }
            }
        }
        assert!(
            checked > 20,
            "只在 IPC 层认出 {checked} 处后端文案取用——提取器坏了，判据在空跑"
        );
        assert!(
            missing.is_empty(),
            "这些后端文案 key 在 resources/i18n/*.ftl 里不存在——用户会拿到一句空说明，\
             而不会有任何东西变红：\n{}",
            missing.join("\n")
        );
    }

    /// `.ftl` 里声明过的消息 id（口径与 `crate::i18n` 那道两份对齐的门禁逐字相同：
    /// 只认顶格的 `id = 值`，注释、续行与 `.attr` 都不是 id）。
    fn bundle_message_ids(src: &str) -> std::collections::BTreeSet<String> {
        src.lines()
            .map(str::trim_end)
            .filter(|line| !line.is_empty() && !line.starts_with(['#', ' ', '\t', '.']))
            .filter_map(|line| line.split_once(" ="))
            .map(|(id, _)| id.trim().to_owned())
            .collect()
    }

    /// 一份源码里所有**被取用**的后端文案 key。
    ///
    /// # 判据是形状，不是位置
    ///
    /// 位置取法（「key 是第 2 个字面量」）在**码本身不是字面量**的调用点上会取错——
    /// `coded_error_with(i18n, violation.code(), "embed-vector-contract")` 就是这一种。
    /// 故改为：在调用点之后的头几个字面量里，按形状挑出消息 id。
    ///
    /// 消息 id = 全小写 ASCII + 至少一个 `-` 或 `_`。这条形状同时排掉了同一个参数表里的
    /// 其它字面量：错误码是 SCREAMING、兜底文案带 CJK、`format!` 模板带花括号、
    /// Fluent 参数名（`need` / `min`）没有分隔符、给人读的句子带空格。而 `chat_busy`
    /// 这种**写错了分隔符**的 key 仍会被收进来并当场报缺——那正是要抓的。
    ///
    /// **`_with` 变体必须一并认**（[`crate::ipc::backend_copy::BackendCopy`]）：漏掉它
    /// 等于给「换个取用来源就绕过门禁」开一扇门，而分片摘要、向量归一化、派生缓存落盘
    /// 这几条链正是走它。
    ///
    /// 扫描面**按括号配平**框到这一次调用的参数表为止。固定窗口（「之后的头 N 个字面量」）
    /// 试过，当场把下一条语句里的 `topic_draft` / `default_embedding` 这类 snake_case 实参
    /// 收了进来并报成缺失 key——判定面必须是这次调用，不能是「附近」。
    fn backend_copy_keys(src: &str) -> Vec<String> {
        /// 一次调用的参数表里的字符串字面量（遇到与本次调用配平的 `)` 即止）。
        ///
        /// 入参是**紧跟在 `foo(` 之后**的那一段，故起始深度为 0；嵌套调用抬高深度，
        /// 其中的字面量照收——嵌套的那次调用若也是取用点，它自己那一轮会再收一遍，
        /// 重复对「key 在不在」这个判据无害。
        fn call_arguments(chunk: &str) -> Vec<String> {
            let mut out = Vec::new();
            let mut depth = 0i32;
            let mut buf: Option<String> = None;
            let mut chars = chunk.chars();
            while let Some(c) = chars.next() {
                if buf.is_some() {
                    match c {
                        // 转义序列整体跳过：`\"` 不是字符串结束。
                        '\\' => {
                            chars.next();
                        }
                        '"' => out.push(buf.take().expect("正在字符串里")),
                        _ => buf.as_mut().expect("正在字符串里").push(c),
                    }
                    continue;
                }
                match c {
                    '"' => buf = Some(String::new()),
                    '(' | '[' | '{' => depth += 1,
                    ')' | ']' | '}' => {
                        if depth == 0 {
                            break;
                        }
                        depth -= 1;
                    }
                    _ => {}
                }
            }
            out
        }
        fn looks_like_message_id(text: &str) -> bool {
            text.is_ascii()
                && text.contains(['-', '_'])
                && text
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        }
        const MARKERS: [&str; 6] = [
            "coded_error(",
            "coded_error_with(",
            "coded_error_args(",
            "coded_error_args_with(",
            "backend_text(",
            "backend_text_with(",
        ];
        let mut keys = Vec::new();
        for marker in MARKERS {
            for chunk in src.split(marker).skip(1) {
                for literal in call_arguments(chunk) {
                    if looks_like_message_id(&literal) {
                        keys.push(literal);
                    }
                }
            }
        }
        keys
    }

    /// **后端文案来源的装配点恰好一处**（ADR-107）。
    ///
    /// `AppState::backend_copy` 是 `OnceLock`：漏了那一次 `attach_backend_copy`，
    /// **什么都不会报错**——`BackendCopy` 是 `Option`，取不到就回落到「只发码」，于是
    /// 界面上突然一片 SCREAMING 码，而全套门禁照样全绿。多调一次同样是错的（第二次
    /// 一定来自别的对象，两处 `AppState` 视角会在切语言那一刻分叉）。
    ///
    /// 判据落在**源码**上而不是运行时：这件事只在 app setup 里发生一次，没有任何用例
    /// 会走到那条路径。
    #[test]
    fn the_backend_copy_is_attached_exactly_once() {
        let lib = include_str!("lib.rs");
        assert_eq!(
            lib.matches("attach_backend_copy(").count(),
            1,
            "`attach_backend_copy` 在 lib.rs 里的调用次数不是 1——漏了它，\
             所有只持 `&AppState` 的地方会静默退回「只发码」"
        );
        // 托管的必须是 `Arc<BackendI18n>`：`try_state::<T>` 是泛型，托管类型与取用类型
        // 不一致时**编译器一个字都不会说**，只是静默拿到 `None`。
        assert!(
            lib.contains("app.manage(std::sync::Arc::clone(&backend_copy));"),
            "托管的不再是 `Arc<BackendI18n>`——`backend_copy_arc` 会静默取不到"
        );
    }

    // ── 以下是门禁自己的自测：证明上面那条断言不是空跑 ──────────────────────────

    fn found(src: &str) -> Vec<String> {
        violations(src).0
    }

    #[test]
    fn a_chinese_coded_error_is_a_violation() {
        assert_eq!(
            found(r#"fn f() { Err("DISTILL_EMPTY: 模型未产出内容。".to_owned()) }"#),
            ["DISTILL_EMPTY: 模型未产出内容。"]
        );
    }

    #[test]
    fn english_and_unprefixed_strings_are_not_violations() {
        // 已迁移的形状：码是参数、文案来自 bundle。
        assert!(found(r#"format!("{code}: {text}")"#).is_empty());
        // 纯英文说明。
        assert!(found(r#"Err("DISTILL_EMPTY: model produced nothing")"#).is_empty());
        // 没有机器前缀的中文串不归本门禁管（日志 target、frontmatter 键名…）。
        assert!(found(r#"let s = "这是一句中文";"#).is_empty());
        // 前缀太短 / 不是全大写，都不是契约码。
        assert!(found(r#"let s = "AB: 中文"; let t = "Distill: 中文";"#).is_empty());
    }

    /// **反向判据：注释里的中文不算违规。**
    ///
    /// 这条是本门禁最容易被做假的地方——若扫描器不剥注释，把违规串**搬进注释**就能变绿，
    /// 而串还在源码里、且看起来像被修好了。三种注释形态各钉一次。
    #[test]
    fn chinese_inside_comments_is_not_a_violation() {
        assert!(found("// 例：\"DISTILL_EMPTY: 模型未产出内容。\"\n").is_empty());
        assert!(found("/// 例：\"DISTILL_EMPTY: 模型未产出内容。\"\n").is_empty());
        assert!(found("/* 例：\"DISTILL_EMPTY: 模型未产出内容。\" */").is_empty());
        // 嵌套块注释：Rust 允许，少数一层就会把后面的代码当注释吃掉。
        assert!(found("/* 外 /* 内 */ \"A_B_C: 中文\" */").is_empty());
    }

    /// 反过来也要钉住：**字符串里的 `//` 不是注释**，否则一句带 URL 的错误说明会静默逃逸。
    #[test]
    fn a_slash_slash_inside_a_string_does_not_start_a_comment() {
        assert_eq!(
            found(r#"Err("FETCH_BAD_URL: 打不开 https://例子.test 这个地址")"#),
            ["FETCH_BAD_URL: 打不开 https://例子.test 这个地址"]
        );
    }

    #[test]
    fn test_modules_are_skipped_but_production_code_after_them_is_not() {
        let src = "\
fn prod_a() { e(\"AAA_ONE: 甲\") }
#[cfg(test)]
mod fixtures {
    use super::*;
    fn f() { e(\"FIXTURE: 夹具不该被扫到\") }
}
fn prod_b() { e(\"BBB_TWO: 乙\") }
";
        assert_eq!(found(src), ["AAA_ONE: 甲", "BBB_TWO: 乙"]);
    }

    /// `#[cfg(test)] use …;` 没有块——跳到分号就得停，否则会把其后的整段生产代码吞掉。
    #[test]
    fn a_block_less_cfg_test_item_does_not_swallow_the_rest_of_the_file() {
        let src = "#[cfg(test)]\nuse std::fmt;\nfn prod() { e(\"CCC_THREE: 丙\") }\n";
        assert_eq!(found(src), ["CCC_THREE: 丙"]);
    }

    /// 续行串：`ipc/providers.rs` 真有三处这么写，按行做的正则一律漏掉。
    #[test]
    fn a_backslash_continued_string_is_still_one_message() {
        let src =
            "fn f() { e(\"REMOTE_ASR_DISABLED: 实时转写已指向服务端，\\\n    但隐私档是仅本地。\") }";
        assert_eq!(
            found(src),
            ["REMOTE_ASR_DISABLED: 实时转写已指向服务端，但隐私档是仅本地。"]
        );
    }

    #[test]
    fn raw_strings_and_lifetimes_do_not_derail_the_cursor() {
        // 生命周期里的 `'` 若被当成字符字面量，后面的字符串会整体错位。
        let src = "fn f<'a>(x: &'a str) { e(r#\"DDD_FOUR: 丁\"#); e(\"EEE_FIVE: 戊\") }";
        assert_eq!(found(src), ["DDD_FOUR: 丁", "EEE_FIVE: 戊"]);
    }

    /// 台账的往返：写进去的一行，读回来必须还是同一条串（含换行与反斜杠）。
    #[test]
    fn ledger_round_trips_messages_containing_newlines() {
        let text = "AAA_ONE: 第一行\n第二行\\结尾";
        assert_eq!(unescape(&escape(text)), text);
        assert!(!escape(text).contains('\n'));
    }
}
