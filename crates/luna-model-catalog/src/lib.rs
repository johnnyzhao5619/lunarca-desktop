//! `luna-model-catalog` — 本地模型目录的**唯一条目定义**与随包目录（ADR-145 ①②）。
//!
//! 客户端（模型管理、安装编排、随包档取回脚本）与服务端（发布链的草稿表单、冻结前校验）
//! 共用这一份类型，所以本 crate 是零 IO 的叶子（文件头解析只读调用方给的 `Read`）：只依赖 serde，不依赖任何 `luna-*`。
//!
//! - [`bundled`]：随包目录，`include_str!("../bundled.json")` 编入，恒在、离线可用。
//!   签名远程目录按条目 id 覆盖它（ADR-145 ①，消费者在应用装配层）。
//! - [`validate`]：条目的形状校验——客户端在测试里对随包目录跑，服务端在冻结发布前跑，
//!   同一套规则。
//! - [`gguf`]：GGUF 文件头解析与「文件头 vs 条目声明」核对（安装前与冻结前共用）。
//! - [`eval`]：生效闸的评测集与达标判定（ADR-145 ⑤ B1）。
//!
//! 字段只收**已经有消费者**的：新能力（GGUF 的上下文、思考标记、推荐档位……）随实现它的
//! 批次一起加，不预留空字段（ADR-046）。

#![forbid(unsafe_code)]

use std::collections::HashSet;
use std::sync::OnceLock;

pub mod eval;
pub mod gguf;

use serde::{Deserialize, Serialize};

/// 目录条目服务的任务。字符串形式就是模型管理 DTO 里的 `asset_kind`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Task {
    /// 语音转写（whisper）。
    Asr,
    /// 说话人细分（分割 / 声纹嵌入）。
    Diarize,
    /// 语音活动检测。
    Vad,
    /// 对话 / 翻译（随包 llama-server，ADR-143）。
    Llm,
    /// 文本嵌入（随包 llama-server）。
    Embed,
}

impl Task {
    /// 序列化形式（`"asr"` / `"diarize"` / `"vad"`）。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Asr => "asr",
            Self::Diarize => "diarize",
            Self::Vad => "vad",
            Self::Llm => "llm",
            Self::Embed => "embed",
        }
    }
}

/// 工件的一个取法。身份是 `sha256 + size_bytes`，来源可换（ADR-035 / ADR-144 ①）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Source {
    /// 直连地址（https）。
    Url {
        /// 地址。
        url: String,
    },
    /// Hugging Face：**钉到提交**（40 位十六进制），不跟 `main`（ADR-144 ①）。
    #[serde(rename = "huggingface")]
    HuggingFace {
        /// `owner/name`。
        repo: String,
        /// 提交号。
        revision: String,
        /// 仓库内文件路径。
        file: String,
    },
    /// ModelScope（中国大陆备选）：只有 `master` 分支、没有可钉的提交号，身份靠 sha256。
    #[serde(rename = "modelscope")]
    ModelScope {
        /// `owner/name`。
        repo: String,
        /// 仓库内文件路径。
        file: String,
    },
}

impl Source {
    /// 下载地址。
    #[must_use]
    pub fn url(&self) -> String {
        match self {
            Self::Url { url } => url.clone(),
            Self::HuggingFace {
                repo,
                revision,
                file,
            } => {
                format!("https://huggingface.co/{repo}/resolve/{revision}/{file}")
            }
            Self::ModelScope { repo, file } => {
                format!("https://www.modelscope.cn/models/{repo}/resolve/master/{file}")
            }
        }
    }
}

/// 用户选的首选下载来源（设置 `models.download_source`，ADR-144 ①）。
///
/// 只改**尝试顺序**，不删来源：首选的那一家失败照样回落到其余来源——中国大陆直连
/// Hugging Face 常超时，选 ModelScope 是为了少等那一轮，不是为了失去兜底。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SourcePreference {
    /// 按目录登记的顺序（Hugging Face 在前）。
    #[default]
    Auto,
    /// Hugging Face 优先。
    HuggingFace,
    /// ModelScope 优先。
    ModelScope,
}

