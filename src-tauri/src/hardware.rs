//! 设备算力检测与档位划分（PRD-14-010/011，TDD-04 §4）。
//!
//! 职责：采集芯片/内存/核心数/系统版本（`sysinfo`），按内存划分设备档位，并给出
//! 推荐的 whisper 档 + 对话模型（随包目录条目 id；映射单一来源在 [`crate::model_store`]）。
//! 档位推导为纯逻辑、headless 可单测；首启引导据此防止低配设备下载跑不动的大模型。
//!
//! 隐私：仅设备级硬件元数据（芯片型号/内存量级），绝不含任何用户内容或可识别序列号。

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::cloud::ComputeLocation;
use crate::model_store::{recommended_asr_for, recommended_llm_for, DeviceTier};

const GIB: u64 = 1024 * 1024 * 1024;

/// 设备算力摘要（命令响应；首启引导「硬件检测」步骤渲染）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct HardwareSummaryDto {
    /// 芯片/CPU 型号（如 "Apple M3"）。
    pub chip: String,
    /// CPU 架构（如 "arm64"）。
    pub arch: String,
    /// 逻辑核心数。
    pub logical_cores: u32,
    /// 物理内存字节数（f64 供 JS number 消费，字节数远不及 2^53）。
    pub memory_bytes: f64,
    /// 操作系统与版本（如 "macOS 15.5"）。
    pub os: String,
    /// 推导出的设备档位。
    pub tier: DeviceTier,
    /// 推荐的 LunaManaged whisper 模型 id（用于生成模型安装计划）。
    pub recommended_asr_model_id: String,
    /// 推荐 whisper 档展示名。
    pub recommended_asr_model_name: String,
    /// 推荐 whisper 档下载体积（字节，下载前披露，PRD-14-020）。
    pub recommended_asr_size_bytes: f64,
    /// 推荐的对话模型（随包目录条目 id，如 `qwen3.5-2b-q8_0-gguf`）。服务端形态恒为最小兜底档，完全本机
    /// 形态才按设备性能推荐主力档。
    pub recommended_llm_model: String,
}

/// 按物理内存划分设备档位（纯逻辑，PRD-14-010）。
///
/// > 假设：以内存为唯一划档信号——<8GiB 低配、8–16GiB 标准、≥16GiB 高配（理由：Apple Silicon
/// > 统一内存下 RAM 同时约束 ASR/LLM 权重驻留，是"跑不跑得动"的第一瓶颈；核数/GPU 差异在同代
/// > 芯片内远小于内存差异，TDD-04 §4 亦以 RAM 为准入信号 `min_ram_gb`）。
#[must_use]
pub fn derive_tier(memory_bytes: u64) -> DeviceTier {
    if memory_bytes < 8 * GIB {
        DeviceTier::Low
    } else if memory_bytes < 16 * GIB {
        DeviceTier::Standard
    } else {
        DeviceTier::High
    }
}

/// 只取设备档位。给只关心「这台机器算哪一档」的调用方（如对话上下文上限）用，
/// 免得为一个枚举值把芯片型号、核数、系统版本全采一遍。
#[must_use]
pub fn probe_tier() -> DeviceTier {
    // 进程级缓存：内存总量在一次运行里不会变，而 `setup_status` 每次窗口重新获得焦点都要
    // 问一遍档位（用户去系统设置授完权切回来就是一次）。采样本身不贵，但它是一次系统调用，
    // 而它的答案在这台机器上从开机起就定死了。
    static TIER: std::sync::OnceLock<DeviceTier> = std::sync::OnceLock::new();
    *TIER.get_or_init(|| {
        let mut sys = sysinfo::System::new();
        sys.refresh_memory();
        derive_tier(sys.total_memory())
    })
}

