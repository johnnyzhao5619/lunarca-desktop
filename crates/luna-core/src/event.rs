//! 流式事件模型。零 IO。事实源：TDD-01 §2.3。
//!
//! ASR 流式语义对标 Chrome SODA / Google Meet（interim/final + stability）：区分
//! "会被改写的暂定"与"已定稿"，避免前端闪烁，并让实时转译只翻译已定稿段。

use crate::error::JobError;
use crate::result::{JobResult, Segment, Usage};
use serde::{Deserialize, Serialize};

/// Job 生命周期事件流。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobEvent {
    /// 已入队。
    Queued,
    /// **本机队列收下了它，但还没给它额度**——等待已经长到该说出口了（ADR-078 ③）。
    ///
    /// 与 [`JobEvent::Queued`] 是两件事，故是两个变体：那一条是**提交回执**，在
    /// `submit` 里同步发出、恒发一次、没有任何信息量；这一条说的是「等了 `waited_ms`
    /// 还没轮到」，是一个**异常信号**。共用一个变体的话，正常入队与卡住的入队在事件流里
    /// 长得一模一样，于是消费者只能一律忽略——`Queued` 此前恰恰就是这么被忽略的：
    /// `recording.rs` 的事件臂根本没有它的分支，不打日志、不发 UI 事件、无超时，于是
    /// 「实时转写在等一个永远不会来的额度」在界面上与「一切正常」完全无法区分。
    ///
    /// 也与 [`JobEvent::RemoteQueued`] 分开：那条是**服务端**队列里的排位（用户只能等
    /// 或取消），这条是**本机**准入闸没放行（是这台机器自己的资源问题）。
    ///
    /// 只对会话池的 Job（流式 ASR / 实时转译）发——计算池里排队是常态，不是缺陷。
    AdmissionDelayed {
        /// 已经等了多久（毫秒）。脱敏：只有时长，无内容。
        waited_ms: u64,
    },
    /// 已开始执行。
    Started,
    /// **实际执行者**（PRD-17-033 降级执行者可见）。
    ///
    /// 只在「路由到的 Provider 与真正干活的那一方不是同一个」时发出——目前是远程
    /// 转写：客户端选的是「服务端转写」，而具体跑在哪个上游、哪个模型由服务端决定，
    /// 且可能因降级而变。本机 Provider 不发这条：那种情形下 Provider 本身就是执行者，
    /// 再发一遍等于让 UI 去区分两种表达同一件事的方式。
    ///
    /// **静默替换是信任事故**：用户以为在用组织的服务端，实际被降级到了第三方，
    /// 而界面上毫无差别——这条事件存在的全部理由就是不让那种情况发生。
    ExecutedBy(ExecutorInfo),
    /// **服务端队列里的排位**（ADR-049 ⑧）。只有远程转写发它。
    ///
    /// 与 [`JobEvent::Queued`] 是两件事，故是两个变体：那一条说的是「本机队列收下了
    /// 它」（`JobQueue` 在 submit 时发），这一条说的是「服务端把它排在了第 N 位」。
    /// 复用同一个变体的话，本机排队与服务端排队在界面上会变成同一句话，而用户能做的事
    /// 完全不同（前者可以调序、可以移除，后者只能等或取消）。
    ///
    /// 会**反复发**：每次前面有任务出队都刷新一次。消费方按最后一帧渲染即可。
    RemoteQueued(RemoteQueueStatus),
    /// **云腿断了，正在静默重连窗内重试**（ADR-038 ⑧）。只有实时远程转写发它。
    ///
    /// 与 [`JobEvent::Failed`] 是两件事，故不是「先失败再恢复」：失败是终态，
    /// 它一出现，`asr_failover` 就把这条会话整体移交给本机兜底，而移交是**单向**的
    /// （回不去了）。重连窗存在的全部意义就是在那之前给瞬断一个安静恢复的机会，
    /// 所以它必须是一个**非终态**事件——界面据此显示「重连中」，账本一个字不动。
    ///
    /// 成对出现：[`ReconnectPhase::Retrying`]（真断连）或 [`ReconnectPhase::Resyncing`]
    /// （上游正常关流）开窗，[`ReconnectPhase::Resumed`] 表示窗内接上了。窗关了则不发
    /// `Resumed`，直接是 `Failed`。
    Reconnecting(ReconnectPhase),
    /// 增量（区分暂定/已定稿）。
    Partial(PartialPayload),
    /// 进度（0.0..=1.0，批量/长音频）。
    Progress(f32),
    /// 用量增量。
    /// 用量**快照**：到此刻为止对方自报的实测用量（「取，不猜」）。`JobQueue` 只留最后
    /// 一份，失败 / 取消终态记账用它——这是失败任务用量不再恒记 0 的唯一通路
    /// （TDD-14 §3 修订注）。不是增量：增量装不下 `tokens_in`，且累计语义要求每个
    /// Provider 都按同一口径切片，而对方自报的从来都是"到目前为止"的总数。
    Usage(Usage),
    /// 完成（终态）。
    Finished(JobResult),
    /// 失败（终态）。
    Failed(JobError),
    /// 已取消（终态）。
    Cancelled,
}

