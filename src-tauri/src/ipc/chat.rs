//! AI 对话命令域（PRD-03，从 `ipc/mod.rs` 纵切）。
//!
//! 事实源：会话语义 [`luna_jobqueue::ChatSession`]（TDD-01 §6.5）、产品需求 PRD-03。
//! 本模块只做 Tauri 命令边界（收发/校验/装配）+ 相关 DTO；上下文组装/截断/落账等业务
//! 在 `luna-jobqueue`（ADR-003）。
//!
//! 会话生命周期（PRD-03-012/013）：**按表面（scope）键控的 App 运行期内存会话**——工作区
//! 标签页与快捷面板各持一个，视图卸载/窗口隐藏不丢会话。持久化只经 Vault `type: chat`
//! 笔记（笔记即文件）。
//!
//! **持久化心智（单一模型，无矛盾）**：每个会话有 `persist` 标志——
//! - 持久会话（`persist=true`，默认取向来自设置 `chat.save_history`）：每轮终局自动写回
//!   Vault、进历史列表；
//! - **临时对话**（`persist=false`，PRD-03-012，对话页「临时对话」开关按会话覆盖）：全程
//!   不落盘、不进历史、App 退出即弃；仍可经「保存此对话」显式转持久。
//!   二者都无"意外丢失"（持久恒已存、临时按定义即弃），故不再有独立的"关闭未存提示"开关。
//!
//! 锁纪律：`ChatSessionStore`（std Mutex）只做短临界区槽位管理，绝不跨 `.await`；
//! 会话本体在 `Arc<tokio::Mutex<ChatSession>>`，生成期间不持锁（`drain_with` 全程无锁，
//! 其它视图仍可 attach 读历史），仅提交与落账两个瞬时点加锁。

use super::*;
use tokio::sync::Mutex as TokioMutex;

/// 对话表面（会话作用域）。每个作用域一个独立内存会话（PRD-03-012）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ChatScope {
    /// 主窗口「对话」标签页。
    Workspace,
    /// 全局快捷键唤起的非激活面板（`quick-chat` 窗口）。
    Panel,
}

/// 读取"新对话默认是否保存到历史"（PRD-03-013：设置 `chat.save_history`，默认开）。
/// = 新建/挂载会话时 `persist` 的默认取向；单次由「临时对话」开关覆盖。
fn save_history_default(state: &tauri::State<'_, AppState>) -> bool {
    state
        .settings_snapshot()
        .get_bool("chat.save_history")
        .unwrap_or(true)
}

/// 新建会话槽的默认取向快照（两项都只在**首次创建**时生效）。
fn slot_defaults(state: &tauri::State<'_, AppState>) -> SlotDefaults {
    SlotDefaults {
        persist: save_history_default(state),
        grounding: grounding_default(state),
        token_budget: chat_token_budget(state),
    }
}

/// 一条对话消息投影（`System` 角色不外发——系统提示词属预设实现细节）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub(crate) struct ChatMessageDto {
    /// 角色（`user` / `assistant`）。
    pub role: String,
    /// 文本内容。
    pub content: String,
    /// 本条回答实际引用的出处（PRD-16-005）。只含**被引用**的那几条——
    /// 把检索到的全摆出来会让用户以为每条都被采信了。
    pub sources: Vec<RetrievalSourceDto>,
    /// 本轮开启知识检索后才有；普通对话保持 `None`，不得被误读为 ModelOnly。
    pub answer_meta: Option<AnswerMetaDto>,
}

/// 会话快照：视图挂载时经 `chat_attach` 恢复（PRD-03-012 App 运行期连续性）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub(crate) struct ChatSnapshotDto {
    /// 作用域。
    pub scope: ChatScope,
    /// 已落账消息（不含进行中一轮的流式部分）。
    pub messages: Vec<ChatMessageDto>,
    /// 是否有进行中的生成（重挂载时提示"正在生成"，终局经 `chat://changed` 通知刷新）。
    pub busy: bool,
    /// **临时对话**（PRD-03-012）：本会话不落盘/不进历史/退出即弃。`true` = 临时。
    pub temporary: bool,
    /// 当前角色预设 id（`None` = 无系统提示词）。
    pub preset_id: Option<String>,
    /// 已链接的归档笔记相对路径（PRD-03-013：历史会话列表高亮当前会话的依据）。
    pub note_path: Option<String>,
    /// 本会话是否检索知识库（PRD-16-005 输入区芯片的真值）。
    pub grounding: bool,
}

/// 历史会话列表条目（PRD-03-013：Vault `type: chat` 笔记即会话，笔记即文件铁律）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub(crate) struct ChatHistoryEntryDto {
    /// 笔记相对路径（载入/删除/跳转笔记库的键）。
    pub path: String,
    /// 标题（= 文件名 stem，归档命名含时间与首问摘要）。
    pub title: String,
    /// 修改时间（Unix 毫秒；索引缺省 0）。
    pub modified_ms: f64,
}

/// 历史会话的一页（PRD-03-013）。
///
/// 游标挂在**信封**上而不是逐行：这条命令的返回本来就要带一个权威总数，套信封没有额外
/// 代价（与 `DraftInboxDto` 同一形态；裸数组那几个消费者才把游标挂末行）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub(crate) struct ChatHistoryDto {
    /// 全库会话总数（侧栏计数徽标用）。
    ///
    /// **不受列表上限截断**：徽标承诺的是「一共有几场」，拿已加载条数去充数，
    /// 在满页时就是谎报一个上限值。
    pub total: u32,
    /// **这一页**的条目（修改时间倒序）。
    pub entries: Vec<ChatHistoryEntryDto>,
    /// 下一页的键集游标；`null` = 已遍历到底（TDD-07 §4.1a）。游标不透明，前端只回传。
    pub next_cursor: Option<String>,
    /// 会话能不能**生成知识笔记**（PRD-06-044）。
    ///
    /// 挂在信封上而不是逐行：这一项的资格由**类型**决定——`NoteType::Chat` 恒为资料
    /// （`NoteStage::resolve` 在 `is_source_material()` 上直接返回，frontmatter 改不动它），
    /// 于是逐行下发就是一列恒为 true 的布尔，把常量伪装成数据。前端仍然不自己拼判据。
    pub distillable: bool,
    /// 会话能不能作为「多条 → 一篇」的源（PRD-06-047）。
    ///
    /// 同上是类型级：`merge_block` 只挡聚合页，而主题页 / 知识地图都是 `type: note`。
    /// 有人手工往一篇会话里写 `luna.topic: true` 时菜单会多给一项，那一次由命令层如实
    /// 拒绝并回流原因——「前端不渲染只是不渲染，命令仍独立判定」是本仓既有口径。
    pub mergeable: bool,
    /// 一次汇总最少 / 最多几条（PRD-06-047）——与 `DraftInboxDto` 的那两个是**同一对
    /// 常量**。
    ///
    /// **常量归 Rust**：前端照着写一个 `> 1` 在这里，就是同一条规则的第二处定义，
    /// 改了一边不会有任何东西变红，而表现只是"菜单说可以、点下去被拒"。
    pub merge_min: u32,
    pub merge_max: u32,
}

/// 对话角色预设投影（PRD-03-010）。`name` 为回退展示名，前端优先按 id 走 i18n。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub(crate) struct ChatPresetDto {
    /// 预设 id（如 `chat_general`）。
    pub id: String,
    /// 回退展示名。
    pub name: String,
}

/// 结构化失败载荷（PRD-03 §5：结构化错误 + 修复建议；前端按 `code` 映射 i18n 文案与动作）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub(crate) struct ChatFailureDto {
    /// 稳定错误码（SCREAMING_SNAKE，`luna_core::JobError::code`）。
    pub code: JobErrorCodeDto,
    /// 是否可原样重试（TDD-01 §5.6）。
    pub retriable: bool,
    /// 服务端要求的最短等待秒数。只对 `RATE_LIMITED` 有值；`None` = 对方未给提示。
    pub retry_after_secs: Option<u32>,
}

impl From<&luna_core::JobError> for ChatFailureDto {
    fn from(err: &luna_core::JobError) -> Self {
        Self {
            code: err.into(),
            retriable: err.is_retriable(),
            retry_after_secs: match err {
                luna_core::JobError::RateLimited {
                    retry_after_secs, ..
                } => retry_after_secs.map(|seconds| u32::try_from(seconds).unwrap_or(u32::MAX)),
                _ => None,
            },
        }
    }
}

/// Chat 流式命令事件。仅下行（Rust → 前端），故不派生 `Deserialize`。
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "snake_case", tag = "kind", content = "data")]
pub(crate) enum ChatEvent {
    /// 已开始。
    Started,
    /// 上下文超预算被截断（PRD-03 §5 显式提示，不静默）。
    Truncated {
        /// 实际提交 token 数。
        tokens_in: u32,
        /// 输入预算上限。
        input_limit: u32,
    },
    /// 本轮的库内检索结果（PRD-16-006）。**在流式开始前**下发，故引用角标不依赖
    /// 生成结束即可点击；`sources` 为空即「未找到相关内容」的诚实降级信号。
    Retrieval(RetrievalInfoDto),
    /// 流式 token 增量（PRD-03-002 [P0] 流式输出）。
    Token(String),
    /// 完成（最终 assistant 全文 + 被实际引用的出处）。
    Done {
        /// 最终回答全文。
        text: String,
        /// 回答里 `[n]` 真正引用到的出处（解析在 Rust，前端只渲染）。
        sources: Vec<RetrievalSourceDto>,
        answer_meta: Option<AnswerMetaDto>,
    },
    /// 失败（结构化，PRD-03 §5）。
    Failed(ChatFailureDto),
    /// 已取消（保留已产 token，PRD-03-007）。
    Cancelled(String),
}

/// 一轮生成终局广播（低频 typed event）：重挂载/其它视图收到后重新 `chat_attach`；
/// 本视图据 `note_path` 同步归档链接（持久会话本轮自动归档可能建立/更新链接，PRD-03-013）。
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[tauri_specta(event_name = "chat://changed")]
pub struct ChatTurnEnded {
    /// 终局的作用域。
    pub scope: ChatScope,
    /// 终局后的归档笔记链接（临时会话恒 `None`）。
    pub note_path: Option<String>,
}

/// 选区带入就绪广播（PRD-03-008）：**不携带正文**（正文经 `chat_prefill_take` 单次拉取，
/// 不进事件系统，TDD-12 日志/事件脱敏纪律）。
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[tauri_specta(event_name = "chat://prefill")]
pub struct ChatPrefillReady {
    /// 目标作用域（当前恒为 `panel`）。
    pub scope: ChatScope,
}

/// 新建会话槽时的默认取向（两个默认值各有来源设置，聚成一体避免调用点长参数列）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct SlotDefaults {
    /// 是否保存到历史（设置 `chat.save_history`）。
    persist: bool,
    /// 是否检索知识库（设置 `chat.grounding`）。
    grounding: bool,
    /// 本次新建会话的上下文预算（自适应模型能力，见 [`chat_token_budget`]）。
    /// 放这里而不是让 `slot_mut` 去拿 `AppState`：槽的默认取向本就是"创建那一刻的快照"，
    /// 窗大小与另外两项同性质。
    token_budget: luna_provider::TokenBudget,
}

