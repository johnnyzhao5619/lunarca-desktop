//! 导入 GGUF 模型（ADR-144 ③）：本地文件、Hugging Face 仓库 id 两条路共用一道闸。
//!
//! 闸：读 GGUF 文件头 → 推断角色（有池化方式 = 嵌入；有对话模板 = 对话；都没有就拒收）→ 用随包
//! `llama-fit-params` 试读一遍（不分配；引擎不认的架构、截断的文件都在这里被挡下）→ 记 sha256。
//!
//! 导入的模型是**用户条目**，与目录并列、不覆盖目录：落在 `<模型根>/gguf-<sha256 前 12 位>/`，
//! 里面是 `weights.gguf` 与一份标明 `kind = "imported"` 的 manifest。唯一的例外：sha256 恰好等于
//! 某个目录条目——那就是那个目录条目，按它安装（硬链接优先），不另起一条用户条目（ADR-144 ②）。

use std::path::{Path, PathBuf};

use luna_model_catalog::gguf::{self, GgufHeader};
use luna_model_catalog::{GgufRole, ModelEntry, Pooling};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// 导入条目 id 前缀。
pub const IMPORTED_PREFIX: &str = "gguf-";
/// manifest 里标明「这是导入的」的 kind 值。
const IMPORTED_KIND: &str = "imported";

/// 导入的来源。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ImportOrigin {
    /// 用户选的本地文件（只记文件名，不记完整路径——路径里常有用户名）。
    LocalFile {
        /// 原文件名。
        file_name: String,
    },
    /// Hugging Face 仓库里的一个文件（钉到提交）。
    HuggingFace {
        /// `owner/name`。
        repo: String,
        /// 提交号。
        revision: String,
        /// 仓库内文件。
        file: String,
    },
}

/// 导入模型的角色（由文件头推断）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ImportedRole {
    /// 对话。
    Chat,
    /// 嵌入。
    Embed {
        /// 池化。
        pooling: Pooling,
        /// 编码器结构（BERT 类）。
        encoder: bool,
    },
}

/// 导入条目的 manifest（`<id>/manifest.json`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportedManifest {
    /// 恒为 `imported`。
    pub kind: String,
    /// `gguf-<sha256 前 12 位>`。
    pub id: String,
    /// 展示名（文件头的 `general.name`，没有就取文件名）。
    pub name: String,
    /// 来源。
    pub origin: ImportOrigin,
    /// 权重 sha256。
    pub sha256: String,
    /// 权重字节数。
    pub size_bytes: u64,
    /// 架构（`general.architecture`）。
    pub architecture: String,
    /// 训练上下文。
    pub context_length: u64,
    /// 角色。
    pub role: ImportedRole,
    /// 许可（HF 仓库卡片里读到的；本地文件无从得知，为空）。
    pub license: Option<String>,
}

impl ImportedManifest {
    /// 给引擎编排用的目录运行参数（导入条目没有目录条目，按文件头推断）。
    #[must_use]
    pub fn gguf_role(&self) -> GgufRole {
        match self.role {
            // 导入的对话模型一律按「可能会思考」处理：`reasoning = off` 对不思考的模型无副作用，
            // 漏关则短请求预算全耗在思考上（P0-1）。
            ImportedRole::Chat => GgufRole::Chat { thinking: true },
            ImportedRole::Embed { pooling, encoder } => GgufRole::Embed { pooling, encoder },
        }
    }
}

/// 导入失败（错误码前缀与前端文案一一对应；明细英文）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportError {
    /// 不是 GGUF / 文件头读不了。
    NotGguf(String),
    /// 既不是对话模型也不是嵌入模型。
    UnsupportedRole,
    /// 随包引擎读不了它（不认识的架构、文件截断……）。
    EngineRejected(String),
    /// 已经导入过同一个文件。
    Duplicate(String),
    /// 读写失败。
    Io(String),
}

impl ImportError {
    /// `CODE: detail`，交给前端按码出文案。
    #[must_use]
    pub fn code(&self) -> String {
        match self {
            Self::NotGguf(d) => format!("MODEL_IMPORT_NOT_GGUF: {d}"),
            Self::UnsupportedRole => {
                "MODEL_IMPORT_UNSUPPORTED_ROLE: no chat template and no pooling".to_owned()
            }
            Self::EngineRejected(d) => format!("MODEL_IMPORT_ENGINE_REJECTED: {d}"),
            Self::Duplicate(id) => format!("MODEL_IMPORT_DUPLICATE: {id}"),
            Self::Io(d) => format!("MODEL_IMPORT_FAILED: {d}"),
        }
    }
}