/// 流式增量载荷。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartialPayload {
    /// ASR：本次新"定稿"段（committed，单调不可改）+ 当前"暂定"尾（hypothesis，可改）。
    Asr {
        /// 新确认、可下传导出/翻译的段。
        committed_delta: Vec<Segment>,
        /// 暂定尾，仅供 UI 灰显，禁止下传。
        hypothesis: String,
        /// 稳定度（0.0 极易变 ..= 1.0 几乎稳定）。
        stability: f32,
    },
    /// LLM：流式 token 增量。
    Token(String),
}

#[cfg(test)]
mod tests {

    /// **优先级 → 服务端档位的映射只有一份**（ADR-049 ⑥）。
    ///
    /// 判据是**用户在不在等**：弹窗翻译、语音输入、单文件转写、停录后的权威重转写都
    /// 有人盯着结果，故都买 `standard` 这条快车道；只有整批导入是「入队时人就走开了」。
    /// 把 `Normal` 划进 `batch` 会让手动点的二次转写排在一整批导入后面——用户点了一下、
    /// 然后等两小时，而他完全不知道自己在等谁。
    #[test]
    fn only_background_work_buys_the_longer_window() {
        use crate::job::Priority;
        assert_eq!(
            QueueTier::from_priority(Priority::Interactive),
            QueueTier::Standard
        );
        assert_eq!(
            QueueTier::from_priority(Priority::Normal),
            QueueTier::Standard,
            "单文件转写与二次转写都有人在等，不该被排到整批导入后面"
        );
        assert_eq!(
            QueueTier::from_priority(Priority::Background),
            QueueTier::Batch
        );
    }

    /// 线上取值要能原样往返；读服务端回传时的兜底方向如实记在这里。
    #[test]
    fn tier_wire_values_round_trip() {
        for tier in [QueueTier::Standard, QueueTier::Batch] {
            assert_eq!(QueueTier::parse(tier.as_str()), tier);
        }
        assert_eq!(
            QueueTier::parse("urgent"),
            QueueTier::Standard,
            "读回传时不认识的档位归 standard——它只影响一句解释性文案，\
             而请求侧的 tier 仍然是必填、无默认"
        );
    }

    /// 排位帧的线上形状：位次恒在，等待区间可缺。
    #[test]
    fn a_queue_status_without_samples_carries_no_time_at_all() {
        let json = serde_json::to_value(JobEvent::RemoteQueued(RemoteQueueStatus {
            position: 3,
            tier: QueueTier::Batch,
            eta: None,
        }))
        .unwrap();
        let payload = &json["remote_queued"];
        assert_eq!(payload["position"], 3);
        assert_eq!(payload["tier"], "batch");
        assert!(
            payload["eta"].is_null(),
            "服务端没有样本时不得编一个等待时间：{json}"
        );
    }

