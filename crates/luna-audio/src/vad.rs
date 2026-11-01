//! 语音活动检测（VAD）抽象。事实源：TDD-02 §3.2、ADR-012。
//!
//! **职责单一**：VAD 只产出逐帧语音/非语音标签，**不做切分决策**（切分归
//! [`crate::endpointer`] 的迟滞状态机，§3.2 末）。
//!
//! MVP 选型偏离说明（经产品拍板，需补一行 ADR）：ADR-012 以 **Silero（ort）为默认**；
//! 但本阶段已背 whisper-rs 构建/CoreML 公证风险，**不在同一阶段叠 ort 受管二进制
//! 签名/分发 + Silero 模型 License/分发的第二个公证未知数**。push-to-talk 下用户手动
//! 控制起止，端点边界精度价值下降，能量门控 + whisper 自身 `no_speech_prob` 已能兜住
//! P0 幻觉抑制（PRD-02-007）。故 **MVP 用 [`EnergyVad`]**，Silero 经本 trait 后置可换
//! （`SileroVad` 增量落在 Phase 4 尾/Phase 7，验证 ort 打包 ROI 后接入）。
//!
//! # 为什么门限必须相对噪声底，而不是一个绝对数
//!
//! 同一台 Mac、同一个人、同一支麦克风、同一个音量，各跑 30 秒实测（`InputLevels`
//! 诊断，5 秒窗，单位 dBFS）：
//!
//! | 路径 | 噪声底 p10 | 噪声底 min | 语音 p50 | 语音 p90 | 可用 SNR |
//! |---|---|---|---|---|---|
//! | cpal（仅麦克风，无语音处理） | −40.7 ~ −44.4 | −44.8 ~ −45.9 | −19 ~ −26 | −5.9 ~ −12.3 | 31–35 dB |
//! | VPIO（Both 档，系统级 AEC+NS，AGC 关） | −59 ~ −97 | −102 ~ −120 | −40 ~ −57.9 | −16.7 ~ −45.6 | 26–54 dB |
//!
//! 两条路径的噪声底相差最多 **75 dB**，语音电平相差最多 **32 dB**——任何一个固定数字
//! 都只能对其中一条成立。旧的 `DEFAULT_THRESHOLD_RMS = 0.006`（= **−44.4 dBFS**）对
//! 两条都错，且**方向相反**：
//! - VPIO 上它**切穿语音分布**（语音 p50 低到 −57.9）→ 说着话一路判静音，现场表现为
//!   「录到一半不出字」；
//! - cpal 上它**低于噪声底**（底噪 min 才 −44.8）→ 闸门全开，连底噪都判语音，
//!   `PRD-02-007` 的幻觉护栏在这条路径上**从未真正生效过**。
//!
//! 故门限改为 **噪声底 + 余量**，噪声底由滑动最小值（minimum statistics）逐轨在线估计。

/// 逐帧语音标签。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VadLabel {
    /// 语音。
    Speech,
    /// 静音/非语音。
    Silence,
}

impl VadLabel {
    /// 是否判为语音。
    #[must_use]
    pub fn is_speech(self) -> bool {
        matches!(self, Self::Speech)
    }
}

/// VAD 定长帧（20ms @16k = 320 样本）。
///
/// **本仓唯一的帧长定义**：管线（[`crate::pipeline`]）按它把消费线程的变长帧重切定长，
/// 噪声底估计的时间常数（[`NOISE_SUBWINDOW_FRAMES`]/[`NOISE_SUBWINDOWS`]）也全部以它
/// 换算成秒。两处各写一个 320 的下场是：改了帧长以后，时间常数会静默地变成另一个值，
/// 而所有单测照样全绿（它们也按各自那份 320 造数据）。
///
/// 取 20ms 的理由：能量 VAD 对帧长不敏感，但定长便于迟滞计时一致，也为 Silero
/// （严格 512@16k，§3.2 硬约束 1）后置替换留好"定长重切"位置。
pub const VAD_FRAME_SAMPLES: usize = 320;

/// 噪声底估计的**子窗长**（帧数）。25 帧 × 20ms = **0.5s**。
///
/// 子窗是「取最小值」的粒度：太长则底跟不上环境变化，太短则每个子窗的最小值都被单帧
/// 噪声抖动主导。0.5s 下每个子窗压着 25 个样本，个别毛刺拉不动它。
pub const NOISE_SUBWINDOW_FRAMES: usize = 25;

/// 噪声底估计的**子窗个数**（环形）。6 × 0.5s = 窗长 **W = 3s**。
///
/// W 的两条约束同时成立，缺一不可：
/// ① **W 是噪声阶跃后误报时长的上界**——环境突然变吵（空调启动、换到嘈杂房间），底要
///    W 秒才追上去，这段时间内底噪会被判成语音。3s 是"用户还没来得及觉得不对"的量级；
/// ② **W 必须大于连续浊音无间隙的经验上限（< 2s）**——真实语音必有音节/词间隙，只要
///    间隙必然落进窗内，底就恒等于间隙电平、不会被语音自己抬上去。W 小于这个上限时，
///    说得久一点就会把自己的门限顶穿（说着说着突然判不出语音）。
///
/// ⚠ **W 是上界，保证保留时长是 W − 0.5s = 2.5s**：环形里有一格是"正在累积"的当前子窗，
/// 于是窗实际覆盖 2.5 ~ 3.0s（随子窗相位）。两条约束都按下界 2.5s 仍然成立，写在这里是
/// 因为单测断言必须按 2.5s 写——按 3.0s 写会在子窗边界对齐时偶发变红。
pub const NOISE_SUBWINDOWS: usize = 6;

/// 噪声底的**下夹取**（dBFS）。
///
/// VPIO（系统级噪声抑制）在停顿期直接输出数字静默级的残噪，实测 min 落到 **−102 ~ −120**；
/// 不夹取的话门限会被拉到 −90 以下，等于没有门限（连 NS 的残渣都判语音，幻觉护栏失效）。
///
/// 取 −75 的推导：normal 档门限 = −75 + 10 = **−65**，
/// - 比实测**最差**语音中位数（VPIO 的 −57.9）还低 **7 dB** 余量 → 最差路径也不切穿语音；
/// - 比 VPIO 残噪上沿（≤ −97）高 **32 dB** → 残噪照样挡在门外。
pub const F_FLOOR_MIN: f32 = -75.0;

