//! Provider 抽象（trait 与支撑类型）。事实源：TDD-01 §3。
//!
//! `luna-core` 只定义"接口与形状"，不含任何实现 / IO；具体 Provider 在 `luna-provider`、
//! 调度在 `luna-jobqueue`。**位置无关**：本地模型 / 在线 API / 本地服务 / 官方云对上层
//! 等价，仅实现不同（ADR-008：云也只是一种 Provider）。

use crate::{Capability, Job, JobEvent, Money, ProviderId};
use async_trait::async_trait;
use futures_core::Stream;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// `execute` 返回的事件流（每 Job 一条、有序、单消费者）。
///
/// 用 `futures_core::Stream` 的 boxed pin 别名——不在 core 引入 `futures-util`，
/// 流的组合/节流在上层（luna-jobqueue）做。
pub type JobEventStream = Pin<Box<dyn Stream<Item = JobEvent> + Send + 'static>>;

/// Provider 所在位置。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderLocation {
    /// 设备内（进程内引擎，如 whisper-rs）。
    Device,
    /// 本地服务（localhost，如随包引擎的 HTTP——**不外发到设备外**）。
    LocalService,
    /// 云 / 在线 API（**内容会外发到设备外**）。
    Cloud,
}

impl ProviderLocation {
    /// 全部取值（供遍历与守护测试）。
    pub const ALL: [Self; 3] = [Self::Device, Self::LocalService, Self::Cloud];

    /// 是否会把内容**外发到设备外**（严格本地模式据此过滤，TDD-12 §1）。
    ///
    /// 注意：`LocalService`（localhost 随包引擎）走 HTTP 但**不外发**，故为 `false`。
    #[must_use]
    pub fn is_external(self) -> bool {
        matches!(self, Self::Cloud)
    }

    /// 落库用的稳定串。逐条写死而不是靠派生：它进 `usage.sqlite` 这张 durable 历史表，
    /// 派生出来的名字会跟着枚举改名悄悄变，而旧行还写着老名字（同 [`crate::ServiceClass::as_str`]）。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Device => "device",
            Self::LocalService => "local_service",
            Self::Cloud => "cloud",
        }
    }

    /// 认不出的取值返回 `None`——不猜一个位置。把一条服务端记录猜成本机，
    /// 「本机省下多少」就会把花掉的钱算成省下的钱。
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|l| l.as_str() == raw)
    }
}

/// Provider 能力声明 + 约束（路由匹配用，TDD-01 §3.1）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilitySet {
    /// 支持的能力集合。
    pub capabilities: BTreeSet<Capability>,
    /// 是否支持流式输出。
    pub streaming: bool,
    /// 所在位置。
    pub location: ProviderLocation,
    /// 是否只接受 [`crate::RoutingHint::preferred_provider`] 的显式定向。
    ///
    /// 实时低延迟档、批处理专用档等 Provider 与默认档可能声明相同能力，但它们不是默认
    /// 兜底。若缺少这一位，Router 只能按能力把它们全部塞进候选链，默认档失联时就会
    /// 静默换模型、换执行位置，界面仍显示用户选择的默认档。
    pub explicit_only: bool,
}

impl CapabilitySet {
    /// 是否支持某能力。
    #[must_use]
    pub fn supports(&self, cap: Capability) -> bool {
        self.capabilities.contains(&cap)
    }
}

/// Provider 描述（名称、位置、是否联网、License 摘要；供 UI 与诊断）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderDescriptor {
    /// 稳定 id。
    pub id: ProviderId,
    /// 展示名。
    pub display_name: String,
    /// 位置。
    pub location: ProviderLocation,
    /// **此刻在用的模型标识**（权重文件名 / 上游模型 id）；`None` = 这个 Provider 不以
    /// 模型为单位工作（OCR、说话人细分），或模型由对端决定、会话建立前还不知道。
    ///
    /// 与 [`Self::display_name`] 是两件事：展示名回答「这是谁」（「本地 Whisper」），
    /// 模型标识回答「用的是哪个权重」（`ggml-base`）。缺了后者时，需要它的消费者只能拿
    /// 展示名顶替——`ExecutorInfo.model` 就是这么被塞进「本地 Whisper」的，而那个字段
    /// 会被归档进 `MediaRef.model`，将来是要当成「当时用了哪个权重」来读的。
    ///
    /// 与 [`crate::job::WindowInput::speech_len`] 同一条教训：接缝里缺的信息，
    /// 第二个消费者只能猜，而猜错不会报错。
    pub model: Option<String>,
    /// License 摘要（由 [TDD-04] 策略门控填充）。
    pub license_summary: Option<String>,
}