/// 一个作用域的会话槽。
struct ChatSlot {
    session: Arc<TokioMutex<ChatSession>>,
    preset_id: Option<String>,
    /// 已完成的问答轮次。仅用于蒸馏的"值不值得整理"门槛（PRD-03-014）——
    /// 一句话的会话没有可蒸的东西，为它生成一篇笔记只是给知识库添堵。
    turns: u32,
    /// 本会话是否保存到历史（PRD-03-012/013）：`false` = 临时对话（不落盘/不进历史）。
    persist: bool,
    busy: bool,
    cancel: Option<CancelToken>,
    /// 代际号：`chat_reset`/`chat_open_note` 顶替会话后，旧 turn 的收尾不得改写新槽状态。
    generation: u64,
    /// 已链接的归档笔记（PRD-03-013）：载入历史/首次归档后建立；新对话解除。
    note_path: Option<String>,
    /// 本会话是否检索知识库（PRD-16-005）。默认取设置 `chat.grounding`，
    /// 由输入区芯片按会话覆盖——**设置只是默认档，不是主控件**（PRD-16 D1）。
    grounding: bool,
    /// 本轮检索的一次性覆盖（PRD-16-006/008）：透明行里「剔除某篇」「临时纳入对话笔记」。
    /// **发新消息即复位**——它们是对"这一次检索"的纠正，不是会话级偏好。
    retrieval_overrides: RetrievalOverrides,
    /// 各条 assistant 消息实际引用的出处，键 = 该消息在 `messages` 里的下标。
    ///
    /// 结构化存在内存、**渲染时才拼进 Markdown**（`render_chat_markdown`）——正文里存一份、
    /// 结构里再存一份会在重复保存时叠加出两行出处。
    sources: HashMap<usize, Vec<RetrievalSourceDto>>,
    /// 与 assistant 消息下标同键的终局依据判定；Markdown 归档仍是唯一可迁移事实源。
    answer_meta: HashMap<usize, AnswerMetaDto>,
}

/// 一次检索的一次性覆盖项（PRD-16-006/008/009）。
#[derive(Debug, Clone, Default)]
pub(crate) struct RetrievalOverrides {
    /// 临时纳入对话笔记。
    include_chat: bool,
    /// 本次**整组排除记录态**（PRD-16-041）。与 `include_chat` 同为透明行上的口径翻转、
    /// 方向相反：一次查询有效、不改设置，随本轮收尾一并复位。
    exclude_drafts: bool,
    /// 本次剔除的笔记路径。
    excluded: Vec<String>,
    /// 本条消息 `@` 提及框定的范围（PRD-16-009）。
    ///
    /// 与前两项同住"一次性覆盖"里而不是会话级状态：`@` 的语义就是**这一条消息**看哪儿
    /// ——它在成功走完的那一轮末尾随其余覆盖项一起复位，下一个问题重新按默认口径检索。
    scope: Vec<luna_vault::RetrieveScope>,
}

impl RetrievalOverrides {
    pub(crate) fn include_chat(&self) -> bool {
        self.include_chat
    }

    pub(crate) fn exclude_drafts(&self) -> bool {
        self.exclude_drafts
    }

    pub(crate) fn excluded(&self) -> &[String] {
        &self.excluded
    }

    pub(crate) fn scope(&self) -> &[luna_vault::RetrieveScope] {
        &self.scope
    }
}

impl ChatSlot {
    fn new(
        preset_id: Option<String>,
        session: ChatSession,
        persist: bool,
        grounding: bool,
    ) -> Self {
        Self {
            session: Arc::new(TokioMutex::new(session)),
            preset_id,
            persist,
            busy: false,
            cancel: None,
            generation: 0,
            note_path: None,
            turns: 0,
            grounding,
            retrieval_overrides: RetrievalOverrides::default(),
            sources: HashMap::new(),
            answer_meta: HashMap::new(),
        }
    }
}

/// 值得整理成知识笔记的最少问答轮次（PRD-03-014）。
const DISTILL_MIN_TURNS: u32 = 2;

/// 内存态会话存储（退出即丢弃，不持久化，PRD-03-012）。
#[derive(Default)]
pub(crate) struct ChatSessionStore {
    slots: HashMap<ChatScope, ChatSlot>,
    /// 待带入面板的选区文本（PRD-03-008）：单槽，`chat_prefill_take` 单次消费。
    prefill: Option<String>,
}

impl ChatSessionStore {
    /// 取（或按默认取向创建）某作用域的会话槽。`defaults` 仅在**首次创建**时生效
    /// （既有槽保留其已选持久性/临时态与知识库开关，不被后续调用覆盖）。
    fn slot_mut(&mut self, scope: ChatScope, defaults: SlotDefaults) -> &mut ChatSlot {
        self.slots.entry(scope).or_insert_with(|| {
            ChatSlot::new(
                None,
                new_chat_session(defaults.token_budget, None),
                defaults.persist,
                defaults.grounding,
            )
        })
    }
}

impl AppState {
    /// 供原生编排（全局快捷键选区捕获，`lib.rs`）投递选区文本：写入单槽并广播就绪事件。
    /// 空白文本静默忽略。
    pub fn offer_chat_prefill(&self, app: &tauri::AppHandle, text: String) {
        if text.trim().is_empty() {
            return;
        }
        if let Ok(mut store) = self.chat.lock() {
            store.prefill = Some(text);
        } else {
            return;
        }
        let _ = ChatPrefillReady {
            scope: ChatScope::Panel,
        }
        .emit(app);
    }
}

/// 探测并记下当前对话路径的上下文窗（启动、换模型、换算力落点时各跑一次）。
///
/// **问跑这条路径的那一方要真相**，取不到就维持当前值（诚实降级）。
///
/// # 问谁，取决于对话此刻的执行路径
///
/// - **本机**（随包推理引擎）→ 取这次加载给默认对话模型的 `ctx-size`（内存预检定、可能
///   退让减小，档位上限已在编排时封过）。超出它的请求会被引擎如实拒绝，故这是唯一真相。
/// - **原生服务端（托管路由，ADR-048）**→ 客户端在发出请求前不知道服务端会把这一条
///   路由给哪个执行者，「这个模型的窗」这个问题不存在；预算取服务端目录中全部 chat
///   能力模型的**最小窗**（[`server_fleet_context_window`]）——对任何可能的执行者都
///   成立才叫预算。设备档上限不适用：KV cache 在服务端那台机器上。
pub(crate) async fn refresh_chat_context_window(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    let settings = state.settings_snapshot();
    let availability = crate::cloud::evaluate_for(app);
    let window =
        if chat_compute_location(&settings, availability, &state.cloud_server()).is_server() {
            server_fleet_context_window(app).await
        } else if let Some(super::LocalLlm(id)) =
            super::local_llm_for_slot(&settings, &state.engine_models_snapshot(), "default_llm")
        {
            // 随包引擎：窗就是这次加载给它的上下文（preset `ctx-size`，内存预检定下、可能被退让
            // 减小）。超出它的请求会被如实拒绝，故这是唯一真相，不再按设备档另封一次顶。
            // 没进 preset（因内存被拒）就维持当前值——请求本身会报「未装进引擎」。
            state
                .local_engine_plan()
                .lock()
                .await
                .models
                .iter()
                .find(|planned| planned.id == id)
                .map(|planned| planned.preset.ctx)
        } else {
            // 本机档却没有引擎里的对话模型：没有窗可问，维持当前值（任务本身会如实报缺模型）。
            None
        };
    let Some(window) = window else {
        return;
    };
    // 脱敏诊断：只有数字，无模型名、无正文。真机排查「面板为什么没截断提示」靠这一行。
    eprintln!("[chat] 上下文窗已探测：{window} token");
    state.set_chat_ctx_window(window);
}

/// 交互式 Job 提交前确保上下文窗已探测过（`0` = 还没有）。
///
/// 探测只在启动 / 换模型 / 换落点 / 引擎编排后跑，而这些时机都可能在引擎编排完成之前就
/// 过去了（真机：切到本机模型后面板仍按「未探测」放到封顶）。补问一次只是读内存里的编排
/// 结论，在第一条交互式 Job 之前补一次是最便宜的自愈。
pub(crate) async fn ensure_chat_context_window(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    if state.chat_ctx_window() == 0 {
        refresh_chat_context_window(app).await;
    }
}

/// 服务端托管路由下的保守上下文预算：目录中全部 chat 能力模型的**最小窗**。
///
/// 客户端不选执行者（ADR-048），预算必须对服务端可能路由到的任何候选都成立——取最小值。
/// 任何一个候选没报窗口就返回 `None`（不猜：按一个可能超限的数裁剪历史，比维持默认窗
/// 更糟）。`None` 同时覆盖取不到目录——未登录 / 网络失败；`service_models` 是
/// `RequiresSession`，无会话时本地短路不出网，零网络不变量（ADR-037）不因本函数破口。
async fn server_fleet_context_window(app: &tauri::AppHandle) -> Option<u32> {
    let cloud = app.try_state::<luna_cloud::commands::CloudState>()?;
    let models = cloud.0.service_models().await.ok()?;
    // 能力串取自槽位表（`MODEL_ASSIGNMENT_SLOTS` 的 default_llm 行），不在这里再写一遍。
    let (_, _, chat_capability) = assignment_meta("default_llm")?;
    models
        .iter()
        .filter(|entry| entry.capabilities.iter().any(|c| c == chat_capability))
        .map(|entry| entry.context_window)
        .collect::<Option<Vec<u32>>>()?
        .into_iter()
        .min()
}

/// 本次会话的上下文预算。
///
/// **它与引擎为该模型加载的窗必须是同一个数**（[`refresh_chat_context_window`] 取的正是
/// preset 的 `ctx-size`）：两侧分家 = 一边按 A 裁历史、引擎按 B 拒收，用户看到的是
/// "我明明没超"却仍失败。
fn chat_token_budget(state: &AppState) -> luna_provider::TokenBudget {
    let window = state.chat_ctx_window();
    let default = luna_provider::TokenBudget::default();
    if window == 0 {
        return default;
    }
    luna_provider::TokenBudget {
        context_window: window,
        ..default
    }
}

/// 由角色预设构造内存会话（`None` = 无系统提示词）。
fn new_chat_session(
    token_budget: luna_provider::TokenBudget,
    preset: Option<&luna_core::PromptPreset>,
) -> ChatSession {
    ChatSession::new(
        ChatSessionConfig {
            system_prompt: preset.and_then(|p| p.system_prompt.clone()),
            token_budget,
            ..ChatSessionConfig::default()
        },
        Arc::new(HeuristicTokenCounter::new()),
    )
}

fn chat_lock_err() -> String {
    "Chat 锁中毒".to_string()
}

/// 会话历史投影（跳过 System——系统提示词属预设实现细节，不进消息列表）。
///
/// `sources` 按**会话内下标**取，故投影后的位置与 `sources` 的归属恒一致；
/// 前端拿到的是「这条回答引用了哪几篇」，不必自己去数第几条。
fn project_messages(
    messages: &[ChatMessage],
    sources: &HashMap<usize, Vec<RetrievalSourceDto>>,
    answer_meta: &HashMap<usize, AnswerMetaDto>,
) -> Vec<ChatMessageDto> {
    messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m.role != ChatRole::System)
        .map(|(idx, m)| ChatMessageDto {
            role: match m.role {
                ChatRole::User => "user".to_string(),
                _ => "assistant".to_string(),
            },
            content: m.content.clone(),
            sources: sources.get(&idx).cloned().unwrap_or_default(),
            answer_meta: answer_meta.get(&idx).cloned(),
        })
        .collect()
}

