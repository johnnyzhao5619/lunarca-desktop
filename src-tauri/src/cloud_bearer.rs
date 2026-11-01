//! `luna-cloud` 会话 → Provider 侧的 `BearerSource` 适配（ADR-036）。
//!
//! # 为什么要这层适配
//!
//! `luna-provider` 是零 Tauri 依赖的库，够不到插件托管状态；而 `luna-cloud` 是 Tauri
//! 插件，凭据在它的钥匙链存储里。装配层把两者接上，于是 Provider **不需要自己碰
//! 钥匙链**——否则就有了第二条读凭据的路径，而「未登录零云网络行为」的机器断言
//! 只盯着 `luna_cloud::commands` 的命令面。
//!
//! # 惰性取状态
//!
//! 持 `AppHandle` 而不是 `CloudState`：Provider 在启动期装配，而插件托管其状态的时刻
//! 不由我们决定。取不到状态时回 `None`（= 未登录），Provider 随即本地短路——
//! 比在启动期 `expect` 一个还没就绪的状态要诚实得多。

use luna_provider::asr_remote::BearerSource;
use tauri::Manager;

pub struct CloudBearer {
    app: tauri::AppHandle,
}

impl CloudBearer {
    #[must_use]
    pub fn new(app: tauri::AppHandle) -> Self {
        Self { app }
    }

    fn client(&self) -> Option<std::sync::Arc<luna_cloud::client::CloudClient>> {
        self.app
            .try_state::<luna_cloud::commands::CloudState>()
            .map(|state| std::sync::Arc::clone(&state.0))
    }
}

#[async_trait::async_trait]
impl BearerSource for CloudBearer {
    async fn bearer(&self) -> Option<String> {
        // 零网络：只读本地凭据。无会话 → None → Provider 本地短路。
        self.client()?.access_token()
    }

    async fn refresh(&self, rejected: &str) -> Option<String> {
        // 401 之后才走到这里；轮换内部对 401 已做清会话 + NeedsRelogin。
        // `rejected` 原样透传：控制面的 `authed` 与这条 Provider 路由此共用同一份
        // 「等锁期间别人换好了就复用」的二次检查（TDD-14 §3.3.1 ⑩）。
        let client = self.client()?;
        client.refreshed_access_token(rejected).await
    }
}
