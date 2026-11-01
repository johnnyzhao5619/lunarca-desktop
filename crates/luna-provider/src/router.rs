//! 路由（TDD-01 §4）：Job → 有序候选 Provider 列表（首选→兜底）。

use crate::registry::ProviderRegistry;
use luna_core::{AudioRef, Capability, Job, JobKind, ProviderId, ProviderLocation};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

/// 路由策略预设（PRD-10-008 的 P1 落地形态，TDD-01 §4.1）：只改**候选顺序与是否允许外发**，
/// 故障处置仍由 `JobError::failover_disposition` 唯一裁决——预设不能把「服务端可达但拒绝」
/// 改写成静默本地兜底，也不能让本地失败外发到云。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RouteStrategy {
    /// 云端 / 组织服务端优先（质量优先；默认，等于既有行为）。
    #[default]
    CloudFirst,
    /// 本机优先（延迟 / 成本优先：省一次网络往返与云端计量；定价表落地前成本优先并入此档）。
    LocalFirst,
    /// 仅本机：外部 Provider 不进候选。与「严格本地模式」不同——那是隐私闸（拦一切出站），
    /// 这只是路由偏好。
    LocalOnly,
    /// 本机优先，本机不可达时**允许**交给服务端（ADR-117）。与 [`Self::LocalFirst`] 的唯一
    /// 区别是放开「本地失败 → 外部」这一条处置；它是用户显式选的一档，说明文案写明内容会
    /// 在本机不可用时发往服务端——Apple 私有云计算同一形态：本机优先、越界须一次明示同意。
    LocalFirstFallback,
}

impl RouteStrategy {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CloudFirst => "cloud_first",
            Self::LocalFirst => "local_first",
            Self::LocalOnly => "local_only",
            Self::LocalFirstFallback => "local_first_fallback",
        }
    }

    /// 本机候选不可达时，是否允许调度器把同一个任务交给外部候选。
    /// 只有 [`Self::LocalFirstFallback`] 为真——其余三档保持「本地失败绝不自动外发」。
    #[must_use]
    pub const fn allows_local_to_cloud(self) -> bool {
        matches!(self, Self::LocalFirstFallback)
    }

    /// 本机候选排在外部候选前面——本机优先的三档都是，只有云端优先不是。
    #[must_use]
    pub const fn prefers_local(self) -> bool {
        !matches!(self, Self::CloudFirst)
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "cloud_first" => Some(Self::CloudFirst),
            "local_first" => Some(Self::LocalFirst),
            "local_only" => Some(Self::LocalOnly),
            "local_first_fallback" => Some(Self::LocalFirstFallback),
            _ => None,
        }
    }

    const fn to_u8(self) -> u8 {
        match self {
            Self::CloudFirst => 0,
            Self::LocalFirst => 1,
            Self::LocalOnly => 2,
            Self::LocalFirstFallback => 3,
        }
    }

    const fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::LocalFirst,
            2 => Self::LocalOnly,
            3 => Self::LocalFirstFallback,
            _ => Self::CloudFirst,
        }
    }
}

/// 可从设置侧热切换的策略句柄（`Router` 与设置副作用共享同一个原子）。
#[derive(Clone, Debug, Default)]
pub struct RouteStrategyHandle(Arc<AtomicU8>);

impl RouteStrategyHandle {
    #[must_use]
    pub fn get(&self) -> RouteStrategy {
        RouteStrategy::from_u8(self.0.load(Ordering::Relaxed))
    }

    pub fn set(&self, strategy: RouteStrategy) {
        self.0.store(strategy.to_u8(), Ordering::Relaxed);
    }
}

/// 路由失败。
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RouteError {
    /// 没有任何 Provider 支持该能力。
    #[error("no provider supports capability {0:?}")]
    NoProvider(Capability),
    /// 有 Provider 支持，但严格本地模式把它们（外发型）全过滤了。
    #[error("strict-local mode: no local provider supports capability {0:?}")]
    NoLocalProvider(Capability),
    /// 流式会话没有指明由谁执行。见 [`Router::resolve`] 里那段论证。
    #[error("streaming transcribe job has no preferred provider (it must match the one given to register_stream)")]
    StreamNotPinned,
    /// 流式会话钉的那个 Provider 此刻不在候选里（未注册，或被严格本地挡掉）。
    #[error("the provider owning this stream session ({0}) is unavailable")]
    StreamProviderUnavailable(ProviderId),
}

/// 路由器：按能力 + 策略把 Job 解析为有序候选 Provider。
pub struct Router {
    registry: Arc<ProviderRegistry>,
    strategy: RouteStrategyHandle,
}