/// 挂载/恢复某作用域的会话（get-or-create，PRD-03-012 App 运行期连续性）。
#[tauri::command]
#[specta::specta]
pub(crate) async fn chat_attach(
    state: tauri::State<'_, AppState>,
    scope: ChatScope,
) -> Result<ChatSnapshotDto, String> {
    let defaults = slot_defaults(&state);
    let (session, busy, persist, preset_id, note_path, grounding, sources, answer_meta) = {
        let mut store = state.chat.lock().map_err(|_| chat_lock_err())?;
        let slot = store.slot_mut(scope, defaults);
        (
            Arc::clone(&slot.session),
            slot.busy,
            slot.persist,
            slot.preset_id.clone(),
            slot.note_path.clone(),
            slot.grounding,
            slot.sources.clone(),
            slot.answer_meta.clone(),
        )
    };
    let messages = {
        let session = session.lock().await;
        project_messages(session.messages(), &sources, &answer_meta)
    };
    Ok(ChatSnapshotDto {
        scope,
        messages,
        busy,
        temporary: !persist,
        preset_id,
        note_path,
        grounding,
    })
}

/// 新对话：以（可选）角色预设顶替该作用域的会话；进行中的生成先取消（PRD-03-010/012）。
///
/// `temporary` 决定新会话持久性（PRD-03-012「临时对话」）：`None` = 跟随设置默认
/// `chat.save_history`；`Some(true)` = 临时（不落盘/不进历史）；`Some(false)` = 保存。
/// 旧持久会话在替换前已逐轮归档进历史，故切换不丢内容。
#[tauri::command]
#[specta::specta]
pub(crate) fn chat_reset(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    scope: ChatScope,
    preset_id: Option<String>,
    temporary: Option<bool>,
) -> Result<ChatSnapshotDto, String> {
    let preset = match preset_id.as_deref() {
        Some(id) => Some(
            luna_core::builtin_chat_presets(luna_core::prompt::registry())
                .into_iter()
                .find(|p| p.id.as_str() == id)
                .ok_or_else(|| format!("未知对话角色预设：{id}"))?,
        ),
        None => None,
    };
    let persist = match temporary {
        Some(t) => !t,
        None => save_history_default(&state),
    };
    let grounding = grounding_default(&state);
    let mut store = state.chat.lock().map_err(|_| chat_lock_err())?;
    // 会话终局 = 开新会话顶替旧的（PRD-03-014）：这是对话唯一有意义的蒸馏触发点——
    // 持久会话**每轮**都归档写盘，逐轮蒸馏既烧算力又整理了一段还没有结论的对话。
    // 只对已落盘（有笔记）且有实质来回的会话触发；临时对话没有源笔记，天然不适用。
    let ended_note = store
        .slots
        .get(&scope)
        .filter(|slot| slot.persist && slot.turns >= DISTILL_MIN_TURNS)
        .and_then(|slot| slot.note_path.clone());
    let generation = store.slots.get(&scope).map_or(0, |slot| {
        if let Some(cancel) = &slot.cancel {
            cancel.cancel();
        }
        slot.generation + 1
    });
    let mut slot = ChatSlot::new(
        preset_id.clone(),
        new_chat_session(chat_token_budget(&state), preset.as_ref()),
        persist,
        grounding,
    );
    slot.generation = generation;
    store.slots.insert(scope, slot);
    drop(store); // 触发前先释放会话锁：蒸馏要读 Vault，绝不在持锁时跨模块调用。
    if let Some(note_rel) = ended_note {
        super::distill::spawn_distill_if_configured(&app, &note_rel);
        // 对话终局 = 这篇对话笔记的内容边界（与蒸馏同一触发点，不挂逐轮归档——
        // 逐轮嵌一段还没结束的对话，每轮都烧一次 GPU 且语义还在变）。对话默认被检索
        // 排除但**可翻转**（PRD-16-008），翻转那一下要有向量可用。
        super::embed::spawn_embed_if_configured(&app, &note_rel);
        super::distill::notify_chat_distill_hint(&app, &note_rel);
    }
    Ok(ChatSnapshotDto {
        scope,
        messages: Vec::new(),
        busy: false,
        temporary: !persist,
        grounding,
        preset_id,
        note_path: None,
    })
}

/// 列出内置对话角色预设（PRD-03-010）。
#[tauri::command]
#[specta::specta]
pub(crate) fn list_chat_presets() -> Vec<ChatPresetDto> {
    luna_core::builtin_chat_presets(luna_core::prompt::registry())
        .into_iter()
        .map(|p| ChatPresetDto {
            id: p.id.as_str().to_string(),
            name: p.name,
        })
        .collect()
}

/// 取走待带入的选区文本（单次消费，PRD-03-008）。
#[tauri::command]
#[specta::specta]
pub(crate) fn chat_prefill_take(
    state: tauri::State<'_, AppState>,
) -> Result<Option<String>, String> {
    let mut store = state.chat.lock().map_err(|_| chat_lock_err())?;
    Ok(store.prefill.take())
}

/// 历史会话列表（PRD-03-013）：Vault `type: chat` 笔记按修改时间倒序，键集游标分页。
/// Vault 未打开（首启引导前）→ 空列表，不报错。
///
/// 此前是写死 100 条的 top-N，而侧栏徽标显示的是**已加载条数**——第 101 场对话之后，
/// 那个数字就永远停在 100，且第 101 场在界面上根本不存在（2026-09-09 复盘）。列表语义
/// 是「全部会话」，故与速记流同一形态：总数走 `COUNT(*)`，条目沿游标穷尽。
#[tauri::command]
#[specta::specta]
pub(crate) fn chat_history_list(
    state: tauri::State<'_, AppState>,
    cursor: Option<String>,
) -> Result<ChatHistoryDto, String> {
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let Some(vault) = guard.as_ref() else {
        return Ok(ChatHistoryDto {
            total: 0,
            entries: Vec::new(),
            next_cursor: None,
            distillable: chat_distillable(),
            mergeable: chat_mergeable(),
            merge_min: super::distill::MERGE_NOTE_MIN as u32,
            merge_max: super::distill::MERGE_NOTE_LIMIT as u32,
        });
    };
    // 总数与列表**同一次取齐**：两次查询之间被写盘插队，就会出现「徽标说 7 场、点进去
    // 只有 6 行」。
    let total = vault
        .count_notes_of_type(NoteType::Chat)
        .map_err(vault_err)?;
    let (entries, next_cursor) = vault
        // `0` = 按 Rust 的单一页大小常量取一页；前端连读都不读它。
        .list_notes_of_type(NoteType::Chat, 0, cursor.as_deref())
        .map_err(vault_err)?;
    Ok(ChatHistoryDto {
        total,
        entries: entries
            .into_iter()
            .map(|entry| ChatHistoryEntryDto {
                path: entry.path,
                title: entry.title,
                modified_ms: entry.modified_ms as f64,
            })
            .collect(),
        next_cursor,
        distillable: chat_distillable(),
        mergeable: chat_mergeable(),
        merge_min: super::distill::MERGE_NOTE_MIN as u32,
        merge_max: super::distill::MERGE_NOTE_LIMIT as u32,
    })
}

/// 会话可蒸馏吗——问的是共享判据本身，不在这里复述"对话是资料"。
fn chat_distillable() -> bool {
    luna_vault::NoteStage::resolve(NoteType::Chat, None).allows_distill()
}

/// 会话可汇总吗——同上，问 `merge_block`，不在这里复述"只挡聚合页"。
fn chat_mergeable() -> bool {
    luna_vault::merge_block(false).is_none()
}

/// 载入一条历史会话（PRD-03-013）：读 `type: chat` 笔记 → 逆解析为内存会话并链接该笔记，
/// 后续轮次自动写回同一笔记；顶替该作用域现有会话（进行中的生成先取消）。
#[tauri::command]
#[specta::specta]
pub(crate) fn chat_open_note(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    scope: ChatScope,
    path: String,
) -> Result<ChatSnapshotDto, String> {
    let doc = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        vault.read(&path).map_err(vault_err)?
    };
    let parsed = parse_chat_markdown(luna_vault::body_of(&doc.content), &doc.path);
    if !parsed
        .messages
        .iter()
        .any(|m| matches!(m.role, ChatRole::User | ChatRole::Assistant))
    {
        // 路径**不进文案**（`crate::i18n` 的隐私边界写着「绝不拼接文件名」），也不必进：
        // 用户刚点开的就是这一篇。
        return Err(coded_error(&app, "NOT_CHAT_NOTE", "chat-not-chat-note"));
    }
    let system_prompt = parsed
        .messages
        .iter()
        .find(|m| m.role == ChatRole::System)
        .map(|m| m.content.clone());
    // 系统提示词与某内置预设一致 → 恢复预设徽标；否则仅恢复行为（提示词本身）。
    let preset_id = system_prompt.as_deref().and_then(|prompt| {
        luna_core::builtin_chat_presets(luna_core::prompt::registry())
            .into_iter()
            .find(|p| p.system_prompt.as_deref().map(str::trim) == Some(prompt.trim()))
            .map(|p| p.id.as_str().to_string())
    });
    let mut session = ChatSession::new(
        ChatSessionConfig {
            system_prompt,
            token_budget: chat_token_budget(&state),
            ..ChatSessionConfig::default()
        },
        Arc::new(HeuristicTokenCounter::new()),
    );
    for message in parsed
        .messages
        .iter()
        .filter(|m| m.role != ChatRole::System)
        .cloned()
    {
        session.push_history(message);
    }
    let messages = project_messages(session.messages(), &parsed.sources, &parsed.answer_meta);

    let mut store = state.chat.lock().map_err(|_| chat_lock_err())?;
    // 会话终局的另一半（PRD-03-014）：载入历史会话同样**顶替**当前会话——与 `chat_reset`
    // 同一判据（已落盘 + ≥门槛轮次），漏掉这条会让"聊完切去看旧会话"的常见路径永不触发。
    // 载入的目标自己不触发（它此刻只是被打开，没有结束）。
    let ended_note = store
        .slots
        .get(&scope)
        .filter(|slot| slot.persist && slot.turns >= DISTILL_MIN_TURNS)
        .and_then(|slot| slot.note_path.clone())
        .filter(|ended| ended != &doc.path);
    let generation = store.slots.get(&scope).map_or(0, |slot| {
        if let Some(cancel) = &slot.cancel {
            cancel.cancel();
        }
        slot.generation + 1
    });
    // 载入历史 = 持久会话（已在 Vault，后续轮次写回同一笔记）。
    let grounding = grounding_default(&state);
    let mut slot = ChatSlot::new(preset_id.clone(), session, true, grounding);
    slot.generation = generation;
    slot.note_path = Some(doc.path.clone());
    slot.sources = parsed.sources;
    slot.answer_meta = parsed.answer_meta;
    store.slots.insert(scope, slot);
    drop(store); // 触发前先释放会话锁：蒸馏要读 Vault，绝不在持锁时跨模块调用。
    if let Some(note_rel) = ended_note {
        super::distill::spawn_distill_if_configured(&app, &note_rel);
        // 对话终局 = 这篇对话笔记的内容边界（与蒸馏同一触发点，不挂逐轮归档——
        // 逐轮嵌一段还没结束的对话，每轮都烧一次 GPU 且语义还在变）。对话默认被检索
        // 排除但**可翻转**（PRD-16-008），翻转那一下要有向量可用。
        super::embed::spawn_embed_if_configured(&app, &note_rel);
        super::distill::notify_chat_distill_hint(&app, &note_rel);
    }
    Ok(ChatSnapshotDto {
        scope,
        messages,
        busy: false,
        temporary: false,
        preset_id,
        note_path: Some(doc.path),
        grounding,
    })
}

/// 会话 Markdown 逆解析（[`render_chat_markdown`] 的逆操作，PRD-03-013 历史会话载入）。
///
/// 协议：行首 `## System` / `## User` / `## Assistant` 为段界，段内容两侧空白修剪
/// （段间空行是渲染语法）。消息正文自身若含行首的三种精确标题会被误判为段界——
/// 该格式同时服务人读（笔记事实源）与机读，此边界显式接受并以 roundtrip 测试锁定主路径。
#[derive(Debug, Default)]
pub(crate) struct ParsedChatDocument {
    pub messages: Vec<ChatMessage>,
    pub sources: HashMap<usize, Vec<RetrievalSourceDto>>,
    pub answer_meta: HashMap<usize, AnswerMetaDto>,
}

