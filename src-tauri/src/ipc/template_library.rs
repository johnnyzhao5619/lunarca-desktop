//! 模板注册表与模板包（ADR-161 / ADR-169 ⑤ ⑦）。
//!
//! **三个域、各自一份清单**：笔记模板、AI 预设、整理模板各按「内置 < 内容层 < 库内」分层
//! （`luna_content::layered`，同名时外层优先），「应用 › 模板」页签与各自的选择器共用这一份投影。
//! 内容层里的条目来自装上的模板包；包里每一类内容交给它现有的归属方解析——笔记模板就是
//! Markdown（`luna-vault` 的渲染器），AI 预设与整理模板是 TOML（`luna_vault::parse_pack_preset`、
//! `luna_core::parse_pack_distill_template`，与库内的同一道校验），配方由 `luna-content-schema`
//! 解析并推导能力。**配方在本地导入阶段只展示、不执行**（执行随纵切 7b）。
//!
//! **从文件导入只在高级模式下开放**（PRD-18-024）：先 [`pick_template_pack`] 选文件并预览（包里有
//! 什么、它会做什么、未经审核），确认后 [`install_template_pack`] 再读一次同一个文件、与预览时的
//! sha256 比对后才走安装管线——确认框里看到的就是装上的那一份。

use std::collections::BTreeSet;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use super::*;
use luna_content::{
    layered, ContentDomain, ContentError, ContentStore, InstallError, Layer, Prepared, Source,
};
use luna_content_schema::{
    sha256_hex, unpack_template_pack, Capabilities, ContentId, PackArchiveError, PackAsset,
    PackAssetKind, Recipe, RelPath, TemplatePackError, TemplatePackManifest, HOST_CONTENT_API,
    MAX_PACK_BYTES, TEMPLATE_PACK_MANIFEST,
};

/// 高级模式的设置键（模板包导入只在这一档出现，PRD-18-024）。
const ADVANCED_MODE_KEY: &str = "settings.advanced_mode";

// ── 来源层 ──────────────────────────────────────────────────────────────────

/// 模板目录的交付变了（新条目、新版本、封禁 / 解除封禁，纵切 7b）：而设置没变，选择面与「获取更多模板」
/// 据此重读。同 `themes://changed` 的来由——封禁要在运行中生效。
#[derive(Debug, Clone, Serialize, Deserialize, Type, tauri_specta::Event)]
#[tauri_specta(event_name = "templates://changed")]
pub struct TemplateLibraryChanged;

/// 一条模板从哪一层来。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum TemplateLayerDto {
    /// 随 Lunarca 发布。
    Builtin,
    /// 装上的模板包（内容层，不随库同步）。
    Content,
    /// 库里你自己的（随库同步）。
    Vault,
}

impl From<Layer> for TemplateLayerDto {
    fn from(layer: Layer) -> Self {
        match layer {
            Layer::Builtin => Self::Builtin,
            Layer::Content => Self::Content,
            Layer::Vault => Self::Vault,
        }
    }
}

// ── 装上的模板包 ─────────────────────────────────────────────────────────────

/// 一个模板包里解析出来的东西（导入预览、已装清单与三个域的投影共用）。
#[derive(Debug, Clone)]
pub(crate) struct PackContents {
    pub manifest: TemplatePackManifest,
    /// 笔记模板：名字（文件名去掉 `.md`）与包内路径。
    pub note_templates: Vec<(String, RelPath)>,
    pub ai_presets: Vec<luna_vault::AiPreset>,
    pub distill_templates: Vec<luna_core::UserTemplate>,
    pub prompts: usize,
    pub recipes: Vec<Recipe>,
}

impl PackContents {
    pub(crate) fn capabilities(&self) -> Capabilities {
        Capabilities::of(&self.manifest, &self.recipes)
    }
}

/// 内容层里的一个模板包。
#[derive(Debug, Clone)]
pub(crate) struct InstalledPack {
    pub contents: PackContents,
    dir: PathBuf,
}

fn asset_error(asset: &PackAsset, reason: impl Into<String>) -> PackArchiveError {
    PackArchiveError::Asset {
        path: asset.path.as_str().to_owned(),
        reason: reason.into(),
    }
}

/// 包内路径的文件名去掉扩展名（条目在各自选择面上的名字与分层的键）。
fn stem_of(path: &RelPath) -> &str {
    let name = path.as_str().rsplit('/').next().unwrap_or(path.as_str());
    name.rsplit_once('.').map_or(name, |(stem, _)| stem)
}