/// 噪声底的**上夹取**（dBFS）。
///
/// 实测最高底噪是 cpal 的 −40.7，取 −30 再留一档余量。
///
/// 上夹取不是防御性冗余，它决定了**持续大信号时的行为**：没有它，一段持续 3s 以上的
/// 大音量输入会把底一路抬到自己头上，于是门限追着信号跑、永远判不出语音（对着麦克风
/// 持续朗读会在 3 秒后突然"失聪"）。夹到 −30 后，任何 ≥ −20 dBFS（normal 档）的持续
/// 信号恒判语音——这也正是 [`crate::types::CaptureDiagnostic::VadSaturated`] 要看见的
/// 那个状态。
pub const F_FLOOR_MAX: f32 = -30.0;

/// 噪声底的**冷启动初值**（dBFS）——会话开头、窗内还没有观测时的假定底噪。
///
/// 取两条路径中**更高**的那个实测底噪（cpal 的 min −44.8 ~ −45.9）。
///
/// 冷启动的误差**不对称**，所以宁高勿低：
/// - 偏高 = 首个停顿之前可能漏检（VPIO 一停顿就落到 −100 以下，底立刻跟上，代价是亚秒级）；
/// - 偏低 = 会话开头把 cpal 的 −41 ~ −45 底噪**整片**判成语音，直接违反 PRD-02-007
///   （静音段绝不喂 whisper），而这一段恰好是幻觉最容易冒出来的地方。
///
/// 实现上它是**上界而非固定值**：环形窗的每个槽位初值都是它，于是开头 W 秒内底最高
/// 只能是 −45（真实底更低时第一帧就压下去），W 秒后初值全部滚出、只剩实测值。
pub const F_INIT: f32 = -45.0;

/// VAD 灵敏度档位——用户可感的唯一 VAD 旋钮，值域即「门限比噪声底高多少 dB」。
///
/// **不暴露绝对 dB 门限**：绝对门限就是本次要删掉的那个东西，它对不同采集路径的
/// 含义完全不同（见模块文档的实测表），给用户调等于让用户去猜自己走的是哪条路径。
///
/// 三个档位名与设置项 `recording.vad_sensitivity`（`low`/`normal`/`high`，默认 `normal`）
/// **逐字对应**，对应关系的唯一实现是 [`Self::from_setting`]。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum VadSensitivity {
    /// 低灵敏（余量 15 dB）：吵环境/远场，宁可漏检也不误收环境声。
    Low,
    /// 默认（余量 10 dB）。
    #[default]
    Normal,
    /// 高灵敏（余量 6 dB）：安静环境/小声说话，宁可多收一点也不漏字。
    High,
}

impl VadSensitivity {
    /// 门限相对噪声底的余量（dB）。
    ///
    /// 三个数都由实测表推出来，且**两条路径必须同时成立**（normal 档）：
    /// - cpal：底 −41 ~ −44 → 门限 −31 ~ −34。语音 p50 −19 ~ −26 稳过；底噪被挡在门外
    ///   2 ~ 9 dB（旧的固定门限在这里是**低于**底噪的，等于闸门全开）；
    /// - VPIO：底 −59 ~ −75（夹取后）→ 门限 −49 ~ −65。语音 p50 −40 ~ −57.9 稳过。
    ///
    /// 6–15 dB 也正是能量 VAD 文献里常用的余量区间：低于 ~6 dB 时噪声自身的帧间抖动
    /// （高斯噪声 320 点帧的 RMS 标准差约 0.35 dB，尾部可到 1 dB 以上）会频繁翻门限；
    /// 高于 ~15 dB 时低 SNR 路径（实测最低 26 dB）的余量被吃掉一大半。
    #[must_use]
    pub fn margin_db(self) -> f32 {
        match self {
            Self::Low => 15.0,
            Self::Normal => 10.0,
            Self::High => 6.0,
        }
    }

    /// 设置项 `recording.vad_sensitivity` 的字面量 → 档位。**无法识别的值（含空串、
    /// 大小写不符、注册表未来改名后残留在用户配置里的旧值）一律回落 [`Self::Normal`]。**
    ///
    /// # 为什么这个映射必须住在 `luna-audio` 里
    ///
    /// 「设置写的是哪几个字符串」与「枚举有哪几个变体」是**同一个事实的两面**，写成两处
    /// 就会漂移：注册表加一档、这边没跟上，表现是设置页多了一个选项而调它没有任何反应——
    /// 正是 `recording.vad_sensitivity` 此前那颗死旋钮的形态。放在档位定义的旁边，加变体
    /// 的人一眼就能看见要同时加字面量，且 `unknown_setting_values_fall_back_to_normal`
    /// 会在漏改时变红。`src-tauri` 侧因此**只传字符串、不做任何字符串比较**。
    ///
    /// 回落而不是报错：设置值来自用户磁盘上的 JSON，坏值不该让一次录音起不来——
    /// 静默用默认档继续录，比"因为一个旋钮拼错了而拒绝采集"正确得多。
    #[must_use]
    pub fn from_setting(value: &str) -> Self {
        match value {
            "low" => Self::Low,
            "high" => Self::High,
            // "normal" 与其它一切（缺失/拼错/旧值）同路：默认档。
            _ => Self::Normal,
        }
    }
}

/// 一帧判定的**全部依据**——现场要能一眼看到「当时的底与门限是多少」。
///
/// 存在的理由是诚实：`Speech`/`Silence` 这一个比特无法区分「环境真的安静」与
/// 「门限画错了」，而这两者的下一步动作完全相反。本仓已经为此付过一次代价
/// （2026-08-18 现场：Both 档麦克风轨反复判静音而用户一直在说话，日志里只有标签，
/// 判不出是 VPIO 消掉了信号还是门限太高）。
///
/// 如实按**现有实现**枚举：目前只有能量 VAD 一种。Silero 落地时加 `Model` arm
/// （携带模型概率），管线经 [`Self::rms_dbfs`] 取值、**不需要改动**。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VadDecision {
    /// 能量 VAD（[`EnergyVad`]）的判定依据。
    Energy {
        /// 本帧 RMS（dBFS）。
        rms_dbfs: f32,
        /// 当前估计的噪声底（dBFS，已夹取到 `[F_FLOOR_MIN, F_FLOOR_MAX]`）。
        floor_dbfs: f32,
        /// 本帧实际使用的门限（dBFS，= 底 + 余量）。
        threshold_dbfs: f32,
    },
    /// 神经 VAD（Silero，ADR-012）的判定依据：只有「是否语音」与本帧电平。
    Neural {
        /// 本帧判定。
        speech: bool,
        /// 本帧 RMS（dBFS）。
        rms_dbfs: f32,
    },
}

