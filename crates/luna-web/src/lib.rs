//! 网页版笔记的 WASM 封装（ADR-177 ⑥、TDD-17 §2 / §3）。
//!
//! 只做一件事：把 `luna-sync-core`（加解密、分块、清单与提交请求体、合并、路径名口径、文件命名规则）与
//! `luna-opaque`（口令登录）原样交给浏览器里的专用 Worker。这里不另写任何格式或规则——两端结果逐字节一致由互通夹具钉住
//! （`luna-sync-core/tests/fixtures/sync-interop-v1.json`，本 crate 的 WASM 测试读同一份）。
//!
//! 接口约定：字节进出用 `Uint8Array`，结构化结果用 JSON 字符串；失败只抛稳定码（`JsError` 的
//! message 是 snake_case 码，不带正文、路径或密钥）。CMK 只活在 [`VaultCrypto`] 里，离开作用域清零。

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use luna_sync_core::commit::{self, CommitItem};
use luna_sync_core::crypto::{Cmk, KeyRing};
use luna_sync_core::hashing::ContentHash;
use luna_sync_core::manifest::{self, EntryPlain, SealedChunk};
use luna_sync_core::merge3::{self, MergeOutcome};
use luna_sync_core::naming::{self, CivilTime};
use luna_sync_core::path;
use opaque_ke::{
    ClientLogin, ClientLoginFinishParameters, ClientRegistration,
    ClientRegistrationFinishParameters, CredentialResponse, RegistrationResponse,
};
use serde_json::json;
use uuid::Uuid;
use wasm_bindgen::prelude::*;
use zeroize::Zeroizing;

use luna_opaque::Suite;

pub mod editing;
pub mod view;

// 附件的上传与下载、版本恢复（PRD-21-014 / 024 / 025 / 026）；全文搜索与标签（PRD-21-041 / 042）。
pub mod media;
pub mod search;

/// 信封里的托管槽（服务端为已登录设备解封后下发明文 CMK，ADR-150 ①）；与桌面 `cloud_sync` 同名。
const MANAGED_SLOT: &str = "managed";

fn code(code: &str) -> JsError {
    JsError::new(code)
}

fn parse_uuid(text: &str) -> Result<Uuid, JsError> {
    Uuid::parse_str(text).map_err(|_| code("invalid_id"))
}

/// JS 的 number → rev / 时间戳（必须是安全整数）。
fn int(value: f64) -> Result<i64, JsError> {
    const MAX_SAFE: f64 = 9_007_199_254_740_991.0;
    if value.fract() != 0.0 || value.abs() > MAX_SAFE {
        return Err(code("invalid_number"));
    }
    #[allow(clippy::cast_possible_truncation)]
    Ok(value as i64)
}

/// 一个云端库的密钥环：由服务端下发的托管 CMK 构造（ADR-150 ①），只在 Worker 内存里。
#[wasm_bindgen]
pub struct VaultCrypto {
    ring: KeyRing,
}

#[wasm_bindgen]
impl VaultCrypto {
    /// `cmk` 是解封后的 32 字节库主密钥；构造完即可丢弃调用方手里的那份。
    ///
    /// # Errors
    /// `invalid_id` / `invalid_key`。
    #[wasm_bindgen(constructor)]
    pub fn new(vault_id: &str, cmk_version: u32, cmk: &[u8]) -> Result<VaultCrypto, JsError> {
        let vault = parse_uuid(vault_id)?;
        let bytes: Zeroizing<[u8; 32]> =
            Zeroizing::new(cmk.try_into().map_err(|_| code("invalid_key"))?);
        let ring = KeyRing::new(vault, &[Cmk::from_bytes(cmk_version, *bytes)])
            .ok_or_else(|| code("invalid_key"))?;
        Ok(Self { ring })
    }

    /// 由 `GET /sync/v1/vaults/{id}/envelopes` 的响应体（原样字节）构造：取托管槽的全部 CMK 版本
    /// （ADR-150 ①）。CMK 只在这里解析、只活在密钥环里——调用方随后应把手里那份响应缓冲区清零。
    ///
    /// # Errors
    /// `invalid_id`；`no_managed_key`（库没有托管信封，即高级数据保护的库，网页上不可访问，
    /// ADR-150 ⑥）；`sync_remote_corrupt`（响应形状不对、密钥不是 32 字节或版本非法）。
    pub fn from_envelopes(vault_id: &str, response: &[u8]) -> Result<VaultCrypto, JsError> {
        let vault = parse_uuid(vault_id)?;
        let cmks = managed_cmks(response).map_err(code)?;
        if cmks.is_empty() {
            return Err(code("no_managed_key"));
        }
        let ring = KeyRing::new(vault, &cmks).ok_or_else(|| code("sync_remote_corrupt"))?;
        Ok(Self { ring })
    }

    /// 批量解开清单类响应里的条目：`GET …/manifest`、`…/changes`、`…/deleted` 三种响应体（原样字节）
    /// 都适用。返回 JSON：顶层其余字段原样保留；`entries` 里每项去掉 `entry_ct`，换成 `entry`
    /// （`EntryPlain`，墓碑或解不开时为 `null`），解不开的另带 `"unreadable": true`。条目密文绑定的
    /// rev 取 `last_rev`（最近删除里墓碑之前那一版）或 `rev`。
    ///
    /// # Errors
    /// `sync_remote_corrupt`（不是带 `entries` 数组的 JSON 对象）。
    pub fn open_entries(&self, response: &[u8]) -> Result<String, JsError> {
        let mut body: serde_json::Value =
            serde_json::from_slice(response).map_err(|_| code("sync_remote_corrupt"))?;
        let entries = body
            .get_mut("entries")
            .and_then(serde_json::Value::as_array_mut)
            .ok_or_else(|| code("sync_remote_corrupt"))?;
        for item in entries.iter_mut() {
            let object = item
                .as_object_mut()
                .ok_or_else(|| code("sync_remote_corrupt"))?;
            let sealed = object.remove("entry_ct");
            let opened = sealed
                .as_ref()
                .and_then(serde_json::Value::as_str)
                .map(|b64| self.open_view_entry(object, b64));
            match opened {
                None => {
                    object.insert("entry".into(), serde_json::Value::Null);
                }
                Some(Some(entry)) => {
                    object.insert("entry".into(), entry);
                }
                Some(None) => {
                    object.insert("entry".into(), serde_json::Value::Null);
                    object.insert("unreadable".into(), serde_json::Value::Bool(true));
                }
            }
        }
        Ok(body.to_string())
    }