/// 逐项读出清单里的资源（`read` 负责找到字节并比对 sha256），交给各自的归属方解析
/// （**纯函数**，导入与读取已装的包同一条路）。
fn parse_contents(
    manifest: TemplatePackManifest,
    mut read: impl FnMut(&PackAsset) -> Result<Vec<u8>, PackArchiveError>,
) -> Result<PackContents, PackArchiveError> {
    let mut contents = PackContents {
        manifest: manifest.clone(),
        note_templates: Vec::new(),
        ai_presets: Vec::new(),
        distill_templates: Vec::new(),
        prompts: 0,
        recipes: Vec::new(),
    };
    // 同一类内容里名字不能重复：名字是选择面上的标签，也是分层时「同名」的那个名。
    let mut names: BTreeSet<(PackAssetKind, String)> = BTreeSet::new();
    for asset in &manifest.assets {
        let bytes = read(asset)?;
        let text = String::from_utf8(bytes).map_err(|_| asset_error(asset, "not UTF-8 text"))?;
        let stem = stem_of(&asset.path).to_owned();
        if asset.kind != PackAssetKind::Prompt && !names.insert((asset.kind, stem.clone())) {
            return Err(asset_error(
                asset,
                "another item of this kind has the same name",
            ));
        }
        match asset.kind {
            PackAssetKind::NoteTemplate => {
                if !asset.path.as_str().ends_with(".md") {
                    return Err(asset_error(asset, "note templates are .md files"));
                }
                contents.note_templates.push((stem, asset.path.clone()));
            }
            PackAssetKind::AiPreset => contents.ai_presets.push(
                luna_vault::parse_pack_preset(&stem, &text)
                    .map_err(|reason| asset_error(asset, reason))?,
            ),
            PackAssetKind::DistillTemplate => contents.distill_templates.push(
                luna_core::parse_pack_distill_template(&stem, &text)
                    .map_err(|error| asset_error(asset, error.to_string()))?,
            ),
            PackAssetKind::Prompt => contents.prompts += 1,
            PackAssetKind::Recipe => contents.recipes.push(manifest.parse_recipe(&text)?),
        }
    }
    TemplatePackManifest::check_recipe_ids(&contents.recipes)?;
    Ok(contents)
}

/// 按模板包解析一个 zip（**纯函数**：安装管线把字节交进来）。解包与核对是与服务端审核共用的那一段
/// （`luna_content_schema::unpack_template_pack`）；宿主兼容性与各类正文的解析在这里。
pub(crate) fn prepare_template_pack(
    bytes: &[u8],
) -> Result<Prepared<PackContents>, PackArchiveError> {
    let pack = unpack_template_pack(bytes)?;
    pack.manifest.check_compatible(HOST_CONTENT_API)?;
    let contents = parse_contents(pack.manifest.clone(), |asset| {
        Ok(pack.asset(asset).to_vec())
    })?;
    Ok(Prepared {
        id: contents.manifest.id.clone(),
        version: contents.manifest.version,
        files: pack.into_files(),
        payload: contents,
    })
}

pub(crate) fn template_pack_store(app: &tauri::AppHandle) -> Result<ContentStore, String> {
    super::themes::content_store(app).map_err(|_| "TEMPLATE_PACK_STORAGE_UNAVAILABLE".to_owned())
}

/// 读一个已装的包（清单与每一项资源都重新解析、重新比对 sha256：内容层被改过的包不列）。
fn read_installed(dir: &Path, id: &ContentId) -> Option<InstalledPack> {
    let manifest = TemplatePackManifest::parse(
        &std::fs::read_to_string(dir.join(TEMPLATE_PACK_MANIFEST)).ok()?,
    )
    .ok()
    .filter(|manifest| manifest.id == *id)?;
    let contents = parse_contents(manifest, |asset| {
        let bytes = std::fs::read(dir.join(asset.path.as_str()))
            .map_err(|_| PackArchiveError::Missing(asset.path.as_str().to_owned()))?;
        if sha256_hex(&bytes) != asset.sha256 {
            return Err(PackArchiveError::Digest(asset.path.as_str().to_owned()));
        }
        Ok(bytes)
    })
    .ok()?;
    Some(InstalledPack {
        contents,
        dir: dir.to_path_buf(),
    })
}

/// 内容层里全部能读、能通过校验的模板包。
pub(crate) fn installed_template_packs(app: &tauri::AppHandle) -> Vec<InstalledPack> {
    let Ok(store) = template_pack_store(app) else {
        return Vec::new();
    };
    store
        .list(ContentDomain::TemplatePacks)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|item| read_installed(&item.dir, &item.id))
        .collect()
}

// ── 三个域的投影 ─────────────────────────────────────────────────────────────