/// 一次导入的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportOutcome {
    /// 落成的条目 id（导入条目或匹配上的目录条目）。
    pub model_id: String,
    /// 展示名。
    pub name: String,
    /// sha256 匹配上了目录条目（按目录条目安装）。
    pub matched_catalog: bool,
    /// 导入闸从文件头读出的角色（对话 / 嵌入）。界面在导入**之后**说出"识别为什么"，
    /// 而不是导入之前先念一段拒收规则（2026-10-08 C41）。
    pub role: ImportedRole,
}

/// 文件头 → 角色。纯函数，可测。
///
/// # Errors
/// 既没有池化也没有对话模板时 [`ImportError::UnsupportedRole`]。
pub fn infer_role(header: &GgufHeader) -> Result<ImportedRole, ImportError> {
    use gguf::Pooling as H;
    let pooling = match header.pooling {
        Some(H::Cls) => Some(Pooling::Cls),
        Some(H::Last) => Some(Pooling::Last),
        Some(H::Mean) => Some(Pooling::Mean),
        Some(H::None | H::Rank) | None => None,
    };
    match pooling {
        Some(pooling) => Ok(ImportedRole::Embed {
            pooling,
            // 编码器结构的嵌入模型（BERT 一族）必须整段进一个 ubatch；按架构名认。
            encoder: header
                .architecture
                .as_deref()
                .is_some_and(|a| a.contains("bert")),
        }),
        None if header.has_chat_template => Ok(ImportedRole::Chat),
        None => Err(ImportError::UnsupportedRole),
    }
}

/// 流式算 sha256（不把文件读进内存）。
///
/// # Errors
/// 读失败。
pub fn sha256_file(path: &Path) -> std::io::Result<(String, u64)> {
    use std::io::Read as _;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut total = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        total += n as u64;
    }
    Ok((format!("{:x}", hasher.finalize()), total))
}

/// 引擎试读：随包 `llama-fit-params` 能给出内存分解 = 引擎认识这个架构、文件完整。
fn engine_probe(fit_params: &Path, weights: &Path) -> Result<(), ImportError> {
    let out = std::process::Command::new(fit_params)
        .arg("-m")
        .arg(weights)
        .args(["-c", "2048", "-lv", "4"])
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| ImportError::Io(format!("spawn llama-fit-params: {e}")))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    if luna_engine::llama::budget::parse_breakdown(&text).is_some() {
        return Ok(());
    }
    let reason = text
        .lines()
        .rev()
        .find(|l| l.contains("error") || l.contains("unknown") || l.contains("failed"))
        .map_or_else(
            || "engine could not read the file".to_owned(),
            |l| l.trim().chars().take(200).collect(),
        );
    Err(ImportError::EngineRejected(reason))
}

/// 把 `src` 放到 `dest`：同卷硬链接（瞬时、不占第二份空间），否则复制。
fn link_or_copy(src: &Path, dest: &Path) -> std::io::Result<()> {
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if std::fs::hard_link(src, dest).is_ok() {
        return Ok(());
    }
    std::fs::copy(src, dest).map(|_| ())
}

/// 读一个导入条目的 manifest（不是导入条目返回 `None`）。
#[must_use]
pub fn read_manifest(dir: &Path) -> Option<ImportedManifest> {
    let manifest: ImportedManifest = serde_json::from_slice(
        &std::fs::read(dir.join(crate::model_store::INSTALL_DIR_MANIFEST_FILE)).ok()?,
    )
    .ok()?;
    (manifest.kind == IMPORTED_KIND
        && dir
            .join(crate::model_store::LUNA_MANAGED_GGUF_FILE)
            .is_file())
    .then_some(manifest)
}

/// 模型根下全部导入条目（按 id 排序）。
#[must_use]
pub fn imported(models_root: &Path) -> Vec<ImportedManifest> {
    let Ok(dirs) = std::fs::read_dir(models_root) else {
        return Vec::new();
    };
    let mut found: Vec<ImportedManifest> = dirs
        .flatten()
        .filter(|d| d.file_name().to_string_lossy().starts_with(IMPORTED_PREFIX))
        .filter_map(|d| read_manifest(&d.path()))
        .collect();
    found.sort_by(|a, b| a.id.cmp(&b.id));
    found
}

