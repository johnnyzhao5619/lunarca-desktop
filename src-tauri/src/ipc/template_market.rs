//! 在线模板目录（纵切 7b B2，ADR-159 ⑥ ⑦、ADR-161 ③，PRD-18-004 / 006 / 007 / 008 / 011）：「获取更多模板」列什么、
//! 每一条此刻是什么状态，以及获取与更新。
//!
//! 条目来自已验签模板目录的本地副本（零网络）；获取时 Rust 按 id 从那份目录里重新取那一条（不信前端交来的任何内容），
//! 经目录客户端同一个 pinned TLS 客户端整块取回包体，与签名目录里的 sha256 比对、走与本地导入同一个
//! `prepare_template_pack`，再用公开的 `TemplateCatalogEntry::derive` 重算一遍与目录那一条比对——服务端审核时算出的
//! 能力与这里装上的能力对不上，就拒装（ADR-161 ③）。更新时新版本能力比已装的多，要用户重新确认（ADR-159 ⑦）。

use luna_content::{ContentDomain, Source};
use luna_content_schema::{
    Capabilities, PackVersion, TemplateCatalogEntry, HOST_CONTENT_API, MAX_PACK_BYTES,
};

use super::template_library::{
    install_error_code, installed_template_packs, prepare_template_pack, template_pack_store,
    PackCapabilitiesDto, TemplatePackDto,
};
use super::*;

fn client_version() -> PackVersion {
    PackVersion::parse(env!("CARGO_PKG_VERSION")).expect("应用版本是 x.y.z")
}

/// 在线目录里一个模板包此刻对这台客户端是什么状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum MarketTemplateState {
    /// 没装，可以获取。
    Available,
    /// 已装同一或更新的版本。
    Installed,
    /// 已装旧版本，可以更新（用户点了才装，PRD-18-011）。
    UpdateAvailable,
    /// 这台客户端低于最低版本、或宿主内容接口不在包要求的区间里：只说「需要更新 Lunarca」（PRD-18-007）。
    NeedsNewerApp,
}

/// 包里每一类内容各有几项。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
pub struct MarketTemplateContentsDto {
    pub note_templates: u32,
    pub ai_presets: u32,
    pub distill_templates: u32,
    pub prompts: u32,
    pub recipes: u32,
}

/// 「获取更多模板」里的一个包（卡片与确认框）。
#[derive(Debug, Clone, Serialize, Type)]
pub struct MarketTemplateDto {
    pub id: String,
    pub version: String,
    pub name: String,
    pub publisher: String,
    pub license: String,
    pub contents: MarketTemplateContentsDto,
    /// 由内容推导的能力（服务端审核时算的，装的时候再算一遍比对）。
    pub capabilities: PackCapabilitiesDto,
    /// 包体字节数。
    pub bytes: f64,
    pub state: MarketTemplateState,
    /// 已装的版本（`UpdateAvailable` 时给出「从哪一版更新」）。
    pub installed_version: Option<String>,
    /// 更新会多出的能力（只在 `UpdateAvailable` 且新版本能力比已装的多时给出）：确认框据此写「新版本会多做这些事」，
    /// 安装命令据此要求带上确认（PRD-18-011）。
    pub new_capabilities: Option<PackCapabilitiesDto>,
}

/// 「获取更多模板」区块的内容。
#[derive(Debug, Clone, Serialize, Type)]
pub struct TemplateMarketDto {
    pub packs: Vec<MarketTemplateDto>,
}

/// 一份签名目录里**认得出、校验得过**的条目（**纯函数**）：本版不认识的形状只跳过那一条，不连累整份目录。
fn catalog_templates(values: Vec<serde_json::Value>) -> Vec<TemplateCatalogEntry> {
    values
        .into_iter()
        .filter_map(|value| serde_json::from_value(value).ok())
        .filter(|entry| luna_content_schema::validate_template_entry(entry).is_ok())
        .collect()
}

/// 此刻可用的在线模板条目；入口不该出现时为 `None`（严格本地、未登录、没有生效的模板目录或目录里一条都没有）。
fn available_catalog_templates(app: &tauri::AppHandle) -> Option<Vec<TemplateCatalogEntry>> {
    if !super::content_market::online_catalog_usable(app) {
        return None;
    }
    let catalogs = app.try_state::<luna_cloud::commands::Catalogs>()?;
    let entries = catalog_templates(catalogs.templates.current()?.entries);
    (!entries.is_empty()).then_some(entries)
}

