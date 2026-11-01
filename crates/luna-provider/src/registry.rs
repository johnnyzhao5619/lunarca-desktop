//! Provider 注册表：按 id 持有 `Arc<dyn AIProvider>`；路由顺序由 Router 的确定性策略决定。

use luna_core::{AIProvider, ModelSlot, ProviderId};
use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

/// 持有一组 Provider，外加**槽位 → 承接它的 Provider id** 这张分配表。
///
/// 注册先后只是存储细节，不表达产品路由策略。
#[derive(Clone, Default)]
pub struct ProviderRegistry {
    providers: Arc<RwLock<Vec<Arc<dyn AIProvider>>>>,
    /// 槽位分配（ADR-083 ⑥）。
    ///
    /// **它住在注册表而不是 Router 的一个构造参数**：分配随用户改设置**在会话中途**
    /// 变（模型分配页保存即生效），而 Router 是 `JobQueue` 起来时就建好的一个长命对象。
    /// 把它做成构造参数就得为「改了分配」再造一遍 Router 与队列，而分配与「哪些 Provider
    /// 在注册表里」本来就是同一次刷新的两半（见 `refresh_realtime_llm_provider`）。
    slots: Arc<RwLock<BTreeMap<ModelSlot, ProviderId>>>,
}

impl ProviderRegistry {
    /// 空注册表。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册一个 Provider（追加到末尾）。
    pub fn register(&self, provider: Arc<dyn AIProvider>) {
        if let Ok(mut providers) = self.providers.write() {
            providers.push(provider);
        }
    }

    /// 按 Provider id 注册或替换。用于模型安装/路径变更后重载同一能力 Provider。
    pub fn register_or_replace(&self, provider: Arc<dyn AIProvider>) {
        let id = provider.id();
        if let Ok(mut providers) = self.providers.write() {
            if let Some(pos) = providers.iter().position(|p| p.id() == id) {
                providers[pos] = provider;
            } else {
                providers.push(provider);
            }
        }
    }

    /// 按 id 移除 Provider（若存在）。返回是否移除。用于设置改为"跟随默认档"时下线
    /// 实时档 LLM Provider，避免残留过期模型的死 Provider（不静默漂移）。
    pub fn remove(&self, id: &ProviderId) -> bool {
        if let Ok(mut providers) = self.providers.write() {
            if let Some(pos) = providers.iter().position(|p| &p.id() == id) {
                providers.remove(pos);
                return true;
            }
        }
        false
    }

    /// 按 id 取 Provider。
    #[must_use]
    pub fn get(&self, id: &ProviderId) -> Option<Arc<dyn AIProvider>> {
        self.providers
            .read()
            .ok()?
            .iter()
            .find(|p| &p.id() == id)
            .cloned()
    }

    /// 全部 Provider（注册顺序）。
    #[must_use]
    pub fn all(&self) -> Vec<Arc<dyn AIProvider>> {
        self.providers
            .read()
            .map(|providers| providers.clone())
            .unwrap_or_default()
    }

    /// 登记 / 撤销一个槽位的承接 Provider（`None` = 撤销）。
    ///
    /// **`DefaultLlm` 恒不登记，这是设计不是遗漏**：本机形态下「默认质量档」不是某
    /// 一个 Provider，它就是**默认候选链本身**（云优先、本机兜底，由 `Router::resolve`
    /// 的确定性排序给出）。给它钉一个 id 等于把兜底链压成一条腿——那正是「配了服务端
    /// 却每次重启都打回本机」那一类缺陷的形状。
    pub fn assign_slot(&self, slot: ModelSlot, provider: Option<ProviderId>) {
        if let Ok(mut slots) = self.slots.write() {
            match provider {
                Some(id) => {
                    slots.insert(slot, id);
                }
                None => {
                    slots.remove(&slot);
                }
            }
        }
    }

    /// 某槽位当前由谁承接（`None` = 未登记 = 走默认候选链）。
    #[must_use]
    pub fn slot_provider(&self, slot: ModelSlot) -> Option<ProviderId> {
        self.slots.read().ok()?.get(&slot).cloned()
    }

    /// 是否为空。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Provider 数量。
    #[must_use]
    pub fn len(&self) -> usize {
        self.providers
            .read()
            .map(|providers| providers.len())
            .unwrap_or(0)
    }
}
