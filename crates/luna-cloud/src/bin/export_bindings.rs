//! Headless 生成云插件 TS 绑定：`cargo run -p luna-cloud --bin export-cloud-bindings`。
//! 产物提交入库；CI/本地重新生成后 `git diff` 应为空（与 upstream 绑定纪律一致）。

fn main() {
    luna_cloud::export_bindings().expect("导出云插件 TS 绑定失败");
    println!("已生成 TS 绑定 → {}", luna_cloud::BINDINGS_PATH);
}
