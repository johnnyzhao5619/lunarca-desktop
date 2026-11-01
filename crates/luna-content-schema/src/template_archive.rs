//! 模板包 zip 的解包与核对（ADR-159 ⑥、ADR-161 ③）：本地导入、市场安装与服务端审核共用的那一段。
//!
//! **纯函数**：字节进、结构出，不碰文件系统、不联网。服务端上传时与客户端安装时各解一遍、各推导一遍
//! 能力（[`UnpackedTemplatePack::capabilities`]），对不上即拒装——所以两边必须是同一份代码。
//!
//! 只做与「包」本身有关的核对：文件数、解压后总大小、路径、清单在场、清单登记的每一项都在且 sha256
//! 对得上、包里没有清单没登记的文件、配方解析得出且 id 不重复。各类内容的正文（笔记模板、AI 预设、
//! 整理模板）交给它们在客户端的归属方解析；宿主兼容性（`content_api`）由安装方判——服务端不因为
//! 包要求更新的宿主就拒收。

use std::collections::BTreeMap;
use std::io::Read as _;

use sha2::{Digest as _, Sha256};
use thiserror::Error;

use crate::{
    Capabilities, PackAsset, PackAssetKind, Recipe, RelPath, TemplatePackError,
    TemplatePackManifest, MAX_PACK_BYTES, MAX_PACK_FILES,
};

/// 包根目录里的清单。
pub const TEMPLATE_PACK_MANIFEST: &str = "manifest.toml";

/// 解包或核对时的拒绝原因。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PackArchiveError {
    #[error("not a readable zip archive: {0}")]
    Zip(String),
    #[error("the pack has more than {MAX_PACK_FILES} files")]
    TooManyFiles,
    #[error("the unpacked pack is larger than {MAX_PACK_BYTES} bytes")]
    TooLarge,
    #[error("entry {0} is not a plain file at a safe relative path")]
    Entry(String),
    #[error("manifest.toml is missing")]
    NoManifest,
    #[error(transparent)]
    Pack(#[from] TemplatePackError),
    #[error("{0} is listed in the manifest but missing from the pack")]
    Missing(String),
    #[error("{0} is in the pack but not listed in the manifest")]
    Unlisted(String),
    #[error("{0} does not match the sha256 in the manifest")]
    Digest(String),
    #[error("{path}: {reason}")]
    Asset { path: String, reason: String },
}

/// 小写十六进制 sha256。
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// 核对过的模板包：清单 + 清单登记的每一份文件（字节已与清单的 sha256 对上）。
#[derive(Debug, Clone)]
pub struct UnpackedTemplatePack {
    pub manifest: TemplatePackManifest,
    files: BTreeMap<RelPath, Vec<u8>>,
}

impl UnpackedTemplatePack {
    /// 清单登记的一项的字节（解包时已确认它在）。
    #[must_use]
    pub fn asset(&self, asset: &PackAsset) -> &[u8] {
        self.files.get(&asset.path).map_or(&[], Vec::as_slice)
    }

    /// 全部文件（含 `manifest.toml`），交给安装管线落盘。
    #[must_use]
    pub fn into_files(self) -> Vec<(RelPath, Vec<u8>)> {
        self.files.into_iter().collect()
    }

    /// 包里的配方，按清单顺序（每份都校验过、id 不重复）。
    ///
    /// # Errors
    /// 配方不是 UTF-8、解析不了、引用了包里没有的提示词，或 id 重复时。
    pub fn recipes(&self) -> Result<Vec<Recipe>, PackArchiveError> {
        let recipes = self
            .manifest
            .assets_of(PackAssetKind::Recipe)
            .map(|asset| {
                let text = std::str::from_utf8(self.asset(asset)).map_err(|_| {
                    PackArchiveError::Asset {
                        path: asset.path.as_str().to_owned(),
                        reason: "not UTF-8 text".to_owned(),
                    }
                })?;
                Ok(self.manifest.parse_recipe(text)?)
            })
            .collect::<Result<Vec<_>, PackArchiveError>>()?;
        TemplatePackManifest::check_recipe_ids(&recipes)?;
        Ok(recipes)
    }

