//! 模板包契约（ADR-161 / ADR-169 ⑤ ⑦）：一个 zip，根目录一份 `manifest.toml` 加资源文件。
//!
//! 包里能有五类内容：笔记模板、AI 预设、整理模板、提示词、工作流配方。前四类只在清单里登记
//! 路径与 sha256，正文交给现有归属方按各自的类型解析（`luna-vault` 的模板与预设、`luna-core` 的
//! 整理模板）；**配方是数据**——有序步骤列表，步骤类型是白名单，这里定义并校验它。
//!
//! 能力由包内容**推导**（[`Capabilities::of`]），不由作者声明：会不会调用 AI、会在库里哪些目录
//! 新建笔记、会不会导出文件。服务端审核与客户端安装各算一遍，对不上即拒装（ADR-161 ③）；
//! 更新后能力变多时要重新确认（ADR-159 ⑦）。

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{ContentId, PackVersion, RelPath};

/// 清单格式版本。
pub const TEMPLATE_PACK_SCHEMA_VERSION: u32 = 1;

/// 宿主这一版实现的内容接口版本（`content_api` 区间与它比）。新增一种配方步骤 = 宿主发一个版本
/// 并升这个数（ADR-161「已知代价」）。
pub const HOST_CONTENT_API: u32 = 1;

/// 解压后总大小上限（ADR-159 ⑥）。
pub const MAX_PACK_BYTES: u64 = 5 * 1024 * 1024;

/// 解压后文件数上限（ADR-159 ⑥；含 `manifest.toml`）。
pub const MAX_PACK_FILES: usize = 200;

/// 清单里登记的资源最多几项（留一个给清单本身）。
const MAX_ASSETS: usize = MAX_PACK_FILES - 1;

/// 一个配方最多几步。
const MAX_RECIPE_STEPS: usize = 32;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TemplatePackError {
    #[error("invalid template pack manifest: {0}")]
    Parse(String),
    #[error("unsupported template pack schema: {0}")]
    Schema(u32),
    #[error("invalid template pack metadata: {0}")]
    Metadata(&'static str),
    #[error("version must look like 1.2.3")]
    Version,
    #[error("content_api must look like \">=1, <2\" or \"1\"")]
    ContentApi,
    #[error("this pack needs content API {0}, this Lunarca provides {HOST_CONTENT_API}")]
    Incompatible(String),
    #[error("sha256 must be 64 lowercase hex characters: {0}")]
    Digest(String),
    #[error("asset {0} is listed more than once")]
    DuplicateAsset(String),
    #[error("too many assets (at most {MAX_ASSETS})")]
    TooManyAssets,
    #[error("recipe {recipe} references {path}, which is not a prompt in this pack")]
    UnknownPrompt { recipe: String, path: String },
    #[error("recipe {0} has no steps or too many (at most {MAX_RECIPE_STEPS})")]
    RecipeLength(String),
    #[error("recipe id {0} is used more than once")]
    DuplicateRecipe(String),
}

/// 包要求的内容接口区间：`">=1, <2"`（下界含、上界不含）或单个 `"1"`（= `>=1, <2`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContentApiRange {
    pub min: u32,
    pub max_exclusive: u32,
}

impl ContentApiRange {
    /// # Errors
    /// 写法不合规或区间为空时。
    pub fn parse(value: &str) -> Result<Self, TemplatePackError> {
        let number = |text: &str| {
            text.trim()
                .parse::<u32>()
                .map_err(|_| TemplatePackError::ContentApi)
        };
        let value = value.trim();
        let range = if let Some((low, high)) = value.split_once(',') {
            let min = number(
                low.trim()
                    .strip_prefix(">=")
                    .ok_or(TemplatePackError::ContentApi)?,
            )?;
            let max_exclusive = number(
                high.trim()
                    .strip_prefix('<')
                    .ok_or(TemplatePackError::ContentApi)?,
            )?;
            Self { min, max_exclusive }
        } else {
            let exact = number(value)?;
            Self {
                min: exact,
                max_exclusive: exact.checked_add(1).ok_or(TemplatePackError::ContentApi)?,
            }
        };
        if range.min == 0 || range.min >= range.max_exclusive {
            return Err(TemplatePackError::ContentApi);
        }
        Ok(range)
    }