impl SourcePreference {
    /// 设置值 → 偏好；认不出的值按 [`Self::Auto`]（注册表已校验取值，这里只兜底）。
    #[must_use]
    pub fn from_setting(value: &str) -> Self {
        match value {
            "huggingface" => Self::HuggingFace,
            "modelscope" => Self::ModelScope,
            _ => Self::Auto,
        }
    }

    fn prefers(self, source: &Source) -> bool {
        matches!(
            (self, source),
            (Self::HuggingFace, Source::HuggingFace { .. })
                | (Self::ModelScope, Source::ModelScope { .. })
        )
    }
}

/// 一个要下载的文件（权重、CoreML 编码器 zip……）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    /// 按顺序尝试的来源，至少一个。
    pub sources: Vec<Source>,
    /// 下载物 sha256（小写十六进制）。
    pub sha256: String,
    /// 下载物字节数。
    pub size_bytes: u64,
    /// 下载物是归档时要取出的成员路径（归档内）；缺省 = 下载物即目标文件。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_member: Option<String>,
}

impl Artifact {
    /// 按尝试顺序的全部下载地址。
    #[must_use]
    pub fn urls(&self) -> Vec<String> {
        self.sources.iter().map(Source::url).collect()
    }

    /// 按用户偏好排好的下载地址：首选那一家的来源提到前面，其余保持目录顺序。
    #[must_use]
    pub fn urls_preferring(&self, preference: SourcePreference) -> Vec<String> {
        let (first, rest): (Vec<&Source>, Vec<&Source>) = self
            .sources
            .iter()
            .partition(|source| preference.prefers(source));
        first.into_iter().chain(rest).map(Source::url).collect()
    }

    /// 首选地址（记入安装 manifest 的溯源）。[`validate`] 保证至少有一个来源。
    #[must_use]
    pub fn primary_url(&self) -> String {
        self.sources.first().map(Source::url).unwrap_or_default()
    }
}

/// 许可登记（ADR-005 / ADR-144 ④）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct License {
    /// 展示用许可名（SPDX 标识，如 `MIT`、`Apache-2.0`）。
    pub label: String,
    /// 原始模型（量化前）的仓库地址；我们分发的是别人量化过的派生物时必填。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    /// 量化者署名（「已修改：量化」声明的主体）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantized_by: Option<String>,
}

/// GGUF 条目（`llm` / `embed`）专属的运行参数。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Gguf {
    /// 需要的最低 llama.cpp 构建号（新架构只有新引擎认）。低于它的客户端看不到这条（ADR-145 ③）。
    pub engine_min_build: u32,
    /// 量化档（展示用，如 `Q8_0`）。
    pub quant: String,
    /// 我们愿意给它的最大上下文（token）；实际值由档位与内存预检在此之下定（ADR-143 ④）。
    pub max_context: u32,
    /// 运行角色。
    pub role: GgufRole,
    /// 专用翻译模型（如 Hy-MT2）：只声明「翻译」能力，不进对话 / 总结 / 润色的候选；
    /// 「翻译与实时档」的设备策略优先取它。只对对话角色有意义。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub translation_only: bool,
}

/// GGUF 模型在路由器里的角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum GgufRole {
    /// 对话 / 翻译。`thinking` = 模型默认开思考（Qwen3.5），运行时必须关（P0-1）。
    Chat {
        /// 默认会思考。
        thinking: bool,
    },
    /// 嵌入。`encoder` = 编码器结构（BERT 类，整段须进一个 ubatch）。
    Embed {
        /// 池化方式。
        pooling: Pooling,
        /// 编码器结构。
        encoder: bool,
    },
}

/// 嵌入池化方式（与 GGUF 元数据一致，仍显式登记）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pooling {
    /// `[CLS]`。
    Cls,
    /// 最后一个 token。
    Last,
    /// 均值。
    Mean,
}

/// 一条目录条目。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelEntry {
    /// 稳定 id：指「这一个模型」，换权重是同 id 新修订（ADR-145 ④）。
    pub id: String,
    /// 展示名。
    pub name: String,
    /// 任务。
    pub task: Task,
    /// 权重工件。
    pub weights: Artifact,
    /// 装机占用预估（归档解压后的体积等）。
    pub est_on_disk_bytes: u64,
    /// 许可。
    pub license: License,
    /// 权重落盘相对路径（相对模型根）；缺省 = whisper 惯例 `<id>/weights.bin`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight_rel: Option<String>,
    /// CoreML 编码器伴生工件（ADR-115），许可继承权重。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coreml_encoder: Option<Artifact>,
    /// GGUF 运行参数；`llm` / `embed` 条目必填，其余不得有。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gguf: Option<Gguf>,
    /// 随应用分发的条目：正常安装从资源物化，下载只用于恢复（PRD-14 §6），
    /// 不出现在可下载列表里。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bundled: bool,
    /// 能用这条的最低应用版本（`x.y.z`）；更旧的客户端看不到它（ADR-145 ③）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_client_version: Option<String>,
}

