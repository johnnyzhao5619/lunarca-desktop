//! 安装管线（ADR-159 ⑥ / ADR-169 ②）：本地导入与市场安装是同一条路。
//!
//! 来源 → 大小上限 → sha256（市场来源）→ 按域解析与校验（调用方给）→ 内容层原子落位。任一步失败
//! 内容层都保持原样。基座不解析内容：`prepare` 把字节变成「id、版本、要落盘的文件」，主题包与
//! 模板包各写各的。

use std::io::Read as _;
use std::path::Path;

use luna_content_schema::{ContentId, PackVersion, RelPath};
use sha2::{Digest as _, Sha256};
use thiserror::Error;

use crate::{ContentDomain, ContentError, ContentStore, InstalledItem};

/// 内容从哪来。
#[derive(Debug)]
pub enum Source<'a> {
    /// 用户选的一个本地文件（「从文件安装…」）。不校验 sha256——没有可以比对的目录。
    LocalFile(&'a Path),
    /// 市场下载下来的字节，连同签名目录里给出的 sha256（纵切 7）。
    Downloaded { bytes: Vec<u8>, sha256: &'a str },
    /// 内联在已验签目录条目里的内容（主题包，ADR-159 ③）：完整性来自目录签名，不另比 sha256；
    /// 大小上限照查。
    Signed(Vec<u8>),
}

/// 按域解析后要落盘的东西。
#[derive(Debug)]
pub struct Prepared<P> {
    pub id: ContentId,
    pub version: PackVersion,
    pub files: Vec<(RelPath, Vec<u8>)>,
    /// 解析出的条目（调用方要拿去更新设置、回显界面）。
    pub payload: P,
}

#[derive(Debug, Error)]
pub enum InstallError<E: std::error::Error + 'static> {
    #[error("content is larger than {max} bytes")]
    TooLarge { max: u64 },
    #[error("could not read the file: {0}")]
    Read(std::io::Error),
    #[error("downloaded bytes do not match the catalog checksum")]
    Checksum,
    #[error("content was rejected: {0}")]
    Invalid(#[source] E),
    #[error(transparent)]
    Store(#[from] ContentError),
}

/// 跑一遍安装管线。
///
/// # Errors
/// 见 [`InstallError`]；同一 id 已装同一或更新版本时是 [`ContentError::AlreadyInstalled`]。
pub fn install<P, E: std::error::Error + 'static>(
    store: &ContentStore,
    domain: ContentDomain,
    source: Source<'_>,
    max_bytes: u64,
    prepare: impl FnOnce(&[u8]) -> Result<Prepared<P>, E>,
) -> Result<(InstalledItem, P), InstallError<E>> {
    let bytes = match source {
        Source::LocalFile(path) => read_capped(path, max_bytes)?,
        Source::Downloaded { bytes, sha256 } => {
            if bytes.len() as u64 > max_bytes {
                return Err(InstallError::TooLarge { max: max_bytes });
            }
            if hex(&Sha256::digest(&bytes)) != sha256 {
                return Err(InstallError::Checksum);
            }
            bytes
        }
        Source::Signed(bytes) => {
            if bytes.len() as u64 > max_bytes {
                return Err(InstallError::TooLarge { max: max_bytes });
            }
            bytes
        }
    };
    let prepared = prepare(&bytes).map_err(InstallError::Invalid)?;
    let installed = store.put(domain, &prepared.id, prepared.version, &prepared.files)?;
    Ok((installed, prepared.payload))
}

/// 读一个文件，但最多读 `max + 1` 个字节：大小在打开后才知道，而文件可能正被改写——按读到的
/// 字节数裁决，不信 `metadata().len()`。
fn read_capped<E: std::error::Error + 'static>(
    path: &Path,
    max: u64,
) -> Result<Vec<u8>, InstallError<E>> {
    let file = std::fs::File::open(path).map_err(InstallError::Read)?;
    let mut bytes = Vec::new();
    file.take(max + 1)
        .read_to_end(&mut bytes)
        .map_err(InstallError::Read)?;
    if bytes.len() as u64 > max {
        return Err(InstallError::TooLarge { max });
    }
    Ok(bytes)
}

fn hex(digest: &[u8]) -> String {
    use std::fmt::Write as _;
    digest
        .iter()
        .fold(String::with_capacity(64), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Error)]
    #[error("bad")]
    struct Bad;

    /// 一个最小的「域」：文件内容就是版本号，id 固定。
    fn prepare(bytes: &[u8]) -> Result<Prepared<String>, Bad> {
        let text = std::str::from_utf8(bytes)
            .map_err(|_| Bad)?
            .trim()
            .to_owned();
        let version = PackVersion::parse(&text).map_err(|_| Bad)?;
        Ok(Prepared {
            id: ContentId::parse("demo").expect("id"),
            version,
            files: vec![(RelPath::parse("item.txt").expect("path"), bytes.to_vec())],
            payload: text,
        })
    }

    #[test]
    fn a_local_file_goes_through_the_same_pipeline() {
        let root = tempfile::tempdir().expect("tempdir");
        let store = ContentStore::new(root.path().join("content"));
        let file = root.path().join("demo.txt");
        std::fs::write(&file, "1.0.0").unwrap();
        let (item, payload) = install(
            &store,
            ContentDomain::Themes,
            Source::LocalFile(&file),
            64,
            prepare,
        )
        .expect("install");
        assert_eq!(payload, "1.0.0");
        assert_eq!(
            std::fs::read_to_string(item.dir.join("item.txt")).unwrap(),
            "1.0.0"
        );
    }

    #[test]
    fn oversized_tampered_or_invalid_content_leaves_the_store_untouched() {
        let root = tempfile::tempdir().expect("tempdir");
        let store = ContentStore::new(root.path().join("content"));
        let file = root.path().join("big.txt");
        std::fs::write(&file, "1.0.0").unwrap();
        assert!(matches!(
            install(
                &store,
                ContentDomain::Themes,
                Source::LocalFile(&file),
                4,
                prepare
            ),
            Err(InstallError::TooLarge { max: 4 })
        ));
        let tampered = Source::Downloaded {
            bytes: b"1.0.0".to_vec(),
            sha256: "0000000000000000000000000000000000000000000000000000000000000000",
        };
        assert!(matches!(
            install(&store, ContentDomain::Themes, tampered, 64, prepare),
            Err(InstallError::Checksum)
        ));
        std::fs::write(&file, "not a version").unwrap();
        assert!(matches!(
            install(
                &store,
                ContentDomain::Themes,
                Source::LocalFile(&file),
                64,
                prepare
            ),
            Err(InstallError::Invalid(Bad))
        ));
        assert!(store.list(ContentDomain::Themes).expect("list").is_empty());
    }

    #[test]
    fn downloaded_bytes_with_the_right_checksum_install() {
        let root = tempfile::tempdir().expect("tempdir");
        let store = ContentStore::new(root.path().join("content"));
        let bytes = b"2.0.0".to_vec();
        let sha = hex(&Sha256::digest(&bytes));
        let (item, _) = install(
            &store,
            ContentDomain::Themes,
            Source::Downloaded {
                bytes,
                sha256: &sha,
            },
            64,
            prepare,
        )
        .expect("install");
        assert_eq!(item.version.to_string(), "2.0.0");
    }
}
