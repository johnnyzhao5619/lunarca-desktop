//! macOS 系统音频采集——Core Audio Process Tap（CATap，macOS 14.4+）。
//! 事实源：[TDD-02 §3.1-I]、ADR-019；实测依据：S11 spike
//! （S11 CATap 系统音频 spike，内部记录，含五坑实录）。
//!
//! 流程（S11 验证通过）：全局 `CATapDescription` → `AudioHardwareCreateProcessTap` →
//! 私有聚合设备（**必须**把默认输出设备设为 `main_sub_device` 作时钟，否则
//! `AudioDeviceStart` 报 `nope`；但 `full_sub_device_list` **必须留空**，理由见 ADR-082
//! 与下面「交付布局」一段）→ 挂 tap 到 `tap_list` → `AudioDeviceIOProc` 把 f32 交错样本经
//! [`luna_audio::FrameInjector`] 无锁灌入消费管线（ADR-009 铁律：回调内禁分配/锁/IO/日志）。
//!
//! **交付布局是聚合设备的属性，不是 tap 的属性（ADR-082，真机坐实）**：IOProc 收到的
//! `AudioBufferList` 逐条对应**聚合设备的输入流**——把默认输出设备列进
//! `full_sub_device_list` 时，该设备自己的输入流会一并并进同一个 ABL 排在 tap 之前。
//! macOS 的 VPIO（ADR-021，Both 档 mic 侧）一旦启动，就会给默认输出设备装上一条
//! 6 声道回声参考输入流；于是 `buffers[0]` 不再是 tap 而是那条参考流，按 tap 的 2 声道
//! 去解释它 → 帧数恰好多算 3.00×（6ch/2ch），采到的还是错的信号。**故聚合设备只挂
//! 时钟、不挂子设备**，并在启动前用聚合设备的实际输入流格式/布局校验一遍（见
//! [`SystemAudioTap::start`]），运行期再由 IOProc 逐回调守一道（布局变了就停推并报源失效）。
//!
//! **绑定方式（本模块不走 cidre/coreaudio-sys）**：tap 相关符号
//! （`AudioHardwareCreateProcessTap`/`Destroy…`）为 macOS 14.2+ 新增——**强链接会让
//! app 在更老系统上无法启动**（dyld 缺符号即中止），故经 `dlsym` 弱化绑定并以探测结果做
//! 版本门控（低版本返回 [`TapError::Unsupported`] → 上层降级仅麦克风，PRD-04-052）；
//! `CATapDescription` 是 ObjC 类，经 `AnyClass::get` 动态查找（同理避免强类引用）；
//! 其余 HAL API（聚合设备/属性/IOProc）自古存在，直接强链接 CoreAudio.framework。
//!
//! **TCC（「系统音频录制」独立类别，TDD-02 §7 / TDD-05 §4）**：授权被拒/被撤销的表现是
//! `AudioDeviceStart` 成功但 IOProc **静默零回调**（无任何错误码）——检测归
//! `luna-audio` 消费线程的零回调看门狗（`CaptureDiagnostic::NoCaptureData`），本模块不做。

use luna_audio::FrameInjector;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, NSObject};
use objc2_foundation::NSArray;
use std::ffi::{c_void, CStr};
use std::sync::atomic::{AtomicU64, Ordering};

use core_foundation::array::CFArray;
use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::{CFString, CFStringRef};

// ---------- Core Audio HAL FFI（仅本模块使用的最小面；类型/常量对齐 AudioHardware.h） ----------

type AudioObjectId = u32;
type OsStatus = i32;

/// `kAudioObjectSystemObject`。
const SYSTEM_OBJECT: AudioObjectId = 1;

const fn fourcc(b: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*b)
}

#[repr(C)]
struct AudioObjectPropertyAddress {
    selector: u32,
    scope: u32,
    element: u32,
}

impl AudioObjectPropertyAddress {
    /// 全局作用域 + 主元素（本模块用到的属性全部如此）。
    const fn global(selector: u32) -> Self {
        Self {
            selector,
            scope: fourcc(b"glob"), // kAudioObjectPropertyScopeGlobal
            element: 0,             // kAudioObjectPropertyElementMain
        }
    }
}

/// `AudioStreamBasicDescription`（读 tap 原生格式用）。
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct AudioStreamBasicDescription {
    sample_rate: f64,
    format_id: u32,
    format_flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels_per_frame: u32,
    bits_per_channel: u32,
    reserved: u32,
}

#[repr(C)]
struct AudioBuffer {
    number_channels: u32,
    data_byte_size: u32,
    data: *mut c_void,
}

#[repr(C)]
struct AudioBufferList {
    number_buffers: u32,
    buffers: [AudioBuffer; 1],
}

/// 只作指针传递、不读字段——不透明声明即可。
#[repr(C)]
struct AudioTimeStamp {
    _opaque: [u8; 0],
}

type AudioDeviceIoProc = extern "C-unwind" fn(
    AudioObjectId,
    *const AudioTimeStamp,
    *const AudioBufferList,
    *const AudioTimeStamp,
    *mut AudioBufferList,
    *const AudioTimeStamp,
    *mut c_void,
) -> OsStatus;

/// `AudioDeviceIOProcID`（可空函数指针）。
type AudioDeviceIoProcId = Option<AudioDeviceIoProc>;

