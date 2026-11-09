//! 口令登录的密码学与报文格式（ADR-158，线协议见 TDD-16 §5.2a）。**纯计算、不联网**：桌面
//! （`luna-cloud`）原生编译、网页版（`luna-web`）编译成 WASM，两端用的是这同一份（ADR-177 ⑥）。
//!
//! - 口令只在本机进 OPAQUE（RFC 9807）：套件 ristretto255 OPRF + 3DH(ristretto255, SHA-512) +
//!   Argon2id KSF，参数随记录版本固定（[`RECORD_VERSION`]），不做运行时协商。
//! - 身份绑定：`client_identity` = [`normalize_email`]，`server_identity` = `deployment_id`——
//!   握手被中继到另一台碰巧有同名同口令账户的部署时 3DH 校验不过。
//! - 信任包：服务端用会话密钥派生的 AES-256-GCM 密钥加密回送；解开且 `deployment_id` 一致才算数。
//! - 领取凭据：会话密钥派生的 `K_claim`，只经钉住的 TLS 原样发送（服务端只存它的 SHA-256）。
//!
//! 两端共用的互通向量在 `tests/fixtures/opaque-login-v16.json`（由服务端实现生成），本模块的测试按同一
//! 流程与随机数脚本重放，逐字节核对。
//!

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine as _;
use hkdf::Hkdf;
use opaque_ke::argon2::{Algorithm, Argon2, Params, Version};
use opaque_ke::ciphersuite::CipherSuite;
use opaque_ke::{Identifiers, Ristretto255, TripleDh};
use serde::Deserialize;
use sha2::Sha512;
use zeroize::Zeroizing;

/// 口令记录的套件（记录版本 1）。
pub struct Suite;

impl CipherSuite for Suite {
    type OprfCs = Ristretto255;
    type KeyExchange = TripleDh<Ristretto255, Sha512>;
    type Ksf = Argon2<'static>;
}

/// 记录版本：KSF 参数与套件随它固定；改任一项 = 换版本、走重新注册（ADR-158 ⑧）。
pub const RECORD_VERSION: u16 = 1;

/// OPAQUE 上下文串（两端必须逐字节一致）。
pub const CONTEXT: &[u8] = b"lunarca-opaque-v1";

/// Argon2id 内存（KiB）。低端 Windows i5（i5-1135G7）release 实测整次登录约 0.14 s。
const KSF_MEMORY_KIB: u32 = 65_536;
/// Argon2id 迭代次数。
const KSF_ITERATIONS: u32 = 3;
/// Argon2id 并行度。
const KSF_PARALLELISM: u32 = 1;

const BUNDLE_INFO: &[u8] = b"lunarca/v16/trust-bundle";
const CLAIM_INFO: &[u8] = b"lunarca/v16/session-claim";
const NONCE_LEN: usize = 12;

/// 本套件的 KSF。参数是常量，构造失败只可能是常量写错——那是编译期就该发现的事。
#[must_use]
pub fn ksf() -> Argon2<'static> {
    let params = Params::new(KSF_MEMORY_KIB, KSF_ITERATIONS, KSF_PARALLELISM, None)
        .expect("Argon2id 参数是编译期常量");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

/// 邮箱规整：去首尾空白 → NFKC → Unicode 小写。它就是 OPAQUE 的 `client_identity` 与服务端查账户的键，
/// 两端必须用同一条规则（TDD-16 §5.2a）：全角 `ＡＬＩＣＥ`、连字 `ﬁ` 与半角写法是同一个邮箱。
#[must_use]
pub fn normalize_email(raw: &str) -> String {
    icu_normalizer::ComposingNormalizerBorrowed::new_nfkc()
        .normalize(raw.trim())
        .to_lowercase()
}

/// 一次性码的长度（Crockford base32，约 50 bit）。
pub const CODE_LEN: usize = 10;

/// 一次性码规整（Crockford base32，TDD-16 §5.2a）：去掉空白与连字符、转大写，再把易混的
/// `O` → `0`、`I` / `L` → `1`；规整后必须恰为 [`CODE_LEN`] 个 Crockford 字符，否则 `None`（不发请求）。
/// 码就是这次握手的口令，两端必须用同一条规则。
#[must_use]
pub fn normalize_code(raw: &str) -> Option<String> {
    const ALPHABET: &str = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let code: String = raw
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            other => other,
        })
        .collect();
    (code.chars().count() == CODE_LEN && code.chars().all(|c| ALPHABET.contains(c))).then_some(code)
}