impl Router {
    /// 用一个注册表构造。
    #[must_use]
    pub fn new(registry: Arc<ProviderRegistry>) -> Self {
        Self {
            registry,
            strategy: RouteStrategyHandle::default(),
        }
    }

    /// 策略句柄：设置侧改一次，之后每次 `resolve` 都按新预设排。
    #[must_use]
    pub fn strategy_handle(&self) -> RouteStrategyHandle {
        self.strategy.clone()
    }

    /// 解析候选 Provider（有序：首选→兜底）。
    ///
    /// 顺序（TDD-01 §4）：① 硬约束（能力匹配 + 严格本地过滤掉外发型）；② 用户明确
    /// 指定的 Provider；③ 默认云端优先、本地兜底。顺序由领域位置与稳定 id 决定，
    /// 不依赖启动时序或注册先后。
    ///
    /// # Errors
    /// 无候选时返回 [`RouteError`]，并区分"压根没有"与"被严格本地挡掉"。
    pub fn resolve(&self, job: &Job) -> Result<Vec<ProviderId>, RouteError> {
        let cap = job.kind.capability();
        let strategy = self.strategy.get();
        // 严格本地（隐私闸）与「仅本机」预设（路由偏好）在这里合流：都不让外部 Provider 进候选。
        let local_only = job.routing.local_only || strategy == RouteStrategy::LocalOnly;
        let preferred = self.preferred_for(job);

        let providers = self.registry.all();
        let mut candidates: Vec<(ProviderId, ProviderLocation)> = providers
            .iter()
            .filter(|p| {
                let caps = p.capabilities();
                let explicitly_selected = preferred
                    .as_ref()
                    .is_some_and(|preferred| preferred == &p.id());
                caps.supports(cap)
                    && (!caps.explicit_only || explicitly_selected)
                    && !(local_only && caps.location.is_external())
            })
            .map(|p| (p.id(), p.capabilities().location))
            .collect();

        if candidates.is_empty() {
            let any_supports = providers.iter().any(|p| p.capabilities().supports(cap));
            return Err(if any_supports && local_only {
                RouteError::NoLocalProvider(cap)
            } else {
                RouteError::NoProvider(cap)
            });
        }

        // 流式会话**绑定在具体 Provider 实例上**：`StreamId` 登记在那一个实例的表里，
        // 采集侧的窗口也只投进它的通道。
        //
        // 于是兜底链在这里不只是无用，而是有害：换一条腿去执行，它的登记表里根本没有
        // 这个 `StreamId`，用户拿到的是「未注册的流式会话」——一条完全指错方向的错误，
        // 真正的原因是首选那条不可用。而在 ADR-038 之后注册表里确实同时存在多个流式
        // 转写 Provider（本机 whisper / 服务端实时 / 两者的降级组合），这不再是假想。
        if matches!(
            &job.kind,
            JobKind::Transcribe {
                audio: AudioRef::Stream(_),
                ..
            }
        ) {
            // **流式腿只认调用点显式钉的那一个，不接受槽位映射代填**：会话绑定在
            // **Provider 实例**上（`StreamId` 登在它的登记表里），而槽位映射解析出来的是
            // 「设置里配的那个 id」——两者平时相同，一旦不同，用户拿到的是「未注册的流式
            // 会话」这条完全指错方向的错误。故这里读的是 `routing.preferred_provider`
            // 本身，不是 `preferred`。
            let Some(pinned) = job.routing.preferred_provider.clone() else {
                return Err(RouteError::StreamNotPinned);
            };
            return if candidates.iter().any(|(id, _)| id == &pinned) {
                Ok(vec![pinned])
            } else {
                Err(RouteError::StreamProviderUnavailable(pinned))
            };
        }

        candidates.sort_by(|(left_id, left_location), (right_id, right_location)| {
            // 预设只改这一档的正负：云端优先 = 外部排前；本机优先 / 仅本机 = 本机排前。
            let rank = |location: ProviderLocation| {
                if strategy.prefers_local() {
                    u8::from(location.is_external())
                } else {
                    u8::from(!location.is_external())
                }
            };
            let left_preferred = preferred.as_ref().is_some_and(|p| p == left_id);
            let right_preferred = preferred.as_ref().is_some_and(|p| p == right_id);
            right_preferred
                .cmp(&left_preferred)
                .then_with(|| rank(*left_location).cmp(&rank(*right_location)))
                .then_with(|| left_id.as_str().cmp(right_id.as_str()))
        });

        Ok(candidates.into_iter().map(|(id, _)| id).collect())
    }

