//! 云插件的 ACL 清单生成（Tauri v2 权限模型）。
//!
//! **为什么必须有**：公开仓显式定义了 `src-tauri/capabilities/default.json`，Tauri 因此
//! **强制执行 ACL**——没有权限声明的插件命令一律被拒，前端拿到的是一个不带 `code` 字段的
//! 拒绝对象（UI 上表现为 `error.undefined`）。
//!
//! 迁移前 `app-official` 之所以"能用"，只是因为它**根本没有 capabilities 目录**、ACL 未被
//! 启用——那是靠缺配置侥幸通过，不是正确形态。合并进公开仓后必须补齐真正的权限清单。
const COMMANDS: &[&str] = &[
    "account_state",
    "account_sign_in_begin",
    "account_sign_in_password",
    "account_sign_in_code",
    "account_sign_in_set_password",
    "account_sign_in_request_code",
    "account_change_password",
    "account_sign_in_browser",
    "account_sign_in_reopen_browser",
    "account_sign_in_cancel",
    "account_sign_in_resolve_device_limit",
    "account_sign_out",
    "devices_list",
    "device_revoke",
    "account_deletion_preview",
    "account_delete",
    "account_export",
    "entitlements",
    "usage",
    "spend_cap_set",
    "billing_offer",
    "billing_available",
    "billing_checkout",
    "billing_portal",
    "billing_verify_card",
    "catalog_status",
    "prompt_catalog_status",
    "trust_status",
    "trust_confirm_change",
];

fn main() {
    tauri_plugin::Builder::new(COMMANDS).build();
}
