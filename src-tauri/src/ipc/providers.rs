//! Provider 装配（IPC 层，从 `ipc/mod.rs` 抽出；架构评估缺口 B）。
//!
//! 单一 AI 主干（ADR-001）的**组装点**：构造 `ProviderRegistry` + 注册各 Provider
//! （随包引擎 `local-llm` / `local-embed`、`local-whisper` 实时/批量档、macOS `vision-ocr`）+ 建 `JobQueue`
//! + 装配 `AppState`；以及 ASR/LLM Provider 的运行时按需注册/热替换与模型路径解析。
//!
//! 与命令域分离：命令层只发命令/订阅事件（各 `ipc/<域>.rs`），装配集中于此，新增 Provider
//! 只动本文件一处。`impl AppState` 的装配方法与 `mod.rs` 的状态访问器同属一个 `AppState`
//! （Rust 允许 `impl` 跨文件）；共享项经 `use super::*` 从 `mod.rs` 取。

use super::*;

/// 该槽位是不是指向了服务端转写（**不含**隐私档判断——那是另一个问题）。
fn slot_points_to_service(settings: &Settings, slot: &str) -> bool {
    crate::ipc::effective_assignment_target(settings, slot)
        .as_deref()
        .is_some_and(crate::ipc::is_remote_asr_target)
}

/// 批量档转写的服务端那条腿处在哪一步（[`AppState::remote_batch_asr`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RemoteBatchAsr {
    /// 槽位没指向服务端：在本机跑。
    NotChosen,
    /// 选了服务端，但严格本地挡着。
    StrictLocal,
    /// 选了服务端，但账户所在组织没开放转写（R4b）：退回本机兜底腿。
    NotServed,
    /// 选了服务端，但服务端转写的同意没给（Provider 没注册）。
    NotRegistered,
    /// 交给服务端：定向路由用的 Provider id。
    Ready(ProviderId),
}

/// 「槽位指向服务端 × 严格本地」这对矛盾的唯一措辞。
///
/// 两条通道（实时 / 离线）撞的是同一条判据，故说同一句话——各写一份的话，改了一处
/// 另一处就成了旧说法，而用户看到的是哪一条取决于他先用了哪个功能。
fn strict_local_conflict_message(i18n: BackendCopy<'_>, key: &str) -> String {
    coded_error_with(i18n, "STRICT_LOCAL_BLOCKED", key)
}

impl AppState {
    /// 备好采集侧要用的流式 ASR 句柄（实时字幕 / 语音输入 / 录音实时转录共用）。
    ///
    /// 三种形状，由「实时转写」槽位与「服务端转写」开关决定：
    ///
    /// | 槽位 | 本机权重 | 装上的句柄 |
    /// | --- | --- | --- |
    /// | 本地模型 | 有 | `WhisperProvider` |
    /// | `service:asr` | 有 | `FailoverAsrProvider{ 服务端实时, whisper }` |
    /// | `service:asr` | 无 | `RealtimeAsrProvider`（单腿，没有可回落的东西） |
    ///
    /// # Errors
    /// 既没有可用本地权重、也没有服务端通道时返回可读原因（缺模型 / 开关没开）。
    pub(crate) fn ensure_asr_provider_ready(&self, app: &tauri::AppHandle) -> Result<(), String> {
        let settings = self.settings_snapshot();
        let root = self.models_root(app);
        let shape = self.desired_stream_asr_shape(&settings, &root);
        // 形状没变就不重建。重建会把**正在录的那个会话**的登记表换掉（`StreamId` 登在
        // 旧实例里），而这只在「录音途中改了个不相干的设置」时才现形。
        if self
            .asr_provider
            .lock()
            .ok()
            .and_then(|g| g.as_ref().map(|(current, _)| current == &shape))
            .unwrap_or(false)
        {
            return Ok(());
        }
        self.install_stream_asr(app, &settings, shape)
    }

    /// 按当前设置算出该组什么形状（不加载任何权重）。
    ///
    /// **纯函数式**：只读设置与目录，不看「此刻装了什么」。设置页因此可以问它
    /// 「兜底模型会是哪一个」——而 ASR 是懒加载的（首个任务才装配），去问已装的那个
    /// 会在冷启动时得到 `None`，于是界面把随包 tiny 说成用户的兜底档，录一次音之后
    /// 说法又变。同一个问题两个时刻两个答案，正是本轮重构要消灭的那类毛病。
    pub(crate) fn desired_stream_asr_shape(
        &self,
        settings: &Settings,
        root: &Path,
    ) -> StreamAsrShape {
        // 先看槽位指向哪儿——**在碰本地模型库之前**（与 `ensure_batch_asr_provider`
        // 同一条纪律）：顺序反过来的话，选了服务端却本地没装权重时会先报「模型缺失，
        // 请去安装」，而用户根本没打算用本地模型。
        // 严格本地硬拦（ADR-038 ④）：这条腿**根本不构造**。
        //
        // 只靠 `Router` 按 `location` 过滤是不够的——过滤只在派发那一刻生效，而组合
        // Provider 一旦装上，采集侧的窗口已经在往它的通道里投了。麦克风内容是隐私量级
        // 最高的一类，拦截点必须在「有没有这条腿」而不是「要不要用这条腿」。
        //
        // 组织没开放转写（R4b）时这条腿同样不构造：先连一次服务端吃 403 再回落，是每次
        // 开录都白付一个往返——而离线那条是定向路由，连回落都没有，直接报错。
        let wants_service =
            !settings.strict_local() && slot_points_to_service(settings, "realtime_asr");
        let to_service = wants_service && self.server_serves("transcribe");
        let local = if wants_service {
            // ADR-038 ⑤「本地 whisper **恒为**兜底」：槽位此刻指向服务端，那里解析不出
            // 本地权重，故兜底腿自己去目录里找一个——随包最小档优先，否则退到已安装的
            // 最小 ASR 模型（`fallback_asr_weight` 里写了为什么是"最小"）。
            //
            // 此前这里只认随包 tiny，于是装了 Whisper Small 却没装 tiny 的机器**根本
            // 没有兜底**：卡片说「还没有本机转写模型」，仪表盘同时说「Whisper Small 就绪」。
            // 一个都没装才是真的没有兜底，那时如实说没有。
            crate::model_store::fallback_asr_weight(root)
        } else {
            whisper_model_path_from_settings_and_root(settings, root)
        };
        StreamAsrShape { to_service, local }
    }

    /// 按形状加载/复用各腿，组装后装上（并让注册表里只留这一个流式候选）。
    fn install_stream_asr(
        &self,
        app: &tauri::AppHandle,
        settings: &Settings,
        shape: StreamAsrShape,
    ) -> Result<(), String> {
        let whisper = shape
            .local
            .as_deref()
            .map(|path| self.ensure_local_whisper(path))
            .transpose()?;
        if !shape.to_service {
            let Some(whisper) = whisper else {
                // 「没有本地腿」有两个完全不同的原因，给的话必须不同：
                // 真的没装模型 → 去装；而**槽位指向服务端却开着严格本地** → 那是两个
                // 设置在打架，去装模型对他毫无帮助。指向错误动作的报错比不报错更伤。
                return Err(
                    if settings.strict_local() && slot_points_to_service(settings, "realtime_asr") {
                        strict_local_conflict_message(
                            self.copy(),
                            "strict-local-blocked-realtime-asr",
                        )
                    } else {
                        asr_model_missing_message(self.copy())
                    },
                );
            };
            return self.set_stream_asr(shape, whisper);
        }
        if !crate::ipc::remote_asr_available(settings) {
            return Err(coded_error(
                app,
                "REMOTE_ASR_DISABLED",
                "remote-asr-disabled-realtime",
            ));
        }
        let remote = Arc::new(luna_provider::asr_realtime::RealtimeAsrProvider::new(
            luna_provider::asr_realtime::RealtimeAsrConfig {
                api_base: self.cloud_server().origin,
                bearer: Arc::new(crate::cloud_bearer::CloudBearer::new(app.clone())),
                gate: self.outbound_gate(),
            },
        ));
        match whisper {
            Some(whisper) => self.set_stream_asr(
                shape,
                Arc::new(luna_provider::asr_failover::FailoverAsrProvider::new(
                    remote, whisper,
                )),
            ),
            None => self.set_stream_asr(shape, remote),
        }
    }

    /// 加载或复用本机实时档 whisper（按权重路径缓存，避免重复读几百 MB）。
    fn ensure_local_whisper(&self, model_path: &Path) -> Result<Arc<WhisperProvider>, String> {
        if !model_path.is_file() {
            return Err(asr_model_missing_message(self.copy()));
        }
        if let Ok(guard) = self.local_asr.lock() {
            if let Some((cached, provider)) = guard.as_ref() {
                if cached == model_path {
                    return Ok(Arc::clone(provider));
                }
            }
        }
        let provider = Arc::new(
            WhisperProvider::load(crate::whisper_backend::config_for(model_path)).map_err(|e| {
                coded_error_args_with(
                    self.copy(),
                    "MODEL_LOAD_FAILED",
                    "model-load-failed-asr",
                    &[("error", e.to_string())],
                )
            })?,
        );
        // 批量转写会按 id 复用这一档（`ensure_batch_asr_provider` 同模型即复用），
        // 故它自己也要在注册表里，而不只是作为组合的一条腿。
        self.provider_registry.register_or_replace(provider.clone());
        if let Ok(mut guard) = self.local_asr.lock() {
            *guard = Some((model_path.to_owned(), Arc::clone(&provider)));
        }
        Ok(provider)
    }