const SOURCES_V1_START: &str = "<!-- luna:sources:v1:start -->";
const SOURCES_V1_END: &str = "<!-- luna:sources:v1:end -->";
const SUPPLEMENT_V1_START: &str = "<!-- luna:model-supplement:v1:start -->";
const SUPPLEMENT_V1_END: &str = "<!-- luna:model-supplement:v1:end -->";
const ANCHOR_V1_PREFIX: &str = "<!-- luna:source-anchor:v1:";

pub(crate) fn parse_chat_markdown(body: &str, note_rel: &str) -> ParsedChatDocument {
    fn flush(out: &mut ParsedChatDocument, current: Option<(ChatRole, String)>, note_rel: &str) {
        if let Some((role, text)) = current {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                let index = out.messages.len();
                if role == ChatRole::Assistant {
                    let parsed = parse_assistant_archive(trimmed, note_rel);
                    out.messages.push(ChatMessage::new(role, parsed.content));
                    if !parsed.sources.is_empty() {
                        out.sources.insert(index, parsed.sources);
                    }
                    if let Some(meta) = parsed.meta {
                        out.answer_meta.insert(index, meta);
                    }
                } else {
                    out.messages.push(ChatMessage::new(role, trimmed));
                }
            }
        }
    }
    let mut parsed = ParsedChatDocument::default();
    let mut current: Option<(ChatRole, String)> = None;
    for line in body.lines() {
        let role = match line.trim_end() {
            "## System" => Some(ChatRole::System),
            "## User" => Some(ChatRole::User),
            "## Assistant" => Some(ChatRole::Assistant),
            _ => None,
        };
        if let Some(role) = role {
            flush(&mut parsed, current.take(), note_rel);
            current = Some((role, String::new()));
        } else if let Some((_, text)) = current.as_mut() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(line);
        }
    }
    flush(&mut parsed, current.take(), note_rel);
    parsed
}

struct ParsedAssistantArchive {
    content: String,
    sources: Vec<RetrievalSourceDto>,
    meta: Option<AnswerMetaDto>,
}

fn parse_assistant_archive(raw: &str, note_rel: &str) -> ParsedAssistantArchive {
    // 补充块与来源块是**两个独立的 v1 结构**：一条 `ModelOnly` 回答天生只有补充块、
    // 没有来源块。把补充块的解析嵌在「来源块解析成功」分支里，这类归档重载后会把
    // `<!-- luna:model-supplement:v1:start -->` 连同本地化标题一起原样显示给用户
    //（真机第七轮实测）——机器边界 marker 永远不该出现在正文里。
    let sources_block = parse_v1_sources(raw, note_rel);
    if let Some((without_sources, sources)) = sources_block {
        let (segments, content, valid_supplement) = parse_v1_supplement(without_sources);
        let outcome = if valid_supplement {
            if segments
                .iter()
                .any(|s| s.kind == AnswerSegmentKind::KnowledgeEvidence)
            {
                GroundingOutcome::Mixed
            } else {
                GroundingOutcome::ModelOnly
            }
        } else if !sources.is_empty() {
            GroundingOutcome::GroundedOnly
        } else {
            GroundingOutcome::ModelOnly
        };
        let segments = if segments.is_empty() {
            vec![AnswerSegmentDto {
                kind: if sources.is_empty() {
                    AnswerSegmentKind::ModelSupplement
                } else {
                    AnswerSegmentKind::KnowledgeEvidence
                },
                markdown: content.clone(),
            }]
        } else {
            segments
        };
        return ParsedAssistantArchive {
            content,
            sources: sources.clone(),
            meta: Some(AnswerMetaDto {
                segments,
                sources,
                grounding_outcome: outcome,
            }),
        };
    }
    // 没有来源块，但有一个完整的补充块 = 纯模型回答（`ModelOnly`）的正常归档形态。
    let (segments, content, valid_supplement) = parse_v1_supplement(raw);
    if valid_supplement {
        return ParsedAssistantArchive {
            content,
            sources: Vec::new(),
            meta: Some(AnswerMetaDto {
                segments,
                sources: Vec::new(),
                grounding_outcome: GroundingOutcome::ModelOnly,
            }),
        };
    }
    if let Some((content, sources)) = parse_v0_sources(raw, note_rel) {
        return ParsedAssistantArchive {
            content: content.clone(),
            sources: sources.clone(),
            meta: Some(AnswerMetaDto {
                // `LegacyUnclassified` 的分段是**未知**，不是「模型补充」。旧回答当时
                // 确实带着出处，把它标成「未由知识库支持」会与紧随其后的出处芯片自相矛盾
                //（真机第七轮实测）。契约要的是「不渲染库内证据分段」，不是改判成补充；
                // 空 `segments` 让前端原样渲染正文，既不冒充有依据、也不冤枉它。
                segments: Vec::new(),
                sources,
                grounding_outcome: GroundingOutcome::LegacyUnclassified,
            }),
        };
    }
    ParsedAssistantArchive {
        content: raw.to_owned(),
        sources: Vec::new(),
        meta: None,
    }
}

fn parse_v1_sources<'a>(
    raw: &'a str,
    note_rel: &str,
) -> Option<(&'a str, Vec<RetrievalSourceDto>)> {
    let end = raw.strip_suffix(SOURCES_V1_END)?.trim_end();
    let start_at = end.rfind(SOURCES_V1_START)?;
    if end[..start_at].contains(SOURCES_V1_START)
        || end[start_at + SOURCES_V1_START.len()..].contains(SOURCES_V1_START)
    {
        return None;
    }
    let body = end[..start_at].trim_end();
    let block = end[start_at + SOURCES_V1_START.len()..].trim();
    let sources = parse_source_links(block, note_rel);
    (!sources.is_empty()).then_some((body, sources))
}

fn parse_v0_sources(raw: &str, note_rel: &str) -> Option<(String, Vec<RetrievalSourceDto>)> {
    let mut lines = raw.lines().collect::<Vec<_>>();
    let last = lines.iter().rposition(|line| !line.trim().is_empty())?;
    let line = lines[last].trim();
    if !(line.starts_with("**出处：**") || line.starts_with("**Sources: **")) {
        return None;
    }
    let sources = parse_source_links(line, note_rel);
    if sources.is_empty() {
        return None;
    }
    lines.remove(last);
    Some((lines.join("\n").trim().to_owned(), sources))
}

fn parse_source_links(block: &str, note_rel: &str) -> Vec<RetrievalSourceDto> {
    let mut out = Vec::new();
    for line in block.lines() {
        let mut cursor = 0usize;
        while let Some(open) = line[cursor..].find('[').map(|n| cursor + n) {
            let Some(mid) = line[open + 1..].find("](").map(|n| open + 1 + n) else {
                break;
            };
            let Some(close) = line[mid + 2..].find(')').map(|n| mid + 2 + n) else {
                break;
            };
            let title = &line[open + 1..mid];
            let href = &line[mid + 2..close];
            let tail = &line[close + 1..];
            let meta = parse_anchor_comment(tail).unwrap_or_default();
            let anchor = meta.anchor;
            out.push(RetrievalSourceDto {
                // 旧注释没有 `index`（或注释损坏）时回落到位置编号：仍可打开，只是不承诺
                // 与正文角标对齐——这与「v0 只能打开、不能承诺精确跳转」同一条降级纪律。
                index: meta.index.unwrap_or(out.len() as u32 + 1),
                path: luna_vault::canonical_link_target(note_rel, href),
                title: title.to_owned(),
                note_type: "note".to_owned(),
                source_material: false,
                draft: false,
                snippet: anchor.quote.clone(),
                anchor,
            });
            cursor = close + 1;
        }
    }
    out
}

/// 归档在锚点注释里携带的、**链接表达不了**的字段：本轮引用编号 + 复合锚。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ArchivedSourceMeta {
    #[serde(default)]
    index: Option<u32>,
    #[serde(flatten)]
    anchor: CitationAnchorDto,
}

fn parse_anchor_comment(tail: &str) -> Option<ArchivedSourceMeta> {
    let start = tail.find(ANCHOR_V1_PREFIX)? + ANCHOR_V1_PREFIX.len();
    let end = tail[start..].find(" -->")? + start;
    let bytes = hex_decode(&tail[start..end])?;
    serde_json::from_slice(&bytes).ok()
}

fn parse_v1_supplement(raw: &str) -> (Vec<AnswerSegmentDto>, String, bool) {
    let starts = raw.match_indices(SUPPLEMENT_V1_START).collect::<Vec<_>>();
    let ends = raw.match_indices(SUPPLEMENT_V1_END).collect::<Vec<_>>();
    if starts.len() != 1 || ends.len() != 1 || starts[0].0 >= ends[0].0 {
        return (Vec::new(), raw.trim().to_owned(), false);
    }
    let evidence = raw[..starts[0].0].trim();
    let mut supplement = raw[starts[0].0 + SUPPLEMENT_V1_START.len()..ends[0].0].trim();
    supplement = supplement
        .strip_prefix("**模型补充 · 未由知识库支持**")
        .or_else(|| {
            supplement.strip_prefix("**Model supplement · not supported by the knowledge base**")
        })
        .unwrap_or(supplement)
        .trim();
    if !raw[ends[0].0 + SUPPLEMENT_V1_END.len()..].trim().is_empty() || supplement.is_empty() {
        return (Vec::new(), raw.trim().to_owned(), false);
    }
    let mut segments = Vec::new();
    if !evidence.is_empty() {
        segments.push(AnswerSegmentDto {
            kind: AnswerSegmentKind::KnowledgeEvidence,
            markdown: evidence.to_owned(),
        });
    }
    segments.push(AnswerSegmentDto {
        kind: AnswerSegmentKind::ModelSupplement,
        markdown: supplement.to_owned(),
    });
    let content = segments
        .iter()
        .map(|s| s.markdown.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    (segments, content, true)
}

fn hex_decode(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let digit = |b: u8| match b {
                b'0'..=b'9' => Some(b - b'0'),
                b'a'..=b'f' => Some(b - b'a' + 10),
                b'A'..=b'F' => Some(b - b'A' + 10),
                _ => None,
            };
            Some(digit(pair[0])? * 16 + digit(pair[1])?)
        })
        .collect()
}

/// 对话归档默认文件名：`<本地化前缀> YYYY-MM-DD HHMM[ <首问摘要>]`（与语音速记
/// `voice_quick_note_stem` 同构，时间靠前使按名排序即按时间排序；摘要经
/// [`sanitized_file_stem`] 清洗，长度上限 [`NOTE_STEM_EXCERPT_CHARS`]）。
fn chat_note_stem(first_user: &str, lang: &str, now: chrono::DateTime<chrono::Local>) -> String {
    let prefix = if lang.starts_with("zh") {
        "对话"
    } else {
        "Chat"
    };
    let time_str = now.format("%Y-%m-%d %H%M").to_string();
    let excerpt = first_user
        .lines()
        .find_map(|line| {
            let trimmed = line.trim();
            (!trimmed.is_empty()).then_some(trimmed)
        })
        .map(|line| {
            line.chars()
                .take(NOTE_STEM_EXCERPT_CHARS)
                .collect::<String>()
        })
        .and_then(|line| sanitized_file_stem(&line));
    match excerpt {
        Some(excerpt) => format!("{prefix} {time_str} {excerpt}"),
        None => format!("{prefix} {time_str}"),
    }
}

