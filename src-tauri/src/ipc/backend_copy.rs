//! 后端产生的**给人读的文案**唯一取用口（IPC 层共用，TDD-11 / TDD-15 §4.3）。
//!
//! 职责边界：把 Fluent 消息 id 变成当前后端语言的一句话，并（对错误）拼上稳定机器前缀。
//! 文案本身住在 `resources/i18n/{zh-CN,en-US}.ftl`，本模块一个字都不带。
//!
//! # 为什么它在这里，而不是继续挂在 `distill.rs` 上
//!
//! 这三个函数原本住在 `ipc/distill.rs`——它们最早只服务「多条 → 一篇」那条路。挂在某条
//! 业务路径下的共享 helper 有一个可预测的结局：下一个人在自己的模块里再写一个一模一样的，
//! 或者干脆 `format!("CODE: 中文")` 了事。`ipc_error_copy_debt.txt` 那 184 行就是这个结局
//! 的账单——它们分布在 25 个模块里，而其中只有 3 个模块想起过 `super::distill::coded_error`。
//!
//! 住进 `ipc/` 自己的模块之后，`mod.rs` 的 `pub(crate) use backend_copy::*` 让每个域模块的
//! `use super::*` 直接拿到它们，**迁移一处只需要改一行**——而摩擦正是那份欠账的主要成因。

use tauri::Manager as _;

/// 从 `AppHandle` 取后端文案来源（`Arc` 形态，可进 `'static` 闭包）。
///
/// **唯一取用口**：`app.try_state::<Arc<BackendI18n>>()` 这句咒语此前抄在十个文件里，
/// 而托管类型从 `BackendI18n` 换成 `Arc<BackendI18n>` 那一刻（ADR-107）**编译器一个字都
/// 不会说**——`try_state::<T>` 是泛型，取错类型只是静默拿到 `None`，表现为界面上突然
/// 冒出一串 SCREAMING 码。抄十份的东西没有「一起改」这回事，所以它只能有一份。
pub(crate) fn backend_copy_arc(
    app: &tauri::AppHandle,
) -> Option<std::sync::Arc<crate::i18n::BackendI18n>> {
    app.try_state::<std::sync::Arc<crate::i18n::BackendI18n>>()
        .map(|state| std::sync::Arc::clone(&state))
}

/// 后端文案的**取用来源**，供拿不到 `AppHandle` 的地方使用。
///
/// # 为什么不是 `&AppHandle`
///
/// 纯函数与只持 `&AppState` 的 helper 也会产出给人读的错误说明（向量归一化、分片摘要、
/// 派生缓存落盘…）。给它们塞一个 `AppHandle` 有两处代价：**能力过宽**（拿到句柄就能建窗口、
/// 发事件、读任意托管状态，而它们只需要读一句话），以及**测不动**（纯函数的单测里没有
/// Tauri 运行时）。收成这个别名之后，签名本身就说清了「这个参数只用来取文案」，
/// 而测试传 `None` 即可——落到与「bundle 尚未托管」完全相同的既有兜底路径上。
pub(crate) type BackendCopy<'a> = Option<&'a crate::i18n::BackendI18n>;

/// 后端可本地化文案的取用口（`fallback` 只在 bundle 缺失时兜底）。
///
/// **不在 IPC 里按语言手写两套文字**（TDD-07 §2.1）：产物默认名、来源区标题、覆盖说明
/// 与全部错误说明都走既有 Fluent 资源，否则每加一种语言就要回来改一遍代码，而漏改的那
/// 一处会给英文界面的用户一段中文。
pub(crate) fn backend_text(app: &tauri::AppHandle, key: &str, fallback: &str) -> String {
    let i18n = backend_copy_arc(app);
    backend_text_with(i18n.as_deref(), key, fallback)
}

/// [`backend_text`] 的取用来源版本（见 [`BackendCopy`]）。
pub(crate) fn backend_text_with(i18n: BackendCopy<'_>, key: &str, fallback: &str) -> String {
    i18n.and_then(|i18n| i18n.translate(key))
        .unwrap_or_else(|| fallback.to_owned())
}

/// 带错误码的错误串：稳定机器前缀 + 本地化说明。
///
/// 前缀是**契约**（调用方按它分支，`processing.rs` 已有先例），说明是给人看的——两者拼在
/// 一起，机器可判、人可读，且不需要第二套错误目录。
///
/// **IPC 层所有出人读的错误说明都必须经过它**（或 [`coded_error_args`]）。
/// 直接 `format!("CODE: 中文")` 会给英文界面的用户一段中文，且没有任何东西会因此变红——
/// 除了 [`crate::ipc_error_copy`] 那道门禁，而它的欠账台账**只减不增**。
pub(crate) fn coded_error(app: &tauri::AppHandle, code: &str, key: &str) -> String {
    coded_error_args(app, code, key, &[])
}

/// [`coded_error`] 的取用来源版本（见 [`BackendCopy`]）。
pub(crate) fn coded_error_with(i18n: BackendCopy<'_>, code: &str, key: &str) -> String {
    coded_error_args_with(i18n, code, key, &[])
}

/// 带参数的版本（条数门槛、额度缺口、路径等由代码作为参数插进文案，绝不拼进串）。
///
/// **参数不许 `format!` 进串**：拼一次就等于把这句话钉死在一种语言上。同一条纪律在
/// `distill-quota-short` 那三个数字上写过一遍，这里是它的通用形态。
pub(crate) fn coded_error_args(
    app: &tauri::AppHandle,
    code: &str,
    key: &str,
    args: &[(&str, String)],
) -> String {
    let i18n = backend_copy_arc(app);
    coded_error_args_with(i18n.as_deref(), code, key, args)
}

/// [`coded_error_args`] 的取用来源版本（见 [`BackendCopy`]）。
pub(crate) fn coded_error_args_with(
    i18n: BackendCopy<'_>,
    code: &str,
    key: &str,
    args: &[(&str, String)],
) -> String {
    let text = i18n
        .and_then(|i18n| i18n.translate_args(key, args))
        .unwrap_or_default();
    format!("{code}: {text}")
}