    /// 宿主这一版能不能装它。
    #[must_use]
    pub fn accepts(self, host: u32) -> bool {
        self.min <= host && host < self.max_exclusive
    }
}

/// 包里一项资源是什么（决定交给谁解析、推导出什么能力）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackAssetKind {
    /// 笔记模板（PRD-06-015 的占位符语法，`luna-vault` 解析）。
    NoteTemplate,
    /// AI 预设（PRD-06-024，`luna-vault` 解析）。
    AiPreset,
    /// 整理模板（PRD-06-051，`luna-core` 解析）。
    DistillTemplate,
    /// 提示词（配方的 `llm` 步骤引用它；不进能力提示词注册表，ADR-161 ④）。
    Prompt,
    /// 工作流配方（本模块的 [`Recipe`]）。
    Recipe,
}

/// 清单里登记的一项资源。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackAsset {
    pub path: RelPath,
    pub kind: PackAssetKind,
    /// 小写十六进制 sha256（解压后逐个比对，`luna-content` 负责）。
    pub sha256: String,
}

/// 一份配方：有序步骤，由 Rust 解释执行并落到 Pipeline 与 AI 主干（ADR-161 ②，执行随纵切 7b）。
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    pub id: ContentId,
    pub title: String,
    pub steps: Vec<RecipeStep>,
}

/// 配方接在哪一类东西上（纵切 7b B5，PRD-18-010「配方出现在对应空间的动作菜单」）。**由步骤推导、不另设字段**
/// （同能力，ADR-161 ③）：第一步是 `transcribe` 的接在录音上，其余接在笔记上——`transcribe` 只能是第一步
/// （[`TemplatePackManifest::parse_recipe`] 守着），于是这条判据不会两可。
/// > 假设：不在清单里加 `input` 字段（理由：加字段会让 1.0.0 客户端按 `deny_unknown_fields` 拒掉整个包，要抬
/// > `content_api`，而「从录音出发」本来就写在第一步里）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecipeInput {
    /// 录音 / 转写笔记：从它的转写稿出发。
    Recording,
    /// 任意一篇笔记：从它的正文出发。
    Note,
}

impl Recipe {
    /// 这份配方接在哪一类东西上（见 [`RecipeInput`]）。
    #[must_use]
    pub fn input(&self) -> RecipeInput {
        match self.steps.first() {
            Some(RecipeStep::Transcribe) => RecipeInput::Recording,
            _ => RecipeInput::Note,
        }
    }

    /// 只这一份配方会做的事（运行前的确认框写它，与整包的 [`Capabilities::of`] 分开：包里的预设会调用 AI，
    /// 不等于这份配方会）。
    #[must_use]
    pub fn capabilities(&self) -> Capabilities {
        let mut capabilities = Capabilities::default();
        capabilities.add_steps(&self.steps);
        capabilities
    }
}

/// 配方步骤白名单（v1）。不能联网、不能执行命令、不能写声明以外的目录（ADR-161 否决项）。
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RecipeStep {
    /// 转写配方输入里的音视频。
    Transcribe,
    /// 翻译上一步的文字到 `to`（语言代码）。
    Translate { to: String },
    /// 调一次 AI：`prompt` 是包内一份提示词的路径。
    Llm { prompt: RelPath },
    /// 在库里 `dir` 下新建一篇笔记（不覆盖已有文件）。
    VaultCreateNote { dir: RelPath },
    /// 按导出注册表里的一种格式导出（格式名由宿主的导出注册表判定，这里只收名字）。
    Export { format: String },
}