    /// 解开库名。
    ///
    /// # Errors
    /// `decrypt_failed`。
    pub fn open_vault_name(&self, sealed: &[u8]) -> Result<String, JsError> {
        self.ring
            .for_ciphertext(sealed)
            .and_then(|keys| keys.open_vault_name(sealed))
            .map_err(|_| code("decrypt_failed"))
    }

    /// 解开一条清单条目，返回 `EntryPlain` 的 JSON。
    ///
    /// # Errors
    /// `invalid_id` / `invalid_number` / `entry_unreadable`。
    pub fn open_entry(&self, file_id: &str, rev: f64, sealed: &[u8]) -> Result<String, JsError> {
        let entry = manifest::open_entry(&self.ring, parse_uuid(file_id)?, int(rev)?, sealed)
            .map_err(|_| code("entry_unreadable"))?;
        serde_json::to_string(&entry).map_err(|_| code("entry_unreadable"))
    }

    /// 按条目拼出文件内容并核对大小与内容哈希。`chunk_ids[i]` 对应 `chunks[i]` 的密文。
    ///
    /// # Errors
    /// `entry_unreadable` / `chunk_missing` / `content_mismatch`。
    pub fn assemble(
        &self,
        entry_json: &str,
        chunk_ids: Vec<String>,
        chunks: Vec<js_sys::Uint8Array>,
    ) -> Result<Vec<u8>, JsError> {
        let entry: EntryPlain =
            serde_json::from_str(entry_json).map_err(|_| code("entry_unreadable"))?;
        let by_id: std::collections::HashMap<String, Vec<u8>> = chunk_ids
            .into_iter()
            .zip(chunks.iter().map(js_sys::Uint8Array::to_vec))
            .collect();
        manifest::assemble(&self.ring, &entry, |id| {
            by_id.get(id).cloned().ok_or_else(|| id.to_owned())
        })
        .map_err(|err| match err {
            manifest::ManifestError::Fetch(_) => code("chunk_missing"),
            _ => code("content_mismatch"),
        })
    }

    /// 切块、补齐、加密一份文件内容。返回 JSON：`{ entry, size_padded, chunks: [{ id, ct_sha256 }] }`，
    /// 块密文按同一顺序由 [`PreparedChunks::sealed`] 取。
    ///
    /// # Errors
    /// `invalid_number`。
    pub fn prepare(
        &self,
        path: &str,
        bytes: &[u8],
        mtime_ms: Option<f64>,
    ) -> Result<PreparedChunks, JsError> {
        let mtime = mtime_ms.map(int).transpose()?;
        let prepared = manifest::prepare(self.ring.current(), path, bytes, mtime);
        let summary = json!({
            "entry": prepared.entry,
            "size_padded": prepared.size_padded,
            "chunks": prepared.chunks.iter().map(|c| json!({
                "id": c.id,
                "ct_sha256": hex(&c.ct_sha256),
            })).collect::<Vec<_>>(),
        })
        .to_string();
        Ok(PreparedChunks {
            summary,
            sealed: prepared.chunks.into_iter().map(|c| c.sealed).collect(),
        })
    }

    /// 封一条清单条目（提交用）。
    ///
    /// # Errors
    /// `invalid_id` / `invalid_number` / `entry_unreadable`。
    pub fn seal_entry(
        &self,
        file_id: &str,
        rev: f64,
        entry_json: &str,
    ) -> Result<Vec<u8>, JsError> {
        let entry: EntryPlain =
            serde_json::from_str(entry_json).map_err(|_| code("entry_unreadable"))?;
        Ok(manifest::seal_entry(
            self.ring.current(),
            parse_uuid(file_id)?,
            int(rev)?,
            &entry,
        ))
    }
}

impl VaultCrypto {
    /// 一条响应条目的密文 → `EntryPlain` 的 JSON 值；任何一步不对都是 `None`（逐条容错，一条坏
    /// 条目不拖垮整棵树）。
    fn open_view_entry(
        &self,
        object: &serde_json::Map<String, serde_json::Value>,
        sealed_b64: &str,
    ) -> Option<serde_json::Value> {
        let file_id = Uuid::parse_str(object.get("file_id")?.as_str()?).ok()?;
        let rev = object
            .get("last_rev")
            .and_then(serde_json::Value::as_i64)
            .or_else(|| object.get("rev").and_then(serde_json::Value::as_i64))?;
        let sealed = STANDARD.decode(sealed_b64).ok()?;
        let entry = manifest::open_entry(&self.ring, file_id, rev, &sealed).ok()?;
        serde_json::to_value(entry).ok()
    }
}

/// 一个已打开的云端库（TDD-17 §3 `open_vault`）：库密钥环 + 解开的清单视图，都只在 Worker 的 WASM
/// 内存里。主线程只拿到树、「最近」、搜索结果、一篇的明文这类命令结果；`free()` 即清零。
#[wasm_bindgen]
pub struct VaultSession {
    crypto: VaultCrypto,
    view: view::VaultView,
}

