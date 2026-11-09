//! 附件的上传与下载、版本的恢复（PRD-21-014 / 024 / 025 / 026，TDD-17 §5.4 / §5.5）在 WASM 这一侧的那几步：
//! 附件放在哪、叫什么、链接怎么写（`luna_sync_core::attachment`，与桌面「粘贴附件」同一份）；任意字节的写入；
//! 以某一版的条目提交新版本；逐块解开大文件。HTTP 的先后在 Worker 里，这里只做判定与加解密。

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use luna_sync_core::attachment::{
    self, asset_ext_from_mime, asset_stamp, attachment_target, free_asset_name, join_vault_rel,
};
use luna_sync_core::commit;
use luna_sync_core::links::is_hidden_rel;
use luna_sync_core::manifest::{self, Assembler, EntryPlain};
use luna_sync_core::path;
use serde_json::json;
use wasm_bindgen::prelude::*;

use crate::{code, int, parse_uuid, Civil, PreparedCommit, VaultSession};

/// 库级设置文件（随库同步，TDD-12 §10.2）：附件位置策略是库级设置，网页上传按这个库自己的设置放。
pub const VAULT_SETTINGS_PATH: &str = ".luna/settings.toml";

/// 网页上单个附件能下载 / 上传的上限（PRD-21-014）：更大的请到桌面端打开。
pub const WEB_FILE_MAX_BYTES: u64 = 200 * 1024 * 1024;

/// 库级设置里附件位置的两项（`vault.attachment_location` / `vault.attachment_folder`）。读不出、没写过都是空串——
/// 空串交给 [`attachment_target`] 即注册表默认（笔记同级 `assets`），与桌面「没写过就用默认」同一结果。
#[must_use]
pub fn attachment_setting(settings_toml: Option<&str>) -> (String, String) {
    #[derive(serde::Deserialize)]
    struct SettingsFile {
        #[serde(default)]
        values: std::collections::BTreeMap<String, toml::Value>,
    }
    let parsed = settings_toml.and_then(|text| toml::from_str::<SettingsFile>(text).ok());
    let read = |key: &str| {
        parsed
            .as_ref()
            .and_then(|file| file.values.get(key))
            .and_then(toml::Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    (
        read("vault.attachment_location"),
        read("vault.attachment_folder"),
    )
}

/// 上传附件的落点（库内路径 + 回填到笔记的相对链接）：`kind = "pasted"` 是粘贴物（没有名字，按
/// `前缀-时间戳.扩展名` 造一个），`"named"` 是拖入的文件（保留它的名字）。名字按库里已有的文件避让（名字比较键）。
///
/// # Errors
/// `invalid_kind` / `invalid_name_*`（落点不合法）/ `vault_create_exhausted`（名字都被占了）。
pub fn upload_target(
    taken: impl Fn(&str) -> bool,
    note_path: &str,
    settings_toml: Option<&str>,
    kind: &str,
    file_name: &str,
    mime: &str,
    at: &luna_sync_core::naming::CivilTime,
) -> Result<UploadTarget, &'static str> {
    let (mode, folder) = attachment_setting(settings_toml);
    let dir = attachment_target(&mode, &folder, note_path, "x").dir_rel;
    let in_dir = |name: &str| taken(&join_vault_rel(&dir, name));
    let name = match kind {
        "pasted" => {
            let prefix = attachment::pasted_asset_prefix(mime);
            let stamp = asset_stamp(at);
            let ext = asset_ext_from_mime(mime);
            free_asset_name(
                |n| attachment::pasted_asset_name(prefix, &stamp, &ext, n),
                in_dir,
            )
        }
        "named" => free_asset_name(|n| attachment::named_asset_name(file_name, n), in_dir),
        _ => return Err("invalid_kind"),
    }
    .ok_or("vault_create_exhausted")?;
    let target = attachment_target(&mode, &folder, note_path, &name);
    let path = join_vault_rel(&target.dir_rel, &name);
    luna_sync_core::naming::check_vault_path(&path).map_err(|err| err.code())?;
    Ok(UploadTarget {
        path,
        link: target.link,
    })
}

/// 一个上传附件的落点。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadTarget {
    /// 库内路径。
    pub path: String,
    /// 回填到笔记的相对链接。
    pub link: String,
}

