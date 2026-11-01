//! 同步的客户端加密（TDD-12 §10.2、ADR-148、ADR-150）。存储只见本模块的输出。
//!
//! # 层级
//!
//! ```text
//! 账户托管（默认，ADR-150）：CMK_v 原样经 TLS 交服务端，由同步面主密钥封存——不经本模块
//! 恢复密钥（256 位）──HKDF──▶ KEK_rk ──封装──▶ CMK_v   （端到端的高级档，未接线）
//! CMK_v ──HKDF（salt = 库 id）──▶ 内容 / 清单 / 块 id / 块 nonce 四把子密钥
//! ```
//!
//! 同步不设第二套密码：密码派生（Argon2id）随 ADR-150 删除。
//!
//! # 密文格式（全部同一种外形）
//!
//! `[格式版本 1B][CMK 版本 4B 大端][nonce 12B][AES-256-GCM 密文 + 16B 标签]`。前 17 字节是明文
//! 头，读的一方据 CMK 版本挑钥匙；头本身也进附加认证数据，改一位就认证失败。
//!
//! # 附加认证数据（把密文钉在它该在的地方）
//!
//! 头 ‖ 用途 ‖ 库 id ‖ 用途相关的绑定：
//! - **块**：块 id。块按内容寻址、可跨文件去重，所以绑的是「这是哪一块」而不是「哪个文件的
//!   第几块」——文件与块的对应关系在条目里，条目自己另有绑定。
//! - **条目**：file_id ‖ rev。服务端拿旧版本的条目冒充新版本、或挪给别的文件，都会认证失败。
//! - **库名**、**信封**（槽位）同理。
//!
//! # 块用确定性 nonce
//!
//! 块的 nonce = HMAC(块 nonce 密钥, 块 id) 的前 12 字节，于是同一份明文永远得到同一份密文：
//! 上传天然幂等，两台设备并发传同一块也不会在服务端撞成「同 id 不同密文」。块 id 本身是明文的
//! HMAC，相同 nonce 只会出现在相同明文上，GCM 的 nonce 复用禁忌不被触发。代价是同一库内
//! 「哪些块相同」对服务端可见——它本来就从块 id 看得见，没有多泄露什么。条目、库名、信封仍用
//! 随机 nonce。

use aes_gcm::aead::{rand_core::RngCore, Aead, AeadCore, KeyInit, OsRng, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use uuid::Uuid;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::recovery::RecoveryKey;

/// 当前密文格式版本。
pub const FORMAT_V1: u8 = 1;
const HEADER_LEN: usize = 1 + 4 + NONCE_LEN;
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;

/// 加解密的失败。都不含任何明文、密钥或密码。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CryptoError {
    #[error("ciphertext malformed")]
    Malformed,
    #[error("unsupported format version {0}")]
    UnsupportedFormat(u8),
    /// 密文用的是另一个版本的 CMK：调用方按版本取对的钥匙再试。
    #[error("ciphertext uses cmk version {found}, key is {expected}")]
    KeyVersion { expected: u32, found: u32 },
    /// 认证失败：钥匙不对、密文被改过、或被挪到了别处（附加认证数据不符）。
    #[error("authentication failed")]
    Authentication,
    /// 块解开了，但明文的 HMAC 与块 id 不符。
    #[error("chunk id mismatch")]
    ChunkIdMismatch,
}

/// 32 字节密钥材料，离开作用域即清零。
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
struct SecretKey([u8; 32]);

impl SecretKey {
    fn random() -> Self {
        let mut bytes = [0_u8; 32];
        OsRng.fill_bytes(&mut bytes);
        Self(bytes)
    }

    fn cipher(&self) -> Aes256Gcm {
        Aes256Gcm::new((&self.0).into())
    }

    fn hmac(&self, data: &[u8]) -> [u8; 32] {
        // HMAC 接受任意长度的钥匙，32 字节永远合法。
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&self.0).expect("hmac key");
        mac.update(data);
        mac.finalize().into_bytes().into()
    }
}

/// 用途标签：进附加认证数据，一种用途的密文不能冒充另一种。
#[derive(Clone, Copy)]
#[repr(u8)]
enum Purpose {
    Chunk = 1,
    Entry = 2,
    VaultName = 3,
    Envelope = 4,
}