#[wasm_bindgen]
impl VaultSession {
    /// 由 `GET …/envelopes` 的原样字节打开（同 [`VaultCrypto::from_envelopes`]）。
    ///
    /// # Errors
    /// `invalid_id` / `no_managed_key` / `sync_remote_corrupt`。
    pub fn open(vault_id: &str, envelopes: &[u8]) -> Result<VaultSession, JsError> {
        Ok(Self {
            crypto: VaultCrypto::from_envelopes(vault_id, envelopes)?,
            view: view::VaultView::default(),
        })
    }

    /// 封存离线锁定时没存上的改动（TDD-17 §4）：密文交回主线程，只留在那一页的内存里。
    pub fn seal_drafts(&self, drafts: &str) -> Vec<u8> {
        self.crypto.ring.current().seal_draft(drafts.as_bytes())
    }

    /// 解开 [`Self::seal_drafts`] 的密文。
    ///
    /// # Errors
    /// `decrypt_failed`：不是这个库的密文，或内容被改过。
    pub fn open_drafts(&self, sealed: &[u8]) -> Result<String, JsError> {
        let plain = self
            .crypto
            .ring
            .for_ciphertext(sealed)
            .and_then(|keys| keys.open_draft(sealed))
            .map_err(|_| code("decrypt_failed"))?;
        String::from_utf8(plain).map_err(|_| code("decrypt_failed"))
    }