/// `new` 比 `old` 多出的那些能力（**纯函数**）：只留新版本有、旧版本没有的。
fn capability_gain(new: &Capabilities, old: &Capabilities) -> PackCapabilitiesDto {
    PackCapabilitiesDto {
        uses_ai: new.uses_ai && !old.uses_ai,
        transcribes: new.transcribes && !old.transcribes,
        translates: new.translates && !old.translates,
        creates_notes_in: new
            .creates_notes_in
            .difference(&old.creates_notes_in)
            .cloned()
            .collect(),
        exports: new.exports && !old.exports,
    }
}

/// 已装的一个包：id、版本、能力（市场状态只需要这三样）。
pub(crate) struct InstalledSummary {
    pub id: String,
    pub version: PackVersion,
    pub capabilities: Capabilities,
}

pub(crate) fn installed_summaries(app: &tauri::AppHandle) -> Vec<InstalledSummary> {
    installed_template_packs(app)
        .iter()
        .map(|pack| InstalledSummary {
            id: pack.contents.manifest.id.as_str().to_owned(),
            version: pack.contents.manifest.version,
            capabilities: pack.contents.capabilities(),
        })
        .collect()
}

/// 目录条目对照已装的包，算出每一条的状态（**纯函数**）。
fn market_templates(
    entries: &[TemplateCatalogEntry],
    installed: &[InstalledSummary],
    client: PackVersion,
    host_content_api: u32,
) -> Vec<MarketTemplateDto> {
    entries
        .iter()
        .map(|entry| {
            let local = installed.iter().find(|item| item.id == entry.id.as_str());
            let state = match local {
                _ if !entry.supports(client, host_content_api) => {
                    MarketTemplateState::NeedsNewerApp
                }
                Some(local) if local.version >= entry.version => MarketTemplateState::Installed,
                Some(_) => MarketTemplateState::UpdateAvailable,
                None => MarketTemplateState::Available,
            };
            let new_capabilities = match local {
                Some(local)
                    if state == MarketTemplateState::UpdateAvailable
                        && entry.capabilities.exceeds(&local.capabilities) =>
                {
                    Some(capability_gain(&entry.capabilities, &local.capabilities))
                }
                _ => None,
            };
            MarketTemplateDto {
                id: entry.id.as_str().to_owned(),
                version: entry.version.to_string(),
                name: entry.name.clone(),
                publisher: entry.publisher.clone(),
                license: entry.license.clone(),
                contents: MarketTemplateContentsDto {
                    note_templates: entry.contents.note_templates,
                    ai_presets: entry.contents.ai_presets,
                    distill_templates: entry.contents.distill_templates,
                    prompts: entry.contents.prompts,
                    recipes: entry.contents.recipes,
                },
                capabilities: entry.capabilities.clone().into(),
                // 包体 ≤ 5 MiB，f64 精确表示。
                bytes: entry.bytes as f64,
                state,
                installed_version: local.map(|local| local.version.to_string()),
                new_capabilities,
            }
        })
        .collect()
}

/// 已装的包在线上有没有更新的版本（`template_packs` 的 `update_to`，与「获取更多模板」同一份状态）。
pub(crate) fn update_versions(app: &tauri::AppHandle) -> Vec<(String, String)> {
    let Some(entries) = available_catalog_templates(app) else {
        return Vec::new();
    };
    market_templates(
        &entries,
        &installed_summaries(app),
        client_version(),
        HOST_CONTENT_API,
    )
    .into_iter()
    .filter(|pack| pack.state == MarketTemplateState::UpdateAvailable)
    .map(|pack| (pack.id, pack.version))
    .collect()
}

/// 「获取更多模板」（**零网络**：读已验签目录的本地副本）。`None` = 这一块不出现（PRD-18-006）。
#[tauri::command]
#[specta::specta]
pub(crate) fn template_market(app: tauri::AppHandle) -> Option<TemplateMarketDto> {
    let entries = available_catalog_templates(&app)?;
    Some(TemplateMarketDto {
        packs: market_templates(
            &entries,
            &installed_summaries(&app),
            client_version(),
            HOST_CONTENT_API,
        ),
    })
}