    /// 细分模型齐备即注册本地说话人细分 Provider（L1，ADR-020）；缺失则静默不注册——
    /// Diarize Job 按能力路由失败，重转写侧按「无细分能力」跳过（诚实降级，TDD-03 §10.2）。
    /// 启动时探测一次；模型安装计划执行完成后即再调本方法热注册。
    /// 改聚类阈值设置（`diarize.cluster_threshold`）后由命令层再调本方法重建热生效。
    pub(crate) fn register_diarize_provider_if_available(&self, app: &tauri::AppHandle) {
        let root = self.models_root(app);
        if crate::diarize_provider::SherpaDiarizeProvider::models_present(&root) {
            let threshold = diarize_cluster_threshold(&self.settings_snapshot());
            self.provider_registry.register_or_replace(Arc::new(
                crate::diarize_provider::SherpaDiarizeProvider::new(&root, threshold),
            ));
        }
    }

    /// 注册服务端转写 Provider（ADR-036 批式通道）。
    ///
    /// **只在 `provider.remote_asr_enabled` 显式开启时注册**。不能靠「注册顺序 =
    /// 本机优先」兜住隐私默认值：本机 whisper 是懒加载的（首个转写任务才注册），
    /// 启动后的一段时间里注册表里根本没有本地候选，云会成为唯一候选。靠时序保证
    /// 「默认档不是云」（ADR-038 ④），是一条随时会被重构掉的隐性依赖。
    ///
    /// 未登录不影响注册：Provider 自己在取不到凭据时本地短路，一个字节都不发。
    /// 反过来说，**不能**用「是否登录」当注册条件——那会让开关的语义变成
    /// 「登录了就自动上传录音」。
    fn refresh_remote_batch_asr(&self, app: &tauri::AppHandle) {
        let settings = self.settings_snapshot();
        let id = luna_core::ProviderId::from(luna_provider::asr_remote::REMOTE_ASR_PROVIDER_ID);
        // 严格本地下**不注册**（ADR-038 ④，与实时通道同一条纪律）：不存在的 Provider
        // 比"存在但会被过滤掉"更难出错——过滤逻辑将来被改动时，不注册这条不会跟着松。
        // 组织没开放转写（R4b）也注销：离线转写因此改走本机，而不是定向打一个必拒的候选。
        if settings.strict_local()
            || !settings
                .get_bool("provider.remote_asr_enabled")
                .unwrap_or(false)
            || !self.server_serves("transcribe")
        {
            // **注销**，不是「不注册就完了」。本方法要能被反复调用并收敛到当前设置：
            // 只注册不注销的话，用户关掉开关后这条腿会一直留在注册表里，直到下次重启。
            self.provider_registry.remove(&id);
            return;
        }
        self.provider_registry.register_or_replace(Arc::new(
            luna_provider::asr_remote::RemoteAsrProvider::new(
                luna_provider::asr_remote::RemoteAsrConfig {
                    api_base: self.cloud_server().origin,
                    bearer: Arc::new(crate::cloud_bearer::CloudBearer::new(app.clone())),
                    gate: self.outbound_gate(),
                },
            ),
        ));
    }

    /// **ASR 装配的唯一刷新入口**：两条通道（实时流式 / 批式）在这里一起对齐到当前设置。
    ///
    /// 分成两个入口是上一版的真实缺陷：批式那条只在启动时注册过一次，流式那条只挂在
    /// 「模型分配变更」上——于是改「服务端转写」开关要重启才生效，而**打开严格本地后
    /// 已装的云腿根本不会消失**。后者是隐私承诺的直接违反：开关显示「仅本地」，下一次
    /// 录音照样往外发。
    ///
    /// 幂等：可反复调用，每次都收敛到当前设置。
    ///
    /// # Errors
    /// 流式那条需要加载本机权重时可能失败（缺模型 / 权重损坏），如实上报。
    pub(crate) fn refresh_asr_assembly(&self, app: &tauri::AppHandle) -> Result<(), String> {
        self.refresh_remote_batch_asr(app);
        self.refresh_stream_asr(app)
    }

    /// 设置变化后按新形状重组流式 ASR——**仅在已经装过的情况下**。
    ///
    /// 没装过就什么都不做，维持懒加载纪律：绝不为一次 LLM/嵌入分配顺手重读几百 MB
    /// whisper 权重，那会把每次改分配都变成秒级的命令线程阻塞。
    pub(crate) fn refresh_stream_asr(&self, app: &tauri::AppHandle) -> Result<(), String> {
        let Some(current) = self
            .asr_provider
            .lock()
            .ok()
            .and_then(|g| g.as_ref().map(|(shape, _)| shape.clone()))
        else {
            return Ok(());
        };
        let settings = self.settings_snapshot();
        let shape = self.desired_stream_asr_shape(&settings, &self.models_root(app));
        if shape == current {
            return Ok(());
        }
        // 新形状既没有本地权重也没有服务端 → 如实下线（下次任务报缺模型），
        // 而不是留着旧句柄假装还能转写。
        if shape.local.is_none() && !shape.to_service {
            self.unregister_asr_provider();
            return Ok(());
        }
        self.install_stream_asr(app, &settings, shape)
    }

    /// 批量档转写此刻会不会交给服务端（不碰本地模型库、不加载权重、不联网）。
    ///
    /// [`Self::ensure_batch_asr_provider`] 与任务前预估（`ipc::estimate`，PRD-13-040 ②）问的是同一个
    /// 问题——前者据它选 Provider，后者据它决定要不要先问一句「约消耗多少」。各判一遍必然有一天
    /// 预估说「从预付扣」而转写其实在本机跑。
    pub(crate) fn remote_batch_asr(&self, settings: &Settings) -> RemoteBatchAsr {
        if !slot_points_to_service(settings, "batch_asr") {
            return RemoteBatchAsr::NotChosen;
        }
        if settings.strict_local() {
            return RemoteBatchAsr::StrictLocal;
        }
        if !self.server_serves("transcribe") {
            return RemoteBatchAsr::NotServed;
        }
        let id = ProviderId::from(luna_provider::asr_remote::REMOTE_ASR_PROVIDER_ID);
        if self.provider_registry.get(&id).is_none() {
            return RemoteBatchAsr::NotRegistered;
        }
        RemoteBatchAsr::Ready(id)
    }

    /// 确保**批量档 ASR Provider** 就绪并注册，返回其定向路由用的 Provider id（PRD-04-030/032）。
    ///
    /// 模型解析：离线转写槽为空时继承实时转写槽。若解析出的路径相同 → 直接复用实时档
    /// （`WHISPER_PROVIDER_ID`），不重复加载权重；否则加载/复用缓存的批量档
    /// （`WHISPER_BATCH_PROVIDER_ID`，与实时档并存），按模型路径缓存避免每次重载。
    pub(crate) fn ensure_batch_asr_provider(
        &self,
        app: &tauri::AppHandle,
    ) -> Result<ProviderId, String> {
        let settings = self.settings_snapshot();
        // 先看槽位是不是指向服务端——**在碰本地模型库之前**。
        //
        // 顺序反过来的话，选了服务端却本地没装 whisper 权重时会先报「模型缺失，请去
        // 安装」，而用户根本没打算用本地模型。错误信息指向一个与他的选择无关的动作，
        // 是最容易把人带偏的一类。
        match self.remote_batch_asr(&settings) {
            RemoteBatchAsr::NotChosen => {}
            RemoteBatchAsr::StrictLocal => {
                return Err(strict_local_conflict_message(
                    self.copy(),
                    "strict-local-blocked-batch-asr",
                ));
            }
            // 组织没开放转写（R4b）：回到实时档那条本机兜底腿（ADR-038 ⑤ 同一份权重），
            // 与对话「没开放的能力在本机运行」同一说法。没有本机权重时它如实报缺模型。
            RemoteBatchAsr::NotServed => {
                self.ensure_asr_provider_ready(app)?;
                return Ok(ProviderId::from(WHISPER_PROVIDER_ID));
            }
            // 开关被关掉、Provider 没注册时如实报错而不是悄悄回落本地：
            // 用户以为音频没出设备 / 出了设备，取决于我们悄悄选了谁——那不能靠猜。
            RemoteBatchAsr::NotRegistered => {
                return Err(coded_error(
                    app,
                    "REMOTE_ASR_DISABLED",
                    "remote-asr-disabled-batch",
                ));
            }
            RemoteBatchAsr::Ready(id) => return Ok(id),
        }

        let root = self.models_root(app);
        let default_path = whisper_model_path_from_settings_and_root(&settings, &root);
        let batch_path = whisper_model_path_for_assignment(&settings, &root, "batch_asr")
            .ok_or_else(|| asr_model_missing_message(self.copy()))?;
        if !batch_path.is_file() {
            return Err(asr_model_missing_message(self.copy()));
        }

        // 与实时档同模型 → 复用实时档 Provider（避免双份权重驻留）。
        if default_path.as_ref() == Some(&batch_path) {
            self.ensure_asr_provider_ready(app)?;
            return Ok(ProviderId::from(WHISPER_PROVIDER_ID));
        }

        // 命中缓存（同模型路径）→ 复用。
        {
            let guard = self
                .batch_asr
                .lock()
                .map_err(|_| "批量档锁中毒".to_string())?;
            if let Some((cached_path, _)) = guard.as_ref() {
                if cached_path == &batch_path {
                    return Ok(ProviderId::from(WHISPER_BATCH_PROVIDER_ID));
                }
            }
        }

        // 加载批量档（独立 id + 解析后端），注册进 registry 供定向路由。
        let provider = Arc::new(
            WhisperProvider::load(
                crate::whisper_backend::config_for(&batch_path)
                    .with_provider_id(WHISPER_BATCH_PROVIDER_ID),
            )
            .map_err(|e| {
                coded_error_args_with(
                    self.copy(),
                    "MODEL_LOAD_FAILED",
                    "model-load-failed-retranscribe",
                    &[("error", e.to_string())],
                )
            })?,
        );
        self.provider_registry.register_or_replace(provider.clone());
        if let Ok(mut guard) = self.batch_asr.lock() {
            *guard = Some((batch_path, provider));
        }
        Ok(ProviderId::from(WHISPER_BATCH_PROVIDER_ID))
    }