    ///
    /// # Errors
    /// `sync_remote_corrupt`。
    pub fn load_manifest(&mut self, response: &[u8]) -> Result<(), JsError> {
        let (body, changes) = self.parse_entries(response)?;
        let epoch = body
            .get("epoch")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| code("sync_remote_corrupt"))?;
        let at_seq = body
            .get("at_seq")
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| code("sync_remote_corrupt"))?;
        self.view.reset(epoch.to_owned(), at_seq, changes);
        Ok(())
    }

    /// 增量变更（`GET …/changes?epoch=&since=` 的原样字节）。返回 JSON：
    /// `{ next_seq, has_more, changed: [file_id] }`。
    ///
    /// # Errors
    /// `sync_epoch_changed`（纪元变了：调用方改拉全量清单）/ `sync_remote_corrupt`。
    pub fn apply_changes(&mut self, response: &[u8]) -> Result<String, JsError> {
        let (body, changes) = self.parse_entries(response)?;
        if body.get("epoch").and_then(serde_json::Value::as_str) != Some(self.view.epoch()) {
            return Err(code("sync_epoch_changed"));
        }
        let next_seq = body
            .get("next_seq")
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| code("sync_remote_corrupt"))?;
        let has_more = body
            .get("has_more")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let changed = self.view.apply(changes);
        self.view.set_head_seq(next_seq);
        Ok(json!({ "next_seq": next_seq, "has_more": has_more, "changed": changed }).to_string())
    }

    /// 清单纪元（`changes?epoch=` 用）。
    #[must_use]
    pub fn epoch(&self) -> String {
        self.view.epoch().to_owned()
    }

    /// 已读到的序号（`changes?since=` 用）。
    #[must_use]
    pub fn head_seq(&self) -> f64 {
        #[allow(clippy::cast_precision_loss)]
        let seq = self.view.head_seq() as f64;
        seq
    }

    /// 路径树 JSON（见 [`view::VaultView::tree`]）。
    #[must_use]
    pub fn tree(&self) -> String {
        json!(self.view.tree()).to_string()
    }

    /// 「最近」JSON。
    #[must_use]
    pub fn recent(&self, limit: usize) -> String {
        json!(self.view.recent(limit)).to_string()
    }

    /// 标题与路径搜索 JSON。
    #[must_use]
    pub fn search(&self, query: &str, limit: usize) -> String {
        json!(self.view.search(query, limit)).to_string()
    }

    /// 一个文件的头：`{ file_id, rev, path, size, chunks }`（读正文前取块清单用）。
    ///
    /// # Errors
    /// `invalid_id` / `not_found`。
    pub fn file(&self, file_id: &str) -> Result<String, JsError> {
        let id = parse_uuid(file_id)?;
        let head = self.view.head(&id).ok_or_else(|| code("not_found"))?;
        Ok(json!({
            "file_id": file_id,
            "rev": head.rev,
            "path": head.entry.path,
            "size": head.entry.size,
            "chunks": head.entry.chunks,
        })
        .to_string())
    }

    /// 按库内路径找文件：`FileSummary` JSON，没有为 `undefined`。
    #[must_use]
    pub fn file_by_path(&self, path: &str) -> Option<String> {
        self.view.by_path(path).map(|hit| json!(hit).to_string())
    }

    /// 拼出一个文件的内容并核对大小与内容哈希（块密文按 `chunk_ids` 的顺序给）。
    ///
    /// # Errors
    /// `invalid_id` / `not_found` / `chunk_missing` / `content_mismatch`。
    pub fn assemble(
        &self,
        file_id: &str,
        chunk_ids: Vec<String>,
        chunks: Vec<js_sys::Uint8Array>,
    ) -> Result<Vec<u8>, JsError> {
        let id = parse_uuid(file_id)?;
        let head = self.view.head(&id).ok_or_else(|| code("not_found"))?;
        let by_id: std::collections::HashMap<String, Vec<u8>> = chunk_ids
            .into_iter()
            .zip(chunks.iter().map(js_sys::Uint8Array::to_vec))
            .collect();
        manifest::assemble(&self.crypto.ring, &head.entry, |chunk| {
            by_id.get(chunk).cloned().ok_or_else(|| chunk.to_owned())
        })
        .map_err(|err| match err {
            manifest::ManifestError::Fetch(_) => code("chunk_missing"),
            _ => code("content_mismatch"),
        })
    }

    /// 一批 wikilink 目标各指向哪一篇：JSON `[路径 | null]`。
    #[must_use]
    pub fn resolve_wikilinks(&self, src: &str, targets: Vec<String>) -> String {
        json!(self.view.resolve_wikilinks(src, &targets)).to_string()
    }

    /// `[[` 补全候选：JSON `[{title, insert, alias, folder, path}]`（排序与同名消歧在 `luna-sync-core`）。
    #[must_use]
    pub fn wikilink_candidates(&self, src: &str, query: &str, limit: usize) -> String {
        json!(self.view.wikilink_candidates(src, query, limit)).to_string()
    }

    /// 一条标准 md 链接指向哪一篇的路径；指不到为 `undefined`。
    #[must_use]
    pub fn resolve_link(&self, src: &str, href: &str) -> Option<String> {
        self.view.resolve_link(src, href)
    }

    /// 笔记相对的图片 / 附件地址 → `FileSummary` JSON；指不到为 `undefined`。
    #[must_use]
    pub fn resolve_asset(&self, note: &str, src: &str) -> Option<String> {
        self.view
            .resolve_asset(note, src)
            .map(|hit| json!(hit).to_string())
    }

    /// 一批嵌入目标各指向什么：JSON（见 [`view::EmbedTarget`]）。
    #[must_use]
    pub fn resolve_embeds(&self, src: &str, targets: Vec<String>) -> String {
        json!(self.view.resolve_embeds(src, &targets)).to_string()
    }

    // ─── 写回（TDD-17 §5.2–§5.4） ─────────────────────────────────────────────

    /// 写入一个版本：保存、新建、冲突副本、在墓碑上恢复都走这里。`base_rev` 缺席 = 新文件；条目封在
    /// 基线 + 1 上（与桌面同一条规则）。`text` 的 UTF-8 字节原样即文件内容——不改行尾、不补换行，与桌面
    /// 写盘的字节相同。路径要合法；它不是这个文件此刻的路径时，还不能撞上别的文件（名字比较键）。
    ///
    /// # Errors
    /// `invalid_id` / `invalid_number` / `invalid_name_*` / `path_taken`。
    pub fn prepare_put(
        &self,
        file_id: &str,
        base_rev: Option<f64>,
        path: &str,
        text: &str,
        mtime_ms: Option<f64>,
    ) -> Result<PreparedCommit, JsError> {
        let id = parse_uuid(file_id)?;
        let base = base_rev.map(int).transpose()?;
        let mtime = mtime_ms.map(int).transpose()?;
        let current = self.view.head(&id).map(|head| head.entry.path.as_str());
        if current != Some(path) {
            self.check_target(path, &id)?;
        }
        let file = manifest::prepare(self.crypto.ring.current(), path, text.as_bytes(), mtime);
        let item = commit::put(self.crypto.ring.current(), id, base, &file);
        Ok(PreparedCommit::new(
            &item,
            Some(file.entry),
            file.size_padded,
            file.chunks,
        ))
    }

    /// 改名或移动（同一 `file_id` 换路径，ADR-148 ③）：内容与块不变，以此刻的头为基线。
    ///
    /// # Errors
    /// `invalid_id` / `not_found` / `invalid_name_*` / `path_taken`。
    pub fn prepare_rename(&self, file_id: &str, path: &str) -> Result<PreparedCommit, JsError> {
        let id = parse_uuid(file_id)?;
        let head = self.view.head(&id).ok_or_else(|| code("not_found"))?;
        self.check_target(path, &id)?;
        let file = manifest::PreparedFile {
            entry: EntryPlain {
                path: path::nfc(path).into_owned(),
                ..head.entry.clone()
            },
            size_padded: head.size_padded,
            chunks: Vec::new(),
        };
        let item = commit::put(self.crypto.ring.current(), id, Some(head.rev), &file);
        Ok(PreparedCommit::new(
            &item,
            Some(file.entry),
            file.size_padded,
            Vec::new(),
        ))
    }

    /// 墓碑（ADR-148 ⑤：带当前 rev）。
    ///
    /// # Errors
    /// `invalid_id` / `invalid_number`。
    pub fn prepare_delete(&self, file_id: &str, base_rev: f64) -> Result<PreparedCommit, JsError> {
        let item = commit::delete(parse_uuid(file_id)?, int(base_rev)?);
        Ok(PreparedCommit::new(&item, None, 0, Vec::new()))
    }

    /// 一次提交成功了：把它记进视图（树、按路径找文件、撞名判定立刻看得到它），不等事件流绕回来。
    ///
    /// # Errors
    /// `invalid_number`。
    pub fn record_commit(&mut self, prepared: &PreparedCommit, rev: f64) -> Result<(), JsError> {
        let rev = int(rev)?;
        let change = match &prepared.entry {
            Some(entry) => view::Change::Put {
                file_id: prepared.file_id,
                rev,
                entry: entry.clone(),
                size_padded: prepared.size_padded,
            },
            None => view::Change::Delete {
                file_id: prepared.file_id,
                rev,
            },
        };
        self.view.apply(vec![change]);
        Ok(())
    }

    /// 解开单个条目的响应（`GET …/history/{file_id}/{rev}` 的原样字节）：`{ file_id, rev, deleted,
    /// entry }`（墓碑的 `entry` 为 `null`）。`adopt` = 这是此刻的头，同时记进视图。
    ///
    /// # Errors
    /// `sync_remote_corrupt` / `entry_unreadable`。
    pub fn open_entry_response(&mut self, response: &[u8], adopt: bool) -> Result<String, JsError> {
        let body: serde_json::Value =
            serde_json::from_slice(response).map_err(|_| code("sync_remote_corrupt"))?;
        let file_id = body
            .get("file_id")
            .and_then(serde_json::Value::as_str)
            .and_then(|id| Uuid::parse_str(id).ok())
            .ok_or_else(|| code("sync_remote_corrupt"))?;
        let rev = body
            .get("rev")
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| code("sync_remote_corrupt"))?;
        let deleted = body
            .get("deleted")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let entry = if deleted {
            None
        } else {
            let sealed = body
                .get("entry_ct")
                .and_then(serde_json::Value::as_str)
                .and_then(|b64| STANDARD.decode(b64).ok())
                .ok_or_else(|| code("sync_remote_corrupt"))?;
            Some(
                manifest::open_entry(&self.crypto.ring, file_id, rev, &sealed)
                    .map_err(|_| code("entry_unreadable"))?,
            )
        };
        let out = json!({
            "file_id": file_id.to_string(),
            "rev": rev,
            "deleted": deleted,
            "entry": entry,
        })
        .to_string();
        if adopt {
            let change = match entry {
                Some(entry) => view::Change::Put {
                    file_id,
                    rev,
                    entry,
                    size_padded: body
                        .get("size_padded")
                        .and_then(serde_json::Value::as_i64)
                        .unwrap_or(0),
                },
                None => view::Change::Delete { file_id, rev },
            };
            self.view.apply(vec![change]);
        }
        Ok(out)
    }

    /// 按一条条目（`EntryPlain` 的 JSON，例如 [`Self::open_entry_response`] 给的）拼出内容并核对。
    ///
    /// # Errors
    /// `entry_unreadable` / `chunk_missing` / `content_mismatch`。
    pub fn assemble_entry(
        &self,
        entry_json: &str,
        chunk_ids: Vec<String>,
        chunks: Vec<js_sys::Uint8Array>,
    ) -> Result<Vec<u8>, JsError> {
        self.crypto.assemble(entry_json, chunk_ids, chunks)
    }

    /// 本方内容另存成冲突副本时用的路径（ADR-148 ⑥）：`名字 (说明 日期).扩展名`，按名字比较键避开
    /// 库里已有的文件，重名加序号。
    ///
    /// # Errors
    /// `conflict_copy_exhausted`（名字都被占了）。
    pub fn conflict_copy_path(
        &self,
        path: &str,
        label: &str,
        date: &str,
    ) -> Result<String, JsError> {
        naming::free_conflict_copy_path(path, label, date, |candidate| {
            self.view.path_taken(candidate, None)
        })
        .ok_or_else(|| code("conflict_copy_exhausted"))
    }

    /// 新笔记的路径（与桌面同一套命名）：`dir` 下由正文标题行派生的名字，没有就是
    /// `前缀_YYYYMMDD_HHMMSS`；重名依次加 `-1`、`-2`……
    ///
    /// # Errors
    /// `invalid_name_*`（`dir` 不合法）/ `vault_create_exhausted`。
    pub fn new_note_path(
        &self,
        dir: &str,
        text: &str,
        fallback_prefix: &str,
        at: &Civil,
    ) -> Result<String, JsError> {
        if !dir.is_empty() {
            naming::check_vault_path(dir).map_err(|err| code(err.code()))?;
        }
        let stem = naming::draft_stem(text, fallback_prefix, &at.0);
        naming::free_note_path(dir, &stem, |candidate| {
            self.view.path_taken(candidate, None)
        })
        .ok_or_else(|| code("vault_create_exhausted"))
    }
}