/// 邮箱的最低形状：`local@domain`，两段都非空、domain 里没有 `@`。只拦明显打错的输入，
/// 不做 RFC 5322 全量校验——真伪由服务端那一侧的账户说了算。
#[must_use]
pub fn email_domain(normalized: &str) -> Option<&str> {
    let (local, domain) = normalized.rsplit_once('@')?;
    (!local.is_empty()
        && !domain.is_empty()
        && !local.contains('@')
        && !domain.contains(char::is_whitespace))
    .then_some(domain)
}

/// 握手两端的身份（ADR-158 ②）。
#[must_use]
pub fn identifiers<'a>(email: &'a str, deployment_id: &'a str) -> Identifiers<'a> {
    Identifiers {
        client: Some(email.as_bytes()),
        server: Some(deployment_id.as_bytes()),
    }
}

/// base64url 无填充（线协议里所有二进制字段的编码）。
#[must_use]
pub fn b64u(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// 解 base64url 无填充；格式不对 → `None`。
#[must_use]
pub fn from_b64u(text: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(text)
        .ok()
}

fn hkdf_32(session_key: &[u8], login_id: &[u8], info: &[u8]) -> Zeroizing<[u8; 32]> {
    let hk = Hkdf::<Sha512>::new(Some(login_id), session_key);
    let mut out = Zeroizing::new([0_u8; 32]);
    hk.expand(info, out.as_mut())
        .expect("32 字节远小于 HKDF-SHA512 的输出上限");
    out
}

/// 信任包的 AES-256-GCM 密钥：HKDF-SHA512(IKM = session_key, salt = login_id, info = trust-bundle)。
#[must_use]
pub fn bundle_key(session_key: &[u8], login_id: &[u8]) -> Zeroizing<[u8; 32]> {
    hkdf_32(session_key, login_id, BUNDLE_INFO)
}

/// 领取凭据 `K_claim`：HKDF-SHA512(IKM = session_key, salt = login_id, info = session-claim)。
#[must_use]
pub fn claim_key(session_key: &[u8], login_id: &[u8]) -> Zeroizing<[u8; 32]> {
    hkdf_32(session_key, login_id, CLAIM_INFO)
}

fn bundle_aad(login_id: &[u8], deployment_id: &str) -> Vec<u8> {
    let mut aad = Vec::with_capacity(BUNDLE_INFO.len() + login_id.len() + deployment_id.len());
    aad.extend_from_slice(BUNDLE_INFO);
    aad.extend_from_slice(login_id);
    aad.extend_from_slice(deployment_id.as_bytes());
    aad
}

/// 服务端怎么证明自己的 TLS 身份。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BundleTls {
    /// 自签部署：只为这台服务端钉住这张根（ADR-153 的钉住机制，来源改为口令认证）。
    PinnedRoot {
        /// 根证书 PEM。
        root_pem: String,
    },
    /// 公网证书（Lunarca 云、自备公网证书的部署）：照常走系统信任库。
    Public,
}

/// 经会话密钥认证过的服务端身份（ADR-158 ③）。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, serde::Serialize)]
pub struct TrustBundle {
    /// 格式版本，当前恒为 1。
    pub format_version: u32,
    /// 部署 id：必须与握手用的 `server_identity` 一致。
    pub deployment_id: String,
    /// TLS 身份。
    pub tls: BundleTls,
    /// Lease 验签公钥（64 hex）。
    pub lease_pubkey: String,
    /// 目录验签公钥（64 hex）。
    pub catalog_pubkey: String,
}

/// 信任包打不开或不可信——任一种都整次丢弃，不钉住任何东西。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BundleError {
    /// 解不开（密钥不对 = 对端不是这个账户的那台服务端，或报文被改过）。
    #[error("trust bundle does not decrypt")]
    Decrypt,
    /// 解开了但读不懂。
    #[error("trust bundle is malformed")]
    Malformed,
    /// 格式版本不认识。
    #[error("trust bundle format version is unsupported")]
    Version,
    /// 部署 id 与握手身份 / 发现结果不一致（中继到另一台部署）。
    #[error("trust bundle names another deployment")]
    DeploymentMismatch,
}