/// 会话写盘核心（显式「存入笔记」与每轮自动归档共用，DRY）：
/// - 已链接且笔记仍在 → 保留 frontmatter 整体替换正文写回；
/// - 已链接但笔记被外部删除 → 尊重删除：`create_if_unlinked` 为真才重建，否则解除链接；
/// - 未链接且 `create_if_unlinked` → 按 `chat_note_stem` 新建并链接。
///
/// 返回写入的笔记路径（`None` = 会话为空或按策略不落盘）；仅代际号未变时回写槽位状态。
async fn write_session_note(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    scope: ChatScope,
    generation: u64,
    create_if_unlinked: bool,
) -> Result<Option<String>, String> {
    let (session, note_path, sources, answer_meta) = {
        let store = state.chat.lock().map_err(|_| chat_lock_err())?;
        let Some(slot) = store.slots.get(&scope) else {
            return Ok(None);
        };
        if slot.generation != generation {
            return Ok(None);
        }
        (
            Arc::clone(&slot.session),
            slot.note_path.clone(),
            slot.sources.clone(),
            slot.answer_meta.clone(),
        )
    };
    let (messages, first_user) = {
        let session = session.lock().await;
        let has_content = session
            .messages()
            .iter()
            .any(|m| m.role != ChatRole::System);
        if !has_content {
            return Ok(None);
        }
        let first_user = session
            .messages()
            .iter()
            .find(|m| m.role == ChatRole::User)
            .map(|m| m.content.clone())
            .unwrap_or_default();
        (session.messages().to_vec(), first_user)
    };
    if note_path.is_none() && !create_if_unlinked {
        return Ok(None);
    }

    let lang = backend_copy_arc(app)
        .map(|i| i.language())
        .unwrap_or_else(|| "zh-CN".to_string());
    let zh = lang.starts_with("zh");

    let written = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        // 已链接：读回原文，保留 frontmatter、整体替换正文。
        // 读取失败（被外部删除/移动）→ 按未链接处理，不静默复活旧路径。
        let existing = note_path.as_ref().and_then(|path| vault.read(path).ok());
        match existing {
            Some(doc) => {
                // 出处链接是**相对这篇笔记**算的，故必须先知道落点再渲染正文。
                let body = render_chat_markdown(&messages, &sources, &answer_meta, &doc.path, zh);
                let frontmatter_len = doc.content.len() - luna_vault::body_of(&doc.content).len();
                let content = format!("{}{}", &doc.content[..frontmatter_len], body);
                vault.write(&doc.path, &content).map_err(vault_err)?;
                Some(doc.path)
            }
            None if create_if_unlinked => {
                let dir = note_type_dir(&state.settings_snapshot(), NoteType::Chat);
                let rel = format!(
                    "{dir}/{}",
                    chat_note_stem(&first_user, &lang, chrono::Local::now())
                );
                // 先按预期落点渲染；`create_unique_note` 遇重名会加后缀，那只改文件名
                // 不改目录，相对链接因此不受影响。
                let body = render_chat_markdown(
                    &messages,
                    &sources,
                    &answer_meta,
                    &format!("{rel}.md"),
                    zh,
                );
                let doc = create_unique_note(state.copy(), vault, &rel, NoteType::Chat, body)?;
                Some(doc.path)
            }
            None => None,
        }
    };

    if let Ok(mut store) = state.chat.lock() {
        if let Some(slot) = store.slots.get_mut(&scope) {
            if slot.generation == generation {
                slot.note_path = written.clone();
            }
        }
    }
    if let Some(path) = &written {
        emit_vault_changed(app, path, false);
    }
    Ok(written)
}

enum ChatTurnKind {
    Send(String),
    Regenerate,
}

/// 发送一轮 chat。会话历史留在 Rust 内存，前端只消费事件（PRD-03-002）。
#[tauri::command]
#[specta::specta]
pub(crate) async fn chat_send(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    scope: ChatScope,
    message: String,
    on_event: Channel<ChatEvent>,
) -> Result<(), String> {
    run_chat_turn(&app, &state, scope, ChatTurnKind::Send(message), &on_event).await
}

/// 重新生成最后一条 assistant 回复（默认替换旧回复，PRD-03-011）。
#[tauri::command]
#[specta::specta]
pub(crate) async fn chat_regenerate(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    scope: ChatScope,
    on_event: Channel<ChatEvent>,
) -> Result<(), String> {
    run_chat_turn(&app, &state, scope, ChatTurnKind::Regenerate, &on_event).await
}

/// 停止当前生成，保留已产 token（PRD-03-007）。
#[tauri::command]
#[specta::specta]
pub(crate) fn chat_stop(state: tauri::State<'_, AppState>, scope: ChatScope) -> Result<(), String> {
    let store = state.chat.lock().map_err(|_| chat_lock_err())?;
    if let Some(cancel) = store.slots.get(&scope).and_then(|s| s.cancel.as_ref()) {
        cancel.cancel();
    }
    Ok(())
}

/// 一轮生成的共同编排：占用槽位 → **库内检索** → 提交 Job → 流式转发 → 落账 → 终局广播。
async fn run_chat_turn(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
    scope: ChatScope,
    kind: ChatTurnKind,
    on_event: &Channel<ChatEvent>,
) -> Result<(), String> {
    // 1) 占用槽位（短临界区）：并发提交直接拒绝，避免同一会话交叉写。
    let defaults = slot_defaults(state);
    let (session, generation, grounding, overrides) = {
        let mut store = state.chat.lock().map_err(|_| chat_lock_err())?;
        let slot = store.slot_mut(scope, defaults);
        if slot.busy {
            return Err(coded_error(app, "BUSY", "chat-busy"));
        }
        slot.busy = true;
        (
            Arc::clone(&slot.session),
            slot.generation,
            slot.grounding,
            slot.retrieval_overrides.clone(),
        )
    };
    let local_only = state.settings_snapshot().strict_local();

    // 2) 库内检索（PRD-16-005/006）：**在提交生成之前**完成并下发，故引用角标不依赖
    //    生成结束即可点击。关闭知识库时一步都不做——「零检索开销」是可验收的承诺。
    let question = match &kind {
        ChatTurnKind::Send(message) => Some(message.clone()),
        // 重新生成沿用最近一条用户消息作为检索问题（PRD-16-006「剔除某篇再答一次」
        // 正是走这条路：覆盖项已经变了，检索必须重跑而不是复用上一轮的摘录）。
        ChatTurnKind::Regenerate => {
            let session = session.lock().await;
            session.last_user_message().map(str::to_owned)
        }
    };
    let retrieval = match (grounding, question.as_deref()) {
        (true, Some(question)) => retrieve_for_question(app, state, question, &overrides).await,
        _ => None,
    };
    let grounding_prompt = retrieval.as_ref().map(|(_, prompt)| prompt.clone());
    let retrieved_sources = retrieval
        .as_ref()
        .map(|(info, _)| info.sources.clone())
        .unwrap_or_default();
    if let Some((info, _)) = &retrieval {
        let _ = on_event.send(ChatEvent::Retrieval(info.clone()));
    }

    // 3) 短暂锁会话：刷新路由策略 + 组装上下文并提交 Job（同步、快），随后全程不持锁。
    let turn = {
        let mut session = session.lock().await;
        session.set_local_only(local_only);
        let grounding_prompt = grounding_prompt.as_deref().filter(|p| !p.is_empty());
        let result = match kind {
            ChatTurnKind::Send(message) => {
                session.send(message, grounding_prompt, state.queue.as_ref())
            }
            ChatTurnKind::Regenerate => session.regenerate(grounding_prompt, state.queue.as_ref()),
        };
        match result {
            Ok(turn) => turn,
            Err(e) => {
                release_slot(state, scope, generation);
                return Err(format!("{e:?}"));
            }
        }
    };
    if let Ok(mut store) = state.chat.lock() {
        if let Some(slot) = store.slots.get_mut(&scope) {
            if slot.generation == generation {
                slot.cancel = Some(turn.cancel_token());
            }
        }
    }

    let _ = on_event.send(ChatEvent::Started);
    let context = turn.context();
    if context.truncated {
        // 上下文超长不静默（PRD-03 §5）。
        let _ = on_event.send(ChatEvent::Truncated {
            tokens_in: context.tokens_in,
            input_limit: context.input_limit,
        });
    }

    // 4) 无锁 drain：逐 token 即时经 Channel 推给前端（PRD-03-002）。
    let mut outcome = turn
        .drain_with(|token| {
            let _ = on_event.send(ChatEvent::Token(token.to_string()));
        })
        .await;

    // 5) 落账 + 记出处 + 释放槽位。
    let answer_meta = (outcome.failed.is_none() && !outcome.assistant.is_empty())
        .then(|| {
            retrieval.as_ref().map(|_| {
                let answer = super::knowledge::strip_claim_notices(
                    &outcome.assistant,
                    &super::knowledge::claim_notices(app),
                );
                classify_grounded_answer(&answer, &retrieved_sources)
            })
        })
        .flatten();
    // Sentinel 只属于模型输出协议，绝不能进入会话正文、复制内容或下一轮上下文。
    // 终局结构是唯一真相；会话里只保留用户可读的净正文。
    if outcome.failed.is_none() {
        if let Some(meta) = &answer_meta {
            outcome.assistant = meta
                .segments
                .iter()
                .map(|segment| segment.markdown.as_str())
                .collect::<Vec<_>>()
                .join("\n\n");
        }
    }
    let cited = answer_meta
        .as_ref()
        .filter(|meta| {
            matches!(
                meta.grounding_outcome,
                GroundingOutcome::GroundedOnly | GroundingOutcome::Mixed
            )
        })
        .map(|meta| meta.sources.clone())
        .unwrap_or_default();
    let (assistant_index, absorbed) = {
        let mut session = session.lock().await;
        session.absorb(&outcome);
        (
            session.messages().len().saturating_sub(1),
            outcome.failed.is_none() && !outcome.assistant.is_empty(),
        )
    };
    if let Ok(mut store) = state.chat.lock() {
        if let Some(slot) = store.slots.get_mut(&scope) {
            // 陈旧代际的收尾不得改写新槽（与 busy/cancel/turns 同一纪律）。
            if slot.generation == generation {
                if absorbed {
                    // 先删后插：重新生成会在**同一下标**落一条新回答，旧出处若残留，
                    // 快照与归档笔记会把上一版的出处安在新回答头上——那是谎报。
                    // 只在真的落了账才做（失败/取消没有新消息，动 len-1 会删到上一条的）。
                    record_cited_sources(&mut slot.sources, assistant_index, &cited);
                    if let Some(meta) = answer_meta.clone() {
                        slot.answer_meta.insert(assistant_index, meta);
                    } else {
                        slot.answer_meta.remove(&assistant_index);
                    }
                }
                // 一次性覆盖只在**成功走完**的那一轮消耗掉：失败/取消后用户会点重试，
                // 那一轮理应沿用他刚设的范围——无条件复位等于把「这次不要这篇」静默丢掉。
                if outcome.failed.is_none() && !outcome.cancelled {
                    slot.retrieval_overrides = RetrievalOverrides::default();
                }
            }
        }
    }
    let turn_ordinal = state
        .chat
        .lock()
        .ok()
        .and_then(|store| store.slots.get(&scope).map(|slot| slot.turns))
        .unwrap_or_default();
    release_slot(state, scope, generation);

    // 6) 归档（PRD-03-012/013）：持久会话已链接则恒写回、未链接则新建；**临时对话不落盘**。
    //    失败（如 Vault 未打开）不阻断对话——链接保持 None，历史列表自然不含它。
    let persist = state
        .chat
        .lock()
        .ok()
        .and_then(|store| store.slots.get(&scope).map(|slot| slot.persist))
        .unwrap_or(false);
    let persisted = if persist {
        write_session_note(app, state, scope, generation, true)
            .await
            .ok()
            .flatten()
            .is_some()
    } else {
        false
    };
    // Phase 11 只统计**已经完成且成功归档**的结构化库内问答。临时对话、写盘失败、普通
    // 对话与 legacy 归档都不进分母；answer id 由会话代际/轮次构成，SQLite 只存其 HMAC。
    if let Some(has_supplement) = answer_metric_value(
        persisted,
        outcome.failed.is_some(),
        outcome.cancelled,
        answer_meta.as_ref(),
    ) {
        record_answer_metric(
            app,
            state,
            has_supplement,
            &format!("{scope:?}:{generation}:{turn_ordinal}:{assistant_index}"),
        );
    }

    // 7) 终局事件 + 跨视图广播（携归档链接，视图据此同步"当前会话=哪条历史"）。
    if let Some(err) = &outcome.failed {
        publish_job_failure_notification(Some(app), err);
        let _ = on_event.send(ChatEvent::Failed(ChatFailureDto::from(err)));
    } else if outcome.cancelled {
        let _ = on_event.send(ChatEvent::Cancelled(outcome.assistant));
    } else {
        let _ = on_event.send(ChatEvent::Done {
            text: outcome.assistant,
            sources: cited,
            answer_meta,
        });
    }
    let note_path = state
        .chat
        .lock()
        .ok()
        .and_then(|store| store.slots.get(&scope).map(|slot| slot.note_path.clone()))
        .unwrap_or(None);
    let _ = ChatTurnEnded { scope, note_path }.emit(app);
    Ok(())
}