#[wasm_bindgen]
impl VaultSession {
    /// 库级设置文件的头：`{ file_id, size, chunks }` JSON；库里没有为 `undefined`。
    #[must_use]
    pub fn vault_settings_file(&self) -> Option<String> {
        self.view.by_path_any(VAULT_SETTINGS_PATH).map(|(id, head)| {
            json!({ "file_id": id.to_string(), "size": head.entry.size, "chunks": head.entry.chunks })
                .to_string()
        })
    }

    /// 上传附件的落点（见 [`upload_target`]）：`{ path, link }` JSON。`settings_toml` 是库级设置文件的正文（没有
    /// 就不给）。
    ///
    /// # Errors
    /// `invalid_kind` / `invalid_name_*` / `vault_create_exhausted`。
    pub fn attachment_upload_target(
        &self,
        note_path: &str,
        settings_toml: Option<String>,
        kind: &str,
        file_name: &str,
        mime: &str,
        at: &Civil,
    ) -> Result<String, JsError> {
        let target = upload_target(
            |candidate| self.view.path_taken(candidate, None),
            note_path,
            settings_toml.as_deref(),
            kind,
            file_name,
            mime,
            &at.0,
        )
        .map_err(code)?;
        Ok(json!({ "path": target.path, "link": target.link }).to_string())
    }

    /// 写入一个版本，内容是任意字节（上传的图片与附件）：规则同 [`VaultSession::prepare_put`]。
    ///
    /// # Errors
    /// `invalid_id` / `invalid_number` / `invalid_name_*` / `path_taken`。
    pub fn prepare_put_bytes(
        &self,
        file_id: &str,
        base_rev: Option<f64>,
        path: &str,
        bytes: &[u8],
        mtime_ms: Option<f64>,
    ) -> Result<PreparedCommit, JsError> {
        let id = parse_uuid(file_id)?;
        let base = base_rev.map(int).transpose()?;
        let mtime = mtime_ms.map(int).transpose()?;
        let current = self.view.head(&id).map(|head| head.entry.path.as_str());
        if current != Some(path) {
            self.check_target(path, &id)?;
        }
        let file = manifest::prepare(self.crypto.ring.current(), path, bytes, mtime);
        let item = commit::put(self.crypto.ring.current(), id, base, &file);
        Ok(PreparedCommit::new(
            &item,
            Some(file.entry),
            file.size_padded,
            file.chunks,
        ))
    }

