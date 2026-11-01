//! 同步面客户端（`/sync/v1/*`；接口形状与错误码只在 TDD-16 §9）。
//!
//! 只发请求、只认 wire 形状：同步逻辑在 `luna-sync`，两边互不依赖（TDD-00 §3），装配层把这里的
//! DTO 翻译成 `luna_sync::ports` 的类型。走 [`CloudClient`] 的鉴权请求，于是严格本地硬闸、401
//! 静默续期、会话被动失效的处置都与其余云请求同一份。只认设备访问令牌（服务端拒绝机器凭据）。
//!
//! 服务端的稳定码原样落在 `CloudError::Server { error_code }` 里（`sync_epoch_changed`、
//! `sync_read_only`……），由装配层表驱动映射，不在这里猜。

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::client::CloudClient;
use crate::error::CloudError;

/// JSON 里的一段二进制（标准 base64）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Base64Bytes(pub Vec<u8>);

impl Serialize for Base64Bytes {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&STANDARD.encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for Base64Bytes {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        STANDARD
            .decode(raw)
            .map(Self)
            .map_err(serde::de::Error::custom)
    }
}

/// 一个远端库（列表、建库、删除、恢复的回包同形，协议 v15）。时刻是 RFC 3339 字符串。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SyncVaultDto {
    pub id: String,
    pub epoch: String,
    pub head_seq: i64,
    /// 已存块的密文字节（含历史版本）。已删除的库照实给出——那是恢复时要加回容量的量，删除期间
    /// 它不计入账户的同步存储占用。
    pub bytes_stored: i64,
    pub name_ct: Base64Bytes,
    pub created_at: String,
    /// `head_seq` 那次提交的时刻；还没有提交 = 建库时刻（「最近更新」）。
    pub last_changed_at: String,
    #[serde(flatten)]
    pub state: SyncVaultStateDto,
}

/// 远端库的状态，按 `status` 区分（协议 v15）。认不出的状态是读不懂的回包，不猜。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SyncVaultStateDto {
    /// 可读可写。
    Active,
    /// 只读（拉取照常，提交与上传回 409 `sync_read_only`）。
    ReadOnly,
    /// 用户删除了这个库：所有设备停止同步（数据面一律 410 `vault_deleted`），本地文件不动；
    /// `purge_after` 之前可恢复，之后彻底清除。
    Deleted {
        deleted_at: String,
        purge_after: String,
    },
}

impl SyncVaultDto {
    /// 被删除了（在恢复期内）。
    #[must_use]
    pub fn is_deleted(&self) -> bool {
        matches!(self.state, SyncVaultStateDto::Deleted { .. })
    }
}

/// 服务端稳定码：库已被删除（数据面 410，协议 v15）。
pub const VAULT_DELETED: &str = "vault_deleted";

/// 这个失败是不是「库已被删除」（410 `vault_deleted`）：调用方据此断开这个库，而不是当成一次
/// 可重试的同步故障。
#[must_use]
pub fn is_vault_deleted(err: &CloudError) -> bool {
    matches!(err, CloudError::Server { status: 410, error_code } if error_code == VAULT_DELETED)
}

/// 提示流（`GET /sync/v1/vaults/{id}/events`，SSE）里认得的一帧。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncEventDto {
    /// 库有新提交（只有序号，不带条目）；连上时先发一帧当前 head。
    Head { seq: i64 },
    /// 终结帧：库被用户删除了，发完即关流（协议 v15）。
    VaultDeleted {
        deleted_at: String,
        purge_after: String,
    },
}

#[derive(Debug, Deserialize)]
struct HeadEventBody {
    seq: i64,
}

#[derive(Debug, Deserialize)]
struct VaultDeletedEventBody {
    deleted_at: String,
    purge_after: String,
}

/// SSE 字节流 → 帧（纯解析，不碰网络）。按空行切帧，`event:` / `data:` 逐行收；以 `:` 开头的
/// 注释行（保活）忽略。认不出的事件名跳过；认得的事件带着读不懂的 `data`，同样跳过——提示流
/// 只是提示，读不懂的一帧不该让整条流失败。
#[derive(Debug, Default)]
pub struct SseFrames {
    buffer: Vec<u8>,
}