fn record_answer_metric(
    app: &tauri::AppHandle,
    state: &AppState,
    has_supplement: bool,
    answer_id: &str,
) {
    let root = state.vault_root().map(|path| path.display().to_string());
    let Some(root) = root else {
        return;
    };
    let _ = app
        .try_state::<crate::metrics::LocalKnowledgeMetrics>()
        .and_then(|metrics| {
            let grant = metrics.begin()?;
            metrics.with_store(grant, |store| {
                store.answer(
                    &root,
                    answer_id,
                    has_supplement,
                    chrono::Utc::now().timestamp_millis(),
                )
            })
        });
}

fn answer_metric_value(
    persisted: bool,
    failed: bool,
    cancelled: bool,
    meta: Option<&AnswerMetaDto>,
) -> Option<bool> {
    use crate::ipc::knowledge::{AnswerSegmentKind, GroundingOutcome};
    if !persisted || failed || cancelled {
        return None;
    }
    let meta = meta?;
    if matches!(meta.grounding_outcome, GroundingOutcome::LegacyUnclassified) {
        return None;
    }
    Some(meta.segments.iter().any(|segment| {
        segment.kind == AnswerSegmentKind::ModelSupplement && !segment.markdown.trim().is_empty()
    }))
}

/// 释放槽位：清 busy/cancel。仅当代际号未变（未被 `chat_reset`/`chat_open_note` 顶替）才写回。
fn release_slot(state: &tauri::State<'_, AppState>, scope: ChatScope, generation: u64) {
    if let Ok(mut store) = state.chat.lock() {
        if let Some(slot) = store.slots.get_mut(&scope) {
            if slot.generation == generation {
                slot.busy = false;
                slot.cancel = None;
                // 轮次只在**本代**的收尾累加（陈旧代际的收尾不得改写新槽，与 busy/cancel 同理）。
                slot.turns = slot.turns.saturating_add(1);
            }
        }
    }
}

/// 「保存此对话」（PRD-03-005）：显式写入 Vault `type: chat` 笔记——未链接时新建并链接，
/// 已链接时写回同一笔记。**并把会话转为持久**（临时对话经此转正，后续轮次自动归档，PRD-03-012）。
#[tauri::command]
#[specta::specta]
pub(crate) async fn chat_save_to_note(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    scope: ChatScope,
) -> Result<NoteDocDto, String> {
    let defaults = slot_defaults(&state);
    let generation = {
        let mut store = state.chat.lock().map_err(|_| chat_lock_err())?;
        store.slot_mut(scope, defaults).generation
    };
    let path = write_session_note(&app, &state, scope, generation, true)
        .await?
        .ok_or_else(|| coded_error(&app, "EMPTY_SESSION", "chat-empty-session"))?;
    // 转持久：本会话此后进历史、逐轮自动归档（临时→持久的唯一入口）。
    if let Ok(mut store) = state.chat.lock() {
        if let Some(slot) = store.slots.get_mut(&scope) {
            if slot.generation == generation {
                slot.persist = true;
            }
        }
    }
    let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
    let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
    vault.read(&path).map(Into::into).map_err(vault_err)
}

/// 会话 → Markdown（`type: chat` 笔记正文，System 段一并保留便于追溯预设行为）。
///
/// 出处（PRD-16-011）在**渲染时**拼进正文而非存在消息内容里：消息内容保持纯净，
/// 于是每次重新渲染都得到同一份结果——存进内容再渲染会在逐轮重写时叠加出多行出处。
/// 载入历史后 `sources` 为空，正文里那行 `**出处：**` 作为普通 Markdown 原样保留。
pub(crate) fn render_chat_markdown(
    messages: &[ChatMessage],
    sources: &HashMap<usize, Vec<RetrievalSourceDto>>,
    answer_meta: &HashMap<usize, AnswerMetaDto>,
    note_rel: &str,
    zh: bool,
) -> String {
    let mut out = String::new();
    for (idx, message) in messages.iter().enumerate() {
        let heading = match message.role {
            ChatRole::System => "System",
            ChatRole::User => "User",
            ChatRole::Assistant => "Assistant",
        };
        out.push_str("## ");
        out.push_str(heading);
        out.push_str("\n\n");
        let rendered_body = answer_meta.get(&idx).map_or_else(
            || message.content.clone(),
            |meta| render_answer_segments(meta, zh),
        );
        out.push_str(&rendered_body);
        if !rendered_body.ends_with('\n') {
            out.push('\n');
        }
        if let Some(meta) = answer_meta
            .get(&idx)
            .filter(|meta| meta.grounding_outcome != GroundingOutcome::LegacyUnclassified)
        {
            if !meta.sources.is_empty() {
                out.push('\n');
                out.push_str(&render_sources_block(note_rel, &meta.sources, zh));
                out.push('\n');
            }
        } else if let Some(cited) = sources.get(&idx).filter(|c| !c.is_empty()) {
            // v0 历史只读兼容：重写会话时维持原有的人类可读行，不伪造从未保存过的锚点。
            out.push('\n');
            out.push_str(&render_sources_line(note_rel, cited, zh));
            out.push('\n');
        }
        out.push('\n');
    }
    out
}

fn render_answer_segments(meta: &AnswerMetaDto, zh: bool) -> String {
    let mut out = String::new();
    for segment in &meta.segments {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        if segment.kind == AnswerSegmentKind::ModelSupplement
            && meta.grounding_outcome != GroundingOutcome::LegacyUnclassified
        {
            out.push_str(SUPPLEMENT_V1_START);
            out.push_str("\n\n**");
            out.push_str(if zh {
                "模型补充 · 未由知识库支持"
            } else {
                "Model supplement · not supported by the knowledge base"
            });
            out.push_str("**\n\n");
            out.push_str(segment.markdown.trim());
            out.push_str("\n\n");
            out.push_str(SUPPLEMENT_V1_END);
        } else {
            out.push_str(segment.markdown.trim());
        }
    }
    out
}

fn render_sources_block(note_rel: &str, sources: &[RetrievalSourceDto], zh: bool) -> String {
    let mut out = String::from(SOURCES_V1_START);
    out.push('\n');
    let entries = sources
        .iter()
        .map(|source| {
            let link = luna_vault::markdown_link(note_rel, &source.path, &source.title);
            let encoded = serde_json::to_vec(&ArchivedSourceMeta {
                // 编号必须随归档一起存：正文 `[n]` 用的是**检索全集**的编号，而来源块
                // 只写被引用的子集（PRD-16-005 的两个编号空间）。按位置重排 1..n 会让
                // 重载后的芯片编号与正文角标对不上——角标当场变成点不动的普通文本。
                index: Some(source.index),
                anchor: source.anchor.clone(),
            })
            .map(|bytes| {
                bytes
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            })
            .unwrap_or_default();
            format!("{link}{ANCHOR_V1_PREFIX}{encoded} -->")
        })
        .collect::<Vec<_>>();
    if entries.len() <= 3 {
        out.push_str(if zh { "**出处：**" } else { "**Sources: **" });
        out.push_str(&entries.join(" · "));
    } else {
        out.push_str(if zh { "## 来源" } else { "## Sources" });
        out.push('\n');
        for (index, entry) in entries.iter().enumerate() {
            out.push_str(&format!("{}. {entry}\n", index + 1));
        }
    }
    out.push_str(SOURCES_V1_END);
    out
}

/// 切换本会话是否检索知识库（PRD-16-005 输入区芯片）。
///
/// 会话级覆盖设置默认档——设置 `chat.grounding` 只回答「新会话默认怎样」，
/// 此刻要不要查库是用户当下的决定（PRD-16 D1：全局布尔不是主控件）。
#[tauri::command]
#[specta::specta]
pub(crate) fn chat_set_grounding(
    state: tauri::State<'_, AppState>,
    scope: ChatScope,
    enabled: bool,
) -> Result<(), String> {
    let defaults = slot_defaults(&state);
    let mut store = state.chat.lock().map_err(|_| chat_lock_err())?;
    store.slot_mut(scope, defaults).grounding = enabled;
    Ok(())
}

/// 设置**下一轮**检索的一次性覆盖（PRD-16-006/008/041：透明行里剔除某篇 / 临时纳入对话
/// 笔记 / 整组排除记录态）。
///
/// 设完由前端触发一次重新生成即可换一批依据重答。覆盖项在该轮收尾时自动复位——
/// 它们是对"这一次检索"的纠正，不是会话级偏好（下一个问题重新按默认口径检索）。
#[tauri::command]
#[specta::specta]
pub(crate) fn chat_set_retrieval_overrides(
    state: tauri::State<'_, AppState>,
    scope: ChatScope,
    include_chat: bool,
    exclude_drafts: bool,
    excluded: Vec<String>,
    mentions: Vec<MentionScopeDto>,
) -> Result<(), String> {
    let defaults = slot_defaults(&state);
    let mut store = state.chat.lock().map_err(|_| chat_lock_err())?;
    store.slot_mut(scope, defaults).retrieval_overrides = RetrievalOverrides {
        include_chat,
        exclude_drafts,
        excluded,
        // 形态解析在 Rust：前端原样回传它从 `mention_candidates` 拿到的 `kind`，
        // 认不出的一律丢弃——绝不因为一个拼错的形态就静默把范围放大成全库。
        scope: mentions.iter().filter_map(MentionScopeDto::parse).collect(),
    };
    Ok(())
}

/// 一次 `@` 提及的 IPC 形态（PRD-16-009）。前端只搬运 `mention_candidates` 给过的值。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub(crate) struct MentionScopeDto {
    /// `note` / `folder` / `tag`。
    pub kind: String,
    /// 笔记/文件夹相对路径，或标签名。
    pub value: String,
}

impl MentionScopeDto {
    fn parse(&self) -> Option<luna_vault::RetrieveScope> {
        let value = self.value.trim();
        if value.is_empty() {
            return None;
        }
        match self.kind.as_str() {
            "note" => Some(luna_vault::RetrieveScope::Note(value.to_owned())),
            "folder" => Some(luna_vault::RetrieveScope::Folder(value.to_owned())),
            "tag" => Some(luna_vault::RetrieveScope::Tag(value.to_owned())),
            _ => None,
        }
    }
}