fn header(cmk_version: u32, nonce: &[u8; NONCE_LEN]) -> [u8; HEADER_LEN] {
    let mut h = [0_u8; HEADER_LEN];
    h[0] = FORMAT_V1;
    h[1..5].copy_from_slice(&cmk_version.to_be_bytes());
    h[5..].copy_from_slice(nonce);
    h
}

fn aad(header: &[u8], purpose: Purpose, vault_id: Uuid, binding: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(header.len() + 1 + 16 + binding.len());
    out.extend_from_slice(header);
    out.push(purpose as u8);
    out.extend_from_slice(vault_id.as_bytes());
    out.extend_from_slice(binding);
    out
}

fn seal(
    key: &SecretKey,
    cmk_version: u32,
    nonce: [u8; NONCE_LEN],
    purpose: Purpose,
    vault_id: Uuid,
    binding: &[u8],
    plaintext: &[u8],
) -> Vec<u8> {
    let head = header(cmk_version, &nonce);
    let ct = key
        .cipher()
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad: &aad(&head, purpose, vault_id, binding),
            },
        )
        .expect("aes-gcm encryption of an in-memory buffer cannot fail");
    let mut out = Vec::with_capacity(HEADER_LEN + ct.len());
    out.extend_from_slice(&head);
    out.extend_from_slice(&ct);
    out
}

/// 读密文头里的 CMK 版本（挑钥匙用）。
pub fn ciphertext_cmk_version(data: &[u8]) -> Result<u32, CryptoError> {
    if data.len() < HEADER_LEN + TAG_LEN {
        return Err(CryptoError::Malformed);
    }
    if data[0] != FORMAT_V1 {
        return Err(CryptoError::UnsupportedFormat(data[0]));
    }
    Ok(u32::from_be_bytes([data[1], data[2], data[3], data[4]]))
}

fn open(
    key: &SecretKey,
    cmk_version: u32,
    purpose: Purpose,
    vault_id: Uuid,
    binding: &[u8],
    data: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    let found = ciphertext_cmk_version(data)?;
    if found != cmk_version {
        return Err(CryptoError::KeyVersion {
            expected: cmk_version,
            found,
        });
    }
    let (head, ct) = data.split_at(HEADER_LEN);
    key.cipher()
        .decrypt(
            Nonce::from_slice(&head[5..]),
            Payload {
                msg: ct,
                aad: &aad(head, purpose, vault_id, binding),
            },
        )
        .map_err(|_| CryptoError::Authentication)
}

fn random_nonce() -> [u8; NONCE_LEN] {
    Aes256Gcm::generate_nonce(&mut OsRng).into()
}

/// 一个远端库的内容主密钥（CMK）的某个版本。只在本机钥匙串与内存里存在，绝不出设备。
#[derive(Clone)]
pub struct Cmk {
    version: u32,
    key: SecretKey,
}

impl std::fmt::Debug for Cmk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cmk")
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

impl Cmk {
    /// 生成一把新的 CMK（建库或轮换）。
    #[must_use]
    pub fn generate(version: u32) -> Self {
        Self {
            version,
            key: SecretKey::random(),
        }
    }

    /// 从钥匙串里存的字节还原。
    #[must_use]
    pub fn from_bytes(version: u32, bytes: [u8; 32]) -> Self {
        Self {
            version,
            key: SecretKey(bytes),
        }
    }

    #[must_use]
    pub fn version(&self) -> u32 {
        self.version
    }

    /// 原始字节——只给本机钥匙串持久化用。
    #[must_use]
    pub fn expose_bytes(&self) -> &[u8; 32] {
        &self.key.0
    }

