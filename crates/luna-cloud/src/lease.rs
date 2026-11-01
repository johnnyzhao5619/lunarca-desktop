//! Lease JWT 本地离线验签（公钥侧）。
//!
//! 服务端以 Ed25519（EdDSA JWT）签发短 TTL 离线租约（PRD-13-005 / TDD-14 §3）；
//! 客户端持公钥离线验签 + 过期检查。**与服务端不共享代码**（PRD-13 §1.8：
//! 共用的是基础设施心智，不是领域模型/实现），验签逻辑独立实现且只做校验、不签发。

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use ed25519_dalek::{Signature, Verifier as _, VerifyingKey};
use serde::Deserialize;

/// Lease claims（服务端 cp-api tokens::LeaseClaims 的镜像形状，客户端只读）。
#[derive(Debug, Deserialize)]
pub struct LeaseClaims {
    pub iss: String,
    /// 账户 id。
    pub sub: String,
    pub device_id: String,
    /// 必须为 `"lease"`（access/lease 用途硬隔离）。
    pub token_use: String,
    pub tier: String,
    pub entitlements: Vec<LeaseEntitlement>,
    pub iat: i64,
    pub exp: i64,
}

#[derive(Debug, Deserialize)]
pub struct LeaseEntitlement {
    pub key: String,
    pub value: serde_json::Value,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum LeaseError {
    #[error("malformed token")]
    Malformed,
    #[error("unsupported algorithm")]
    UnsupportedAlg,
    #[error("signature verification failed")]
    BadSignature,
    #[error("wrong token_use")]
    WrongUse,
    #[error("lease expired")]
    Expired,
}

/// 验签 + `token_use` + `exp`（以传入 `now_unix` 为准，便于测试注入时钟）。
pub fn verify_lease(
    token: &str,
    key: &VerifyingKey,
    now_unix: i64,
) -> Result<LeaseClaims, LeaseError> {
    let mut parts = token.split('.');
    let (header_b64, payload_b64, sig_b64) =
        match (parts.next(), parts.next(), parts.next(), parts.next()) {
            (Some(h), Some(p), Some(s), None) => (h, p, s),
            _ => return Err(LeaseError::Malformed),
        };

    // 头校验：只认 EdDSA（防算法混淆）。
    let header_raw = B64.decode(header_b64).map_err(|_| LeaseError::Malformed)?;
    let header: serde_json::Value =
        serde_json::from_slice(&header_raw).map_err(|_| LeaseError::Malformed)?;
    if header.get("alg").and_then(|v| v.as_str()) != Some("EdDSA") {
        return Err(LeaseError::UnsupportedAlg);
    }

    let sig_raw = B64.decode(sig_b64).map_err(|_| LeaseError::Malformed)?;
    let sig_bytes: [u8; 64] = sig_raw.try_into().map_err(|_| LeaseError::Malformed)?;
    let signing_input = format!("{header_b64}.{payload_b64}");
    key.verify(signing_input.as_bytes(), &Signature::from_bytes(&sig_bytes))
        .map_err(|_| LeaseError::BadSignature)?;

    let payload_raw = B64.decode(payload_b64).map_err(|_| LeaseError::Malformed)?;
    let claims: LeaseClaims =
        serde_json::from_slice(&payload_raw).map_err(|_| LeaseError::Malformed)?;
    if claims.token_use != "lease" {
        return Err(LeaseError::WrongUse);
    }
    if claims.exp <= now_unix {
        return Err(LeaseError::Expired);
    }
    Ok(claims)
}

/// 当前 Unix 秒。
pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use ed25519_dalek::{Signer as _, SigningKey};

    fn sign(claims: &serde_json::Value, key: &SigningKey) -> String {
        let header = B64.encode(br#"{"alg":"EdDSA","typ":"JWT"}"#);
        let payload = B64.encode(serde_json::to_vec(claims).unwrap());
        let input = format!("{header}.{payload}");
        let sig = key.sign(input.as_bytes());
        format!("{input}.{}", B64.encode(sig.to_bytes()))
    }

    fn lease_claims(exp: i64) -> serde_json::Value {
        serde_json::json!({
            "iss": "https://api.test",
            "sub": "0d4e4f30-1111-2222-3333-444455556666",
            "device_id": "0d4e4f30-aaaa-bbbb-cccc-dddd11112222",
            "token_use": "lease",
            "tier": "free",
            "entitlements": [{ "key": "devices.max", "value": 2 }],
            "iat": 1000,
            "exp": exp,
        })
    }

    #[test]
    fn roundtrip_and_expiry() {
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let token = sign(&lease_claims(2000), &key);
        let claims = verify_lease(&token, &key.verifying_key(), 1500).unwrap();
        assert_eq!(claims.tier, "free");
        assert_eq!(
            verify_lease(&token, &key.verifying_key(), 2000).unwrap_err(),
            LeaseError::Expired
        );
    }

    #[test]
    fn wrong_key_and_wrong_use_rejected() {
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let other = SigningKey::from_bytes(&[9u8; 32]);
        let token = sign(&lease_claims(2000), &key);
        assert_eq!(
            verify_lease(&token, &other.verifying_key(), 1500).unwrap_err(),
            LeaseError::BadSignature
        );
        let mut access_like = lease_claims(2000);
        access_like["token_use"] = serde_json::json!("access");
        let token = sign(&access_like, &key);
        assert_eq!(
            verify_lease(&token, &key.verifying_key(), 1500).unwrap_err(),
            LeaseError::WrongUse
        );
    }
}