/// 采集本机硬件摘要，并给出**这一种算力落点下**的本机模型推荐。
///
/// 推荐吃 `location` 而不是只吃档位：算力在服务端时本机模型只是断线兜底，推荐档
/// 因此完全不同（见 [`recommended_asr_for`]）。此前这里只看硬件，于是一台连着服务端的
/// 高配机器仍被引导下载 1.6GB 的大档——为一个但愿用不上的兜底。
#[must_use]
pub fn probe(location: ComputeLocation) -> HardwareSummaryDto {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    sys.refresh_cpu_list(sysinfo::CpuRefreshKind::nothing());

    let memory_bytes = sys.total_memory();
    let tier = derive_tier(memory_bytes);
    let asr = recommended_asr_for(tier, location);
    let chip = sys
        .cpus()
        .first()
        .map(|cpu| cpu.brand().trim().to_owned())
        .filter(|brand| !brand.is_empty())
        .unwrap_or_else(sysinfo::System::cpu_arch);

    HardwareSummaryDto {
        chip,
        arch: sysinfo::System::cpu_arch(),
        logical_cores: u32::try_from(sys.cpus().len()).unwrap_or(0),
        memory_bytes: memory_bytes as f64,
        os: sysinfo::System::long_os_version()
            .unwrap_or_else(|| sysinfo::System::name().unwrap_or_default()),
        tier,
        recommended_asr_model_id: asr.id.to_owned(),
        recommended_asr_model_name: asr.name.to_owned(),
        recommended_asr_size_bytes: asr.weights.size_bytes as f64,
        recommended_llm_model: recommended_llm_for(tier, location).to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier_thresholds_split_at_8_and_16_gib() {
        assert_eq!(derive_tier(4 * GIB), DeviceTier::Low);
        assert_eq!(derive_tier(8 * GIB - 1), DeviceTier::Low);
        assert_eq!(derive_tier(8 * GIB), DeviceTier::Standard);
        assert_eq!(derive_tier(16 * GIB - 1), DeviceTier::Standard);
        assert_eq!(derive_tier(16 * GIB), DeviceTier::High);
        assert_eq!(derive_tier(64 * GIB), DeviceTier::High);
    }

    /// 档位在一次运行里恒定：缓存过的答案与现采的必须一致，且反复问不改口。
    ///
    /// 这条同时是缓存的**正确性**判据——如果哪天有人把随运行变化的东西（可用内存、
    /// 电源状态）塞进 `derive_tier` 的输入，它会在这里以"两次结果不同"的形式现形。
    #[test]
    fn device_tier_is_stable_within_a_process() {
        let first = probe_tier();
        assert_eq!(first, probe_tier());
        let mut sys = sysinfo::System::new();
        sys.refresh_memory();
        assert_eq!(first, derive_tier(sys.total_memory()));
    }

    #[test]
    fn probe_returns_consistent_recommendation() {
        let summary = probe(ComputeLocation::OnDevice);
        assert!(summary.memory_bytes > 0.0);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let memory_bytes = summary.memory_bytes as u64;
        assert_eq!(summary.tier, derive_tier(memory_bytes));
        assert_eq!(
            summary.recommended_asr_model_id,
            recommended_asr_for(summary.tier, ComputeLocation::OnDevice).id
        );
        assert!(!summary.recommended_llm_model.is_empty());
        assert_eq!(summary.tier, probe_tier());
    }

    /// 算力在服务端时推荐必须落到**内置 tiny**（下载量为零）——本机模型此时只是
    /// 断线兜底。这条断言拦的是「高配机器连着服务端，却被引导下载 1.6GB 大档」。
    #[test]
    fn server_compute_recommends_the_bundled_fallback_not_a_big_download() {
        for location in [
            ComputeLocation::OfficialCloud,
            ComputeLocation::Organization,
        ] {
            let summary = probe(location);
            assert_eq!(
                summary.recommended_asr_model_id,
                crate::model_store::BUNDLED_WHISPER_ID,
                "{location:?} 下应推荐内置兜底档"
            );
        }
    }

    /// 反向：用户选了「完全在本机跑」时，档位推荐必须还在——否则产品质量被钉死在最低档。
    #[test]
    fn on_device_compute_still_recommends_by_tier() {
        for location in [ComputeLocation::OnDevice, ComputeLocation::StrictLocal] {
            let summary = probe(location);
            assert_eq!(
                summary.recommended_asr_model_id,
                crate::model_store::recommended_asr_for_tier(summary.tier).id,
                "{location:?} 下应按设备档位推荐"
            );
            assert!(!summary.recommended_llm_model.is_empty());
        }
    }

    /// 算力在服务端时只推荐最小本机 LLM，不能按高配机器推荐主力大模型。
    #[test]
    fn server_compute_recommends_the_smallest_local_llm_fallback() {
        for location in [
            ComputeLocation::OfficialCloud,
            ComputeLocation::Organization,
        ] {
            assert_eq!(
                probe(location).recommended_llm_model,
                crate::model_store::recommended_llm_for_tier(DeviceTier::Low),
                "{location:?} 下必须只推荐最小本机兜底"
            );
        }
    }
}