/// 一份笔记模板指向哪里（新建与插入时交回来）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "layer", rename_all = "snake_case")]
pub enum TemplateFileDto {
    /// 装上的模板包里的一份 Markdown。
    Content { pack: String, path: String },
    /// 库里模板目录下的一份 Markdown（相对模板目录）。
    Vault { path: String },
}

/// 笔记模板清单里的一项。
#[derive(Debug, Clone, Serialize, Type)]
pub struct NoteTemplateEntryDto {
    /// 分层的键：内置是笔记类型，其余是名字。
    pub key: String,
    /// 展示名：内置为 `None`（前端按笔记类型取译名），其余是文件名。
    pub name: Option<String>,
    pub layer: TemplateLayerDto,
    /// 内置模板对应的笔记类型（交给 `create_note`）；其余为 `None`。
    pub note_type: Option<NoteTypeDto>,
    /// 其余模板的文件（交给 `create_note_from_template` / `render_template_for_insert`）。
    pub file: Option<TemplateFileDto>,
    /// 来自哪个模板包（展示名）；不是包里的为 `None`。
    pub pack_name: Option<String>,
    /// 被它盖住的同名模板在哪几层。
    pub shadows: Vec<TemplateLayerDto>,
}

struct NoteCandidate {
    key: String,
    entry: NoteTemplateEntryDto,
}

/// 笔记模板注册表（**纯函数**；三层由调用方读好交进来）。
fn note_template_registry(
    builtin: Vec<(String, NoteTypeDto)>,
    packs: &[InstalledPack],
    vault: Vec<UserTemplateInfoDto>,
) -> Vec<NoteTemplateEntryDto> {
    let entry = |key: String, name, layer, note_type, file, pack_name| NoteCandidate {
        key: key.clone(),
        entry: NoteTemplateEntryDto {
            key,
            name,
            layer,
            note_type,
            file,
            pack_name,
            shadows: Vec::new(),
        },
    };
    let builtin = builtin
        .into_iter()
        .map(|(key, note_type)| {
            entry(
                key,
                None,
                TemplateLayerDto::Builtin,
                Some(note_type),
                None,
                None,
            )
        })
        .collect();
    let content = packs
        .iter()
        .flat_map(|pack| {
            let manifest = &pack.contents.manifest;
            pack.contents
                .note_templates
                .iter()
                .map(move |(name, path)| {
                    entry(
                        name.clone(),
                        Some(name.clone()),
                        TemplateLayerDto::Content,
                        None,
                        Some(TemplateFileDto::Content {
                            pack: manifest.id.as_str().to_owned(),
                            path: path.as_str().to_owned(),
                        }),
                        Some(manifest.name.clone()),
                    )
                })
        })
        .collect();
    let vault = vault
        .into_iter()
        .map(|template| {
            entry(
                template.name.clone(),
                Some(template.name),
                TemplateLayerDto::Vault,
                None,
                Some(TemplateFileDto::Vault {
                    path: template.path,
                }),
                None,
            )
        })
        .collect();
    layered(
        vec![
            (Layer::Builtin, builtin),
            (Layer::Content, content),
            (Layer::Vault, vault),
        ],
        |candidate| candidate.key.clone(),
    )
    .into_iter()
    .map(|merged| NoteTemplateEntryDto {
        shadows: merged.shadows.into_iter().map(Into::into).collect(),
        ..merged.item.entry
    })
    .collect()
}