/// 健康状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    /// 就绪。
    Ready,
    /// 降级可用。
    Degraded,
    /// 不可用。
    Unavailable,
}

/// 健康/就绪检查结果。`health()` 须由实现侧缓存 + 熔断（TDD-01 §3.1），core 只定形状。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderHealth {
    /// 状态。
    pub status: HealthStatus,
    /// 详情（不可用原因等）。
    pub detail: Option<String>,
}

impl ProviderHealth {
    /// 构造一个"就绪"。
    #[must_use]
    pub fn ready() -> Self {
        Self {
            status: HealthStatus::Ready,
            detail: None,
        }
    }

    /// 是否就绪。
    #[must_use]
    pub fn is_ready(&self) -> bool {
        matches!(self.status, HealthStatus::Ready)
    }
}

/// 用量/费用预估（价格估算功能依赖，TDD-01 §3.1）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageEstimate {
    /// 预估输入 token。
    pub est_tokens_in: Option<u32>,
    /// 预估输出 token。
    pub est_tokens_out: Option<u32>,
    /// 预估费用（本地为 0/None）。
    pub est_cost: Option<Money>,
    /// 预估耗时（ms）。
    pub est_duration_ms: Option<u64>,
}

/// 轻量取消令牌（`Arc<AtomicBool>`，无 IO、无 tokio 依赖）。
///
/// Provider 在**事件/解码边界**轮询 `is_cancelled()`；调度层触发 `cancel()`。
/// 异步 await-on-cancel 的编排（`select!` / `Notify`）归 luna-jobqueue（tokio），不入 core。
/// 取消传播契约见 TDD-01 §5.5。
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    /// 新建未取消的令牌。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 触发取消（幂等）。
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// 是否已取消。
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// 统一 AI 执行接口（ADR-001 的主干）。
///
/// 一切 AI 动作 = 构造 `Job` → 由路由选中的 `AIProvider` 执行 → 产出 `JobEvent` 流。
/// 新增功能不得新起调用栈。对象安全：以 `Arc<dyn AIProvider>` 持有；`execute` 返回
/// `'static` 流，故实现需把 `Arc<Self>`（而非 `&self` 借用）move 进流闭包（TDD-01 §3.1）。
#[async_trait]
pub trait AIProvider: Send + Sync {
    /// 稳定 id。
    fn id(&self) -> ProviderId;

    /// 描述（名称、位置、License 摘要…）。
    fn descriptor(&self) -> ProviderDescriptor;

    /// 声明能力 + 约束。
    fn capabilities(&self) -> CapabilitySet;

    /// 健康/就绪检查（实现侧须缓存 + 熔断，禁止每 Job 探活）。
    async fn health(&self) -> ProviderHealth;

    /// 执行一个 Job，返回事件流。实现负责把 `Job` 翻成自身协议、把响应规整回 `JobEvent`。
    async fn execute(&self, job: Job, cancel: CancelToken) -> JobEventStream;

    /// 用量/费用预估（无法预估时返回 `None`）。
    fn estimate(&self, job: &Job) -> Option<UsageEstimate>;
}

/// 会话内的转写语言控制面（现场切换 + 回读自动检测结果，PRD-04-046）。
///
/// **只暴露调用方真正需要的两个动作**：本机 whisper 的粘性语言状态机里还有
/// `observe(lang_id: i32)` 这类引擎私有概念（whisper 的语言编号），把它一并搬进
/// 领域层就是让 core 认识某个具体引擎的内部编码。远程转写的语言控制是另一套
/// （握手字段），两者只在这两个动作上同构。
pub trait StreamLangControl: Send + Sync {
    /// 现场切换转写语言（`Some` = 显式指定；`None` = 回 auto 重新检测）。
    fn set_configured(&self, configured: Option<String>);
    /// auto 模式下已检测锁定的语言（面板回显；显式指定或尚未检测到时 `None`）。
    fn auto_detected(&self) -> Option<String>;
}

