//! 结构化、可执行修复的错误模型。零 IO。事实源：TDD-01 §2.4、§5.6；TDD-15 §1.7。
//!
//! 每个错误携带稳定**错误码**（SCREAMING_SNAKE，不随文案变，供 i18n/Fluent key 映射，
//! TDD-15 §4.3）与可选的面向用户的 `hint`；前端据此展示"修复"按钮而非堆栈。

use crate::capability::Capability;
use crate::ids::{ModelId, ProviderId};
use crate::job::ServiceClass;
use crate::provider::ProviderLocation;
use serde::{Deserialize, Serialize};

/// 一个候选 Provider 失败后，调度器对下一候选的处置。
///
/// 这与 [`JobError::is_retriable`] 刻意分开：后者回答“同一路由稍后重试有没有意义”，
/// 本类型回答“能不能把同一个 Job 悄悄换到另一条执行腿”。把两个问题混成一个布尔值，
/// 会让限流、服务端 5xx 等“端点仍可达”的错误也被误判为本地兜底信号。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailoverDisposition {
    /// 当前云候选不可达，尝试下一个云候选。
    TryNextCloud,
    /// 云端不可达，且下一候选在设备侧；允许在首次可见输出前降到本地。
    FallBackToLocal,
    /// 本地候选不可达，尝试另一条本地候选。
    TryNextLocal,
    /// 本地候选不可达，且用户显式选了「本机优先，不可用时用服务端」：交给外部候选（ADR-117）。
    EscalateToCloud,
    /// 不允许换腿；保留原错误交给调用方处理。
    Stop,
}

impl FailoverDisposition {
    /// 是否允许调度器继续执行下一候选。
    #[must_use]
    pub fn continues(self) -> bool {
        !matches!(self, Self::Stop)
    }
}

/// 需要的系统权限种类（macOS 优先，TDD-05 §4）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionKind {
    /// 麦克风。
    Microphone,
    /// 辅助功能（双击复制监听/文本注入）。
    Accessibility,
    /// 输入监控。
    InputMonitoring,
    /// 通知。
    Notification,
    /// 屏幕录制（系统音频 loopback）。
    ScreenRecording,
}