    /// 服务端报了我们不认识的执行者性质时，**绝不能当成本地**。
    ///
    /// 当成本地 = 界面告诉用户「数据没出你的组织」，而我们其实并不知道——一次实打实的
    /// 隐私误述。新版服务端加一种上游性质是完全正常的演进，这条保证老客户端在那种
    /// 情况下说「不知道」而不是说错。
    #[test]
    fn unknown_executor_kind_never_reads_as_local() {
        assert_eq!(
            ExecutorKind::parse("local_runtime"),
            ExecutorKind::LocalRuntime
        );
        assert_eq!(ExecutorKind::parse("third_party"), ExecutorKind::ThirdParty);
        for weird in ["partner_cloud", "", "LOCAL_RUNTIME", "local", "third-party"] {
            assert_eq!(
                ExecutorKind::parse(weird),
                ExecutorKind::Unknown,
                "`{weird}` 被解析成了已知性质"
            );
            assert!(
                ExecutorKind::parse(weird).leaves_deployment(),
                "未知性质必须按「数据可能已离开部署方边界」处理——不知道时往保守的一侧说"
            );
        }
        assert!(!ExecutorKind::LocalRuntime.leaves_deployment());
        assert!(ExecutorKind::ThirdParty.leaves_deployment());
    }

    /// 执行者载荷不得含地址或凭据线索（那是运营信息，不该发给每个终端用户）。
    /// 「离开了这台设备吗」与「在服务端那侧转给第三方了吗」是**两个问题**。
    ///
    /// 混用过一次，真机才打出来：服务端把它自己的 WhisperLive 如实报成 `local_runtime`
    /// （对**它**而言确实是自有运行时），而客户端拿 `kind.leaves_deployment()` 当联网
    /// 指示的判据 → 收到执行者回报的那一刻指示熄灭，**而音频正在持续上传**。
    /// 一次不会报错、只能靠抓包发现的隐私误述。
    ///
    /// 这条钉住：`local_runtime` + `leaves_device: true` 是一个**完全合法**的组合，
    /// 两位不得互相推导。
    #[test]
    fn leaving_the_device_is_not_the_same_question_as_leaving_the_deployment() {
        let server_side_local = ExecutorInfo {
            display_name: "本机 WhisperLive".into(),
            // 服务端说：在我这边跑的，没转给第三方。
            kind: ExecutorKind::LocalRuntime,
            model: "tiny".into(),
            // 客户端说：可音频确实已经离开我的设备了。
            leaves_device: true,
        };
        assert!(
            !server_side_local.kind.leaves_deployment(),
            "服务端的自有运行时不算「出了部署方」"
        );
        assert!(
            server_side_local.leaves_device,
            "但它**确实**离开了这台设备——联网指示必须据此点亮"
        );

        // 反过来：降级切回本机，两位都为假。
        let on_device = ExecutorInfo {
            display_name: "本地 Whisper".into(),
            kind: ExecutorKind::LocalRuntime,
            model: "ggml-base".into(),
            leaves_device: false,
        };
        assert!(!on_device.leaves_device);
    }

    #[test]
    fn executor_info_wire_carries_no_address() {
        let json = serde_json::to_string(&JobEvent::ExecutedBy(ExecutorInfo {
            leaves_device: true,
            display_name: "本机 whisper.cpp".into(),
            kind: ExecutorKind::LocalRuntime,
            model: "ggml-base".into(),
        }))
        .unwrap();
        assert!(
            json.contains("executed_by"),
            "wire 标签应为 snake_case：{json}"
        );
        assert!(
            json.contains("local_runtime"),
            "性质应以 snake_case 上线：{json}"
        );
        for leak in ["base_url", "http", "credential", "api_key", "token"] {
            assert!(!json.contains(leak), "执行者事件泄露了 `{leak}`：{json}");
        }
    }
    use super::*;