/// 洞见回流（PRD-16-010）：把某条回答存成**知识笔记**，并自动拼「来源」区。
///
/// 与 `save_ai_result_to_note`（悬浮菜单单次结果）分开是因为这条要带出处——
/// 而出处只有 Rust 知道（前端只拿到渲染用的投影）。参数是**投影后的下标**
/// （前端消息列表里的位置），映射回会话下标的口径与 `project_messages` 同源。
#[tauri::command]
#[specta::specta]
pub(crate) async fn chat_save_message_to_note(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    scope: ChatScope,
    message_index: u32,
) -> Result<NoteDocDto, String> {
    let (session, answer_meta) = {
        let store = state.chat.lock().map_err(|_| chat_lock_err())?;
        let slot = store
            .slots
            .get(&scope)
            .ok_or_else(|| coded_error(&app, "EMPTY_SESSION", "chat-empty-session"))?;
        (Arc::clone(&slot.session), slot.answer_meta.clone())
    };
    let (body, cited) = {
        let session = session.lock().await;
        let idx = session_index_of_projected(session.messages(), message_index as usize)
            .ok_or_else(|| coded_error(&app, "MESSAGE_NOT_FOUND", "chat-message-not-found"))?;
        let message = &session.messages()[idx];
        if message.role != ChatRole::Assistant || message.content.trim().is_empty() {
            return Err(coded_error(&app, "EMPTY_RESULT", "chat-empty-result"));
        }
        let meta = answer_meta.get(&idx).ok_or_else(|| {
            coded_error(
                &app,
                "KNOWLEDGE_WRITEBACK_UNAVAILABLE",
                "chat-writeback-no-evidence",
            )
        })?;
        if !matches!(
            meta.grounding_outcome,
            GroundingOutcome::GroundedOnly | GroundingOutcome::Mixed
        ) {
            return Err(coded_error(
                &app,
                "KNOWLEDGE_WRITEBACK_UNAVAILABLE",
                "chat-writeback-unverifiable",
            ));
        }
        let evidence = meta
            .segments
            .iter()
            .filter(|segment| segment.kind == AnswerSegmentKind::KnowledgeEvidence)
            .map(|segment| segment.markdown.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        (evidence, meta.sources.clone())
    };

    let zh = is_zh(&app);
    let dir = note_type_dir(&state.settings_snapshot(), NoteType::Note);
    let stem = knowledge_note_stem(&body, zh, chrono::Local::now());
    let rel = {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        super::distill::unique_note_path_in(vault, &dir, &stem)
    };
    // 来源区按最终落点渲染（同归档路径的理由：相对链接依赖落点）。
    let content = match cited.is_empty() {
        true => body.clone(),
        false => format!("{body}\n\n{}\n", render_sources_line(&rel, &cited, zh)),
    };
    let intent =
        super::knowledge::prepare_knowledge_write(&app, &rel, &content, "chat_writeback", false)?;
    let write_result = (|| -> Result<NoteDoc, String> {
        let guard = state.vault.lock().map_err(|_| "Vault 锁中毒".to_string())?;
        let vault = guard.as_ref().ok_or_else(|| state.vault_not_open())?;
        vault
            .create(
                &rel,
                NoteType::Note,
                &luna_vault::TemplateVars {
                    body: content.clone(),
                    ..luna_vault::TemplateVars::default()
                },
            )
            .map_err(vault_err)
    })();
    let doc = match write_result {
        Ok(doc) => doc,
        Err(error) => {
            super::knowledge::abandon_knowledge_write(&app, intent);
            return Err(error);
        }
    };
    emit_vault_changed(&app, &doc.path, false);
    super::knowledge::commit_knowledge_write(&app, intent)?;
    Ok(doc.into())
}

/// 记录一条落账消息的出处：**先删后插**（PRD-16-005）。
///
/// 重新生成会让新回答落在与旧回答**相同的会话下标**上；只插不删的话，新回答没有引用时
/// 旧条目残留，快照与归档笔记会把上一版的出处安在新回答头上。
fn record_cited_sources(
    sources: &mut HashMap<usize, Vec<RetrievalSourceDto>>,
    index: usize,
    cited: &[RetrievalSourceDto],
) {
    sources.remove(&index);
    if !cited.is_empty() {
        sources.insert(index, cited.to_vec());
    }
}

/// 投影下标（前端消息列表位置）→ 会话内下标。判定与 [`project_messages`] 的过滤同源，
/// 故两处对「第几条」的答案恒一致。
fn session_index_of_projected(messages: &[ChatMessage], projected: usize) -> Option<usize> {
    messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m.role != ChatRole::System)
        .map(|(idx, _)| idx)
        .nth(projected)
}

/// 洞见回流笔记的默认文件名：`<本地化前缀> YYYY-MM-DD HHMM <首行摘要>`
/// （与对话归档、语音速记同构——时间靠前使按名排序即按时间排序）。
fn knowledge_note_stem(body: &str, zh: bool, now: chrono::DateTime<chrono::Local>) -> String {
    let prefix = if zh { "洞见" } else { "Insight" };
    let time_str = now.format("%Y-%m-%d %H%M").to_string();
    let excerpt = body
        .lines()
        .find_map(|line| {
            let trimmed = line.trim().trim_start_matches(['#', '-', '*', '>', ' ']);
            (!trimmed.is_empty()).then_some(trimmed)
        })
        .map(|line| {
            line.chars()
                .take(NOTE_STEM_EXCERPT_CHARS)
                .collect::<String>()
        })
        .and_then(|line| sanitized_file_stem(&line));
    match excerpt {
        Some(excerpt) => format!("{prefix} {time_str} {excerpt}"),
        None => format!("{prefix} {time_str}"),
    }
}

