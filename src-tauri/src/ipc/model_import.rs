//! 导入 GGUF 模型的命令（ADR-144 ③）。闸与落位在 [`crate::model_import`]；这里只接对话框、
//! 跑阻塞任务、导入成功后重新编排引擎。
//!
//! 跨模块可见性：命令与 DTO `pub(crate)`，经 `mod.rs` 的 `use model_import::*` 回到
//! `collect_commands!` 作用域。

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::Manager as _;

use super::AppState;

/// 一次导入的结果。
#[derive(Debug, Serialize, Deserialize, Type)]
pub(crate) struct ModelImportResultDto {
    /// 落成的条目 id。
    pub model_id: String,
    /// 展示名。
    pub name: String,
    /// sha256 与目录条目一致，按目录条目安装（不另起用户条目）。
    pub matched_catalog: bool,
    /// 导入闸读出的角色（界面在导入后说出"识别为对话 / 嵌入模型"）。
    pub role: ImportedModelRoleDto,
}

/// 导入模型的角色（`model_import::ImportedRole` 的投影；池化细节不过界）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ImportedModelRoleDto {
    /// 对话 / 文本生成。
    Chat,
    /// 嵌入（语义检索）。
    Embedding,
}

impl From<&crate::model_import::ImportedRole> for ImportedModelRoleDto {
    fn from(role: &crate::model_import::ImportedRole) -> Self {
        match role {
            crate::model_import::ImportedRole::Chat => Self::Chat,
            crate::model_import::ImportedRole::Embed { .. } => Self::Embedding,
        }
    }
}

/// 随包 `llama-fit-params`（导入闸的引擎试读要用它）；GGUF 不可用的构建里返回 `None`。
pub(crate) fn fit_params(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    crate::model_store::engine_build()?;
    let root = app.path().resource_dir().ok()?;
    luna_engine::llama::runtime::locate(&root).map(|rt| rt.fit_params)
}

/// 从本地文件导入一个 GGUF 模型（离线动作，不联网）。用户取消选择时返回 `None`。
///
/// # Errors
/// `MODEL_IMPORT_UNAVAILABLE`（本构建不支持 GGUF）或导入闸的拒收码（`MODEL_IMPORT_*`）。
#[tauri::command]
#[specta::specta]
pub(crate) async fn import_local_gguf(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<Option<ModelImportResultDto>, String> {
    let Some(fit) = fit_params(&app) else {
        return Err(
            "MODEL_IMPORT_UNAVAILABLE: local engine not available in this build".to_owned(),
        );
    };
    let Some(path) = super::pick_file(app.clone()).await? else {
        return Ok(None);
    };
    let source = std::path::PathBuf::from(path);
    let file_name = source
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let root = state.models_root(&app);
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        std::fs::create_dir_all(&root).map_err(|e| format!("MODEL_IMPORT_FAILED: {e}"))?;
        crate::model_import::import_file(
            &source,
            crate::model_import::ImportOrigin::LocalFile { file_name },
            None,
            &root,
            &fit,
            &crate::model_import::catalog_entry_by_sha,
        )
        .map_err(|e| e.code())
    })
    .await
    .map_err(|e| format!("MODEL_IMPORT_FAILED: {e}"))??;
    crate::local_engine::spawn_reconcile_restart(&app);
    Ok(Some(ModelImportResultDto {
        model_id: outcome.model_id,
        name: outcome.name,
        matched_catalog: outcome.matched_catalog,
        role: (&outcome.role).into(),
    }))
}

/// HF 仓库里的一个 GGUF 文件。
#[derive(Debug, Serialize, Deserialize, Type)]
pub(crate) struct HfGgufFileDto {
    /// 仓库内路径。
    pub path: String,
    /// 字节数。
    pub size_bytes: f64,
    /// sha256 与目录条目相同时，那个条目的展示名（导入即按它安装）。
    pub catalog_name: Option<String>,
    /// 已经装在本机。
    pub already_installed: bool,
}

/// 一个 HF 仓库在当前提交上的 GGUF 清单。
#[derive(Debug, Serialize, Deserialize, Type)]
pub(crate) struct HfRepoDto {
    /// `owner/name`。
    pub repo: String,
    /// 钉住的提交号。
    pub revision: String,
    /// 模型卡里登记的许可。
    pub license: Option<String>,
    /// GGUF 文件。
    pub files: Vec<HfGgufFileDto>,
}

fn hf_error_code(error: luna_model_fetch::hf::HfError) -> String {
    use luna_model_fetch::hf::HfError;
    match error {
        HfError::InvalidRepo => "MODEL_IMPORT_HF_INVALID: expected owner/name".to_owned(),
        HfError::NotFound => {
            "MODEL_IMPORT_HF_NOT_FOUND: repository not found or not public".to_owned()
        }
        HfError::Gated => {
            "MODEL_IMPORT_HF_GATED: repository requires sign-in or access approval".to_owned()
        }
        HfError::Network(detail) => format!("MODEL_IMPORT_HF_NETWORK: {detail}"),
    }
}

/// 严格本地模式拦截联网（与模型下载同一处通知与错误码）。
fn strict_local_guard(app: &tauri::AppHandle, state: &AppState) -> Result<(), String> {
    if state.strict_local() {
        if let Some(center) = app.try_state::<crate::notifications::NotificationCenter>() {
            center.publish(
                crate::notifications::NotificationEvent::strict_local_blocked(),
                app,
            );
        }
        return Err(super::coded_error(
            app,
            "STRICT_LOCAL_BLOCKED",
            "strict-local-blocked-install",
        ));
    }
    Ok(())
}

