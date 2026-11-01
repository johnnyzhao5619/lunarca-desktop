//! macOS 语音处理输入——Voice Processing I/O（AUVoiceIO，`kAudioUnitSubType_VoiceProcessingIO`）。
//! 事实源：ADR-021、[TDD-02 §3.1-I 层③]；实测依据：S14 spike
//! （S14 VPIO spike，内部记录：外放泄漏消除 ~24dB、whisper 零泄漏、
//! 16k mono 直出、AGC 可关、静音期非精确零不误触数字零看门狗）。
//!
//! **用途**：Both 双轨（会议）模式的 mic 采集主力——回声消除在**系统层**完成（VPIO 的
//! 消除范围明确覆盖「其他应用音频」，按机型声学调音、FaceTime 同源），mic 轨不再混入
//! 外放的对方声音；应用侧零 AEC 计算。**Mic 单源模式不用本模块**（无回声问题，cpal
//! 原始采集保留保真，不套语音处理）。
//!
//! **绑定方式**：AudioUnit C API 自古稳定（10.7+），**强链接 AudioToolbox.framework**
//! ——区别于 CATap（14.2+ 新符号须 dlsym 弱绑定）。
//!
//! **双工形态（S14 实测）**：输入 element 1 开启 + 输出 element 0 保持默认开启并挂
//! **静音渲染回调**（不经 VPIO 播任何声音；回声参考 = 系统 "other audio"）。
//!
//! **回调铁律（ADR-009 同款）**：输入回调内仅 `AudioUnitRender` 到预分配 scratch +
//! [`FrameInjector::push_f32`] 无锁灌帧——禁分配/锁/IO/日志。scratch 归回调独占
//! （单实时线程，经裸指针独占访问，无锁）。
//!
//! **TCC**：与 cpal 同一「麦克风」权限，无新增授权面。
#![allow(non_snake_case, non_upper_case_globals)]

use luna_audio::FrameInjector;
use std::ffi::c_void;

// ---------- AudioToolbox 最小 FFI（仅本模块使用的面；类型对齐 AudioUnit.h） ----------

type OsStatus = i32;
type AudioUnit = *mut c_void;