/// 由包内容推导出的能力（安装与更新时展示；更新后变多要重新确认）。也是模板目录条目的一部分
/// （`TemplateCatalogEntry`），故收线格式时不许有多余字段。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    /// 会调用 AI（配方的 `llm` 步骤，或包里有 AI 预设 / 整理模板）。
    pub uses_ai: bool,
    /// 会转写音视频。
    pub transcribes: bool,
    /// 会翻译。
    pub translates: bool,
    /// 会在库里这些目录下新建笔记（配方声明的目录，去重排序）。
    pub creates_notes_in: BTreeSet<String>,
    /// 会导出文件。
    pub exports: bool,
}

impl Capabilities {
    /// 从清单与已解析的配方推导能力（**纯函数**）。
    #[must_use]
    pub fn of(manifest: &TemplatePackManifest, recipes: &[Recipe]) -> Self {
        let mut capabilities = Self {
            uses_ai: manifest.assets.iter().any(|asset| {
                matches!(
                    asset.kind,
                    PackAssetKind::AiPreset | PackAssetKind::DistillTemplate
                )
            }),
            ..Self::default()
        };
        for recipe in recipes {
            capabilities.add_steps(&recipe.steps);
        }
        capabilities
    }

    fn add_steps(&mut self, steps: &[RecipeStep]) {
        for step in steps {
            match step {
                RecipeStep::Transcribe => self.transcribes = true,
                RecipeStep::Translate { .. } => self.translates = true,
                RecipeStep::Llm { .. } => self.uses_ai = true,
                RecipeStep::VaultCreateNote { dir } => {
                    self.creates_notes_in.insert(dir.as_str().to_owned());
                }
                RecipeStep::Export { .. } => self.exports = true,
            }
        }
    }

    /// 这一份比 `previous` 多出了能力吗（更新时据此决定要不要重新确认，ADR-159 ⑦）。
    #[must_use]
    pub fn exceeds(&self, previous: &Self) -> bool {
        (self.uses_ai && !previous.uses_ai)
            || (self.transcribes && !previous.transcribes)
            || (self.translates && !previous.translates)
            || (self.exports && !previous.exports)
            || !self.creates_notes_in.is_subset(&previous.creates_notes_in)
    }
}

/// `manifest.toml` 的线格式（解析后再校验成 [`TemplatePackManifest`]）。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    schema_version: u32,
    id: ContentId,
    version: String,
    content_api: String,
    name: String,
    publisher: String,
    license: String,
    #[serde(default)]
    locales: Vec<String>,
    #[serde(default)]
    assets: Vec<RawAsset>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAsset {
    path: RelPath,
    kind: PackAssetKind,
    sha256: String,
}

/// 校验过的模板包清单。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplatePackManifest {
    pub id: ContentId,
    pub version: PackVersion,
    pub content_api: ContentApiRange,
    pub name: String,
    pub publisher: String,
    /// SPDX 表达式（是否为已知许可证由审核把关，这里只校验长度与字符）。
    pub license: String,
    pub locales: Vec<String>,
    pub assets: Vec<PackAsset>,
}

impl TemplatePackManifest {
    /// 解析并校验 `manifest.toml`。不检查宿主兼容性——那是 [`Self::check_compatible`]。
    ///
    /// # Errors
    /// 见 [`TemplatePackError`]。
    pub fn parse(source: &str) -> Result<Self, TemplatePackError> {
        let raw: RawManifest =
            toml::from_str(source).map_err(|error| TemplatePackError::Parse(error.to_string()))?;
        if raw.schema_version != TEMPLATE_PACK_SCHEMA_VERSION {
            return Err(TemplatePackError::Schema(raw.schema_version));
        }
        text_field(&raw.name, "name")?;
        text_field(&raw.publisher, "publisher")?;
        text_field(&raw.license, "license")?;
        for locale in &raw.locales {
            let ok = (2..=16).contains(&locale.len())
                && locale
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-');
            if !ok {
                return Err(TemplatePackError::Metadata("locales"));
            }
        }
        if raw.assets.len() > MAX_ASSETS {
            return Err(TemplatePackError::TooManyAssets);
        }
        let mut seen = BTreeSet::new();
        let mut assets = Vec::with_capacity(raw.assets.len());
        for asset in raw.assets {
            if !is_sha256(&asset.sha256) {
                return Err(TemplatePackError::Digest(asset.path.as_str().to_owned()));
            }
            if !seen.insert(asset.path.as_str().to_owned())
                || asset.path.as_str() == "manifest.toml"
            {
                return Err(TemplatePackError::DuplicateAsset(
                    asset.path.as_str().to_owned(),
                ));
            }
            assets.push(PackAsset {
                path: asset.path,
                kind: asset.kind,
                sha256: asset.sha256,
            });
        }
        Ok(Self {
            id: raw.id,
            version: PackVersion::parse(&raw.version).map_err(|_| TemplatePackError::Version)?,
            content_api: ContentApiRange::parse(&raw.content_api)?,
            name: raw.name,
            publisher: raw.publisher,
            license: raw.license,
            locales: raw.locales,
            assets,
        })
    }