    /// 本次路由的**定向 Provider**：调用点显式钉的那一个，否则按「泳道 → 模型槽位」
    /// 总映射（[`Job::model_slot`]）从注册表的槽位分配表里解析。
    ///
    /// **选路收敛成一处是 ADR-083 ⑥ 的全部
    /// 内容**。此前档位由各调用点自己解析槽位、再把结果当 `preferred_provider` 传下来，
    /// 那个做法有两处必然失效：① 调用点解析出来的只可能是**本机**的 provider id
    ///（`realtime_llm_provider_id`），所以算力一交给服务端「实时档」这个区分当场消失，
    /// 而设置页仍显示它生效；② 声明是会漏的——弹窗翻译这条调用点从头到尾没进过那条装配，
    /// 于是本该最看重延迟的调用落在质量档上。现在档位由 `Job` 的形状推导，调用点一律不传。
    ///
    /// 仍然保留「调用点显式钉」这条通路：它表达的是**实例绑定**（流式会话钉在
    /// `register_stream` 的那一个实例上、批量 ASR 钉在 `explicit_only` 的批量档 whisper 上），
    /// 与「这次调用要哪一档服务」是两个问题。
    fn preferred_for(&self, job: &Job) -> Option<ProviderId> {
        job.routing.preferred_provider.clone().or_else(|| {
            job.model_slot()
                .and_then(|slot| self.registry.slot_provider(slot))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::{capability_set, MockProvider, MockScript};
    use luna_core::{Job, JobKind, JobOrigin, Lang, ProviderLocation, RoutingHint, TextRef};

    fn translate_job(routing: RoutingHint) -> Job {
        let mut job = Job::new(
            JobKind::Translate {
                text: TextRef::Inline("hi".into()),
                source: None,
                target: Lang::from("zh"),
                instruction: None,
                template: None,
            },
            JobOrigin::PopupTranslate,
        );
        job.routing = routing;
        job
    }

    fn registry_with(
        providers: Vec<(&str, ProviderLocation, Capability)>,
    ) -> Arc<ProviderRegistry> {
        let reg = ProviderRegistry::new();
        for (id, loc, cap) in providers {
            reg.register(Arc::new(MockProvider::new(
                id,
                capability_set(&[cap], loc),
                MockScript::TextResult("x".into()),
            )));
        }
        Arc::new(reg)
    }

    #[test]
    fn explicit_only_provider_never_enters_an_untargeted_fallback_chain() {
        let reg = ProviderRegistry::new();
        reg.register(Arc::new(MockProvider::new(
            "default",
            capability_set(&[Capability::Translate], ProviderLocation::Cloud),
            MockScript::TextResult("default".into()),
        )));
        let mut realtime_caps =
            capability_set(&[Capability::Translate], ProviderLocation::LocalService);
        realtime_caps.explicit_only = true;
        reg.register(Arc::new(MockProvider::new(
            "realtime",
            realtime_caps,
            MockScript::TextResult("realtime".into()),
        )));
        let reg = Arc::new(reg);

        let untargeted = Router::new(Arc::clone(&reg))
            .resolve(&translate_job(RoutingHint::default()))
            .unwrap();
        assert_eq!(untargeted, vec![ProviderId::from("default")]);

        let targeted = Router::new(reg)
            .resolve(&translate_job(RoutingHint {
                preferred_provider: Some(ProviderId::from("realtime")),
                ..RoutingHint::default()
            }))
            .unwrap();
        assert_eq!(
            targeted,
            vec![ProviderId::from("realtime"), ProviderId::from("default")]
        );
    }

    #[test]
    fn capability_filter_is_cloud_first_independent_of_registration_order() {
        let reg = registry_with(vec![
            (
                "local",
                ProviderLocation::LocalService,
                Capability::Translate,
            ),
            ("cloud", ProviderLocation::Cloud, Capability::Translate),
            ("asr", ProviderLocation::Device, Capability::Transcribe),
        ]);
        let got = Router::new(reg)
            .resolve(&translate_job(RoutingHint::default()))
            .unwrap();
        assert_eq!(
            got,
            vec![ProviderId::from("cloud"), ProviderId::from("local")]
        );
    }

    #[test]
    fn cloud_first_order_is_stable_when_registration_order_changes() {
        let first = registry_with(vec![
            (
                "local",
                ProviderLocation::LocalService,
                Capability::Translate,
            ),
            ("cloud", ProviderLocation::Cloud, Capability::Translate),
        ]);
        let second = registry_with(vec![
            ("cloud", ProviderLocation::Cloud, Capability::Translate),
            (
                "local",
                ProviderLocation::LocalService,
                Capability::Translate,
            ),
        ]);
        let job = translate_job(RoutingHint::default());
        assert_eq!(
            Router::new(first).resolve(&job),
            Router::new(second).resolve(&job)
        );
    }

    #[test]
    fn local_only_excludes_external() {
        let reg = registry_with(vec![
            (
                "local",
                ProviderLocation::LocalService,
                Capability::Translate,
            ),
            ("cloud", ProviderLocation::Cloud, Capability::Translate),
        ]);
        let routing = RoutingHint {
            local_only: true,
            ..Default::default()
        };
        let got = Router::new(reg).resolve(&translate_job(routing)).unwrap();
        assert_eq!(got, vec![ProviderId::from("local")]);
    }

    #[test]
    fn local_only_with_only_cloud_errors_distinctly() {
        let reg = registry_with(vec![(
            "cloud",
            ProviderLocation::Cloud,
            Capability::Translate,
        )]);
        let routing = RoutingHint {
            local_only: true,
            ..Default::default()
        };
        assert_eq!(
            Router::new(reg).resolve(&translate_job(routing)),
            Err(RouteError::NoLocalProvider(Capability::Translate))
        );
    }

    #[test]
    fn no_provider_for_capability() {
        let reg = registry_with(vec![(
            "asr",
            ProviderLocation::Device,
            Capability::Transcribe,
        )]);
        assert_eq!(
            Router::new(reg).resolve(&translate_job(RoutingHint::default())),
            Err(RouteError::NoProvider(Capability::Translate))
        );
    }

    #[test]
    fn strategy_presets_reorder_and_local_only_filters_external() {
        let reg = ProviderRegistry::new();
        reg.register(Arc::new(MockProvider::new(
            "cloud",
            capability_set(&[Capability::Translate], ProviderLocation::Cloud),
            MockScript::TextResult("c".into()),
        )));
        reg.register(Arc::new(MockProvider::new(
            "local",
            capability_set(&[Capability::Translate], ProviderLocation::LocalService),
            MockScript::TextResult("l".into()),
        )));
        let router = Router::new(Arc::new(reg));
        let job = translate_job(RoutingHint::default());
        let ids = |v: Vec<ProviderId>| {
            v.iter()
                .map(|id| id.as_str().to_owned())
                .collect::<Vec<_>>()
        };

        // 默认 = 云端优先，等于既有行为。
        assert_eq!(ids(router.resolve(&job).unwrap()), ["cloud", "local"]);
        router.strategy_handle().set(RouteStrategy::LocalFirst);
        assert_eq!(ids(router.resolve(&job).unwrap()), ["local", "cloud"]);
        router.strategy_handle().set(RouteStrategy::LocalOnly);
        assert_eq!(ids(router.resolve(&job).unwrap()), ["local"]);
        // 显式偏好仍压过预设：用户在分配页选的那个永远排首位。
        router.strategy_handle().set(RouteStrategy::LocalFirst);
        let pinned = translate_job(RoutingHint {
            preferred_provider: Some(ProviderId::from("cloud")),
            ..RoutingHint::default()
        });
        assert_eq!(ids(router.resolve(&pinned).unwrap()), ["cloud", "local"]);
        assert_eq!(
            RouteStrategy::parse("local_only"),
            Some(RouteStrategy::LocalOnly)
        );
        assert_eq!(RouteStrategy::parse("nope"), None);
        // 第四档：顺序与本机优先相同，只多放开「本地失败 → 外部」这一条处置（ADR-117）。
        router
            .strategy_handle()
            .set(RouteStrategy::LocalFirstFallback);
        assert_eq!(ids(router.resolve(&job).unwrap()), ["local", "cloud"]);
        assert!(RouteStrategy::LocalFirstFallback.allows_local_to_cloud());
        assert!(!RouteStrategy::LocalFirst.allows_local_to_cloud());
        assert!(!RouteStrategy::CloudFirst.allows_local_to_cloud());
        assert_eq!(
            RouteStrategy::parse("local_first_fallback"),
            Some(RouteStrategy::LocalFirstFallback)
        );
    }

    #[test]
    fn preferred_provider_bumped_to_front() {
        let reg = registry_with(vec![
            (
                "local",
                ProviderLocation::LocalService,
                Capability::Translate,
            ),
            ("cloud", ProviderLocation::Cloud, Capability::Translate),
        ]);
        let routing = RoutingHint {
            preferred_provider: Some(ProviderId::from("cloud")),
            ..Default::default()
        };
        let got = Router::new(reg).resolve(&translate_job(routing)).unwrap();
        assert_eq!(
            got,
            vec![ProviderId::from("cloud"), ProviderId::from("local")]
        );
    }
}