impl VaultSession {
    /// 一个要写入 `file_id` 的路径：合法，且没有被别的文件占着。
    fn check_target(&self, path: &str, file_id: &Uuid) -> Result<(), JsError> {
        naming::check_vault_path(path).map_err(|err| code(err.code()))?;
        if self.view.path_taken(path, Some(file_id)) {
            return Err(code("path_taken"));
        }
        Ok(())
    }

    /// 清单类响应 → 顶层对象 + 逐条解开的变更（解不开的条目跳过：一条坏条目不拖垮整棵树）。
    fn parse_entries(
        &self,
        response: &[u8],
    ) -> Result<(serde_json::Value, Vec<view::Change>), JsError> {
        let mut body: serde_json::Value =
            serde_json::from_slice(response).map_err(|_| code("sync_remote_corrupt"))?;
        let entries = body
            .get_mut("entries")
            .and_then(serde_json::Value::as_array_mut)
            .map(std::mem::take)
            .ok_or_else(|| code("sync_remote_corrupt"))?;
        let mut changes = Vec::with_capacity(entries.len());
        for item in entries {
            let Some(object) = item.as_object() else {
                continue;
            };
            let Some(file_id) = object
                .get("file_id")
                .and_then(serde_json::Value::as_str)
                .and_then(|id| Uuid::parse_str(id).ok())
            else {
                continue;
            };
            let Some(rev) = object.get("rev").and_then(serde_json::Value::as_i64) else {
                continue;
            };
            if object
                .get("deleted")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
            {
                changes.push(view::Change::Delete { file_id, rev });
                continue;
            }
            let opened = object
                .get("entry_ct")
                .and_then(serde_json::Value::as_str)
                .and_then(|b64| STANDARD.decode(b64).ok())
                .and_then(|sealed| {
                    manifest::open_entry(&self.crypto.ring, file_id, rev, &sealed).ok()
                });
            if let Some(entry) = opened {
                changes.push(view::Change::Put {
                    file_id,
                    rev,
                    entry,
                    size_padded: object
                        .get("size_padded")
                        .and_then(serde_json::Value::as_i64)
                        .unwrap_or(0),
                });
            }
        }
        Ok((body, changes))
    }
}