    /// 由内容推导的能力（服务端审核与客户端安装各算一遍，ADR-161 ③）。
    ///
    /// # Errors
    /// 同 [`Self::recipes`]。
    pub fn capabilities(&self) -> Result<Capabilities, PackArchiveError> {
        Ok(Capabilities::of(&self.manifest, &self.recipes()?))
    }
}

/// 把一个 zip 解成「包内路径 → 字节」。文件数、解压后总大小、路径都在读的时候卡住：zip 头里写的
/// 大小不可信，按真读到的字节数裁决。
fn unzip(bytes: &[u8]) -> Result<BTreeMap<RelPath, Vec<u8>>, PackArchiveError> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|error| PackArchiveError::Zip(error.to_string()))?;
    if archive.len() > MAX_PACK_FILES * 2 {
        return Err(PackArchiveError::TooManyFiles);
    }
    let mut files = BTreeMap::new();
    let mut total: u64 = 0;
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|error| PackArchiveError::Zip(error.to_string()))?;
        if entry.is_dir() {
            continue;
        }
        let name = String::from_utf8_lossy(entry.name_raw()).into_owned();
        if entry.is_symlink() || entry.encrypted() {
            return Err(PackArchiveError::Entry(name));
        }
        let path = RelPath::parse(&name).map_err(|_| PackArchiveError::Entry(name.clone()))?;
        let mut content = Vec::new();
        entry
            .take(MAX_PACK_BYTES - total + 1)
            .read_to_end(&mut content)
            .map_err(|error| PackArchiveError::Zip(error.to_string()))?;
        total += content.len() as u64;
        if total > MAX_PACK_BYTES {
            return Err(PackArchiveError::TooLarge);
        }
        if files.insert(path, content).is_some() {
            return Err(PackArchiveError::Entry(name));
        }
        if files.len() > MAX_PACK_FILES {
            return Err(PackArchiveError::TooManyFiles);
        }
    }
    Ok(files)
}

/// 解包并核对一个模板包。包里不许有清单没登记的文件，登记的每一项都要在、都要对得上 sha256。
/// 不判宿主兼容性（见模块说明），也不解析配方以外的正文。
///
/// # Errors
/// 见 [`PackArchiveError`]。
pub fn unpack_template_pack(bytes: &[u8]) -> Result<UnpackedTemplatePack, PackArchiveError> {
    if bytes.len() as u64 > MAX_PACK_BYTES {
        return Err(PackArchiveError::TooLarge);
    }
    let files = unzip(bytes)?;
    let manifest_source = files
        .iter()
        .find(|(path, _)| path.as_str() == TEMPLATE_PACK_MANIFEST)
        .map(|(_, bytes)| bytes)
        .ok_or(PackArchiveError::NoManifest)?;
    let manifest = TemplatePackManifest::parse(
        std::str::from_utf8(manifest_source)
            .map_err(|_| TemplatePackError::Parse("manifest.toml must be UTF-8".to_owned()))?,
    )?;
    if let Some(unlisted) = files.keys().find(|path| {
        path.as_str() != TEMPLATE_PACK_MANIFEST
            && !manifest.assets.iter().any(|asset| asset.path == **path)
    }) {
        return Err(PackArchiveError::Unlisted(unlisted.as_str().to_owned()));
    }
    for asset in &manifest.assets {
        let bytes = files
            .get(&asset.path)
            .ok_or_else(|| PackArchiveError::Missing(asset.path.as_str().to_owned()))?;
        if sha256_hex(bytes) != asset.sha256 {
            return Err(PackArchiveError::Digest(asset.path.as_str().to_owned()));
        }
    }
    Ok(UnpackedTemplatePack { manifest, files })
}

#[cfg(test)]
pub(crate) mod tests {
    use std::io::Write as _;

    use super::*;

    /// 把（路径，内容）打成 zip（测试用；别的模块的测试也拿它造包）。
    pub(crate) fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(&mut out);
        for (name, bytes) in files {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
        out.into_inner()
    }

    pub(crate) const RECIPE: &str = r#"
id = "com.example.studio.summary"
title = "Summarize and file"
steps = [
  { type = "llm", prompt = "prompts/summary.md" },
  { type = "vault_create_note", dir = "Summaries" },
]
"#;

