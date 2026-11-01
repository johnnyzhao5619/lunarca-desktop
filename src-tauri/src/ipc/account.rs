//! 账户的宿主侧（统一登录，ADR-155）：变化广播 + `lunarca://connect` 送达。
//!
//! 账户状态本身（登录了哪台、登录到哪一步）的事实源在 `luna_cloud::account`；这里只做两件
//! 它够不着的事——把全量状态推给每个窗口，以及接住系统送来的链接。

use super::*;

/// 账户变了（选定服务器 / 登录 / 退出 / 等浏览器 / 收到链接）。载荷是**全量**状态：
/// 主窗与设置窗各自订阅同一事件、读同一份 Rust 状态，不各自推导。
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
#[tauri_specta(event_name = "account://changed")]
pub struct AccountChanged {
    pub state: luna_cloud::account::AccountStateDto,
}

/// 广播一次账户全量状态。
pub(crate) fn emit_account_changed(app: &tauri::AppHandle) {
    if let Some(cloud) = app.try_state::<luna_cloud::commands::CloudState>() {
        let _ = AccountChanged {
            state: cloud.0.account_state(),
        }
        .emit(app);
    }
}

/// URL scheme 送达（运行中或冷启动）：交给云客户端**只存不发**、亮主窗、去账户页、广播。
/// 不写任何东西、不发网络请求（ADR-037）——用户在登录界面点「继续」才开始核验。
pub(crate) fn handle_incoming_deep_link(app: &tauri::AppHandle, url: &str) {
    // 同一个入口按 host 分派（ADR-141 ⑥）：指向库里内容的链接另有去处。
    if luna_vault::uri::is_vault_link(url) {
        super::vault_links::receive_vault_link(app, url);
        return;
    }
    let Some(cloud) = app.try_state::<luna_cloud::commands::CloudState>() else {
        return;
    };
    if let Err(rejection) = cloud.0.stash_incoming_link(url) {
        eprintln!("[account] 收到无法解析的 lunarca:// 链接（{rejection}），已忽略");
        return;
    }
    crate::show_main_window(app);
    // 存下再通知（[`super::request_workspace_navigation`]）：冷启动时工作区还没挂上，只发事件
    // 的话这一跳会丢——链接本身已存进云客户端，丢的是"去账户页"。
    super::request_workspace_navigation(app, "settings://account".to_owned());
    emit_account_changed(app);
}
