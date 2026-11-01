//! Whisper 运行后端解析：**进程外 Metal 探针 + 自动启用**（TDD-03 §4、TDD-13 §3.1）。
//!
//! 根因（用户实测「实时转写降级/不可用」横幅）：纯 CPU 推理在 Apple Silicon 上跑 small 及以上
//! 模型 RTF 逼近/超过 1.0，触发 [`luna_audio::DegradationController`] 升档。根治 = 启用 Metal
//! GPU。但 whisper.cpp 的 Metal 初始化在部分环境会 **native crash**（不可捕获的信号），进程内
//! 直接开会打崩主进程——故用**独立子进程**试跑：
//!
//! 1. 首次遇到某模型：spawn `current_exe --luna-probe-metal <model>`，子进程以 Metal 加载+微解码。
//! 2. 子进程 `exit(0)` = Metal 可用 → 持久化标记，之后进程内直接走 Metal。
//! 3. 子进程非零退出 / 被信号杀死 / 超时 = 不可用 → 安全回退 CPU，并记住结论（不再反复探测）。
//!
//! 结论按「模型文件指纹 + 探针版本」缓存于模型同目录 `metal_probe.txt`；模型或探针逻辑变更即失效。
//! 环境变量 `LUNA_WHISPER_BACKEND=cpu|metal|auto` 可强制覆盖（CI 确定性 / 安全逃生阀）。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use luna_engine::{WhisperBackend, WhisperConfig, WhisperProvider};

/// 子进程探针参数（`run()` 最先解析；命中即跑探针后 `exit`，绝不进入 GUI 初始化）。
const PROBE_ARG: &str = "--luna-probe-metal";

/// 探针逻辑/口径版本；变更（如换 whisper-rs 大版本或改判定）时 +1 使旧缓存失效。
const PROBE_VERSION: &str = "whisper-rs-0.14-metal-v1";

/// 缓存文件名（存于模型权重同目录）。
const CACHE_FILE: &str = "metal_probe.txt";

/// 子进程探针等待上限（大模型 GPU 加载可能数秒；超时判不可用回退 CPU）。
const PROBE_TIMEOUT: Duration = Duration::from_secs(120);

/// 编码器在场时的探针上限：首次加载要把 mlmodelc 编译到 ANE，大档可达数分钟（ADR-115）。
const COREML_PROBE_TIMEOUT: Duration = Duration::from_secs(600);

/// 若本进程是被 spawn 出来的 Metal 探针子进程，则执行探针并 `exit`（永不返回）。
///
/// 必须在 [`crate::run`] 最开头、任何 Tauri/窗口初始化之前调用。
pub fn run_probe_if_requested() {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some(PROBE_ARG) {
        return;
    }
    let Some(model) = args.next() else {
        std::process::exit(2); // 用法错误
    };
    // 子进程内真正试跑 Metal：Ok→0；优雅失败→1；原生崩溃→进程被信号杀死（父进程判非零）。
    match WhisperProvider::probe(PathBuf::from(model), WhisperBackend::Metal) {
        Ok(()) => std::process::exit(0),
        Err(_) => std::process::exit(1),
    }
}

/// 解析某模型应使用的运行后端：优先环境覆盖，其次缓存，最后进程外探针。
///
/// 仅 macOS 尝试 Metal；其它平台恒 CPU。任何异常路径都安全回退 [`WhisperBackend::Cpu`]。
#[must_use]
pub fn resolve_backend(model_path: &Path) -> WhisperBackend {
    // 1) 环境强制覆盖（逃生阀 / CI 确定性）。
    match std::env::var("LUNA_WHISPER_BACKEND").ok().as_deref() {
        Some("cpu") => return WhisperBackend::Cpu,
        Some("metal") => return WhisperBackend::Metal,
        _ => {}
    }

    // Metal 仅存在于 macOS；其它平台直接 CPU。
    if !cfg!(target_os = "macos") {
        return WhisperBackend::Cpu;
    }

    let key = fingerprint(model_path);

    // 2) 命中缓存直接用。
    if let Some(cached) = read_cache(model_path, &key) {
        return cached;
    }

    // 3) 进程外探针。
    let backend = probe_via_subprocess(model_path);
    write_cache(model_path, &key, backend);
    backend
}

/// **强制重探**（无视缓存）：CoreML 编码器刚装好 / 刚移除时调用——同一份权重加上编码器
/// 之后，whisper.cpp 在 `create_state` 里多了一段 ANE 编译与加载，此前的「Metal 可用」结论
/// 不覆盖它。指纹已含编码器在场与否，故普通路径下一次也会重探；这里只是把那一次提前到
/// 安装流程里，让「编译」阶段有人看着（PRD-14-020 转换进度）。
///
/// 返回 `true` = 探针通过（子进程正常退出）；`false` = 崩溃 / 超时 / 优雅失败。
pub fn reprobe(model_path: &Path) -> bool {
    if !cfg!(target_os = "macos") {
        return true;
    }
    let key = fingerprint(model_path);
    let backend = probe_via_subprocess(model_path);
    write_cache(model_path, &key, backend);
    backend == WhisperBackend::Metal
}