impl VadDecision {
    /// 本帧 RMS（dBFS）。
    ///
    /// **不是便利方法，是去重点**：管线的电平统计（`InputLevels`）与 VAD 判定用的
    /// 是同一个帧 RMS，各算一次就是每帧多跑 320 次乘加。所有 arm 都必须能给出这个值。
    #[must_use]
    pub fn rms_dbfs(self) -> f32 {
        match self {
            Self::Energy { rms_dbfs, .. } | Self::Neural { rms_dbfs, .. } => rms_dbfs,
        }
    }
}

/// VAD 统一接口。**有状态实现（如 Silero RNN、本 crate 的噪声底跟踪）须在每个采集会话
/// 开始 `reset()`**（清隐藏状态，否则上个会话状态污染本次开头判定，§3.2 硬约束 2）。
pub trait Vad: Send {
    /// 对一帧单声道 16k f32 评分，返回语音/非语音标签。
    fn score(&mut self, frame: &[f32]) -> VadLabel;
    /// 重置内部状态（会话开始调用）。无状态实现为 no-op。
    fn reset(&mut self);
    /// 最近一次 [`Self::score`] 的判定依据（未评分过时给出初始状态）。
    fn last_decision(&self) -> VadDecision;
}

/// 能量 VAD（MVP）：帧 RMS 与**相对本轨噪声底的自适应门限**比较。零模型依赖、极低延迟、
/// 确定性可单测。
///
/// # 算法（滑动最小值 / minimum statistics）
///
/// 1. 每帧算 RMS → 转 dBFS，**全程在 dB 域跟踪**——时间常数与余量才有可验证的量纲
///    （线性域里"低 10 dB"是个随电平变化的比例，写不出可断言的常量）；
/// 2. 噪声底 = [`NOISE_SUBWINDOWS`] 个子窗**各自最小值**中的最小值，子窗长
///    [`NOISE_SUBWINDOW_FRAMES`]；子窗满即环形滚动，全程零分配；
/// 3. 底夹取到 `[F_FLOOR_MIN, F_FLOOR_MAX]`；
/// 4. 门限 = 底 + [`VadSensitivity::margin_db`]，`rms_dbfs >= 门限` → `Speech`。
///
/// # 刻意**不做**的两件事
///
/// - **不做 Schmitt 迟滞**：时间迟滞（起音确认 / hangover）归 [`crate::endpointer`] 与
///   [`crate::segmenter`]，两处都做 = 同一件事两个时间常数，现场没人算得清最终行为。
/// - **不做「语音帧不抬底」**：那条规则看着很对（说话时别让底跟着涨），但它会在噪声
///   阶跃后**永久锁死**——环境变吵 → 底噪被判成语音 → 语音帧不抬底 → 底永远停在旧值 →
///   永远判语音。这是本设计**上一版被否掉的 blocker**，由
///   `floor_escapes_noise_step` 单测钉住。代价是连续 > W 秒无间隙的大信号会把底抬上来，
///   而 W 正是按"真实语音间隙必然落进窗"选的（见 [`NOISE_SUBWINDOWS`]）。
///
/// 局限：噪声/音乐/呼吸下不如 Silero 精准（§3.2 理由）——故仅作 MVP。
#[derive(Clone, Debug)]
pub struct EnergyVad {
    /// 灵敏度档位（决定门限相对噪声底的余量）。
    sensitivity: VadSensitivity,
    /// 各子窗的最小 dBFS；`sub_mins[cur]` 是**正在累积**的那个。
    ///
    /// 构造时全部置 [`F_INIT`]：于是冷启动的假定底噪会随子窗滚动**逐格退场**，
    /// W 秒后窗内只剩实测值——不需要额外的"是否已预热"标志位。
    sub_mins: [f32; NOISE_SUBWINDOWS],
    /// 当前正在累积的子窗下标（环形）。
    cur: usize,
    /// 当前子窗已累积的帧数（达 [`NOISE_SUBWINDOW_FRAMES`] 即滚动）。
    cur_frames: usize,
    /// 当前噪声底（dBFS，已夹取）。
    floor_dbfs: f32,
    /// 最近一帧的 RMS（dBFS）；未评分过时为 [`DBFS_FLOOR`]。
    last_rms_dbfs: f32,
}

impl EnergyVad {
    /// 按灵敏度档位构造。
    #[must_use]
    pub fn new(sensitivity: VadSensitivity) -> Self {
        Self {
            sensitivity,
            sub_mins: [F_INIT; NOISE_SUBWINDOWS],
            cur: 0,
            cur_frames: 0,
            floor_dbfs: F_INIT,
            last_rms_dbfs: DBFS_FLOOR,
        }
    }

    /// 当前噪声底（dBFS，已夹取）。
    #[must_use]
    pub fn floor_dbfs(&self) -> f32 {
        self.floor_dbfs
    }

    /// 当前门限（dBFS）= 噪声底 + 灵敏度余量。
    #[must_use]
    pub fn threshold_dbfs(&self) -> f32 {
        self.floor_dbfs + self.sensitivity.margin_db()
    }

    /// 把一帧的 dBFS 记入噪声底估计（**仅**满帧且非数字静默的帧走到这里）。
    fn observe(&mut self, rms_dbfs: f32) {
        if self.cur_frames == NOISE_SUBWINDOW_FRAMES {
            // 子窗满 → 环形前进一格，新格清为 +∞（"尚无观测"，不参与取最小值）。
            // 新格绝不清成某个具体 dBFS：那等于每 0.5s 往估计里注入一个凭空的观测。
            self.cur = (self.cur + 1) % NOISE_SUBWINDOWS;
            self.sub_mins[self.cur] = f32::INFINITY;
            self.cur_frames = 0;
        }
        self.sub_mins[self.cur] = self.sub_mins[self.cur].min(rms_dbfs);
        self.cur_frames += 1;
        // 窗内恒有 ≥ 1 个有限值（正在累积的那格刚被 min 过），故不会取到 +∞。
        self.floor_dbfs = self
            .sub_mins
            .iter()
            .copied()
            .fold(f32::INFINITY, f32::min)
            .clamp(F_FLOOR_MIN, F_FLOOR_MAX);
    }
}

impl Default for EnergyVad {
    fn default() -> Self {
        Self::new(VadSensitivity::default())
    }
}

