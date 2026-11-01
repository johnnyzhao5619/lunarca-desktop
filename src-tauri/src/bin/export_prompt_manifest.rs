//! 导出能力提示词的**必备模板清单**（manifest）为 JSON（ADR-050 修订 2026-09-04）。
//!
//! 唯一事实源是 `luna_core::prompt::REQUIRED_TEMPLATES` + `PROMPT_MANIFEST_VERSION`；本工具
//! 只是它的投影。产物 `crates/luna-core/prompts/manifest.json` 随包嵌入并由单测
//! `manifest_keys_are_monotonic_and_checked_in` 钉住与代码一致；服务端发布校验读的是这份
//! JSON 的副本（商业仓 `cp-domain`），不再手抄第二份清单。
//!
//! 改了 `REQUIRED_TEMPLATES` 之后：
//!
//! ```bash
//! cargo run -p luna-app --bin export-prompt-manifest > crates/luna-core/prompts/manifest.json
//! ```
//!
//! 这里没有任何用户的东西：清单只有模板**名字**与版本号，不含模板正文。

fn main() {
    let json =
        serde_json::to_string_pretty(&luna_core::prompt_manifest()).expect("序列化 manifest 失败");
    println!("{json}");
}
