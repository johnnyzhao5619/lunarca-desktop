//! 随包推理引擎的身份与文件清单（ADR-143 ①）——**唯一事实源**。
//!
//! `scripts/fetch-llama-runtime.py` 从这里读出构建号、包地址、sha256 与文件清单去取回并
//! 校验，`scripts/check-llama-runtime-wiring.py` 校验打包配置与 `.gitignore` 跟它对齐；
//! 换引擎版本只改这一处。二进制不入库（AGPL 仓库的历史背上它就卸不掉）。
//!
//! 清单只有 `llama-server` 与 `llama-fit-params` 递归依赖的那些 dylib（发布包 37 个里的
//! 11 个），按 `@rpath` 引用的 soname 落盘（发布包里是符号链接，取回时解引用）。

use std::path::{Path, PathBuf};

/// 一个平台的引擎发布包。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeSpec {
    /// llama.cpp 构建号（稳定标签 `v0.6.0` 对应的构建）。
    pub build: &'static str,
    /// 官方发布包地址。
    pub archive_url: &'static str,
    /// 发布包 sha256。
    pub archive_sha256: &'static str,
    /// 发布包字节数。
    pub archive_size: u64,
    /// 包内顶层目录。
    pub archive_root: &'static str,
    /// 要取出的文件（soname / 可执行名），全部落在资源目录 `llama/` 下。
    pub files: &'static [&'static str],
}

/// 随包资源里引擎目录的相对路径（`bundle.resources` 的目标）。
pub const RESOURCE_DIR: &str = "llama";
/// 应用的最低 macOS 版本 = 引擎包的 `minos`（ADR-143 ①：由 10.15 升到 13.3）。
/// `tauri.conf.json` 的 `minimumSystemVersion` 与 `.cargo/config.toml` 的部署目标必须与它一致
/// （`scripts/check-llama-runtime-wiring.py`）：低于它的系统上应用能启动、引擎却在 dyld 阶段就退出。
pub const MIN_MACOS: &str = "13.3";
/// 路由器可执行文件名。
pub const SERVER_BIN: &str = "llama-server";
/// 内存估算工具可执行文件名（[`super::budget::FitParamsEstimator`]）。
pub const FIT_PARAMS_BIN: &str = "llama-fit-params";

/// macOS Apple Silicon 的引擎包。最低系统 13.3 来自它的 `minos`（ADR-143 ①）。
pub const MACOS_ARM64: RuntimeSpec = RuntimeSpec {
    build: "b11429",
    archive_url: "https://github.com/ggml-org/llama.cpp/releases/download/b11429/llama-b11429-bin-macos-arm64.tar.gz",
    archive_sha256: "740288ec6887be94280a5dfa25b5e23a78285cab104519e6c7e218904ee82459",
    archive_size: 11_971_406,
    archive_root: "llama-b11429",
    files: &[
        "LICENSE",
        "llama-server",
        "llama-fit-params",
        "libllama-server-impl.dylib",
        "libllama-fit-params-impl.dylib",
        "libllama-common.0.dylib",
        "libllama.0.dylib",
        "libmtmd.0.dylib",
        "libggml.0.dylib",
        "libggml-base.0.dylib",
        "libggml-cpu.0.dylib",
        "libggml-blas.0.dylib",
        "libggml-metal.0.dylib",
        "libggml-rpc.0.dylib",
    ],
};

/// 本构建目标随包的引擎；没有的平台返回 `None`（界面如实说「本平台暂无本地引擎」）。
#[must_use]
pub fn current() -> Option<&'static RuntimeSpec> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some(&MACOS_ARM64)
    } else {
        None
    }
}

/// 资源目录里定位到的引擎。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePaths {
    /// 构建号。
    pub build: &'static str,
    /// `llama-server` 绝对路径。
    pub server: PathBuf,
    /// `llama-fit-params` 绝对路径。
    pub fit_params: PathBuf,
}

/// 在资源根目录下定位引擎：本平台有引擎包、且清单里每个文件都在场才算找到。
///
/// 缺任何一个 dylib，`llama-server` 都会在 dyld 阶段直接退出——与其让监管器拉起后
/// 熔断，不如在这里就说「引擎文件不完整」。
#[must_use]
pub fn locate(resource_root: &Path) -> Option<RuntimePaths> {
    let spec = current()?;
    let dir = resource_root.join(RESOURCE_DIR);
    spec.files
        .iter()
        .all(|f| dir.join(f).is_file())
        .then(|| RuntimePaths {
            build: spec.build,
            server: dir.join(SERVER_BIN),
            fit_params: dir.join(FIT_PARAMS_BIN),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_includes_both_executables_and_the_license() {
        for f in [SERVER_BIN, FIT_PARAMS_BIN, "LICENSE"] {
            assert!(MACOS_ARM64.files.contains(&f), "{f}");
        }
        assert!(MACOS_ARM64.archive_url.contains(MACOS_ARM64.build));
        assert_eq!(MACOS_ARM64.archive_sha256.len(), 64);
    }

    #[test]
    fn locate_requires_every_listed_file() {
        let root = std::env::temp_dir().join(format!("luna-runtime-locate-{}", std::process::id()));
        let dir = root.join(RESOURCE_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        for f in MACOS_ARM64.files {
            std::fs::write(dir.join(f), b"").unwrap();
        }
        if current().is_some() {
            let found = locate(&root).unwrap();
            assert_eq!(found.server, dir.join(SERVER_BIN));
            std::fs::remove_file(dir.join("libggml-metal.0.dylib")).unwrap();
            assert_eq!(locate(&root), None, "a missing dylib means no engine");
        } else {
            assert_eq!(locate(&root), None);
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
