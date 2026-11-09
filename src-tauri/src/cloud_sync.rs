//! 官方同步的装配层适配（ADR-148；TDD-14 §5）：把 `luna-cloud` 的同步接口客户端与凭据存储接到
//! `luna-sync` 引擎要的接口上。
//!
//! # 为什么要这层
//!
//! `luna-sync` 是中层，与 `luna-cloud` 互不依赖（TDD-00 §3）：同步核心发不出请求，「未开启同步
//! 时零云网络行为」只需在装配层证明。两边各持一份形状，这里一对一翻译、错误码表驱动映射，
//! 不猜、不吞。
//!
//! 本地库那一侧（`luna_vault::Vault` → `LocalVault`）要接自写抑制、`vault://changed` 与编辑器
//! 横幅，随界面接线一批做（S4）。

use std::sync::Arc;

use async_trait::async_trait;
use luna_cloud::client::CloudClient;
use luna_cloud::error::CloudError;
use luna_cloud::store::CredentialStore;
use luna_cloud::sync_client::SyncEntryDto;
use luna_sync::keystore::{decode, encode, keychain_entry, merged, CmkStore, KeyStoreError};
use luna_sync::ports::{Remote, RemoteChanges, RemoteEntry, RemoteError, RemoteManifest};
use luna_sync_core::commit::{CommitItem, CommitReply};
use luna_sync_core::crypto::Cmk;
use uuid::Uuid;

/// 服务端稳定码 → 引擎的远端错误（TDD-16 §9 的码表）。认不出的码原样带着。
#[must_use]
pub fn remote_error(err: CloudError) -> RemoteError {
    match err {
        CloudError::Server { error_code, .. } => match error_code.as_str() {
            "sync_epoch_changed" => RemoteError::EpochChanged,
            "sync_cursor_expired" => RemoteError::CursorExpired,
            "sync_read_only" => RemoteError::ReadOnly,
            "sync_quota_exceeded" => RemoteError::QuotaExceeded,
            "sync_not_entitled" => RemoteError::NotEntitled,
            "sync_storage_unavailable" => RemoteError::StorageUnavailable,
            "not_found" => RemoteError::NotFound,
            // 云端库被删除了（协议 v15）：不是可重试的故障，同步服务据此断开这个库。
            luna_cloud::sync_client::VAULT_DELETED => RemoteError::VaultDeleted,
            "unauthorized" | "forbidden" => RemoteError::Unauthorized,
            _ => RemoteError::Other(error_code),
        },
        // 账户被冻结（纵切 3 C3）：`check_status` 已把 403 `account_frozen` 认成它；引擎据此暂停上传、
        // 照常下载，不当成一轮失败。
        CloudError::AccountFrozen => RemoteError::AccountFrozen,
        CloudError::WebAccessUnavailable => RemoteError::Other("web_access_unavailable".into()),
        CloudError::RateLimited { retry_after_secs } => RemoteError::RateLimited {
            retry_after_secs: retry_after_secs.map(u64::from),
        },
        CloudError::NotLoggedIn
        | CloudError::NeedsRelogin
        | CloudError::NoPendingLogin
        | CloudError::LoginExpired
        | CloudError::LoginRejected { .. } => RemoteError::Unauthorized,
        CloudError::Network { kind, .. } => RemoteError::Network(network_code(kind).to_owned()),
        CloudError::StrictLocalBlocked => RemoteError::Other("strict_local_blocked".into()),
        CloudError::UnreadableResponse { .. } => RemoteError::Other("unreadable_response".into()),
        CloudError::InvalidEndpoint { .. } => RemoteError::Other("invalid_endpoint".into()),
        CloudError::TrustAnchorChanged => RemoteError::Other("trust_anchor_changed".into()),
        CloudError::TrustUnsupportedFormat { .. } => {
            RemoteError::Other("trust_unsupported_format".into())
        }
        CloudError::Keychain { .. } => RemoteError::Other("keychain".into()),
        // 以下只出现在登录流程里，同步路径上不会真的遇到；如实带码而不是吞掉。
        CloudError::AlreadySignedIn
        | CloudError::NotAnAddress
        | CloudError::NoServerAtAddress { .. }
        | CloudError::NotAnEmail
        | CloudError::LinkRejected { .. }
        | CloudError::ServerIdentityRejected
        | CloudError::NoInvite
        | CloudError::DeviceLimitExpired
        | CloudError::EmailDomainNotAllowed => RemoteError::Other("sign_in_flow".into()),
        // 只出现在收单路径上（纵切 3 C2）：同步不会遇到，同样如实带码。
        CloudError::BillingDisabled
        | CloudError::PlanUnavailable
        | CloudError::NoBillingAccount => RemoteError::Other("billing_flow".into()),
        // 只出现在注销与导出上（纵切 3 C6）：同步不会遇到，同样如实带码。
        CloudError::BillingUnavailable
        | CloudError::LastOrgOwner
        | CloudError::ExportWriteFailed { .. } => RemoteError::Other("account_data".into()),
        // 月度预付上限（纵切 3 C4）：只管推理 / 转写从预付扣的那几笔，同步存储不走它；如实带码。
        CloudError::SpendCapReached | CloudError::InvalidSpendCap => {
            RemoteError::Other("spend_cap".into())
        }
        // 免费额度与付款方式验证（纵切 3 C5）：只管推理 / 转写的预留与收单页，同步不会遇到；如实带码。
        CloudError::FreeQuotaWithheld | CloudError::VerificationNotRequired => {
            RemoteError::Other("free_quota".into())
        }
        CloudError::ClientTooOld { .. } => RemoteError::Other("client_too_old".into()),
        CloudError::ServerTooOld { .. } => RemoteError::Other("server_too_old".into()),
        CloudError::Internal { .. } => RemoteError::Other("internal".into()),
    }
}