#[link(name = "CoreAudio", kind = "framework")]
extern "C-unwind" {
    fn AudioObjectGetPropertyData(
        object: AudioObjectId,
        address: *const AudioObjectPropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        io_size: *mut u32,
        out_data: *mut c_void,
    ) -> OsStatus;
    fn AudioObjectGetPropertyDataSize(
        object: AudioObjectId,
        address: *const AudioObjectPropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        out_size: *mut u32,
    ) -> OsStatus;
    fn AudioObjectSetPropertyData(
        object: AudioObjectId,
        address: *const AudioObjectPropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: u32,
        data: *const c_void,
    ) -> OsStatus;
    fn AudioHardwareCreateAggregateDevice(
        description: core_foundation::dictionary::CFDictionaryRef,
        out_device: *mut AudioObjectId,
    ) -> OsStatus;
    fn AudioHardwareDestroyAggregateDevice(device: AudioObjectId) -> OsStatus;
    fn AudioDeviceCreateIOProcID(
        device: AudioObjectId,
        io_proc: AudioDeviceIoProc,
        client_data: *mut c_void,
        out_proc_id: *mut AudioDeviceIoProcId,
    ) -> OsStatus;
    fn AudioDeviceDestroyIOProcID(device: AudioObjectId, proc_id: AudioDeviceIoProcId) -> OsStatus;
    fn AudioDeviceStart(device: AudioObjectId, proc_id: AudioDeviceIoProcId) -> OsStatus;
    fn AudioDeviceStop(device: AudioObjectId, proc_id: AudioDeviceIoProcId) -> OsStatus;
}

/// tap 创建/销毁函数签名（macOS 14.2+ 符号，经 dlsym 弱化绑定）。
type CreateTapFn = unsafe extern "C-unwind" fn(*mut AnyObject, *mut AudioObjectId) -> OsStatus;
type DestroyTapFn = unsafe extern "C-unwind" fn(AudioObjectId) -> OsStatus;

fn dlsym_fn(name: &CStr) -> *mut c_void {
    // Safety：RTLD_DEFAULT 全局符号查找，仅读。
    unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr()) }
}

fn tap_fns() -> Option<(CreateTapFn, DestroyTapFn)> {
    let create = dlsym_fn(c"AudioHardwareCreateProcessTap");
    let destroy = dlsym_fn(c"AudioHardwareDestroyProcessTap");
    if create.is_null() || destroy.is_null() {
        return None;
    }
    // Safety：符号存在即为系统框架导出的对应函数；签名对齐 AudioHardwareTapping.h。
    unsafe {
        Some((
            std::mem::transmute::<*mut c_void, CreateTapFn>(create),
            std::mem::transmute::<*mut c_void, DestroyTapFn>(destroy),
        ))
    }
}

// ---------- 公开 API ----------

/// CATap 采集错误（仅状态码/阶段，绝不含音频内容——日志脱敏铁律）。
#[derive(Debug, thiserror::Error)]
pub enum TapError {
    /// 系统不支持（macOS < 14.2 无 tap 符号，或 CATapDescription 类缺失）。
    /// 产品口径按 14.4 宣传（第三方 app 实际可用版本，ADR-019）。
    #[error("system audio capture is unsupported on this system (requires macOS 14.4+)")]
    Unsupported,
    /// 建 tap 失败。
    #[error("failed to create the system audio tap (OSStatus {0})")]
    CreateTap(OsStatus),
    /// 读 tap 属性（uid/格式）失败。
    #[error("failed to read a tap property (OSStatus {0})")]
    TapProperty(OsStatus),
    /// tap 返回的 PCM 布局不是下游明确支持的 32-bit float stereo。
    #[error("the system audio tap returned an unsupported audio format")]
    UnsupportedFormat,
    /// 解析默认输出设备（时钟子设备）失败。
    #[error("failed to resolve the default output device (OSStatus {0})")]
    OutputDevice(OsStatus),
    /// 建聚合设备/挂 tap 失败。
    #[error("failed to create the aggregate capture device (OSStatus {0})")]
    Aggregate(OsStatus),
    /// 聚合设备**实际交付**的输入格式/布局与 tap 声明不符（ADR-082）。
    ///
    /// 这条不是"设备坏了"，而是"我们对交付形态的假设不成立"——带病启动的代价是整条轨
    /// 的时间线与计量倍速跑偏（历史上是 3.00×），且采到的根本不是 tap 的信号。故一律
    /// 如实失败，由上层降级为仅麦克风。载荷只有数值/布局，无任何音频内容（脱敏铁律）。
    #[error("system audio delivered a format other than the one declared ({0})")]
    DeliveredMismatch(String),
    /// IOProc 创建/启动失败。
    #[error("failed to start system audio io (OSStatus {0})")]
    Io(OsStatus),
}

/// tap 原生流格式（S11 实测 48kHz/2ch/f32；仍显式携带，不做假设——§3.1-B 同款纪律）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TapFormat {
    /// 采样率（Hz）。
    pub sample_rate: u32,
    /// 交错声道数。
    pub channels: u16,
    layout: TapSampleLayout,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TapSampleLayout {
    Interleaved,
    PlanarStereo,
}

impl TapFormat {
    /// 本格式**必然**对应的 IOProc 交付布局：交错 = 1 条 N 声道缓冲；平面 = N 条单声道缓冲。
    ///
    /// 这是把「声明」翻译成「可与实际交付逐字比对的形状」的唯一处——启动前校验
    /// （聚合设备的 stream configuration）与运行期守卫（每回调的 ABL）共用它，
    /// 两处判据不得各写一份（DRY；判据分家正是 ADR-082 那个 bug 能活下来的土壤）。
    fn expected_abl(self) -> AblLayout {
        match self.layout {
            TapSampleLayout::Interleaved => AblLayout {
                buffers: 1,
                channels_per_buffer: u32::from(self.channels),
            },
            TapSampleLayout::PlanarStereo => AblLayout {
                buffers: u32::from(self.channels),
                channels_per_buffer: 1,
            },
        }
    }
}

/// IOProc 交付的 `AudioBufferList` 形状（缓冲条数 + 每条缓冲的声道数）。
///
/// **为什么必须显式建模**：ABL 的缓冲**逐条对应聚合设备的输入流**，而聚合设备的输入流
/// 集合不由我们单独决定——系统里别的组件（ADR-021 的 VPIO）可以给子设备装上新的输入流。
/// 「`buffers[0]` 就是 tap」是一个会被外部推翻的假设，推翻时**没有任何错误码**，只表现为
/// 帧数按错误声道数计量（ADR-082：多算 6ch/2ch = 3.00×）。故声明与交付必须逐字比对。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AblLayout {
    buffers: u32,
    channels_per_buffer: u32,
}