    /// 以某一版的条目提交同一个 `file_id` 的新版本（「恢复为新版本」与「最近删除」里的恢复，TDD-17 §5.4）：块
    /// 原样引用、不重传，修改时间记为现在。`response` 是那一版的 `GET …/history/{file_id}/{rev}` 原样字节（条目在它
    /// 自己的 rev 上解开，补齐后大小取服务端记的那个数）；`path` 是这次要的路径（历史恢复用此刻的路径），缺席即那一版
    /// 的路径（删除恢复）。
    ///
    /// # Errors
    /// `invalid_id` / `invalid_number` / `sync_remote_corrupt` / `entry_unreadable`（那一版是墓碑或解不开）/
    /// `invalid_name_*` / `path_taken`。
    pub fn prepare_revert(
        &self,
        file_id: &str,
        base_rev: f64,
        path: Option<String>,
        response: &[u8],
        mtime_ms: Option<f64>,
    ) -> Result<PreparedCommit, JsError> {
        let id = parse_uuid(file_id)?;
        let base = int(base_rev)?;
        let mtime = mtime_ms.map(int).transpose()?;
        let body: serde_json::Value =
            serde_json::from_slice(response).map_err(|_| code("sync_remote_corrupt"))?;
        let rev = body
            .get("rev")
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| code("sync_remote_corrupt"))?;
        let size_padded = body
            .get("size_padded")
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| code("sync_remote_corrupt"))?;
        let sealed = body
            .get("entry_ct")
            .and_then(serde_json::Value::as_str)
            .and_then(|b64| STANDARD.decode(b64).ok())
            .ok_or_else(|| code("entry_unreadable"))?;
        let old = manifest::open_entry(&self.crypto.ring, id, rev, &sealed)
            .map_err(|_| code("entry_unreadable"))?;
        let path = path.unwrap_or_else(|| old.path.clone());
        let current = self.view.head(&id).map(|head| head.entry.path.as_str());
        if current != Some(path.as_str()) {
            self.check_target(&path, &id)?;
        }
        let file = manifest::PreparedFile {
            entry: EntryPlain {
                path: path::nfc(&path).into_owned(),
                mtime_ms: mtime,
                ..old
            },
            size_padded,
            chunks: Vec::new(),
        };
        let item = commit::put(self.crypto.ring.current(), id, Some(base), &file);
        Ok(PreparedCommit::new(
            &item,
            Some(file.entry),
            file.size_padded,
            Vec::new(),
        ))
    }

    /// 「最近删除」的一页（`GET …/deleted` 的原样字节，TDD-17 §5.4）：`{ items: [{ file_id, rev, last_rev, path,
    /// name, deleted_at, restorable }], next_cursor }` JSON，顺序照服务端（按删除时间倒序）。路径与名字解自墓碑之前
    /// 那一版；那一版已过保留期或解不开的只列出、不给恢复。隐藏路径（`.luna/` 这类机器文件）不列，与树同一口径。
    ///
    /// # Errors
    /// `sync_remote_corrupt`。
    pub fn deleted_page(&self, response: &[u8]) -> Result<String, JsError> {
        let opened: serde_json::Value = serde_json::from_str(&self.crypto.open_entries(response)?)
            .map_err(|_| code("sync_remote_corrupt"))?;
        let entries = opened
            .get("entries")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| code("sync_remote_corrupt"))?;
        let items: Vec<serde_json::Value> = entries
            .iter()
            .filter_map(|item| {
                let path = item
                    .get("entry")
                    .and_then(|entry| entry.get("path"))
                    .and_then(serde_json::Value::as_str);
                if path.is_some_and(is_hidden_rel) {
                    return None;
                }
                let last_rev = item.get("last_rev").and_then(serde_json::Value::as_i64);
                Some(json!({
                    "file_id": item.get("file_id"),
                    "rev": item.get("rev"),
                    "last_rev": last_rev,
                    "path": path,
                    "name": path.map(|p| p.rsplit('/').next().unwrap_or(p)),
                    "deleted_at": item.get("deleted_at"),
                    "restorable": last_rev.is_some() && path.is_some(),
                }))
            })
            .collect();
        Ok(json!({
            "items": items,
            "next_cursor": opened.get("next_cursor").and_then(serde_json::Value::as_i64),
        })
        .to_string())
    }

    /// 网页单文件上限（字节）：下载与上传共用（PRD-21-014 / 025）。
    #[must_use]
    pub fn web_file_max_bytes(&self) -> f64 {
        #[allow(clippy::cast_precision_loss)]
        let max = WEB_FILE_MAX_BYTES as f64;
        max
    }

    /// 下载一个文件要的东西：`{ path, name, size, too_large, chunks }` JSON（`chunks` 按拼装顺序，可重复）。
    /// 超过 [`WEB_FILE_MAX_BYTES`] 的 `too_large` 为真：网页上不下载，请到桌面端打开（PRD-21-014）。
    ///
    /// # Errors
    /// `invalid_id` / `not_found`。
    pub fn download_plan(&self, file_id: &str) -> Result<String, JsError> {
        let id = parse_uuid(file_id)?;
        let head = self.view.head(&id).ok_or_else(|| code("not_found"))?;
        let path = &head.entry.path;
        Ok(json!({
            "path": path,
            "name": path.rsplit('/').next().unwrap_or(path),
            "size": head.entry.size,
            "too_large": head.entry.size > WEB_FILE_MAX_BYTES,
            "chunks": head.entry.chunks,
        })
        .to_string())
    }

    /// 开始逐块拼装此刻这一版（大文件下载）。
    ///
    /// # Errors
    /// `invalid_id` / `not_found`。
    pub fn begin_assembly(&self, file_id: &str) -> Result<Assembly, JsError> {
        let id = parse_uuid(file_id)?;
        let head = self.view.head(&id).ok_or_else(|| code("not_found"))?;
        Ok(Assembly(Some(Assembler::new(&head.entry))))
    }
}