    /// 装上流式 ASR 句柄（与 registry 内同一 `Arc`），供录音/语音输入 `register_stream`。
    ///
    /// 类型是 [`luna_core::StreamingAsr`] 接缝而不是 `WhisperProvider`：实时转写现在
    /// 可能是本机 whisper、服务端实时通道，或两者的降级组合（ADR-038 ⑤），采集侧不该
    /// 知道是哪个。
    ///
    /// 同时保证注册表里**只有这一个**流式实时候选。留着上一个形状的话，
    /// 路由钉住的那个 id 可能指向一条已经不该用的腿——用户在设置里关掉了服务端转写，
    /// 而下一次录音照样往外发。
    fn set_stream_asr<P: luna_core::StreamingAsr + 'static>(
        &self,
        shape: StreamAsrShape,
        provider: Arc<P>,
    ) -> Result<(), String> {
        let id = provider.id();
        for other in [
            luna_provider::asr_failover::FAILOVER_ASR_PROVIDER_ID,
            luna_provider::asr_realtime::REALTIME_ASR_PROVIDER_ID,
        ] {
            let other = luna_core::ProviderId::from(other);
            if other != id {
                self.provider_registry.remove(&other);
            }
        }
        self.provider_registry.register_or_replace(provider.clone());
        let mut guard = self
            .asr_provider
            .lock()
            .map_err(|_| "实时 ASR 句柄锁中毒".to_owned())?;
        *guard = Some((shape, provider));
        Ok(())
    }

    /// 外发闸句柄（注入给外发型 Provider；事实源是隐私档，写入点唯一）。
    pub(crate) fn outbound_gate(&self) -> Arc<dyn luna_core::OutboundGate> {
        Arc::clone(&self.outbound_gate) as Arc<dyn luna_core::OutboundGate>
    }

    /// 按当前隐私档对齐外发闸。见 [`PrivacyOutboundGate`]。
    pub(crate) fn sync_outbound_gate(&self) {
        self.outbound_gate.sync(self.strict_local());
    }

    /// 当前流式 ASR 句柄（`None` = 既没有本地模型也没有服务端通道）。
    #[must_use]
    pub fn asr_provider_handle(&self) -> Option<Arc<dyn luna_core::StreamingAsr>> {
        self.asr_provider
            .lock()
            .ok()
            .and_then(|g| g.as_ref().map(|(_, provider)| Arc::clone(provider)))
    }

    /// 写进 `MediaRef.model` 的转写模型名（导出与二次转写提示用）。
    ///
    /// 主腿是服务端时返回 `None`：此刻我们**并不知道**服务端会用哪个模型，那是
    /// 会话建立后由 `executed_by` 告知的。往笔记元数据里写一个猜测，比留空更糟——
    /// 它以后会被当成事实读。
    /// 只回答「**此刻装载**的是哪个权重」，故只服务归档用途（`MediaRef.model`）。
    /// 设置页要的「兜底模型会是哪一个」**不能**问它——ASR 懒加载，冷启动时它恒为 `None`，
    /// 界面会把随包 tiny 说成用户的兜底档。那个问题问 [`Self::desired_stream_asr_shape`]。
    /// 不把这段抽成公开访问器，正是为了让下一个人拿不到这个诱人的错答案。
    #[must_use]
    pub(crate) fn asr_model_display_name(&self) -> Option<String> {
        let guard = self.asr_provider.lock().ok()?;
        let (shape, _) = guard.as_ref()?;
        if shape.to_service {
            return None;
        }
        shape
            .local
            .as_deref()
            .map(crate::ipc::processing::model_display_name)
    }

    /// 下线实时 ASR Provider（分配已解析不出模型时的诚实收场；下次任务如实报缺模型）。
    pub(crate) fn unregister_asr_provider(&self) {
        for id in [
            WHISPER_PROVIDER_ID,
            luna_provider::asr_failover::FAILOVER_ASR_PROVIDER_ID,
            luna_provider::asr_realtime::REALTIME_ASR_PROVIDER_ID,
        ] {
            self.provider_registry
                .remove(&luna_core::ProviderId::from(id));
        }
        if let Ok(mut guard) = self.asr_provider.lock() {
            *guard = None;
        }
        if let Ok(mut guard) = self.local_asr.lock() {
            *guard = None;
        }
    }

    /// 注册默认档 LLM Provider —— 云端为主、本机最小模型为不可达兜底。
    ///
    /// | 端点来源 | 装的 Provider | 打哪儿 |
    /// | --- | --- | --- |
    /// | `OnDevice` | `LlmHttpProvider::local_engine` | 随包引擎（回环地址，每次请求现问） |
    /// | `LunarcaServer` / `OfficialCloud` | [`CloudLlmProvider`] | `{基址}/inference/v1`（原生面） |
    ///
    /// 登录或连接服务端后同时注册远端与本机候选；Router 确定性地把云端放在前面，
    /// JobQueue 只在连接失败/超时且尚未产生可见输出时切到本机。鉴权、额度、限流与 5xx
    /// 都不会被本地结果掩盖。用户显式选择本机或开启严格本地时只注册本机候选。
    ///
    /// # 装配这件事只剩这一个实现
    ///
    /// 曾经有两条路：构造期（`build_state…`，拿不到 `AppHandle`）无条件按本机档装，
    /// 热切期这一条才解析端点。于是配了服务端的用户每次重启后对话都回落本机，而设置页
    /// 写着「由服务端执行」——改任意一个云设置才「莫名其妙好了」。构造期那条已经删除。
    ///
    /// 剩下两个调用点，各自回答一个不同的问题，都不得再自己解析端点：
    /// - [`crate::cloud::reconfigure_cloud_stack`] —— **落点变了**（启动 / 换服务端 /
    ///   翻隐私档 / 登录态变化）；
    /// - `models::refresh_model_assignment_consumers` —— **模型变了**。它没有并进上面那个，
    ///   是因为模型分配要求「配置落盘与 Provider 刷新同成同败」：ASR 重装失败必须把分配
    ///   回滚，而落点入口对同一个失败只记日志（那里没有可回滚的东西）。
    ///
    /// `availability` 必须由调用方传入而不是就地求值：门控、落点展示与本函数从此读同一个
    /// 答案，签名本身就拦住了「再算一遍」。
    ///
    /// `bearer` 是**凭据来源的注入点**（与 `StrictLocalProbe` / `CloudGate` 同一形态）：
    /// 生产传 [`crate::cloud_bearer::CloudBearer`]（账户会话，ADR-155 唯一的云凭据），测试传桩。
    ///
    /// # 原生服务端不接收客户端具体模型
    ///
    /// 本函数从同一份设置快照解析端点与有效分配，调用方不能另传一份可能分叉的目标。
    /// 原生推理面始终省略 `model`，具体模型由服务端路由，实际执行者由 `executed_by` 回传。
    ///
    /// | 端点与分配来源 | 实际执行 |
    /// | --- | --- |
    /// | 无远端端点 | 本机条目 id 在随包引擎执行；`service:<m>` 就绪判不兼容 |
    /// | 原生服务端 + **未显式分配** | 服务端执行且不发 model，让服务端选默认 |
    /// | 原生服务端 + 显式本机条目 | **仍在本机执行**，连接不能改写用户选择 |
    /// | 原生服务端 + 旧 `service:<m>` | 不兼容；不解释、不迁移 |
    ///
    /// 本机条目 id / `service:` 是执行位置契约，不是展示标签。只有**未显式选择**时才沿用
    /// ADR-037 的默认「连接后使用服务端」；否则连接状态覆盖分配目标，会让设置与字节去向相反。
    pub(crate) fn register_llm_provider(
        &self,
        availability: crate::cloud::CloudAvailability,
        bearer: Arc<dyn luna_provider::asr_remote::BearerSource>,
    ) {
        let settings = self.settings_snapshot();
        let endpoint = crate::ipc::compute_endpoint_source(availability, &self.cloud_server());
        let local = crate::ipc::local_llm_for_slot(
            &settings,
            &self.engine_models_snapshot(),
            "default_llm",
        );
        let explicit_local = endpoint.is_server()
            && !crate::ipc::models::assignment_uses_native_service(
                &settings,
                "default_llm",
                &endpoint,
            );

        // 组织把对话类能力全关了（R4b），或这台服务端一个对话上游都没配（ADR-041 ①）：服务端
        // 这条腿一项都不能声明，与本机档同一装法。
        let served = self.served_capabilities();
        let nothing_served = luna_provider::cloud_llm::served_under(served.as_deref()).is_empty();
        if matches!(endpoint, crate::cloud::ComputeEndpoint::OnDevice)
            || explicit_local
            || nothing_served
        {
            // 本机档只认本机模型。`service:` 目标在这里不该出现（就绪判 `incompatible`）；
            // 真出现了也宁可装一个空名字——引擎会如实说模型不存在，好过把一个服务端模型名
            // 静默喂给本机运行时，那种失败指向的排查方向是错的。
            let providers = local
                .map(|llm| vec![llm.provider(self, false)])
                .unwrap_or_default();
            self.install_default_llm_candidates(providers);
            return;
        }

        let remote: Arc<dyn luna_core::AIProvider> = Arc::new(
            luna_provider::CloudLlmProvider::new(luna_provider::CloudLlmConfig {
                api_base: endpoint.url().to_owned(),
                display_name: match &endpoint {
                    crate::cloud::ComputeEndpoint::OfficialCloud(_) => "Lunarca 云".to_owned(),
                    _ => format!("服务端（{}）", endpoint.url()),
                },
                bearer,
                gate: self.outbound_gate(),
                allowed: served,
            }),
        );
        let mut providers = vec![remote];
        if let Some(llm) = &local {
            providers.push(llm.provider(self, false));
        }
        self.install_default_llm_candidates(providers);
    }

    /// 原子式替换默认 LLM 候选集合；集合顺序不表达路由策略。
    fn install_default_llm_candidates(&self, providers: Vec<Arc<dyn luna_core::AIProvider>>) {
        for id in [
            luna_provider::LOCAL_LLM_PROVIDER_ID,
            luna_provider::CLOUD_LLM_PROVIDER_ID,
        ] {
            self.provider_registry
                .remove(&luna_core::ProviderId::from(id));
        }
        for provider in providers {
            self.provider_registry.register_or_replace(provider);
        }
    }

    /// 热切实时档 LLM Provider：继承默认档时下线定向 Provider；显式不同目标才注册。
    ///
    /// **必须在 [`Self::register_llm_provider`] 之后调用**：实时档只接受显式定向，
    /// 不进入默认候选链；先刷新默认集合可避免短暂保留已失效的端点配置。
    ///
    /// **登记 / 撤销 `ModelSlot::RealtimeLlm` 的槽位分配与装卸 Provider 是同一步**
    /// （ADR-083 ⑥）：路由层按「泳道 → 槽位」总映射解析定向 Provider，各调用点不再自己
    /// 传 `preferred_provider`。两件事分成两处做，就会出现「Provider 在注册表里但没有
    /// 任何东西指向它」或反过来「槽位指着一个已经下线的 id」——后者的表现是实时翻译静默
    /// 回落默认档，而设置页仍显示实时档生效，正是本批修的那个缺陷的形状。
    ///
    /// **`ModelSlot::DefaultLlm` 不登记**：本机形态下的「默认质量档」不是某一个
    /// Provider，它就是默认候选链本身（见 `ProviderRegistry::assign_slot` 的注）。
    pub(crate) fn refresh_realtime_llm_provider(
        &self,
        availability: crate::cloud::CloudAvailability,
    ) {
        let settings = self.settings_snapshot();
        let realtime = crate::ipc::realtime_local_llm(
            &settings,
            &self.engine_models_snapshot(),
            availability,
            &self.cloud_server(),
        );
        // 先下线旧的：槽位清空后注册表里不能留一个没有槽位指向、却仍按能力可被选中的
        // 实时档 Provider。
        self.provider_registry.remove(&luna_core::ProviderId::from(
            luna_provider::LOCAL_LLM_REALTIME_PROVIDER_ID,
        ));
        match realtime {
            Some(llm) => {
                self.provider_registry
                    .register_or_replace(llm.provider(self, true));
                self.provider_registry.assign_slot(
                    luna_core::ModelSlot::RealtimeLlm,
                    Some(llm.realtime_provider_id()),
                );
            }
            None => {
                self.provider_registry
                    .assign_slot(luna_core::ModelSlot::RealtimeLlm, None);
            }
        }
    }
}