    /// 派生这个库的四把子密钥。
    #[must_use]
    pub fn derive(&self, vault_id: Uuid) -> VaultKeys {
        let hk = Hkdf::<Sha256>::new(Some(vault_id.as_bytes()), &self.key.0);
        let sub = |label: &[u8]| {
            let mut out = [0_u8; 32];
            let mut info = b"lunarca-sync/v1/".to_vec();
            info.extend_from_slice(label);
            hk.expand(&info, &mut out)
                .expect("32 bytes is a valid hkdf length");
            SecretKey(out)
        };
        VaultKeys {
            vault_id,
            cmk_version: self.version,
            content: sub(b"content"),
            manifest: sub(b"manifest"),
            chunk_id: sub(b"chunk-id"),
            chunk_nonce: sub(b"chunk-nonce"),
        }
    }
}

/// 一个库在某个 CMK 版本下的子密钥。
pub struct VaultKeys {
    vault_id: Uuid,
    cmk_version: u32,
    content: SecretKey,
    manifest: SecretKey,
    chunk_id: SecretKey,
    chunk_nonce: SecretKey,
}

impl VaultKeys {
    #[must_use]
    pub fn cmk_version(&self) -> u32 {
        self.cmk_version
    }

    /// 块 id：明文块的 HMAC（64 位小写十六进制）。只在同一个库的密钥域内去重。
    #[must_use]
    pub fn chunk_id(&self, plaintext: &[u8]) -> String {
        hex(&self.chunk_id.hmac(plaintext))
    }

    /// 加密一块（确定性 nonce，见模块头）。
    #[must_use]
    pub fn seal_chunk(&self, chunk_id: &str, plaintext: &[u8]) -> Vec<u8> {
        let mut nonce = [0_u8; NONCE_LEN];
        nonce.copy_from_slice(&self.chunk_nonce.hmac(chunk_id.as_bytes())[..NONCE_LEN]);
        seal(
            &self.content,
            self.cmk_version,
            nonce,
            Purpose::Chunk,
            self.vault_id,
            chunk_id.as_bytes(),
            plaintext,
        )
    }

    /// 解一块，并核对明文的 HMAC 就是这个块 id。
    pub fn open_chunk(&self, chunk_id: &str, data: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let plain = open(
            &self.content,
            self.cmk_version,
            Purpose::Chunk,
            self.vault_id,
            chunk_id.as_bytes(),
            data,
        )?;
        if self.chunk_id(&plain) != chunk_id {
            return Err(CryptoError::ChunkIdMismatch);
        }
        Ok(plain)
    }

    /// 加密一个清单条目（绑定 file_id 与这次提交将得到的 rev）。
    #[must_use]
    pub fn seal_entry(&self, file_id: Uuid, rev: i64, plaintext: &[u8]) -> Vec<u8> {
        seal(
            &self.manifest,
            self.cmk_version,
            random_nonce(),
            Purpose::Entry,
            self.vault_id,
            &entry_binding(file_id, rev),
            plaintext,
        )
    }

    pub fn open_entry(&self, file_id: Uuid, rev: i64, data: &[u8]) -> Result<Vec<u8>, CryptoError> {
        open(
            &self.manifest,
            self.cmk_version,
            Purpose::Entry,
            self.vault_id,
            &entry_binding(file_id, rev),
            data,
        )
    }

    /// 加密库的显示名。
    #[must_use]
    pub fn seal_vault_name(&self, name: &str) -> Vec<u8> {
        seal(
            &self.manifest,
            self.cmk_version,
            random_nonce(),
            Purpose::VaultName,
            self.vault_id,
            &[],
            name.as_bytes(),
        )
    }

    pub fn open_vault_name(&self, data: &[u8]) -> Result<String, CryptoError> {
        let plain = open(
            &self.manifest,
            self.cmk_version,
            Purpose::VaultName,
            self.vault_id,
            &[],
            data,
        )?;
        String::from_utf8(plain).map_err(|_| CryptoError::Malformed)
    }
}

/// 一个库在本机的全部钥匙：新写入用最新版本，读按密文头里的版本挑（轮换是懒的，旧块不重加密）。
pub struct KeyRing {
    current: u32,
    keys: std::collections::BTreeMap<u32, VaultKeys>,
}

impl KeyRing {
    /// 没有任何 CMK → `None`。
    #[must_use]
    pub fn new(vault_id: Uuid, cmks: &[Cmk]) -> Option<Self> {
        let keys: std::collections::BTreeMap<u32, VaultKeys> = cmks
            .iter()
            .map(|c| (c.version(), c.derive(vault_id)))
            .collect();
        let current = *keys.keys().next_back()?;
        Some(Self { current, keys })
    }