/// 用会话密钥解开信任包，并核对它就是握手时那台部署。
///
/// # Errors
/// 见 [`BundleError`]。
pub fn open_bundle(
    sealed: &[u8],
    session_key: &[u8],
    login_id: &[u8],
    deployment_id: &str,
) -> Result<TrustBundle, BundleError> {
    if sealed.len() <= NONCE_LEN {
        return Err(BundleError::Decrypt);
    }
    let (nonce, ciphertext) = sealed.split_at(NONCE_LEN);
    let key = bundle_key(session_key, login_id);
    let cipher = Aes256Gcm::new_from_slice(key.as_ref()).map_err(|_| BundleError::Decrypt)?;
    let aad = bundle_aad(login_id, deployment_id);
    let plain = Zeroizing::new(
        cipher
            .decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: ciphertext,
                    aad: &aad,
                },
            )
            .map_err(|_| BundleError::Decrypt)?,
    );
    let bundle: TrustBundle = serde_json::from_slice(&plain).map_err(|_| BundleError::Malformed)?;
    if bundle.format_version != 1 {
        return Err(BundleError::Version);
    }
    ensure_same_deployment(&bundle, deployment_id)?;
    Ok(bundle)
}

/// 信任包里的部署必须就是握手（以及发现结果）里那一台。
///
/// # Errors
/// 不一致 → [`BundleError::DeploymentMismatch`]。
pub fn ensure_same_deployment(
    bundle: &TrustBundle,
    deployment_id: &str,
) -> Result<(), BundleError> {
    if bundle.deployment_id == deployment_id {
        Ok(())
    } else {
        Err(BundleError::DeploymentMismatch)
    }
}