impl ModelEntry {
    /// 这个应用版本能不能用这条（`min_client_version` 缺省 = 都能）。
    #[must_use]
    pub fn supports_client(&self, client_version: &str) -> bool {
        self.min_client_version
            .as_deref()
            .is_none_or(|min| version_tuple(client_version) >= version_tuple(min))
    }
}

/// `x.y.z`（可带 `-pre` 后缀，忽略）→ 可比较的三元组；解析不了的段按 0。
fn version_tuple(version: &str) -> (u64, u64, u64) {
    let core = version.split(['-', '+']).next().unwrap_or("");
    let mut parts = core
        .split('.')
        .map(|p| p.trim().parse::<u64>().unwrap_or(0));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

/// 随包目录文件的形状。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogFile {
    /// 条目 schema 版本。破坏性变更开新版本区间，不在同一版本里塞兼容分支（ADR-046）。
    pub schema: u32,
    /// 条目。
    pub entries: Vec<ModelEntry>,
}

/// 当前条目 schema 版本。
pub const SCHEMA: u32 = 1;

const BUNDLED_JSON: &str = include_str!("../bundled.json");

/// 随包目录。解析失败或校验不过是构建缺陷（本 crate 的测试会先红），故 panic。
///
/// # Panics
/// 随包 JSON 不合法或未通过 [`validate`] 时。
#[must_use]
pub fn bundled() -> &'static [ModelEntry] {
    static ENTRIES: OnceLock<Vec<ModelEntry>> = OnceLock::new();
    ENTRIES.get_or_init(|| {
        let file: CatalogFile =
            serde_json::from_str(BUNDLED_JSON).expect("bundled.json is not a valid catalog");
        assert_eq!(file.schema, SCHEMA, "bundled.json schema mismatch");
        let problems = validate(&file.entries);
        assert!(
            problems.is_empty(),
            "bundled.json fails validation: {problems:?}"
        );
        file.entries
    })
}

/// 按 id 查随包目录。
#[must_use]
pub fn find(id: &str) -> Option<&'static ModelEntry> {
    bundled().iter().find(|entry| entry.id == id)
}

/// 形状校验，返回问题清单（空 = 通过）。客户端与服务端发布链同一套规则。
#[must_use]
pub fn validate(entries: &[ModelEntry]) -> Vec<String> {
    let mut problems = Vec::new();
    let mut ids = HashSet::new();
    for entry in entries {
        let id = &entry.id;
        if id.is_empty()
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            problems.push(format!("{id:?}: id must be non-empty [A-Za-z0-9._-]"));
        }
        if !ids.insert(id.as_str()) {
            problems.push(format!("{id}: duplicate id"));
        }
        if entry.name.trim().is_empty() {
            problems.push(format!("{id}: empty name"));
        }
        if entry.license.label.trim().is_empty() {
            problems.push(format!("{id}: empty license"));
        }
        if let Some(min) = &entry.min_client_version {
            let ok = min.split('.').count() == 3
                && min
                    .split('.')
                    .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
            if !ok {
                problems.push(format!("{id}: min_client_version must be x.y.z"));
            }
        }
        if let Some(rel) = &entry.weight_rel {
            if rel.is_empty()
                || rel.starts_with('/')
                || rel.split('/').any(|seg| seg == ".." || seg.is_empty())
            {
                problems.push(format!(
                    "{id}: weight_rel must be a relative path without '..'"
                ));
            }
        }
        check_artifact(id, "weights", &entry.weights, &mut problems);
        match (&entry.gguf, entry.task) {
            (Some(gguf), Task::Llm | Task::Embed) => {
                let role_ok = matches!(
                    (gguf.role, entry.task),
                    (GgufRole::Chat { .. }, Task::Llm) | (GgufRole::Embed { .. }, Task::Embed)
                );
                if !role_ok {
                    problems.push(format!("{id}: gguf role does not match task"));
                }
                if gguf.translation_only && !matches!(gguf.role, GgufRole::Chat { .. }) {
                    problems.push(format!(
                        "{id}: translation_only applies to chat models only"
                    ));
                }
                if gguf.max_context < 512
                    || gguf.engine_min_build == 0
                    || gguf.quant.trim().is_empty()
                {
                    problems.push(format!(
                        "{id}: gguf needs max_context >= 512, engine_min_build and quant"
                    ));
                }
                if entry.license.quantized_by.is_none() || entry.license.upstream.is_none() {
                    problems.push(format!(
                        "{id}: quantized weights must credit upstream and quantizer"
                    ));
                }
            }
            (None, Task::Llm | Task::Embed) => {
                problems.push(format!("{id}: llm / embed entries need gguf"))
            }
            (Some(_), _) => problems.push(format!("{id}: gguf only applies to llm / embed")),
            (None, _) => {}
        }
        if let Some(encoder) = &entry.coreml_encoder {
            check_artifact(id, "coreml_encoder", encoder, &mut problems);
            if entry.task != Task::Asr {
                problems.push(format!("{id}: coreml_encoder only applies to asr"));
            }
        }
    }
    problems
}