    /// 这一版 Lunarca 能不能装它（不兼容时只给「需要更新 Lunarca」，PRD-18-007）。
    ///
    /// # Errors
    /// 宿主的内容接口不在包要求的区间里时。
    pub fn check_compatible(&self, host: u32) -> Result<(), TemplatePackError> {
        if self.content_api.accepts(host) {
            Ok(())
        } else {
            Err(TemplatePackError::Incompatible(format!(
                ">={}, <{}",
                self.content_api.min, self.content_api.max_exclusive
            )))
        }
    }

    /// 清单里某一类资源的路径。
    pub fn assets_of(&self, kind: PackAssetKind) -> impl Iterator<Item = &PackAsset> {
        self.assets.iter().filter(move |asset| asset.kind == kind)
    }

    /// 解析并校验一份配方：步数在限内，`llm` 步骤引用的必须是本包登记的提示词。
    ///
    /// # Errors
    /// 见 [`TemplatePackError`]。
    pub fn parse_recipe(&self, source: &str) -> Result<Recipe, TemplatePackError> {
        let recipe: Recipe =
            toml::from_str(source).map_err(|error| TemplatePackError::Parse(error.to_string()))?;
        text_field(&recipe.title, "recipe title")?;
        if recipe.steps.is_empty() || recipe.steps.len() > MAX_RECIPE_STEPS {
            return Err(TemplatePackError::RecipeLength(
                recipe.id.as_str().to_owned(),
            ));
        }
        // `transcribe` 转写的是配方的输入（录音），它前面不能有别的步骤：那时手里已经是文字了。
        if recipe
            .steps
            .iter()
            .skip(1)
            .any(|step| matches!(step, RecipeStep::Transcribe))
        {
            return Err(TemplatePackError::Metadata(
                "transcribe can only be the first recipe step",
            ));
        }
        for step in &recipe.steps {
            if let RecipeStep::Llm { prompt } = step {
                let known = self
                    .assets_of(PackAssetKind::Prompt)
                    .any(|asset| asset.path == *prompt);
                if !known {
                    return Err(TemplatePackError::UnknownPrompt {
                        recipe: recipe.id.as_str().to_owned(),
                        path: prompt.as_str().to_owned(),
                    });
                }
            }
            if let RecipeStep::Translate { to } | RecipeStep::Export { format: to } = step {
                if to.is_empty()
                    || to.len() > 32
                    || !to
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
                {
                    return Err(TemplatePackError::Metadata("recipe step argument"));
                }
            }
        }
        Ok(recipe)
    }

    /// 一组配方的 id 不能重复（同一包内）。
    ///
    /// # Errors
    /// 有重复时。
    pub fn check_recipe_ids(recipes: &[Recipe]) -> Result<(), TemplatePackError> {
        let mut seen = BTreeSet::new();
        for recipe in recipes {
            if !seen.insert(recipe.id.as_str()) {
                return Err(TemplatePackError::DuplicateRecipe(
                    recipe.id.as_str().to_owned(),
                ));
            }
        }
        Ok(())
    }
}