    /// 新写入用的那一版。
    #[must_use]
    pub fn current(&self) -> &VaultKeys {
        &self.keys[&self.current]
    }

    /// 解这份密文要用的那一版；本机没有 → [`CryptoError::KeyVersion`]。
    pub fn for_ciphertext(&self, data: &[u8]) -> Result<&VaultKeys, CryptoError> {
        let version = ciphertext_cmk_version(data)?;
        self.keys.get(&version).ok_or(CryptoError::KeyVersion {
            expected: self.current,
            found: version,
        })
    }
}

fn entry_binding(file_id: Uuid, rev: i64) -> Vec<u8> {
    let mut out = file_id.as_bytes().to_vec();
    out.extend_from_slice(&rev.to_be_bytes());
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 封装 CMK 的钥匙（KEK）。
pub struct Kek(SecretKey);

impl Kek {
    /// 恢复密钥 → KEK（恢复密钥本身就是 256 位随机数，只需 HKDF 换个用途）。
    #[must_use]
    pub fn from_recovery_key(key: &RecoveryKey) -> Self {
        let hk = Hkdf::<Sha256>::new(None, key.expose_bytes());
        let mut out = [0_u8; 32];
        hk.expand(b"lunarca-sync/v1/recovery-kek", &mut out)
            .expect("32 bytes is a valid hkdf length");
        Self(SecretKey(out))
    }

    /// 把 CMK 封进一个信封（槽位进附加认证数据：一个槽位的信封不能冒充另一个槽位）。
    #[must_use]
    pub fn wrap(&self, vault_id: Uuid, slot: &str, cmk: &Cmk) -> Vec<u8> {
        seal(
            &self.0,
            cmk.version,
            random_nonce(),
            Purpose::Envelope,
            vault_id,
            slot.as_bytes(),
            &cmk.key.0,
        )
    }

    /// 打开信封。钥匙不对、信封被改过、槽位或库对不上，都是 [`CryptoError::Authentication`]。
    pub fn unwrap(&self, vault_id: Uuid, slot: &str, wrapped: &[u8]) -> Result<Cmk, CryptoError> {
        let version = ciphertext_cmk_version(wrapped)?;
        let mut plain = open(
            &self.0,
            version,
            Purpose::Envelope,
            vault_id,
            slot.as_bytes(),
            wrapped,
        )?;
        let bytes: [u8; 32] = plain
            .as_slice()
            .try_into()
            .map_err(|_| CryptoError::Malformed)?;
        plain.zeroize();
        Ok(Cmk::from_bytes(version, bytes))
    }
}

/// 信封上的 KDF 参数（服务端不解释，原样存取）。恢复密钥本身是 256 位随机数，没有可调参数。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "alg", rename_all = "snake_case")]
pub enum KdfParams {
    RecoveryKey,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault() -> Uuid {
        Uuid::from_u128(0x5eed)
    }

    #[test]
    fn chunks_round_trip_deterministically_and_refuse_substitution() {
        let keys = Cmk::generate(1).derive(vault());
        let id = keys.chunk_id(b"hello");
        assert_eq!(id.len(), 64);
        let a = keys.seal_chunk(&id, b"hello");
        let b = keys.seal_chunk(&id, b"hello");
        assert_eq!(a, b, "同一块永远同一份密文：上传幂等");
        assert_eq!(keys.open_chunk(&id, &a).unwrap(), b"hello");

        // 挪到别的块 id 下：认证失败。
        let other = keys.chunk_id(b"world");
        assert_eq!(
            keys.open_chunk(&other, &a),
            Err(CryptoError::Authentication)
        );
        // 改一位：认证失败。
        let mut tampered = a.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 1;
        assert_eq!(
            keys.open_chunk(&id, &tampered),
            Err(CryptoError::Authentication)
        );
        // 别的库的钥匙：认证失败。
        let foreign =
            Cmk::from_bytes(1, *Cmk::generate(1).expose_bytes()).derive(Uuid::from_u128(9));
        assert_eq!(
            foreign.open_chunk(&id, &a),
            Err(CryptoError::Authentication)
        );
    }

    #[test]
    fn the_same_cmk_gives_each_vault_its_own_keys() {
        let cmk = Cmk::generate(1);
        let one = cmk.derive(Uuid::from_u128(1));
        let two = cmk.derive(Uuid::from_u128(2));
        assert_ne!(one.chunk_id(b"x"), two.chunk_id(b"x"));
        let ct = one.seal_chunk(&one.chunk_id(b"x"), b"x");
        assert!(two.open_chunk(&one.chunk_id(b"x"), &ct).is_err());
    }

    #[test]
    fn entries_are_bound_to_their_file_and_rev() {
        let keys = Cmk::generate(3).derive(vault());
        let file = Uuid::from_u128(7);
        let ct = keys.seal_entry(file, 4, b"{\"path\":\"a.md\"}");
        assert_ne!(
            ct,
            keys.seal_entry(file, 4, b"{\"path\":\"a.md\"}"),
            "随机 nonce"
        );
        assert_eq!(
            keys.open_entry(file, 4, &ct).unwrap(),
            b"{\"path\":\"a.md\"}"
        );
        assert_eq!(
            keys.open_entry(file, 5, &ct),
            Err(CryptoError::Authentication),
            "旧版本冒充新版本"
        );
        assert_eq!(
            keys.open_entry(Uuid::from_u128(8), 4, &ct),
            Err(CryptoError::Authentication),
            "挪给别的文件"
        );
        // 一种用途的密文不能当另一种解。
        assert!(keys.open_vault_name(&ct).is_err());
        assert_eq!(ciphertext_cmk_version(&ct), Ok(3));
        let newer = Cmk::generate(4).derive(vault());
        assert_eq!(
            newer.open_entry(file, 4, &ct),
            Err(CryptoError::KeyVersion {
                expected: 4,
                found: 3
            })
        );
    }

    #[test]
    fn a_recovery_key_opens_its_own_envelope() {
        let cmk = Cmk::generate(2);
        let key = RecoveryKey::generate();
        let wrapped = Kek::from_recovery_key(&key).wrap(vault(), "recovery_key", &cmk);
        let back = Kek::from_recovery_key(&RecoveryKey::parse(&key.display()).unwrap())
            .unwrap(vault(), "recovery_key", &wrapped)
            .unwrap();
        assert_eq!(back.expose_bytes(), cmk.expose_bytes());
        assert!(Kek::from_recovery_key(&RecoveryKey::generate())
            .unwrap(vault(), "recovery_key", &wrapped)
            .is_err());
        let kek = Kek::from_recovery_key(&key);
        assert!(
            kek.unwrap(vault(), "recovery_contact", &wrapped).is_err(),
            "槽位进了认证数据"
        );
        assert!(
            kek.unwrap(Uuid::from_u128(1), "recovery_key", &wrapped)
                .is_err(),
            "库 id 进了认证数据"
        );
    }

    #[test]
    fn the_key_ring_writes_with_the_newest_and_reads_any_held_version() {
        let (v1, v2) = (Cmk::generate(1), Cmk::generate(2));
        let ring = KeyRing::new(vault(), &[v2.clone(), v1.clone()]).unwrap();
        assert_eq!(ring.current().cmk_version(), 2);
        let old = v1.derive(vault()).seal_vault_name("库");
        assert_eq!(
            ring.for_ciphertext(&old)
                .unwrap()
                .open_vault_name(&old)
                .unwrap(),
            "库"
        );
        let only_new = KeyRing::new(vault(), &[v2]).unwrap();
        assert!(matches!(
            only_new.for_ciphertext(&old),
            Err(CryptoError::KeyVersion { found: 1, .. })
        ));
        assert!(KeyRing::new(vault(), &[]).is_none());
    }

    #[test]
    fn debug_output_never_shows_key_material() {
        let cmk = Cmk::from_bytes(1, [0xAB; 32]);
        let shown = format!("{cmk:?}");
        assert!(
            !shown.contains("171") && !shown.to_lowercase().contains("ab, "),
            "{shown}"
        );
    }
}