fn check_artifact(id: &str, which: &str, artifact: &Artifact, problems: &mut Vec<String>) {
    if artifact.sources.is_empty() {
        problems.push(format!("{id}.{which}: no sources"));
    }
    for source in &artifact.sources {
        match source {
            Source::Url { url } => {
                if !url.starts_with("https://") {
                    problems.push(format!("{id}.{which}: source must be https"));
                }
            }
            Source::HuggingFace {
                repo,
                revision,
                file,
            } => {
                if !is_repo(repo) || !is_repo_path(file) {
                    problems.push(format!("{id}.{which}: bad huggingface repo / file"));
                }
                if revision.len() != 40
                    || !revision.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
                {
                    problems.push(format!(
                        "{id}.{which}: huggingface revision must be a 40-hex commit, not a branch"
                    ));
                }
            }
            Source::ModelScope { repo, file } => {
                if !is_repo(repo) || !is_repo_path(file) {
                    problems.push(format!("{id}.{which}: bad modelscope repo / file"));
                }
            }
        }
    }
    if artifact.sha256.len() != 64
        || !artifact
            .sha256
            .chars()
            .all(|c| matches!(c, '0'..='9' | 'a'..='f'))
    {
        problems.push(format!("{id}.{which}: sha256 must be 64 lowercase hex"));
    }
    if artifact.size_bytes == 0 {
        problems.push(format!("{id}.{which}: size_bytes must be > 0"));
    }
    if let Some(member) = &artifact.archive_member {
        if member.is_empty() || member.starts_with('/') || member.split('/').any(|seg| seg == "..")
        {
            problems.push(format!(
                "{id}.{which}: archive_member must be a relative path without '..'"
            ));
        }
    }
}

/// 逐条解析远程目录（ADR-145 ③）：解析不了或校验不过的**单条**跳过并计数，不让整份作废——
/// 将来服务端加了本版不认识的字段或任务类型，旧客户端只是看不到那几条，其余照常生效。
/// 不是兼容分支：新形状对旧客户端就是「不存在」。
#[must_use]
pub fn parse_lenient(values: Vec<serde_json::Value>) -> (Vec<ModelEntry>, usize) {
    let mut accepted: Vec<ModelEntry> = Vec::new();
    let mut skipped = 0;
    for value in values {
        match serde_json::from_value::<ModelEntry>(value) {
            Ok(entry)
                if validate(std::slice::from_ref(&entry)).is_empty()
                    && !accepted.iter().any(|e| e.id == entry.id) =>
            {
                accepted.push(entry);
            }
            _ => skipped += 1,
        }
    }
    (accepted, skipped)
}

/// 进程级的已验签远程目录（与提示词目录装进 `PromptRegistry` 同一形态）。
pub mod remote {
    use super::ModelEntry;
    use std::sync::RwLock;