/// 列库时解开一个库的名字：信封只在这一次调用里用，CMK 随返回清零（不为列表常驻任何密钥）。
///
/// # Errors
/// `invalid_id` / `no_managed_key` / `sync_remote_corrupt` / `decrypt_failed`。
#[wasm_bindgen]
pub fn open_vault_name(
    vault_id: &str,
    envelopes: &[u8],
    name_ct_b64: &str,
) -> Result<String, JsError> {
    let crypto = VaultCrypto::from_envelopes(vault_id, envelopes)?;
    let sealed = STANDARD
        .decode(name_ct_b64)
        .map_err(|_| code("sync_remote_corrupt"))?;
    crypto.open_vault_name(&sealed)
}

/// 账户默认库的 id（ADR-178 ③）：规整后的邮箱取 SHA-256、截成 UUIDv8。两个标签页同时进入只会建成
/// 一个（服务端按 id 去重，回 409 `sync_vault_key_exists`）。
#[wasm_bindgen]
#[must_use]
pub fn default_vault_id(email: &str) -> String {
    let hash = ContentHash::of(format!("lunarca-default-vault\0{email}").as_bytes());
    let bytes = Uuid::parse_str(&hash.as_str()[..32])
        .expect("a SHA-256 hex digest starts with 32 hex digits")
        .into_bytes();
    uuid::Builder::from_custom_bytes(bytes)
        .into_uuid()
        .to_string()
}

/// 新建云端库的请求体（`POST /sync/v1/vaults`，ADR-150 ①，与桌面 `sync_enable` 同形）：CMK 在这里
/// 随机生成，作为托管信封经 TLS 交给服务端封存；调用方发出后把这块缓冲区清零。
///
/// # Errors
/// `invalid_id` / `internal`。
#[wasm_bindgen]
pub fn new_vault_request(vault_id: &str, name: &str) -> Result<Vec<u8>, JsError> {
    let id = parse_uuid(vault_id)?;
    let cmk = Cmk::generate(1);
    serde_json::to_vec(&json!({
        "id": id.to_string(),
        "name_ct": STANDARD.encode(cmk.derive(id).seal_vault_name(name)),
        "envelope": {
            "slot": MANAGED_SLOT,
            "cmk_version": 1,
            "wrapped_cmk": STANDARD.encode(cmk.expose_bytes()),
            "kdf_params": {},
        },
    }))
    .map_err(|_| code("internal"))
}

/// 托管槽的全部 CMK 版本（服务端为已登录设备解封后下发，ADR-150 ①）。失败只回稳定码。
fn managed_cmks(response: &[u8]) -> Result<Vec<Cmk>, &'static str> {
    #[derive(serde::Deserialize)]
    struct Envelope {
        slot: String,
        cmk_version: i64,
        wrapped_cmk: String,
    }
    #[derive(serde::Deserialize)]
    struct Envelopes {
        envelopes: Vec<Envelope>,
    }
    const CORRUPT: &str = "sync_remote_corrupt";
    let parsed: Envelopes = serde_json::from_slice(response).map_err(|_| CORRUPT)?;
    parsed
        .envelopes
        .into_iter()
        .filter(|e| e.slot == MANAGED_SLOT)
        .map(|e| {
            let version = u32::try_from(e.cmk_version).map_err(|_| CORRUPT)?;
            let bytes = Zeroizing::new(STANDARD.decode(&e.wrapped_cmk).map_err(|_| CORRUPT)?);
            let key: [u8; 32] = bytes.as_slice().try_into().map_err(|_| CORRUPT)?;
            Ok(Cmk::from_bytes(version, key))
        })
        .collect()
}

/// [`VaultCrypto::prepare`] 的结果：摘要 JSON 与各块密文。
#[wasm_bindgen]
pub struct PreparedChunks {
    summary: String,
    sealed: Vec<Vec<u8>>,
}

#[wasm_bindgen]
impl PreparedChunks {
    /// 摘要 JSON（不含密文）。
    #[must_use]
    pub fn summary(&self) -> String {
        self.summary.clone()
    }

    /// 块数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.sealed.len()
    }

    /// 是否没有块（空文件也有一个空块，故恒为 `false`；给 clippy 的配对方法）。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sealed.is_empty()
    }

    /// 第 `index` 块的密文。
    ///
    /// # Errors
    /// `out_of_range`。
    pub fn sealed(&self, index: usize) -> Result<Vec<u8>, JsError> {
        self.sealed
            .get(index)
            .cloned()
            .ok_or_else(|| code("out_of_range"))
    }
}

/// 一次准备好的提交（[`VaultSession::prepare_put`] / `prepare_rename` / `prepare_delete`）：请求体与要上传
/// 的块密文。网络失败后原样重发（同一个幂等键配同一份请求体），所以它在 Worker 里留到服务端给出结论。
#[wasm_bindgen]
pub struct PreparedCommit {
    file_id: Uuid,
    entry: Option<EntryPlain>,
    size_padded: i64,
    body: String,
    chunks: Vec<SealedChunk>,
}

impl PreparedCommit {
    fn new(
        item: &CommitItem,
        entry: Option<EntryPlain>,
        size_padded: i64,
        chunks: Vec<SealedChunk>,
    ) -> Self {
        Self {
            file_id: item.file_id,
            body: commit::body_json(std::slice::from_ref(item)),
            entry,
            size_padded,
            chunks,
        }
    }
}

#[wasm_bindgen]
impl PreparedCommit {
    #[must_use]
    pub fn file_id(&self) -> String {
        self.file_id.to_string()
    }

    /// 写入的路径（墓碑为 `undefined`）。
    #[must_use]
    pub fn path(&self) -> Option<String> {
        self.entry.as_ref().map(|entry| entry.path.clone())
    }