/// 笔记模板（新建笔记的模板选择器、插入模板与「应用 › 模板」共用）。没打开库时只有内置与包里的。
#[tauri::command]
#[specta::specta]
pub(crate) fn note_templates(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Vec<NoteTemplateEntryDto> {
    let settings = state.settings_snapshot();
    let builtin = luna_vault::list_builtin()
        .iter()
        .filter(|info| luna_config::apps::note_type_available(&settings, info.id.key()))
        .map(|info| (info.id.key().to_owned(), info.id.into()))
        .collect();
    let vault = state
        .vault
        .lock()
        .ok()
        .and_then(|guard| {
            guard.as_ref().and_then(|vault| {
                vault
                    .list_user_templates(&user_templates_dir(&settings))
                    .ok()
            })
        })
        .unwrap_or_default()
        .into_iter()
        .map(Into::into)
        .collect();
    note_template_registry(builtin, &installed_template_packs(&app), vault)
}

/// 读一份笔记模板的原文（库里的经 Vault 的路径校验，包里的只认清单登记过的笔记模板）。
pub(crate) fn read_template_file(
    app: &tauri::AppHandle,
    state: &AppState,
    file: &TemplateFileDto,
) -> Result<String, String> {
    match file {
        TemplateFileDto::Vault { path } => {
            let dir = user_templates_dir(&state.settings_snapshot());
            let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
            let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
            vault.read_user_template(&dir, path).map_err(vault_err)
        }
        TemplateFileDto::Content { pack, path } => {
            let packs = installed_template_packs(app);
            let pack = packs
                .iter()
                .find(|installed| installed.contents.manifest.id.as_str() == pack)
                .ok_or_else(|| "TEMPLATE_NOT_FOUND".to_owned())?;
            let (_, path) = pack
                .contents
                .note_templates
                .iter()
                .find(|(_, listed)| listed.as_str() == path)
                .ok_or_else(|| "TEMPLATE_NOT_FOUND".to_owned())?;
            std::fs::read_to_string(pack.dir.join(path.as_str()))
                .map_err(|_| "TEMPLATE_NOT_FOUND".to_owned())
        }
    }
}

/// AI 预设清单（库里的与包里的，同名时库里的优先）。
pub(crate) fn ai_preset_registry(
    packs: &[InstalledPack],
    vault: Vec<luna_vault::AiPreset>,
) -> Vec<FloatingPresetDto> {
    let content = packs
        .iter()
        .flat_map(|pack| {
            let manifest = &pack.contents.manifest;
            pack.contents
                .ai_presets
                .iter()
                .map(move |preset| FloatingPresetDto {
                    id: format!("{}/{}", manifest.id, preset.id),
                    name: preset.name.clone(),
                    instruction: preset.instruction.clone(),
                    layer: TemplateLayerDto::Content,
                    pack_name: Some(manifest.name.clone()),
                })
        })
        .collect();
    let vault = vault.into_iter().map(Into::into).collect();
    layered(
        vec![(Layer::Content, content), (Layer::Vault, vault)],
        |preset: &FloatingPresetDto| preset.name.clone(),
    )
    .into_iter()
    .map(|merged| merged.item)
    .collect()
}

/// 模板包带来的整理模板（连同包的展示名）。
pub(crate) fn pack_distill_templates(
    packs: &[InstalledPack],
) -> Vec<(luna_core::PackTemplate, String)> {
    packs
        .iter()
        .flat_map(|pack| {
            let manifest = &pack.contents.manifest;
            pack.contents.distill_templates.iter().map(move |template| {
                (
                    luna_core::PackTemplate {
                        pack: manifest.id.as_str().to_owned(),
                        template: template.clone(),
                    },
                    manifest.name.clone(),
                )
            })
        })
        .collect()
}

// ── 模板包的导入与卸载 ───────────────────────────────────────────────────────

/// 由包内容推导出的能力（确认框与已装清单写「它会做的事」）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct PackCapabilitiesDto {
    pub uses_ai: bool,
    pub transcribes: bool,
    pub translates: bool,
    /// 会在库里这些目录下新建笔记（配方声明的，去重排序）。
    pub creates_notes_in: Vec<String>,
    pub exports: bool,
}

impl From<Capabilities> for PackCapabilitiesDto {
    fn from(capabilities: Capabilities) -> Self {
        Self {
            uses_ai: capabilities.uses_ai,
            transcribes: capabilities.transcribes,
            translates: capabilities.translates,
            creates_notes_in: capabilities.creates_notes_in.into_iter().collect(),
            exports: capabilities.exports,
        }
    }
}

/// 一个模板包（导入预览与「已装的模板包」共用）。
#[derive(Debug, Clone, Serialize, Type)]
pub struct TemplatePackDto {
    pub id: String,
    pub version: String,
    pub name: String,
    pub publisher: String,
    pub license: String,
    pub note_templates: Vec<String>,
    pub ai_presets: Vec<String>,
    pub distill_templates: Vec<String>,
    pub prompts: u32,
    /// 工作流配方的标题（导入后只展示、不执行，执行随在线模板包）。
    pub recipes: Vec<String>,
    pub capabilities: PackCapabilitiesDto,
    /// 在线模板目录里有更新的版本（纵切 7b B2，与「获取更多模板」同一份状态）；没有或目录不可用为 `None`。
    pub update_to: Option<String>,
}

impl From<&PackContents> for TemplatePackDto {
    fn from(contents: &PackContents) -> Self {
        let manifest = &contents.manifest;
        Self {
            id: manifest.id.as_str().to_owned(),
            version: manifest.version.to_string(),
            name: manifest.name.clone(),
            publisher: manifest.publisher.clone(),
            license: manifest.license.clone(),
            note_templates: contents
                .note_templates
                .iter()
                .map(|(name, _)| name.clone())
                .collect(),
            ai_presets: contents
                .ai_presets
                .iter()
                .map(|preset| preset.name.clone())
                .collect(),
            distill_templates: contents
                .distill_templates
                .iter()
                .map(|template| template.name.clone())
                .collect(),
            prompts: u32::try_from(contents.prompts).unwrap_or(u32::MAX),
            recipes: contents
                .recipes
                .iter()
                .map(|recipe| recipe.title.clone())
                .collect(),
            capabilities: contents.capabilities().into(),
            update_to: None,
        }
    }
}