/// 列出一个 HF 仓库当前提交上的 GGUF 文件（高级模式「按 HF 仓库 id 添加」；联网，用户显式动作）。
///
/// # Errors
/// 严格本地拦截、`MODEL_IMPORT_UNAVAILABLE` 或 `MODEL_IMPORT_HF_*`。
#[tauri::command]
#[specta::specta]
pub(crate) async fn hf_repo_ggufs(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    repo: String,
) -> Result<HfRepoDto, String> {
    if fit_params(&app).is_none() {
        return Err(
            "MODEL_IMPORT_UNAVAILABLE: local engine not available in this build".to_owned(),
        );
    }
    strict_local_guard(&app, &state)?;
    let client =
        luna_model_fetch::catalog_client().map_err(|e| format!("MODEL_IMPORT_HF_NETWORK: {e}"))?;
    let listing = luna_model_fetch::hf::repo_ggufs(&client, repo.trim())
        .await
        .map_err(hf_error_code)?;
    let root = state.models_root(&app);
    Ok(HfRepoDto {
        files: listing
            .files
            .into_iter()
            .map(|f| HfGgufFileDto {
                catalog_name: crate::model_import::catalog_entry_by_sha(&f.sha256)
                    .map(|e| e.name.clone()),
                already_installed: crate::model_import::already_installed(&root, &f.sha256),
                path: f.path,
                size_bytes: f.size_bytes as f64,
            })
            .collect(),
        repo: listing.repo,
        revision: listing.revision,
        license: listing.license,
    })
}

/// 下载 HF 仓库里的一个 GGUF 文件并导入。服务端侧重新取清单核对文件（不信前端传来的大小与
/// 摘要），下载钉在当前提交上、边下边校验 sha256；下完走同一道导入闸。进度经
/// `model://download-progress` 发出，键为 `hf:<repo>/<path>`。
///
/// # Errors
/// 严格本地拦截、`MODEL_IMPORT_*` / `MODEL_IMPORT_HF_*` / 下载失败码。
#[tauri::command]
#[specta::specta]
pub(crate) async fn import_hf_gguf(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    repo: String,
    path: String,
) -> Result<ModelImportResultDto, String> {
    use tauri_specta::Event as _;
    let Some(fit) = fit_params(&app) else {
        return Err(
            "MODEL_IMPORT_UNAVAILABLE: local engine not available in this build".to_owned(),
        );
    };
    strict_local_guard(&app, &state)?;
    let client =
        luna_model_fetch::catalog_client().map_err(|e| format!("MODEL_IMPORT_HF_NETWORK: {e}"))?;
    let listing = luna_model_fetch::hf::repo_ggufs(&client, repo.trim())
        .await
        .map_err(hf_error_code)?;
    let file = listing
        .files
        .iter()
        .find(|f| f.path == path)
        .cloned()
        .ok_or_else(|| format!("MODEL_IMPORT_NOT_FOUND: {path}"))?;
    let root = state.models_root(&app);
    let staging = root.join(format!(".hf-{}", uuid::Uuid::now_v7().simple()));
    std::fs::create_dir_all(&staging).map_err(|e| format!("MODEL_IMPORT_FAILED: {e}"))?;
    let part = staging.join(crate::model_store::INSTALL_PART_FILE);
    let key = format!("hf:{}/{}", listing.repo, file.path);
    let total = file.size_bytes;
    let progress_app = app.clone();
    let progress_key = key.clone();
    let on_progress = move |received: u64| {
        let _ = super::ModelDownloadProgress {
            model_id: progress_key.clone(),
            downloaded_bytes: received as f64,
            total_bytes: total as f64,
            done: false,
            paused: false,
        }
        .emit(&progress_app);
    };
    let url = luna_model_fetch::hf::resolve_url(&listing.repo, &listing.revision, &file.path);
    if let Err(err) =
        luna_model_fetch::download(&client, &[url], &part, total, &file.sha256, &on_progress).await
    {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(crate::artifact_fetcher::fetch_error_code(err));
    }
    let origin = crate::model_import::ImportOrigin::HuggingFace {
        repo: listing.repo.clone(),
        revision: listing.revision.clone(),
        file: file.path.clone(),
    };
    let license = listing.license.clone();
    let import_root = root.clone();
    let source = part.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        crate::model_import::import_file(
            &source,
            origin,
            license,
            &import_root,
            &fit,
            &crate::model_import::catalog_entry_by_sha,
        )
        .map_err(|e| e.code())
    })
    .await
    .map_err(|e| format!("MODEL_IMPORT_FAILED: {e}"));
    // 导入是硬链接（同卷）或复制：暂存那一份无论成败都删掉。
    let _ = std::fs::remove_dir_all(&staging);
    let outcome = outcome??;
    let _ = super::ModelDownloadProgress {
        model_id: key,
        downloaded_bytes: total as f64,
        total_bytes: total as f64,
        done: true,
        paused: false,
    }
    .emit(&app);
    crate::local_engine::spawn_reconcile_restart(&app);
    Ok(ModelImportResultDto {
        model_id: outcome.model_id,
        name: outcome.name,
        matched_catalog: outcome.matched_catalog,
        role: (&outcome.role).into(),
    })
}