#[repr(C)]
struct AudioComponentDescription {
    component_type: u32,
    component_sub_type: u32,
    component_manufacturer: u32,
    component_flags: u32,
    component_flags_mask: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
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
struct SmpteTime {
    subframes: i16,
    subframe_divisor: i16,
    counter: u32,
    time_type: u32,
    flags: u32,
    hours: i16,
    minutes: i16,
    seconds: i16,
    frames: i16,
}

#[repr(C)]
struct AudioTimeStamp {
    sample_time: f64,
    host_time: u64,
    rate_scalar: f64,
    word_clock_time: u64,
    smpte_time: SmpteTime,
    flags: u32,
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

type AuRenderCallback = extern "C" fn(
    ref_con: *mut c_void,
    io_action_flags: *mut u32,
    in_time_stamp: *const AudioTimeStamp,
    in_bus_number: u32,
    in_number_frames: u32,
    io_data: *mut AudioBufferList,
) -> OsStatus;

#[repr(C)]
struct AuRenderCallbackStruct {
    input_proc: AuRenderCallback,
    input_proc_ref_con: *mut c_void,
}

/// `AUVoiceIOOtherAudioDuckingConfiguration`（macOS 14+）：Boolean(1B) + 3B pad + u32。
#[repr(C)]
struct AuVoiceIoOtherAudioDuckingConfiguration {
    enable_advanced_ducking: u8,
    ducking_level: u32,
}

#[link(name = "AudioToolbox", kind = "framework")]
unsafe extern "C" {
    fn AudioComponentFindNext(
        in_component: *mut c_void,
        in_desc: *const AudioComponentDescription,
    ) -> *mut c_void;
    fn AudioComponentInstanceNew(
        in_component: *mut c_void,
        out_instance: *mut AudioUnit,
    ) -> OsStatus;
    fn AudioComponentInstanceDispose(in_instance: AudioUnit) -> OsStatus;
    fn AudioUnitSetProperty(
        in_unit: AudioUnit,
        in_id: u32,
        in_scope: u32,
        in_element: u32,
        in_data: *const c_void,
        in_data_size: u32,
    ) -> OsStatus;
    fn AudioUnitInitialize(in_unit: AudioUnit) -> OsStatus;
    fn AudioUnitUninitialize(in_unit: AudioUnit) -> OsStatus;
    fn AudioOutputUnitStart(ci: AudioUnit) -> OsStatus;
    fn AudioOutputUnitStop(ci: AudioUnit) -> OsStatus;
    fn AudioUnitRender(
        in_unit: AudioUnit,
        io_action_flags: *mut u32,
        in_time_stamp: *const AudioTimeStamp,
        in_output_bus_number: u32,
        in_number_frames: u32,
        io_data: *mut AudioBufferList,
    ) -> OsStatus;
}

const fn fourcc(b: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*b)
}

const AU_TYPE_OUTPUT: u32 = fourcc(b"auou");
const AU_SUBTYPE_VPIO: u32 = fourcc(b"vpio");
const AU_MANUFACTURER_APPLE: u32 = fourcc(b"appl");
const FORMAT_LINEAR_PCM: u32 = fourcc(b"lpcm");
/// float | packed（f32 交错）。
const FORMAT_FLAGS_F32_PACKED: u32 = 0x9;
const PROP_STREAM_FORMAT: u32 = 8; // kAudioUnitProperty_StreamFormat
const PROP_MAX_FRAMES_PER_SLICE: u32 = 14; // kAudioUnitProperty_MaximumFramesPerSlice
const PROP_SET_RENDER_CALLBACK: u32 = 23; // kAudioUnitProperty_SetRenderCallback
const PROP_ENABLE_IO: u32 = 2003; // kAudioOutputUnitProperty_EnableIO
const PROP_SET_INPUT_CALLBACK: u32 = 2005; // kAudioOutputUnitProperty_SetInputCallback
const PROP_VPIO_BYPASS: u32 = 2100; // kAUVoiceIOProperty_BypassVoiceProcessing
const PROP_VPIO_ENABLE_AGC: u32 = 2101; // kAUVoiceIOProperty_VoiceProcessingEnableAGC
const PROP_VPIO_OTHER_AUDIO_DUCKING: u32 = 2108; // kAUVoiceIOProperty_OtherAudioDuckingConfiguration
/// `kAUVoiceIOOtherAudioDuckingLevelMin`：最小压低——对方轨（CATap 采集的系统播放）
/// 是录音内容本体，绝不能被 VPIO 大幅压低（S14 验收项）。
const DUCKING_LEVEL_MIN: u32 = 10;
const SCOPE_GLOBAL: u32 = 0;
const SCOPE_INPUT: u32 = 1;
const SCOPE_OUTPUT: u32 = 2;
const BUS_OUTPUT: u32 = 0;
const BUS_INPUT: u32 = 1;
/// 单次回调帧数上限（经 MaximumFramesPerSlice 声明；scratch 预分配至此，回调零分配）。
const MAX_FRAMES_PER_SLICE: u32 = 4096;

/// VPIO 输出采样率/声道（S14 实测 16k mono 直接协商成功——管线 passthrough 零重采样）。
pub const VPIO_SAMPLE_RATE: u32 = 16_000;
pub const VPIO_CHANNELS: u16 = 1;

/// VPIO 错误（脱敏：仅 OSStatus/类别，无内容）。
#[derive(Debug, thiserror::Error)]
pub enum VpioError {
    /// 组件不存在（理论上 10.7+ 恒在；防御性）。
    #[error("the vpio audio component is unavailable")]
    ComponentUnavailable,
    /// AudioUnit 调用失败。
    #[error("vpio {0} failed: OSStatus {1}")]
    Os(&'static str, i32),
}

/// 输入回调上下文：回调线程**独占**（单实时线程经裸指针访问，无锁；ADR-009）。
struct CallbackCtx {
    unit: AudioUnit,
    injector: FrameInjector,
    scratch: Vec<f32>,
}

extern "C" fn input_cb(
    ref_con: *mut c_void,
    io_action_flags: *mut u32,
    in_time_stamp: *const AudioTimeStamp,
    in_bus_number: u32,
    in_number_frames: u32,
    _io_data: *mut AudioBufferList,
) -> OsStatus {
    // SAFETY：ref_con 在 Start 前设置、Stop 后才释放；回调为单实时线程独占访问。
    let ctx = unsafe { &mut *(ref_con as *mut CallbackCtx) };
    // 帧数超 scratch 容量（已声明 MaximumFramesPerSlice，理论不发生）→ 丢弃本回调
    // （绝不在实时回调内分配；FrameInjector 侧环形缓冲溢出同款诚实丢弃语义）。
    let n = in_number_frames as usize;
    if n > ctx.scratch.len() {
        return 0;
    }
    let mut abl = AudioBufferList {
        number_buffers: 1,
        buffers: [AudioBuffer {
            number_channels: u32::from(VPIO_CHANNELS),
            data_byte_size: in_number_frames * 4,
            data: ctx.scratch.as_mut_ptr() as *mut c_void,
        }],
    };
    let st = unsafe {
        AudioUnitRender(
            ctx.unit,
            io_action_flags,
            in_time_stamp,
            in_bus_number,
            in_number_frames,
            &mut abl,
        )
    };
    if st == 0 {
        ctx.injector.push_f32(&ctx.scratch[..n]);
    }
    st
}

extern "C" fn silent_render_cb(
    _ref_con: *mut c_void,
    _io_action_flags: *mut u32,
    _in_time_stamp: *const AudioTimeStamp,
    _in_bus_number: u32,
    _in_number_frames: u32,
    io_data: *mut AudioBufferList,
) -> OsStatus {
    // 输出静音（双工形态要求输出路径存在；我们不经 VPIO 播任何声音）。
    unsafe {
        let abl = &mut *io_data;
        let buffers = std::ptr::addr_of_mut!(abl.buffers) as *mut AudioBuffer;
        for i in 0..abl.number_buffers as usize {
            let b = &mut *buffers.add(i);
            std::ptr::write_bytes(b.data as *mut u8, 0, b.data_byte_size as usize);
        }
    }
    0
}

/// 运行中的 VPIO 采集（RAII：drop 即停 unit + 释放回调上下文）。
///
/// 停止顺序契约（与 CATap 同款）：**先 drop 本守卫**（unit 停止、不再灌帧），
/// 再停 `CaptureHandle` 收尾 flush/落盘。
pub struct RunningVoiceProcessInput {
    unit: AudioUnit,
    ctx: *mut CallbackCtx,
}

// SAFETY：unit/ctx 仅在 drop 时从持有线程访问（回调已由 AudioOutputUnitStop 停止）。
unsafe impl Send for RunningVoiceProcessInput {}

impl Drop for RunningVoiceProcessInput {
    fn drop(&mut self) {
        // SAFETY：标准 AudioUnit 拆除序列；Stop 返回后输入回调不再触发，
        // 此后释放 ctx 无竞态。
        unsafe {
            let _ = AudioOutputUnitStop(self.unit);
            let _ = AudioUnitUninitialize(self.unit);
            let _ = AudioComponentInstanceDispose(self.unit);
            drop(Box::from_raw(self.ctx));
        }
    }
}

fn os(st: OsStatus, what: &'static str) -> Result<(), VpioError> {
    if st == 0 {
        Ok(())
    } else {
        Err(VpioError::Os(what, st))
    }
}

/// 启动 VPIO 麦克风采集：16k mono f32 经 `injector` 无锁灌入消费管线。
///
/// 配置口径（S14）：AGC 关（录音响度交混流收口，不做采集期增益）；bypass 关（启用
/// 语音处理=回声消除本体）；other-audio ducking 设为最小压低（macOS 14+ 属性，失败
/// 容忍——对方轨响度是录音内容，绝不允许被大幅压低）。
///
/// # Errors
/// 组件缺失/任一 AudioUnit 调用失败返回 [`VpioError`]（调用方回退 cpal + AEC3）。
pub fn start_voice_process_input(
    injector: FrameInjector,
) -> Result<RunningVoiceProcessInput, VpioError> {
    // SAFETY：本函数按 AudioUnit 标准装配序列调用 C API；所有指针入参为栈上有效对象；
    // ctx 所有权经 Box::into_raw 移交给运行期，Drop 中回收。
    unsafe {
        let desc = AudioComponentDescription {
            component_type: AU_TYPE_OUTPUT,
            component_sub_type: AU_SUBTYPE_VPIO,
            component_manufacturer: AU_MANUFACTURER_APPLE,
            component_flags: 0,
            component_flags_mask: 0,
        };
        let comp = AudioComponentFindNext(std::ptr::null_mut(), &desc);
        if comp.is_null() {
            return Err(VpioError::ComponentUnavailable);
        }
        let mut unit: AudioUnit = std::ptr::null_mut();
        os(AudioComponentInstanceNew(comp, &mut unit), "InstanceNew")?;
        // 从此任何失败都须 dispose unit——集中经闭包 + 手动清理。
        let cleanup = |unit: AudioUnit| {
            let _ = AudioComponentInstanceDispose(unit);
        };

        let enable: u32 = 1;
        if let Err(e) = os(
            AudioUnitSetProperty(
                unit,
                PROP_ENABLE_IO,
                SCOPE_INPUT,
                BUS_INPUT,
                (&raw const enable).cast(),
                4,
            ),
            "EnableIO(input)",
        ) {
            cleanup(unit);
            return Err(e);
        }

        let asbd = AudioStreamBasicDescription {
            sample_rate: f64::from(VPIO_SAMPLE_RATE),
            format_id: FORMAT_LINEAR_PCM,
            format_flags: FORMAT_FLAGS_F32_PACKED,
            bytes_per_packet: 4,
            frames_per_packet: 1,
            bytes_per_frame: 4,
            channels_per_frame: u32::from(VPIO_CHANNELS),
            bits_per_channel: 32,
            reserved: 0,
        };
        let asbd_size = std::mem::size_of::<AudioStreamBasicDescription>() as u32;
        // 输入总线的输出侧（我们读到的格式）+ 输出总线的输入侧（静音渲染格式）。
        for (scope, bus, what) in [
            (SCOPE_OUTPUT, BUS_INPUT, "StreamFormat(input)"),
            (SCOPE_INPUT, BUS_OUTPUT, "StreamFormat(output)"),
        ] {
            if let Err(e) = os(
                AudioUnitSetProperty(
                    unit,
                    PROP_STREAM_FORMAT,
                    scope,
                    bus,
                    (&raw const asbd).cast(),
                    asbd_size,
                ),
                what,
            ) {
                cleanup(unit);
                return Err(e);
            }
        }
        let max_frames: u32 = MAX_FRAMES_PER_SLICE;
        let _ = AudioUnitSetProperty(
            unit,
            PROP_MAX_FRAMES_PER_SLICE,
            SCOPE_GLOBAL,
            0,
            (&raw const max_frames).cast(),
            4,
        );

        // AGC 关（录音响度归混流收口）；bypass 关（语音处理=回声消除本体）。
        let zero: u32 = 0;
        let _ = AudioUnitSetProperty(
            unit,
            PROP_VPIO_ENABLE_AGC,
            SCOPE_GLOBAL,
            BUS_INPUT,
            (&raw const zero).cast(),
            4,
        );
        let _ = AudioUnitSetProperty(
            unit,
            PROP_VPIO_BYPASS,
            SCOPE_GLOBAL,
            BUS_INPUT,
            (&raw const zero).cast(),
            4,
        );
        // other-audio ducking → 最小压低（macOS 14+；失败容忍但记录——对方轨是录音内容）。
        let ducking = AuVoiceIoOtherAudioDuckingConfiguration {
            enable_advanced_ducking: 0,
            ducking_level: DUCKING_LEVEL_MIN,
        };
        let duck_st = AudioUnitSetProperty(
            unit,
            PROP_VPIO_OTHER_AUDIO_DUCKING,
            SCOPE_GLOBAL,
            0,
            (&raw const ducking).cast(),
            std::mem::size_of::<AuVoiceIoOtherAudioDuckingConfiguration>() as u32,
        );
        if duck_st != 0 {
            eprintln!(
                "[vpio] ducking 配置不受支持（OSStatus {duck_st}，macOS <14 预期）——沿用系统默认。"
            );
        }

        let ctx = Box::into_raw(Box::new(CallbackCtx {
            unit,
            injector,
            scratch: vec![0.0; MAX_FRAMES_PER_SLICE as usize],
        }));
        let free_ctx = |ctx: *mut CallbackCtx| drop(Box::from_raw(ctx));

        let in_cb = AuRenderCallbackStruct {
            input_proc: input_cb,
            input_proc_ref_con: ctx.cast(),
        };
        if let Err(e) = os(
            AudioUnitSetProperty(
                unit,
                PROP_SET_INPUT_CALLBACK,
                SCOPE_GLOBAL,
                0,
                (&raw const in_cb).cast(),
                std::mem::size_of::<AuRenderCallbackStruct>() as u32,
            ),
            "SetInputCallback",
        ) {
            free_ctx(ctx);
            cleanup(unit);
            return Err(e);
        }
        let render_cb = AuRenderCallbackStruct {
            input_proc: silent_render_cb,
            input_proc_ref_con: std::ptr::null_mut(),
        };
        if let Err(e) = os(
            AudioUnitSetProperty(
                unit,
                PROP_SET_RENDER_CALLBACK,
                SCOPE_INPUT,
                BUS_OUTPUT,
                (&raw const render_cb).cast(),
                std::mem::size_of::<AuRenderCallbackStruct>() as u32,
            ),
            "SetRenderCallback",
        ) {
            free_ctx(ctx);
            cleanup(unit);
            return Err(e);
        }

        if let Err(e) = os(AudioUnitInitialize(unit), "Initialize") {
            free_ctx(ctx);
            cleanup(unit);
            return Err(e);
        }
        if let Err(e) = os(AudioOutputUnitStart(unit), "Start") {
            let _ = AudioUnitUninitialize(unit);
            free_ctx(ctx);
            cleanup(unit);
            return Err(e);
        }
        Ok(RunningVoiceProcessInput { unit, ctx })
    }
}