/// 获取或更新在线目录里的一个模板包（PRD-18-008 / 010 / 011）。
///
/// `accept_new_capabilities`：用户已在确认框里看过「新版本会多做这些事」。更新时新版本能力比已装的多而它为假，
/// 什么都不下载、直接回 `TEMPLATE_MARKET_CONFIRM_CAPABILITIES`。
///
/// # Errors
/// `TEMPLATE_MARKET_UNAVAILABLE`（严格本地 / 未登录 / 没有生效的模板目录）、`TEMPLATE_MARKET_NOT_FOUND`、
/// `TEMPLATE_MARKET_NEEDS_UPDATE`、`TEMPLATE_MARKET_CONFIRM_CAPABILITIES`、`STRICT_LOCAL_BLOCKED`、
/// `TEMPLATE_MARKET_DOWNLOAD_FAILED`、`TEMPLATE_MARKET_MISMATCH`（下回来的包与目录对不上），以及安装管线的
/// `TEMPLATE_PACK_*`。
#[tauri::command]
#[specta::specta]
pub(crate) async fn install_market_template(
    app: tauri::AppHandle,
    id: String,
    accept_new_capabilities: bool,
) -> Result<TemplatePackDto, String> {
    let entries = available_catalog_templates(&app)
        .ok_or_else(|| "TEMPLATE_MARKET_UNAVAILABLE".to_owned())?;
    let entry = entries
        .into_iter()
        .find(|entry| entry.id.as_str() == id)
        .ok_or_else(|| format!("TEMPLATE_MARKET_NOT_FOUND: {id}"))?;
    if !entry.supports(client_version(), HOST_CONTENT_API) {
        return Err(format!(
            "TEMPLATE_MARKET_NEEDS_UPDATE: requires {} and content API {}",
            entry.min_client_version, entry.content_api
        ));
    }
    let installed = installed_summaries(&app)
        .into_iter()
        .find(|item| item.id == entry.id.as_str());
    if let Some(installed) = &installed {
        if entry.capabilities.exceeds(&installed.capabilities) && !accept_new_capabilities {
            return Err(format!("TEMPLATE_MARKET_CONFIRM_CAPABILITIES: {id}"));
        }
    }
    let catalogs = app
        .try_state::<luna_cloud::commands::Catalogs>()
        .map(|catalogs| catalogs.inner().clone())
        .ok_or_else(|| "TEMPLATE_MARKET_UNAVAILABLE".to_owned())?;
    let bytes = catalogs
        .templates
        .fetch_package(
            &luna_cloud::catalog::template_package_path(&entry.sha256),
            MAX_PACK_BYTES,
        )
        .await
        .map_err(|error| match error {
            luna_cloud::CloudError::StrictLocalBlocked => "STRICT_LOCAL_BLOCKED".to_owned(),
            error => format!("TEMPLATE_MARKET_DOWNLOAD_FAILED: {error}"),
        })?;
    verify_against_catalog(&bytes, &entry)?;
    let (_, contents) = luna_content::install(
        &template_pack_store(&app)?,
        ContentDomain::TemplatePacks,
        Source::Downloaded {
            bytes,
            sha256: &entry.sha256,
        },
        MAX_PACK_BYTES,
        prepare_template_pack,
    )
    .map_err(|error| install_error_code(&error))?;
    let _ = tauri_specta::Event::emit(&super::template_library::TemplateLibraryChanged, &app);
    Ok((&contents).into())
}