/// Job 执行错误。
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobError {
    /// 模型缺失 → 引导下载。
    #[error("model missing: {model}")]
    ModelMissing {
        /// 缺失的模型。
        model: ModelId,
        /// 修复建议。
        hint: String,
    },
    /// 权限未授予 → 引导授权。
    #[error("permission not granted: {kind:?}")]
    PermissionDenied {
        /// 权限种类。
        kind: PermissionKind,
    },
    /// 账户会话被拒（未登录 / 会话失效）→ 稳定码 `SIGN_IN_REQUIRED`，引导去账户页重新登录。
    ///
    /// 客户端只剩账户会话这一种云凭据（ADR-155），故没有「被拒的是哪一份」之分。
    ///
    /// **不复用 [`JobError::PermissionDenied`]**：那条说的是**系统权限**
    /// （麦克风/辅助功能…，见 [`PermissionKind`]），修复动作是去系统设置里授权；这条说的是
    /// **账户会话**，修复动作是重新登录。把两者混成一条，UI 就只能给出一个
    /// 对其中一半用户必然是错的按钮。
    ///
    /// **也不能留在 [`JobError::Upstream`] 里**：`Upstream` 是可重试的，而同一份
    /// （缺失或已失效的）凭据重试多少次都还是 401——与 402 必须归 `QuotaExhausted`
    /// 是同一个教训（重试永不成功，对用户是无声的卡死）。
    #[error("account session rejected [{provider}]")]
    Unauthorized {
        /// 拒绝凭据的 Provider。
        provider: ProviderId,
    },
    /// 没有为该能力装配任何 Provider。
    ///
    /// Job 的输入仍可能完全有效；这只是设备/服务端配置尚未形成一条可执行链。把它归
    /// [`Self::InvalidInput`] 会让用户去改正文，并掩盖真正应检查的模型、端点或装配状态。
    #[error("no provider is wired up for capability {capability:?}")]
    ProviderNotConfigured {
        /// 尚未形成执行链的能力。
        capability: Capability,
    },
    /// 资源不足 → 建议更小模型/释放显存。
    ///
    /// **只指本机资源**（内存/显存）。服务端额度用尽是 [`JobError::QuotaExhausted`]
    /// ——两者的修复动作毫无交集（换小模型 vs 等周期重置/找管理员加量），
    /// 混成一条就意味着对其中一半用户必然给错建议。
    #[error("insufficient resources: {detail}")]
    ResourceExhausted {
        /// 详情。
        detail: String,
    },
    /// **服务端额度用尽** → 等周期重置，或联系管理员加量。
    ///
    /// 与 [`JobError::ResourceExhausted`] 分开的理由见那条。此前两者共用一个码，
    /// 于是配额用尽的用户被告知「请关闭其他模型任务或选择更小的模型」——一条与他的
    /// 处境毫无关系的建议，照做也不会有任何改善。
    ///
    /// 不可重试：额度不会因为再试一次而回来（与 `Unauthorized` 同一类）。
    #[error("server quota exhausted{}", dimension.as_ref().map(|d| format!(" ({d})")).unwrap_or_default())]
    QuotaExhausted {
        /// **是哪一维**超了（`tokens_out` / `asr_seconds` / …）。
        ///
        /// 用户看到「额度用尽」却不知道是 token 还是转写分钟，就无法判断该等周期重置
        /// 还是该找管理员加量。`None` = 上游只回了 402 而没说维度（我们不猜）。
        dimension: Option<String>,
    },
    /// **账户所在组织没开放这项能力**（服务端 403 `capability_not_allowed`，统一登录 R4b）。
    ///
    /// 与 [`JobError::QuotaExhausted`] 分开：额度用尽是「等重置或加量」，这一条是「这项能力
    /// 不归服务端管，回本机跑」——路由据它回落到本机腿（见 [`Self::failover_disposition_with`]）。
    /// 不可重试：策略不会因为再试一次就变。
    #[error("capability not allowed by the account's policy [{provider}]")]
    CapabilityNotAllowed {
        /// 拒绝它的那个 Provider（服务端）。
        provider: ProviderId,
    },
    /// **账户因付款争议被冻结为只读**（服务端 403 `account_frozen`，纵切 3 C3）。
    ///
    /// 与 [`JobError::QuotaExhausted`] 分开：额度用尽的下一步是「升级或等重置」，冻结的下一步
    /// 是**联系支持**——给一颗「升级」只会把用户送去一个点了也 403 的结账页。与
    /// [`JobError::Unauthorized`] 也分开：会话是好的，登录、读权益、读用量都照常。
    ///
    /// 处置与额度用尽同一侧：不可重试、结构性、不换腿（冻结是账户状态，换下一句、换一条腿都
    /// 还是同一个账户）；实时字幕据此停掉云腿而不是反复重连。
    #[error("account is frozen [{provider}]")]
    AccountFrozen {
        /// 拒绝它的那个 Provider（服务端）。
        provider: ProviderId,
    },
    /// **撞上了用户自设的本月预付消费上限**（服务端 402 `spend_cap_reached`，纵切 3 C4，
    /// PRD-13-040 ④）。
    ///
    /// 与 [`JobError::QuotaExhausted`] 分开：额度用尽的下一步是「升级或购买」，这一条的下一步是
    /// **调整你自己设的上限**——上限是用户写下的预算，给一颗「购买」等于劝他绕过自己的预算。
    /// 上限只管预付 credits，订阅包含的额度从不受它影响（用户裁决 2026-10-14）。
    ///
    /// 处置与额度用尽同一侧：不可重试、结构性、不换腿（上限是账户上的一个数，换下一句、换一条
    /// 腿都还是同一个上限）；实时字幕在下一个计量窗口撞上它时停掉云腿而不是反复重连，已产出保留。
    #[error("monthly prepaid spend cap reached [{provider}]")]
    SpendCapReached {
        /// 拒绝它的那个 Provider（服务端）。
        provider: ProviderId,
    },
    /// **这个账户此刻领不到免费额度**（服务端 402 `free_quota_withheld`，纵切 3 C5，
    /// PRD-13-018 L1 / L2）。
    ///
    /// 两种成因由服务端判定、写在权益 `free_quota` 里：这台设备上已有别的账户领过（免费额度挂
    /// 「账户 × 设备」），或要先做一次零扣费的付款方式验证。与 [`JobError::QuotaExhausted`] 分开：
    /// 额度用尽的下一步是「升级或购买」，这一条的下一步要看成因——验证一次付款方式（不扣费）就能
    /// 恢复的人，不该被一颗「购买」劝去花钱。账户页「订阅与用量」一段按成因说清下一步。
    ///
    /// 处置与额度用尽同一侧：不可重试、结构性、不换腿（是账户 × 设备上的一个判定，换下一句、换
    /// 一条腿都还是同一个判定）；实时字幕据此停掉云腿而不是反复重连，已产出保留。
    #[error("free quota withheld [{provider}]")]
    FreeQuotaWithheld {
        /// 拒绝它的那个 Provider（服务端）。
        provider: ProviderId,
    },
    /// **请求频率 / 并发受限** → 等待服务端给出的窗口后重试，或由路由切到其它 Provider。
    ///
    /// 与 [`JobError::QuotaExhausted`] 必须分开：429 表示短期流量闸，重试会恢复；额度耗尽
    /// 表示账本没有余额，原样重试不会恢复。两者都可能来自 HTTP 429，故不能靠状态码合并。
    #[error("rate limited [{provider}]")]
    RateLimited {
        /// 正在限流的 Provider。
        provider: ProviderId,
        /// 服务端建议的最短等待秒数；`None` = 对方未提供，调用方使用既有退避策略。
        retry_after_secs: Option<u64>,
    },
    /// **服务端已经受理过这次请求**（幂等键命中：正在处理，或已完成而产出不可重放）。
    ///
    /// **不能留在 [`JobError::Upstream`] 里**，那是这条存在的全部理由：`Upstream`
    /// 可重试，而拿同一个幂等键重试**永远**不会成功——服务端每次都会给出同一个裁决。
    /// 落进可重试桶的后果是队列白白烧掉候选链，而界面显示一句无从下手的「上游出错」
    /// （前端读的是变体投影出的稳定码，塞在 `Upstream.code` 里的字符串它根本看不到）。
    /// 同一个教训在 `Unauthorized` 与 `QuotaExhausted` 上各付过一次。
    ///
    /// **两种服务端裁决合成一条**（`idempotency_in_flight` / `idempotency_replayed`）：
    /// 对用户它们的下一步是同一个——**重新发起一次**。分成两条要多一份文案与一次
    /// 前端分支，而换来的区别只是「还在跑」与「已经跑完」，两者都拿不回这次的结果。
    /// 服务端仍分两个码（那一侧要能区分，见 TDD-16 §5.3.2）。
    #[error("the server already accepted this request [{provider}]")]
    AlreadyProcessed {
        /// 给出该裁决的 Provider。
        provider: ProviderId,
    },
    /// Provider 不可用 → 触发兜底路由。
    #[error("provider unavailable: {provider}")]
    ProviderUnavailable {
        /// 不可用的 Provider。
        provider: ProviderId,
    },
    /// **服务端队列在承诺窗口内没排上**（ADR-049 ⑦）。
    ///
    /// 服务端到点把它落成 `expired`：释放预留、**不计费**、音频留着以便重试。
    ///
    /// **不归 [`JobError::Upstream`]**（那是可自动重试的）：立刻重试只会把它排到
    /// 同一条排不动的队伍后面，而那条队伍此刻该被让给正在等的人。也**不归
    /// [`JobError::RateLimited`]**：那条带一个「等这么久再来」的窗口，而这里服务端
    /// 恰恰答不上来——排不上正是因为它不知道要多久。
    ///
    /// 用户的下一步是**他自己的选择**（换个时段再发，或把这一件从后台档提到普通档），
    /// 故不可自动重试，但界面要给一个显式的重试入口。
    #[error("the server queue did not admit this job within its promised window [{provider}]")]
    QueueExpired {
        /// 给出该终态的 Provider。
        provider: ProviderId,
    },
    /// **任务已经跑完，但结果取不回来了**——服务端在终态上没有给出产物。
    ///
    /// 触发面只有一处：远程批式转写在 `finished` 上 GET 结果，而响应里**没有 `segments`
    /// 这个字段**（结果已被取走——那一侧是「取走即删」——或已过期，或行与缓存尚未一致）。
    ///
    /// **存在的全部理由是「缺席」与「空」不是同一件事**。服务端刻意把两者分开：过期的
    /// 结果回 `null` 而不是 `[]`，因为空数组在客户端表现为「转写出来是空的」，与「结果已
    /// 过期」完全同形。客户端此前用 `unwrap_or_default()` 把这两件事又合回一处，于是
    /// 一段 24 分钟、服务端明明 `finished` 的录音，在笔记里落成了一个空转写块 + 一个
    /// 写满了的 `model` 字段——用户看到「暂无转写内容」，没有任何错误，也无从知道该重试。
    /// 「成功但是空的」是所有失败形态里最贵的一种：它会**写进 Vault**（唯一事实源），
    /// 而错误不会。
    ///
    /// **不归 [`JobError::Upstream`]**：那是可自动重试的，而自动重试在这里既拿不回
    /// 已经不存在的结果，又是一次**全新的上游调用与全新的计费**——用户没有同意第二次
    /// 付费。与 `QuotaExhausted` / `Unauthorized` 落在不可重试一侧是同一个判据。
    ///
    /// **也不归 [`JobError::AlreadyProcessed`]**：那条说的是「你这次请求撞上了幂等键，
    /// 服务端此前已经受理过」，用户读到的是「我是不是重复提交了」；这条说的是「这一次
    /// 就是你发的那一次，它跑完了，只是产物没了」。两者的下一步动作恰好相同（重新发起），
    /// 但**说给用户的话完全不同**，而错误的价值就在那句话上。
    ///
    /// 用户的下一步是**他自己的选择**：重新发起一次二次转写（新的一次调用、新的一次计费）。
    /// 故不可自动重试，但界面要给显式的重试入口。
    #[error("the server finished but the result could not be fetched [{provider}]")]
    ResultUnavailable {
        /// 给出该终态的 Provider。
        provider: ProviderId,
    },
    /// **服务端上这一档的候选链一个成员都没有**（服务端码 `route_lane_empty`，
    /// ADR-083 ⑧）。
    ///
    /// **与 [`JobError::ProviderUnavailable`] 分成两条，这是它存在的全部理由**：那条说的是
    /// 「有候选，但此刻都不行」——可重试、可换腿、可回落本机；这条说的是**运营者还没往那条
    /// 泳道上排任何东西**（或者他刻意清空了它）。两者混成一个码的代价此前是常态发生的：
    /// 服务端翻译链为空时客户端把它读成 `ProviderUnavailable`，于是**每一句**实时译文都先打
    /// 一次服务端、失败、再回本机——延迟翻倍，而用户看见的只是「译文慢」，运营那侧的账本上
    /// 则是一排看起来像故障的记录。
    ///
    /// **不可重试、不换腿**（[`Self::failover_disposition`] 恒 `Stop`）：泳道成员是运营者
    /// 在管理端排出来的一份名单，它不会因为再问一次、或者换一条腿就凭空多出一个成员。
    /// 用户（或运营者）的下一步是**去那条泳道上加一个候选**，而不是等。
    ///
    /// **[`Self::is_structural`] 恒为真**：换下一句话也一样空。逐句/逐单元的消费者据此
    /// 把它升格为**一次**会话级信号（实时管线）或**整批停**（批量循环），而不是把一百次注定
    /// 失败的请求发出去、最后交回一份「成功了但一句译文都没有」的结果。
    #[error("no candidate on the server chain for {capability:?} / {}", class.as_str())]
    RouteLaneEmpty {
        /// 空的是哪条能力链。
        capability: Capability,
        /// 空的是哪一档（同一个能力的两档各有各的链，见 ADR-083 ①）。
        class: ServiceClass,
    },
    /// 严格本地模式阻止外发型 Provider。
    #[error("strict-local mode blocked outbound routing for capability {capability:?}")]
    StrictLocalBlocked {
        /// 被阻止的能力。
        capability: Capability,
    },
    /// **本机会话池在宽限期内没腾出额度**，这条流式腿没能开张（ADR-078 ①）。
    ///
    /// **不归 [`JobError::ResourceExhausted`]**：那条说的是内存/显存不够，修复动作是
    /// 换小模型；这条说的是本机的**实时会话额度**被占着，而会话额度是按采集轨数派生的
    /// ——它满了只可能是上一条会话没收干净，换小模型一点用都没有。
    ///
    /// **也不归 [`JobError::Timeout`]**：超时是「跑起来了但太慢」，可重试；这条是
    /// 「压根没跑起来」，而原样重试只会再排一次同一条队。
    ///
    /// 不可重试：额度不会因为再问一次就出现。用户的下一步是**结束上一场会话**——
    /// 而在此期间录音本身照常进行、音频照常落盘，停录后的二次转写补齐权威结果。
    #[error("no local realtime session slot: still not admitted after waiting {waited_ms}ms")]
    LocalSessionBusy {
        /// 放弃前实际等了多久（毫秒）。脱敏：只有时长。
        waited_ms: u64,
    },
    /// 超时。
    #[error("timed out")]
    Timeout,
    /// 已取消。
    #[error("cancelled")]
    Cancelled,
    /// 输入非法。
    #[error("invalid input: {0}")]
    InvalidInput(String),
    /// **服务端受理了这次请求，但它自己那一侧没能把它交出去**（服务端码
    /// `upstream_unavailable` / `internal`，TDD-16 §5.3 映射表）。
    ///
    /// **与 [`Self::Upstream`] 分成两条，这是它存在的全部理由**：`Upstream` 说的是
    /// 「**我们直连的那个**上游回了错」——随包引擎的错误帧，用户手上有那条链路的
    /// 全部旋钮（换模型、重启引擎）。这条说的是
    /// **我们连的服务端是好的，它背后那一层出了问题**：它选中的上游此刻不可用，或它
    /// 自己内部出错了。两者混成一个码的代价是具体的，且真机坐实过（2026-08-31）：
    /// 组织服务端的 `translate/realtime` 泳道上游挂掉时，弹窗翻译显示的是
    /// 「AI 引擎返回错误，请重试」——用户手上没有任何一个旋钮能改变它，而同一台服务端
    /// 的批量档同一时刻完全正常，于是他既无法自查、也无从求助。
    ///
    /// **可重试**（上游会恢复、内部错误常是瞬时的），但用户真正做得到的是：稍后再试，
    /// 或改用本机模型；管理员那侧则要去看这一档链上的候选健康。
    #[error("server-side failure [{provider}]: {code}")]
    ServerSideFailure {
        /// 给出该终态的 Provider。
        provider: ProviderId,
        /// 服务端自述的稳定码（`upstream_unavailable` / `internal`）——只进日志与用量
        /// 账本供运维定位，**不上界面**（界面读的是 [`Self::code`] 投影出的稳定码）。
        code: String,
    },
    /// **服务端这台部署配错了**（服务端码 `egress_blocked` / `upstream_protocol`）。
    ///
    /// **不可重试，这是它与 [`Self::ServerSideFailure`] 唯一的、也是全部的分界**：
    /// 出站被网关策略拒绝、上游回了服务端认不出的协议——两者都不会因为再问一次就变对，
    /// 而它们此前落在 `Upstream`（可重试）里，表现是队列白烧候选链、界面写着「请重试」
    /// 而重试永远不会成功。同一个教训在 `Unauthorized`（401 重试还是 401）、
    /// `QuotaExhausted`（402）与 `RouteLaneEmpty` 上各付过一次。
    ///
    /// **[`Self::is_structural`] 恒为真**：换下一句话也一样。逐句消费者据此一次性
    /// 升格为会话级信号，而不是逐句失败一百次。
    ///
    /// 用户改不动它，但要说清这是**服务端侧**的事而不是他的网络：下一步是找管理员，
    /// 或先切回本机模型。
    #[error("server misconfigured [{provider}]: {code}")]
    ServerMisconfigured {
        /// 给出该终态的 Provider。
        provider: ProviderId,
        /// 服务端自述的稳定码（`egress_blocked` / `upstream_protocol`）。同上，只进
        /// 日志与用量账本。
        code: String,
    },
    /// **模型把这一次的输出额度全花在推理上，正文一个字都没有**（帧序列判定：见过
    /// `thinking`、拿到了正常终态、正文为空）。
    ///
    /// **与 [`Self::Upstream`] 分成两条的全部理由：这不是「上游回了错」，而是「上游
    /// 说它成功了，而结果是空的」**。判定发生在客户端——服务端如实转发了它收到的一切，
    /// 没有任何一帧是错误帧，故服务端侧不可能给出这个码。真机坐实（2026-08-31）：组织
    /// 服务端 `summarize/batch` 泳道上一个推理模型连发 130–156 个 `thinking` 帧、正文帧
    /// 0 个而终态为 `done`，四次里三次如此。此前它落在 `Upstream` 里，界面显示「AI 引擎
    /// 返回错误，请重试」——话没说错，但用户既不知道发生了什么，也不知道反复失败时该
    /// 找谁；而 `stream_terminal` 那句写好的可行动指引因为 DTO 只投影 [`Self::code`]，
    /// 一个字都到不了界面上。
    ///
    /// **只由服务端落点产出**（`luna-provider::cloud_llm`）：只有那条链把推理单独发成
    /// `thinking` 帧，客户端因此分得清「思考吃光了预算」与「真的什么都没产出」。
    ///
    /// **可重试，且重试是正确的第一步**：服务端每次受理都重新挑一次上游，换一个候选就
    /// 出结果（实测四次里有一次同一模型自己产出了正文）。反复如此则是运营侧的事——那条
    /// 泳道上站着一个推理模型，要么给它配上「不返回推理」的请求覆盖，要么挪出去。
    #[error("upstream produced reasoning but no content [{provider}]")]
    EmptyAfterThinking {
        /// 给出该终态的 Provider。
        provider: ProviderId,
    },
    /// **输入超过了模型这一次加载的上下文窗口**（本机推理引擎如实拒绝、绝不截断，ADR-143）。
    ///
    /// 与 [`Self::Upstream`] 分开的理由：超窗输入若被静默截断，用户看到的是「总结得不好」
    /// 而不是任何错误；随包引擎当场拒绝——对话超窗回 400 `exceed_context_size_error`，
    /// 编码器嵌入超批回 500「input is too large」（P0 实测，状态码不同、语义相同）。落进 `Upstream` 的话它可重试，于是同一段超长输入要原样再撞
    /// 几次才报一句「AI 引擎返回错误」——而用户真正能做的是**缩短输入**，或由调用方分块重来。
    ///
    /// **不可重试**（原样再发还是超窗）；**不是结构性的**（下一句未必这么长）。
    #[error("input too long [{provider}]: {input_tokens:?} tokens > window {window:?}")]
    InputTooLong {
        /// 给出该终态的 Provider。
        provider: ProviderId,
        /// 引擎自报的输入 token 数（不报则 `None`，不估）。
        input_tokens: Option<u32>,
        /// 引擎自报的窗口 / 批大小（不报则 `None`）。
        window: Option<u32>,
    },
    /// 上游错误（含云/在线 API）。
    #[error("upstream error [{provider}]: {message}")]
    Upstream {
        /// 上游 Provider。
        provider: ProviderId,
        /// 上游错误码（可选）。
        code: Option<String>,
        /// 上游消息。
        message: String,
    },
}