fn text_field(value: &str, field: &'static str) -> Result<(), TemplatePackError> {
    let len = value.trim().chars().count();
    if !(1..=64).contains(&len) || value.chars().any(char::is_control) {
        return Err(TemplatePackError::Metadata(field));
    }
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn manifest_source() -> String {
        format!(
            r#"
schema_version = 1
id = "com.example.creator-kit"
version = "1.2.0"
content_api = ">=1, <2"
name = "创作者起步包"
publisher = "Lunarca"
license = "CC-BY-4.0"
locales = ["zh-CN", "en-US"]

[[assets]]
path = "templates/weekly.md"
kind = "note_template"
sha256 = "{DIGEST}"

[[assets]]
path = "prompts/summary.md"
kind = "prompt"
sha256 = "{DIGEST}"

[[assets]]
path = "recipes/publish.toml"
kind = "recipe"
sha256 = "{DIGEST}"
"#
        )
    }

    const RECIPE: &str = r#"
id = "publish"
title = "一键发布包"

[[steps]]
type = "transcribe"

[[steps]]
type = "llm"
prompt = "prompts/summary.md"

[[steps]]
type = "vault_create_note"
dir = "Projects"

[[steps]]
type = "export"
format = "srt"
"#;

    #[test]
    fn a_valid_manifest_parses_and_is_compatible_with_this_host() {
        let manifest = TemplatePackManifest::parse(&manifest_source()).expect("valid");
        assert_eq!(manifest.id.as_str(), "com.example.creator-kit");
        assert_eq!(manifest.version.to_string(), "1.2.0");
        assert!(manifest.check_compatible(HOST_CONTENT_API).is_ok());
        assert!(manifest.check_compatible(2).is_err());
        assert_eq!(manifest.assets_of(PackAssetKind::Recipe).count(), 1);
    }

    #[test]
    fn capabilities_are_derived_from_the_recipe_not_declared() {
        let manifest = TemplatePackManifest::parse(&manifest_source()).expect("valid");
        let recipe = manifest.parse_recipe(RECIPE).expect("recipe");
        let capabilities = Capabilities::of(&manifest, &[recipe]);
        assert!(capabilities.uses_ai && capabilities.transcribes && capabilities.exports);
        assert!(!capabilities.translates);
        assert_eq!(
            capabilities.creates_notes_in.iter().collect::<Vec<_>>(),
            ["Projects"]
        );

        let note_only = Capabilities::of(&manifest, &[]);
        assert_eq!(
            note_only,
            Capabilities::default(),
            "只有笔记模板与提示词：什么都不做"
        );
        assert!(capabilities.exceeds(&note_only));
        assert!(!note_only.exceeds(&capabilities));
        let mut more_dirs = capabilities.clone();
        more_dirs.creates_notes_in.insert("Inbox".to_owned());
        assert!(
            more_dirs.exceeds(&capabilities),
            "多一个能写的目录也算能力变多"
        );
    }

    /// 配方接在哪：第一步是转写的接录音，其余接笔记；转写只能是第一步（纵切 7b B5）。只这份配方的能力不含
    /// 包里预设带来的「调用 AI」。
    #[test]
    fn a_recipe_input_follows_its_first_step_and_transcribe_only_comes_first() {
        let manifest = TemplatePackManifest::parse(&manifest_source()).expect("valid");
        let recording = manifest.parse_recipe(RECIPE).expect("recipe");
        assert_eq!(recording.input(), RecipeInput::Recording);
        let note = manifest
            .parse_recipe("id = \"com.example.tidy\"\ntitle = \"Tidy\"\nsteps = [{ type = \"vault_create_note\", dir = \"Inbox\" }]\n")
            .expect("recipe");
        assert_eq!(note.input(), RecipeInput::Note);
        assert_eq!(
            note.capabilities()
                .creates_notes_in
                .iter()
                .collect::<Vec<_>>(),
            ["Inbox"]
        );
        assert!(!note.capabilities().uses_ai);
        let late = "id = \"com.example.late\"\ntitle = \"Late\"\nsteps = [{ type = \"translate\", to = \"en\" }, { type = \"transcribe\" }]\n";
        assert!(manifest.parse_recipe(late).is_err());
    }

    #[test]
    fn presets_and_distill_templates_count_as_using_ai() {
        let source = manifest_source().replace("kind = \"note_template\"", "kind = \"ai_preset\"");
        let manifest = TemplatePackManifest::parse(&source).expect("valid");
        assert!(Capabilities::of(&manifest, &[]).uses_ai);
    }

    #[test]
    fn recipes_cannot_reach_outside_the_whitelist() {
        let manifest = TemplatePackManifest::parse(&manifest_source()).expect("valid");
        for bad in [
            RECIPE.replace("type = \"export\"", "type = \"shell\""),
            RECIPE.replace("dir = \"Projects\"", "dir = \"../outside\""),
            RECIPE.replace("dir = \"Projects\"", "dir = \".luna/settings\""),
            RECIPE.replace("prompts/summary.md", "prompts/elsewhere.md"),
            RECIPE.replace("format = \"srt\"", "format = \"srt; rm -rf\""),
            RECIPE.replace("format = \"srt\"", "format = \"srt\"\nurl = \"https://x\""),
            "id = \"empty\"\ntitle = \"空\"\nsteps = []\n".to_owned(),
        ] {
            assert!(manifest.parse_recipe(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn manifests_reject_bad_versions_ranges_digests_and_duplicates() {
        let source = manifest_source();
        for bad in [
            source.replace("version = \"1.2.0\"", "version = \"1.2\""),
            source.replace("version = \"1.2.0\"", "version = \"1.2.0-beta\""),
            source.replace("version = \"1.2.0\"", "version = \"01.2.0\""),
            source.replace("content_api = \">=1, <2\"", "content_api = \">=2, <2\""),
            source.replace("content_api = \">=1, <2\"", "content_api = \"*\""),
            source.replacen(DIGEST, "ABC", 1),
            source.replace("prompts/summary.md", "templates/weekly.md"),
            source.replace(
                "path = \"recipes/publish.toml\"",
                "path = \"manifest.toml\"",
            ),
            source.replace("schema_version = 1", "schema_version = 2"),
            source.replace(
                "license = \"CC-BY-4.0\"",
                "license = \"CC-BY-4.0\"\nscript = \"x\"",
            ),
        ] {
            assert!(TemplatePackManifest::parse(&bad).is_err(), "{bad}");
        }
        assert_eq!(
            ContentApiRange::parse("1"),
            Ok(ContentApiRange {
                min: 1,
                max_exclusive: 2
            })
        );
    }

    #[test]
    fn recipe_ids_must_be_unique_within_a_pack() {
        let manifest = TemplatePackManifest::parse(&manifest_source()).expect("valid");
        let recipe = manifest.parse_recipe(RECIPE).expect("recipe");
        assert!(TemplatePackManifest::check_recipe_ids(std::slice::from_ref(&recipe)).is_ok());
        assert!(TemplatePackManifest::check_recipe_ids(&[recipe.clone(), recipe]).is_err());
    }

    #[test]
    fn garbled_manifests_and_recipes_never_panic() {
        let source = manifest_source();
        let manifest = TemplatePackManifest::parse(&source).expect("valid");
        let mut seed: u32 = 0x1234_5678;
        for text in [source.as_str(), RECIPE] {
            for cut in (0..text.len()).filter(|cut| text.is_char_boundary(*cut)) {
                let _ = TemplatePackManifest::parse(&text[..cut]);
                let _ = manifest.parse_recipe(&text[..cut]);
            }
            for _ in 0..500 {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let at = (seed as usize) % text.len();
                if !text.is_char_boundary(at) {
                    continue;
                }
                let mut garbled = text.to_owned();
                garbled.insert(at, char::from(b'!' + (seed % 90) as u8));
                let _ = TemplatePackManifest::parse(&garbled);
                let _ = manifest.parse_recipe(&garbled);
            }
        }
    }
}