/// 一次逐块拼装：按条目里的顺序喂块密文，拿回每一段明文，最后核对大小与内容哈希。
#[wasm_bindgen]
pub struct Assembly(Option<Assembler>);

#[wasm_bindgen]
impl Assembly {
    /// 下一块要的块 id；都给过了为 `undefined`。
    #[must_use]
    pub fn next_chunk(&self) -> Option<String> {
        self.0
            .as_ref()
            .and_then(Assembler::next_chunk)
            .map(str::to_owned)
    }

    /// 解开下一块，返回这一段明文。
    ///
    /// # Errors
    /// `used` / `content_mismatch`（块不对、解不开）。
    pub fn push(
        &mut self,
        session: &VaultSession,
        chunk_id: &str,
        sealed: &[u8],
    ) -> Result<Vec<u8>, JsError> {
        self.0
            .as_mut()
            .ok_or_else(|| code("used"))?
            .push(&session.crypto.ring, chunk_id, sealed)
            .map_err(|_| code("content_mismatch"))
    }

    /// 都给过了：核对大小与内容哈希。
    ///
    /// # Errors
    /// `used` / `content_mismatch`。
    pub fn finish(&mut self) -> Result<(), JsError> {
        self.0
            .take()
            .ok_or_else(|| code("used"))?
            .finish()
            .map_err(|_| code("content_mismatch"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luna_sync_core::naming::CivilTime;

    const AT: CivilTime = CivilTime {
        year: 2026,
        month: 11,
        day: 6,
        hour: 9,
        minute: 5,
        second: 7,
    };

    #[test]
    fn uploads_follow_the_vault_settings_and_avoid_taken_names() {
        let settings = "schema_version = 1\n\n[values]\n\"vault.attachment_location\" = \"specified_folder\"\n\"vault.attachment_folder\" = \"附件\"\n";
        let taken = |path: &str| path == "附件/Pasted-image-20261106-090507.png";
        let target = upload_target(
            taken,
            "写作/上传.md",
            Some(settings),
            "pasted",
            "image.png",
            "image/png",
            &AT,
        )
        .unwrap();
        assert_eq!(target.path, "附件/Pasted-image-20261106-090507-1.png");
        assert_eq!(target.link, "../附件/Pasted-image-20261106-090507-1.png");
        // 没有设置文件（或读不出）= 注册表默认：笔记同级 assets。
        let target = upload_target(
            |_| false,
            "写作/上传.md",
            Some("不是 toml ["),
            "named",
            "季度 报告.PDF",
            "application/pdf",
            &AT,
        )
        .unwrap();
        assert_eq!(target.path, "写作/assets/季度-报告.pdf");
        assert_eq!(target.link, "assets/季度-报告.pdf");
        assert_eq!(
            upload_target(|_| false, "a.md", None, "other", "x", "", &AT).unwrap_err(),
            "invalid_kind"
        );
        // 隐藏名字的文件进不了库。
        assert_eq!(
            upload_target(|_| false, "a.md", None, "named", ".env", "", &AT).unwrap_err(),
            "invalid_name_hidden"
        );
    }
}