impl AblLayout {
    /// 每条缓冲声道数相同时的形状（`channels` 逐条列出实际交付的声道数）。
    fn from_channels(channels: &[u32]) -> Option<Self> {
        let first = *channels.first()?;
        channels.iter().all(|c| *c == first).then_some(Self {
            buffers: channels.len() as u32,
            channels_per_buffer: first,
        })
    }

    /// 人类可读的形状描述（**仅数值**，无音频内容——脱敏铁律）。
    fn describe(self) -> String {
        format!("{}×{}ch", self.buffers, self.channels_per_buffer)
    }
}

/// 比对「声明的交付布局」与「聚合设备实际交付的每缓冲声道数」。
///
/// 纯函数，headless 可单测——ADR-082 的反向夹具就锁在这里：把 VPIO 在场时的实际布局
/// `[6, 2]` 喂进来必须变红，否则那个 3.00× 会原样复活。
fn check_abl_layout(expected: AblLayout, actual_channels: &[u32]) -> Result<(), TapError> {
    let actual = AblLayout::from_channels(actual_channels).filter(|a| *a == expected);
    if actual.is_some() {
        return Ok(());
    }
    let actual_desc = actual_channels
        .iter()
        .map(|c| format!("{c}ch"))
        .collect::<Vec<_>>()
        .join("+");
    Err(TapError::DeliveredMismatch(format!(
        "输入缓冲布局 期望 {} 实测 [{}]",
        expected.describe(),
        if actual_desc.is_empty() {
            "空".to_string()
        } else {
            actual_desc
        }
    )))
}

/// 挂上 tap 后等聚合设备发布流拓扑的上限（会话启动路径，非实时线程）。
///
/// 真机实测未就绪窗口在十毫秒量级；给到 1s 是为了留足慢机余量，同时**必须有界**——
/// 启动路径上的无限等待就是把「系统 API 卡住」翻译成「HUD 永远停在启动中」（同
/// [`RunningSystemAudioTap::drop`] 里那条实测教训）。
const AGG_TOPOLOGY_SETTLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);
/// 拓扑轮询间隔。
const AGG_TOPOLOGY_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);

const LINEAR_PCM_FORMAT: u32 = fourcc(b"lpcm");
const FORMAT_FLAG_FLOAT: u32 = 1 << 0;
const FORMAT_FLAG_PACKED: u32 = 1 << 3;
const FORMAT_FLAG_NON_INTERLEAVED: u32 = 1 << 5;

fn tap_format_from_asbd(asbd: AudioStreamBasicDescription) -> Result<TapFormat, TapError> {
    let supported_flags = FORMAT_FLAG_FLOAT | FORMAT_FLAG_PACKED;
    if asbd.format_id != LINEAR_PCM_FORMAT
        || asbd.format_flags & supported_flags != supported_flags
        || asbd.bits_per_channel != 32
        || asbd.channels_per_frame != 2
        || !asbd.sample_rate.is_finite()
        || asbd.sample_rate <= 0.0
        || asbd.sample_rate > f64::from(u32::MAX)
    {
        return Err(TapError::UnsupportedFormat);
    }
    let layout = if asbd.format_flags & FORMAT_FLAG_NON_INTERLEAVED != 0 {
        TapSampleLayout::PlanarStereo
    } else {
        TapSampleLayout::Interleaved
    };
    Ok(TapFormat {
        sample_rate: asbd.sample_rate.round() as u32,
        channels: 2,
        layout,
    })
}

/// 本系统是否具备 CATap 能力（符号 + 类双探测；不触发 TCC）。
#[must_use]
pub fn is_supported() -> bool {
    tap_fns().is_some() && AnyClass::get(c"CATapDescription").is_some()
}

/// 默认输出是否为**内建扬声器**（外放场景检测；PRD-04-052「建议佩戴耳机」提示，
/// TDD-02 §3.1-I 外放对策层①）。
///
/// 判定口径：默认输出设备 transport = BuiltIn（`bltn`）且输出 data source ≠ 3.5mm
/// 耳机口（`hdpn`）。蓝牙/USB 等外部设备**不判外放**——外接音箱与外接耳机 DAC 在
/// HAL 层无法区分，宁可漏报不误报（提示是建议不是门禁）。查询失败按「非外放」处理。
#[must_use]
pub fn default_output_is_builtin_speaker() -> bool {
    let Ok(out_dev) = get_prop::<AudioObjectId>(
        SYSTEM_OBJECT,
        &AudioObjectPropertyAddress::global(fourcc(b"dOut")),
    ) else {
        return false;
    };
    // kAudioDevicePropertyTransportType（'tran'）。
    let Ok(transport) = get_prop::<u32>(
        out_dev,
        &AudioObjectPropertyAddress::global(fourcc(b"tran")),
    ) else {
        return false;
    };
    if transport != fourcc(b"bltn") {
        return false;
    }
    // 内建设备再分内建扬声器 vs 耳机口：kAudioDevicePropertyDataSource（'ssrc'，输出作用域）。
    let addr = AudioObjectPropertyAddress {
        selector: fourcc(b"ssrc"),
        scope: fourcc(b"outp"), // kAudioObjectPropertyScopeOutput
        element: 0,
    };
    match get_prop::<u32>(out_dev, &addr) {
        Ok(source) => source != fourcc(b"hdpn"),
        // 无 data source 属性的内建输出（无耳机口机型）按扬声器处理。
        Err(_) => true,
    }
}

/// 已建 tap、未启动 IO 的中间态：先读格式（上层据此建管线），再 [`start`](Self::start)。
pub struct SystemAudioTap {
    create_destroy: (CreateTapFn, DestroyTapFn),
    /// `Some` = 本句柄持有销毁责任；`start()` 移交后置 `None`（Drop 不再销毁）。
    tap_id: Option<AudioObjectId>,
    tap_uid: CFString,
    format: TapFormat,
}