/// 权重旁是否有 CoreML 编码器目录（whisper.cpp 派生规则：`weights.bin` → `weights-encoder.mlmodelc`）。
fn coreml_encoder_beside(model_path: &Path) -> Option<std::fs::Metadata> {
    let mut name = model_path.file_stem()?.to_os_string();
    name.push("-encoder.mlmodelc");
    std::fs::metadata(model_path.with_file_name(name)).ok()
}

/// 以解析出的后端构造 [`WhisperConfig`]（各加载点统一入口，避免重复探测逻辑漂移）。
#[must_use]
pub fn config_for(model_path: &Path) -> WhisperConfig {
    WhisperConfig::new(model_path).with_backend(resolve_backend(model_path))
}

// ─── 内部 ─────────────────────────────────────────────────────────────────────

/// 模型指纹：探针版本 + 路径 + 文件长度 + mtime（任一变化即令缓存失效）。
fn fingerprint(model_path: &Path) -> String {
    let (len, mtime) = std::fs::metadata(model_path)
        .map(|m| {
            let mtime = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs());
            (m.len(), mtime)
        })
        .unwrap_or((0, 0));
    // 编码器在场与否（及其 mtime）进指纹：装上 / 移除 CoreML 编码器都要重探。
    let coreml = coreml_encoder_beside(model_path)
        .map(|m| {
            m.modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(1, |d| d.as_secs().max(1))
        })
        .unwrap_or(0);
    format!(
        "{PROBE_VERSION}|{}|{len}|{mtime}|coreml={coreml}",
        model_path.to_string_lossy()
    )
}

fn cache_path(model_path: &Path) -> Option<PathBuf> {
    model_path.parent().map(|dir| dir.join(CACHE_FILE))
}

/// 缓存文件两行：第一行指纹，第二行 backend（`metal`/`cpu`）。
fn read_cache(model_path: &Path, key: &str) -> Option<WhisperBackend> {
    let content = std::fs::read_to_string(cache_path(model_path)?).ok()?;
    let mut lines = content.lines();
    if lines.next()? != key {
        return None; // 指纹不符 → 失效
    }
    match lines.next()?.trim() {
        "metal" => Some(WhisperBackend::Metal),
        "cpu" => Some(WhisperBackend::Cpu),
        _ => None,
    }
}

fn write_cache(model_path: &Path, key: &str, backend: WhisperBackend) {
    let Some(path) = cache_path(model_path) else {
        return;
    };
    let tag = match backend {
        WhisperBackend::Metal => "metal",
        WhisperBackend::Cpu => "cpu",
    };
    // 写失败无害（下次重探），不阻断。
    let _ = std::fs::write(path, format!("{key}\n{tag}\n"));
}

/// spawn 自身探针子进程并按退出码判定后端；超时/错误一律 CPU。
fn probe_via_subprocess(model_path: &Path) -> WhisperBackend {
    let Ok(exe) = std::env::current_exe() else {
        return WhisperBackend::Cpu;
    };
    let child = std::process::Command::new(exe)
        .arg(PROBE_ARG)
        .arg(model_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(_) => return WhisperBackend::Cpu,
    };

    let timeout = if coreml_encoder_beside(model_path).is_some() {
        COREML_PROBE_TIMEOUT
    } else {
        PROBE_TIMEOUT
    };
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return if status.success() {
                    WhisperBackend::Metal
                } else {
                    WhisperBackend::Cpu
                };
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return WhisperBackend::Cpu;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return WhisperBackend::Cpu,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_override_forces_backend() {
        // SAFETY: 单测串行设置进程级 env；断言后清理。
        std::env::set_var("LUNA_WHISPER_BACKEND", "cpu");
        assert_eq!(
            resolve_backend(Path::new("/any/model.bin")),
            WhisperBackend::Cpu
        );
        std::env::set_var("LUNA_WHISPER_BACKEND", "metal");
        assert_eq!(
            resolve_backend(Path::new("/any/model.bin")),
            WhisperBackend::Metal
        );
        std::env::remove_var("LUNA_WHISPER_BACKEND");
    }

    #[test]
    fn cache_roundtrip_matches_on_same_fingerprint() {
        let dir = std::env::temp_dir().join(format!("luna-mp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let model = dir.join("weights.bin");
        std::fs::write(&model, b"stub").unwrap();
        let key = fingerprint(&model);
        write_cache(&model, &key, WhisperBackend::Metal);
        assert_eq!(read_cache(&model, &key), Some(WhisperBackend::Metal));
        // 指纹不符 → 失效。
        assert_eq!(read_cache(&model, "different-key"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
