//! 两个设备本地度量库共用的进程内授权状态（TDD-12 §2.1）。
//!
//! 开关、世代与管理操作屏障必须是一个不可撕裂的事实源：调用方只取得不透明凭据，存储层在
//! 自己的写锁内复验。清除期间暂停签发新凭据；并发设置变更只更新这里的目标开关，不会被清除
//! 结束时的“恢复”覆盖。

use std::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AuthorizationGrant(u64);

#[derive(Debug)]
struct AuthorizationState {
    enabled: bool,
    generation: u64,
    suspensions: u32,
}

/// 度量授权的唯一运行时事实源。
pub(crate) struct MetricsAuthorization(Mutex<AuthorizationState>);

impl MetricsAuthorization {
    pub(crate) fn new(enabled: bool) -> Self {
        Self(Mutex::new(AuthorizationState {
            enabled,
            generation: 0,
            suspensions: 0,
        }))
    }

    pub(crate) fn enabled(&self) -> bool {
        self.0.lock().map(|state| state.enabled).unwrap_or(false)
    }

    /// 设置持久偏好的运行时投影。真实翻转会开新世代，重复写同一值保持幂等。
    pub(crate) fn set_enabled(&self, enabled: bool) {
        let Ok(mut state) = self.0.lock() else {
            return;
        };
        if state.enabled != enabled {
            state.enabled = enabled;
            state.generation = state.generation.wrapping_add(1);
        }
    }

    /// 开始一次观测。管理操作进行中或用户未授权时不签发凭据。
    pub(crate) fn begin(&self) -> Option<AuthorizationGrant> {
        let state = self.0.lock().ok()?;
        (state.enabled && state.suspensions == 0).then_some(AuthorizationGrant(state.generation))
    }

    /// 存储写锁内复验凭据。
    pub(crate) fn is_current(&self, grant: AuthorizationGrant) -> bool {
        self.0
            .lock()
            .map(|state| state.enabled && state.suspensions == 0 && state.generation == grant.0)
            .unwrap_or(false)
    }

    /// 暂停签发并作废全部既有凭据。返回的屏障离开作用域后才重新允许按最新设置签发。
    pub(crate) fn suspend(&self) -> MetricsAuthorizationSuspension<'_> {
        if let Ok(mut state) = self.0.lock() {
            state.suspensions = state.suspensions.saturating_add(1);
            state.generation = state.generation.wrapping_add(1);
        }
        MetricsAuthorizationSuspension {
            authorization: self,
        }
    }
}

/// 管理操作屏障。它只恢复“允许签发”状态，不恢复旧开关值，因此并发设置变更不会丢失。
pub(crate) struct MetricsAuthorizationSuspension<'a> {
    authorization: &'a MetricsAuthorization,
}

impl Drop for MetricsAuthorizationSuspension<'_> {
    fn drop(&mut self) {
        if let Ok(mut state) = self.authorization.0.lock() {
            state.suspensions = state.suspensions.saturating_sub(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setting_and_generation_are_one_fact() {
        let auth = MetricsAuthorization::new(true);
        let old = auth.begin().expect("enabled");
        auth.set_enabled(false);
        auth.set_enabled(true);
        assert!(!auth.is_current(old));
        assert!(auth.is_current(auth.begin().expect("re-enabled")));
    }

    #[test]
    fn suspension_blocks_new_grants_and_preserves_concurrent_setting_changes() {
        let auth = MetricsAuthorization::new(true);
        let old = auth.begin().expect("enabled");
        let suspension = auth.suspend();
        assert!(auth.begin().is_none());
        assert!(!auth.is_current(old));
        auth.set_enabled(false);
        drop(suspension);
        assert!(!auth.enabled());
        assert!(auth.begin().is_none());
    }
}