/// 运行中的系统音频采集：持有 tap + 聚合设备 + IOProc；`stop()`/Drop 逆序拆除。
pub struct RunningSystemAudioTap {
    destroy_tap: DestroyTapFn,
    tap_id: AudioObjectId,
    agg_id: AudioObjectId,
    proc_id: AudioDeviceIoProcId,
    /// IOProc client data；start→stop 期间由 HAL 实时线程独占使用。
    ctx: *mut IoCtx,
}

// Safety：句柄仅做生命周期管理；ctx 在 start 后由 HAL 线程独占、Drop 先停 IO 再回收，
// 跨线程转移句柄本身不构成数据竞争。
unsafe impl Send for RunningSystemAudioTap {}

struct IoCtx {
    injector: FrameInjector,
    layout: TapSampleLayout,
    /// 启动前已校验过的交付布局；运行期每回调复核一次（ADR-082）。
    expected: AblLayout,
}

extern "C-unwind" fn io_proc(
    _device: AudioObjectId,
    _now: *const AudioTimeStamp,
    in_data: *const AudioBufferList,
    _in_time: *const AudioTimeStamp,
    _out_data: *mut AudioBufferList,
    _out_time: *const AudioTimeStamp,
    client: *mut c_void,
) -> OsStatus {
    // ADR-009 铁律：实时回调内只做无锁拷贝（FrameInjector::push_f32），禁分配/锁/IO/日志。
    // Safety：client 由 start() 装入 Box<IoCtx>，stop/Drop 先停设备再回收；
    // 期间 HAL IO 线程独占本指针（SPSC 单生产者）。
    unsafe {
        if in_data.is_null() || client.is_null() {
            return 0;
        }
        let ctx = &mut *client.cast::<IoCtx>();
        let abl = &*in_data;
        let first = std::ptr::addr_of!((*in_data).buffers).cast::<AudioBuffer>();
        // ADR-082 运行期守卫：交付布局必须与启动前校验过的声明**逐条**一致。
        // 不一致的形态是「聚合设备多了一条别人的输入流」（VPIO 中途启动/输出设备切换），
        // 此时 `buffers[0]` 已不是 tap——照推就是把别的信号当系统声音，且按错误声道数
        // 计量帧数使整条轨倍速。故停止灌帧并标记源失效（原子写，wait-free；上层走既有
        // DeviceLost 降级路径），**绝不带病继续**。比较至多两条缓冲，无分配无锁。
        let mut layout_ok = abl.number_buffers == ctx.expected.buffers;
        for i in 0..abl.number_buffers {
            layout_ok &=
                (*first.add(i as usize)).number_channels == ctx.expected.channels_per_buffer;
        }
        if !layout_ok {
            ctx.injector.mark_source_lost();
            return 0;
        }
        if ctx.layout == TapSampleLayout::Interleaved {
            let buf = &*first;
            let n = buf.data_byte_size as usize / std::mem::size_of::<f32>();
            if !buf.data.is_null() && n > 0 {
                ctx.injector
                    .push_f32(std::slice::from_raw_parts(buf.data.cast::<f32>(), n));
            }
        } else {
            let left = &*first;
            let right = &*first.add(1);
            let left_len = left.data_byte_size as usize / std::mem::size_of::<f32>();
            let right_len = right.data_byte_size as usize / std::mem::size_of::<f32>();
            if !left.data.is_null() && !right.data.is_null() && left_len > 0 && right_len > 0 {
                ctx.injector.push_f32_planar_stereo(
                    std::slice::from_raw_parts(left.data.cast::<f32>(), left_len),
                    std::slice::from_raw_parts(right.data.cast::<f32>(), right_len),
                );
            }
        }
    }
    0
}

impl SystemAudioTap {
    /// 建全局系统音频 tap（排除进程列表为空 = 采全部输出）并读取原生格式。
    ///
    /// 正式签名构建下，首次调用触发系统「系统音频录制」授权弹框（TCC 细节见模块文档）。
    ///
    /// # Errors
    /// 系统不支持返回 [`TapError::Unsupported`]；HAL 调用失败返回带 OSStatus 的对应变体。
    pub fn create() -> Result<Self, TapError> {
        let fns = tap_fns().ok_or(TapError::Unsupported)?;
        let cls = AnyClass::get(c"CATapDescription").ok_or(TapError::Unsupported)?;

        // CATapDescription：initStereoGlobalTapButExcludeProcesses:@[]（全局立体声 mixdown）。
        let exclude: Retained<NSArray<NSObject>> = NSArray::new();
        // Safety：动态查得的类按头文件契约调用 alloc/init；init 失败返回 nil。
        let desc: Option<Retained<AnyObject>> = unsafe {
            let alloc: *mut AnyObject = msg_send![cls, alloc];
            let obj: *mut AnyObject =
                msg_send![alloc, initStereoGlobalTapButExcludeProcesses: &*exclude];
            Retained::from_raw(obj)
        };
        let desc = desc.ok_or(TapError::Unsupported)?;

        let mut tap_id: AudioObjectId = 0;
        // Safety：desc 为合法 CATapDescription 实例；out 指针有效。
        let status = unsafe { (fns.0)(Retained::as_ptr(&desc).cast_mut(), &raw mut tap_id) };
        if status != 0 || tap_id == 0 {
            return Err(TapError::CreateTap(status));
        }
        // tap 已建立；后续失败路径由守卫兜底销毁。
        let mut guard = TapIdGuard {
            destroy: fns.1,
            tap_id: Some(tap_id),
        };

        let tap_uid = read_tap_uid(tap_id).map_err(TapError::TapProperty)?;
        let asbd = read_tap_format(tap_id).map_err(TapError::TapProperty)?;
        let format = tap_format_from_asbd(asbd)?;

        guard.tap_id.take(); // 销毁责任移交 Self
        Ok(Self {
            create_destroy: fns,
            tap_id: Some(tap_id),
            tap_uid,
            format,
        })
    }