/// 导入一个本机已有的 GGUF 文件（本地文件直接走这里；HF 下载完的文件也走这里）。
///
/// `catalog` 是本客户端可见的目录条目查找（按 sha256）；匹配上就按目录条目装。
///
/// # Errors
/// 见 [`ImportError`]。
pub fn import_file(
    source: &Path,
    origin: ImportOrigin,
    license: Option<String>,
    models_root: &Path,
    fit_params: &Path,
    catalog_by_sha: &dyn Fn(&str) -> Option<&'static ModelEntry>,
) -> Result<ImportOutcome, ImportError> {
    let header = {
        let file = std::fs::File::open(source).map_err(|e| ImportError::Io(e.to_string()))?;
        gguf::read_header(&mut std::io::BufReader::new(file))
            .map_err(|e| ImportError::NotGguf(e.to_string()))?
    };
    let role = infer_role(&header)?;
    let context_length = header
        .context_length
        .ok_or_else(|| ImportError::NotGguf("missing context length".to_owned()))?;
    let architecture = header
        .architecture
        .clone()
        .ok_or_else(|| ImportError::NotGguf("missing architecture".to_owned()))?;
    engine_probe(fit_params, source)?;
    let (sha256, size_bytes) = sha256_file(source).map_err(|e| ImportError::Io(e.to_string()))?;

    // 与目录条目同一个文件：就是那个目录条目（ADR-144 ②）。
    if let Some(entry) = catalog_by_sha(&sha256).filter(|e| e.gguf.is_some()) {
        let final_dir = models_root.join(&entry.id);
        if crate::model_store::spec_weight_path(models_root, entry).is_file() {
            return Err(ImportError::Duplicate(entry.id.clone()));
        }
        let staging = models_root.join(format!(".import-{}", uuid::Uuid::now_v7().simple()));
        let result = (|| {
            link_or_copy(
                source,
                &staging.join(crate::model_store::weight_file_name(entry)),
            )?;
            let manifest = serde_json::json!({
                "id": entry.id,
                "name": entry.name,
                "source_url": entry.weights.primary_url(),
                "sha256": entry.weights.sha256,
                "license": entry.license.label,
                "license_upstream": entry.license.upstream,
                "quantized_by": entry.license.quantized_by,
                "size_bytes": entry.weights.size_bytes,
            });
            std::fs::write(
                staging.join(crate::model_store::INSTALL_DIR_MANIFEST_FILE),
                serde_json::to_vec_pretty(&manifest).unwrap_or_default(),
            )?;
            std::fs::rename(&staging, &final_dir)
        })();
        if let Err(e) = result {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(ImportError::Io(e.to_string()));
        }
        return Ok(ImportOutcome {
            model_id: entry.id.clone(),
            name: entry.name.clone(),
            matched_catalog: true,
            role,
        });
    }

    let id = format!("{IMPORTED_PREFIX}{}", &sha256[..12]);
    let final_dir = models_root.join(&id);
    if final_dir.exists() {
        return Err(ImportError::Duplicate(id));
    }
    let name = display_name(header.name.as_deref(), source).unwrap_or_else(|| id.clone());
    let manifest = ImportedManifest {
        kind: IMPORTED_KIND.to_owned(),
        id: id.clone(),
        name: name.clone(),
        origin,
        sha256,
        size_bytes,
        architecture,
        context_length,
        role,
        license,
    };
    let staging = models_root.join(format!(".import-{}", uuid::Uuid::now_v7().simple()));
    let result = (|| {
        link_or_copy(
            source,
            &staging.join(crate::model_store::LUNA_MANAGED_GGUF_FILE),
        )?;
        std::fs::write(
            staging.join(crate::model_store::INSTALL_DIR_MANIFEST_FILE),
            serde_json::to_vec_pretty(&manifest).unwrap_or_default(),
        )?;
        std::fs::rename(&staging, &final_dir)
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(ImportError::Io(e.to_string()));
    }
    Ok(ImportOutcome {
        model_id: id,
        name,
        matched_catalog: false,
        role,
    })
}

/// 这个 sha256 是否已经装在本机（作为目录条目或导入条目）。
#[must_use]
pub fn already_installed(models_root: &Path, sha256: &str) -> bool {
    if let Some(entry) = catalog_entry_by_sha(sha256) {
        if crate::model_store::spec_weight_path(models_root, entry).is_file() {
            return true;
        }
    }
    sha256.len() >= 12
        && models_root
            .join(format!("{IMPORTED_PREFIX}{}", &sha256[..12]))
            .exists()
}

/// 导入条目的展示名：取文件头里的名字，没有再退回文件名。
fn display_name(header_name: Option<&str>, source: &Path) -> Option<String> {
    header_name
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_owned)
        .or_else(|| source.file_stem().map(|s| s.to_string_lossy().into_owned()))
}