impl SseFrames {
    /// 收下一段字节，返回这段字节补齐的全部帧（按到达顺序）。
    pub fn push(&mut self, bytes: &[u8]) -> Vec<SyncEventDto> {
        self.buffer.extend_from_slice(bytes);
        let mut out = Vec::new();
        while let Some((end, sep)) = frame_end(&self.buffer) {
            let frame: Vec<u8> = self.buffer.drain(..end + sep).take(end).collect();
            if let Some(event) = parse_frame(&String::from_utf8_lossy(&frame)) {
                out.push(event);
            }
        }
        out
    }
}

/// 第一处空行（`\n\n` 或 `\r\n\r\n`）：（帧尾，分隔符长度）。
fn frame_end(buffer: &[u8]) -> Option<(usize, usize)> {
    let lf = buffer
        .windows(2)
        .position(|w| w == b"\n\n")
        .map(|at| (at, 2));
    let crlf = buffer
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|at| (at, 4));
    match (lf, crlf) {
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
        (a, b) => a.or(b),
    }
}

fn parse_frame(frame: &str) -> Option<SyncEventDto> {
    let mut name = "message";
    let mut data = String::new();
    for line in frame.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with(':') {
            continue;
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => name = value,
            "data" => {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(value);
            }
            _ => {}
        }
    }
    match name {
        "head" => serde_json::from_str::<HeadEventBody>(&data)
            .ok()
            .map(|b| SyncEventDto::Head { seq: b.seq }),
        VAULT_DELETED => serde_json::from_str::<VaultDeletedEventBody>(&data)
            .ok()
            .map(|b| SyncEventDto::VaultDeleted {
                deleted_at: b.deleted_at,
                purge_after: b.purge_after,
            }),
        _ => None,
    }
}

/// 打开着的提示流。
pub struct SyncEventStream {
    response: reqwest::Response,
    frames: SseFrames,
    pending: std::collections::VecDeque<SyncEventDto>,
    api_base: String,
}

impl SyncEventStream {
    /// 下一帧；流正常结束 = `None`。`idle` 是两段字节之间最多等多久——保活注释也算字节（服务端每
    /// 30 秒一次），所以「很久没有事件」不会被当成断线，而「连字节都没有」会。
    ///
    /// # Errors
    /// 连接断开；`idle` 内一个字节都没收到 = 网络超时。
    pub async fn next_event(
        &mut self,
        idle: std::time::Duration,
    ) -> Result<Option<SyncEventDto>, CloudError> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Ok(Some(event));
            }
            let chunk = tokio::time::timeout(idle, self.response.chunk())
                .await
                .map_err(|_| CloudError::Network {
                    host: crate::config::server_bucket(&self.api_base),
                    detail: "event stream idle".into(),
                    kind: crate::error::NetworkFailureKind::Timeout,
                })?
                .map_err(|err| CloudError::network(&err, &self.api_base))?;
            let Some(chunk) = chunk else {
                return Ok(None);
            };
            self.pending.extend(self.frames.push(&chunk));
        }
    }
}

#[derive(Debug, Deserialize)]
struct VaultsResponse {
    vaults: Vec<SyncVaultDto>,
}

/// 整库时间点恢复的落点（PRD-13-007）。时刻是 RFC 3339 字符串。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SyncRestorePointDto {
    pub at_seq: i64,
    pub committed_at: Option<String>,
    pub earliest_seq: i64,
    pub earliest_at: String,
    pub head_seq: i64,
}

/// 退订 / 降级后的只读宽限（PRD-13-015）：上传被拒、拉取照常，到 `purge_after` 清除云副本。
/// 时刻是 RFC 3339 字符串（同 `SyncVaultDto::created_at`）。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SyncGraceDto {
    pub read_only_since: String,
    pub purge_after: String,
}