    #[test]
    fn job_event_round_trip() {
        let ev = JobEvent::Partial(PartialPayload::Token("你".into()));
        let json = serde_json::to_string(&ev).unwrap();
        let back: JobEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn cancelled_is_unit_tagged() {
        assert_eq!(
            serde_json::to_string(&JobEvent::Cancelled).unwrap(),
            "\"cancelled\""
        );
    }
}

/// 实际执行者的身份（[`JobEvent::ExecutedBy`] 的载荷）。
///
/// **不含地址与凭据线索**：用户要回答的是「这是本机、我的组织服务端，还是某个
/// 第三方」，而不是「你们用了哪家的哪个 URL」。后者是运营信息，发给每个终端用户
/// 就等于公开供应商清单。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutorInfo {
    /// 展示名（如「本机 whisper.cpp」）。由服务端给出，客户端原样展示。
    pub display_name: String,
    /// 执行者性质——**在服务端那一侧**是自有运行时还是第三方。决定 UI 要不要提示
    /// 「数据进一步到了第三方」。
    pub kind: ExecutorKind,
    /// 上游侧模型标识。
    pub model: String,
    /// 音频/文本有没有**离开这台设备**。
    ///
    /// 与 [`Self::kind`] 是两个不同的问题，混用过一次、真机才打出来：服务端把它自己的
    /// WhisperLive 如实报成 `local_runtime`（对**它**而言确实是自有运行时），而客户端
    /// 拿 [`ExecutorKind::leaves_deployment`] 当联网指示的判据 → 收到执行者回报的那一刻
    /// 指示熄灭，**而音频正在持续上传**。一次不会报错的隐私误述。
    ///
    /// 「离开了这台设备吗」**只有客户端答得上来**（它知道自己把字节发去了哪），故这一位
    /// 由客户端侧填：远程 Provider 转发服务端回报时恒 `true`（音频确实已经出去了），
    /// 降级组合合成本机腿时按该腿的 `location` 填。服务端永远不参与这一位的判断。
    pub leaves_device: bool,
}

/// 静默重连窗的相位（[`JobEvent::Reconnecting`] 的载荷，ADR-038 ⑧）。
///
/// **不带「第几次重试」**：界面上「重连中」与「重连中（第 3 次）」对用户是同一句话
/// ——他能做的事完全一样（等，或者停录）。重试次数属于日志，而把它放进事件就等于
/// 让界面去渲染一个每几百毫秒变一次的数字。
///
/// **`Retrying` 与 `Resyncing` 是两回事，且只有前者该弹「网络不稳」横幅**（ADR-065）：
/// 断线开窗有两种起因——真的传输断连（Wi-Fi 掉、拔网线：没有 close 握手的 `Err`/`None`），
/// 和上游把这条流**正常关流**（服务端在一条活着的连接上发来带正常 code 的 Close 帧，
/// WhisperLive 每段解码收尾即如此）。后者要继续转写同样得再连一条，但它**不是网络故障**，
/// 把它也报成「网络不稳」会让横幅在链路完全正常时几乎常驻（真机上 `Retrying↔Resumed`
/// 每 ~2 秒翻转，而字幕照出）。故拆出 `Resyncing`：同样开窗重连，但界面**不打扰用户**。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconnectPhase {
    /// **真断连**，窗内正在重试——界面弹「网络不稳」。
    Retrying,
    /// **上游正常关流**后的静默重连（正常 close code）：窗内重试，但**不是网络故障**，
    /// 界面不打扰用户（无「网络不稳」横幅）。
    Resyncing,
    /// 窗内接上了，云腿继续——界面据此撤掉任何「重连中」提示。
    Resumed,
}

/// 服务端队列里的排位（[`JobEvent::RemoteQueued`] 的载荷，ADR-049 ⑧）。
///
/// **位次恒在，等待区间可缺**：位次是服务端确知的事实，等待时间不是。刚部署完、
/// 一件都还没跑过的服务端给不出任何样本——那时给「排队第 3 位」而不给时间，比按某个
/// 想当然的实时倍率算一个数诚实得多（同 `JobEvent` 里「不编解码百分比」的那条）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteQueueStatus {
    /// 从 1 起算（「排队第 1 位」= 下一个被领取的那件）。
    pub position: u32,
    /// 调度档位。用户据此理解「为什么我排在后面」。
    pub tier: QueueTier,
    /// 等待区间（秒）。`None` = 服务端尚无样本，只报位次。
    pub eta: Option<EtaRange>,
}