/// 一次流式 ASR 会话的注册句柄。
///
/// 采集侧持有它向 Provider 投喂窗口；**与具体 Provider 无关**——本机 whisper 与
/// 远程流式转写走同一个接缝，采集侧不需要知道自己在喂谁。
pub struct StreamRegistration {
    /// 窗口发送端：投递 `Some(window)` 推进解码；**发 `None` 或丢掉本发送端**关闭流。
    ///
    /// **两种关流写法都必须接住，而采集侧用的是后一种**：`recording.rs` 停录那一段
    /// 做的是 `drop(window_tx.take())`，它从不投 `None`。只接住 `None` 的实现在门禁里
    /// 照样全绿（用例投的都是 `None`），而在生产上表现为「停止字幕之后这条流永不收尾」
    /// ——对本机腿是一个不退出的解码线程，对远程腿是一条不发 `END`、不关闭的 WebSocket，
    /// 而按连接时长计费的上游会一直计下去（2026-08-18 真机，见 `asr_failover::spawn_pump`）。
    pub window_tx: std::sync::mpsc::SyncSender<Option<crate::job::WindowInput>>,
    /// 每窗处理耗时（ms），供 RTF 聚合。
    ///
    /// 本机是推理耗时，远程是往返耗时——两者对"这一窗花了多久"是同一个问题，
    /// 故共用一条通道；消费侧只做聚合，不解释它来自哪一层。
    pub infer_rx: std::sync::mpsc::Receiver<u64>,
    /// 会话语言控制面。
    pub lang: std::sync::Arc<dyn StreamLangControl>,
    /// 本条**登记**的回收句柄：见 [`StreamRelease`]，它是唯一一条不依赖 `execute` 的回收路径。
    ///
    /// **要单独接住**。`let window_tx = reg.window_tx;` 这种只取一个字段的写法会把它
    /// 就地丢掉 —— 那等于在 `execute` 摘走之前把这条会话从登记表里撤了，随后 `execute`
    /// 报「未注册的流式会话」。
    pub release: StreamRelease,
}

/// 一次流式会话**登记**的回收句柄。丢掉它 = 注销这条登记（已被 `execute` 摘走则无事发生）。
///
/// # 为什么登记必须有一条不依赖 `execute` 的回收路径
///
/// 三个流式 ASR 实现（本机 whisper / 远程实时转写 / 降级组合）都是「`register_stream`
/// 登记 → `execute` 摘走」这一对，而采集侧是**先拿句柄、再 `submit(job)`**。Job 有两条
/// 走不到 `execute` 的出口：会话池准入到期即拒（`JobError::LocalSessionBusy`，ADR-078 ③——
/// 「停录后立刻再录」正是它要处理的瞬态），以及路由解析失败（严格本地挡下 / 会话所属
/// Provider 掉线）——两条都在 `execute` 之前就产出终态。**只由 `execute` 回收**的登记
/// 于是永久滞留。
///
/// 代价不是一个空 map 条目：滞留的条目攥着 `window_rx` 那一端，而采集侧在 Job 失败之前
/// 一直在投窗。通道深 64、单窗最大 ~10 秒 16 kHz f32（640 KB），故**每条滞留的登记最多
/// 攥着约 40 MB**；而流式 Provider 实例在形状未变时是被复用的（装配层的
/// `ensure_stream_asr_provider`），这些条目因此活到进程退出，一次一次叠加。
///
/// # 谁持有它、什么时候丢
///
/// 采集侧持有，在 **Job 终态**那一刻丢。
///
/// **不是关流那一刻**：关流恒早于终态——采集侧 `drop(window_tx)` 之后，`execute` 仍要
/// 把缓冲里剩下的窗解完才发 `Finished`（语音输入正是这个形状：松手即关流，识别还在跑）。
/// 把回收绑在关流上，等于把用户说完的最后几窗连同登记一起扔掉。
///
/// 忘了显式丢也还有兜底：句柄随 [`StreamRegistration`] 的其余部分进采集会话的作用域，
/// 会话一散就回收。两条路径同一语义，先到者生效。
#[must_use = "丢掉它就是当场注销这条会话登记——要显式回收请写 drop(...)"]
pub struct StreamRelease {
    /// `None` = 本身不登记的实现（见 [`StreamRelease::detached`]）。
    release: Option<Box<dyn FnOnce() + Send + Sync>>,
}