fn network_code(kind: luna_cloud::error::NetworkFailureKind) -> &'static str {
    use luna_cloud::error::NetworkFailureKind as K;
    match kind {
        K::Dns => "dns",
        K::Refused => "refused",
        K::Timeout => "timeout",
        K::Tls => "tls",
        K::Unreachable => "unreachable",
        K::Other => "other",
    }
}

fn malformed() -> RemoteError {
    RemoteError::Other("malformed_response".into())
}

fn uuid(raw: &str) -> Result<Uuid, RemoteError> {
    Uuid::parse_str(raw).map_err(|_| malformed())
}

fn entry(dto: SyncEntryDto) -> Result<RemoteEntry, RemoteError> {
    Ok(RemoteEntry {
        file_id: uuid(&dto.file_id)?,
        rev: dto.rev,
        seq: dto.seq,
        deleted: dto.deleted,
        entry_ct: dto.entry_ct.map(|b| b.0),
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `luna-cloud` 的同步接口 → 引擎的 [`Remote`]。
pub struct CloudRemote {
    client: Arc<CloudClient>,
}

impl CloudRemote {
    #[must_use]
    pub fn new(client: Arc<CloudClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl Remote for CloudRemote {
    async fn changes(
        &self,
        vault_id: Uuid,
        epoch: Uuid,
        since: i64,
        limit: u32,
    ) -> Result<RemoteChanges, RemoteError> {
        let dto = self
            .client
            .sync_changes(&vault_id.to_string(), &epoch.to_string(), since, limit)
            .await
            .map_err(remote_error)?;
        Ok(RemoteChanges {
            epoch: uuid(&dto.epoch)?,
            entries: dto
                .entries
                .into_iter()
                .map(entry)
                .collect::<Result<_, _>>()?,
            next_seq: dto.next_seq,
            has_more: dto.has_more,
        })
    }

    async fn entry_at(
        &self,
        vault_id: Uuid,
        file_id: Uuid,
        rev: i64,
    ) -> Result<RemoteEntry, RemoteError> {
        let dto = self
            .client
            .sync_entry_at(&vault_id.to_string(), &file_id.to_string(), rev)
            .await
            .map_err(remote_error)?;
        entry(dto)
    }

    async fn manifest(
        &self,
        vault_id: Uuid,
        at_seq: Option<i64>,
    ) -> Result<RemoteManifest, RemoteError> {
        let dto = self
            .client
            .sync_manifest(&vault_id.to_string(), at_seq)
            .await
            .map_err(remote_error)?;
        Ok(RemoteManifest {
            epoch: uuid(&dto.epoch)?,
            at_seq: dto.at_seq,
            entries: dto
                .entries
                .into_iter()
                .map(entry)
                .collect::<Result<_, _>>()?,
        })
    }

    async fn missing_blobs(
        &self,
        vault_id: Uuid,
        chunk_ids: &[String],
    ) -> Result<Vec<String>, RemoteError> {
        self.client
            .sync_missing_blobs(&vault_id.to_string(), chunk_ids)
            .await
            .map_err(remote_error)
    }

    async fn put_blob(
        &self,
        vault_id: Uuid,
        chunk_id: &str,
        sealed: &[u8],
        ct_sha256: [u8; 32],
    ) -> Result<(), RemoteError> {
        self.client
            .sync_put_blob(&vault_id.to_string(), chunk_id, sealed, &hex(&ct_sha256))
            .await
            .map_err(remote_error)
    }

    async fn get_blob(&self, vault_id: Uuid, chunk_id: &str) -> Result<Vec<u8>, RemoteError> {
        self.client
            .sync_get_blob(&vault_id.to_string(), chunk_id)
            .await
            .map_err(remote_error)
    }

    async fn commit(
        &self,
        vault_id: Uuid,
        idempotency_key: &str,
        items: Vec<CommitItem>,
    ) -> Result<CommitReply, RemoteError> {
        self.client
            .sync_commit(&vault_id.to_string(), idempotency_key, &items)
            .await
            .map_err(remote_error)
    }
}

/// 账户托管信封的槽位名（ADR-150 ①；服务端 `EnvelopeSlot::Managed`）。
pub const MANAGED_SLOT: &str = "managed";

/// 取这个远端库由账户托管的全部库主密钥版本（ADR-150 ①）。服务端只把它们交给本账户的
/// 已登录设备；库里没有托管信封（端到端的高级档）→ 空列表，由调用方决定怎么说。
///
/// # Errors
/// 网络或服务端失败；信封形状不对（不是 32 字节、版本非法）→ `server: sync_remote_corrupt`。
pub async fn managed_keys(client: &CloudClient, remote: Uuid) -> Result<Vec<Cmk>, RemoteError> {
    let envelopes = client
        .sync_envelopes(&remote.to_string())
        .await
        .map_err(remote_error)?;
    envelopes
        .into_iter()
        .filter(|e| e.slot == MANAGED_SLOT)
        .map(|e| {
            let version = u32::try_from(e.cmk_version).map_err(|_| corrupt())?;
            let bytes: [u8; 32] = e
                .wrapped_cmk
                .0
                .as_slice()
                .try_into()
                .map_err(|_| corrupt())?;
            Ok(Cmk::from_bytes(version, bytes))
        })
        .collect()
}

fn corrupt() -> RemoteError {
    RemoteError::Other("sync_remote_corrupt".into())
}

/// 解封后的 CMK 存进 `luna-cloud` 的凭据存储（生产 = 本机保险箱的逻辑条目
/// `cloud/sync-cmk/<远端库 id>`，`crate::secrets`、TDD-12 §2 / §10.2）。复用同一个存储，主密钥
/// 读失败的静默期与其余云凭据同一套。
pub struct CredentialCmkStore {
    store: Arc<dyn CredentialStore>,
}

impl CredentialCmkStore {
    #[must_use]
    pub fn new(store: Arc<dyn CredentialStore>) -> Self {
        Self { store }
    }
}

fn unavailable(err: CloudError) -> KeyStoreError {
    KeyStoreError::Unavailable(match err {
        CloudError::Keychain { .. } => "keychain".into(),
        _ => "credential_store".into(),
    })
}

impl CmkStore for CredentialCmkStore {
    fn load(&self, vault_id: Uuid) -> Result<Vec<Cmk>, KeyStoreError> {
        match self
            .store
            .get(&keychain_entry(vault_id))
            .map_err(unavailable)?
        {
            Some(text) => decode(&text),
            None => Ok(Vec::new()),
        }
    }

    fn save(&self, vault_id: Uuid, cmk: &Cmk) -> Result<(), KeyStoreError> {
        let text = encode(&merged(self.load(vault_id)?, cmk));
        self.store
            .set(&keychain_entry(vault_id), &text)
            .map_err(unavailable)
    }

    fn forget(&self, vault_id: Uuid) -> Result<(), KeyStoreError> {
        self.store
            .delete(&keychain_entry(vault_id))
            .map_err(unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luna_cloud::error::NetworkFailureKind;
    use luna_cloud::store::MemoryStore;

    #[test]
    fn every_documented_sync_code_maps_to_its_own_variant() {
        let server = |code: &str| CloudError::Server {
            status: 409,
            error_code: code.to_owned(),
        };
        let table = [
            ("sync_epoch_changed", RemoteError::EpochChanged),
            ("sync_cursor_expired", RemoteError::CursorExpired),
            ("sync_read_only", RemoteError::ReadOnly),
            ("sync_quota_exceeded", RemoteError::QuotaExceeded),
            ("sync_not_entitled", RemoteError::NotEntitled),
            ("sync_storage_unavailable", RemoteError::StorageUnavailable),
            ("not_found", RemoteError::NotFound),
            ("vault_deleted", RemoteError::VaultDeleted),
            ("unauthorized", RemoteError::Unauthorized),
        ];
        for (code, want) in table {
            // 界面拿到的码与服务端的码是同一个字（`code()` 往回翻得出原码）。
            assert_eq!(want.code(), code, "{code}");
            assert_eq!(remote_error(server(code)), want, "{code}");
        }
        // 认不出的码原样带着，不落进别的分支。
        assert_eq!(
            remote_error(server("sync_chunk_mismatch")),
            RemoteError::Other("sync_chunk_mismatch".into())
        );
        assert_eq!(
            remote_error(CloudError::RateLimited {
                retry_after_secs: Some(7)
            }),
            RemoteError::RateLimited {
                retry_after_secs: Some(7)
            }
        );
        assert_eq!(
            remote_error(CloudError::NeedsRelogin),
            RemoteError::Unauthorized
        );
        // 冻结有自己的变体与稳定码，不落进 `Other` / `Unauthorized`。
        assert_eq!(
            remote_error(CloudError::AccountFrozen),
            RemoteError::AccountFrozen
        );
        assert_eq!(RemoteError::AccountFrozen.code(), "account_frozen");
        assert_eq!(
            remote_error(CloudError::Network {
                host: "h".into(),
                detail: "d".into(),
                kind: NetworkFailureKind::Timeout,
            }),
            RemoteError::Network("timeout".into())
        );
        // 界面只拿稳定码：网络原因不外露，服务端码不带 `server:` 前缀。
        assert_eq!(RemoteError::Network("timeout".into()).code(), "network");
        assert_eq!(
            remote_error(server("sync_vault_key_exists")).code(),
            "sync_vault_key_exists"
        );
        assert_eq!(
            RemoteError::RateLimited {
                retry_after_secs: None
            }
            .code(),
            "rate_limited"
        );
    }

    #[test]
    fn cmk_versions_round_trip_through_the_credential_store() {
        let store = CredentialCmkStore::new(Arc::new(MemoryStore::default()));
        let vault = Uuid::now_v7();
        assert!(store.load(vault).unwrap().is_empty());
        let (v1, v2) = (Cmk::generate(1), Cmk::generate(2));
        store.save(vault, &v1).unwrap();
        store.save(vault, &v2).unwrap();
        let back = store.load(vault).unwrap();
        assert_eq!(
            back.iter().map(Cmk::version).collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(back[1].expose_bytes(), v2.expose_bytes());
        store.forget(vault).unwrap();
        assert!(store.load(vault).unwrap().is_empty());
    }
}
