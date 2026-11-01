//! 凭据存储接口：生产实现在装配层（本机保险箱，TDD-12 §2），内存实现供测试。
//!
//! TDD-14 §3：access / refresh / profile 三个逻辑条目分开存——refresh 是长期高价值凭据，
//! 与短 TTL access 分条便于独立清除与最小读取。条目落在哪里不归本 crate 管：装配层把它们存进
//! 本机保险箱（系统钥匙串里只有一条主密钥，其余逐条加密落盘），本 crate 因此不碰系统钥匙串
//! ——与 `luna-sync` 的 `CmkStore` 同一种分工。
//!
//! 接口是同步的：保险箱读写是毫秒级的本地文件操作（主密钥一个进程只取一次）；调用点在 tauri
//! async 命令内直接调用（不值得为此上 spawn_blocking 的搬运成本）。

use std::collections::HashMap;
use std::sync::Mutex;

use crate::error::CloudError;

/// 条目键：短 TTL access token（Ed25519 JWT）。
pub const KEY_ACCESS: &str = "access-token";
/// 条目键：refresh token（不透明串；轮换语义见 client.rs）。
pub const KEY_REFRESH: &str = "refresh-token";
/// 条目键：账户档案 JSON（account_id / email；登录成功时经 /v1/me 取回缓存，
/// 供账户状态离线读取——不含任何 token）。
pub const KEY_PROFILE: &str = "profile";
/// 条目键：兑换一次性凭据前的试写（写完即删，见 `CloudClient::password_grant`）。落在保险箱里：
/// 试的是「主密钥取得到、保险箱文件写得进」，不再往钥匙串里写任何条目。
pub const KEY_WRITE_PROBE: &str = "write-probe";

/// 凭据存储：**只认字面 key，不懂业务**。
///
/// 这里**刻意没有** `clear_session()` 之类的便利方法。会话条目的真实 key 是
/// 「服务端桶名 + `/` + 条目名」（凭据按服务端分桶，见 `client::CloudClient::scoped`），
/// 而本 trait 不知道当前连的是哪一台。曾经存在过一个遍历 `[KEY_ACCESS, KEY_REFRESH,
/// KEY_PROFILE]` 的默认实现——分桶之后它删的是**一组不存在的 key**，也就是一个
/// 「调用了、返回 Ok、什么也没发生」的清会话，而调用点会以为凭据已经清掉了。
/// 让它压根不存在，比留着并寄希望于没人调用要可靠。清会话的唯一入口是
/// `CloudClient::cred_clear_session`。
pub trait CredentialStore: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<String>, CloudError>;
    fn set(&self, key: &str, value: &str) -> Result<(), CloudError>;
    fn delete(&self, key: &str) -> Result<(), CloudError>;
}

/// 内存实现：测试用（CI 无钥匙串交互面）。
#[derive(Default)]
pub struct MemoryStore {
    map: Mutex<HashMap<String, String>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl CredentialStore for MemoryStore {
    fn get(&self, key: &str) -> Result<Option<String>, CloudError> {
        Ok(self
            .map
            .lock()
            .expect("MemoryStore mutex poisoned")
            .get(key)
            .cloned())
    }

    fn set(&self, key: &str, value: &str) -> Result<(), CloudError> {
        self.map
            .lock()
            .expect("MemoryStore mutex poisoned")
            .insert(key.to_owned(), value.to_owned());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), CloudError> {
        self.map
            .lock()
            .expect("MemoryStore mutex poisoned")
            .remove(key);
        Ok(())
    }
}