    /// 明文字节数（单文件上限预检用；墓碑为 0）。
    #[must_use]
    pub fn size(&self) -> f64 {
        #[allow(clippy::cast_precision_loss)]
        let size = self.entry.as_ref().map_or(0, |entry| entry.size) as f64;
        size
    }

    /// `POST …/commits` 的请求体（JSON，原样发出）。
    #[must_use]
    pub fn body(&self) -> String {
        self.body.clone()
    }

    /// 要上传的块（去重后；改名与墓碑没有）。
    #[must_use]
    pub fn chunk_ids(&self) -> Vec<String> {
        self.chunks.iter().map(|chunk| chunk.id.clone()).collect()
    }

    /// 第 `index` 块的密文。
    ///
    /// # Errors
    /// `out_of_range`。
    pub fn sealed(&self, index: usize) -> Result<Vec<u8>, JsError> {
        self.chunks
            .get(index)
            .map(|chunk| chunk.sealed.clone())
            .ok_or_else(|| code("out_of_range"))
    }

    /// 第 `index` 块密文的 sha256（小写十六进制，`x-lunarca-ct-sha256` 头）。
    ///
    /// # Errors
    /// `out_of_range`。
    pub fn ct_sha256(&self, index: usize) -> Result<String, JsError> {
        self.chunks
            .get(index)
            .map(|chunk| hex(&chunk.ct_sha256))
            .ok_or_else(|| code("out_of_range"))
    }
}

/// 一个本地时刻（Worker 从浏览器时钟取）：冲突副本名里的日期与新笔记兜底名里的时间戳都由它按
/// `luna-sync-core` 的同一种格式写出。
#[wasm_bindgen]
pub struct Civil(CivilTime);