    /// 一个合格的包：一份笔记模板、一份提示词、一份配方。
    pub(crate) fn good_pack() -> Vec<u8> {
        let template = b"# {{title}}\n";
        let prompt = b"Summarize the note.\n";
        let manifest = format!(
            r#"schema_version = 1
id = "com.example.studio"
version = "1.2.0"
content_api = "1"
name = "Studio pack"
publisher = "Example Studio"
license = "CC-BY-4.0"
locales = ["en"]

[[assets]]
path = "templates/brief.md"
kind = "note_template"
sha256 = "{}"

[[assets]]
path = "prompts/summary.md"
kind = "prompt"
sha256 = "{}"

[[assets]]
path = "recipes/summary.toml"
kind = "recipe"
sha256 = "{}"
"#,
            sha256_hex(template),
            sha256_hex(prompt),
            sha256_hex(RECIPE.as_bytes()),
        );
        zip_of(&[
            ("manifest.toml", manifest.as_bytes()),
            ("templates/brief.md", template),
            ("prompts/summary.md", prompt),
            ("recipes/summary.toml", RECIPE.as_bytes()),
        ])
    }

    #[test]
    fn a_good_pack_unpacks_and_derives_its_capabilities_from_the_recipes() {
        let pack = unpack_template_pack(&good_pack()).unwrap();
        assert_eq!(pack.manifest.id.as_str(), "com.example.studio");
        assert_eq!(pack.recipes().unwrap().len(), 1);
        let capabilities = pack.capabilities().unwrap();
        assert!(capabilities.uses_ai);
        assert!(!capabilities.transcribes);
        assert_eq!(
            capabilities
                .creates_notes_in
                .into_iter()
                .collect::<Vec<_>>(),
            vec!["Summaries".to_owned()]
        );
        assert_eq!(pack.into_files().len(), 4);
    }

    #[test]
    fn unlisted_missing_and_tampered_files_are_refused() {
        let good = good_pack();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(good)).unwrap();
        let mut files: Vec<(String, Vec<u8>)> = (0..archive.len())
            .map(|index| {
                let mut entry = archive.by_index(index).unwrap();
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes).unwrap();
                (
                    String::from_utf8_lossy(entry.name_raw()).into_owned(),
                    bytes,
                )
            })
            .collect();
        let as_zip = |files: &[(String, Vec<u8>)]| {
            zip_of(
                &files
                    .iter()
                    .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
                    .collect::<Vec<_>>(),
            )
        };

        let mut extra = files.clone();
        extra.push(("notes.txt".to_owned(), b"hi".to_vec()));
        assert!(matches!(
            unpack_template_pack(&as_zip(&extra)),
            Err(PackArchiveError::Unlisted(path)) if path == "notes.txt"
        ));

        let missing: Vec<_> = files
            .iter()
            .filter(|(name, _)| name != "prompts/summary.md")
            .cloned()
            .collect();
        assert!(matches!(
            unpack_template_pack(&as_zip(&missing)),
            Err(PackArchiveError::Missing(path)) if path == "prompts/summary.md"
        ));

        for (name, bytes) in &mut files {
            if name == "templates/brief.md" {
                bytes.push(b'!');
            }
        }
        assert!(matches!(
            unpack_template_pack(&as_zip(&files)),
            Err(PackArchiveError::Digest(path)) if path == "templates/brief.md"
        ));
    }

    #[test]
    fn not_a_zip_no_manifest_and_unsafe_paths_are_refused() {
        assert!(matches!(
            unpack_template_pack(b"plain text"),
            Err(PackArchiveError::Zip(_))
        ));
        assert_eq!(
            unpack_template_pack(&zip_of(&[("readme.md", b"x")])).unwrap_err(),
            PackArchiveError::NoManifest
        );
        assert!(matches!(
            unpack_template_pack(&zip_of(&[("../escape.md", b"x")])),
            Err(PackArchiveError::Entry(_))
        ));
        assert_eq!(
            unpack_template_pack(&vec![0; (MAX_PACK_BYTES + 1) as usize]).unwrap_err(),
            PackArchiveError::TooLarge
        );
    }
}
