//! Hugging Face 仓库的 GGUF 文件清单（ADR-144 ③：高级模式「按 HF 仓库 id 添加」）。
//!
//! 只认仓库 id，**不接受任意 URL**：先问仓库信息拿到当前提交号与许可，再按提交号列文件——
//! 之后的下载钉在这个提交上，仓库作者后来改了文件也不会悄悄换掉用户要的那一份。

use serde::Deserialize;

/// 仓库里的一个 GGUF 文件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HfFile {
    /// 仓库内路径。
    pub path: String,
    /// 字节数。
    pub size_bytes: u64,
    /// sha256（LFS oid）。
    pub sha256: String,
}

/// 一个仓库在某个提交上的 GGUF 清单。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HfRepo {
    /// `owner/name`。
    pub repo: String,
    /// 提交号（40 位十六进制）。
    pub revision: String,
    /// 模型卡里登记的许可（`cardData.license`）。
    pub license: Option<String>,
    /// GGUF 文件（不含视觉投影 `mmproj`），按路径排序。
    pub files: Vec<HfFile>,
}

/// 取清单失败。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HfError {
    /// 仓库 id 不合法。
    InvalidRepo,
    /// 仓库不存在或不公开（HF 对不存在的仓库也回 401）。
    NotFound,
    /// 需要登录或申请授权的受限仓库。
    Gated,
    /// 网络 / HTTP / 响应格式问题。
    Network(String),
}

/// `owner/name`，两段都是 `[A-Za-z0-9._-]`、不是 `..`。
#[must_use]
pub fn valid_repo_id(repo: &str) -> bool {
    let mut parts = repo.split('/');
    let ok = |p: Option<&str>| {
        p.is_some_and(|p| {
            !p.is_empty()
                && p != "."
                && p != ".."
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        })
    };
    ok(parts.next()) && ok(parts.next()) && parts.next().is_none()
}

/// 文件下载地址（钉到提交）。
#[must_use]
pub fn resolve_url(repo: &str, revision: &str, path: &str) -> String {
    format!("https://huggingface.co/{repo}/resolve/{revision}/{path}")
}

#[derive(Deserialize)]
struct RepoInfo {
    sha: Option<String>,
    #[serde(default)]
    private: bool,
    #[serde(default)]
    gated: serde_json::Value,
    #[serde(default, rename = "cardData")]
    card: Option<CardData>,
}

#[derive(Deserialize)]
struct CardData {
    license: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct TreeEntry {
    #[serde(rename = "type")]
    kind: String,
    path: String,
    lfs: Option<Lfs>,
}

#[derive(Deserialize)]
struct Lfs {
    oid: String,
    size: u64,
}

/// 解析仓库信息 → (提交号, 许可)。受限 / 私有即拒。
///
/// # Errors
/// 见 [`HfError`]。
pub fn parse_repo_info(json: &str) -> Result<(String, Option<String>), HfError> {
    let info: RepoInfo = serde_json::from_str(json).map_err(|e| HfError::Network(e.to_string()))?;
    if info.private {
        return Err(HfError::NotFound);
    }
    if !matches!(
        info.gated,
        serde_json::Value::Bool(false) | serde_json::Value::Null
    ) {
        return Err(HfError::Gated);
    }
    let revision = info
        .sha
        .filter(|s| s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit()))
        .ok_or_else(|| HfError::Network("repository info has no commit".to_owned()))?;
    let license = info.card.and_then(|c| c.license).and_then(|l| match l {
        serde_json::Value::String(s) => Some(s),
        serde_json::Value::Array(items) => {
            items.first().and_then(|v| v.as_str()).map(str::to_owned)
        }
        _ => None,
    });
    Ok((revision, license))
}