/// 找一个 sha256 对应的可见目录条目（导入时用来判断「这其实是目录里的那个」）。
#[must_use]
pub fn catalog_entry_by_sha(sha256: &str) -> Option<&'static ModelEntry> {
    luna_model_catalog::discoverable()
        .into_iter()
        .chain(luna_model_catalog::bundled().iter())
        .filter(|e| e.weights.sha256 == sha256)
        .find_map(|e| crate::model_store::remote_model_spec(&e.id))
}

/// 导入条目的权重路径。
#[must_use]
pub fn weights_path(models_root: &Path, id: &str) -> PathBuf {
    models_root
        .join(id)
        .join(crate::model_store::LUNA_MANAGED_GGUF_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(arch: &str, pooling: Option<gguf::Pooling>, template: bool) -> GgufHeader {
        GgufHeader {
            version: 3,
            tensor_count: 1,
            architecture: Some(arch.into()),
            name: None,
            context_length: Some(8192),
            block_count: Some(1),
            pooling,
            file_type: None,
            has_chat_template: template,
        }
    }

    #[test]
    fn role_follows_pooling_then_template() {
        assert_eq!(
            infer_role(&header("bert", Some(gguf::Pooling::Cls), false)),
            Ok(ImportedRole::Embed {
                pooling: Pooling::Cls,
                encoder: true
            })
        );
        // Qwen3-Embedding：有模板也有 LAST 池化 → 嵌入，解码器结构
        assert_eq!(
            infer_role(&header("qwen3", Some(gguf::Pooling::Last), true)),
            Ok(ImportedRole::Embed {
                pooling: Pooling::Last,
                encoder: false
            })
        );
        assert_eq!(
            infer_role(&header("qwen35", None, true)),
            Ok(ImportedRole::Chat)
        );
        assert_eq!(
            infer_role(&header("qwen35", Some(gguf::Pooling::None), true)),
            Ok(ImportedRole::Chat)
        );
        assert_eq!(
            infer_role(&header("llama", None, false)),
            Err(ImportError::UnsupportedRole)
        );
    }

    #[test]
    fn display_name_prefers_the_header_then_the_file_stem() {
        assert_eq!(
            display_name(Some("Qwen3.5-4B"), Path::new("/a/m.gguf")).as_deref(),
            Some("Qwen3.5-4B")
        );
        assert_eq!(
            display_name(Some("  "), Path::new("/a/m.gguf")).as_deref(),
            Some("m")
        );
    }

    #[test]
    fn non_gguf_is_rejected_before_anything_is_written() {
        let temp = tempfile::tempdir().unwrap();
        let src = temp.path().join("notes.gguf");
        std::fs::write(&src, b"definitely not a gguf").unwrap();
        let root = temp.path().join("models");
        std::fs::create_dir_all(&root).unwrap();
        let err = import_file(
            &src,
            ImportOrigin::LocalFile {
                file_name: "notes.gguf".into(),
            },
            None,
            &root,
            Path::new("/nonexistent/llama-fit-params"),
            &|_| None,
        )
        .unwrap_err();
        assert!(matches!(err, ImportError::NotGguf(_)), "{err:?}");
        assert_eq!(
            std::fs::read_dir(&root).unwrap().count(),
            0,
            "nothing staged"
        );
    }

    #[test]
    fn manifests_are_recognised_only_with_the_imported_kind_and_weights() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("gguf-abc");
        std::fs::create_dir_all(&dir).unwrap();
        let manifest = ImportedManifest {
            kind: IMPORTED_KIND.into(),
            id: "gguf-abc".into(),
            name: "x".into(),
            origin: ImportOrigin::LocalFile {
                file_name: "bge-m3.gguf".into(),
            },
            sha256: "0".repeat(64),
            size_bytes: 1,
            architecture: "bert".into(),
            context_length: 8192,
            role: ImportedRole::Embed {
                pooling: Pooling::Cls,
                encoder: true,
            },
            license: None,
        };
        std::fs::write(
            dir.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        assert!(read_manifest(&dir).is_none(), "no weights yet");
        std::fs::write(dir.join("weights.gguf"), b"w").unwrap();
        assert_eq!(read_manifest(&dir), Some(manifest.clone()));
        assert_eq!(imported(temp.path()).len(), 1);
        assert_eq!(
            manifest.gguf_role(),
            GgufRole::Embed {
                pooling: Pooling::Cls,
                encoder: true
            }
        );
    }

    /// HF 一路的真实调用（界面胶水之外的全部）：列清单钉提交 → 下载边下边校验 → 导入闸 →
    /// 用户条目（目录里没有 0.8B 的 Q4_K_M）。约 0.5 GB。
    /// `LUNA_LLAMA_BIN_DIR=… cargo test -p luna-app --lib hf_repo_file_imports -- --ignored`
    #[tokio::test]
    #[ignore = "联网下载约 0.5 GB，需要随包 llama-fit-params"]
    async fn hf_repo_file_imports_as_a_pinned_user_entry() {
        let fit =
            PathBuf::from(std::env::var("LUNA_LLAMA_BIN_DIR").unwrap()).join("llama-fit-params");
        let client = luna_model_fetch::catalog_client().unwrap();
        let listing = luna_model_fetch::hf::repo_ggufs(&client, "unsloth/Qwen3.5-0.8B-GGUF")
            .await
            .unwrap();
        let file = listing
            .files
            .iter()
            .find(|f| f.path == "Qwen3.5-0.8B-Q4_K_M.gguf")
            .unwrap()
            .clone();
        let temp = tempfile::tempdir().unwrap();
        let part = temp.path().join("dl.part");
        let url = luna_model_fetch::hf::resolve_url(&listing.repo, &listing.revision, &file.path);
        luna_model_fetch::download(
            &client,
            &[url],
            &part,
            file.size_bytes,
            &file.sha256,
            &|_| {},
        )
        .await
        .unwrap();
        let root = temp.path().join("models");
        std::fs::create_dir_all(&root).unwrap();
        let out = import_file(
            &part,
            ImportOrigin::HuggingFace {
                repo: listing.repo.clone(),
                revision: listing.revision.clone(),
                file: file.path.clone(),
            },
            listing.license.clone(),
            &root,
            &fit,
            &|sha| {
                luna_model_catalog::bundled()
                    .iter()
                    .find(|e| e.weights.sha256 == sha)
            },
        )
        .unwrap();
        assert!(!out.matched_catalog);
        let manifest = read_manifest(&root.join(&out.model_id)).unwrap();
        assert_eq!(manifest.role, ImportedRole::Chat);
        assert_eq!(manifest.license.as_deref(), Some("apache-2.0"));
        assert_eq!(manifest.sha256, file.sha256);
        assert!(
            matches!(manifest.origin, ImportOrigin::HuggingFace { ref revision, .. } if revision.len() == 40)
        );
    }

    /// 真文件全流程（P0 那批）：`LUNA_GGUF_DIR=~/Dev/lunarca-p0/models LUNA_LLAMA_BIN_DIR=… cargo test … -- --ignored`
    #[test]
    #[ignore = "需要本机 GGUF 与随包 llama-fit-params"]
    fn imports_real_files_and_matches_catalog_by_sha() {
        let dir = PathBuf::from(std::env::var("LUNA_GGUF_DIR").unwrap());
        let fit =
            PathBuf::from(std::env::var("LUNA_LLAMA_BIN_DIR").unwrap()).join("llama-fit-params");
        let temp = tempfile::tempdir().unwrap();
        // 目录里有的文件（sha256 相同）→ 按目录条目装
        let out = import_file(
            &dir.join("bge-m3-q8_0.gguf"),
            ImportOrigin::LocalFile {
                file_name: "bge-m3-q8_0.gguf".into(),
            },
            None,
            temp.path(),
            &fit,
            &|sha| {
                luna_model_catalog::bundled()
                    .iter()
                    .find(|e| e.weights.sha256 == sha)
            },
        )
        .unwrap();
        assert!(out.matched_catalog);
        assert_eq!(out.model_id, "bge-m3-q8_0-gguf");
        // 目录里没有的文件 → 用户条目
        let out = import_file(
            &dir.join("bge-m3-f16.gguf"),
            ImportOrigin::LocalFile {
                file_name: "bge-m3-f16.gguf".into(),
            },
            None,
            temp.path(),
            &fit,
            &|_| None,
        )
        .unwrap();
        assert!(!out.matched_catalog);
        let manifest = read_manifest(&temp.path().join(&out.model_id)).unwrap();
        assert_eq!(
            manifest.role,
            ImportedRole::Embed {
                pooling: Pooling::Cls,
                encoder: true
            }
        );
        // 再导一次 → 重复
        let again = import_file(
            &dir.join("bge-m3-f16.gguf"),
            ImportOrigin::LocalFile {
                file_name: "bge-m3-f16.gguf".into(),
            },
            None,
            temp.path(),
            &fit,
            &|_| None,
        );
        assert!(matches!(again, Err(ImportError::Duplicate(_))));
    }
}