    /// tap 原生格式（上层以此构造 `ExternalSourceSpec` 建消费管线）。
    #[must_use]
    pub fn format(&self) -> TapFormat {
        self.format
    }

    /// 建私有聚合设备（默认输出为时钟子设备，S11 硬要求①）→ 挂 tap → 启动 IOProc。
    ///
    /// # Errors
    /// 聚合设备/属性设置/IO 启动失败时返回对应变体；失败路径完整回收已建对象。
    pub fn start(mut self, injector: FrameInjector) -> Result<RunningSystemAudioTap, TapError> {
        let destroy_tap = self.create_destroy.1;
        let tap_uid = self.tap_uid.clone();
        // 销毁责任移交本函数（守卫兜底失败路径）；self 随后正常 Drop（仅释放 tap_uid）。
        let tap_id = self.tap_id.take().expect("tap_id 只在 start 移交一次");
        let mut tap_guard = TapIdGuard {
            destroy: destroy_tap,
            tap_id: Some(tap_id),
        };

        // 1) 默认输出设备及其 UID（时钟子设备）。
        let out_dev = get_prop::<AudioObjectId>(
            SYSTEM_OBJECT,
            &AudioObjectPropertyAddress::global(fourcc(b"dOut")),
        )
        .map_err(TapError::OutputDevice)?;
        let out_uid =
            read_cfstring_prop(out_dev, fourcc(b"uid ")).map_err(TapError::OutputDevice)?;

        // 2) 私有聚合设备（不出现在系统设备列表；tapautostart 使 tap 随设备启动）。
        static AGG_SEQ: AtomicU64 = AtomicU64::new(0);
        let agg_uid = format!(
            "com.luna.systap.{}.{}",
            std::process::id(),
            AGG_SEQ.fetch_add(1, Ordering::Relaxed)
        );
        let desc = CFDictionary::from_CFType_pairs(&[
            (
                CFString::from_static_string("uid").as_CFType(),
                CFString::new(&agg_uid).as_CFType(),
            ),
            (
                CFString::from_static_string("name").as_CFType(),
                CFString::from_static_string("Lunarca System Audio Tap").as_CFType(),
            ),
            (
                CFString::from_static_string("private").as_CFType(),
                CFBoolean::true_value().as_CFType(),
            ),
            (
                CFString::from_static_string("tapautostart").as_CFType(),
                CFBoolean::true_value().as_CFType(),
            ),
        ]);
        let mut agg_id: AudioObjectId = 0;
        // Safety：dict 为合法 CFDictionary；out 指针有效。
        let status = unsafe {
            AudioHardwareCreateAggregateDevice(desc.as_concrete_TypeRef(), &raw mut agg_id)
        };
        if status != 0 || agg_id == 0 {
            return Err(TapError::Aggregate(status));
        }
        let mut agg_guard = AggIdGuard {
            agg_id: Some(agg_id),
        };

        // 3) 时钟（main sub device）+ tap 列表。
        //
        // **`full_sub_device_list` 必须是空数组**（ADR-082）：默认输出设备只当时钟
        // （`amst`）就足以让 `AudioDeviceStart` 通过（S11 那条「否则报 nope」的硬要求指的
        // 是它，不是子设备列表）。一旦把该设备列进子设备列表，**它自己的输入流会并进
        // IOProc 的同一个 `AudioBufferList` 并排在 tap 前面**——Both 档 mic 侧的 VPIO
        // （ADR-021）恰好会给默认输出设备装上一条 6 声道回声参考输入流，于是 `buffers[0]`
        // 不再是 tap。空列表让「聚合设备的输入流 = tap 且仅 tap」成为结构事实，而不是
        // 一个每次系统更新都要重新赌一次的假设。
        let subs = CFArray::<CFString>::from_CFTypes(&[]);
        set_cftype_prop(agg_id, fourcc(b"grup"), &subs.as_concrete_TypeRef().cast())
            .map_err(TapError::Aggregate)?;
        set_cftype_prop(
            agg_id,
            fourcc(b"amst"),
            &out_uid.as_concrete_TypeRef().cast(),
        )
        .map_err(TapError::Aggregate)?;
        let taps = CFArray::from_CFTypes(&[tap_uid]);
        set_cftype_prop(agg_id, fourcc(b"tap#"), &taps.as_concrete_TypeRef().cast())
            .map_err(TapError::Aggregate)?;

        // 4) **交付格式/布局校验**（ADR-082）：以聚合设备**实际**会交付的东西为准，与
        // `create()` 时读到的 tap 声明逐字比对。管线已按声明建好，对不上就只能如实失败
        // ——上层降级为仅麦克风，绝不静默按错误声道数计量。
        verify_delivered_format(agg_id, self.format)?;

        // 5) IOProc + 启动。
        let ctx = Box::into_raw(Box::new(IoCtx {
            injector,
            layout: self.format.layout,
            expected: self.format.expected_abl(),
        }));
        let mut proc_id: AudioDeviceIoProcId = None;
        // Safety：agg_id 有效；ctx 在 stop 前始终存活。
        let status = unsafe {
            AudioDeviceCreateIOProcID(agg_id, io_proc, ctx.cast::<c_void>(), &raw mut proc_id)
        };
        if status != 0 || proc_id.is_none() {
            // Safety：IOProc 未创建成功，ctx 无使用者，回收。
            unsafe { drop(Box::from_raw(ctx)) };
            return Err(TapError::Io(status));
        }
        // Safety：proc_id 刚创建。
        let status = unsafe { AudioDeviceStart(agg_id, proc_id) };
        if status != 0 {
            // Safety：先销毁 IOProc（未启动成功），再回收 ctx。
            unsafe {
                let _ = AudioDeviceDestroyIOProcID(agg_id, proc_id);
                drop(Box::from_raw(ctx));
            }
            return Err(TapError::Io(status));
        }

        // 全部成功：解除守卫，交 RAII 句柄接管。
        tap_guard.tap_id.take();
        agg_guard.agg_id.take();
        Ok(RunningSystemAudioTap {
            destroy_tap,
            tap_id,
            agg_id,
            proc_id,
            ctx,
        })
    }
}

