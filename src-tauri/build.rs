use chrono::{SecondsFormat, TimeZone, Utc};
use std::{env, process::Command};

const BUILD_SHA_ENV: &str = "LUNARCA_GIT_SHA";
const BUILD_TIME_ENV: &str = "LUNARCA_BUILD_TIME";

fn assert_full_git_sha(sha: String) -> String {
    assert!(
        sha.len() == 40 && sha.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "{BUILD_SHA_ENV} must be a full 40-character hexadecimal Git SHA"
    );
    sha
}

fn required_git_sha() -> String {
    if let Ok(sha) = env::var(BUILD_SHA_ENV) {
        let sha = sha.trim();
        if !sha.is_empty() {
            return assert_full_git_sha(sha.to_owned());
        }
    }

    let output = Command::new("git")
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .expect("release builds require git or LUNARCA_GIT_SHA");
    assert!(
        output.status.success(),
        "release builds require a Git SHA; set {BUILD_SHA_ENV} when building outside a Git checkout"
    );
    assert_full_git_sha(
        String::from_utf8(output.stdout)
            .expect("git revision must be UTF-8")
            .trim()
            .to_owned(),
    )
}

fn rerun_on_git_head() {
    let mut refs = vec!["HEAD".to_owned(), "packed-refs".to_owned()];
    if let Ok(output) = Command::new("git")
        .args(["symbolic-ref", "--quiet", "HEAD"])
        .output()
    {
        if output.status.success() {
            refs.push(
                String::from_utf8(output.stdout)
                    .expect("git ref must be UTF-8")
                    .trim()
                    .to_owned(),
            );
        }
    }
    // HEAD stays unchanged when committing on the same branch; the branch ref moves.
    for git_ref in refs {
        if let Ok(output) = Command::new("git")
            .args(["rev-parse", "--git-path", &git_ref])
            .output()
        {
            if output.status.success() {
                let path = String::from_utf8(output.stdout).expect("git ref path must be UTF-8");
                println!("cargo:rerun-if-changed={}", path.trim());
            }
        }
    }
}

fn build_time() -> String {
    if let Ok(time) = env::var(BUILD_TIME_ENV) {
        let time = time.trim();
        if !time.is_empty() {
            return chrono::DateTime::parse_from_rfc3339(time)
                .expect("LUNARCA_BUILD_TIME must be an RFC 3339 timestamp")
                .with_timezone(&Utc)
                .to_rfc3339_opts(SecondsFormat::Secs, true);
        }
    }

    if let Ok(epoch) = env::var("SOURCE_DATE_EPOCH") {
        let seconds = epoch
            .parse::<i64>()
            .expect("SOURCE_DATE_EPOCH must be Unix seconds");
        return Utc
            .timestamp_opt(seconds, 0)
            .single()
            .expect("SOURCE_DATE_EPOCH must be a valid Unix timestamp")
            .to_rfc3339_opts(SecondsFormat::Secs, true);
    }

    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// 随包资源（转写档 PRD-14 §6、推理引擎 ADR-143）在不在。
///
/// `tauri_build::build()` 会因为 `bundle.resources` 指向的路径不存在而中断，报的是一句
/// 只说路径、不说怎么办的话。而这两份二进制都**刻意不入库**（见 `.gitignore` 里的理由），
/// 所以"新克隆的工作树第一次构建就撞上"是常态而不是异常——在这里先把那条命令说出来。
/// 以 `/` 结尾的键是目录（推理引擎）：本平台没有引擎包时取回脚本只建空目录，目录在即可。
fn assert_bundled_resources_present() {
    // 清单从打包配置里读，不在这里抄第二份路径——抄一份的代价是换档时这条检查仍在
    // 盯着上一档的文件名，于是它会在一个真正缺文件的构建上说"没问题"。
    let config: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string("tauri.conf.json").expect("读不到 tauri.conf.json"),
    )
    .expect("tauri.conf.json 不是合法 JSON");
    let Some(resources) = config["bundle"]["resources"].as_object() else {
        return;
    };
    for source in resources.keys() {
        let path = std::path::Path::new(source);
        let present = if source.ends_with('/') {
            path.is_dir()
        } else {
            path.is_file()
        };
        assert!(
            present,
            "缺少随包资源 src-tauri/{source}。\n\
             随包转写档与推理引擎都不入库（见 .gitignore 里的理由），构建前各取回一次：\n\
             \n    python3 scripts/fetch-bundled-asr.py\n    python3 scripts/fetch-llama-runtime.py\n\
             \n地址与 sha256 分别取自 crates/luna-model-catalog/bundled.json 里标了 bundled 的条目\n\
             与 crates/luna-engine/src/llama/runtime.rs 的 MACOS_ARM64。"
        );
    }
}

fn main() {
    println!("cargo:rerun-if-env-changed={BUILD_SHA_ENV}");
    println!("cargo:rerun-if-env-changed={BUILD_TIME_ENV}");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    rerun_on_git_head();
    println!("cargo:rustc-env=LUNARCA_GIT_SHA={}", required_git_sha());
    println!("cargo:rustc-env=LUNARCA_BUILD_TIME={}", build_time());
    assert_bundled_resources_present();
    tauri_build::build();
}
