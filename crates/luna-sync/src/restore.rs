//! 整库时间点恢复（PRD-13-007、ADR-148 ⑪）：把远端库在某个序号时的全部文件解密写进一个**新的空
//! 文件夹**。从不碰当前库，也不动同步基线——恢复出来的只是一份普通文件夹，用户比对后自己决定
//! 拿回哪些。
//!
//! 每个文件：开条目 → 取块 → 组装（组装时核对内容哈希与大小）→ 写入。条目或块解不开的文件
//! 跳过并计数（不让一个坏文件拖垮整次恢复）；本机缺那一版钥匙 → 整次失败（`KeysMissing`），
//! 那一刻的库本机就是读不了。
//!
//! **整次恢复是原子的**：先写进目标旁边的隐藏暂存文件夹，全部写完才改名成目标；中途失败（断网、
//! 缺钥匙、磁盘满）就删掉暂存。否则用户会拿到一个名字写着「恢复到某时刻」、内容却缺一半的文件夹，
//! 而且再试一次会因为它不空被拒。

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use uuid::Uuid;

use crate::ports::{Remote, RemoteError};
use luna_sync_core::crypto::{CryptoError, KeyRing};
use luna_sync_core::manifest::{assemble, open_entry, ManifestError};

/// 一次恢复的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RestoreReport {
    pub files: u64,
    pub bytes: u64,
    /// 条目或内容解不开、路径不合法而跳过的文件数。
    pub skipped: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum RestoreError {
    #[error(transparent)]
    Remote(#[from] RemoteError),
    /// 本机没有解那一刻数据所需的钥匙版本。
    #[error("keys for this point in time are missing on this device")]
    KeysMissing,
    /// 目标文件夹已存在且不是空的：恢复从不覆盖任何东西。
    #[error("destination is not empty")]
    DestinationNotEmpty,
    #[error("local io: {0}")]
    Io(String),
}

impl RestoreError {
    /// 稳定码（界面按码出文案；日志不含路径）。
    #[must_use]
    pub fn code(&self) -> String {
        match self {
            Self::Remote(remote) => remote.code(),
            Self::KeysMissing => "sync_keys_missing".into(),
            Self::DestinationNotEmpty => "restore_destination_not_empty".into(),
            Self::Io(_) => "restore_local_io".into(),
        }
    }
}

fn io(err: std::io::Error) -> RestoreError {
    RestoreError::Io(err.kind().to_string())
}

/// 库相对路径落到目标文件夹里：只认普通段，`..`、绝对路径、空段一律不收（防止写出目标文件夹）。
fn target_of(dest: &Path, rel: &str) -> Option<PathBuf> {
    let rel = Path::new(rel);
    let mut out = dest.to_path_buf();
    let mut any = false;
    for part in rel.components() {
        match part {
            Component::Normal(name) => {
                out.push(name);
                any = true;
            }
            _ => return None,
        }
    }
    any.then_some(out)
}

/// 把远端库 `vault` 在 `at_seq` 时的全部文件恢复到 `dest`（不存在就建；已存在必须为空）。
/// `progress(已处理, 总数)` 每个条目之前回调一次、结束时再报一次满。
///
/// # Errors
/// 目标不空、网络或服务端失败（含那个时间点已超出保留期）、本机缺钥匙、写不了磁盘。
pub async fn restore_to_folder(
    remote: &dyn Remote,
    ring: &KeyRing,
    vault: Uuid,
    at_seq: i64,
    dest: &Path,
    mut progress: impl FnMut(u64, u64),
) -> Result<RestoreReport, RestoreError> {
    if dest.exists() {
        let mut entries = std::fs::read_dir(dest).map_err(io)?;
        if entries.next().is_some() {
            return Err(RestoreError::DestinationNotEmpty);
        }
    }
    let manifest = remote.manifest(vault, Some(at_seq)).await?;
    let parent = dest
        .parent()
        .ok_or_else(|| RestoreError::Io("no parent".into()))?;
    std::fs::create_dir_all(parent).map_err(io)?;
    let staging = parent.join(format!(".lunarca-restoring-{}", Uuid::now_v7().simple()));
    std::fs::create_dir(&staging).map_err(io)?;
    let result = async {
        let report = fill(
            remote,
            ring,
            vault,
            &manifest.entries,
            &staging,
            &mut progress,
        )
        .await?;
        // 目标若是用户选的空文件夹，先拿掉它再把暂存改名过去（同一父目录，改名是原子的）。
        if dest.exists() {
            std::fs::remove_dir(dest).map_err(|_| RestoreError::DestinationNotEmpty)?;
        }
        std::fs::rename(&staging, dest).map_err(io)?;
        Ok(report)
    }
    .await;
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result
}

async fn fill(
    remote: &dyn Remote,
    ring: &KeyRing,
    vault: Uuid,
    entries: &[crate::ports::RemoteEntry],
    dest: &Path,
    progress: &mut impl FnMut(u64, u64),
) -> Result<RestoreReport, RestoreError> {
    let total = entries.len() as u64;
    let mut report = RestoreReport::default();
    let mut blobs: HashMap<String, Vec<u8>> = HashMap::new();
    for (done, entry) in entries.iter().enumerate() {
        // 每个条目都报一次「已处理几个」：跳过的也算处理过，否则进度会停在跳过之处、最后一下跳满。
        progress(done as u64, total);
        // 服务端的清单不含已删除的条目；这里再挡一次，免得墓碑被算成「解不开」。
        if entry.deleted {
            continue;
        }
        let Some(ct) = entry.entry_ct.as_deref() else {
            report.skipped += 1;
            continue;
        };
        let plain = match open_entry(ring, entry.file_id, entry.rev, ct) {
            Ok(plain) => plain,
            Err(ManifestError::Crypto(CryptoError::KeyVersion { .. })) => {
                return Err(RestoreError::KeysMissing);
            }
            Err(_) => {
                report.skipped += 1;
                continue;
            }
        };
        let Some(target) = target_of(dest, &plain.path) else {
            report.skipped += 1;
            continue;
        };
        // 大文件（录音）逐块解密直接写盘、边写边算哈希：不把几百 MB 拼在内存里。
        if plain.size > luna_sync_core::chunk::WHOLE_FILE_MAX as u64 {
            if write_streamed(remote, ring, vault, &plain, &target).await? {
                report.files += 1;
                report.bytes += plain.size;
            } else {
                report.skipped += 1;
            }
            continue;
        }
        for id in &plain.chunks {
            if !blobs.contains_key(id) {
                let sealed = remote.get_blob(vault, id).await?;
                blobs.insert(id.clone(), sealed);
            }
        }
        let bytes = match assemble(ring, &plain, |id| {
            blobs.get(id).cloned().ok_or_else(|| "missing".into())
        }) {
            Ok(bytes) => bytes,
            Err(ManifestError::Crypto(CryptoError::KeyVersion { .. })) => {
                return Err(RestoreError::KeysMissing);
            }
            Err(_) => {
                report.skipped += 1;
                continue;
            }
        };
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        luna_vault::atomic::write_atomic(&target, &bytes)
            .map_err(|e| RestoreError::Io(e.to_string()))?;
        // 这个文件的块用完就放掉：大库不必把全部密文攒在内存里（同一块被后面的文件复用时再取）。
        for id in &plain.chunks {
            blobs.remove(id);
        }
        report.files += 1;
        report.bytes += bytes.len() as u64;
    }
    progress(total, total);
    Ok(report)
}

/// 把一个大文件逐块写到 `target`；块解不开、拼出来对不上 → 删掉写了一半的文件，返回 `false`。
async fn write_streamed(
    remote: &dyn Remote,
    ring: &KeyRing,
    vault: Uuid,
    plain: &luna_sync_core::manifest::EntryPlain,
    target: &Path,
) -> Result<bool, RestoreError> {
    use sha2::Digest as _;
    use std::io::Write as _;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(io)?;
    }
    let mut out = std::io::BufWriter::new(std::fs::File::create(target).map_err(io)?);
    let mut hasher = sha2::Sha256::new();
    let mut size: u64 = 0;
    let mut ok = true;
    for id in &plain.chunks {
        let sealed = remote.get_blob(vault, id).await?;
        let opened = ring
            .for_ciphertext(&sealed)
            .and_then(|keys| keys.open_chunk(id, &sealed));
        let padded = match opened {
            Ok(padded) => padded,
            Err(CryptoError::KeyVersion { .. }) => return Err(RestoreError::KeysMissing),
            Err(_) => {
                ok = false;
                break;
            }
        };
        let Ok(chunk) = luna_sync_core::chunk::unpad(&padded) else {
            ok = false;
            break;
        };
        hasher.update(chunk);
        size += chunk.len() as u64;
        out.write_all(chunk).map_err(io)?;
    }
    out.flush().map_err(io)?;
    drop(out);
    let digest: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if !ok || size != plain.size || digest != plain.content_hash {
        let _ = std::fs::remove_file(target);
        return Ok(false);
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_never_leave_the_destination() {
        let dest = Path::new("/tmp/restore-dest");
        assert_eq!(target_of(dest, "a/b.md"), Some(dest.join("a/b.md")));
        for bad in ["../x.md", "/etc/passwd", "a/../../x", "", "."] {
            assert_eq!(target_of(dest, bad), None, "{bad}");
        }
    }
}
