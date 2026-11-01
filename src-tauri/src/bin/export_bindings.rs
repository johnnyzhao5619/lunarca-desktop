//! Headless 生成 TS 绑定：`cargo run -p luna-app --bin export-bindings`。
//! CI 在其后 `git diff --exit-code apps/desktop/src/ipc/bindings.ts` 校验无漂移（TDD-13 §2）。
//! 顺带重新生成随包的内置主题样式表（ADR-169 ③；它的漂移由 `cargo test` 判）。

fn main() {
    luna_app_lib::export_bindings().expect("导出 TS 绑定失败");
    println!("已生成 TS 绑定 → {}", luna_app_lib::BINDINGS_PATH);
    println!("已生成内置主题样式 → {}", luna_app_lib::THEMES_CSS_PATH);
}