/// 服务端那一侧的封包（只给测试与本仓的模拟服务端用；真实服务端在商业仓按同一份契约实现）。
/// AAD 用指定的握手部署，故也能模拟「解开了、明文却写另一台」的服务端。
#[doc(hidden)]
pub fn seal_bundle_for(
    bundle: &TrustBundle,
    handshake_deployment: &str,
    session_key: &[u8],
    login_id: &[u8],
    nonce: [u8; NONCE_LEN],
) -> Vec<u8> {
    let key = bundle_key(session_key, login_id);
    let cipher = Aes256Gcm::new_from_slice(key.as_ref()).expect("32 字节密钥");
    let aad = bundle_aad(login_id, handshake_deployment);
    let plain = serde_json::to_vec(bundle).expect("bundle serialize");
    let mut out = nonce.to_vec();
    out.extend(
        cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &plain,
                    aad: &aad,
                },
            )
            .expect("encrypt"),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes_gcm::aead::rand_core::{CryptoRng, Error as RandError, RngCore};
    use opaque_ke::{
        ClientLogin, ClientLoginFinishParameters, ClientRegistration,
        ClientRegistrationFinishParameters, CredentialResponse, RegistrationResponse, ServerLogin,
        ServerLoginParameters, ServerRegistration, ServerSetup,
    };
    use rand_chacha::rand_core::SeedableRng as _;
    use rand_chacha::ChaCha20Rng;
    use sha2::{Digest as _, Sha256};

    /// 两端共用的互通向量（由服务端实现生成，见文件内 `description` / `conventions`）。
    const FIXTURE: &str = include_str!("../tests/fixtures/opaque-login-v16.json");

    fn unhex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex"))
            .collect()
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// 按给定字节循环供给随机数（同 opaque-ke 测试里的 CycleRng，向量的 `conventions.rng`）。
    struct CycleRng {
        bytes: Vec<u8>,
        at: usize,
    }

    impl CycleRng {
        fn new(hex_bytes: &str) -> Self {
            let bytes = unhex(hex_bytes);
            assert!(!bytes.is_empty());
            Self { bytes, at: 0 }
        }
    }

    impl RngCore for CycleRng {
        fn next_u32(&mut self) -> u32 {
            let mut buf = [0_u8; 4];
            self.fill_bytes(&mut buf);
            u32::from_le_bytes(buf)
        }

        fn next_u64(&mut self) -> u64 {
            let mut buf = [0_u8; 8];
            self.fill_bytes(&mut buf);
            u64::from_le_bytes(buf)
        }

        fn fill_bytes(&mut self, dest: &mut [u8]) {
            for byte in dest {
                *byte = self.bytes[self.at % self.bytes.len()];
                self.at += 1;
            }
        }

        fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), RandError> {
            self.fill_bytes(dest);
            Ok(())
        }
    }

    impl CryptoRng for CycleRng {}

    fn fixture() -> serde_json::Value {
        serde_json::from_str(FIXTURE).expect("fixture json")
    }

    fn s<'a>(value: &'a serde_json::Value, path: &[&str]) -> &'a str {
        let mut at = value;
        for key in path {
            at = &at[*key];
        }
        at.as_str()
            .unwrap_or_else(|| panic!("fixture field {path:?}"))
    }

    #[test]
    fn the_suite_matches_the_shared_fixture() {
        let f = fixture();
        assert_eq!(s(&f, &["suite", "context_utf8"]).as_bytes(), CONTEXT);
        assert_eq!(f["suite"]["record_version"], u64::from(RECORD_VERSION));
        assert_eq!(f["suite"]["ksf"]["memory_kib"], u64::from(KSF_MEMORY_KIB));
        assert_eq!(f["suite"]["ksf"]["iterations"], u64::from(KSF_ITERATIONS));
        assert_eq!(f["suite"]["ksf"]["parallelism"], u64::from(KSF_PARALLELISM));
    }

    #[test]
    fn email_and_code_normalization_match_the_shared_fixture() {
        let f = fixture();
        for case in f["email_normalization"].as_array().unwrap() {
            assert_eq!(
                normalize_email(s(case, &["input"])),
                s(case, &["normalized"]),
                "{case}"
            );
        }
        for case in f["code_normalization"].as_array().unwrap() {
            assert_eq!(
                normalize_code(s(case, &["input"])).as_deref(),
                case["normalized"].as_str(),
                "{case}"
            );
        }
    }

    /// 客户端照同一流程与随机数脚本重放每一组向量：注册、登录三条报文、会话密钥、领取凭据与信任包
    /// 逐字节相等，即与服务端互通。
    #[test]
    fn the_client_reproduces_every_shared_vector_byte_for_byte() {
        let f = fixture();
        let ksf = ksf();
        for v in f["vectors"].as_array().unwrap() {
            let name = s(v, &["name"]);
            let email = normalize_email(s(v, &["email_input"]));
            assert_eq!(email, s(v, &["client_identity"]), "{name}");
            let secret = match s(v, &["purpose"]) {
                "code" => normalize_code(s(v, &["secret_input"])).expect("code"),
                _ => s(v, &["secret_input"]).to_owned(),
            };
            assert_eq!(secret, s(v, &["password_utf8"]), "{name}");
            let deployment = s(v, &["server_identity"]);
            let ids = identifiers(&email, deployment);

            // 注册。
            let reg = &v["registration"];
            let mut rng = CycleRng::new(s(reg, &["rng", "client_start_hex"]));
            let start = ClientRegistration::<Suite>::start(&mut rng, secret.as_bytes()).unwrap();
            assert_eq!(
                hex(&start.message.serialize()),
                s(reg, &["registration_request_hex"]),
                "{name}: registration_request"
            );
            let response = RegistrationResponse::<Suite>::deserialize(&unhex(s(
                reg,
                &["registration_response_hex"],
            )))
            .unwrap();
            let mut rng = CycleRng::new(s(reg, &["rng", "client_finish_hex"]));
            let finish = start
                .state
                .finish(
                    &mut rng,
                    secret.as_bytes(),
                    response,
                    ClientRegistrationFinishParameters::new(ids, Some(&ksf)),
                )
                .unwrap();
            assert_eq!(
                hex(&finish.message.serialize()),
                s(reg, &["registration_upload_hex"]),
                "{name}: registration_upload"
            );

            // 登录。
            let login = &v["login"];
            let mut rng = CycleRng::new(s(login, &["rng", "client_start_hex"]));
            let start = ClientLogin::<Suite>::start(&mut rng, secret.as_bytes()).unwrap();
            assert_eq!(
                hex(&start.message.serialize()),
                s(login, &["credential_request_hex"]),
                "{name}: credential_request"
            );
            let response = CredentialResponse::<Suite>::deserialize(&unhex(s(
                login,
                &["credential_response_hex"],
            )))
            .unwrap();
            // 客户端 finish 不消耗随机数（conventions.rng）。
            let mut rng = CycleRng::new("00");
            let finished = start
                .state
                .finish(
                    &mut rng,
                    secret.as_bytes(),
                    response,
                    ClientLoginFinishParameters::new(Some(CONTEXT), ids, Some(&ksf)),
                )
                .unwrap();
            assert_eq!(
                hex(&finished.message.serialize()),
                s(login, &["credential_finalization_hex"]),
                "{name}: credential_finalization"
            );
            assert_eq!(
                hex(&finished.session_key),
                s(login, &["session_key_hex"]),
                "{name}: session_key"
            );

            // 领取凭据。
            let session = &v["session"];
            let login_id = unhex(s(session, &["login_id_hex"]));
            assert_eq!(b64u(&login_id), s(session, &["login_id_b64u"]), "{name}");
            let claim = claim_key(&finished.session_key, &login_id);
            assert_eq!(hex(claim.as_ref()), s(session, &["k_claim_hex"]), "{name}");
            assert_eq!(b64u(claim.as_ref()), s(session, &["claim_b64u"]), "{name}");
            assert_eq!(
                hex(&Sha256::digest(claim.as_ref())),
                s(session, &["claim_sha256_hex"]),
                "{name}"
            );

            // 信任包。
            let bundle = &v["trust_bundle"];
            assert_eq!(
                hex(bundle_key(&finished.session_key, &login_id).as_ref()),
                s(bundle, &["bundle_key_hex"]),
                "{name}"
            );
            assert_eq!(
                hex(&bundle_aad(&login_id, deployment)),
                s(bundle, &["aad_hex"]),
                "{name}"
            );
            let sealed = from_b64u(s(bundle, &["sealed_bundle_b64u"])).unwrap();
            assert_eq!(hex(&sealed), s(bundle, &["sealed_bundle_hex"]), "{name}");
            let opened = open_bundle(&sealed, &finished.session_key, &login_id, deployment)
                .unwrap_or_else(|err| panic!("{name}: {err}"));
            let expected: TrustBundle =
                serde_json::from_str(s(bundle, &["plaintext_json"])).unwrap();
            assert_eq!(opened, expected, "{name}");
            // 换一台部署当握手身份：AAD 不同，解不开。
            assert_eq!(
                open_bundle(&sealed, &finished.session_key, &login_id, "dep-other"),
                Err(BundleError::Decrypt),
                "{name}"
            );
        }
    }

    /// 解开了、但包里写的是另一台：仍然拒绝。
    #[test]
    fn a_bundle_naming_another_deployment_is_refused() {
        let key = [0x11_u8; 64];
        let login_id = [0x22_u8; 16];
        let lying = TrustBundle {
            format_version: 1,
            deployment_id: "dep-other".to_owned(),
            tls: BundleTls::Public,
            lease_pubkey: "a".repeat(64),
            catalog_pubkey: "b".repeat(64),
        };
        let sealed = seal_bundle_for(&lying, "dep-real", &key, &login_id, [0x44; NONCE_LEN]);
        assert_eq!(
            open_bundle(&sealed, &key, &login_id, "dep-real"),
            Err(BundleError::DeploymentMismatch)
        );
        assert_eq!(
            open_bundle(&sealed, &[0x12_u8; 64], &login_id, "dep-real"),
            Err(BundleError::Decrypt),
            "会话密钥不对（对端不知道这个账户的记录）：解不开"
        );
    }

    #[test]
    fn a_relayed_handshake_to_another_deployment_fails_in_3dh() {
        let email = normalize_email("relay@example.com");
        let password = b"same password on both deployments";
        let ksf = ksf();
        let mut rng = ChaCha20Rng::from_seed([7; 32]);
        // 部署 Y 恰好有同名同口令账户。
        let setup_y = ServerSetup::<Suite>::new(&mut rng);
        let ids_y = identifiers(&email, "dep-Y");
        let reg = ClientRegistration::<Suite>::start(&mut rng, password).unwrap();
        let sr =
            ServerRegistration::<Suite>::start(&setup_y, reg.message, email.as_bytes()).unwrap();
        let rf = reg
            .state
            .finish(
                &mut rng,
                password,
                sr.message,
                ClientRegistrationFinishParameters::new(ids_y, Some(&ksf)),
            )
            .unwrap();
        let record_y = ServerRegistration::<Suite>::finish(rf.message);
        // 客户端以为在连 X。
        let login = ClientLogin::<Suite>::start(&mut rng, password).unwrap();
        let server_login = ServerLogin::start(
            &mut rng,
            &setup_y,
            Some(record_y),
            login.message,
            email.as_bytes(),
            ServerLoginParameters {
                context: Some(CONTEXT),
                identifiers: ids_y,
            },
        )
        .unwrap();
        let outcome = login.state.finish(
            &mut rng,
            password,
            server_login.message,
            ClientLoginFinishParameters::new(
                Some(CONTEXT),
                identifiers(&email, "dep-X"),
                Some(&ksf),
            ),
        );
        assert!(outcome.is_err(), "身份绑定不同的握手必须失败");
    }
}