/// 一个失败码（可带外层前缀，如 `TRANSCRIBE_JOB: RESULT_UNAVAILABLE`）对应的失败
/// **是否已经产生了一次计费**——即用户手动重跑时是一次新的、会再计费的调用。
///
/// 只有「服务端已跑完、结果取不回」（[`JobError::ResultUnavailable`]）属于这一类：
/// 排队过期（`QUEUE_EXPIRED`）明确未计费，其余失败没有产出。界面据此把后果写在
/// 重试按钮上，而不是只留在原因文案里（2026-10-08 文案与交互审查 C32）。
#[must_use]
pub fn rerun_charges_again(code: &str) -> bool {
    code.split(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
        .any(|token| token == "RESULT_UNAVAILABLE")
}

impl JobError {
    /// 判定能否从当前执行位置切到下一候选。
    ///
    /// 只有“链路不可达”才允许换腿：连接失败归 `ProviderUnavailable`，连接/首包超时归
    /// `Timeout`。鉴权、额度、限流、上游业务错误都证明端点已经可达，必须把真实问题
    /// 告诉用户，不能用本地结果静默掩盖。
    #[must_use]
    pub fn failover_disposition(
        &self,
        current: ProviderLocation,
        next: ProviderLocation,
    ) -> FailoverDisposition {
        self.failover_disposition_with(current, next, false)
    }

    /// 同 [`Self::failover_disposition`]，但 `allow_local_to_cloud` 为真时放开「本地不可达 →
    /// 外部候选」这一条（路由策略第四档「本机优先，不可用时用服务端」，ADR-117）。
    /// 放开的仍只是**链路不可达**：鉴权、额度、上游业务错误照旧 `Stop`。
    #[must_use]
    pub fn failover_disposition_with(
        &self,
        current: ProviderLocation,
        next: ProviderLocation,
        allow_local_to_cloud: bool,
    ) -> FailoverDisposition {
        // 服务端没开放这项能力：只有「云 → 本机」这一条路有意义（本机不受组织策略管）。
        if matches!(self, Self::CapabilityNotAllowed { .. }) {
            return if current.is_external() && !next.is_external() {
                FailoverDisposition::FallBackToLocal
            } else {
                FailoverDisposition::Stop
            };
        }
        let unavailable = matches!(self, Self::ProviderUnavailable { .. } | Self::Timeout);
        if !unavailable {
            return FailoverDisposition::Stop;
        }

        match (current.is_external(), next.is_external()) {
            (true, true) => FailoverDisposition::TryNextCloud,
            (true, false) => FailoverDisposition::FallBackToLocal,
            (false, false) => FailoverDisposition::TryNextLocal,
            // 本地失败后自动把数据送上云会反转用户预期，也可能越过联网同意——
            // 只有用户显式选了第四档才放开（那一档的说明文案写明了这件事）。
            (false, true) if allow_local_to_cloud => FailoverDisposition::EscalateToCloud,
            (false, true) => FailoverDisposition::Stop,
        }
    }

    /// 稳定错误码（SCREAMING_SNAKE，不随文案变；TDD-15 §1.7/§4.3 的 i18n/Fluent key 映射地基）。
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::ModelMissing { .. } => "MODEL_MISSING",
            Self::PermissionDenied { .. } => "PERMISSION_DENIED",
            Self::Unauthorized { .. } => "SIGN_IN_REQUIRED",
            Self::ProviderNotConfigured { .. } => "PROVIDER_NOT_CONFIGURED",
            Self::ResourceExhausted { .. } => "RESOURCE_EXHAUSTED",
            Self::QuotaExhausted { .. } => "QUOTA_EXHAUSTED",
            Self::CapabilityNotAllowed { .. } => "CAPABILITY_NOT_ALLOWED",
            Self::AccountFrozen { .. } => "ACCOUNT_FROZEN",
            Self::SpendCapReached { .. } => "SPEND_CAP_REACHED",
            Self::FreeQuotaWithheld { .. } => "FREE_QUOTA_WITHHELD",
            Self::RateLimited { .. } => "RATE_LIMITED",
            Self::AlreadyProcessed { .. } => "ALREADY_PROCESSED",
            Self::ProviderUnavailable { .. } => "PROVIDER_UNAVAILABLE",
            Self::QueueExpired { .. } => "QUEUE_EXPIRED",
            Self::ResultUnavailable { .. } => "RESULT_UNAVAILABLE",
            Self::RouteLaneEmpty { .. } => "ROUTE_LANE_EMPTY",
            Self::StrictLocalBlocked { .. } => "STRICT_LOCAL_BLOCKED",
            Self::LocalSessionBusy { .. } => "LOCAL_SESSION_BUSY",
            Self::Timeout => "TIMEOUT",
            Self::Cancelled => "CANCELLED",
            Self::InvalidInput(_) => "INVALID_INPUT",
            Self::ServerSideFailure { .. } => "SERVER_SIDE_FAILURE",
            Self::ServerMisconfigured { .. } => "SERVER_MISCONFIGURED",
            Self::EmptyAfterThinking { .. } => "EMPTY_AFTER_THINKING",
            Self::InputTooLong { .. } => "INPUT_TOO_LONG",
            Self::Upstream { .. } => "UPSTREAM",
        }
    }

    /// 是否可重试。事实源：TDD-01 §5.6——`Timeout` / `ProviderUnavailable` /
    /// `Upstream`(5xx/限流) 可重试；其余需用户介入或重试无意义。
    ///
    /// `Unauthorized` 属于后者：凭据没换，重试就还是 401。
    #[must_use]
    pub fn is_retriable(&self) -> bool {
        matches!(
            self,
            Self::Timeout
                | Self::ProviderUnavailable { .. }
                | Self::RateLimited { .. }
                | Self::ServerSideFailure { .. }
                | Self::EmptyAfterThinking { .. }
                | Self::Upstream { .. }
        )
    }

    /// 这个错误是**结构性**的吗——重试改变不了它、换腿改变不了它，**换下一句也一样**。
    ///
    /// 逐句/逐单元的消费者（实时转译管线、批量转译循环）据它决定「本会话/本批到此为止」
    /// 而不是逐句失败一百次（ADR-083 ⑧，
    /// 处置见 TDD-08 §4）。判据是「这个错不会因为换一句就变对」——继续跑完只是把一百次
    /// 注定失败的请求发出去，并让调用方最后拿到一份「成功了但一句译文都没有」的结果。
    ///
    /// **[`JobError::Cancelled`] 恒 `false`**：用户自己按的停止不是故障，把它算进来
    /// 就会在每一次正常取消之后弹一条「翻译已停止」的横幅。
    ///
    /// **穷尽 match、不写 `_ =>`**：新增一个 `JobError` 变体必须是编译错误——静默落进
    /// 「非结构性」那一侧的表现，正是本条要消灭的那种逐句静默失败。
    #[must_use]
    pub fn is_structural(&self) -> bool {
        match self {
            // 模型没装、权限没给、凭据无效、额度耗尽、隐私档挡住——四者都是**配置**，
            // 而配置不会因为换下一句话就变了。
            Self::ModelMissing { .. }
            | Self::PermissionDenied { .. }
            | Self::Unauthorized { .. }
            | Self::ProviderNotConfigured { .. }
            | Self::QuotaExhausted { .. }
            | Self::CapabilityNotAllowed { .. }
            | Self::StrictLocalBlocked { .. } => true,
            // 账户被冻结：是账户状态，换下一句还是同一个账户（与额度用尽同一判据）。
            Self::AccountFrozen { .. } => true,
            // 本月预付上限：账户上的一个数，换下一句还是同一个上限（与额度用尽同一判据）。
            Self::SpendCapReached { .. } => true,
            // 免费额度被扣下：账户 × 设备上的一个判定，换下一句还是同一个判定（与额度用尽同一判据）。
            Self::FreeQuotaWithheld { .. } => true,
            // 泳道上一个成员都没有：这是运营者排出来的一份名单，它不会因为换下一句话就
            // 多出一个候选。逐句重试的表现是一百次注定失败的请求 + 用户侧零信号。
            Self::RouteLaneEmpty { .. } => true,
            // 服务端这台部署配错了（出网被拒、上游协议认不出）——与泳道为空同类：
            // 它是一份配置，不会因为换下一句话就变对。
            Self::ServerMisconfigured { .. } => true,
            // 可重试的四类：换一次、等一会儿就可能对，逐句继续是正确处置。
            // `ServerSideFailure` 在这一侧：上游会恢复，服务端内部错误常是瞬时的。
            Self::Timeout
            | Self::ProviderUnavailable { .. }
            | Self::RateLimited { .. }
            | Self::ServerSideFailure { .. }
            | Self::Upstream { .. } => false,
            // **不是结构性的，尽管它看着像**：这一句空了，下一句未必空——服务端每次
            // 受理都重新挑一次上游，且同一个推理模型也不是每次都把预算花光（实测四次里
            // 一次产出了正文）。判成结构性会让一整批转译在第一句空掉时整个中止，而那批
            // 里剩下的句子本来多半能出结果。
            Self::EmptyAfterThinking { .. } => false,
            // 本机资源不够是**随负载起落**的（别的任务跑完就腾出来了），不是结构性的。
            Self::ResourceExhausted { .. } => false,
            // 会话额度被上一场没收干净的会话占着——它会随那场会话结束而恢复，
            // 且已有 `AdmissionDelayed` 这条专门的可见信号（ADR-078 ③）。
            Self::LocalSessionBusy { .. } => false,
            // 三条都是**这一次调用**的终态（幂等键撞上、队列没排上、产物没了），
            // 下一句是全新的一次调用，与它们无关。
            Self::AlreadyProcessed { .. }
            | Self::QueueExpired { .. }
            | Self::ResultUnavailable { .. } => false,
            // 这一句的输入不合法 / 太长，下一句未必。
            Self::InvalidInput(_) | Self::InputTooLong { .. } => false,
            // 用户自己按的停止不是故障。
            Self::Cancelled => false,
        }
    }

    /// 面向用户的可执行修复建议（若有）。
    #[must_use]
    pub fn hint(&self) -> Option<&str> {
        match self {
            Self::ModelMissing { hint, .. } => Some(hint.as_str()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_codes() {
        assert_eq!(JobError::Timeout.code(), "TIMEOUT");
        assert_eq!(
            JobError::ModelMissing {
                model: ModelId::from("whisper-large-v3-turbo"),
                hint: "去下载".into()
            }
            .code(),
            "MODEL_MISSING"
        );
    }

    /// 组织没开放的能力（R4b）只走「云 → 本机」：本机不受组织策略管；云 → 云换一台还是同一份策略。
    #[test]
    fn a_capability_the_policy_does_not_allow_falls_back_to_local_only() {
        let err = JobError::CapabilityNotAllowed {
            provider: ProviderId::from("cloud"),
        };
        assert_eq!(err.code(), "CAPABILITY_NOT_ALLOWED");
        assert!(!err.is_retriable());
        assert!(err.is_structural());
        assert_eq!(
            err.failover_disposition(ProviderLocation::Cloud, ProviderLocation::LocalService),
            FailoverDisposition::FallBackToLocal
        );
        assert_eq!(
            err.failover_disposition(ProviderLocation::Cloud, ProviderLocation::Cloud),
            FailoverDisposition::Stop
        );
    }

    /// 账户被冻结（纵切 3 C3）：与额度用尽同一侧——不重试、不换腿、结构性；码独立，
    /// 界面据此说「联系支持」而不是「升级」。
    #[test]
    fn a_frozen_account_stops_like_an_exhausted_quota_but_has_its_own_code() {
        let err = JobError::AccountFrozen {
            provider: ProviderId::from("lunarca-cloud"),
        };
        assert_eq!(err.code(), "ACCOUNT_FROZEN");
        assert_ne!(
            err.code(),
            JobError::QuotaExhausted { dimension: None }.code()
        );
        assert!(!err.is_retriable());
        assert!(err.is_structural());
        for next in [ProviderLocation::Cloud, ProviderLocation::LocalService] {
            assert_eq!(
                err.failover_disposition(ProviderLocation::Cloud, next),
                FailoverDisposition::Stop
            );
        }
        let json = serde_json::to_string(&err).unwrap();
        let back: JobError = serde_json::from_str(&json).unwrap();
        assert_eq!(err, back);
    }

    /// 撞上本月预付上限（纵切 3 C4）：与额度用尽同一侧——不重试、不换腿、结构性；码独立，
    /// 界面据此说「调整上限」而不是「购买」。
    #[test]
    fn a_reached_spend_cap_stops_like_an_exhausted_quota_but_has_its_own_code() {
        let err = JobError::SpendCapReached {
            provider: ProviderId::from("lunarca-cloud"),
        };
        assert_eq!(err.code(), "SPEND_CAP_REACHED");
        assert_ne!(
            err.code(),
            JobError::QuotaExhausted { dimension: None }.code()
        );
        assert!(!err.is_retriable());
        assert!(err.is_structural());
        for next in [ProviderLocation::Cloud, ProviderLocation::LocalService] {
            assert_eq!(
                err.failover_disposition(ProviderLocation::Cloud, next),
                FailoverDisposition::Stop
            );
        }
        let json = serde_json::to_string(&err).unwrap();
        let back: JobError = serde_json::from_str(&json).unwrap();
        assert_eq!(err, back);
    }

    /// 免费额度被扣下（纵切 3 C5）：与额度用尽同一侧——不重试、不换腿、结构性；码独立，
    /// 界面据此去账户页按成因说下一步，而不是给「购买」。
    #[test]
    fn withheld_free_quota_stops_like_an_exhausted_quota_but_has_its_own_code() {
        let err = JobError::FreeQuotaWithheld {
            provider: ProviderId::from("lunarca-cloud"),
        };
        assert_eq!(err.code(), "FREE_QUOTA_WITHHELD");
        assert_ne!(
            err.code(),
            JobError::QuotaExhausted { dimension: None }.code()
        );
        assert!(!err.is_retriable());
        assert!(err.is_structural());
        for next in [ProviderLocation::Cloud, ProviderLocation::LocalService] {
            assert_eq!(
                err.failover_disposition(ProviderLocation::Cloud, next),
                FailoverDisposition::Stop
            );
        }
        let json = serde_json::to_string(&err).unwrap();
        let back: JobError = serde_json::from_str(&json).unwrap();
        assert_eq!(err, back);
    }

    #[test]
    fn retriable_classification_matches_tdd_01_5_6() {
        assert!(JobError::Timeout.is_retriable());
        assert!(JobError::ProviderUnavailable {
            provider: ProviderId::from("local-llm")
        }
        .is_retriable());
        assert!(JobError::Upstream {
            provider: ProviderId::from("openai-compat"),
            code: Some("503".into()),
            message: "overloaded".into()
        }
        .is_retriable());
        assert!(JobError::RateLimited {
            provider: ProviderId::from("openai-compat"),
            retry_after_secs: Some(3),
        }
        .is_retriable());

        assert!(!JobError::Cancelled.is_retriable());
        assert!(!JobError::InvalidInput("empty".into()).is_retriable());
        assert!(!JobError::PermissionDenied {
            kind: PermissionKind::Microphone
        }
        .is_retriable());
        assert!(!JobError::StrictLocalBlocked {
            capability: Capability::Translate
        }
        .is_retriable());
        assert!(!JobError::ModelMissing {
            model: ModelId::from("x"),
            hint: "y".into()
        }
        .is_retriable());
        // 凭据没换，重试就还是 401：留在可重试一侧会让队列反复退避到用户放弃，
        // 而那段时间里界面与"正在生成"完全同形——与 402 归 `QuotaExhausted` 同一教训。
        assert!(!JobError::Unauthorized {
            provider: ProviderId::from("remote-asr"),
        }
        .is_retriable());
        assert!(!JobError::ProviderNotConfigured {
            capability: Capability::Chat
        }
        .is_retriable());
        // 服务端**配错了**：出网被拒 / 上游协议认不出，重试永远得到同一个裁决。
        // 它们此前借 `Upstream`（可重试）表达，代价是队列白烧候选链 + 一句「请重试」。
        assert!(!JobError::ServerMisconfigured {
            provider: ProviderId::from("lunarca-cloud"),
            code: "egress_blocked".into(),
        }
        .is_retriable());
        // 服务端背后那一层此刻不可用：它会恢复，留在可重试一侧才对。
        assert!(JobError::ServerSideFailure {
            provider: ProviderId::from("lunarca-cloud"),
            code: "upstream_unavailable".into(),
        }
        .is_retriable());
    }

    /// 「服务端背后那一层出了问题」不是「我们直连的上游回了错」（ADR-097）。
    ///
    /// 真机坐实（2026-08-31，组织服务端 10.0.0.62）：`translate/realtime` 泳道的上游挂掉，
    /// 服务端回 `502 {"error":"upstream_unavailable"}`，而**同一台服务端、同一把凭据**的
    /// `translate/batch` 同一秒 200 正常。客户端当时把它压成 `UPSTREAM`，界面显示
    /// 「AI 引擎返回错误，请重试」——用户手上一个能改变结果的旋钮都没有，而屏幕让他重试。
    #[test]
    fn a_server_side_failure_is_not_the_same_code_as_an_upstream_error() {
        let server = JobError::ServerSideFailure {
            provider: ProviderId::from("lunarca-cloud"),
            code: "upstream_unavailable".into(),
        };
        let upstream = JobError::Upstream {
            provider: ProviderId::from("openai-compat"),
            code: Some("503".into()),
            message: "overloaded".into(),
        };
        assert_eq!(server.code(), "SERVER_SIDE_FAILURE");
        assert_eq!(upstream.code(), "UPSTREAM");
        assert_ne!(server.code(), upstream.code());
        // 两者都可重试，但**说给用户的话不同**——而错误的价值就在那句话上。
        assert!(server.is_retriable() && upstream.is_retriable());
        // 服务端已给出业务响应 = 端点可达 = 绝不是换腿信号（同 `Unauthorized` 判据）。
        assert_eq!(
            server.failover_disposition(ProviderLocation::Cloud, ProviderLocation::Device),
            FailoverDisposition::Stop
        );
    }

    #[test]
    fn candidate_failover_only_accepts_unreachable_routes() {
        let cloud = ProviderLocation::Cloud;
        let local = ProviderLocation::LocalService;
        assert_eq!(
            JobError::Timeout.failover_disposition(cloud, local),
            FailoverDisposition::FallBackToLocal
        );
        assert_eq!(
            JobError::ProviderUnavailable {
                provider: ProviderId::from("cloud")
            }
            .failover_disposition(cloud, cloud),
            FailoverDisposition::TryNextCloud
        );
        for reachable_error in [
            JobError::RateLimited {
                provider: ProviderId::from("cloud"),
                retry_after_secs: Some(3),
            },
            JobError::Upstream {
                provider: ProviderId::from("cloud"),
                code: Some("503".into()),
                message: "overloaded".into(),
            },
            JobError::Unauthorized {
                provider: ProviderId::from("cloud"),
            },
        ] {
            assert_eq!(
                reachable_error.failover_disposition(cloud, local),
                FailoverDisposition::Stop
            );
        }
    }

    /// 「需要登录」有独立稳定码，且不与系统权限共用一条——前端据此给的是「去登录」，
    /// 而不是「去系统设置授权麦克风」。
    #[test]
    fn unauthorized_is_distinct_from_system_permission() {
        let e = JobError::Unauthorized {
            provider: ProviderId::from("remote-asr"),
        };
        assert_ne!(
            e.code(),
            JobError::PermissionDenied {
                kind: PermissionKind::Microphone
            }
            .code()
        );
        let json = serde_json::to_string(&e).unwrap();
        let back: JobError = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }

    /// 云通道只认账户会话（ADR-155）：被拒即「去登录」，且不可重试——凭据没换，重试还是 401。
    #[test]
    fn a_rejected_session_means_sign_in_and_is_not_retried() {
        let e = JobError::Unauthorized {
            provider: ProviderId::from("lunarca-cloud"),
        };
        assert_eq!(e.code(), "SIGN_IN_REQUIRED");
        assert!(!e.is_retriable(), "{e:?}");
        assert!(e.is_structural(), "{e:?}");
    }

    #[test]
    fn serde_round_trip_preserves_variant() {
        let e = JobError::Upstream {
            provider: ProviderId::from("openai-compat"),
            code: Some("429".into()),
            message: "rate limited".into(),
        };
        let json = serde_json::to_string(&e).unwrap();
        let back: JobError = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
        assert_eq!(back.code(), "UPSTREAM");
    }

    /// 重跑会再计费的只有「结果取不回」：带外层前缀也要认出来，排队过期（未计费）不算。
    #[test]
    fn only_an_unretrievable_result_charges_again_on_rerun() {
        assert!(rerun_charges_again("RESULT_UNAVAILABLE"));
        assert!(rerun_charges_again("TRANSCRIBE_JOB: RESULT_UNAVAILABLE"));
        assert!(!rerun_charges_again("QUEUE_EXPIRED"));
        assert!(!rerun_charges_again("RESULT_UNAVAILABLE_SOON"));
        assert!(!rerun_charges_again("FETCH_NETWORK"));
    }

    /// 「跑完了但结果取不回来」不可自动重试——重试是**新的一次调用、新的一次计费**。
    ///
    /// 留在可重试一侧的后果不是「多等一会儿」：队列会替用户自动再买一次转写，
    /// 而第一次的钱已经花了、结果已经没了。它与 `QuotaExhausted`/`Unauthorized`
    /// 落在同一侧是同一个判据（原样重试不可能成功，只会白白付出代价）。
    #[test]
    fn a_result_that_is_gone_is_not_something_to_retry_automatically() {
        let e = JobError::ResultUnavailable {
            provider: ProviderId::from("remote-asr"),
        };
        assert_eq!(e.code(), "RESULT_UNAVAILABLE");
        assert!(
            !e.is_retriable(),
            "自动重试拿不回已经不存在的结果，却会再计一次费"
        );
        // 也不许被当成「链路不可达」而静默换腿到本机——那会把一次云端转写的失败
        // 悄悄替换成一份本机结果，用户既不知道换了引擎，也不知道云端那次白花了钱。
        assert_eq!(
            e.failover_disposition(ProviderLocation::Cloud, ProviderLocation::LocalService),
            FailoverDisposition::Stop
        );
        let json = serde_json::to_string(&e).unwrap();
        let back: JobError = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }

    #[test]
    fn strict_local_blocked_has_stable_code_and_round_trips() {
        let e = JobError::StrictLocalBlocked {
            capability: Capability::Chat,
        };
        assert_eq!(e.code(), "STRICT_LOCAL_BLOCKED");
        let json = serde_json::to_string(&e).unwrap();
        let back: JobError = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }
    /// **结构性 = 重试与换腿都改变不了，换下一句也一样**（ADR-083 ⑧）。
    ///
    /// 逐句消费者据它决定「本会话/本批到此为止」。判错的代价是对称的两种：判宽了会在
    /// 一次瞬时抖动之后把整场会议的译文停掉；判窄了就退回本批要消灭的那个形态——一百句
    /// 发一百次注定失败的请求，界面零信号。
    #[test]
    fn structural_errors_are_the_ones_a_next_sentence_cannot_fix() {
        let structural: Vec<JobError> = vec![
            JobError::ModelMissing {
                model: ModelId::from("m"),
                hint: "装一个".into(),
            },
            JobError::PermissionDenied {
                kind: PermissionKind::Microphone,
            },
            JobError::Unauthorized {
                provider: ProviderId::from("p"),
            },
            JobError::ProviderNotConfigured {
                capability: Capability::Chat,
            },
            JobError::QuotaExhausted {
                dimension: Some("tokens_out".into()),
            },
            JobError::AccountFrozen {
                provider: ProviderId::from("p"),
            },
            JobError::SpendCapReached {
                provider: ProviderId::from("p"),
            },
            JobError::FreeQuotaWithheld {
                provider: ProviderId::from("p"),
            },
            JobError::StrictLocalBlocked {
                capability: Capability::Translate,
            },
            // 服务端那条链**一个成员都没有**：运营者排出来的名单不会因为换下一句就
            // 多出一个候选（ADR-083 ⑧）。它与 `ProviderUnavailable` 分列两侧正是本条
            // 用例的价值——后者就在下面那张表里。
            JobError::RouteLaneEmpty {
                capability: Capability::Translate,
                class: crate::job::ServiceClass::Realtime,
            },
        ];
        for err in &structural {
            assert!(err.is_structural(), "{} 应判结构性", err.code());
            // 结构性 ⇒ 不可重试、且不允许换腿（四种腿组合都是 `Stop`）。
            assert!(!err.is_retriable(), "{} 结构性却可重试", err.code());
            for current in [ProviderLocation::Cloud, ProviderLocation::Device] {
                for next in [ProviderLocation::Cloud, ProviderLocation::Device] {
                    assert_eq!(
                        err.failover_disposition(current, next),
                        FailoverDisposition::Stop,
                        "{} 结构性却允许换腿",
                        err.code()
                    );
                }
            }
        }

        let not_structural: Vec<JobError> = vec![
            JobError::Timeout,
            JobError::ProviderUnavailable {
                provider: ProviderId::from("p"),
            },
            JobError::RateLimited {
                provider: ProviderId::from("p"),
                retry_after_secs: None,
            },
            JobError::Upstream {
                provider: ProviderId::from("p"),
                code: None,
                message: "boom".into(),
            },
            JobError::ResourceExhausted {
                detail: "显存".into(),
            },
            JobError::LocalSessionBusy { waited_ms: 1_000 },
            JobError::AlreadyProcessed {
                provider: ProviderId::from("p"),
            },
            JobError::QueueExpired {
                provider: ProviderId::from("p"),
            },
            JobError::ResultUnavailable {
                provider: ProviderId::from("p"),
            },
            JobError::InvalidInput("empty".into()),
            // 用户自己按的停止不是故障。
            JobError::Cancelled,
        ];
        for err in &not_structural {
            assert!(!err.is_structural(), "{} 不该判结构性", err.code());
        }

        // 覆盖性：两张表加起来必须是**全部**变体。漏掉一个的表现是它静默落进
        // 「非结构性」那一侧，也就是本批要消灭的逐句静默失败。
        let mut codes: Vec<&str> = structural
            .iter()
            .chain(not_structural.iter())
            .map(JobError::code)
            .collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), 21, "新增 JobError 变体必须在这两张表里表态");
    }

    /// **`Cancelled` 单独再钉一条**：它是唯一一个「不可重试 + 不换腿」却**不**该触发
    /// 会话级横幅的变体。混进去的表现是每一次正常停录都弹一条「翻译已停止」。
    #[test]
    fn user_cancellation_is_never_structural() {
        assert!(!JobError::Cancelled.is_structural());
        assert!(!JobError::Cancelled.is_retriable());
        assert_eq!(
            JobError::Cancelled
                .failover_disposition(ProviderLocation::Device, ProviderLocation::Device),
            FailoverDisposition::Stop,
            "它确实不换腿——正因如此，只看 failover_disposition 会把它误判成结构性"
        );
    }

    /// **`route_lane_empty` 与 `upstream_unavailable` 在「云 → 本机」这一格上必须分家**
    /// （ADR-083 ⑧、⑦ 的已知代价 ②）。
    ///
    /// 这一格就是本条错误码存在的全部理由：服务端翻译链为空时，此前客户端把它读成
    /// `ProviderUnavailable` → `FallBackToLocal`，于是**每一句**实时译文都先打一次服务端、
    /// 失败、再回本机——延迟翻倍，而用户看见的只是「译文慢」。本用例把两者钉在一起对比，
    /// 因为单独看任何一条都看不出这个区别。
    ///
    /// 反向验证：把 `RouteLaneEmpty` 加进 `failover_disposition` 里那个 `unavailable`
    /// 的 `matches!`，第一条断言当场变红。
    #[test]
    fn an_empty_lane_never_falls_back_to_local_but_an_unavailable_provider_does() {
        let empty = JobError::RouteLaneEmpty {
            capability: Capability::Translate,
            class: crate::job::ServiceClass::Realtime,
        };
        assert_eq!(
            empty.failover_disposition(ProviderLocation::Cloud, ProviderLocation::Device),
            FailoverDisposition::Stop,
            "空泳道是运营者的一次显式选择，逐句回落本机会把延迟翻倍而用户零信号"
        );
        assert_eq!(
            JobError::ProviderUnavailable {
                provider: ProviderId::from("p"),
            }
            .failover_disposition(ProviderLocation::Cloud, ProviderLocation::Device),
            FailoverDisposition::FallBackToLocal,
            "「有候选但此刻都不行」的既有语义不动——两条码分家才有意义"
        );
        assert_eq!(empty.code(), "ROUTE_LANE_EMPTY");
        assert!(!empty.is_retriable());
        assert!(empty.is_structural());
    }
}