/// 下回来的包体用公开的 `derive` 重算一遍，与目录那一条比对（**纯函数**，ADR-161 ③）：摘要、能力、sha256、大小
/// 任一不同即拒——服务端审核看到的就是这里装上的。
fn verify_against_catalog(bytes: &[u8], entry: &TemplateCatalogEntry) -> Result<(), String> {
    match TemplateCatalogEntry::derive(bytes, entry.min_client_version) {
        Ok(derived) if derived.same_package(entry) => Ok(()),
        Ok(_) => Err(format!("TEMPLATE_MARKET_MISMATCH: {}", entry.id.as_str())),
        Err(error) => Err(format!("TEMPLATE_MARKET_MISMATCH: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::template_library::tests::{manifest_for, raw_zip};

    fn v(text: &str) -> PackVersion {
        PackVersion::parse(text).unwrap()
    }

    /// 一个包：一份笔记模板，可选一份在 `dir` 下新建笔记的配方。
    fn pack(id: &str, version: &str, creates_in: Option<&str>) -> Vec<u8> {
        let template = "# {{title}}\n";
        let mut files: Vec<(&str, &str, String)> =
            vec![("templates/brief.md", "note_template", template.to_owned())];
        let recipe;
        if let Some(dir) = creates_in {
            recipe = format!(
                "id = \"{id}.file\"\ntitle = \"File it\"\nsteps = [{{ type = \"vault_create_note\", dir = \"{dir}\" }}]\n"
            );
            files.push(("recipes/file.toml", "recipe", recipe.clone()));
        }
        let listed: Vec<(&str, &str, &str)> = files
            .iter()
            .map(|(path, kind, content)| (*path, *kind, content.as_str()))
            .collect();
        let manifest = manifest_for(id, version, &listed);
        let mut entries: Vec<(&str, String)> = vec![("manifest.toml", manifest)];
        entries.extend(
            files
                .iter()
                .map(|(path, _, content)| (*path, content.clone())),
        );
        raw_zip(&entries)
    }

    fn entry(bytes: &[u8]) -> TemplateCatalogEntry {
        TemplateCatalogEntry::derive(bytes, v("1.0.0")).unwrap()
    }

    fn installed(id: &str, version: &str, creates_in: Option<&str>) -> InstalledSummary {
        let e = entry(&pack(id, version, creates_in));
        InstalledSummary {
            id: id.to_owned(),
            version: e.version,
            capabilities: e.capabilities,
        }
    }

    #[test]
    fn states_follow_the_installed_version_and_what_this_host_supports() {
        let catalog = vec![
            entry(&pack("com.example.fresh", "1.0.0", None)),
            entry(&pack("com.example.same", "1.0.0", None)),
            entry(&pack("com.example.newer", "1.2.0", None)),
            {
                let mut future = entry(&pack("com.example.future", "1.0.0", None));
                future.min_client_version = v("99.0.0");
                future
            },
        ];
        let local = vec![
            installed("com.example.same", "1.0.0", None),
            installed("com.example.newer", "1.1.0", None),
        ];
        let states: Vec<_> = market_templates(&catalog, &local, v("1.0.0"), HOST_CONTENT_API)
            .into_iter()
            .map(|pack| {
                (
                    pack.id,
                    pack.state,
                    pack.installed_version,
                    pack.new_capabilities,
                )
            })
            .collect();
        assert_eq!(
            states,
            vec![
                (
                    "com.example.fresh".to_owned(),
                    MarketTemplateState::Available,
                    None,
                    None
                ),
                (
                    "com.example.same".to_owned(),
                    MarketTemplateState::Installed,
                    Some("1.0.0".to_owned()),
                    None
                ),
                (
                    "com.example.newer".to_owned(),
                    MarketTemplateState::UpdateAvailable,
                    Some("1.1.0".to_owned()),
                    None
                ),
                (
                    "com.example.future".to_owned(),
                    MarketTemplateState::NeedsNewerApp,
                    None,
                    None
                ),
            ]
        );
        // 宿主内容接口不在区间里同样是「需要更新 Lunarca」。
        let other_host = market_templates(&catalog[..1], &[], v("1.0.0"), HOST_CONTENT_API + 1);
        assert_eq!(other_host[0].state, MarketTemplateState::NeedsNewerApp);
    }

    /// 更新后能力变多：只把多出来的那几样交给确认框（PRD-18-011）；没变多就不打扰。
    #[test]
    fn an_update_that_gains_capabilities_names_exactly_what_it_gains() {
        let catalog = vec![entry(&pack("com.example.grow", "2.0.0", Some("Projects")))];
        let gained = market_templates(
            &catalog,
            &[installed("com.example.grow", "1.0.0", None)],
            v("1.0.0"),
            HOST_CONTENT_API,
        );
        let gain = gained[0].new_capabilities.clone().expect("能力变多了");
        assert_eq!(gain.creates_notes_in, vec!["Projects".to_owned()]);
        assert!(!gain.uses_ai && !gain.exports && !gain.transcribes && !gain.translates);

        let same = market_templates(
            &catalog,
            &[installed("com.example.grow", "1.0.0", Some("Projects"))],
            v("1.0.0"),
            HOST_CONTENT_API,
        );
        assert!(same[0].new_capabilities.is_none());
    }

    /// 下回来的包与目录那一条对不上（换了包、目录里能力被改小）即拒；原样的放行（ADR-161 ③）。
    #[test]
    fn downloaded_bytes_must_derive_exactly_the_catalog_entry() {
        let bytes = pack("com.example.studio", "1.0.0", Some("Summaries"));
        let listed = entry(&bytes);
        assert!(verify_against_catalog(&bytes, &listed).is_ok());

        let mut quieter = listed.clone();
        quieter.capabilities.creates_notes_in.clear();
        assert!(verify_against_catalog(&bytes, &quieter)
            .unwrap_err()
            .starts_with("TEMPLATE_MARKET_MISMATCH"));

        let other = pack("com.example.studio", "1.0.0", None);
        assert!(verify_against_catalog(&other, &listed)
            .unwrap_err()
            .starts_with("TEMPLATE_MARKET_MISMATCH"));
        assert!(verify_against_catalog(b"not a zip", &listed)
            .unwrap_err()
            .starts_with("TEMPLATE_MARKET_MISMATCH"));
    }

    #[test]
    fn unreadable_or_malformed_catalog_entries_are_skipped_one_by_one() {
        let good = serde_json::to_value(entry(&pack("com.example.ok", "1.0.0", None))).unwrap();
        let mut bad_sha = good.clone();
        bad_sha["sha256"] = serde_json::json!("nope");
        let entries = catalog_templates(vec![good, bad_sha, serde_json::json!({ "id": 1 })]);
        assert_eq!(entries.len(), 1);
    }
}