/// 构造应用状态：注册**纯本机**档 Provider（macOS vision-ocr）
/// 的 JobQueue + 语言检测器 + 引擎句柄 + 给定设置。
///
/// **必须在 tokio 运行时内调用**（`JobQueue::new` 内部 spawn 调度循环）；调用方在
/// `tauri::async_runtime::block_on` 内构造（见 `lib::run`）。
///
/// # 这里**不注册默认档 LLM**
///
/// 默认档 LLM 的落点由端点解析决定，而端点解析需要「云此刻可不可用」——那是 `AppHandle`
/// 才答得出的问题，本函数拿不到。此前这里无条件注册了一个本机档，代价是：配了
/// 组织服务端或登录了 Lunarca 云的用户，**每次重启后对话与翻译都打回本机**，而设置页
/// 同时写着「由服务端执行」；改任意一个云设置才会「莫名其妙好了」。
///
/// 装配因此收成**一个入口**：[`crate::cloud::reconfigure_cloud_stack`]，启动时在
/// `app.manage(state)` 之后调用一次（= 「从无到有换一次服务端」）。留在本函数里的只有
/// **不依赖端点解析**的那几档——它们恒在本机，没有第二种可能。
///
/// `artifact_fetcher`：装配注入的工件下载实现（TDD-14 §3.2 ②，经
/// [`crate::AppAssembly::with_artifact_fetcher`] 传入）；`None`（OSS 默认）= 直连实现。
#[must_use]
pub fn build_state_with_settings_persistence(
    settings: Settings,
    settings_persistence: SettingsPersistence,
    device_usage: UsageStore,
    artifact_fetcher: Option<Arc<dyn crate::artifact_fetcher::ArtifactFetcher>>,
) -> AppState {
    // 设备级配置目录（库注册表落点；`config.toml` 的同级目录）。在 `settings_persistence`
    // 被 move 进 AppState **之前**取好。
    let config_dir = match &settings_persistence {
        SettingsPersistence::Writable(config_path) => {
            config_path.parent().map(std::path::Path::to_path_buf)
        }
        SettingsPersistence::MemoryOnly => None,
    };
    // 启动直开不经过 `bind_vault`，所以必须在任何 Vault 级设置、续跑档、媒体服务或注册表
    // 消费路径之前就锚定真实根。否则同一次启动会同时拥有“配置别名”和“Vault 真实根”两种
    // 身份，软链替换还可能让不同子系统各自落到不同目录。
    // 配置里指着、却打不开的库根（外置盘拔了 / 目录被删）——记下来，让工作区的空态
    // 能说「上次的库在 X 找不到了」而不是「尚未选择库」（PRD-14-062）。
    let configured_root = explicit_vault_root(&settings);
    let explicit_root = configured_root.as_ref().and_then(|root| {
        std::fs::canonicalize(root)
            .map_err(|error| {
                eprintln!(
                    "[vault] 打开已配置 Vault 失败：{error}（path={}）；将回到未选择状态。",
                    root.display()
                );
            })
            .ok()
    });
    let vault_registry = luna_vault::registry::VaultRegistry::new(config_dir.clone());
    // 库列表跟着账户走（ADR-151 修订 2026-10-15「续二」）：启动装载不经 `bind_vault`，可见性在
    // 这里判——启动指针指着的库列表账户看不见，就开它上次打开的；都没有就停在空状态。必须在
    // Vault 级设置、续跑档之前定下来（它们都按这个根取）。
    let (explicit_root, hidden_at_startup) =
        crate::vault_lists::startup_root(&vault_registry, explicit_root);
    // 续跑档随库（ADR-025）：启动时按当前库定落点；无库（首启未绑定）则不持久化，
    // 绑定库后由 `open_vault_path` / 切库编排经 `ProcessingQueue::rebind` 接上。
    let queue_state_path = explicit_root
        .as_deref()
        .map(crate::processing_queue::queue_state_path);
    let settings = explicit_root
        .as_ref()
        .map(|root| settings_with_vault_scope_from_root(settings.clone(), root))
        .unwrap_or(settings);
    let registry = Arc::new(ProviderRegistry::new());
    // **两档 LLM 都不在这里注册**（见本函数文档）。
    //
    // 默认档是因为落点要看端点解析；实时档跟着一起走，理由是**顺序**：注册顺序即兜底
    // 顺序（`Router::resolve` 的「兜底 = 注册顺序」），只把默认档挪到启动后，实时档就
    // 抢到了首位——于是每一条没有 `preferred_provider` 的对话/翻译/摘要都会落到那个
    // 「配了小模型换低延迟」的实时档上，而它的存在理由恰恰是**只**服务实时字幕。
    // 两档同批由 `cloud::reconfigure_cloud_stack` 按「默认 → 实时」装上，顺序即在那里。
    //
    // **嵌入也不在这里注册**——与两档 LLM 同一纪律、同一理由：落点要看端点解析，
    // 唯一注册点是装配（`refresh_embed_provider`，启动经 `cloud::reconfigure_cloud_stack`
    // 走到）。此前这里按设备策略无条件装上本机档，装配的服务端分支没装上时保留的就是
    // 这个陈旧本机档：屏幕写「服务端托管」，向量却在本机静默生成（真机第八轮实测本机
    // 嵌入档 finished）——正是设置页明文承诺不做的「自动改写为本地索引」。注册表空时
    // 身份解析如实回 `server_unavailable`。
    // OCR 走单一 AI 主干（ADR-001）：macOS 注册 Vision OCR Provider（Capability::Ocr）；
    // 其它平台不注册 → OCR Job 按能力路由失败（诚实"暂无 OCR"，插槽留给 Windows/云 OCR）。
    #[cfg(target_os = "macos")]
    registry.register(Arc::new(crate::ocr_provider::VisionOcrProvider));
    let asr_provider: StreamAsrCache = Arc::new(Mutex::new(None));
    let local_asr: LocalAsrCache = Arc::new(Mutex::new(None));
    let outbound_gate = Arc::new(PrivacyOutboundGate::default());
    let batch_asr: BatchAsrCache = Arc::new(Mutex::new(None));
    // ASR 由首个实时/批量任务经分配服务懒加载；启动不猜模型目录，也不在主线程读权重。
    let voice_input = Arc::new(VoiceInputController::default());
    let recording = Arc::new(RecordingController::default());
    let voice_options = voice_start_options_from_settings(&settings);
    voice_input.set_mode(
        voice_options.landing_mode,
        voice_options.input_mode,
        voice_options.auto_refine,
        voice_options.translate_enabled,
        voice_options.translate_target_lang.to_string(),
    );
    let snapshot_max = snapshot_max_from_settings(&settings);
    let media_server = MediaServer::spawn().expect("启动本地媒体 Range 服务失败");
    let device_state = crate::device_state::DeviceStateStore::new(config_dir.clone());
    // MVP-0 门控（PRD-14）：不再自动创建/打开默认 Vault。仅当显式配置了 vault.root（或
    // LUNA_VAULT_ROOT）时启动即打开——即沿用用户上次在引导内选择的 Vault，免每次重选。
    // 否则保持 None，由首启引导调用 select_vault / create_vault_root 绑定。
    let vault = explicit_root.and_then(|root| match Vault::open(&root) {
        Ok(opened) => {
            let mut opened = opened.with_snapshot_max_versions(snapshot_max);
            // 不在这里同步扫描：1 万篇的库实测要 50 秒、期间应用起不来（笔记库对标轮 V1）。
            // 索引在 `lib.rs` setup 里起后台增量扫描（`start_index_scan`），与切库同一入口。
            let root = opened.root().to_path_buf();
            media_server.set_root(root.clone());
            // 登记进设备级库清单（PRD-06-049）：启动装载与 `bind_vault` 是打开库的两条
            // 路径，语义必须一致——只在 bind 里登记的话，启动即带库的既有用户永远不在
            // 清单里，切换器按"空清单"整个不渲染，多库入口无从谈起。
            // 此刻还不知道谁登录着（云客户端在 setup 里才有）：账户那一笔由启动对齐补记
            // （`vault_lists::startup`）。
            super::register_opened_vault(&vault_registry, &mut opened, config_dir.as_deref(), None);
            Some(opened)
        }
        Err(e) => {
            eprintln!(
                "[vault] 打开已配置 Vault 失败：{e}（path={}）；将回到未选择状态。",
                root.display()
            );
            None
        }
    });

    // 启动指针指着的库只是这个账户看不见（不是找不到了）：空状态不说「上次的库找不到了」。
    let startup_vault_missing = match &vault {
        Some(_) => None,
        None if hidden_at_startup => None,
        None => configured_root,
    };
    let vault = Arc::new(Mutex::new(vault));
    let usage_sink = Arc::new(AppUsageSink::new(device_usage.clone(), Arc::clone(&vault)));

    AppState {
        backend_copy: std::sync::OnceLock::new(),
        // **准入额度的唯一注入点**（ADR-078 ①）。此前这里是一个裸 `4`，四类消费者
        // ——实时主轨 ASR、实时副轨 ASR、实时转译、批式二次转写——共用它：Both 档一场
        // 实时会话自己就占掉 3 个，批式占掉第 4 个，零余量。而流式 ASR 的额度是在
        // `execute()` **之前**发的，于是「批式在跑」直接等于「新开的实时会话连一条
        // session.start 都发不出去」。
        //
        // 会话池容量按**采集轨数**派生（不写死）：`RecordingCaptureSource` 的轨数上限
        // （Both = 麦克风 + 系统回环 = 2）+ 实时转译 1 条。麦克风硬互斥（TDD-05 §5.3）
        // 保证同一时刻只有一路采集会话，故这也是全进程的上限。
        queue: Arc::new(JobQueue::with_usage_sink(
            Arc::clone(&registry),
            AdmissionCapacity::for_capture_tracks(RecordingCaptureSource::max_track_count()),
            usage_sink,
        )),
        provider_registry: registry,
        asr_provider,
        local_asr,
        outbound_gate,
        batch_asr,
        // 语言检测器唯一装配点：此处决定注入哪个 `LanguageDetector` 实现（当前 lingua）。
        // 持有端全部依赖 `Arc<dyn LanguageDetector>` 接口，换实现只改这一行（自动 unsize）。
        detector: Arc::new(LinguaDetector::new()),
        settings: Mutex::new(settings),
        settings_persistence,
        local_engine: luna_engine::llama::LlamaServerSupervisor::new(),
        local_engine_plan: tokio::sync::Mutex::new(crate::local_engine::EnginePlan::default()),
        engine_models: Mutex::new(Vec::new()),
        cloud_server: Mutex::new(crate::cloud::ServerRef::of(
            luna_cloud::config::DEFAULT_API_BASE,
        )),
        cloud_capabilities: Mutex::new(None),
        cloud_deployment: Mutex::new(None),
        popup: Arc::new(Mutex::new(PopupSession::default())),
        popup_blur: Mutex::new(PopupBlurState::default()),
        desktop_lens: DesktopLensSession::default(),
        mic: Arc::new(Mutex::new(MicArbiter::new())),
        voice_input,
        recording,
        tts: Arc::new(crate::tts::TtsController::new()),
        device_usage,
        vault,
        startup_vault_missing,
        vault_watcher: Mutex::new(None),
        chat: Arc::new(Mutex::new(ChatSessionStore::default())),
        media_server,
        // 续跑档与设备级配置同目录（PRD-05-012）：配置不可写时 `None` = 不持久化，
        // 与设置的 `Disabled` 语义一致（不半写、不猜路径）。
        processing: Arc::new(crate::processing_queue::ProcessingQueue::new(
            queue_state_path.clone(),
        )),
        floating_menu: Mutex::new(FloatingMenuRun::default()),
        popup_runs: Mutex::new(std::collections::HashMap::new()),
        model_installs: Mutex::new(std::collections::HashMap::new()),
        floating_pending: Mutex::new(std::collections::HashMap::new()),
        note_windows: Mutex::new(std::collections::HashMap::new()),
        note_window_seq: std::sync::atomic::AtomicU64::new(0),
        desktop_note_windows: Mutex::new(HashMap::new()),
        teleprompter_script: Mutex::new(None),
        teleprompter_follow: Mutex::new(None),
        teleprompter_marks: Mutex::new(Vec::new()),
        desktop_note_idle: Mutex::new(Vec::new()),
        desktop_note_seq: std::sync::atomic::AtomicU64::new(0),
        graph_tags_inflight: Mutex::new(std::collections::HashSet::new()),
        graph_tags_pending: Mutex::new(std::collections::HashSet::new()),
        graph_rescan: Mutex::new(GraphRescan::default()),
        distill_inflight: Mutex::new(std::collections::HashSet::new()),
        distill_preview: Mutex::new(None),
        embed_matrix: Mutex::new(None),
        embed_index_identity: Mutex::new(None),
        embed_inflight: Mutex::new(std::collections::HashSet::new()),
        embed_rebuild: Mutex::new(crate::ipc::EmbedRebuild::default()),
        checkup_run: Mutex::new(crate::ipc::CheckupRun::default()),
        chat_ctx_window: std::sync::atomic::AtomicU32::new(0),
        model_install_plans: Mutex::new(std::collections::HashMap::new()),
        model_delete_plans: Mutex::new(std::collections::HashMap::new()),
        storage_relocation: Mutex::default(),
        pending_vault_link: Mutex::default(),
        pending_navigation: Default::default(),
        vault_generation: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        vault_switching: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        vault_registry,
        device_state,
        secondary_vaults: Mutex::new(std::collections::HashMap::new()),
        vault_window_seq: std::sync::atomic::AtomicU64::new(0),
        scan: Arc::new(ScanControl::default()),
        external_renames: ExternalRenameWatch::default(),
        // 工件下载接缝唯一装配点（TDD-14 §3.2 ②）：装配注入（`AppAssembly::
        // with_artifact_fetcher`，官方构建 = 制品面实现：digest → 镜像集 → 断点续传 →
        // 本地 sha256 终验）优先；未注入（OSS 默认）恒落直连实现——行为零变化。
        artifact_fetcher: artifact_fetcher
            .unwrap_or_else(|| Arc::new(crate::artifact_fetcher::DirectArtifactFetcher)),
    }
}