/// 导入前的预览：确认之后凭 `path` 与 `sha256` 安装。
#[derive(Debug, Clone, Serialize, Type)]
pub struct TemplatePackPreviewDto {
    /// 用户选的文件。
    pub path: String,
    /// 预览时读到的那份字节的 sha256（安装时再读一次，对不上就拒）。
    pub sha256: String,
    pub pack: TemplatePackDto,
    /// 已经装了这个包的哪一版（这次是更新）；没装为 `None`。
    pub installed_version: Option<String>,
}

fn require_advanced_mode(state: &AppState) -> Result<(), String> {
    if state
        .settings_snapshot()
        .get_bool(ADVANCED_MODE_KEY)
        .unwrap_or(false)
    {
        Ok(())
    } else {
        Err("TEMPLATE_PACK_ADVANCED_ONLY".to_owned())
    }
}

/// 读用户选的文件，最多读上限加一个字节（按读到的字节裁决，不信文件元数据）。
fn read_pack_file(path: &Path) -> Result<Vec<u8>, String> {
    let file =
        std::fs::File::open(path).map_err(|error| format!("TEMPLATE_PACK_READ_FAILED: {error}"))?;
    let mut bytes = Vec::new();
    file.take(MAX_PACK_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("TEMPLATE_PACK_READ_FAILED: {error}"))?;
    if bytes.len() as u64 > MAX_PACK_BYTES {
        return Err(format!(
            "TEMPLATE_PACK_REJECTED: {}",
            PackArchiveError::TooLarge
        ));
    }
    Ok(bytes)
}

/// 解析失败 → 稳定错误码（前端按码取文案）。宿主太旧单独一个码：那时要说「需要更新 Lunarca」。
fn pack_error_code(error: &PackArchiveError) -> String {
    match error {
        PackArchiveError::Pack(TemplatePackError::Incompatible(_)) => {
            format!("TEMPLATE_PACK_INCOMPATIBLE: {error}")
        }
        _ => format!("TEMPLATE_PACK_REJECTED: {error}"),
    }
}

pub(crate) fn install_error_code(error: &InstallError<PackArchiveError>) -> String {
    match error {
        InstallError::Invalid(inner) => pack_error_code(inner),
        InstallError::TooLarge { .. } => format!("TEMPLATE_PACK_REJECTED: {error}"),
        InstallError::Checksum => "TEMPLATE_PACK_CHANGED".to_owned(),
        InstallError::Store(ContentError::AlreadyInstalled { .. }) => {
            format!("TEMPLATE_PACK_ALREADY_INSTALLED: {error}")
        }
        InstallError::Read(_) => format!("TEMPLATE_PACK_READ_FAILED: {error}"),
        InstallError::Store(ContentError::Io(_)) => format!("TEMPLATE_PACK_WRITE_FAILED: {error}"),
    }
}