impl Drop for SystemAudioTap {
    fn drop(&mut self) {
        if let Some(id) = self.tap_id {
            // Safety：id 由 create() 建立且未移交（start() 已 take 则跳过）。
            unsafe {
                let _ = (self.create_destroy.1)(id);
            }
        }
    }
}

impl RunningSystemAudioTap {
    /// 停止并逆序拆除（stop IO → destroy IOProc → destroy 聚合设备 → destroy tap）。
    ///
    /// 与消费侧的停止顺序约定见 [`luna_audio::start_external_capture`]：先停本句柄再停
    /// `CaptureHandle`，保证尾帧完整。
    pub fn stop(self) {
        drop(self);
    }
}

impl Drop for RunningSystemAudioTap {
    fn drop(&mut self) {
        // ⚠ 真机实测（2026-07-15，S11 后续）：**TCC 拒绝态**下 `AudioDeviceDestroyIOProcID`
        // 会在与 coreaudiod 的 mach_msg 上无限阻塞（`HALC_ProxyIOContext::
        // _TellServerAboutStreamUsage`）——而拒绝态恰是上层收到 NoCaptureData 后停会话的
        // 必经路径。拆除必须完全脱离会话控制面：Drop 只负责把逆序清理移交守护线程，
        // 绝不等待 CoreAudio 返回，否则一个系统 API 的卡死就会把 HUD 永久困在「停止中」。
        // ctx 在 HAL 可能仍引用期间绝不能释放；拒绝态下清理线程若永久阻塞，就安全泄漏
        // 这些对象以保全进程。正常授权态拆除仍会在该线程中毫秒级完成。
        let agg_id = self.agg_id;
        let proc_id = self.proc_id;
        let tap_id = self.tap_id;
        let destroy_tap = self.destroy_tap;
        let ctx_addr = self.ctx as usize;
        let spawned = std::thread::Builder::new()
            .name("luna-systap-teardown".into())
            .spawn(move || {
                // Safety：逆序拆除；停设备且 DestroyIOProcID 返回后 HAL 不再调用 io_proc，
                // ctx 方可回收。任何一步阻塞则本线程悬挂、ctx 永不释放（安全泄漏）。
                unsafe {
                    let _ = AudioDeviceStop(agg_id, proc_id);
                    let _ = AudioDeviceDestroyIOProcID(agg_id, proc_id);
                    let _ = AudioHardwareDestroyAggregateDevice(agg_id);
                    let _ = destroy_tap(tap_id);
                    drop(Box::from_raw(ctx_addr as *mut IoCtx));
                }
            })
            .is_ok();
        if !spawned {
            // 仅结构化状态，无任何音频/用户内容（日志脱敏铁律）。
            eprintln!("[luna-systap] 拆除线程不可用——对象泄漏以保全进程");
        }
    }
}

// ---------- 内部工具 ----------

/// 失败路径的 tap 销毁守卫（成功路径 take() 解除）。
struct TapIdGuard {
    destroy: DestroyTapFn,
    tap_id: Option<AudioObjectId>,
}

impl Drop for TapIdGuard {
    fn drop(&mut self) {
        if let Some(id) = self.tap_id {
            // Safety：id 由 create 建立且未移交。
            unsafe {
                let _ = (self.destroy)(id);
            }
        }
    }
}

/// 失败路径的聚合设备销毁守卫。
struct AggIdGuard {
    agg_id: Option<AudioObjectId>,
}

impl Drop for AggIdGuard {
    fn drop(&mut self) {
        if let Some(id) = self.agg_id {
            // Safety：id 由 AudioHardwareCreateAggregateDevice 建立且未移交。
            unsafe {
                let _ = AudioHardwareDestroyAggregateDevice(id);
            }
        }
    }
}

/// 读定长 POD 属性。
fn get_prop<T: Copy + Default>(
    object: AudioObjectId,
    addr: &AudioObjectPropertyAddress,
) -> Result<T, OsStatus> {
    let mut value = T::default();
    let mut size = u32::try_from(std::mem::size_of::<T>()).unwrap_or(0);
    // Safety：out 缓冲为 T 大小；HAL 按 io_size 写入。
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            addr,
            0,
            std::ptr::null(),
            &raw mut size,
            (&raw mut value).cast::<c_void>(),
        )
    };
    if status != 0 {
        return Err(status);
    }
    Ok(value)
}

/// 读 CFString 属性（Core Audio create 规则：调用方持有释放责任）。
fn read_cfstring_prop(object: AudioObjectId, selector: u32) -> Result<CFString, OsStatus> {
    let addr = AudioObjectPropertyAddress::global(selector);
    let mut string_ref: CFStringRef = std::ptr::null();
    let mut size = u32::try_from(std::mem::size_of::<CFStringRef>()).unwrap_or(0);
    // Safety：out 缓冲为 CFStringRef 大小。
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            &addr,
            0,
            std::ptr::null(),
            &raw mut size,
            (&raw mut string_ref).cast::<c_void>(),
        )
    };
    if status != 0 || string_ref.is_null() {
        return Err(if status != 0 { status } else { -1 });
    }
    // Safety：create 规则返回 +1 引用，wrap_under_create_rule 接管释放。
    Ok(unsafe { CFString::wrap_under_create_rule(string_ref) })
}

fn read_tap_uid(tap_id: AudioObjectId) -> Result<CFString, OsStatus> {
    read_cfstring_prop(tap_id, fourcc(b"tuid"))
}

fn read_tap_format(tap_id: AudioObjectId) -> Result<AudioStreamBasicDescription, OsStatus> {
    get_prop::<AudioStreamBasicDescription>(
        tap_id,
        &AudioObjectPropertyAddress::global(fourcc(b"tfmt")),
    )
}