    /// 当前生效的远程目录。
    #[derive(Debug, Clone, Copy)]
    pub struct Remote {
        /// 目录版本。
        pub version: u64,
        /// 条目（已逐条解析、校验）。
        pub entries: &'static [ModelEntry],
        /// 逐条解析时跳过的条数（本版不认识或不合法）。
        pub skipped: usize,
    }

    static CURRENT: RwLock<Option<Remote>> = RwLock::new(None);

    /// 装上一份已验签目录。条目被泄漏成 `'static`：调用方拿到的条目引用要活过任意一次
    /// 后续刷新（模型管理里到处是 `&'static ModelEntry`），而刷新最多每几小时一次、
    /// 一份几十 KB——用常驻换掉一整套生命周期改造，划算。
    pub fn install(version: u64, entries: Vec<ModelEntry>, skipped: usize) {
        let entries: &'static [ModelEntry] = Box::leak(entries.into_boxed_slice());
        if let Ok(mut current) = CURRENT.write() {
            *current = Some(Remote {
                version,
                entries,
                skipped,
            });
        }
    }

    /// 回到无远程覆盖（换服务端且新桶没有缓存、或缓存失效）。
    pub fn clear() {
        if let Ok(mut current) = CURRENT.write() {
            *current = None;
        }
    }

    /// 当前生效的远程目录。
    #[must_use]
    pub fn current() -> Option<Remote> {
        CURRENT.read().ok().and_then(|current| *current)
    }
}

/// 可发现的条目（可下载列表的来源）：远程目录生效时**以它为准**——不在其中的条目撤出发现
/// （ADR-145 ⑤：删 = 下一版不再包含）；随包档（`bundled`）恒取随包那份。没有远程时用随包目录。
#[must_use]
pub fn discoverable() -> Vec<&'static ModelEntry> {
    match remote::current() {
        Some(remote) => bundled()
            .iter()
            .filter(|e| e.bundled)
            .chain(remote.entries.iter().filter(|e| !e.bundled))
            .collect(),
        None => bundled().iter().collect(),
    }
}

/// 按 id 查条目：远程优先、随包兜底——撤出发现的条目对**已装**的模型照常可查（PRD-13-035：
/// 已装的不删、已分配的不改写）。
#[must_use]
pub fn lookup(id: &str) -> Option<&'static ModelEntry> {
    remote::current()
        .and_then(|remote| remote.entries.iter().find(|e| e.id == id && !e.bundled))
        .or_else(|| find(id))
}

/// `owner/name`，两段都是 `[A-Za-z0-9._-]`。
fn is_repo(repo: &str) -> bool {
    let mut parts = repo.split('/');
    let ok = |p: Option<&str>| {
        p.is_some_and(|p| {
            !p.is_empty()
                && p != ".."
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        })
    };
    ok(parts.next()) && ok(parts.next()) && parts.next().is_none()
}