/// 选一个模板包文件并预览（高级模式，PRD-18-024）。不写任何东西；用户在选文件时取消返回 `None`。
///
/// # Errors
/// `TEMPLATE_PACK_ADVANCED_ONLY`、`TEMPLATE_PACK_REJECTED`（格式、校验、过大）、
/// `TEMPLATE_PACK_INCOMPATIBLE`（要更新 Lunarca）、`TEMPLATE_PACK_ALREADY_INSTALLED`（已装同一或更新
/// 版本）、`TEMPLATE_PACK_READ_FAILED`。
#[tauri::command]
#[specta::specta]
pub(crate) async fn pick_template_pack(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<Option<TemplatePackPreviewDto>, String> {
    use tauri_plugin_dialog::DialogExt as _;

    require_advanced_mode(&state)?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter("Lunarca Template Pack", &["zip"])
        .pick_file(move |path| {
            let result = path
                .map(|path| path.into_path().map_err(|error| error.to_string()))
                .transpose();
            let _ = tx.send(result);
        });
    let Some(path) = rx
        .await
        .map_err(|_| "TEMPLATE_PACK_DIALOG_CANCELLED".to_owned())??
    else {
        return Ok(None);
    };
    let bytes = read_pack_file(&path)?;
    let prepared = prepare_template_pack(&bytes).map_err(|error| pack_error_code(&error))?;
    let installed = template_pack_store(&app)?
        .get(ContentDomain::TemplatePacks, &prepared.id)
        .ok()
        .flatten();
    if let Some(installed) = &installed {
        if installed.version >= prepared.version {
            return Err(format!(
                "TEMPLATE_PACK_ALREADY_INSTALLED: {} {}",
                prepared.id, installed.version
            ));
        }
    }
    Ok(Some(TemplatePackPreviewDto {
        path: path.to_string_lossy().into_owned(),
        sha256: sha256_hex(&bytes),
        pack: (&prepared.payload).into(),
        installed_version: installed.map(|item| item.version.to_string()),
    }))
}

/// 装上确认过的那个模板包：再读一次同一个文件，与预览时的 sha256 比对后走安装管线（与市场安装
/// 同一条，ADR-169 ②）。任一步失败内容层都不变。
///
/// # Errors
/// 同 [`pick_template_pack`]；文件在确认期间被改过时 `TEMPLATE_PACK_CHANGED`；写盘失败
/// `TEMPLATE_PACK_WRITE_FAILED`。
#[tauri::command]
#[specta::specta]
pub(crate) fn install_template_pack(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
    sha256: String,
) -> Result<TemplatePackDto, String> {
    require_advanced_mode(&state)?;
    let bytes = read_pack_file(Path::new(&path))?;
    let (_, contents) = luna_content::install(
        &template_pack_store(&app)?,
        ContentDomain::TemplatePacks,
        Source::Downloaded {
            bytes,
            sha256: &sha256,
        },
        MAX_PACK_BYTES,
        prepare_template_pack,
    )
    .map_err(|error| install_error_code(&error))?;
    Ok((&contents).into())
}

/// 装上的模板包（「应用 › 模板」的「模板包」一节与「已安装」页签），连同在线目录里有没有更新的版本。
#[tauri::command]
#[specta::specta]
pub(crate) fn template_packs(app: tauri::AppHandle) -> Vec<TemplatePackDto> {
    let updates = super::template_market::update_versions(&app);
    installed_template_packs(&app)
        .iter()
        .map(|pack| {
            let mut dto: TemplatePackDto = (&pack.contents).into();
            dto.update_to = updates
                .iter()
                .find(|(id, _)| *id == dto.id)
                .map(|(_, version)| version.clone());
            dto
        })
        .collect()
}

/// 卸载一个模板包：它带来的模板、预设与整理模板从各自的选择面消失；库里的内容不受影响
/// （用它新建的笔记、记着它的整理模板键的整理笔记都还在，PRD-18-013）。本来就没装也算成功。
///
/// # Errors
/// id 不合规或删除失败时 `TEMPLATE_PACK_UNINSTALL_FAILED`。
#[tauri::command]
#[specta::specta]
pub(crate) fn uninstall_template_pack(app: tauri::AppHandle, id: String) -> Result<(), String> {
    let id = ContentId::parse(&id)
        .map_err(|error| format!("TEMPLATE_PACK_UNINSTALL_FAILED: {error}"))?;
    template_pack_store(&app)?
        .remove(ContentDomain::TemplatePacks, &id)
        .map_err(|error| format!("TEMPLATE_PACK_UNINSTALL_FAILED: {error}"))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write as _;

    pub(crate) fn manifest_for(id: &str, version: &str, files: &[(&str, &str, &str)]) -> String {
        let mut manifest = format!(
            "schema_version = 1\nid = \"{id}\"\nversion = \"{version}\"\ncontent_api = \">=1, <2\"\nname = \"起步包\"\npublisher = \"Ada\"\nlicense = \"CC0-1.0\"\n"
        );
        for (path, kind, content) in files {
            manifest.push_str(&format!(
                "\n[[assets]]\npath = \"{path}\"\nkind = \"{kind}\"\nsha256 = \"{}\"\n",
                sha256_hex(content.as_bytes())
            ));
        }
        manifest
    }

    /// 拼一个模板包 zip：`files` 是（包内路径，种类，内容），清单里的 sha256 按内容自动算。
    pub(crate) fn pack_zip(id: &str, version: &str, files: &[(&str, &str, &str)]) -> Vec<u8> {
        let mut entries: Vec<(&str, String)> =
            vec![(TEMPLATE_PACK_MANIFEST, manifest_for(id, version, files))];
        entries.extend(
            files
                .iter()
                .map(|(path, _, content)| (*path, (*content).to_owned())),
        );
        raw_zip(&entries)
    }

    pub(crate) fn raw_zip(entries: &[(&str, String)]) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(&mut out);
        for (name, content) in entries {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(content.as_bytes()).unwrap();
        }
        writer.finish().unwrap();
        out.into_inner()
    }

    pub(crate) const PRESET: &str = "name = \"收紧\"\ninstruction = \"删掉一半的字\"\n";
    pub(crate) const DISTILL: &str =
        "name = \"周报\"\n\n[[sections]]\nheading = \"本周\"\n\n[[sections]]\nheading = \"下周\"\n";
    pub(crate) const PROMPT: &str = "把下面的文字改写成短视频脚本。";
    pub(crate) const RECIPE: &str = "id = \"publish\"\ntitle = \"发布包\"\n\n[[steps]]\ntype = \"llm\"\nprompt = \"prompts/script.md\"\n\n[[steps]]\ntype = \"vault_create_note\"\ndir = \"Projects\"\n";

    pub(crate) fn full_pack(version: &str) -> Vec<u8> {
        pack_zip(
            "com.example.kit",
            version,
            &[
                (
                    "notes/weekly.md",
                    "note_template",
                    "# {{title}}\n\n## 本周\n",
                ),
                ("presets/tighten.toml", "ai_preset", PRESET),
                ("distill/weekly.toml", "distill_template", DISTILL),
                ("prompts/script.md", "prompt", PROMPT),
                ("recipes/publish.toml", "recipe", RECIPE),
            ],
        )
    }

    #[test]
    fn a_full_pack_is_parsed_by_each_owner_and_its_capabilities_are_derived() {
        let prepared = prepare_template_pack(&full_pack("1.0.0")).expect("valid pack");
        let dto = TemplatePackDto::from(&prepared.payload);
        assert_eq!(dto.note_templates, ["weekly"]);
        assert_eq!(dto.ai_presets, ["收紧"]);
        assert_eq!(dto.distill_templates, ["周报"]);
        assert_eq!(dto.prompts, 1);
        assert_eq!(dto.recipes, ["发布包"]);
        assert!(dto.capabilities.uses_ai);
        assert_eq!(dto.capabilities.creates_notes_in, ["Projects"]);
        assert!(!dto.capabilities.exports);
        assert_eq!(prepared.files.len(), 6, "清单加五项资源原样落盘");
    }

    #[test]
    fn packs_with_tampered_unlisted_unsafe_or_invalid_content_are_rejected() {
        let rejected = |bytes: Vec<u8>| prepare_template_pack(&bytes).map(|_| ()).unwrap_err();
        assert!(matches!(
            rejected(b"not a zip".to_vec()),
            PackArchiveError::Zip(_)
        ));

        // 清单写的 sha256 与内容对不上。
        let good = full_pack("1.0.0");
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(good)).unwrap();
        let mut entries = Vec::new();
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).unwrap();
            let mut text = String::new();
            entry.read_to_string(&mut text).unwrap();
            let name = String::from_utf8_lossy(entry.name_raw()).into_owned();
            entries.push((
                name.clone(),
                if name == "presets/tighten.toml" {
                    PRESET.replace("一半", "三成")
                } else {
                    text
                },
            ));
        }
        let view: Vec<(&str, String)> = entries
            .iter()
            .map(|(n, t)| (n.as_str(), t.clone()))
            .collect();
        assert!(matches!(
            rejected(raw_zip(&view)),
            PackArchiveError::Digest(_)
        ));

        // 夹带清单没登记的文件。
        let mut smuggled = view.clone();
        smuggled.push(("run.sh", "rm -rf ~".to_owned()));
        assert!(matches!(
            rejected(raw_zip(&smuggled)),
            PackArchiveError::Unlisted(_)
        ));

        // 越出包根的路径。
        assert!(matches!(
            rejected(raw_zip(&[
                (TEMPLATE_PACK_MANIFEST, String::new()),
                ("../escape.md", "x".to_owned())
            ])),
            PackArchiveError::Entry(_)
        ));

        // 各归属方的校验：预设多字段、整理模板没区块、配方引用包里没有的提示词。
        for (path, kind, content) in [
            (
                "presets/x.toml",
                "ai_preset",
                "name = \"a\"\ninstruction = \"b\"\nmodel = \"c\"\n",
            ),
            (
                "distill/x.toml",
                "distill_template",
                "name = \"a\"\nsections = []\n",
            ),
            ("recipes/x.toml", "recipe", RECIPE),
            ("notes/x.txt", "note_template", "x"),
        ] {
            assert!(
                prepare_template_pack(&pack_zip(
                    "com.example.kit",
                    "1.0.0",
                    &[(path, kind, content)]
                ))
                .is_err(),
                "{path} 应被拒"
            );
        }

        // 要更新 Lunarca 的包单独一个码。
        let newer = manifest_for("com.example.kit", "1.0.0", &[]).replace(">=1, <2", ">=2, <3");
        let error = rejected(raw_zip(&[(TEMPLATE_PACK_MANIFEST, newer)]));
        assert!(pack_error_code(&error).starts_with("TEMPLATE_PACK_INCOMPATIBLE"));
    }

    #[test]
    fn the_install_pipeline_stores_the_pack_and_refuses_reinstalling_the_same_version() {
        let root = tempfile::tempdir().expect("tempdir");
        let store = ContentStore::new(root.path());
        let install = |bytes: Vec<u8>| {
            let sha = sha256_hex(&bytes);
            luna_content::install(
                &store,
                ContentDomain::TemplatePacks,
                Source::Downloaded {
                    bytes,
                    sha256: &sha,
                },
                MAX_PACK_BYTES,
                prepare_template_pack,
            )
        };
        let (item, _) = install(full_pack("1.0.0")).expect("install");
        let installed = read_installed(&item.dir, &item.id).expect("readable");
        assert_eq!(installed.contents.ai_presets[0].name, "收紧");
        let again = install(full_pack("1.0.0")).expect_err("same version");
        assert!(install_error_code(&again).starts_with("TEMPLATE_PACK_ALREADY_INSTALLED"));
        // 确认后文件被换掉：sha256 对不上。
        let changed = luna_content::install(
            &store,
            ContentDomain::TemplatePacks,
            Source::Downloaded {
                bytes: full_pack("2.0.0"),
                sha256: &sha256_hex(&full_pack("1.1.0")),
            },
            MAX_PACK_BYTES,
            prepare_template_pack,
        )
        .expect_err("changed file");
        assert_eq!(install_error_code(&changed), "TEMPLATE_PACK_CHANGED");
        // 内容层被改过的包不列。
        std::fs::write(
            item.dir.join("presets/tighten.toml"),
            "name = \"x\"\ninstruction = \"y\"\n",
        )
        .unwrap();
        assert!(read_installed(&item.dir, &item.id).is_none());
    }

    fn installed_full_pack() -> (tempfile::TempDir, InstalledPack) {
        let root = tempfile::tempdir().expect("tempdir");
        let store = ContentStore::new(root.path());
        let bytes = full_pack("1.0.0");
        let sha = sha256_hex(&bytes);
        let (item, _) = luna_content::install(
            &store,
            ContentDomain::TemplatePacks,
            Source::Downloaded {
                bytes,
                sha256: &sha,
            },
            MAX_PACK_BYTES,
            prepare_template_pack,
        )
        .expect("install");
        let pack = read_installed(&item.dir, &item.id).expect("readable");
        (root, pack)
    }

    #[test]
    fn note_templates_are_layered_builtin_then_pack_then_vault_and_same_names_shadow() {
        let (_root, pack) = installed_full_pack();
        let registry = note_template_registry(
            vec![
                ("note".to_owned(), NoteTypeDto::Note),
                ("chat".to_owned(), NoteTypeDto::Chat),
            ],
            std::slice::from_ref(&pack),
            vec![
                UserTemplateInfoDto {
                    path: "weekly.md".into(),
                    name: "weekly".into(),
                },
                UserTemplateInfoDto {
                    path: "chat.md".into(),
                    name: "chat".into(),
                },
            ],
        );
        let view: Vec<_> = registry
            .iter()
            .map(|entry| (entry.key.as_str(), entry.layer, entry.shadows.clone()))
            .collect();
        assert_eq!(
            view,
            [
                ("note", TemplateLayerDto::Builtin, vec![]),
                (
                    "chat",
                    TemplateLayerDto::Vault,
                    vec![TemplateLayerDto::Builtin]
                ),
                (
                    "weekly",
                    TemplateLayerDto::Vault,
                    vec![TemplateLayerDto::Content]
                ),
            ]
        );
        let only_pack = note_template_registry(Vec::new(), std::slice::from_ref(&pack), Vec::new());
        assert_eq!(only_pack[0].pack_name.as_deref(), Some("起步包"));
        assert_eq!(
            only_pack[0].file,
            Some(TemplateFileDto::Content {
                pack: "com.example.kit".into(),
                path: "notes/weekly.md".into()
            })
        );
    }

    #[test]
    fn presets_from_packs_join_the_vault_ones_and_lose_to_the_same_name() {
        let (_root, pack) = installed_full_pack();
        let mine = luna_vault::AiPreset {
            id: "p1".into(),
            name: "收紧".into(),
            instruction: "我自己的".into(),
        };
        let presets = ai_preset_registry(std::slice::from_ref(&pack), vec![mine]);
        assert_eq!(presets.len(), 1);
        assert_eq!(presets[0].layer, TemplateLayerDto::Vault);
        assert_eq!(presets[0].instruction, "我自己的");
        let presets = ai_preset_registry(std::slice::from_ref(&pack), Vec::new());
        assert_eq!(presets[0].id, "com.example.kit/tighten");
        assert_eq!(presets[0].pack_name.as_deref(), Some("起步包"));
    }
}