/// 读变长属性到 8 字节对齐的存储（`AudioBufferList` / 流对象数组等含指针的 C 结构）。
///
/// 返回 `Vec<u64>` 而非 `Vec<u8>` 是为了对齐：`AudioBuffer` 含裸指针，按 `u8` 缓冲起头
/// 转指针是未定义行为的经典写法。
fn get_prop_var(
    object: AudioObjectId,
    addr: &AudioObjectPropertyAddress,
) -> Result<(Vec<u64>, usize), OsStatus> {
    let mut size: u32 = 0;
    // Safety：仅查询长度，不写出参缓冲。
    let status =
        unsafe { AudioObjectGetPropertyDataSize(object, addr, 0, std::ptr::null(), &raw mut size) };
    if status != 0 {
        return Err(status);
    }
    let bytes = size as usize;
    let mut storage = vec![0u64; bytes.div_ceil(std::mem::size_of::<u64>()).max(1)];
    // Safety：out 缓冲不小于 HAL 报告的长度，且 8 字节对齐。
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            addr,
            0,
            std::ptr::null(),
            &raw mut size,
            storage.as_mut_ptr().cast::<c_void>(),
        )
    };
    if status != 0 {
        return Err(status);
    }
    Ok((storage, bytes))
}

/// 聚合设备**输入作用域**的流对象列表。
fn read_input_streams(device: AudioObjectId) -> Result<Vec<AudioObjectId>, OsStatus> {
    let addr = AudioObjectPropertyAddress {
        selector: fourcc(b"stm#"), // kAudioDevicePropertyStreams
        scope: fourcc(b"inpt"),    // kAudioObjectPropertyScopeInput
        element: 0,
    };
    let (storage, bytes) = get_prop_var(device, &addr)?;
    let count = bytes / std::mem::size_of::<AudioObjectId>();
    // Safety：HAL 按 AudioObjectID 数组写入，长度取自其自报字节数。
    let ids =
        unsafe { std::slice::from_raw_parts(storage.as_ptr().cast::<AudioObjectId>(), count) };
    Ok(ids.to_vec())
}

/// 流的**虚拟格式**（= IOProc 实际看到的格式，与设备物理格式可以不同）。
fn read_stream_virtual_format(
    stream: AudioObjectId,
) -> Result<AudioStreamBasicDescription, OsStatus> {
    get_prop::<AudioStreamBasicDescription>(
        stream,
        // kAudioStreamPropertyVirtualFormat
        &AudioObjectPropertyAddress::global(fourcc(b"sfmt")),
    )
}

/// 设备输入作用域的 `AudioBufferList` 布局——**每条缓冲的声道数**，逐条按 IOProc 的顺序。
fn read_input_abl_channels(device: AudioObjectId) -> Result<Vec<u32>, OsStatus> {
    let addr = AudioObjectPropertyAddress {
        selector: fourcc(b"slay"), // kAudioDevicePropertyStreamConfiguration
        scope: fourcc(b"inpt"),
        element: 0,
    };
    let (storage, _) = get_prop_var(device, &addr)?;
    // Safety：HAL 按 AudioBufferList 写入；缓冲 8 字节对齐，条数取自结构自身。
    unsafe {
        let abl = storage.as_ptr().cast::<AudioBufferList>();
        let count = (*abl).number_buffers as usize;
        let first = std::ptr::addr_of!((*abl).buffers).cast::<AudioBuffer>();
        Ok((0..count)
            .map(|i| (*first.add(i)).number_channels)
            .collect())
    }
}

/// 校验聚合设备**实际交付**的输入格式与布局是否就是 tap 声明的那一个（ADR-082）。
///
/// 两道：① 输入流有且只有一条，其虚拟格式解析回来必须与 `declared` 全等；
/// ② IOProc 将收到的 ABL 形状必须等于该格式对应的 [`TapFormat::expected_abl`]。
/// 任一不符即 [`TapError::DeliveredMismatch`]——**不带病启动**。
fn verify_delivered_format(agg_id: AudioObjectId, declared: TapFormat) -> Result<(), TapError> {
    // 设 `tap#` 之后**流拓扑不是同步就绪的**：真机实测约每 3~4 次启动就有一次，紧接着
    // 读输入流会得到 0 条（HAL 还没把 tap 的流发布到聚合设备上）。把这一瞬当成"交付不符"
    // 会平白把系统轨降级掉——那是拿一个正确的校验制造出一个新的假阴性。故先**有界**等它
    // 落定：只把「一条都还没有」当作未就绪，一旦出现流就立刻严格比对（错误的拓扑不会
    // 因为多等而变对）。等不到就如实失败，绝不无限等在会话启动路径上。
    let deadline = std::time::Instant::now() + AGG_TOPOLOGY_SETTLE_TIMEOUT;
    let streams = loop {
        let streams = read_input_streams(agg_id).map_err(TapError::Aggregate)?;
        if !streams.is_empty() || std::time::Instant::now() >= deadline {
            break streams;
        }
        std::thread::sleep(AGG_TOPOLOGY_POLL_INTERVAL);
    };
    let [stream] = streams[..] else {
        return Err(TapError::DeliveredMismatch(format!(
            "聚合设备输入流 期望 1 条（仅 tap）实测 {} 条",
            streams.len()
        )));
    };
    let asbd = read_stream_virtual_format(stream).map_err(TapError::Aggregate)?;
    let delivered = tap_format_from_asbd(asbd)?;
    if delivered != declared {
        return Err(TapError::DeliveredMismatch(format!(
            "流格式 声明 {}Hz/{}ch/{:?} 实测 {}Hz/{}ch/{:?}",
            declared.sample_rate,
            declared.channels,
            declared.layout,
            delivered.sample_rate,
            delivered.channels,
            delivered.layout
        )));
    }
    let channels = read_input_abl_channels(agg_id).map_err(TapError::Aggregate)?;
    check_abl_layout(declared.expected_abl(), &channels)
}