/// dBFS 读数的下封底。
///
/// 两个理由都必须成立才封底：
/// ① `20·log10(0) = -∞`——`-∞` 一旦进日志格式化就是 `-inf`、进 JSON 不合法、参与排序
///    与相减会污染整份分位数统计；
/// ② 底下那一大段量程**没有任何信息**：−120dBFS 已比 16-bit 量化噪声底（≈ −96dBFS）
///    还低 24dB，任何真实采集设备都到不了；去直流高通滤掉常量输入后剩下的数值残渣能
///    算出 −175dBFS 这种读数，它只会让读日志的人以为发现了什么。
///
/// 于是口径统一成一句话：**读到 −120 = 这条轨没有任何有意义的信号**（精确数字零，或
/// 只剩浮点残渣），与 [`crate::types::CaptureDiagnostic::SilentInput`] 的判据同义。
pub const DBFS_FLOOR: f32 = -120.0;

/// 线性幅度（0..1）→ dBFS，下封底到 [`DBFS_FLOOR`]。**这是本仓 dB 换算的唯一实现**。
///
/// # 为什么放在 `vad` 而不是 `level`
///
/// dBFS 在本仓只有一个用途：表达**VAD 判据的量纲**——门限、帧 RMS、噪声底、可用 SNR
/// 都是这条尺子上的点，后续替换 VAD（Silero）时这条尺子不变。`level` 是波形显示的
/// 量化（给 UI 画柱子的 `u8`），职责不同、精度要求也不同；把两者放一起，改显示的人
/// 迟早会顺手改掉判据的量纲。
#[must_use]
pub fn dbfs(linear: f32) -> f32 {
    if linear > 0.0 {
        (20.0 * linear.log10()).max(DBFS_FLOOR)
    } else {
        DBFS_FLOOR
    }
}

/// 计算一帧的 RMS（均方根幅度）。空帧返回 0。
#[must_use]
pub fn frame_rms(frame: &[f32]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = frame.iter().map(|&s| s * s).sum();
    (sum_sq / frame.len() as f32).sqrt()
}

impl Vad for EnergyVad {
    fn score(&mut self, frame: &[f32]) -> VadLabel {
        let rms = frame_rms(frame);
        let rms_dbfs = dbfs(rms);

        // 入窗的两道闸，都是"这一帧能不能代表底噪"的问题，与判定无关（两种情况都照常出标签）：
        //
        // ① **精确零帧不入窗**：数字静默是「无信息」不是「底噪极低」——macOS 未授权麦克风、
        //    虚拟设备无信号、VPIO 的 NS 全抑，输出的都是精确零。放它进窗会把底一路压到
        //    F_FLOOR_MIN，门限跟着塌到 −65，恢复有信号后整段底噪被判成语音。
        //    连带地它也**不推进子窗计数**：一段长数字静默期间估计器整体冻结，恢复时底还是
        //    静默之前那个（而不是被 6 个空子窗冲刷成"无观测"）。
        //
        //    **它是启发式兜底，不是软静音（mute）的实现**，两者别混：
        //    - mute 是**采集层已知的事实**——用户按了静音，`ConsumerPipeline::set_muted`
        //      显式传下来，管线在 muted 期间根本不调 `score()`，VAD 整体冻结；
        //    - 精确零是**来源未知的信号特征**——我们只看见一帧全零，猜不出是权限没给、
        //      设备没信号还是 NS 全抑，只能按"无信息"处理。
        //    保留这道闸是因为上面那三个真实来源都不经过 mute 开关；而 mute 必须显式传递，
        //    因为**它的可观测特征根本挡不住**：置零后归一化的去直流高通还有一小段
        //    非零衰减拖尾（`0.9686^n`，见 `crate::normalizer::DcBlocker`），那段残渣是
        //    "正但极小"的合法读数，会被这道闸放行、把底一路拖到 F_FLOOR_MIN。
        // ② **短于一帧的尾帧只判定、不入窗**：`finish()` 会把不足 320 的残余补喂一次，
        //    它的 RMS 是在更少样本上算的、方差更大，用它更新底等于让会话末尾的估计抖一下——
        //    而那一帧的标签本身还是要出的（否则尾音被整个丢掉）。
        if frame.len() >= VAD_FRAME_SAMPLES && rms > 0.0 {
            self.observe(rms_dbfs);
        }
        self.last_rms_dbfs = rms_dbfs;

        // `>=` 而非 `>`：门限是"够得上就算"，且让 `clamps_and_margin_monotone` 里
        // 恰好等于门限的构造用例有确定答案（浮点相等在这条路径上是可控的：门限由
        // 常量相加得到，不经开方）。
        if rms_dbfs >= self.threshold_dbfs() {
            VadLabel::Speech
        } else {
            VadLabel::Silence
        }
    }

    fn reset(&mut self) {
        // 硬约束 2：清窗 + 底复位到冷启动初值。**不保留上个会话的底**——换设备/换档位
        // （cpal ↔ VPIO）后底会差 30 dB 以上，沿用旧底等于开局就把门限画错。
        *self = Self::new(self.sensitivity);
    }