#[derive(Debug, Deserialize)]
struct GraceResponse {
    grace: Option<SyncGraceDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SyncEnvelopeInput {
    pub slot: String,
    pub cmk_version: i32,
    pub wrapped_cmk: Base64Bytes,
    pub kdf_params: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SyncEnvelopeDto {
    pub slot: String,
    pub cmk_version: i32,
    pub wrapped_cmk: Base64Bytes,
    pub kdf_params: serde_json::Value,
    pub created_by_device: String,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
struct EnvelopesResponse {
    envelopes: Vec<SyncEnvelopeDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SyncCreateVault {
    pub id: String,
    pub name_ct: Base64Bytes,
    pub envelope: SyncEnvelopeInput,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SyncEntryDto {
    pub file_id: String,
    pub rev: i64,
    pub seq: i64,
    pub deleted: bool,
    pub entry_ct: Option<Base64Bytes>,
    pub size_padded: i64,
    pub device_id: String,
    pub committed_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SyncChangesDto {
    pub epoch: String,
    pub entries: Vec<SyncEntryDto>,
    pub next_seq: i64,
    pub has_more: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SyncManifestDto {
    pub epoch: String,
    pub at_seq: i64,
    pub entries: Vec<SyncEntryDto>,
}

/// 一个条目的改动。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SyncChangeInput {
    Put {
        entry_ct: Base64Bytes,
        size_padded: i64,
        chunk_ids: Vec<String>,
    },
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SyncCommitItem {
    pub file_id: String,
    pub base_rev: Option<i64>,
    pub change: SyncChangeInput,
}

#[derive(Debug, Serialize)]
struct CommitBody<'a> {
    items: &'a [SyncCommitItem],
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum SyncItemResultDto {
    Committed {
        file_id: String,
        rev: i64,
        seq: i64,
    },
    StaleBase {
        file_id: String,
        current_rev: Option<i64>,
    },
    MissingBlobs {
        file_id: String,
        chunk_ids: Vec<String>,
    },
    /// 超过单文件上限（ADR-156 ④；服务端按引用块的密文字节求和判）：这一条没提交，其余照常。
    TooLarge {
        file_id: String,
        max_bytes: i64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SyncCommitReplyDto {
    pub head_seq: i64,
    pub results: Vec<SyncItemResultDto>,
}

#[derive(Debug, Serialize)]
struct MissingBody<'a> {
    chunk_ids: &'a [String],
}

#[derive(Debug, Deserialize)]
struct MissingResponse {
    missing: Vec<String>,
}

#[derive(Debug, Serialize)]
struct AckBody {
    seq: i64,
}

/// 上传块时给服务端校验的密文 sha256 头。
pub const CT_SHA256_HEADER: &str = "x-lunarca-ct-sha256";
/// 提交的幂等键头。
pub const IDEMPOTENCY_KEY_HEADER: &str = "idempotency-key";

impl CloudClient {
    async fn sync_json<T: serde::de::DeserializeOwned>(
        &self,
        response: reqwest::Response,
    ) -> Result<T, CloudError> {
        response
            .json()
            .await
            .map_err(|err| CloudError::body(&err, &self.config().api_base))
    }

    /// 本账户的远端库。
    pub async fn sync_vaults(&self) -> Result<Vec<SyncVaultDto>, CloudError> {
        let response = self
            .authed_with(reqwest::Method::GET, "/sync/v1/vaults", |r| r)
            .await?;
        Ok(self.sync_json::<VaultsResponse>(response).await?.vaults)
    }

    /// 删除云端库（协议 v15）：所有设备停止同步、各设备的本地文件不动；30 天内可恢复，删除即不计
    /// 容量。幂等（删过的再删回同一份）。成员删组织库 → `Server { 403, forbidden }`；看不到 / 清除中
    /// → `Server { 404, not_found }`。
    pub async fn sync_delete_vault(&self, vault_id: &str) -> Result<SyncVaultDto, CloudError> {
        let response = self
            .authed_with(
                reqwest::Method::DELETE,
                &format!("/sync/v1/vaults/{vault_id}"),
                |r| r,
            )
            .await?;
        self.sync_json(response).await
    }

    /// 恢复已删除的云端库（协议 v15）。没删 → `Server { 409, vault_not_deleted }`；已过恢复期 →
    /// `Server { 404, not_found }`；恢复会超容量 → `Server { 402, sync_quota_exceeded }`。
    pub async fn sync_restore_vault(&self, vault_id: &str) -> Result<SyncVaultDto, CloudError> {
        let response = self
            .authed_with(
                reqwest::Method::POST,
                &format!("/sync/v1/vaults/{vault_id}/restore"),
                |r| r,
            )
            .await?;
        self.sync_json(response).await
    }

    /// 打开一个库的提示流（SSE）。库已被删除 → `Server { 410, vault_deleted }`（[`is_vault_deleted`]）。
    pub async fn sync_events(&self, vault_id: &str) -> Result<SyncEventStream, CloudError> {
        let response = self
            .authed_with(
                reqwest::Method::GET,
                &format!("/sync/v1/vaults/{vault_id}/events"),
                |r| r.header(reqwest::header::ACCEPT, "text/event-stream"),
            )
            .await?;
        Ok(SyncEventStream {
            response,
            frames: SseFrames::default(),
            pending: std::collections::VecDeque::new(),
            api_base: self.config().api_base.clone(),
        })
    }

    /// 只读宽限（不在宽限中 → `None`）。服务端首次查到失去同步权益即开始计时。
    pub async fn sync_grace(&self) -> Result<Option<SyncGraceDto>, CloudError> {
        let response = self
            .authed_with(reqwest::Method::GET, "/sync/v1/grace", |r| r)
            .await?;
        Ok(self.sync_json::<GraceResponse>(response).await?.grace)
    }

    /// 建库并写首个信封。库已有主密钥 → `Server { 409, sync_vault_key_exists }`。
    pub async fn sync_create_vault(
        &self,
        request: &SyncCreateVault,
    ) -> Result<SyncVaultDto, CloudError> {
        let response = self
            .authed_with(reqwest::Method::POST, "/sync/v1/vaults", |r| {
                r.json(request)
            })
            .await?;
        self.sync_json(response).await
    }

    pub async fn sync_envelopes(&self, vault_id: &str) -> Result<Vec<SyncEnvelopeDto>, CloudError> {
        let response = self
            .authed_with(
                reqwest::Method::GET,
                &format!("/sync/v1/vaults/{vault_id}/envelopes"),
                |r| r,
            )
            .await?;
        Ok(self
            .sync_json::<EnvelopesResponse>(response)
            .await?
            .envelopes)
    }

    pub async fn sync_put_envelope(
        &self,
        vault_id: &str,
        envelope: &SyncEnvelopeInput,
    ) -> Result<(), CloudError> {
        self.authed_with(
            reqwest::Method::PUT,
            &format!("/sync/v1/vaults/{vault_id}/envelopes"),
            |r| r.json(envelope),
        )
        .await?;
        Ok(())
    }

    pub async fn sync_changes(
        &self,
        vault_id: &str,
        epoch: &str,
        since: i64,
        limit: u32,
    ) -> Result<SyncChangesDto, CloudError> {
        let path = format!("/sync/v1/vaults/{vault_id}/changes");
        let query = [
            ("epoch", epoch.to_owned()),
            ("since", since.to_string()),
            ("limit", limit.to_string()),
        ];
        let response = self
            .authed_with(reqwest::Method::GET, &path, |r| r.query(&query))
            .await?;
        self.sync_json(response).await
    }

    /// 某个文件某一版的条目（录音按需下载：仅云端的文件要它的块列表）。没有这一版 → 404。
    pub async fn sync_entry_at(
        &self,
        vault_id: &str,
        file_id: &str,
        rev: i64,
    ) -> Result<SyncEntryDto, CloudError> {
        let path = format!("/sync/v1/vaults/{vault_id}/history/{file_id}/{rev}");
        let response = self.authed_with(reqwest::Method::GET, &path, |r| r).await?;
        self.sync_json(response).await
    }

    /// 全量清单；`at_seq` 给了就是那个序号时的库（整库时间点恢复）。
    pub async fn sync_manifest(
        &self,
        vault_id: &str,
        at_seq: Option<i64>,
    ) -> Result<SyncManifestDto, CloudError> {
        let path = match at_seq {
            Some(at) => format!("/sync/v1/vaults/{vault_id}/manifest?at_seq={at}"),
            None => format!("/sync/v1/vaults/{vault_id}/manifest"),
        };
        let response = self.authed_with(reqwest::Method::GET, &path, |r| r).await?;
        self.sync_json(response).await
    }

    /// 整库时间点恢复：`at`（RFC 3339）那一刻对应的序号与可恢复的最早时刻。早于历史保留下限 →
    /// `Server { 410, sync_cursor_expired }`。
    pub async fn sync_restore_point(
        &self,
        vault_id: &str,
        at: &str,
    ) -> Result<SyncRestorePointDto, CloudError> {
        let at: String = at
            .chars()
            .map(|c| {
                if c == ':' {
                    "%3A".to_owned()
                } else if c == '+' {
                    "%2B".to_owned()
                } else {
                    c.to_string()
                }
            })
            .collect();
        let response = self
            .authed_with(
                reqwest::Method::GET,
                &format!("/sync/v1/vaults/{vault_id}/restore-point?at={at}"),
                |r| r,
            )
            .await?;
        self.sync_json(response).await
    }

    pub async fn sync_missing_blobs(
        &self,
        vault_id: &str,
        chunk_ids: &[String],
    ) -> Result<Vec<String>, CloudError> {
        let body = MissingBody { chunk_ids };
        let response = self
            .authed_with(
                reqwest::Method::POST,
                &format!("/sync/v1/vaults/{vault_id}/blobs/missing"),
                |r| r.json(&body),
            )
            .await?;
        Ok(self.sync_json::<MissingResponse>(response).await?.missing)
    }

    /// 上传一块密文（`ct_sha256_hex` = 密文的 sha256，服务端校验）。
    pub async fn sync_put_blob(
        &self,
        vault_id: &str,
        chunk_id: &str,
        sealed: &[u8],
        ct_sha256_hex: &str,
    ) -> Result<(), CloudError> {
        self.authed_with(
            reqwest::Method::PUT,
            &format!("/sync/v1/vaults/{vault_id}/blobs/{chunk_id}"),
            |r| {
                r.header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
                    .header(CT_SHA256_HEADER, ct_sha256_hex)
                    .body(sealed.to_vec())
            },
        )
        .await?;
        Ok(())
    }

    pub async fn sync_get_blob(
        &self,
        vault_id: &str,
        chunk_id: &str,
    ) -> Result<Vec<u8>, CloudError> {
        let response = self
            .authed_with(
                reqwest::Method::GET,
                &format!("/sync/v1/vaults/{vault_id}/blobs/{chunk_id}"),
                |r| r,
            )
            .await?;
        response
            .bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|err| CloudError::body(&err, &self.config().api_base))
    }

    /// 批量提交。重放同一个幂等键返回原结果。
    pub async fn sync_commit(
        &self,
        vault_id: &str,
        idempotency_key: &str,
        items: &[SyncCommitItem],
    ) -> Result<SyncCommitReplyDto, CloudError> {
        let body = CommitBody { items };
        let response = self
            .authed_with(
                reqwest::Method::POST,
                &format!("/sync/v1/vaults/{vault_id}/commits"),
                |r| {
                    r.header(IDEMPOTENCY_KEY_HEADER, idempotency_key)
                        .json(&body)
                },
            )
            .await?;
        self.sync_json(response).await
    }

    /// 这台设备应用到的序号（「最后同步」）。
    pub async fn sync_ack(&self, vault_id: &str, seq: i64) -> Result<(), CloudError> {
        let body = AckBody { seq };
        self.authed_with(
            reqwest::Method::POST,
            &format!("/sync/v1/vaults/{vault_id}/ack"),
            |r| r.json(&body),
        )
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::client::TokenResponse;
    use crate::store::MemoryStore;

    /// 一个按顺序回答的假服务端：记下每个请求（请求行 + 头 + 体），回给定的状态与体。
    /// 假服务端记下的请求：（请求行 + 头，请求体）。
    type Seen = Arc<Mutex<Vec<(String, Vec<u8>)>>>;
    /// 一条预备的回答：（状态行，体，Content-Type）。
    type Reply = (&'static str, Vec<u8>, &'static str);

    fn fake_server(replies: Vec<Reply>) -> (String, Seen, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        let handle = std::thread::spawn(move || {
            for (status, body, content_type) in replies {
                let (mut stream, _) = listener.accept().unwrap();
                let mut raw = Vec::new();
                let mut buffer = [0_u8; 8192];
                let (head, body_start) = loop {
                    let n = stream.read(&mut buffer).unwrap();
                    raw.extend_from_slice(&buffer[..n]);
                    if let Some(at) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                        break (String::from_utf8_lossy(&raw[..at]).into_owned(), at + 4);
                    }
                };
                let length = head
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                while raw.len() < body_start + length {
                    let n = stream.read(&mut buffer).unwrap();
                    raw.extend_from_slice(&buffer[..n]);
                }
                log.lock()
                    .unwrap()
                    .push((head, raw[body_start..body_start + length].to_vec()));
                let mut response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .into_bytes();
                response.extend_from_slice(&body);
                stream.write_all(&response).unwrap();
            }
        });
        (format!("http://{address}"), seen, handle)
    }

    fn logged_in(api_base: &str) -> CloudClient {
        let client = {
            let client = CloudClient::new(Box::new(MemoryStore::default()), Arc::new(|| false));
            client.account().set_server(crate::account::AccountServer {
                origin: api_base.to_owned(),
                display_name: None,
            });
            client.reconfigure();
            client
        };
        client
            .store_token_pair(&TokenResponse {
                access_token: "device-access".into(),
                refresh_token: "device-refresh".into(),
            })
            .unwrap();
        client
    }

    const JSON: &str = "application/json";

    #[tokio::test]
    async fn the_sync_requests_carry_the_right_shapes() {
        let entry_ct = STANDARD.encode([7_u8; 20]);
        let replies = vec![
            ("200 OK", format!(r#"{{"epoch":"e1","entries":[{{"file_id":"f1","rev":2,"seq":5,"deleted":false,"entry_ct":"{entry_ct}","size_padded":4096,"device_id":"d","committed_at":"t"}}],"next_seq":5,"has_more":false}}"#).into_bytes(), JSON),
            ("200 OK", br#"{"missing":["b"]}"#.to_vec(), JSON),
            ("201 Created", Vec::new(), JSON),
            ("200 OK", vec![1, 2, 3, 4], "application/octet-stream"),
            ("200 OK", br#"{"head_seq":6,"results":[{"outcome":"committed","file_id":"f1","rev":3,"seq":6},{"outcome":"stale_base","file_id":"f2","current_rev":null}]}"#.to_vec(), JSON),
            ("409 Conflict", br#"{"error":"sync_epoch_changed"}"#.to_vec(), JSON),
        ];
        let (base, seen, server) = fake_server(replies);
        let client = logged_in(&base);

        let changes = client.sync_changes("v1", "e1", 4, 500).await.unwrap();
        assert_eq!(changes.entries[0].entry_ct, Some(Base64Bytes(vec![7; 20])));
        let missing = client
            .sync_missing_blobs("v1", &["a".into(), "b".into()])
            .await
            .unwrap();
        assert_eq!(missing, vec!["b".to_owned()]);
        client
            .sync_put_blob("v1", "b", &[9, 9], "ab".repeat(32).as_str())
            .await
            .unwrap();
        assert_eq!(
            client.sync_get_blob("v1", "b").await.unwrap(),
            vec![1, 2, 3, 4]
        );
        let reply = client
            .sync_commit(
                "v1",
                "key-1",
                &[
                    SyncCommitItem {
                        file_id: "f1".into(),
                        base_rev: Some(2),
                        change: SyncChangeInput::Put {
                            entry_ct: Base64Bytes(vec![1; 3]),
                            size_padded: 4096,
                            chunk_ids: vec!["b".into()],
                        },
                    },
                    SyncCommitItem {
                        file_id: "f2".into(),
                        base_rev: Some(1),
                        change: SyncChangeInput::Delete,
                    },
                ],
            )
            .await
            .unwrap();
        assert_eq!(
            reply.results[1],
            SyncItemResultDto::StaleBase {
                file_id: "f2".into(),
                current_rev: None
            }
        );
        let err = client.sync_manifest("v1", None).await.unwrap_err();
        assert!(
            matches!(err, CloudError::Server { status: 409, ref error_code } if error_code == "sync_epoch_changed"),
            "{err:?}"
        );
        server.join().unwrap();

        let seen = seen.lock().unwrap();
        let (head, _) = &seen[0];
        assert!(
            head.starts_with("GET /sync/v1/vaults/v1/changes?epoch=e1&since=4&limit=500 "),
            "{head}"
        );
        assert!(head
            .to_ascii_lowercase()
            .contains("authorization: bearer device-access"));
        let (head, body) = &seen[1];
        assert!(head.starts_with("POST /sync/v1/vaults/v1/blobs/missing "));
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(body).unwrap()["chunk_ids"],
            serde_json::json!(["a", "b"])
        );
        let (head, body) = &seen[2];
        let lower = head.to_ascii_lowercase();
        assert!(head.starts_with("PUT /sync/v1/vaults/v1/blobs/b "));
        assert!(lower.contains(&format!("x-lunarca-ct-sha256: {}", "ab".repeat(32))));
        assert!(lower.contains("content-type: application/octet-stream"));
        assert_eq!(body, &vec![9, 9]);
        let (head, body) = &seen[4];
        assert!(head.to_ascii_lowercase().contains("idempotency-key: key-1"));
        let json: serde_json::Value = serde_json::from_slice(body).unwrap();
        assert_eq!(json["items"][0]["change"]["kind"], "put");
        assert_eq!(
            json["items"][0]["change"]["entry_ct"],
            STANDARD.encode([1_u8; 3])
        );
        assert_eq!(
            json["items"][1]["change"],
            serde_json::json!({"kind": "delete"})
        );
        assert_eq!(json["items"][1]["base_rev"], 1);
    }

    fn vault_json(status: &str, extra: &str) -> String {
        format!(
            r#"{{"id":"v1","epoch":"e1","head_seq":3,"bytes_stored":4096,"name_ct":"AAEC","created_at":"2026-10-01T00:00:00Z","last_changed_at":"2026-10-02T00:00:00Z","status":"{status}"{extra}}}"#
        )
    }

    /// 协议 v15 的库形状：三种状态都读得出；已删除必带清除日期，少一格、少 `last_changed_at`、
    /// 认不出的状态都是读不懂的回包（不猜、不留可选字段过渡）。
    #[test]
    fn the_vault_shape_is_v15_and_the_deleted_variant_requires_its_dates() {
        let active: SyncVaultDto = serde_json::from_str(&vault_json("active", "")).unwrap();
        assert_eq!(active.state, SyncVaultStateDto::Active);
        assert_eq!(active.last_changed_at, "2026-10-02T00:00:00Z");
        assert_eq!(active.name_ct, Base64Bytes(vec![0, 1, 2]));
        assert!(!active.is_deleted());
        let read_only: SyncVaultDto = serde_json::from_str(&vault_json("read_only", "")).unwrap();
        assert_eq!(read_only.state, SyncVaultStateDto::ReadOnly);
        let deleted: SyncVaultDto = serde_json::from_str(&vault_json(
            "deleted",
            r#","deleted_at":"2026-10-15T00:00:00Z","purge_after":"2026-11-14T00:00:00Z""#,
        ))
        .unwrap();
        assert!(deleted.is_deleted());
        assert_eq!(
            deleted.state,
            SyncVaultStateDto::Deleted {
                deleted_at: "2026-10-15T00:00:00Z".into(),
                purge_after: "2026-11-14T00:00:00Z".into(),
            }
        );
        assert!(serde_json::from_str::<SyncVaultDto>(&vault_json(
            "deleted",
            r#","deleted_at":"2026-10-15T00:00:00Z""#
        ))
        .is_err());
        assert!(serde_json::from_str::<SyncVaultDto>(&vault_json("deleted", "")).is_err());
        assert!(serde_json::from_str::<SyncVaultDto>(&vault_json("purging", "")).is_err());
        let without_changed =
            vault_json("active", "").replace(r#","last_changed_at":"2026-10-02T00:00:00Z""#, "");
        assert!(serde_json::from_str::<SyncVaultDto>(&without_changed).is_err());
    }

    /// 删除 / 恢复 / 列表的请求形状，以及服务端各稳定码原样落进 `Server { status, error_code }`。
    #[tokio::test]
    async fn delete_restore_and_list_carry_the_right_shapes_and_codes() {
        let deleted = vault_json(
            "deleted",
            r#","deleted_at":"2026-10-15T00:00:00Z","purge_after":"2026-11-14T00:00:00Z""#,
        );
        let replies = vec![
            (
                "200 OK",
                format!(r#"{{"vaults":[{}, {deleted}]}}"#, vault_json("active", "")).into_bytes(),
                JSON,
            ),
            ("200 OK", deleted.clone().into_bytes(), JSON),
            ("200 OK", vault_json("active", "").into_bytes(), JSON),
            ("403 Forbidden", br#"{"error":"forbidden"}"#.to_vec(), JSON),
            (
                "409 Conflict",
                br#"{"error":"vault_not_deleted"}"#.to_vec(),
                JSON,
            ),
            ("404 Not Found", br#"{"error":"not_found"}"#.to_vec(), JSON),
            (
                "402 Payment Required",
                br#"{"error":"sync_quota_exceeded"}"#.to_vec(),
                JSON,
            ),
            ("410 Gone", br#"{"error":"vault_deleted"}"#.to_vec(), JSON),
            (
                "403 Forbidden",
                br#"{"error":"account_frozen"}"#.to_vec(),
                JSON,
            ),
        ];
        let (base, seen, server) = fake_server(replies);
        let client = logged_in(&base);

        let vaults = client.sync_vaults().await.unwrap();
        assert_eq!(vaults.len(), 2);
        assert!(!vaults[0].is_deleted() && vaults[1].is_deleted());
        assert!(client.sync_delete_vault("v1").await.unwrap().is_deleted());
        assert!(!client.sync_restore_vault("v1").await.unwrap().is_deleted());
        let code = |err: CloudError| match err {
            CloudError::Server { status, error_code } => (status, error_code),
            other => panic!("{other:?}"),
        };
        assert_eq!(
            code(client.sync_delete_vault("v1").await.unwrap_err()),
            (403, "forbidden".to_owned())
        );
        assert_eq!(
            code(client.sync_restore_vault("v1").await.unwrap_err()),
            (409, "vault_not_deleted".to_owned())
        );
        assert_eq!(
            code(client.sync_restore_vault("v1").await.unwrap_err()),
            (404, "not_found".to_owned())
        );
        assert_eq!(
            code(client.sync_restore_vault("v1").await.unwrap_err()),
            (402, "sync_quota_exceeded".to_owned())
        );
        let gone = client.sync_changes("v1", "e1", 0, 10).await.unwrap_err();
        assert!(is_vault_deleted(&gone), "{gone:?}");
        let frozen = client.sync_delete_vault("v1").await.unwrap_err();
        assert!(matches!(frozen, CloudError::AccountFrozen), "{frozen:?}");
        assert!(!is_vault_deleted(&frozen));
        server.join().unwrap();

        let seen = seen.lock().unwrap();
        assert!(
            seen[0].0.starts_with("GET /sync/v1/vaults "),
            "{}",
            seen[0].0
        );
        assert!(
            seen[1].0.starts_with("DELETE /sync/v1/vaults/v1 "),
            "{}",
            seen[1].0
        );
        assert!(
            seen[2].0.starts_with("POST /sync/v1/vaults/v1/restore "),
            "{}",
            seen[2].0
        );
    }

    /// 提示流：按空行切帧，保活注释与认不出的事件跳过，跨块拼帧；终结帧带清除日期。
    #[test]
    fn sse_frames_are_split_on_blank_lines_and_unknown_events_are_skipped() {
        let mut frames = SseFrames::default();
        assert!(frames
            .push(b": keep-alive\n\nevent: head\ndata: {\"se")
            .is_empty());
        assert_eq!(
            frames.push(b"q\":7}\n\nevent: other\ndata: {}\n\n"),
            vec![SyncEventDto::Head { seq: 7 }]
        );
        assert_eq!(
            frames.push(
                b"event: vault_deleted\r\ndata: {\"deleted_at\":\"a\",\"purge_after\":\"b\"}\r\n\r\n"
            ),
            vec![SyncEventDto::VaultDeleted {
                deleted_at: "a".into(),
                purge_after: "b".into()
            }]
        );
        // 认得的事件、读不懂的 data：跳过这一帧，不让整条流失败。
        assert!(frames.push(b"event: head\ndata: nope\n\n").is_empty());
    }

    /// 连上提示流：先 head、再终结帧、随后关流；库已删除时打开就是 410 `vault_deleted`。
    #[tokio::test]
    async fn the_event_stream_delivers_the_terminal_frame_then_ends() {
        let body = b"event: head\ndata: {\"seq\":3}\n\nevent: vault_deleted\ndata: {\"deleted_at\":\"2026-10-15T00:00:00Z\",\"purge_after\":\"2026-11-14T00:00:00Z\"}\n\n".to_vec();
        let replies = vec![
            ("200 OK", body, "text/event-stream"),
            ("410 Gone", br#"{"error":"vault_deleted"}"#.to_vec(), JSON),
        ];
        let (base, seen, server) = fake_server(replies);
        let client = logged_in(&base);
        let mut stream = client.sync_events("v1").await.unwrap();
        let idle = std::time::Duration::from_secs(5);
        assert_eq!(
            stream.next_event(idle).await.unwrap(),
            Some(SyncEventDto::Head { seq: 3 })
        );
        assert!(matches!(
            stream.next_event(idle).await.unwrap(),
            Some(SyncEventDto::VaultDeleted { .. })
        ));
        assert_eq!(stream.next_event(idle).await.unwrap(), None);
        let gone = client.sync_events("v1").await.err().unwrap();
        assert!(is_vault_deleted(&gone), "{gone:?}");
        server.join().unwrap();
        let seen = seen.lock().unwrap();
        assert!(seen[0].0.starts_with("GET /sync/v1/vaults/v1/events "));
        assert!(seen[0]
            .0
            .to_ascii_lowercase()
            .contains("accept: text/event-stream"));
    }

    #[tokio::test]
    async fn strict_local_mode_sends_nothing() {
        let client = {
            let client = CloudClient::new(Box::new(MemoryStore::default()), Arc::new(|| true));
            client.account().set_server(crate::account::AccountServer {
                origin: "http://127.0.0.1:9".into(),
                display_name: None,
            });
            client.reconfigure();
            client
        };
        assert!(matches!(
            client.sync_vaults().await,
            Err(CloudError::StrictLocalBlocked)
        ));
    }
}