/// 写 CFType 指针属性（sub device list / main sub device / tap list）。
fn set_cftype_prop(
    object: AudioObjectId,
    selector: u32,
    value_ref: &*const c_void,
) -> Result<(), OsStatus> {
    let addr = AudioObjectPropertyAddress::global(selector);
    // Safety：data 指向一个 CFTypeRef 槽位，size 为指针大小（HAL 属性约定）。
    let status = unsafe {
        AudioObjectSetPropertyData(
            object,
            &addr,
            0,
            std::ptr::null(),
            u32::try_from(std::mem::size_of::<*const c_void>()).unwrap_or(0),
            std::ptr::from_ref(value_ref).cast::<c_void>(),
        )
    };
    if status != 0 {
        return Err(status);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stereo_f32_asbd(flags: u32) -> AudioStreamBasicDescription {
        AudioStreamBasicDescription {
            sample_rate: 48_000.0,
            format_id: LINEAR_PCM_FORMAT,
            format_flags: flags,
            bytes_per_packet: 8,
            frames_per_packet: 1,
            bytes_per_frame: 8,
            channels_per_frame: 2,
            bits_per_channel: 32,
            reserved: 0,
        }
    }

    #[test]
    fn tap_format_accepts_interleaved_and_planar_float_stereo() {
        let interleaved =
            tap_format_from_asbd(stereo_f32_asbd(FORMAT_FLAG_FLOAT | FORMAT_FLAG_PACKED))
                .expect("interleaved f32 stereo");
        assert_eq!(interleaved.layout, TapSampleLayout::Interleaved);

        let planar = tap_format_from_asbd(stereo_f32_asbd(
            FORMAT_FLAG_FLOAT | FORMAT_FLAG_PACKED | FORMAT_FLAG_NON_INTERLEAVED,
        ))
        .expect("planar f32 stereo");
        assert_eq!(planar.layout, TapSampleLayout::PlanarStereo);
    }

    /// ADR-082 正向：交付布局与声明一致时放行。
    #[test]
    fn abl_layout_accepts_matching_delivery() {
        let interleaved =
            tap_format_from_asbd(stereo_f32_asbd(FORMAT_FLAG_FLOAT | FORMAT_FLAG_PACKED)).unwrap();
        assert_eq!(
            interleaved.expected_abl(),
            AblLayout {
                buffers: 1,
                channels_per_buffer: 2
            }
        );
        check_abl_layout(interleaved.expected_abl(), &[2]).expect("交错立体声 = 1 条 2 声道缓冲");

        let planar = tap_format_from_asbd(stereo_f32_asbd(
            FORMAT_FLAG_FLOAT | FORMAT_FLAG_PACKED | FORMAT_FLAG_NON_INTERLEAVED,
        ))
        .unwrap();
        assert_eq!(
            planar.expected_abl(),
            AblLayout {
                buffers: 2,
                channels_per_buffer: 1
            }
        );
        check_abl_layout(planar.expected_abl(), &[1, 1]).expect("平面立体声 = 2 条单声道缓冲");
    }

    /// **ADR-082 反向夹具**：这就是真机上发生过的那个布局。
    ///
    /// Both 档 mic 侧 VPIO（ADR-021）给默认输出设备装上一条 6 声道回声参考输入流，聚合设备
    /// 把它并进同一个 ABL 且排在 tap 之前 → `[6ch, 2ch]`。此前实现盲取 `buffers[0]`、按 tap
    /// 的 2 声道解释 6 声道数据，帧数恰好多算 3.00×（真机诊断 drift ≈ +2,002,000ppm，
    /// 90 秒领先 90 秒）。**把这个布局放回来必须变红**——本用例正是它变红的地方。
    #[test]
    fn abl_layout_rejects_vpio_reference_stream_ahead_of_tap() {
        let tap = tap_format_from_asbd(stereo_f32_asbd(FORMAT_FLAG_FLOAT | FORMAT_FLAG_PACKED))
            .expect("48k/2ch/f32 交错 tap");
        let err = check_abl_layout(tap.expected_abl(), &[6, 2])
            .expect_err("VPIO 参考流排在 tap 之前必须被检出，绝不放行");
        let TapError::DeliveredMismatch(detail) = err else {
            panic!("应判为交付不符");
        };
        assert!(
            detail.contains("6ch"),
            "诊断须点出实测布局（实得 {detail}）"
        );

        // 盲取 buffers[0] 会多算多少：6 声道的字节数按 2 声道计帧 = 3.00×，正是真机 drift。
        let bogus_frames = 6 * 512 / u32::from(tap.channels);
        assert_eq!(bogus_frames / 512, 3, "误计量倍率应为 3.00×");
    }

    /// 空 ABL / 声道数不齐 / 条数不对：一律不放行（不得挑一条像的凑合用）。
    #[test]
    fn abl_layout_rejects_degenerate_shapes() {
        let tap = tap_format_from_asbd(stereo_f32_asbd(FORMAT_FLAG_FLOAT | FORMAT_FLAG_PACKED))
            .expect("tap 格式");
        for actual in [vec![], vec![2, 2], vec![1, 1], vec![6]] {
            assert!(
                check_abl_layout(tap.expected_abl(), &actual).is_err(),
                "布局 {actual:?} 与交错立体声声明不符，必须拒绝"
            );
        }
    }

    #[test]
    fn tap_format_rejects_ambiguous_pcm() {
        let mut integer_pcm = stereo_f32_asbd(FORMAT_FLAG_PACKED);
        assert!(matches!(
            tap_format_from_asbd(integer_pcm),
            Err(TapError::UnsupportedFormat)
        ));
        integer_pcm.format_flags = FORMAT_FLAG_FLOAT | FORMAT_FLAG_PACKED;
        integer_pcm.channels_per_frame = 1;
        assert!(matches!(
            tap_format_from_asbd(integer_pcm),
            Err(TapError::UnsupportedFormat)
        ));
    }
}