impl StreamRelease {
    /// 给**本身不维护登记表**的实现用（测试假腿、直接持有通道的桩）。
    ///
    /// 不是真实 Provider 的逃生口：真实实现漏掉回收，表现就是上面那 40 MB。
    pub fn detached() -> Self {
        Self { release: None }
    }
}

impl Drop for StreamRelease {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            release();
        }
    }
}

/// 流式会话登记表：「登记 → 摘走 / 回收」的**唯一**实现。
///
/// 收在这里而不是三个 Provider 各写一份 `Mutex<HashMap<StreamId, _>>`：那三份此前
/// 逐字同形，也就把「只有 `execute` 会 `remove`」这个缺陷原样复制了三份（见
/// [`StreamRelease`]）。登记表的**回收语义**在这里只写一次，三个实现那边只剩「登记什么」。
pub struct StreamRegistry<E> {
    /// 值带一枚**代次**：回收句柄只撤**自己登记的那一条**。
    ///
    /// 照 id 盲删的话，同一个 `StreamId` 在同一张表里被重新登记之后，晚到的旧句柄撤掉
    /// 的会是**正在用的**那条会话——表现为「刚建好的会话报未注册」，而没有任何东西会指向
    /// 那个早已被丢掉的旧句柄。本仓当下没有哪条路径会这么登记（降级组合的两条腿各有各的
    /// 表），但句柄的正确性不该建立在「调用方碰巧不这么用」上：那种前提没有任何东西守着。
    entries: Arc<std::sync::Mutex<std::collections::HashMap<crate::ids::StreamId, (u64, E)>>>,
    next_generation: Arc<std::sync::atomic::AtomicU64>,
}

/// 手写而非 `derive`：`derive(Clone)` 会平白要求 `E: Clone`，而这里克隆的只是那把
/// `Arc`（`execute` 要把表带进事件流里，见本机 whisper 的流式分支）。
impl<E> Clone for StreamRegistry<E> {
    fn clone(&self) -> Self {
        Self {
            entries: Arc::clone(&self.entries),
            next_generation: Arc::clone(&self.next_generation),
        }
    }
}

impl<E> Default for StreamRegistry<E> {
    fn default() -> Self {
        Self {
            entries: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            next_generation: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }
}

impl<E: Send + 'static> StreamRegistry<E> {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记一条会话，返回它的[回收句柄](StreamRelease)。
    pub fn register(&self, id: crate::ids::StreamId, entry: E) -> StreamRelease {
        let generation = self
            .next_generation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        lock(&self.entries).insert(id, (generation, entry));
        let entries = Arc::clone(&self.entries);
        StreamRelease {
            release: Some(Box::new(move || {
                let mut entries = lock(&entries);
                if entries.get(&id).is_some_and(|(g, _)| *g == generation) {
                    entries.remove(&id);
                }
            })),
        }
    }

    /// `execute` 摘走这条会话（拿到即从表里消失；回收句柄随后成为空操作）。
    pub fn take(&self, id: crate::ids::StreamId) -> Option<E> {
        lock(&self.entries).remove(&id).map(|(_, entry)| entry)
    }