/// 解析文件树 → GGUF 文件（跳过 `mmproj` 视觉投影与非 LFS 的小文件）。
///
/// # Errors
/// 响应格式不对。
pub fn parse_tree(json: &str) -> Result<Vec<HfFile>, HfError> {
    let entries: Vec<TreeEntry> =
        serde_json::from_str(json).map_err(|e| HfError::Network(e.to_string()))?;
    let mut files: Vec<HfFile> = entries
        .into_iter()
        .filter(|e| e.kind == "file")
        .filter(|e| e.path.to_ascii_lowercase().ends_with(".gguf"))
        .filter(|e| !e.path.to_ascii_lowercase().contains("mmproj"))
        .filter_map(|e| {
            let lfs = e.lfs?;
            (lfs.oid.len() == 64).then_some(HfFile {
                path: e.path,
                size_bytes: lfs.size,
                sha256: lfs.oid,
            })
        })
        .collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// 取一个仓库当前提交上的 GGUF 清单。
///
/// # Errors
/// 见 [`HfError`]。
pub async fn repo_ggufs(client: &reqwest::Client, repo: &str) -> Result<HfRepo, HfError> {
    if !valid_repo_id(repo) {
        return Err(HfError::InvalidRepo);
    }
    let get = |url: String| async move {
        let response = client
            .get(&url)
            .send()
            .await
            .map_err(|e| HfError::Network(e.to_string()))?;
        match response.status().as_u16() {
            200 => response
                .text()
                .await
                .map_err(|e| HfError::Network(e.to_string())),
            401 | 404 => Err(HfError::NotFound),
            403 => Err(HfError::Gated),
            code => Err(HfError::Network(format!("HTTP {code}"))),
        }
    };
    let (revision, license) =
        parse_repo_info(&get(format!("https://huggingface.co/api/models/{repo}")).await?)?;
    let files = parse_tree(
        &get(format!(
            "https://huggingface.co/api/models/{repo}/tree/{revision}?recursive=true"
        ))
        .await?,
    )?;
    Ok(HfRepo {
        repo: repo.to_owned(),
        revision,
        license,
        files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_ids_are_owner_slash_name_only() {
        assert!(valid_repo_id("unsloth/Qwen3.5-0.8B-GGUF"));
        for bad in [
            "",
            "noslash",
            "a/b/c",
            "../x",
            "a/..",
            "a/b?c",
            "https://huggingface.co/a/b",
            "a /b",
        ] {
            assert!(!valid_repo_id(bad), "{bad}");
        }
    }

    #[test]
    fn repo_info_pins_the_commit_and_refuses_gated_or_private() {
        let ok = r#"{"sha":"6ab461498e2023f6e3c1baea90a8f0fe38ab64d0","private":false,"gated":false,"cardData":{"license":"apache-2.0"}}"#;
        assert_eq!(
            parse_repo_info(ok),
            Ok((
                "6ab461498e2023f6e3c1baea90a8f0fe38ab64d0".into(),
                Some("apache-2.0".into())
            ))
        );
        let gated = r#"{"sha":"6ab461498e2023f6e3c1baea90a8f0fe38ab64d0","gated":"manual"}"#;
        assert_eq!(parse_repo_info(gated), Err(HfError::Gated));
        let private = r#"{"sha":"6ab461498e2023f6e3c1baea90a8f0fe38ab64d0","private":true}"#;
        assert_eq!(parse_repo_info(private), Err(HfError::NotFound));
    }

    #[test]
    fn tree_keeps_lfs_gguf_files_and_drops_projectors() {
        let tree = r#"[
          {"type":"file","path":"README.md"},
          {"type":"file","path":"Model-Q8_0.gguf","lfs":{"oid":"0ad885ffd4bb022fab5ba2e0f2e6d6d0b1c0b3c5c5e0c9e0a7b1c2d3e4f5a6b7","size":811843840}},
          {"type":"file","path":"mmproj-F16.gguf","lfs":{"oid":"1111111111111111111111111111111111111111111111111111111111111111","size":2}},
          {"type":"directory","path":"sub"},
          {"type":"file","path":"sub/Big.GGUF","lfs":{"oid":"2222222222222222222222222222222222222222222222222222222222222222","size":9}}
        ]"#;
        let files = parse_tree(tree).unwrap();
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["Model-Q8_0.gguf", "sub/Big.GGUF"]);
    }

    /// 实网：`cargo test -p luna-model-fetch live_hf_listing -- --ignored`
    #[tokio::test]
    #[ignore = "联网"]
    async fn live_hf_listing() {
        let repo = repo_ggufs(
            &crate::catalog_client().unwrap(),
            "unsloth/Qwen3.5-0.8B-GGUF",
        )
        .await
        .unwrap();
        assert_eq!(repo.revision.len(), 40);
        assert_eq!(repo.license.as_deref(), Some("apache-2.0"));
        assert!(repo
            .files
            .iter()
            .any(|f| f.path == "Qwen3.5-0.8B-Q8_0.gguf"));
        assert!(repo.files.iter().all(|f| !f.path.contains("mmproj")));
        let missing =
            repo_ggufs(&crate::catalog_client().unwrap(), "nobody-here/nothing-xyz").await;
        assert_eq!(missing, Err(HfError::NotFound));
    }
}