/// 隐藏快捷对话面板（Esc / 关闭按钮；会话保留在内存，PRD-03-012）。
#[tauri::command]
#[specta::specta]
pub(crate) fn quick_chat_close(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("quick-chat") {
        win.hide().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retrieval_overrides_default_to_the_product_default_and_reset_wholesale() {
        // PRD-16-041：「排除记录并重答」是**一次查询有效**的口径翻转，与「临时纳入对话
        // 笔记」同住一次性覆盖里。两件事各锁一条：
        // ① 缺省即产品默认口径——记录态**参与**检索（只是降权），对话默认排除；
        let default = RetrievalOverrides::default();
        assert!(
            !default.exclude_drafts(),
            "记录态默认参与检索，不进默认排除集"
        );
        assert!(!default.include_chat());

        // ② 复位是**整体**复位：用户勾了排除记录、又剔了一篇，成功走完那一轮之后
        //    两者必须一起失效——漏掉任一项，下一个问题就带着上一轮的隐形范围在跑。
        let used = RetrievalOverrides {
            include_chat: true,
            exclude_drafts: true,
            excluded: vec!["Notes/x.md".into()],
            scope: Vec::new(),
        };
        assert!(used.exclude_drafts() && used.include_chat() && !used.excluded().is_empty());
        let after_turn = RetrievalOverrides::default();
        assert!(!after_turn.exclude_drafts());
        assert!(after_turn.excluded().is_empty());
    }

    #[test]
    fn render_chat_markdown_keeps_roles_in_order() {
        let out = render_chat_markdown(
            &[
                ChatMessage::new(ChatRole::User, "问题"),
                ChatMessage::new(ChatRole::Assistant, "回答"),
            ],
            &HashMap::new(),
            &HashMap::new(),
            "Chats/x.md",
            true,
        );
        assert_eq!(out, "## User\n\n问题\n\n## Assistant\n\n回答\n\n");
    }

    #[test]
    fn project_messages_hides_system_prompt() {
        let projected = project_messages(
            &[
                ChatMessage::new(ChatRole::System, "you are luna"),
                ChatMessage::new(ChatRole::User, "hi"),
                ChatMessage::new(ChatRole::Assistant, "hello"),
            ],
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(projected.len(), 2);
        assert_eq!(projected[0].role, "user");
        assert_eq!(projected[1].role, "assistant");
    }

    #[test]
    fn parse_chat_markdown_roundtrips_render_output() {
        // PRD-03-013：载入历史 = 渲染的逆操作。多行/代码块内容原样保留。
        let messages = vec![
            ChatMessage::new(ChatRole::System, "you are luna"),
            ChatMessage::new(ChatRole::User, "解释这段代码\n\n```rust\nfn main() {}\n```"),
            ChatMessage::new(
                ChatRole::Assistant,
                "这是入口函数。\n\n- 无参数\n- 无返回值",
            ),
            ChatMessage::new(ChatRole::User, "继续"),
        ];
        let rendered = render_chat_markdown(
            &messages,
            &HashMap::new(),
            &HashMap::new(),
            "Chats/x.md",
            true,
        );
        assert_eq!(
            parse_chat_markdown(&rendered, "Chats/x.md").messages,
            messages
        );
    }

    #[test]
    fn parse_chat_markdown_ignores_non_chat_content() {
        assert!(parse_chat_markdown("# 普通笔记\n\n正文内容", "Chats/x.md")
            .messages
            .is_empty());
    }

    #[test]
    fn chat_note_stem_carries_time_and_first_question_excerpt() {
        use chrono::TimeZone as _;
        let now = chrono::Local
            .with_ymd_and_hms(2026, 7, 14, 15, 30, 0)
            .unwrap();
        let stem = chat_note_stem("如何配置 Redis 的 maxmemory 参数？", "zh-CN", now);
        assert!(stem.starts_with("对话 2026-07-14 1530 "), "{stem}");
        assert!(!stem.contains('？'), "文件系统保留/清洗字符不入名：{stem}");

        let plain = chat_note_stem("", "en-US", now);
        assert_eq!(plain, "Chat 2026-07-14 1530");
    }

    /// 出处在**渲染时**拼入，不进消息内容——否则逐轮重写会叠加出多行出处。
    #[test]
    fn sources_line_is_rendered_not_stored_so_repeated_saves_never_duplicate() {
        let messages = vec![
            ChatMessage::new(ChatRole::User, "问题"),
            ChatMessage::new(ChatRole::Assistant, "回答"),
        ];
        let mut sources = HashMap::new();
        sources.insert(
            1usize,
            vec![RetrievalSourceDto {
                index: 1,
                path: "Notes/甲.md".to_string(),
                title: "甲".to_string(),
                note_type: "note".to_string(),
                source_material: false,
                draft: false,
                snippet: "摘录".to_string(),
                anchor: CitationAnchorDto {
                    heading_path: vec![],
                    quote: String::new(),
                    prefix: String::new(),
                    suffix: String::new(),
                    line_hint: 1,
                    source_hash: "hash".to_string(),
                },
            }],
        );
        let once = render_chat_markdown(&messages, &sources, &HashMap::new(), "Chats/x.md", true);
        assert_eq!(once.matches("**出处：**").count(), 1, "{once}");
        // 同一份消息再渲染一次仍是一行——归档每轮都会重渲染整篇。
        let twice = render_chat_markdown(&messages, &sources, &HashMap::new(), "Chats/x.md", true);
        assert_eq!(once, twice);
        // 消息内容本身保持纯净。
        assert!(!messages[1].content.contains("出处"));
    }

    #[test]
    fn structured_archive_roundtrips_sources_anchors_and_mixed_segments() {
        let messages = vec![
            ChatMessage::new(ChatRole::User, "问题"),
            ChatMessage::new(ChatRole::Assistant, "库内结论 [1]\n\n模型补充"),
        ];
        let source = RetrievalSourceDto {
            index: 1,
            path: "Notes/甲.md".into(),
            title: "甲".into(),
            note_type: "note".into(),
            source_material: false,
            draft: false,
            snippet: "摘录".into(),
            anchor: CitationAnchorDto {
                heading_path: vec!["结论".into()],
                quote: "这是能够重新定位的原句".into(),
                prefix: "前文".into(),
                suffix: "后文".into(),
                line_hint: 9,
                source_hash: "source-hash".into(),
            },
        };
        let mut sources = HashMap::new();
        sources.insert(1, vec![source.clone()]);
        let mut meta = HashMap::new();
        meta.insert(
            1,
            AnswerMetaDto {
                segments: vec![
                    AnswerSegmentDto {
                        kind: AnswerSegmentKind::KnowledgeEvidence,
                        markdown: "库内结论 [1]".into(),
                    },
                    AnswerSegmentDto {
                        kind: AnswerSegmentKind::ModelSupplement,
                        markdown: "模型补充".into(),
                    },
                ],
                sources: vec![source],
                grounding_outcome: GroundingOutcome::Mixed,
            },
        );
        let rendered = render_chat_markdown(&messages, &sources, &meta, "Chats/x.md", true);
        let parsed = parse_chat_markdown(&rendered, "Chats/x.md");
        assert_eq!(parsed.sources[&1][0].anchor.source_hash, "source-hash");
        assert_eq!(parsed.sources[&1][0].anchor.line_hint, 9);
        assert_eq!(
            parsed.answer_meta[&1].grounding_outcome,
            GroundingOutcome::Mixed
        );
        assert_eq!(parsed.answer_meta[&1].segments.len(), 2);
        assert!(!parsed.messages[1].content.contains("luna:sources"));
        assert!(!parsed.messages[1].content.contains("luna:model-supplement"));
    }

    #[test]
    fn archived_sources_keep_their_original_citation_numbers() {
        // 正文 `[n]` 用的是**检索全集**编号，来源块只写被引用的子集：按位置重排成
        // 1..n 会让重载后的芯片与正文角标对不上，角标当场变成点不动的普通文本
        //（真机第七轮实测：会话里是 [2]/[4]，重开后芯片成了 ①②）。
        fn src(index: u32, title: &str) -> RetrievalSourceDto {
            RetrievalSourceDto {
                index,
                path: format!("Notes/{title}.md"),
                title: title.into(),
                note_type: "note".into(),
                source_material: false,
                draft: false,
                snippet: String::new(),
                anchor: CitationAnchorDto {
                    quote: format!("{title} 的一句足够长的原文"),
                    line_hint: index * 3,
                    ..CitationAnchorDto::default()
                },
            }
        }
        let messages = vec![
            ChatMessage::new(ChatRole::User, "问题"),
            ChatMessage::new(ChatRole::Assistant, "结论 [2] 与 [4]。"),
        ];
        let cited = vec![src(2, "乙"), src(4, "丁")];
        let mut sources = HashMap::new();
        sources.insert(1, cited.clone());
        let mut meta = HashMap::new();
        meta.insert(
            1,
            AnswerMetaDto {
                segments: vec![AnswerSegmentDto {
                    kind: AnswerSegmentKind::KnowledgeEvidence,
                    markdown: "结论 [2] 与 [4]。".into(),
                }],
                sources: cited.clone(),
                grounding_outcome: GroundingOutcome::GroundedOnly,
            },
        );
        let rendered = render_chat_markdown(&messages, &sources, &meta, "Chats/x.md", true);
        let parsed = parse_chat_markdown(&rendered, "Chats/x.md");
        assert_eq!(
            parsed.sources[&1]
                .iter()
                .map(|s| s.index)
                .collect::<Vec<_>>(),
            vec![2, 4],
            "归档往返必须保住原始引用编号"
        );
        assert_eq!(parsed.sources[&1][1].anchor.line_hint, 12);
    }

    #[test]
    fn legacy_v0_archive_lists_sources_without_claiming_anything_about_grounding() {
        // 旧归档的分段是**未知**：既不能渲染成「库内证据」，也不能改判成「模型补充 ·
        // 未由知识库支持」——后者与它紧随其后列出的出处芯片自相矛盾（真机第七轮实测）。
        let raw = "旧格式回答，只有一行普通出处。\n\n**出处：**[甲](../Notes/%E7%94%B2.md)"
            .replace("%E7%94%B2", "甲");
        let doc = parse_chat_markdown(&format!("## Assistant\n\n{raw}"), "Chats/x.md");
        let meta = &doc.answer_meta[&0];
        assert_eq!(meta.grounding_outcome, GroundingOutcome::LegacyUnclassified);
        assert_eq!(meta.sources.len(), 1, "旧出处清单仍要恢复");
        assert!(meta.segments.is_empty(), "边界未知就不要凭空造一个分段");
        assert!(
            !doc.messages[0].content.contains("**出处：**"),
            "来源行必须从正文里剥掉"
        );
    }

    #[test]
    fn model_only_archive_strips_its_supplement_markers() {
        // 纯模型回答天生只有补充块、没有来源块。解析若把补充块嵌在「来源块解析成功」
        // 分支里，这类归档重载后会把 `<!-- luna:model-supplement:v1:start -->` 与本地化
        // 标题一起原样显示给用户（真机第七轮实测）。
        let messages = vec![
            ChatMessage::new(ChatRole::User, "问题"),
            ChatMessage::new(ChatRole::Assistant, "这一整条都是模型自己说的。"),
        ];
        let sources = HashMap::new();
        let mut meta = HashMap::new();
        meta.insert(
            1,
            AnswerMetaDto {
                segments: vec![AnswerSegmentDto {
                    kind: AnswerSegmentKind::ModelSupplement,
                    markdown: "这一整条都是模型自己说的。".into(),
                }],
                sources: Vec::new(),
                grounding_outcome: GroundingOutcome::ModelOnly,
            },
        );
        let rendered = render_chat_markdown(&messages, &sources, &meta, "Chats/x.md", true);
        assert!(
            rendered.contains("luna:model-supplement"),
            "归档里应当有边界 marker"
        );
        let parsed = parse_chat_markdown(&rendered, "Chats/x.md");
        assert!(
            !parsed.messages[1].content.contains("luna:model-supplement"),
            "marker 绝不能出现在呈现给用户的正文里：{}",
            parsed.messages[1].content
        );
        assert_eq!(
            parsed.answer_meta[&1].grounding_outcome,
            GroundingOutcome::ModelOnly
        );
        assert_eq!(parsed.answer_meta[&1].segments.len(), 1);
    }

    /// 投影下标 → 会话下标的映射必须与 [`project_messages`] 的过滤同源，
    /// 否则「存这条回答」会存错一条（System 消息在会话里占位、在投影里不占）。
    #[test]
    fn projected_index_maps_back_past_the_hidden_system_message() {
        let messages = [
            ChatMessage::new(ChatRole::System, "预设"),
            ChatMessage::new(ChatRole::User, "问"),
            ChatMessage::new(ChatRole::Assistant, "答"),
        ];
        assert_eq!(session_index_of_projected(&messages, 0), Some(1));
        assert_eq!(session_index_of_projected(&messages, 1), Some(2));
        assert_eq!(session_index_of_projected(&messages, 2), None);
        // 与投影本身对齐（同一组输入，投影长度即可用下标数）。
        assert_eq!(
            project_messages(&messages, &HashMap::new(), &HashMap::new()).len(),
            2
        );
    }

    /// 出处落账必须**先删后插**：重新生成的新回答落在同一下标，旧出处残留即谎报。
    #[test]
    fn record_cited_sources_replaces_and_clears_stale_entries() {
        fn src(index: u32) -> RetrievalSourceDto {
            RetrievalSourceDto {
                index,
                path: format!("Notes/{index}.md"),
                title: index.to_string(),
                note_type: "note".to_string(),
                source_material: false,
                draft: false,
                snippet: String::new(),
                anchor: CitationAnchorDto {
                    heading_path: vec![],
                    quote: String::new(),
                    prefix: String::new(),
                    suffix: String::new(),
                    line_hint: 1,
                    source_hash: String::new(),
                },
            }
        }
        let mut map = HashMap::new();
        record_cited_sources(&mut map, 1, &[src(1), src(2)]);
        assert_eq!(map.get(&1).map(Vec::len), Some(2));

        // 重新生成：新回答换了引用 → 整组替换，不残留旧条目。
        record_cited_sources(&mut map, 1, &[src(3)]);
        assert_eq!(map.get(&1).map(Vec::len), Some(1));
        assert_eq!(map[&1][0].index, 3);

        // 新回答没有引用 → 旧条目必须清掉，否则上一版的出处安在新回答头上。
        record_cited_sources(&mut map, 1, &[]);
        assert!(!map.contains_key(&1));
        // 其它下标不受影响。
        record_cited_sources(&mut map, 5, &[src(9)]);
        record_cited_sources(&mut map, 1, &[]);
        assert!(map.contains_key(&5));
    }

    #[test]
    fn answer_metric_uses_only_saved_structured_grounding_outcomes() {
        fn meta(outcome: GroundingOutcome, supplement: &str) -> AnswerMetaDto {
            AnswerMetaDto {
                segments: vec![AnswerSegmentDto {
                    kind: AnswerSegmentKind::ModelSupplement,
                    markdown: supplement.to_owned(),
                }],
                sources: Vec::new(),
                grounding_outcome: outcome,
            }
        }
        let grounded = meta(GroundingOutcome::GroundedOnly, "");
        let mixed = meta(GroundingOutcome::Mixed, "补充");
        let model_only = meta(GroundingOutcome::ModelOnly, "补充");
        let legacy = meta(GroundingOutcome::LegacyUnclassified, "补充");
        assert_eq!(
            answer_metric_value(true, false, false, Some(&grounded)),
            Some(false)
        );
        assert_eq!(
            answer_metric_value(true, false, false, Some(&mixed)),
            Some(true)
        );
        assert_eq!(
            answer_metric_value(true, false, false, Some(&model_only)),
            Some(true)
        );
        assert_eq!(answer_metric_value(true, false, false, Some(&legacy)), None);
        assert_eq!(answer_metric_value(false, false, false, Some(&mixed)), None);
        assert_eq!(answer_metric_value(true, true, false, Some(&mixed)), None);
        assert_eq!(answer_metric_value(true, false, true, Some(&mixed)), None);
        assert_eq!(answer_metric_value(true, false, false, None), None);
    }

    #[test]
    fn chat_failure_dto_exposes_stable_action_fields_without_backend_text() {
        let err = luna_core::JobError::ModelMissing {
            model: luna_core::ModelId::from("qwen3"),
            hint: "去模型页下载".into(),
        };
        let dto = ChatFailureDto::from(&err);
        assert_eq!(dto.code, JobErrorCodeDto::ModelMissing);
        assert!(!dto.retriable);
        assert_eq!(dto.retry_after_secs, None);

        let session = luna_core::JobError::Unauthorized {
            provider: luna_core::ProviderId::from("lunarca-cloud"),
        };
        let dto = ChatFailureDto::from(&session);
        assert_eq!(dto.code, JobErrorCodeDto::SignInRequired);
        assert!(!dto.retriable);

        let upstream = luna_core::JobError::Upstream {
            provider: luna_core::ProviderId::from("local-llm"),
            code: Some("503".into()),
            message: "overloaded".into(),
        };
        let dto = ChatFailureDto::from(&upstream);
        assert_eq!(dto.code, JobErrorCodeDto::Upstream);
        assert!(dto.retriable);
        assert_eq!(dto.retry_after_secs, None);

        let limited = luna_core::JobError::RateLimited {
            provider: luna_core::ProviderId::from("lunarca-cloud"),
            retry_after_secs: Some(11),
        };
        let dto = ChatFailureDto::from(&limited);
        assert_eq!(dto.code, JobErrorCodeDto::RateLimited);
        assert!(dto.retriable);
        assert_eq!(dto.retry_after_secs, Some(11));
    }
}