/// 细分聚类阈值（`diarize.cluster_threshold`，×100 整数存储，S12 结论 #5）。
/// clamp 到 [0.30, 0.95]：阈值离谱会把聚类拉爆（全并 1 人/全裂散）。
#[must_use]
fn diarize_cluster_threshold(settings: &Settings) -> f32 {
    (settings
        .get_int("diarize.cluster_threshold")
        .unwrap_or(70)
        .clamp(30, 95) as f32)
        / 100.0
}

#[must_use]
fn asr_model_missing_message(i18n: BackendCopy<'_>) -> String {
    coded_error_with(i18n, "MODEL_MISSING", "model-missing-realtime-asr")
}

#[must_use]
pub fn models_root_from_settings(settings: &Settings, app_data_dir: Option<PathBuf>) -> PathBuf {
    // 搬移状态机写过的绝对目录优先（PRD-10-014）；空 / 非绝对一律回默认，不猜。
    if let Some(custom) = settings
        .get_str("storage.models_root")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
    {
        return custom;
    }
    app_data_dir
        .map(|dir| dir.join("models"))
        .unwrap_or_else(|| std::env::temp_dir().join("luna-models"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::CloudAvailability;

    /// 嵌入 Provider 的**唯一注册点是装配**（与两档 LLM 同一纪律）：`build_state`
    /// 曾按设备策略无条件装上本机档，装配的服务端分支没装上时保留的就是这个陈旧本机档——屏幕写「服务端托管」，向量却在本机静默生成，正是
    /// 设置页明文承诺不做的「自动改写为本地索引」（真机第八轮实测）。
    #[test]
    fn build_state_registers_no_embed_provider_of_its_own() {
        // 整个 providers.rs（非测试区）都不该构造嵌入 Provider——构造与注册全归
        // embed.rs 的装配函数，本文件只负责状态壳与 LLM 档。
        let code = crate::ipc::source_without_tests(include_str!("providers.rs"));
        assert!(
            !code.contains("EmbedHttpProvider::new") && !code.contains("CloudEmbedProvider::new"),
            "providers.rs 又开始自己构造嵌入 Provider 了——注册只归装配（embed.rs）"
        );
    }

    /// 服务端装配守护：钉「设置 → 实际打到哪台、`location` 有没有如实标成 Cloud」这一段
    /// （严格本地据此过滤）。不连真网络，只断言装配出来的 Provider 描述符。
    ///
    /// `base_url` 是记住的那台服务器（登录与否由各用例传入的门控结论决定）；空 = 内置官方后端。
    fn state_for_endpoint(config_dir: &std::path::Path, base_url: &str) -> AppState {
        let state = build_state_with_settings_persistence(
            Settings::with_defaults(),
            SettingsPersistence::Writable(config_dir.join("config.toml")),
            luna_vault::UsageStore::open(config_dir.join("usage-device.sqlite")).unwrap(),
            None,
        );
        if !base_url.is_empty() {
            state.set_cloud_server(crate::cloud::ServerRef::of(base_url));
        }
        state
    }

    /// 装配测试用的凭据桩。
    ///
    /// 装配这件事本就不该依赖「此刻有没有会话」——凭据是**执行**时的事，装配只决定
    /// 打哪一台、走哪条协议。故这些用例注入一个恒无凭据的来源，断言的也只是 Provider
    /// 的 id 与 `location`。
    fn stub_bearer() -> Arc<dyn luna_provider::asr_remote::BearerSource> {
        struct NoBearer;
        #[async_trait::async_trait]
        impl luna_provider::asr_remote::BearerSource for NoBearer {
            async fn bearer(&self) -> Option<String> {
                None
            }
            async fn refresh(&self, _rejected: &str) -> Option<String> {
                None
            }
        }
        Arc::new(NoBearer)
    }

    fn default_llm_provider(state: &AppState) -> Arc<dyn luna_core::AIProvider> {
        for id in [
            luna_provider::CLOUD_LLM_PROVIDER_ID,
            luna_provider::LOCAL_LLM_PROVIDER_ID,
        ] {
            if let Some(p) = state.provider_registry.get(&ProviderId::from(id)) {
                return p;
            }
        }
        panic!("默认档 LLM 必须至少注册一个可用候选");
    }

    /// 让引擎清单里有一个装好的通用对话模型（设备策略的本机默认档由它而来，ADR-143）。
    fn with_installed_chat(state: &AppState) {
        state.record_engine_models(vec![crate::local_engine::EngineModelRef {
            id: "qwen3.5-0.8b-q8_0-gguf".into(),
            kind: crate::local_engine::EngineModelKind::Chat,
            digest: None,
            translation_only: false,
            size_bytes: 800,
        }]);
    }

    fn default_llm_location(state: &AppState) -> luna_core::ProviderLocation {
        default_llm_provider(state).descriptor().location
    }

    /// 离线转写选了「服务端转写」→ 必须真的路由到远程 Provider。
    ///
    /// 这是整条链路的关键一环：候选里给得出这个选项、就绪判定说 ready，但解析时
    /// 仍去找本地 whisper 权重的话，用户选了服务端却收到「模型缺失，请去安装」
    /// ——错误信息指向一个与他的选择毫无关系的动作。
    ///
    /// 不连真网络：只断言解析出来的 Provider id。
    #[test]
    fn batch_asr_slot_pointing_at_service_resolves_to_remote_provider() {
        let mut settings = Settings::with_defaults();
        settings.set("provider.remote_asr_enabled", SettingValue::Bool(true));
        settings.set("ai.assignment.batch_asr", crate::ipc::REMOTE_ASR_TARGET);
        assert_eq!(
            crate::ipc::effective_assignment_target(&settings, "batch_asr").as_deref(),
            Some(crate::ipc::REMOTE_ASR_TARGET),
            "槽位没解析成服务端目标——后面的路由无从谈起"
        );
        assert!(crate::ipc::is_remote_asr_target(
            crate::ipc::effective_assignment_target(&settings, "batch_asr")
                .as_deref()
                .unwrap()
        ));
    }

    /// 槽位继承是**单向**的：`batch_asr` 空值时跟随 `realtime_asr`，反过来不成立。
    ///
    /// 这条原本还断言「实时槽位不得是服务端目标」——那个理由（ADR-038 的双向流式通道
    /// 尚未实现）**已经作废**，实时通道 2026-08-08 真机验过。但方向这一半仍然承重：
    /// 只想让**离线**转写上云的人，麦克风不该跟着上去——那是一次没有经过同意的隐私
    /// 变更，而它不会有任何提示。
    #[test]
    fn slot_inheritance_only_flows_from_realtime_to_batch() {
        let mut settings = Settings::with_defaults();
        // 刻意**不开**转写同意：此时实时档的默认是随包的本机兜底模型。
        settings.set("ai.assignment.batch_asr", crate::ipc::REMOTE_ASR_TARGET);
        let realtime = crate::ipc::effective_assignment_target(&settings, "realtime_asr");
        assert!(
            realtime
                .as_deref()
                .is_none_or(|t| !crate::ipc::is_remote_asr_target(t)),
            "离线槽位指向服务端把实时槽位也带过去了——麦克风在用户没同意的情况下上了云：{realtime:?}"
        );
    }

    /// 「兜底模型会是哪一个」必须在**装配之前**就答得出来。
    ///
    /// ASR 是懒加载的（首个任务才装配），而设置页在冷启动时就要回答这个问题。此前
    /// `compute_status` 问的是已装的 Provider，于是冷启动时恒得 `None` → 退到随包 tiny，
    /// 把用户配的 Whisper Small 说成「Whisper Tiny」，并误报「你只装了最小档」；
    /// 录一次音之后说法又变——同一个问题两个时刻两个答案。
    ///
    /// 这条钉住的是那个性质本身：**没有装配过任何 Provider** 的全新 state，也必须解析出
    /// 设置里那一档。
    #[tokio::test]
    async fn the_fallback_model_resolves_before_anything_is_assembled() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("models");
        // 造一份「用户配了 base 档」的现场：权重文件存在，但一个 Provider 都没装。
        let id = crate::model_store::WHISPER_BASE_REMOTE_ID;
        let spec = crate::model_store::remote_model_spec(id).expect("目录里应有 base 档");
        let weight = crate::model_store::spec_weight_path(&root, spec);
        std::fs::create_dir_all(weight.parent().unwrap()).unwrap();
        std::fs::write(&weight, b"stub").unwrap();

        let mut settings = Settings::with_defaults();
        settings.set("ai.assignment.realtime_asr", id);
        let state = state_for_endpoint(dir.path(), "");
        assert!(
            state.asr_provider.lock().unwrap().is_none(),
            "测试前提：这份 state 还没有装配过 ASR"
        );

        let shape = state.desired_stream_asr_shape(&settings, &root);
        assert!(!shape.to_service);
        assert_eq!(
            shape.local.as_deref(),
            Some(weight.as_path()),
            "冷启动下解析不出用户配的那一档——界面会把随包 tiny 说成他的兜底模型"
        );
    }

    /// 组织没开放转写（R4b）时，指向服务端的实时槽位**不构造**服务端腿，直接装本机兜底。
    ///
    /// 此前形状只看槽位：服务端腿照装，每次开录先吃一次 403 才回落；离线那条是定向路由，
    /// 只有服务端一个候选，于是重新转写直接报「组织没开放」——而本机明明有权重。
    #[tokio::test]
    async fn a_transcription_the_server_does_not_allow_runs_on_the_local_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("models");
        let bundled = crate::model_store::luna_managed_weight_path(
            &root,
            crate::model_store::BUNDLED_WHISPER_ID,
        );
        std::fs::create_dir_all(bundled.parent().unwrap()).unwrap();
        std::fs::write(&bundled, b"stub").unwrap();

        let mut settings = Settings::with_defaults();
        settings.set("ai.assignment.realtime_asr", crate::ipc::REMOTE_ASR_TARGET);
        let state = state_for_endpoint(dir.path(), "");

        state.set_cloud_capabilities(Some(vec!["chat".to_owned(), "transcribe".to_owned()]));
        let served = state.desired_stream_asr_shape(&settings, &root);
        assert!(served.to_service, "开放了转写就该走服务端");

        state.set_cloud_capabilities(Some(vec!["chat".to_owned()]));
        let refused = state.desired_stream_asr_shape(&settings, &root);
        assert!(
            !refused.to_service,
            "组织没开放转写，却仍构造服务端腿——每次开录先白吃一次 403"
        );
        assert_eq!(
            refused.local.as_deref(),
            Some(bundled.as_path()),
            "没开放时应退到本机兜底那一档（ADR-038 ⑤ 同一份权重）"
        );
    }

    /// 服务端肯跑 = 组织开放 ∩ 部署配好（ADR-041 ①）。部署一个上游都没有时，组织开放了什么
    /// 都不算数：每一项都在本机跑，且原因记在部署方头上而不是组织头上。
    #[tokio::test]
    async fn an_unconfigured_deployment_serves_nothing_whatever_the_account_allows() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_for_endpoint(dir.path(), "");
        let facts =
            |served: &[&str], setup_incomplete: bool| luna_cloud::protocol::DeploymentFacts {
                billing_enabled: false,
                setup_incomplete,
                served: served.iter().map(|c| (*c).to_owned()).collect(),
            };

        // 两样都不知道：按服务端跑得了处理（真被拒时服务端回错，当场回落本机）。
        assert_eq!(state.server_offer("chat"), crate::ipc::ServerOffer::Serves);
        assert_eq!(state.served_capabilities(), None);

        state.set_cloud_capabilities(Some(vec!["chat".to_owned(), "transcribe".to_owned()]));
        state.set_cloud_deployment(Some(facts(&[], true)));
        assert!(state.server_setup_incomplete());
        for capability in ["chat", "embed", "transcribe"] {
            assert_eq!(
                state.server_offer(capability),
                crate::ipc::ServerOffer::NotConfigured,
                "{capability}：部署没配上游，组织开放了也不算"
            );
        }
        assert_eq!(state.served_capabilities(), Some(Vec::new()));

        // 配了一部分：取交集，没配的记在部署方头上、没开放的记在组织头上。
        state.set_cloud_deployment(Some(facts(&["chat", "embed"], false)));
        assert!(!state.server_setup_incomplete());
        assert_eq!(state.server_offer("chat"), crate::ipc::ServerOffer::Serves);
        assert_eq!(
            state.server_offer("embed"),
            crate::ipc::ServerOffer::NotAllowed
        );
        assert_eq!(
            state.server_offer("transcribe"),
            crate::ipc::ServerOffer::NotConfigured
        );
        assert_eq!(state.served_capabilities(), Some(vec!["chat".to_owned()]));
    }

    #[tokio::test]
    async fn empty_endpoint_keeps_default_llm_local() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_for_endpoint(dir.path(), "");
        with_installed_chat(&state);
        state.register_llm_provider(CloudAvailability::SignedOut, stub_bearer());
        assert_eq!(
            default_llm_location(&state),
            luna_core::ProviderLocation::LocalService,
            "没配端点时必须是本机——这是绝大多数用户的路径，也是「默认不联网」的兑现处"
        );
    }

    /// **构造 state 不得注册任何 LLM 档**——冷启动那个 bug 的根，就在这一条上。
    ///
    /// 构造期（`build_state_with_settings_persistence`）拿不到 `AppHandle`，因此解析不出
    /// 端点。它此前仍然无条件注册了一个本机默认档，于是配了组织服务端或登录了
    /// Lunarca 云的用户**每次重启后对话与翻译都打回本机**，而设置页同时写着「由服务端
    /// 执行」——改任意一个云设置才会「莫名其妙好了」，这类 bug 用户报告不出来。
    ///
    /// 断言写成「构造后注册表里没有这两个 id」而不是「源码里没有那一行」：后者挡不住
    /// 有人换个写法把它加回来。
    ///
    /// 实时档一并断言：注册顺序即兜底顺序（`Router::resolve`），实时档若在构造期抢到
    /// 首位，每一条没有 `preferred_provider` 的对话/翻译/摘要都会落到那个「小模型换低
    /// 延迟」的档上——而它的存在理由恰恰是只服务实时字幕。
    #[tokio::test]
    async fn construction_registers_no_llm_at_all() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = Settings::with_defaults();
        // 刻意配一个**与默认档不同**的实时档：这是实时档会被注册的唯一条件。
        settings.set("ai.assignment.realtime_llm", "qwen3.5-0.8b-q8_0-gguf");
        let state = build_state_with_settings_persistence(
            settings,
            SettingsPersistence::Writable(dir.path().join("config.toml")),
            luna_vault::UsageStore::open(dir.path().join("usage-device.sqlite")).unwrap(),
            None,
        );
        for id in [
            luna_provider::LOCAL_LLM_PROVIDER_ID,
            luna_provider::LOCAL_LLM_REALTIME_PROVIDER_ID,
        ] {
            assert!(
                state.provider_registry.get(&ProviderId::from(id)).is_none(),
                "构造期注册了 `{id}`——它绕过了端点解析（默认档）或抢走了兜底首位（实时档）。\
                 两档都必须由 `cloud::reconfigure_cloud_stack` 在启动时按「默认 → 实时」装上"
            );
        }
    }

    /// 槽位指向随包引擎里的对话条目（裸 id）→ 装随包引擎 Provider；引擎清单里没有它就
    /// **什么都不装**（如实以「没有可执行 Provider」失败，不臆造一个目标）。
    ///
    /// 同时钉住：① 引擎清单里没有、或登记为嵌入模型的裸 id 不算对话目标；② 实时档换形态时
    /// 旧形态的实时 Provider 下线，注册表里不留没有槽位指向的实时档。
    #[tokio::test]
    async fn a_bare_id_of_an_engine_chat_entry_assembles_the_bundled_engine() {
        use crate::local_engine::{EngineModelKind, EngineModelRef};
        let dir = tempfile::tempdir().unwrap();
        let mut settings = Settings::with_defaults();
        settings.set("ai.assignment.default_llm", "qwen3.5-2b-q8_0-gguf");
        settings.set("ai.assignment.realtime_llm", "hy-mt2-1.8b-q8_0-gguf");
        let state = build_state_with_settings_persistence(
            settings,
            SettingsPersistence::Writable(dir.path().join("config.toml")),
            luna_vault::UsageStore::open(dir.path().join("usage-device.sqlite")).unwrap(),
            None,
        );
        let assemble = |state: &AppState| {
            state.register_llm_provider(CloudAvailability::SignedOut, stub_bearer());
            state.refresh_realtime_llm_provider(CloudAvailability::SignedOut);
        };
        let registered = |state: &AppState, id: &str| {
            state.provider_registry.get(&ProviderId::from(id)).is_some()
        };

        // 引擎还没登记任何条目：裸 id 不认识，不装。
        assemble(&state);
        assert!(!registered(&state, luna_provider::LOCAL_LLM_PROVIDER_ID));

        assert!(state.record_engine_models(vec![
            EngineModelRef {
                id: "qwen3.5-2b-q8_0-gguf".into(),
                kind: EngineModelKind::Chat,
                digest: None,
                translation_only: false,
                size_bytes: 0,
            },
            EngineModelRef {
                id: "hy-mt2-1.8b-q8_0-gguf".into(),
                kind: EngineModelKind::Chat,
                digest: None,
                translation_only: false,
                size_bytes: 0,
            },
            EngineModelRef {
                id: "bge-m3-q8_0-gguf".into(),
                kind: EngineModelKind::Embed,
                digest: None,
                translation_only: false,
                size_bytes: 0,
            },
        ]));
        assert!(
            !state.record_engine_models(state.engine_models_snapshot()),
            "清单没变不报变化"
        );
        assemble(&state);
        let default = default_llm_provider(&state);
        assert_eq!(
            default.id(),
            ProviderId::from(luna_provider::LOCAL_LLM_PROVIDER_ID)
        );
        assert_eq!(
            default.descriptor().model.as_deref(),
            Some("qwen3.5-2b-q8_0-gguf"),
            "请求里的 model 必须是 preset 节名（条目 id）"
        );
        assert_eq!(
            default.descriptor().location,
            luna_core::ProviderLocation::LocalService
        );
        assert!(registered(
            &state,
            luna_provider::LOCAL_LLM_REALTIME_PROVIDER_ID
        ));

        // 嵌入条目不是对话目标。
        state
            .settings
            .lock()
            .unwrap()
            .set("ai.assignment.default_llm", "bge-m3-q8_0-gguf");
        assemble(&state);
        assert!(!registered(&state, luna_provider::LOCAL_LLM_PROVIDER_ID));

        // 实时档指向一个引擎里没有的裸 id（例如迁移不掉的遗留值）：实时档下线，不留一个
        // 没有槽位指向的定向 Provider。
        state
            .settings
            .lock()
            .unwrap()
            .set("ai.assignment.realtime_llm", "not-in-engine-gguf");
        assemble(&state);
        assert!(!registered(
            &state,
            luna_provider::LOCAL_LLM_REALTIME_PROVIDER_ID
        ));
    }

    /// **Lunarca 服务端走原生推理面**：走错的话能力全被记成 `chat`（按能力分维的计量与
    /// 配额因此全错）、执行者不可见、取消只断本地连接。
    ///
    /// 断言 Provider 的 **id** 而不是"源码里有没有那个类型名"：后者挡不住把分支写反。
    #[tokio::test]
    async fn a_lunarca_server_gets_the_native_provider() {
        let dir = tempfile::tempdir().unwrap();

        // 已登录组织服务器。
        let state = build_state_with_settings_persistence(
            Settings::with_defaults(),
            SettingsPersistence::Writable(dir.path().join("config.toml")),
            luna_vault::UsageStore::open(dir.path().join("usage-device.sqlite")).unwrap(),
            None,
        );
        state.set_cloud_server(crate::cloud::ServerRef::of("http://10.0.0.62:8090"));
        state.register_llm_provider(CloudAvailability::Allowed, stub_bearer());
        assert_eq!(
            default_llm_provider(&state).id(),
            ProviderId::from(luna_provider::CLOUD_LLM_PROVIDER_ID),
            "连的是 Lunarca 服务端却没走原生推理面——能力会被全部记成 chat，\
             执行者不可见，取消也只断本地连接"
        );
    }

    /// **服务端主路由与本地兜底必须同时存在**，严格本地则只保留本机。
    ///
    /// Router 按落点确定主次，不再把注册顺序当产品语义；只有服务端不可达或超时才会进入
    /// 本地候选。严格本地会移除全部云端候选，避免任何外发可能。
    #[tokio::test]
    async fn cloud_primary_and_local_fallback_coexist_until_strict_local_is_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let state = build_state_with_settings_persistence(
            Settings::with_defaults(),
            SettingsPersistence::Writable(dir.path().join("config.toml")),
            luna_vault::UsageStore::open(dir.path().join("usage-device.sqlite")).unwrap(),
            None,
        );
        state.set_cloud_server(crate::cloud::ServerRef::of("http://10.0.0.62:8090"));
        with_installed_chat(&state);

        let registered = || {
            [
                luna_provider::LOCAL_LLM_PROVIDER_ID,
                luna_provider::CLOUD_LLM_PROVIDER_ID,
            ]
            .into_iter()
            .filter(|id| {
                state
                    .provider_registry
                    .get(&ProviderId::from(*id))
                    .is_some()
            })
            .collect::<Vec<_>>()
        };

        // 服务端 → 云主路由与本地兜底同时存在，解析出的默认执行位置仍必须是云端。
        state.register_llm_provider(CloudAvailability::Allowed, stub_bearer());
        assert_eq!(
            registered(),
            vec![
                luna_provider::LOCAL_LLM_PROVIDER_ID,
                luna_provider::CLOUD_LLM_PROVIDER_ID,
            ]
        );
        assert_eq!(
            default_llm_location(&state),
            luna_core::ProviderLocation::Cloud
        );

        // 翻到严格本地 → 只剩本机那一个。云候选必须消失，确保不会外发。
        state.register_llm_provider(CloudAvailability::BlockedByStrictLocal, stub_bearer());
        assert_eq!(
            registered(),
            vec![luna_provider::LOCAL_LLM_PROVIDER_ID],
            "严格本地仍保留云端候选——任务存在外发风险"
        );
    }

    /// **登录 Lunarca 云 —— 对话必须真的上云**，落点展示与字节去向同源。
    #[tokio::test]
    async fn logging_into_the_official_cloud_actually_moves_chat_off_device() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_for_endpoint(dir.path(), "");
        state.register_llm_provider(CloudAvailability::Allowed, stub_bearer());
        assert_eq!(
            default_llm_location(&state),
            luna_core::ProviderLocation::Cloud,
            "登录官方云后对话仍在本机跑，而卡片写着「AI 由 Lunarca 云提供」"
        );
        // 落点展示与字节去向同源——两者共读一个 `availability` 与同一份端点解析。
        assert_eq!(
            crate::cloud::compute_location(
                CloudAvailability::Allowed,
                &crate::ipc::compute_endpoint_source(
                    CloudAvailability::Allowed,
                    &state.cloud_server()
                ),
            ),
            crate::cloud::ComputeLocation::OfficialCloud,
            "落点不是官方云——用户选的就是它"
        );
    }

    /// **记住了服务器、但没登录 = 全在本机**（ADR-155 取消「有地址没登录」）。
    ///
    /// 落点与字节去向同源：Provider 只装本机，界面也只能说「在这台设备上」。
    #[tokio::test]
    async fn a_remembered_server_without_a_session_stays_on_device() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_for_endpoint(dir.path(), "http://10.0.0.62:8090");
        with_installed_chat(&state);
        state.register_llm_provider(CloudAvailability::SignedOut, stub_bearer());
        assert_eq!(
            default_llm_location(&state),
            luna_core::ProviderLocation::LocalService,
            "没登录却装了远端 Provider——正文会离开这台设备"
        );
        let location = crate::cloud::compute_location(
            CloudAvailability::SignedOut,
            &crate::ipc::compute_endpoint_source(
                CloudAvailability::SignedOut,
                &state.cloud_server(),
            ),
        );
        assert_eq!(location, crate::cloud::ComputeLocation::OnDevice);
    }

    /// **原生服务端不接收客户端具体模型**（TDD-14 §3.3.1 的行为契约）。
    ///
    /// 原生服务端省略具体模型，由服务端路由；实际模型只从 `executed_by` 读取。
    /// 显式本机覆盖另由 `explicit_device_assignment_stays_on_device_when_server_is_connected`
    /// 断言，避免构造“函数参数与持久化设置互相矛盾”的无效状态。
    #[tokio::test]
    async fn native_service_omits_client_model_selection() {
        let dir = tempfile::tempdir().unwrap();
        let state = build_state_with_settings_persistence(
            Settings::with_defaults(),
            SettingsPersistence::Writable(dir.path().join("config.toml")),
            luna_vault::UsageStore::open(dir.path().join("usage-device.sqlite")).unwrap(),
            None,
        );
        state.set_cloud_server(crate::cloud::ServerRef::of("http://10.0.0.62:8090"));
        let native = CloudAvailability::Allowed;

        state.register_llm_provider(native, stub_bearer());
        assert_eq!(
            default_llm_provider(&state).id(),
            ProviderId::from(luna_provider::CLOUD_LLM_PROVIDER_ID)
        );
        assert_eq!(
            default_llm_provider(&state).descriptor().model,
            None,
            "原生服务端必须托管模型选择，客户端不得把具体模型固化进请求"
        );
    }

    /// 连接服务端不等于覆盖用户的显式本机选择。
    #[tokio::test]
    async fn explicit_device_assignment_stays_on_device_when_server_is_connected() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = Settings::with_defaults();
        settings.set("ai.assignment.default_llm", "qwen3.5-0.8b-q8_0-gguf");
        let state = build_state_with_settings_persistence(
            settings,
            SettingsPersistence::Writable(dir.path().join("config.toml")),
            luna_vault::UsageStore::open(dir.path().join("usage-device.sqlite")).unwrap(),
            None,
        );
        state.set_cloud_server(crate::cloud::ServerRef::of("http://10.0.0.62:8090"));
        with_installed_chat(&state);
        state.register_llm_provider(CloudAvailability::Allowed, stub_bearer());
        assert_eq!(
            default_llm_location(&state),
            luna_core::ProviderLocation::LocalService,
            "用户明确选了本机模型，连接服务端不能把执行位置改写成云端"
        );
    }

    /// 启动装载必须把当前库登记进设备级清单（PRD-06-049）。
    ///
    /// 打开库有两条路径——启动装载（本函数）与 `bind_vault`（引导/切库）——语义必须一致：
    /// 只在 bind 里登记的话，启动即带库的既有用户永远不在清单里，切换器按"空清单"
    /// 整个不渲染，多库入口无从谈起（真机实测 `vaults.json` 不存在即此缺陷）。
    #[tokio::test]
    async fn startup_open_registers_current_vault_in_registry() {
        let config_dir = tempfile::tempdir().unwrap();
        let vault_dir = tempfile::tempdir().unwrap();
        let mut settings = Settings::with_defaults();
        settings.set(
            "storage.vault_root",
            vault_dir.path().to_string_lossy().as_ref(),
        );
        let state = build_state_with_settings_persistence(
            settings,
            SettingsPersistence::Writable(config_dir.path().join("config.toml")),
            luna_vault::UsageStore::open(config_dir.path().join("usage-device.sqlite")).unwrap(),
            None,
        );
        let entries = state.vault_registry.list();
        assert!(
            entries
                .iter()
                .any(|e| e.path == std::fs::canonicalize(vault_dir.path()).unwrap()),
            "启动装载的库必须出现在设备级清单里（切换器的数据源）"
        );
    }

    /// 命令按窗口身份路由落点库（PRD-06-050）：副库窗口读写它自己的库，
    /// 其余窗口恒落主库——路由错一格就是跨库读写。
    #[tokio::test]
    async fn with_vault_routes_by_window_identity() {
        let config_dir = tempfile::tempdir().unwrap();
        let main_dir = tempfile::tempdir().unwrap();
        let side_dir = tempfile::tempdir().unwrap();
        let mut settings = Settings::with_defaults();
        settings.set(
            "storage.vault_root",
            main_dir.path().to_string_lossy().as_ref(),
        );
        let state = build_state_with_settings_persistence(
            settings,
            SettingsPersistence::Writable(config_dir.path().join("config.toml")),
            luna_vault::UsageStore::open(config_dir.path().join("usage-device.sqlite")).unwrap(),
            None,
        );
        let side = luna_vault::Vault::open(side_dir.path()).unwrap();
        state
            .secondary_vaults
            .lock()
            .unwrap()
            .insert("vault-window-0".into(), Arc::new(Mutex::new(side)));

        let main_root = state
            .with_vault("main", |v| Ok(v.root().to_path_buf()))
            .unwrap();
        assert_eq!(main_root, std::fs::canonicalize(main_dir.path()).unwrap());
        let side_root = state
            .with_vault("vault-window-0", |v| Ok(v.root().to_path_buf()))
            .unwrap();
        assert_eq!(side_root, std::fs::canonicalize(side_dir.path()).unwrap());
        // note-editor 弹出窗恒落主库（它是主库的一窗一笔记）。
        let editor_root = state
            .with_vault("note-editor-3", |v| Ok(v.root().to_path_buf()))
            .unwrap();
        assert_eq!(editor_root, std::fs::canonicalize(main_dir.path()).unwrap());
        // 已销毁的副库窗口：诚实报失效，绝不静默回落主库（那是跨库读写）。
        assert!(state
            .with_vault("vault-window-99", |v| Ok(v.root().to_path_buf()))
            .is_err());
    }
}