/// 服务端等待预估的**区间**（秒）。
///
/// **区间而不是点值**：一个「预计 7 分钟」会被读成承诺，而它的真实含义是
/// 「4 到 12 分钟之间，取决于队列前面那几段录音有多难解」。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EtaRange {
    /// 区间下界（秒）。
    pub min_secs: u32,
    /// 区间上界（秒）。
    pub max_secs: u32,
}

/// 服务端的调度档位（ADR-049 ⑥）。
///
/// 由 [`crate::job::Priority`] 映射而来：交互式与普通档都买「用户在等」这条快车道，
/// 后台档换更长的等待。**映射只有一份**（[`QueueTier::from_priority`]），
/// 各处各写一遍必然出现「面板显示 batch、实际发的是 standard」。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueTier {
    /// 用户在等结果：停录后的权威重转写、手动二次转写、语音输入。
    Standard,
    /// 用户走开了：整批导入的媒体转写。
    Batch,
}

impl QueueTier {
    /// 线上取值（与服务端的 `tier` 字段逐字对应）。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Batch => "batch",
        }
    }

    /// 解析服务端回传的取值；不认识的一律归 [`QueueTier::Standard`]。
    ///
    /// 这里的兜底方向与 `ExecutorKind::Unknown` 刻意相反，因为**代价不对称**：
    /// 这个值只用来渲染一句「后台任务，可能要等更久」，猜错的后果是一句解释不够贴切；
    /// 而它没有「不知道」这一档可显示。请求侧的 `tier` 仍然是必填、无默认——
    /// 兜底只发生在**读服务端回传**这一侧。
    #[must_use]
    pub fn parse(raw: &str) -> Self {
        match raw {
            "batch" => Self::Batch,
            _ => Self::Standard,
        }
    }

    /// 本机优先级 → 服务端档位。**唯一的映射点**。
    #[must_use]
    pub fn from_priority(priority: crate::job::Priority) -> Self {
        match priority {
            // 用户正看着结果出来：弹窗翻译、语音输入、单文件转写、停录后的重转写。
            crate::job::Priority::Interactive | crate::job::Priority::Normal => Self::Standard,
            // 整批导入：入队时用户就已经走开了，换更长的窗口给别人让路。
            crate::job::Priority::Background => Self::Batch,
        }
    }
}

/// 执行者性质。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutorKind {
    /// 部署方自己跑的运行时（本机随包引擎 / 组织服务端上的 whisper…）。
    LocalRuntime,
    /// 第三方 API。UI 据此给出更强的数据去向提示。
    ThirdParty,
    /// 服务端报了我们不认识的性质。
    ///
    /// **必须有这一档，且不能默认按 `LocalRuntime` 处理**：新版服务端加一种性质
    /// 时，把未知值当成"本地"就是一次实打实的隐私误述——界面会告诉用户数据没出组织，
    /// 而实际上我们并不知道。不知道就说不知道。
    Unknown,
}

impl ExecutorKind {
    /// 从服务端的 wire 值解析；不认识的一律归 [`ExecutorKind::Unknown`]。
    #[must_use]
    pub fn parse(raw: &str) -> Self {
        match raw {
            "local_runtime" => Self::LocalRuntime,
            "third_party" => Self::ThirdParty,
            _ => Self::Unknown,
        }
    }

    /// 数据是否离开了部署方自己的边界（UI 据此决定提示强度）。
    ///
    /// `Unknown` 归 `true`：不知道时按更保守的一侧说话。
    #[must_use]
    pub fn leaves_deployment(self) -> bool {
        !matches!(self, Self::LocalRuntime)
    }
}
