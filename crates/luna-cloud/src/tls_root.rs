//! 自签部署的反代根证书：按服务端钉住（ADR-153）。
//!
//! # 信任从哪来
//!
//! 两条来源，都与 ADR-060 ① 主路径同一档完整性或更高：口令登录送达的信任包（ADR-158 ③：
//! 用 OPAQUE 会话密钥解开，服务端拿不到账户记录就造不出来，见 `crate::first_contact`），
//! 以及组织配置文件里直接带的根证书（MDM 批量预置）。服务端在普通请求里自报什么都不构成信任。
//!
//! # 钉住之后
//!
//! 登记进 [`luna_model_fetch::pinned_tls`]，发往这台服务端的请求只认这张根（系统信任库照旧
//! 管别的主机）。服务端换了根证书，连接如实失败（`tls`），再登录一次即由新的信任包重钉——
//! 不在普通请求里自动采用服务端新给的证书。
//!
//! # 分桶与持久化
//!
//! 按 [`crate::config::server_bucket`] 分桶，与信任锚、凭据同一判据。与 `cloud-trust.json`
//! **分文件**：两者的来源与生命周期不同（根证书可以单独因换反代而重钉，签名公钥不变）。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// 应用数据目录下的钉住文件名。
pub const TLS_ROOT_STATE_FILE: &str = "cloud-tls-roots.json";

/// PEM 里第一张证书的 DER。不是 PEM 证书 → `None`。
#[must_use]
pub fn der_of(pem: &str) -> Option<Vec<u8>> {
    const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
    const END: &str = "-----END CERTIFICATE-----";
    let start = pem.find(BEGIN)? + BEGIN.len();
    let len = pem[start..].find(END)?;
    let body: String = pem[start..start + len]
        .chars()
        .filter(|c| !c.is_ascii_whitespace())
        .collect();
    let der = base64::engine::general_purpose::STANDARD
        .decode(body)
        .ok()?;
    (!der.is_empty()).then_some(der)
}

/// 根证书指纹：DER 的 sha256（64 位小写 hex）。**与控制台同一算法**（商业仓
/// `connect_link::tls_root_fingerprint`）：算 DER 而不是 PEM 文本，换行风格不影响结果。
#[must_use]
pub fn fingerprint(pem: &str) -> Option<String> {
    let der = der_of(pem)?;
    Some(
        Sha256::digest(&der)
            .iter()
            .fold(String::with_capacity(64), |mut acc, byte| {
                use std::fmt::Write as _;
                let _ = write!(acc, "{byte:02x}");
                acc
            }),
    )
}

/// 钉住失败的原因。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TlsRootError {
    /// 交来的不是一张能解析的 PEM 证书。
    #[error("not a certificate")]
    NotCertificate,
}

/// 一台服务端钉住的根证书。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PinnedTlsRoot {
    /// 钉住时的服务端 origin（登记进 `pinned_tls` 用）。
    pub origin: String,
    /// [`fingerprint`]。
    pub sha256: String,
    pub pem: String,
    /// Unix 秒。
    pub pinned_at: i64,
}

/// 钉住状态（内存 + 文件）。
#[derive(Default)]
pub struct TlsRootStore {
    path: Mutex<Option<PathBuf>>,
    pinned: Mutex<BTreeMap<String, PinnedTlsRoot>>,
}

impl TlsRootStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 注入持久化路径、装载既有钉住并登记进 `pinned_tls`（插件 setup 调用）。
    pub fn attach_path(&self, path: PathBuf) {
        let loaded: BTreeMap<String, PinnedTlsRoot> = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        for root in loaded.values() {
            register(root);
        }
        *self.pinned.lock().expect("tls root mutex poisoned") = loaded;
        *self.path.lock().expect("tls root path mutex poisoned") = Some(path);
    }

    /// 该桶钉住的根证书。
    #[must_use]
    pub fn pinned(&self, bucket: &str) -> Option<PinnedTlsRoot> {
        self.pinned
            .lock()
            .expect("tls root mutex poisoned")
            .get(bucket)
            .cloned()
    }

    /// 为 `origin` 钉住一张**已核验过**的根证书（替换旧的）。
    ///
    /// # Errors
    /// PEM 不是证书：不登记一张解析不了的证书。
    pub fn pin(&self, origin: &str, pem: &str, now: i64) -> Result<(), TlsRootError> {
        let sha256 = fingerprint(pem).ok_or(TlsRootError::NotCertificate)?;
        let root = PinnedTlsRoot {
            origin: origin.trim_end_matches('/').to_owned(),
            sha256,
            pem: pem.to_owned(),
            pinned_at: now,
        };
        register(&root);
        self.pinned
            .lock()
            .expect("tls root mutex poisoned")
            .insert(crate::config::server_bucket(origin), root);
        self.persist();
        Ok(())
    }

    /// 撤销该桶的钉住（信任包说这台改用公网证书，或一次钉住在重连核验前被撤回）。
    pub fn unpin(&self, bucket: &str) {
        let removed = self
            .pinned
            .lock()
            .expect("tls root mutex poisoned")
            .remove(bucket);
        if let Some(root) = removed {
            luna_model_fetch::pinned_tls::set_pinned_root(&root.origin, None);
            self.persist();
        }
    }

    /// 恢复到某个先前的状态（`None` = 没钉过）：重连核验失败时撤回这一次钉住。
    pub fn restore(&self, bucket: &str, previous: Option<PinnedTlsRoot>) {
        match previous {
            Some(root) => {
                register(&root);
                self.pinned
                    .lock()
                    .expect("tls root mutex poisoned")
                    .insert(bucket.to_owned(), root);
                self.persist();
            }
            None => self.unpin(bucket),
        }
    }

    /// 写状态文件。写不下去留一条日志（只有错误串）：这次钉住活不过重启。
    fn persist(&self) {
        let Some(path) = self
            .path
            .lock()
            .expect("tls root path mutex poisoned")
            .clone()
        else {
            return;
        };
        let snapshot = self.pinned.lock().expect("tls root mutex poisoned").clone();
        let Ok(bytes) = serde_json::to_vec(&snapshot) else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(err) = std::fs::write(&path, bytes) {
            eprintln!("[luna-cloud] 根证书钉住写盘失败，活不过重启：{err}");
        }
    }
}