    /// 表里还剩几条登记。**守护用例的断言对象**：滞留在这里的每一条都攥着一份窗缓冲。
    #[must_use]
    pub fn len(&self) -> usize {
        lock(&self.entries).len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// 取锁，中毒也照常用。
///
/// 回收路径跑在 `Drop` 里，那里 panic 会直接 abort 掉整个进程——为一张「id → 会话」
/// 的表而炸掉用户正在录的音，代价方向是反的。而这张表本身不会因为某个持锁者 panic 就
/// 变得不可信：中毒时**不**回收才是真正的代价（那 40 MB）。
fn lock<E>(
    entries: &std::sync::Mutex<std::collections::HashMap<crate::ids::StreamId, (u64, E)>>,
) -> std::sync::MutexGuard<'_, std::collections::HashMap<crate::ids::StreamId, (u64, E)>> {
    entries
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 支持**流式喂帧**的 ASR Provider。
///
/// 单独一个 trait 而不是塞进 [`AIProvider`]：绝大多数 Provider（LLM / 嵌入 / OCR）
/// 没有"会话"这回事，给它们加一个必须实现的 `register_stream` 只会逼出一堆
/// `unimplemented!()`——那正是"接口说有、实现说没有"的来源。
pub trait StreamingAsr: AIProvider {
    /// 注册一次流式会话。
    fn register_stream(&self, id: crate::ids::StreamId) -> StreamRegistration;
}

/// 外发闸：**此刻**这台设备允不允许把内容发出去（严格本地模式，TDD-12 §1）。
///
/// # 为什么装配时过滤一次不够
///
/// 「严格本地」的既有实现是两道：路由按 [`ProviderLocation::is_external`] 过滤、装配层
/// 干脆不构造云腿。两道都只在**会话开始那一刻**生效——而实时转写是**常开**的那条路径
/// （ADR-038 ④）。用户在一场一小时的会议中途按下「仅本地」，期望是麦克风内容当场停止
/// 外发；没有这个闸的话，正在跑的那条腿仍持有自己的 `Arc`，会一路上传到会议结束，
/// 而界面上的隐私档已经显示「仅本地」。**开关看起来生效了，实际没有**——这是隐私承诺
/// 里最坏的一种失败：它不报错、不留痕，只有抓包才看得见。
///
/// 故外发型 Provider 必须在**每次真正要发字节之前**问一次。判据的事实源仍在装配层
/// （它读设置、它更新闸），Provider 只负责服从。
pub trait OutboundGate: Send + Sync {
    /// 此刻允许把内容发出设备吗。
    ///
    /// 会在音频热路径上被高频调用（每窗一次），实现必须是**便宜且无锁**的——
    /// 去抢一把全局设置锁会把隐私检查变成音频链路上的抖动源。
    fn allows_outbound(&self) -> bool;
}

/// 恒定放行的闸（本地测试与不涉外发的装配用）。
///
/// 单独给一个具名类型而不是让调用方各写一个 `struct Yes`：一处默认实现意味着
/// 「放行」这个决定只在这里出现一次，`grep` 得到。
#[derive(Debug, Clone, Copy, Default)]
pub struct AllowOutbound;

impl OutboundGate for AllowOutbound {
    fn allows_outbound(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {

    /// 流式喂帧接缝必须是 **Provider 无关**的。
    ///
    /// 存在的理由：采集侧此前直接调 `WhisperProvider::register_stream`，于是"喂给谁"
    /// 被写死成了本机 whisper——远程流式转写（ADR-038）结构性地接不进来（`luna-provider`
    /// 只依赖 `luna-core`，看不见 `luna-engine` 的类型）。把接缝提到领域层之后，
    /// 「断线切回本地」才有可能：两条通道走同一个句柄类型。
    ///
    /// 这条断言的方式是**用一个与 whisper 无关的假实现走通整条接缝**——它一旦编译不过，
    /// 就说明接缝里又混进了某个引擎的私有类型。
    #[test]
    fn streaming_seam_is_provider_agnostic() {
        use crate::ids::StreamId;
        use std::sync::Arc;

        struct Lang;
        impl StreamLangControl for Lang {
            fn set_configured(&self, _configured: Option<String>) {}
            fn auto_detected(&self) -> Option<String> {
                Some("zh".to_owned())
            }
        }

        // 只造句柄，不实现整个 AIProvider——本测要证的是**句柄类型**不含引擎私有概念。
        let (window_tx, window_rx) = std::sync::mpsc::sync_channel(4);
        let (infer_tx, infer_rx) = std::sync::mpsc::sync_channel(4);
        let reg = StreamRegistration {
            window_tx,
            infer_rx,
            lang: Arc::new(Lang),
            release: StreamRelease::detached(),
        };
        let _ = StreamId::new();

        // 采集侧要做的三件事都只依赖领域类型。
        reg.window_tx
            .send(Some(crate::job::WindowInput {
                samples: vec![0.0; 4],
                abs_start: 0,
                speech_len: 4,
            }))
            .expect("投窗");
        assert_eq!(reg.lang.auto_detected().as_deref(), Some("zh"));
        reg.lang.set_configured(None);
        let _ = infer_tx.send(12);
        assert_eq!(reg.infer_rx.recv().ok(), Some(12), "每窗耗时通道要能用");
        assert!(window_rx.recv().is_ok());

        // 关流 = 发 None（本机与远程同一约定）。
        reg.window_tx.send(None).expect("关流");
    }
    use super::*;

    #[test]
    fn cancel_token_flips_once() {
        let t = CancelToken::new();
        assert!(!t.is_cancelled());
        let clone = t.clone();
        t.cancel();
        assert!(t.is_cancelled());
        // 克隆共享同一状态。
        assert!(clone.is_cancelled());
    }

    #[test]
    fn only_cloud_is_external() {
        assert!(ProviderLocation::Cloud.is_external());
        assert!(!ProviderLocation::Device.is_external());
        assert!(!ProviderLocation::LocalService.is_external());
    }

    #[test]
    fn capability_set_supports() {
        let set = CapabilitySet {
            capabilities: [Capability::Translate, Capability::Refine]
                .into_iter()
                .collect(),
            streaming: true,
            location: ProviderLocation::LocalService,
            explicit_only: false,
        };
        assert!(set.supports(Capability::Translate));
        assert!(!set.supports(Capability::Transcribe));
    }

    #[test]
    fn provider_health_ready() {
        let h = ProviderHealth::ready();
        assert!(h.is_ready());
    }

    // ── 会话登记表：登记 → 摘走 / 回收（`StreamRegistry`）─────────────────────

    /// `execute` 摘走之后，回收句柄必须是**空操作**——不然它会把下一条同 id 会话撤掉，
    /// 而 `execute` 摘走与采集侧丢句柄的先后顺序是不确定的。
    #[test]
    fn taking_an_entry_makes_its_release_a_no_op() {
        use crate::ids::StreamId;

        let registry = StreamRegistry::<u32>::new();
        let id = StreamId::new();
        let release = registry.register(id, 7);
        assert_eq!(registry.take(id), Some(7), "execute 应能摘到自己那条");
        assert!(registry.is_empty());

        // 摘走之后再登记一条同 id 的（等价于「同一条流被重新登记」）。
        let _second = registry.register(id, 9);
        drop(release);
        assert_eq!(registry.len(), 1, "旧句柄不得撤掉新登记");
    }

    /// **`execute` 没跑到时，登记由句柄回收，且窗缓冲当场释放。**
    ///
    /// 断言两件事而不是一件：表里没有条目 **且** 采集侧再投窗会得到 `Disconnected`
    /// ——后者才是那 40 MB 的直接证据（`std::sync::mpsc` 的缓冲随**接收端**被丢才释放，
    /// 只把发送端丢掉的话，队列里那 64 窗仍好端端地挂在通道上）。
    ///
    /// 反向验证：把 `Drop for StreamRelease` 的函数体清空（= 回到「只有 execute 会
    /// remove」），本用例两条断言同时红。
    #[test]
    fn dropping_the_handle_recycles_an_entry_execute_never_took() {
        use crate::ids::StreamId;
        use crate::job::WindowInput;

        let registry = StreamRegistry::new();
        let id = StreamId::new();
        let (window_tx, window_rx) = std::sync::mpsc::sync_channel::<Option<WindowInput>>(4);
        let release = registry.register(id, window_rx);

        // 采集侧在 Job 失败之前一直在投窗：这些窗此刻攥在通道里。
        let window = || {
            Some(WindowInput {
                samples: vec![0.0; 16],
                abs_start: 0,
                speech_len: 16,
            })
        };
        window_tx.try_send(window()).expect("登记还在，投窗应成功");

        // Job 在 execute 之前就失败了——采集侧收到终态，丢掉回收句柄。
        drop(release);

        assert!(
            registry.is_empty(),
            "登记滞留在表里，它攥着的窗缓冲永远不会被释放"
        );
        assert!(
            matches!(
                window_tx.try_send(window()),
                Err(std::sync::mpsc::TrySendError::Disconnected(_))
            ),
            "接收端还在 = 缓冲还在"
        );
    }
}