#[wasm_bindgen]
impl Civil {
    #[wasm_bindgen(constructor)]
    #[must_use]
    pub fn new(year: i32, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> Civil {
        Self(CivilTime {
            year,
            month,
            day,
            hour,
            minute,
            second,
        })
    }

    /// `YYYY-MM-DD`。
    #[must_use]
    pub fn date(&self) -> String {
        self.0.date()
    }

    /// `YYYYMMDD_HHMMSS`。
    #[must_use]
    pub fn stamp(&self) -> String {
        self.0.stamp()
    }
}

/// 冲突副本的路径（不查撞名，给定序号；`attempt` 从 1 起）：与桌面同一条命名规则（ADR-148 ⑥）。
#[wasm_bindgen]
#[must_use]
pub fn conflict_copy_path(path: &str, label: &str, date: &str, attempt: u32) -> String {
    naming::conflict_copy_path(path, label, date, attempt)
}

/// 改名后的路径：同目录、给的名字、原扩展名（与桌面改名同一条规则）。
///
/// # Errors
/// `invalid_name_*`（名字不能用）。
#[wasm_bindgen]
pub fn renamed_path(path: &str, name: &str) -> Result<String, JsError> {
    naming::renamed_entry_path(path, name, false).map_err(|err| code(err.code()))
}

/// 移到 `dir`（库根为空串）之后的路径：文件名不变。
///
/// # Errors
/// `invalid_name_*`（`dir` 不合法）。
#[wasm_bindgen]
pub fn moved_path(path: &str, dir: &str) -> Result<String, JsError> {
    naming::moved_entry_path(path, dir).map_err(|err| code(err.code()))
}

/// 新建文件夹的路径：`dir`（库根为空串）里名为 `name` 的子文件夹。
///
/// # Errors
/// `invalid_name_*`（名字或文件夹不合法）。
#[wasm_bindgen]
pub fn child_path(dir: &str, name: &str) -> Result<String, JsError> {
    naming::child_entry_path(dir, name).map_err(|err| code(err.code()))
}

/// 一个要写入的库相对路径合不合法：合法为 `undefined`，否则是稳定码（`invalid_name_*`）。
#[wasm_bindgen]
#[must_use]
pub fn vault_path_error(path: &str) -> Option<String> {
    naming::check_vault_path(path)
        .err()
        .map(|err| err.code().to_owned())
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

/// 三路合并（与桌面同一个合并器，ADR-148 ②）。返回 JSON：`{kind:"merged",text}` /
/// `{kind:"conflict",regions}` / `{kind:"too_large"}`。
#[wasm_bindgen]
#[must_use]
pub fn merge3(base: &str, ours: &str, theirs: &str) -> String {
    match merge3::merge3(base, ours, theirs) {
        MergeOutcome::Merged(text) => json!({ "kind": "merged", "text": text }),
        MergeOutcome::Conflict { regions } => json!({ "kind": "conflict", "regions": regions }),
        MergeOutcome::TooLarge => json!({ "kind": "too_large" }),
    }
    .to_string()
}

/// 路径的 NFC 形态（只用于比较与显示，ADR-148 ⑥）。
#[wasm_bindgen]
#[must_use]
pub fn nfc(text: &str) -> String {
    path::nfc(text).into_owned()
}

/// 名字比较键：NFC + 小写（撞名判定）。
#[wasm_bindgen]
#[must_use]
pub fn name_key(text: &str) -> String {
    path::name_key(text)
}

/// 内容的 sha256（小写十六进制），与清单条目的 `content_hash` 同一口径。
#[wasm_bindgen]
#[must_use]
pub fn content_hash(bytes: &[u8]) -> String {
    manifest::content_hash(bytes)
}

/// 新文件的 `file_id`（UUIDv7，ADR-148 ③）。
#[wasm_bindgen]
#[must_use]
pub fn new_file_id() -> String {
    Uuid::now_v7().to_string()
}

/// 规整后的邮箱（OPAQUE 的 `client_identity`，ADR-158 ②）。
#[wasm_bindgen]
#[must_use]
pub fn normalize_email(raw: &str) -> String {
    luna_opaque::normalize_email(raw)
}

/// 规整验证码（去空格连字符、统一大小写）；形状不对 = `undefined`。
#[wasm_bindgen]
#[must_use]
pub fn normalize_code(raw: &str) -> Option<String> {
    luna_opaque::normalize_code(raw)
}

/// OPAQUE 注册的第一段（ADR-178 ⑤）：口令只在这里进 OPAQUE，不出 Worker。
#[wasm_bindgen]
pub struct RegistrationStart {
    state: Option<ClientRegistration<Suite>>,
    request: String,
    password: Zeroizing<Vec<u8>>,
}

#[wasm_bindgen]
impl RegistrationStart {
    /// 开始一次注册。
    ///
    /// # Errors
    /// `opaque_failed`。
    #[wasm_bindgen(constructor)]
    pub fn new(password: &str) -> Result<RegistrationStart, JsError> {
        let password = Zeroizing::new(password.as_bytes().to_vec());
        let mut rng = aes_gcm::aead::OsRng;
        let start = ClientRegistration::<Suite>::start(&mut rng, &password)
            .map_err(|_| code("opaque_failed"))?;
        Ok(Self {
            request: luna_opaque::b64u(&start.message.serialize()),
            state: Some(start.state),
            password,
        })
    }

    /// 发给服务端的 `registration_request`（base64url）。
    #[must_use]
    pub fn request(&self) -> String {
        self.request.clone()
    }

    /// 第二段：用服务端的 `registration_response` 算出 `registration_upload`（base64url）。
    ///
    /// # Errors
    /// `used` / `invalid_response` / `opaque_failed`。
    pub fn finish(
        &mut self,
        registration_response: &str,
        email: &str,
        deployment_id: &str,
    ) -> Result<String, JsError> {
        let state = self.state.take().ok_or_else(|| code("used"))?;
        let response = luna_opaque::from_b64u(registration_response)
            .and_then(|bytes| RegistrationResponse::<Suite>::deserialize(&bytes).ok())
            .ok_or_else(|| code("invalid_response"))?;
        let ksf = luna_opaque::ksf();
        let mut rng = aes_gcm::aead::OsRng;
        let finished = state
            .finish(
                &mut rng,
                &self.password,
                response,
                ClientRegistrationFinishParameters::new(
                    luna_opaque::identifiers(email, deployment_id),
                    Some(&ksf),
                ),
            )
            .map_err(|_| code("opaque_failed"))?;
        Ok(luna_opaque::b64u(&finished.message.serialize()))
    }
}

/// OPAQUE 登录的第一段：口令只在这里进 OPAQUE，不出 Worker。
#[wasm_bindgen]
pub struct LoginStart {
    state: Option<ClientLogin<Suite>>,
    request: String,
    password: Zeroizing<Vec<u8>>,
}

#[wasm_bindgen]
impl LoginStart {
    /// 开始一次登录。
    ///
    /// # Errors
    /// `opaque_failed`。
    #[wasm_bindgen(constructor)]
    pub fn new(password: &str) -> Result<LoginStart, JsError> {
        let password = Zeroizing::new(password.as_bytes().to_vec());
        let mut rng = aes_gcm::aead::OsRng;
        let start =
            ClientLogin::<Suite>::start(&mut rng, &password).map_err(|_| code("opaque_failed"))?;
        Ok(Self {
            request: luna_opaque::b64u(&start.message.serialize()),
            state: Some(start.state),
            password,
        })
    }

    /// 发给服务端的 `credential_request`（base64url）。
    #[must_use]
    pub fn request(&self) -> String {
        self.request.clone()
    }

    /// 第二段：用服务端的 `credential_response` 完成握手。返回 JSON：
    /// `{ credential_finalization }`；会话密钥留在 [`LoginStart::session_key`] 供后续派生。
    ///
    /// # Errors
    /// `invalid_response` / `invalid_credentials`（口令不对，或对端不是注册时的那台部署）/ `used`。
    pub fn finish(
        &mut self,
        credential_response: &str,
        email: &str,
        deployment_id: &str,
    ) -> Result<LoginFinish, JsError> {
        let state = self.state.take().ok_or_else(|| code("used"))?;
        let response = luna_opaque::from_b64u(credential_response)
            .and_then(|bytes| CredentialResponse::<Suite>::deserialize(&bytes).ok())
            .ok_or_else(|| code("invalid_response"))?;
        let ksf = luna_opaque::ksf();
        let mut rng = aes_gcm::aead::OsRng;
        let finished = state
            .finish(
                &mut rng,
                &self.password,
                response,
                ClientLoginFinishParameters::new(
                    Some(luna_opaque::CONTEXT),
                    luna_opaque::identifiers(email, deployment_id),
                    Some(&ksf),
                ),
            )
            .map_err(|_| code("invalid_credentials"))?;
        Ok(LoginFinish {
            finalization: luna_opaque::b64u(&finished.message.serialize()),
            session_key: Zeroizing::new(finished.session_key.to_vec()),
        })
    }
}

/// OPAQUE 登录的结果。
#[wasm_bindgen]
pub struct LoginFinish {
    finalization: String,
    session_key: Zeroizing<Vec<u8>>,
}

#[wasm_bindgen]
impl LoginFinish {
    /// 发给服务端的 `credential_finalization`（base64url）。
    #[must_use]
    pub fn finalization(&self) -> String {
        self.finalization.clone()
    }

    /// 本次握手的会话密钥长度（只给测试核对；密钥本身不出 WASM）。
    #[must_use]
    pub fn session_key_len(&self) -> usize {
        self.session_key.len()
    }
}