fn register(root: &PinnedTlsRoot) {
    luna_model_fetch::pinned_tls::set_pinned_root(&root.origin, der_of(&root.pem));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 与商业仓 `connect_link` 单测同一张夹具；期望值由 `openssl x509 -outform der | shasum -a 256`
    /// 独立算出。
    const ROOT_PEM: &str = "-----BEGIN CERTIFICATE-----
MIIBbDCCARKgAwIBAgIUCaxSJZTtUFopdREqDXcVMPYi3J0wCgYIKoZIzj0EAwIw
HDEaMBgGA1UEAwwRTHVuYXJjYSBUZXN0IFJvb3QwHhcNMjYxMDEyMTYxMDEzWhcN
MzYxMDA5MTYxMDEzWjAcMRowGAYDVQQDDBFMdW5hcmNhIFRlc3QgUm9vdDBZMBMG
ByqGSM49AgEGCCqGSM49AwEHA0IABMOd5K3fg3dGMvmzcHbYQ3M7+Prm6/jxLdde
Xu3hHRsomPNC6fUyBRMMaFyaLah7UCN/zq3lw0/9/PAVBH1DW6ejMjAwMB0GA1Ud
DgQWBBTLmpaWy76kk0B4Z/0WiiexnMCVbzAPBgNVHRMBAf8EBTADAQH/MAoGCCqG
SM49BAMCA0gAMEUCIQDzTi45sCJeVLTWZDsxLq/33nMegMQ12KnTAqYL+psGoAIg
QO+Yewx9o0G+2gn52W7PW/m+RhLBymDZbG4TPoHfVhg=
-----END CERTIFICATE-----
";
    const ROOT_SHA256: &str = "420bddc19376464bc743a78107d2c07925a8754b7215d0d9e38f335b913e0615";

    #[test]
    fn the_fingerprint_matches_the_console() {
        assert_eq!(fingerprint(ROOT_PEM).as_deref(), Some(ROOT_SHA256));
        assert_eq!(
            fingerprint(&ROOT_PEM.replace('\n', "\r\n")).as_deref(),
            Some(ROOT_SHA256)
        );
        assert_eq!(fingerprint("<html>"), None);
    }

    #[test]
    fn a_pin_survives_a_restart_and_registers_for_its_server() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(TLS_ROOT_STATE_FILE);
        let store = TlsRootStore::new();
        store.attach_path(path.clone());
        store
            .pin("https://tls-root-test.invalid:8443", ROOT_PEM, 1)
            .unwrap();
        luna_model_fetch::pinned_tls::set_pinned_root("https://tls-root-test.invalid:8443", None);

        let reloaded = TlsRootStore::new();
        reloaded.attach_path(path);
        let pinned = reloaded.pinned("tls-root-test.invalid:8443").unwrap();
        assert_eq!(pinned.sha256, ROOT_SHA256);
        assert_eq!(
            luna_model_fetch::pinned_tls::pinned_root("https://tls-root-test.invalid:8443/v1/me")
                .as_deref(),
            der_of(ROOT_PEM).as_deref(),
            "重启后装载即登记，不等下一次连接"
        );
        assert_eq!(
            store.pin("https://x.invalid", "nope", 1),
            Err(TlsRootError::NotCertificate)
        );
        // 撤回：钉过的恢复成先前那张，没钉过的撤掉登记。
        let before = store.pinned("tls-root-test.invalid:8443");
        store.unpin("tls-root-test.invalid:8443");
        assert_eq!(store.pinned("tls-root-test.invalid:8443"), None);
        assert_eq!(
            luna_model_fetch::pinned_tls::pinned_root("https://tls-root-test.invalid:8443/v1/me"),
            None
        );
        store.restore("tls-root-test.invalid:8443", before.clone());
        assert_eq!(store.pinned("tls-root-test.invalid:8443"), before);
        luna_model_fetch::pinned_tls::set_pinned_root("https://tls-root-test.invalid:8443", None);
    }
}