    fn last_decision(&self) -> VadDecision {
        VadDecision::Energy {
            rms_dbfs: self.last_rms_dbfs,
            floor_dbfs: self.floor_dbfs,
            threshold_dbfs: self.threshold_dbfs(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testsig::{lin, noise, tone, Rng};

    /// 噪声底窗长（帧）——W = 3s（上界）。
    const W_FRAMES: usize = NOISE_SUBWINDOW_FRAMES * NOISE_SUBWINDOWS;
    /// **保证**保留时长（帧）= W − 一个子窗 = 2.5s。环形里恒有一格是正在累积的当前子窗，
    /// 故"旧观测至少还在窗里"只能按这个下界断言（见 [`NOISE_SUBWINDOWS`] 的 ⚠ 注）。
    const RETAINED_FRAMES: usize = W_FRAMES - NOISE_SUBWINDOW_FRAMES;

    /// `n` 帧高斯噪声，目标帧 RMS = `rms_dbfs`。
    fn noise_frames(rng: &mut Rng, rms_dbfs: f32, n: usize) -> Vec<Vec<f32>> {
        (0..n)
            .map(|_| noise(rng, rms_dbfs, VAD_FRAME_SAMPLES))
            .collect()
    }

    /// 一帧 300Hz 正弦，帧 RMS **精确**等于 `rms_dbfs`（相位在帧界自然对齐，故每帧同形）。
    fn tone_frame(rms_dbfs: f32) -> Vec<f32> {
        tone(rms_dbfs, VAD_FRAME_SAMPLES, 0)
    }

    /// `n` 帧同电平正弦。
    fn tone_frames(rms_dbfs: f32, n: usize) -> Vec<Vec<f32>> {
        let f = tone_frame(rms_dbfs);
        (0..n).map(|_| f.clone()).collect()
    }

    /// 逐帧跑一段，返回标签序列。
    fn run(vad: &mut EnergyVad, frames: &[Vec<f32>]) -> Vec<VadLabel> {
        frames.iter().map(|f| vad.score(f)).collect()
    }

    /// 1️⃣ 噪声阶跃后底必须追上去——**上一版设计（「语音帧不抬底」）在此必红**：
    /// 那一版里阶跃后的底噪被判成语音，语音帧不更新底，于是底永远停在 −60，
    /// 门限永远 −50，−40 的底噪被永久判成语音（现场表现：一开空调就一直"在说话"）。
    #[test]
    fn floor_escapes_noise_step() {
        let mut rng = Rng::new(0xA11CE);
        let mut vad = EnergyVad::new(VadSensitivity::Normal);
        let quiet = noise_frames(&mut rng, -60.0, 250); // 5s
        let loud = noise_frames(&mut rng, -40.0, 500); // 10s
        run(&mut vad, &quiet);
        let after = run(&mut vad, &loud);

        // W + 0.1s 之后必须全静音，且**不再翻回**（不是"大部分静音"）。
        let settle = W_FRAMES + 5;
        assert!(
            after[settle..].iter().all(|l| *l == VadLabel::Silence),
            "阶跃后 {settle} 帧仍有语音标签——底没有追上去（底 {:.1} 门限 {:.1}）",
            vad.floor_dbfs(),
            vad.threshold_dbfs()
        );
        assert!(
            vad.floor_dbfs() > -45.0,
            "底应已抬到 −40 量级，实际 {:.1}",
            vad.floor_dbfs()
        );
    }

    /// 2️⃣ VPIO 的 ducking（对端说话时把麦克风轨压下去）松开后不能锁死：
    /// 压低期的极低电平进了窗，松开后底还停在低处 → 误报，但**必须在 W 内自愈**。
    #[test]
    fn vpio_ducking_release_does_not_lock() {
        let mut rng = Rng::new(0xD0CE);
        let mut vad = EnergyVad::new(VadSensitivity::Normal);
        run(&mut vad, &noise_frames(&mut rng, -55.0, 150)); // 3s 常态底
        run(&mut vad, &noise_frames(&mut rng, -70.0, 100)); // 2s ducking
        let back = run(&mut vad, &noise_frames(&mut rng, -55.0, 500)); // 10s 回来

        let speech = back.iter().filter(|l| l.is_speech()).count();
        assert!(
            speech <= W_FRAMES,
            "ducking 松开后误报 {speech} 帧 > W({W_FRAMES})——底被压低值锁住了"
        );
        assert!(
            back[W_FRAMES..].iter().all(|l| *l == VadLabel::Silence),
            "W 之后仍在误报"
        );
    }

    /// 3️⃣ 带间隙的连续语音**不会把自己的底抬上来**——这是 W 取 3s 的直接检验
    /// （真实语音的音节/词间隙必然落进 3s 窗）。
    #[test]
    fn speech_with_gaps_keeps_floor() {
        let mut rng = Rng::new(0x5EED);
        let mut vad = EnergyVad::new(VadSensitivity::Normal);
        // 以**间隙开头**：真实语音必有起音前的沉默；"从语音正中开机"是
        // cold_start_* 两条用例专门钉的场景，这里不重复钉。
        // 0.6s 语音（−35）/ 0.15s 间隙（−60）交替，共 30s。
        let speech_frames = 30usize; // 0.6s
        let gap_frames = 8usize; // 0.15s → 取整到 8 帧（0.16s）：帧长即本仓时间栅格
        let cycles = 30 * 50 / (speech_frames + gap_frames);
        let mut labels = Vec::new();
        let mut speech_idx = Vec::new();
        for _ in 0..cycles {
            let gap = noise_frames(&mut rng, -60.0, gap_frames);
            labels.extend(run(&mut vad, &gap));
            let base = labels.len();
            labels.extend(run(&mut vad, &tone_frames(-35.0, speech_frames)));
            speech_idx.extend(base..base + speech_frames);
        }

        assert!(
            (-63.0..=-57.0).contains(&vad.floor_dbfs()),
            "底应恒在 −60±3，实际 {:.1}",
            vad.floor_dbfs()
        );
        let hit = speech_idx
            .iter()
            .filter(|i| labels[**i].is_speech())
            .count();
        let rate = hit as f32 / speech_idx.len() as f32;
        assert!(rate >= 0.95, "语音帧命中率仅 {:.1}%", rate * 100.0);
    }

    /// 4️⃣ 微弱语音在**一个窗内**不会把门限顶上来。
    ///
    /// **与任务书的偏差，如实记在这里**：任务书要求「−60 底稳态后连续喂 −48 共 4s，
    /// 断言门限不上升」。这在本算法下**不可能成立**，而且不是实现取舍的问题——
    /// 滑动最小值的定义就是"窗内最小值"，任何**恒定电平持续超过 W** 都会把底收敛到
    /// 它自己（−48 如此，−55 也如此）。唯一能让它成立的规则是「语音帧不抬底」，
    /// 而那正是 `floor_escapes_noise_step` 钉住的、被否掉的上一版设计。
    ///
    /// 故本用例钉住**真正可保证的那一半**：保证保留时长（2.5s，见 `RETAINED_FRAMES`）
    /// 之内门限不被这段微弱语音抬起来，它全程判语音；并把之后的必然收敛也断言出来——
    /// 那不是缺陷而是设计代价，代价的可接受性由 W 的取值担保（真实语音的间隙 < 2s，
    /// 必落进窗，见 `speech_with_gaps_keeps_floor`）。
    #[test]
    fn subthreshold_speech_does_not_raise_floor_within_window() {
        let mut rng = Rng::new(0xBEEF);
        let mut vad = EnergyVad::new(VadSensitivity::Normal);
        run(&mut vad, &noise_frames(&mut rng, -60.0, 250)); // 5s 稳态底
        let base_threshold = vad.threshold_dbfs();

        // 保证保留时长内：门限不得升到足以吞掉这段微弱语音（底噪自身的帧间抖动会让门限在
        // ±1dB 内浮动，那不是"被语音抬上来"，故容差按抖动量级给，不按 0）。
        let inside = tone_frames(-48.0, RETAINED_FRAMES);
        for f in &inside {
            let label = vad.score(f);
            assert!(
                vad.threshold_dbfs() < -48.0 && vad.threshold_dbfs() < base_threshold + 1.0,
                "窗内门限被微弱语音抬上来了：{:.2} → {:.2}",
                base_threshold,
                vad.threshold_dbfs()
            );
            assert_eq!(
                label,
                VadLabel::Speech,
                "−48 应稳过门限 {base_threshold:.1}"
            );
        }

        // 补到 4s：底必然收敛到 −48，这是最小值统计的定义性行为。
        run(&mut vad, &tone_frames(-48.0, 200 - RETAINED_FRAMES));
        assert!(
            (vad.floor_dbfs() - (-48.0)).abs() < 0.5,
            "W 之后底应收敛到 −48，实际 {:.1}",
            vad.floor_dbfs()
        );
    }

    /// 5️⃣ 冷启动即大声说话（cpal 语音量级 −25）必须**第 0 帧**就是语音——
    /// 没有"预热窗"这种东西：push-to-talk 下用户按下就说，前几百毫秒丢了就是丢了。
    /// 这正是 F_INIT 取"两条路径中更高的那个实测底噪"的用途：冷启动门限 −35 恰好在
    /// cpal 底噪（−41 ~ −45）之上、语音（p50 −19 ~ −26）之下。
    #[test]
    fn cold_start_loud_speech_at_t0() {
        let mut vad = EnergyVad::new(VadSensitivity::Normal);
        let labels = run(&mut vad, &tone_frames(-25.0, W_FRAMES));
        assert_eq!(labels[0], VadLabel::Speech, "第 0 帧就必须判语音");
        // 整个 W 窗内都不能翻——真实一句话说不到 3 秒不换气，这段必须完整拿到。
        // （W 之后这段**恒定** −25 电平会被底追上并判静音：恒定不变的信号本来就不是
        // 语音，而真实语音有音节间隙把底压住，见 `speech_with_gaps_keeps_floor`。）
        assert!(
            labels.iter().all(|l| l.is_speech()),
            "W 窗内 −25 必须全程判语音"
        );
    }

    /// 6️⃣ 冷启动即**小声**说话（VPIO 量级 −48）会被 F_INIT 的保守初值挡住——
    /// 这是"宁高勿低"明知的代价；但**首个停顿一到，底必须立刻跟上**，
    /// 第二段说话 ≤60ms（3 帧）内恢复。VPIO 的停顿是数字静默级（−100）。
    #[test]
    fn cold_start_quiet_speech_recovers_after_first_gap() {
        let mut vad = EnergyVad::new(VadSensitivity::Normal);
        run(&mut vad, &tone_frames(-48.0, 100)); // 2s 小声说，预期被挡
        run(&mut vad, &tone_frames(-100.0, 25)); // 0.5s VPIO 停顿
        let second = run(&mut vad, &tone_frames(-48.0, 100)); // 再说 2s
        let first_speech = second
            .iter()
            .position(|l| l.is_speech())
            .expect("第二段必须能出语音");
        assert!(
            first_speech <= 3,
            "停顿后第 {first_speech} 帧才恢复（>60ms）——底没有立刻跟上"
        );
    }

    /// 7️⃣ 精确零帧（数字静默 = 无信息）不得进入噪声底窗。
    /// 进了窗的下场：底被压到 F_FLOOR_MIN(−75)，门限塌到 −65，恢复有信号后
    /// 整片底噪被判成语音（PRD-02-007 直接失效）。
    #[test]
    fn digital_silence_frames_do_not_enter_window() {
        let mut rng = Rng::new(0x2E60);
        let mut vad = EnergyVad::new(VadSensitivity::Normal);
        run(&mut vad, &noise_frames(&mut rng, -60.0, 150)); // 3s 正常底噪
        let before = vad.floor_dbfs();

        let zeros: Vec<Vec<f32>> = (0..300).map(|_| vec![0.0f32; VAD_FRAME_SAMPLES]).collect();
        let labels = run(&mut vad, &zeros); // 6s 精确零（> W，足以冲刷整个环形窗）

        assert!(
            labels.iter().all(|l| *l == VadLabel::Silence),
            "精确零当然是静音"
        );
        assert!(
            (vad.floor_dbfs() - before).abs() < 0.01,
            "数字静默期间底必须**冻结**：{before:.1} → {:.1}",
            vad.floor_dbfs()
        );
        assert!(
            vad.floor_dbfs() > F_FLOOR_MIN + 1.0,
            "底被数字静默拖到了下夹取 {:.1}",
            vad.floor_dbfs()
        );
    }

    /// 8️⃣ 短于一帧的尾帧（`finish()` 补喂的残余）只判定、不更新底。
    #[test]
    fn short_frame_scores_but_does_not_update() {
        let mut rng = Rng::new(0x7A11);
        let mut vad = EnergyVad::new(VadSensitivity::Normal);
        run(&mut vad, &noise_frames(&mut rng, -60.0, 150));
        let before = vad.last_decision();

        let tail = vec![0.2f32; 100]; // 100 样本 < 320，且电平很高
        let label = vad.score(&tail);
        let after = vad.last_decision();

        assert_eq!(label, VadLabel::Speech, "尾帧照常出标签");
        let (
            VadDecision::Energy { floor_dbfs: f0, .. },
            VadDecision::Energy { floor_dbfs: f1, .. },
        ) = (before, after)
        else {
            panic!("能量 VAD 只产 Energy 判定");
        };
        assert!(
            (f0 - f1).abs() < f32::EPSILON,
            "尾帧不得改动底：{f0} → {f1}"
        );
    }

    /// 9️⃣ 上下夹取生效；且同一输入下 `Speech` 集合随余量单调：low ⊆ normal ⊆ high。
    #[test]
    fn clamps_and_margin_monotone() {
        // 下夹取：一帧 −95 就足以把底压到 F_FLOOR_MIN（min 是逐帧生效的）。
        let mut low_vad = EnergyVad::new(VadSensitivity::Normal);
        low_vad.score(&tone_frame(-95.0));
        assert!(
            (low_vad.floor_dbfs() - F_FLOOR_MIN).abs() < 1e-3,
            "底应夹到 {F_FLOOR_MIN}，实际 {:.2}",
            low_vad.floor_dbfs()
        );

        // 上夹取：需要 > W 让 F_INIT 的初值全部滚出窗，之后底才由 −5 决定。
        let mut high_vad = EnergyVad::new(VadSensitivity::Normal);
        run(&mut high_vad, &tone_frames(-5.0, W_FRAMES + 25));
        assert!(
            (high_vad.floor_dbfs() - F_FLOOR_MAX).abs() < 1e-3,
            "底应夹到 {F_FLOOR_MAX}，实际 {:.2}",
            high_vad.floor_dbfs()
        );

        // 单调性：底的演化与标签无关（这正是"不做语音帧豁免"的副产品），
        // 故三档跑同一段信号时底逐帧相同，Speech 集合只由余量决定。
        let mut rng = Rng::new(0x1234);
        let mut signal = noise_frames(&mut rng, -60.0, 100);
        signal.extend(tone_frames(-52.0, 50));
        signal.extend(tone_frames(-46.0, 50));
        signal.extend(noise_frames(&mut rng, -60.0, 100));
        let set = |s: VadSensitivity| -> Vec<bool> {
            let mut v = EnergyVad::new(s);
            run(&mut v, &signal).iter().map(|l| l.is_speech()).collect()
        };
        let (lo, no, hi) = (
            set(VadSensitivity::Low),
            set(VadSensitivity::Normal),
            set(VadSensitivity::High),
        );
        for i in 0..signal.len() {
            assert!(!lo[i] || no[i], "帧 {i}: low ⊄ normal");
            assert!(!no[i] || hi[i], "帧 {i}: normal ⊄ high");
        }
        assert!(
            hi.iter().filter(|b| **b).count() > lo.iter().filter(|b| **b).count(),
            "夹具没有落在三档之间，单调断言是空跑"
        );
    }

    /// 🔟 `reset()` 必须等价于新实例（`Vad` trait 硬约束 2）——
    /// 上一会话可能是 cpal（底 −45）、这一会话是 VPIO（底 −100），沿用旧底开局即错。
    #[test]
    fn reset_equals_fresh() {
        let mut rng = Rng::new(0xFEED);
        let mut used = EnergyVad::new(VadSensitivity::Normal);
        run(&mut used, &tone_frames(-10.0, 300)); // 把底顶到上夹取
        run(&mut used, &tone_frames(-100.0, 30)); // 再压到下夹取
        used.reset();

        let mut fresh = EnergyVad::new(VadSensitivity::Normal);
        let mut probe = noise_frames(&mut rng, -58.0, 120);
        probe.extend(tone_frames(-30.0, 60));
        probe.extend(noise_frames(&mut rng, -58.0, 120));
        assert_eq!(run(&mut used, &probe), run(&mut fresh, &probe));
        assert_eq!(used.floor_dbfs(), fresh.floor_dbfs());
    }

    /// 1️⃣1️⃣ **现场回归夹具（2026-08-18 VPIO 形态）**——现场那一次就长这个样子：
    /// 一条 −60dBFS 的底噪垫，上面是 4Hz 的音节包络（每个音节 ~90ms 出声 + ~160ms 闭合），
    /// 峰帧 RMS **−48**、谷回到 −60。
    ///
    /// **为什么峰帧取 −48 而不是更响的 −35**：模块文档那张实测表里，VPIO 路径的语音
    /// p50 落在 **−40 ~ −57.9**，−48 正是这个区间的中部；−35 比实测最响的那次还高 5dB，
    /// 拿它当"现场语音"等于挑了一个连坏掉的旧门限都快够得着的乐观样本——回归夹具要钉的
    /// 恰好是**难的那一半**。谷仍取 −60（VPIO 停顿期的底噪垫），故本夹具的有效 SNR 只剩
    /// 12dB，比 −35 那版紧了 13dB。
    ///
    /// 两个断言**跑在同一批峰帧上**（apples-to-apples）：
    /// - **新门限**：峰帧 ≥90% 判 `Speech`（底跟到 −60，门限 −50，−48 的音节仍稳过）；
    /// - **旧门限**：同一批峰帧用本地闭包 `|rms| rms >= 0.006`（= −44.4dBFS）复算，命中率
    ///   **<10%**——现场那次的语音电平，旧门限几乎一帧都认不出来，于是音节被整段吞掉，
    ///   下游 `StreamSegmenter` 根本收不到窗（现场表现：录到一半不出字）。
    ///
    /// 分母是**同一批峰帧**，刻意如此：两条断言回答的是同一个问题——"现场那次的语音
    /// 电平，新旧两侧各认不认"——只有分母相同，两个比率才能并排读。
    /// 旧门限**绝不以类型形式保留**——它是被删掉的代码，只以这条闭包的形式活在回归夹具里。
    #[test]
    fn field_regression_vpio_syllabic_envelope() {
        const OLD_FIXED_THRESHOLD_RMS: f32 = 0.006; // 旧 DEFAULT_THRESHOLD_RMS = −44.4dBFS
        /// 峰帧判据：帧 RMS 落在峰值 2dB 以内（包络顶部）。
        const PEAK_FLOOR_DBFS: f32 = -50.0;
        let peak_amp = lin(-48.0) * std::f32::consts::SQRT_2;
        let trough_amp = lin(-60.0) * std::f32::consts::SQRT_2;
        let burst = 0.09f32; // 音节出声段 90ms
        let period = 0.25f32; // 4Hz 音节率

        let mut rng = Rng::new(0x2026_0818);
        let mut vad = EnergyVad::new(VadSensitivity::Normal);
        run(&mut vad, &noise_frames(&mut rng, -60.0, 100)); // 2s 底噪垫，底先落到 −60

        // 5s 音节段：300Hz 载波 × 音节包络。
        let syllables: Vec<Vec<f32>> = (0..250)
            .map(|f| {
                (0..VAD_FRAME_SAMPLES)
                    .map(|i| {
                        let t = (f * VAD_FRAME_SAMPLES + i) as f32 / 16_000.0;
                        let u = (t % period) / burst;
                        let env = if u < 1.0 {
                            (std::f32::consts::PI * u).sin().powi(2)
                        } else {
                            0.0
                        };
                        let amp = trough_amp + (peak_amp - trough_amp) * env;
                        (2.0 * std::f32::consts::PI * 300.0 * t).sin() * amp
                    })
                    .collect()
            })
            .collect();
        let labels = run(&mut vad, &syllables);

        // 峰帧集合：两条断言共用的**同一个分母**。
        let peaks: Vec<usize> = (0..syllables.len())
            .filter(|i| dbfs(frame_rms(&syllables[*i])) >= PEAK_FLOOR_DBFS)
            .collect();
        assert!(peaks.len() >= 20, "夹具没造出足够的峰帧：{}", peaks.len());
        let peak_hit = peaks.iter().filter(|i| labels[**i].is_speech()).count();
        let peak_rate = peak_hit as f32 / peaks.len() as f32;
        assert!(
            peak_rate >= 0.90,
            "自适应门限只认出 {:.1}% 的音节峰帧（底 {:.1} 门限 {:.1}）",
            peak_rate * 100.0,
            vad.floor_dbfs(),
            vad.threshold_dbfs()
        );

        // 同一批峰帧，换旧的固定门限复算。
        let old_hit = peaks
            .iter()
            .filter(|i| frame_rms(&syllables[**i]) >= OLD_FIXED_THRESHOLD_RMS)
            .count();
        let old_rate = old_hit as f32 / peaks.len() as f32;
        assert!(
            old_rate < 0.10,
            "旧固定门限在峰帧上命中 {:.1}%——夹具电平画高了，这条回归失去意义",
            old_rate * 100.0
        );
    }

    /// 1️⃣2️⃣ cpal 房间底噪必须被挡住——**本次测量才发现的反向缺陷**：
    /// 旧的固定门限 −44.4dBFS **低于** cpal 实测底噪（p10 −40.7 ~ −44.4），等于闸门全开、
    /// 连底噪都判语音，`PRD-02-007`（绝不把静音喂给 whisper）在这条路径上从未真正生效过。
    ///
    /// 两段各 10s，覆盖实测底噪带的两端：−45（min 形态）与 −42（p10 形态）。
    /// 语音帧都必须 ≤5%；后一段同时是**反向夹具**——旧门限在它上面近乎全中，
    /// 证明这条断言不是空跑（若断言写反或门限失效，那 10 秒会整片变成"语音"）。
    #[test]
    fn cpal_room_noise_is_gated() {
        const OLD_FIXED_THRESHOLD_RMS: f32 = 0.006; // 旧 DEFAULT_THRESHOLD_RMS = −44.4dBFS
        for room_dbfs in [-45.0f32, -42.0] {
            let mut rng = Rng::new(0xC9A1);
            let mut vad = EnergyVad::new(VadSensitivity::Normal);
            let room = noise_frames(&mut rng, room_dbfs, 500); // 10s
            let labels = run(&mut vad, &room);
            let rate = labels.iter().filter(|l| l.is_speech()).count() as f32 / labels.len() as f32;
            assert!(
                rate <= 0.05,
                "{room_dbfs} dBFS 的 cpal 底噪被判语音 {:.1}%（底 {:.1} 门限 {:.1}）",
                rate * 100.0,
                vad.floor_dbfs(),
                vad.threshold_dbfs()
            );
        }

        let mut rng = Rng::new(0xC9A1);
        let p10_room = noise_frames(&mut rng, -42.0, 500);
        let old_hit = p10_room
            .iter()
            .filter(|f| frame_rms(f) >= OLD_FIXED_THRESHOLD_RMS)
            .count();
        assert!(
            old_hit as f32 / p10_room.len() as f32 > 0.9,
            "旧门限本应把 p10 量级的 cpal 底噪整片判成语音，夹具电平画错了"
        );
    }

    /// 设置字面量 ↔ 档位的对应关系只有这一处实现，故也只在这里钉：
    /// 三个合法值逐字对应，其余一切（缺失/拼错/大小写/未来改名后的旧值）回落 `Normal`。
    #[test]
    fn setting_values_map_to_levels_and_unknown_falls_back_to_normal() {
        assert_eq!(VadSensitivity::from_setting("low"), VadSensitivity::Low);
        assert_eq!(
            VadSensitivity::from_setting("normal"),
            VadSensitivity::Normal
        );
        assert_eq!(VadSensitivity::from_setting("high"), VadSensitivity::High);
        for bad in ["", "  ", "LOW", "High", "medium", "0", "null"] {
            assert_eq!(
                VadSensitivity::from_setting(bad),
                VadSensitivity::Normal,
                "无法识别的设置值 {bad:?} 必须回落默认档，绝不 panic、也绝不静默变成别的档"
            );
        }
        // 默认档与注册表默认值（`normal`）必须同源，否则"没改过设置"与"显式选了 normal"
        // 会走出两种行为。
        assert_eq!(VadSensitivity::default(), VadSensitivity::Normal);
    }

    #[test]
    fn rms_of_silence_is_zero() {
        assert_eq!(frame_rms(&[0.0; 512]), 0.0);
        assert_eq!(frame_rms(&[]), 0.0);
    }

    #[test]
    fn rms_of_full_scale_is_one() {
        // 方波 ±1 的 RMS = 1。
        let f: Vec<f32> = (0..512)
            .map(|i| if i % 2 == 0 { 1.0 } else { -1.0 })
            .collect();
        assert!((frame_rms(&f) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn dbfs_maps_full_scale_and_floors_zero() {
        assert!((dbfs(1.0) - 0.0).abs() < 1e-6, "满幅 = 0dBFS");
        // 0.1 线性 = −20dBFS（每 ×0.1 降 20dB，分位数读数的对齐锚点）。
        assert!((dbfs(0.1) + 20.0).abs() < 1e-4);
        // 数字零必须封底，绝不产出 -inf（否则分位数与日志格式化双双失效）。
        assert_eq!(dbfs(0.0), DBFS_FLOOR);
        assert!(dbfs(0.0).is_finite());
        // 封底也管"正但小到无意义"的残渣（去直流后能算出 −175dBFS 这种读数）。
        assert_eq!(dbfs(1e-9), DBFS_FLOOR);
    }

    /// 判定依据必须**如实**反映本帧：现场靠它一眼看出"底与门限当时是多少"。
    #[test]
    fn last_decision_reports_actual_threshold() {
        let mut vad = EnergyVad::new(VadSensitivity::High);
        vad.score(&tone_frame(-50.0));
        let VadDecision::Energy {
            rms_dbfs,
            floor_dbfs,
            threshold_dbfs,
        } = vad.last_decision()
        else {
            panic!("能量 VAD 只产 Energy 判定");
        };
        assert!((rms_dbfs - (-50.0)).abs() < 0.05);
        assert!(
            (threshold_dbfs - (floor_dbfs + 6.0)).abs() < 1e-4,
            "High 档余量 6dB"
        );
        assert_eq!(vad.last_decision().rms_dbfs(), rms_dbfs);
    }
}