/// 仓库内相对路径：不空、不以 `/` 起、不含 `..`、不带查询串。
fn is_repo_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains(['?', '#'])
        && path.split('/').all(|seg| !seg.is_empty() && seg != "..")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_catalog_parses_and_validates() {
        assert!(!bundled().is_empty());
        assert_eq!(
            bundled().iter().filter(|e| e.bundled).count(),
            1,
            "exactly one bundled entry (the shipped ASR)"
        );
    }

    #[test]
    fn bundled_json_round_trips_byte_for_byte_semantics() {
        let file: CatalogFile = serde_json::from_str(BUNDLED_JSON).unwrap();
        let again: CatalogFile =
            serde_json::from_str(&serde_json::to_string(&file).unwrap()).unwrap();
        assert_eq!(file, again);
    }

    #[test]
    fn unknown_fields_are_rejected_not_silently_dropped() {
        let json = r#"{"schema":1,"entries":[{"id":"a","name":"A","task":"asr","weights":{"sources":[{"kind":"url","url":"https://x"}],"sha256":"00","size_bytes":1},"est_on_disk_bytes":1,"license":{"label":"MIT"},"surprise":1}]}"#;
        assert!(serde_json::from_str::<CatalogFile>(json).is_err());
    }

    fn entry() -> ModelEntry {
        find("whisper-base-ggml").unwrap().clone()
    }

    #[test]
    fn validation_catches_each_defect() {
        let mut dup = entry();
        dup.name = "dup".into();
        assert!(validate(&[entry(), dup])
            .iter()
            .any(|p| p.contains("duplicate")));

        let mut http = entry();
        http.weights.sources = vec![Source::Url {
            url: "http://x".into(),
        }];
        assert!(validate(&[http]).iter().any(|p| p.contains("https")));

        let mut sha = entry();
        sha.weights.sha256 = "ABC".into();
        assert!(validate(&[sha]).iter().any(|p| p.contains("sha256")));

        let mut traversal = entry();
        traversal.weight_rel = Some("../etc/passwd".into());
        assert!(validate(&[traversal])
            .iter()
            .any(|p| p.contains("weight_rel")));

        let mut no_source = entry();
        no_source.weights.sources.clear();
        assert!(validate(&[no_source])
            .iter()
            .any(|p| p.contains("no sources")));

        let mut vad_encoder = find("silero-vad").unwrap().clone();
        vad_encoder.coreml_encoder = entry().coreml_encoder;
        assert!(validate(&[vad_encoder])
            .iter()
            .any(|p| p.contains("coreml_encoder")));

        let mut branch = entry();
        branch.weights.sources = vec![Source::HuggingFace {
            repo: "tencent/Hy-MT2-1.8B-GGUF".into(),
            revision: "main".into(),
            file: "Hy-MT2-1.8B-Q8_0.gguf".into(),
        }];
        assert!(validate(&[branch]).iter().any(|p| p.contains("40-hex")));

        let mut escape = entry();
        escape.weights.sources = vec![Source::ModelScope {
            repo: "a/../b".into(),
            file: "x.gguf".into(),
        }];
        assert!(validate(&[escape]).iter().any(|p| p.contains("modelscope")));

        assert!(validate(&[entry()]).is_empty());
    }

    #[test]
    fn gguf_entries_must_be_consistent_and_credited() {
        let good = find("bge-m3-q8_0-gguf").unwrap().clone();
        assert!(validate(std::slice::from_ref(&good)).is_empty());

        let mut wrong_role = good.clone();
        wrong_role.task = Task::Llm;
        assert!(validate(&[wrong_role]).iter().any(|p| p.contains("role")));

        let mut missing = good.clone();
        missing.gguf = None;
        assert!(validate(&[missing]).iter().any(|p| p.contains("need gguf")));

        let mut translating_embedder = good.clone();
        translating_embedder.gguf.as_mut().unwrap().translation_only = true;
        assert!(validate(&[translating_embedder])
            .iter()
            .any(|p| p.contains("translation_only")));

        let mut uncredited = good.clone();
        uncredited.license.quantized_by = None;
        assert!(validate(&[uncredited]).iter().any(|p| p.contains("credit")));

        let mut stray = find("silero-vad").unwrap().clone();
        stray.gguf = good.gguf.clone();
        assert!(validate(&[stray])
            .iter()
            .any(|p| p.contains("only applies")));
    }

    #[test]
    fn every_gguf_entry_has_a_pinned_hub_source_and_a_china_fallback() {
        for e in bundled().iter().filter(|e| e.gguf.is_some()) {
            assert!(
                matches!(e.weights.sources.first(), Some(Source::HuggingFace { .. })),
                "{}",
                e.id
            );
            assert!(
                e.weights
                    .sources
                    .iter()
                    .any(|s| matches!(s, Source::ModelScope { .. })),
                "{}",
                e.id
            );
        }
    }

    #[test]
    fn client_version_gate_compares_numerically() {
        let mut e = entry();
        assert!(e.supports_client("0.0.1"));
        e.min_client_version = Some("1.10.0".into());
        assert!(e.supports_client("1.10.0"));
        assert!(e.supports_client("1.10.3-dev"));
        assert!(
            !e.supports_client("1.9.9"),
            "1.9.9 < 1.10.0 numerically, not lexically"
        );
        e.min_client_version = Some("1.2".into());
        assert!(validate(&[e]).iter().any(|p| p.contains("x.y.z")));
    }

    #[test]
    fn lenient_parse_skips_unknown_shapes_one_by_one() {
        let good = serde_json::to_value(entry()).unwrap();
        let mut future = good.clone();
        future["id"] = "future-model".into();
        future["task"] = "tts".into(); // 本版不认识的任务类型
        let mut extra = good.clone();
        extra["id"] = "extra-field".into();
        extra["new_field"] = 1.into(); // 本版不认识的字段
        let mut invalid = good.clone();
        invalid["id"] = "bad".into();
        invalid["weights"]["sha256"] = "nope".into();
        let (accepted, skipped) = parse_lenient(vec![good.clone(), future, extra, invalid, good]);
        assert_eq!(accepted.len(), 1, "only the valid, known, first-seen entry");
        assert_eq!(
            skipped, 4,
            "unknown task, unknown field, invalid, duplicate"
        );
    }

    /// 远程覆盖的三条语义；同一进程内的全局状态，放在一个测试里按顺序验，收尾清掉。
    #[test]
    fn remote_overlay_discovery_and_lookup() {
        let mut renamed = entry();
        renamed.name = "Whisper Base (remote)".into();
        let mut extra = entry();
        extra.id = "remote-only".into();
        remote::install(9, vec![renamed, extra], 0);

        let ids: Vec<&str> = discoverable().iter().map(|e| e.id.as_str()).collect();
        assert!(ids.contains(&"remote-only"));
        assert!(
            ids.contains(&"whisper-tiny-bundled"),
            "bundled entry always from the bundle"
        );
        assert!(
            !ids.contains(&"whisper-small-ggml"),
            "absent from remote = withdrawn from discovery"
        );
        assert_eq!(
            lookup("whisper-base-ggml").unwrap().name,
            "Whisper Base (remote)"
        );
        assert!(
            lookup("whisper-small-ggml").is_some(),
            "withdrawn entries stay resolvable for installed models"
        );

        remote::clear();
        assert_eq!(lookup("whisper-base-ggml").unwrap().name, "Whisper Base");
        assert!(discoverable().iter().any(|e| e.id == "whisper-small-ggml"));
    }

    #[test]
    fn hub_sources_build_the_resolve_urls_we_verified() {
        let hf = Source::HuggingFace {
            repo: "tencent/Hy-MT2-1.8B-GGUF".into(),
            revision: "a0c709d9fac510f2c807aa3af52872340dc37a4a".into(),
            file: "Hy-MT2-1.8B-Q8_0.gguf".into(),
        };
        assert_eq!(
            hf.url(),
            "https://huggingface.co/tencent/Hy-MT2-1.8B-GGUF/resolve/a0c709d9fac510f2c807aa3af52872340dc37a4a/Hy-MT2-1.8B-Q8_0.gguf"
        );
        let ms = Source::ModelScope {
            repo: "Tencent-Hunyuan/Hy-MT2-1.8B-GGUF".into(),
            file: "Hy-MT2-1.8B-Q8_0.gguf".into(),
        };
        assert_eq!(
            ms.url(),
            "https://www.modelscope.cn/models/Tencent-Hunyuan/Hy-MT2-1.8B-GGUF/resolve/master/Hy-MT2-1.8B-Q8_0.gguf"
        );
        let json = serde_json::to_string(&hf).unwrap();
        assert!(json.contains(r#""kind":"huggingface""#), "{json}");
    }

    /// 偏好只改顺序、不删来源：首选失败时其余来源仍是兜底。
    #[test]
    fn source_preference_reorders_without_dropping_fallbacks() {
        let artifact = Artifact {
            sources: vec![
                Source::HuggingFace {
                    repo: "o/r".into(),
                    revision: "a".repeat(40),
                    file: "w.gguf".into(),
                },
                Source::ModelScope {
                    repo: "o/r".into(),
                    file: "w.gguf".into(),
                },
            ],
            sha256: "0".repeat(64),
            size_bytes: 1,
            archive_member: None,
        };
        let hf = artifact.sources[0].url();
        let ms = artifact.sources[1].url();
        assert_eq!(
            artifact.urls_preferring(SourcePreference::Auto),
            [hf.clone(), ms.clone()]
        );
        assert_eq!(
            artifact.urls_preferring(SourcePreference::HuggingFace),
            [hf.clone(), ms.clone()]
        );
        assert_eq!(
            artifact.urls_preferring(SourcePreference::ModelScope),
            [ms, hf]
        );
        assert_eq!(
            SourcePreference::from_setting("modelscope"),
            SourcePreference::ModelScope
        );
        assert_eq!(
            SourcePreference::from_setting("bogus"),
            SourcePreference::Auto
        );
    }
}
