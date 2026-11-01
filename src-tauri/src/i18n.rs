//! 后端 i18n（Fluent runtime）。
//!
//! 职责边界（TDD-11）：前端 UI 文案统一走 react-i18next（`apps/desktop/src/i18n`）；
//! 本模块只服务**后端产生**的可本地化字符串（通知/错误 key），供 NotificationCenter 等
//! 在需要后端侧文案时查询。语言切换经 IPC 同步，避免前后端半中半英。
//!
//! 隐私铁律（AGENTS.md §7）：本模块只解析静态 key → 预置文案，绝不接收/拼接转写文本、
//! 剪贴板、笔记正文或文件名。

// 用并发版 bundle（Mutex 化 memoizer）：BackendI18n 需 Send + Sync 以便 `tauri::Manager::manage`。
use fluent_bundle::concurrent::FluentBundle;
use fluent_bundle::FluentResource;
use std::sync::RwLock;
use unic_langid::{langid, LanguageIdentifier};

/// 内置 Fluent 资源（编译期嵌入；离线默认，无运行期文件依赖）。
const ZH_CN_FTL: &str = include_str!("../resources/i18n/zh-CN.ftl");
const EN_US_FTL: &str = include_str!("../resources/i18n/en-US.ftl");

/// 把 `appearance.language` 的**设置值**解析为具体语言标签。
///
/// **`system` 是设置值，不是语言标签**。它的含义是「跟随系统」，而解析系统语言这件事
/// 客户端只有前端做得到（`navigator.language`；`use-appearance` 解析后经
/// `set_backend_language` 同步过来，那条命令的契约就写着「前端解析 `system` 后传入具体
/// 语言」）。后端在那次同步到达之前必须先有一个语言，此处取与前端 `fallbackLng` 同值的
/// **zh-CN**。
///
/// **不接这一步的代价是默认安装的后端「启动即英文」**：该设置项的注册表默认值就是
/// `system`，而设置读取会回落到注册表默认值，于是 `"system"` 被原样送进
/// [`BackendI18n::build_inner`]；那里只判 `starts_with("zh")`，`"system"` 落进 else 分支
/// 变成 `en-US`。这不只影响后端文案——**启动窗口内触发的自动打标会据此打出英文标签**，
/// 与库里其余中文标签分裂成两套词汇表，而「标签是库级词汇表、一个库一种语言」正是
/// PRD-06-035 那条 2026-07-29 修订的全部前提（症状：同一概念裂成 `#architecture` 与
/// `#架构` 各挂一半笔记，主题页「同标签 ≥3 篇」的判据因此永远凑不满）。
///
/// **它消不掉整个窗口，只消掉「猜错」的那一半**：系统语言是英文而设置为 `system` 的
/// 用户，在前端同步到达之前仍会短暂落在 zh-CN 上。要彻底消除，后端得自己读得到系统 locale
/// （需引入依赖，按 ADR-005 要过 License 核查），本函数不做那个决定。
#[must_use]
pub fn resolve_language_setting(value: &str) -> String {
    if value.starts_with("en") {
        "en-US".to_owned()
    } else {
        // `zh*` 与 `system`（及任何将来新增的非语言取值）一律落 zh-CN——与前端 `fallbackLng`
        // 同值。刻意**不**把未知取值送去 en-US：那正是本函数存在的原因。
        "zh-CN".to_owned()
    }
}

/// 后端本地化 bundle。
pub struct BackendI18n {
    inner: RwLock<BackendI18nInner>,
}

struct BackendI18nInner {
    language: String,
    bundle: FluentBundle<FluentResource>,
}

impl BackendI18n {
    /// 以给定语言构造（暂不支持时回退 `en-US`，`zh*` 走 `zh-CN`）。
    #[must_use]
    pub fn new(language: impl Into<String>) -> Self {
        Self {
            inner: RwLock::new(Self::build_inner(language.into())),
        }
    }

    /// 运行期切换后端语言（前端 appearance hook 写设置后调用）。
    pub fn set_language(&self, language: impl Into<String>) {
        if let Ok(mut inner) = self.inner.write() {
            *inner = Self::build_inner(language.into());
        }
    }

    fn build_inner(language: String) -> BackendI18nInner {
        // 说明：入参是**具体语言标签**（`zh-CN` / `en-US`）。设置值 `system` 不得直接送进来，
        // 先过 [`resolve_language_setting`]——理由见那里。
        let (resolved_language, resource) = if language.starts_with("zh") {
            ("zh-CN".to_owned(), ZH_CN_FTL)
        } else {
            ("en-US".to_owned(), EN_US_FTL)
        };
        let langid: LanguageIdentifier = resolved_language
            .parse()
            .unwrap_or_else(|_| langid!("en-US"));
        let mut bundle = FluentBundle::new_concurrent(vec![langid]);
        // 关闭 Unicode 方向隔离标记：后端文案可能进日志/通知，避免出现不可见控制符。
        bundle.set_use_isolating(false);
        if let Ok(resource) = FluentResource::try_new(resource.to_owned()) {
            // 资源重复添加才会出错；内置单资源此处稳定成功。
            let _ = bundle.add_resource(resource);
        }
        BackendI18nInner {
            language: resolved_language,
            bundle,
        }
    }

    /// 当前语言标签。
    #[must_use]
    pub fn language(&self) -> String {
        self.inner
            .read()
            .map(|inner| inner.language.clone())
            .unwrap_or_else(|_| "en-US".to_owned())
    }

    /// 将 i18n key（如 `notifications:config_recovered`）映射为 Fluent 消息 id。
    ///
    /// Fluent 标识符不允许 `:`/`.`，统一规范化为 `-`。
    fn message_id(key: &str) -> String {
        key.replace([':', '.'], "-")
    }

    /// 解析 key 对应的本地化文案；未注册或格式化失败返回 `None`（调用方决定兜底）。
    #[must_use]
    pub fn translate(&self, key: &str) -> Option<String> {
        self.format(key, None)
    }

    /// 带参数解析（Fluent `{ $name }` 占位；如 `track-remote-n` 的 `$n`）。
    #[must_use]
    pub fn translate_args(&self, key: &str, args: &[(&str, String)]) -> Option<String> {
        let mut fluent_args = fluent_bundle::FluentArgs::new();
        for (name, value) in args {
            fluent_args.set(*name, value.clone());
        }
        self.format(key, Some(&fluent_args))
    }

    fn format(&self, key: &str, args: Option<&fluent_bundle::FluentArgs<'_>>) -> Option<String> {
        let id = Self::message_id(key);
        let inner = self.inner.read().ok()?;
        let message = inner.bundle.get_message(&id)?;
        let pattern = message.value()?;
        let mut errors = Vec::new();
        let value = inner.bundle.format_pattern(pattern, args, &mut errors);
        if errors.is_empty() {
            Some(value.into_owned())
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn i18n_defaults_to_en_us_when_language_unparseable() {
        let i18n = BackendI18n::new("not-a-langid!!");
        assert_eq!(i18n.language(), "en-US");
        // 即便标签无法解析，bundle 仍回退 en-US，可解析内置 key。
        assert!(i18n.translate("notifications:config_recovered").is_some());
    }

    /// `system` 是设置值、不是语言标签——它必须落 zh-CN，绝不能落 en-US。
    ///
    /// 这条断言拦的是一个已经发生过的真实缺陷：`appearance.language` 的注册表默认值就是
    /// `system`，而设置读取会回落到默认值，于是原值直传时后端「启动即英文」。它的下游
    /// 不只是后端文案——启动窗口内的自动打标会据此打出英文标签，把库级标签词汇表劈成
    /// 中英两套（PRD-06-035 2026-07-29 修订的前提正是「一个库一种语言」）。
    #[test]
    fn system_setting_value_resolves_to_the_declared_fallback_not_english() {
        assert_eq!(resolve_language_setting("system"), "zh-CN");
        // 空值（键缺失时的 `unwrap_or_default`）同样不得漂到英文。
        assert_eq!(resolve_language_setting(""), "zh-CN");
        // 将来新增的任何非语言取值都走同一条回落，而不是 else 分支的 en-US。
        assert_eq!(resolve_language_setting("follow-os"), "zh-CN");
    }

    #[test]
    fn concrete_language_tags_survive_resolution_unchanged() {
        // 前端同步过来的是具体标签，解析必须原样保留——否则英文界面会被拽回中文。
        assert_eq!(resolve_language_setting("en-US"), "en-US");
        assert_eq!(resolve_language_setting("zh-CN"), "zh-CN");
        assert_eq!(
            BackendI18n::new(resolve_language_setting("en-US")).language(),
            "en-US"
        );
        assert_eq!(
            BackendI18n::new(resolve_language_setting("system")).language(),
            "zh-CN"
        );
    }

    #[test]
    fn i18n_translates_registered_notification_keys() {
        let i18n = BackendI18n::new("zh-CN");
        for key in [
            "notifications:config_recovered",
            "notifications:permission_revoked",
            "notifications:strict_local_blocked",
            "errors:model_missing",
        ] {
            assert!(i18n.translate(key).is_some(), "missing backend i18n: {key}");
        }
    }

    /// **两份 .ftl 的 key 集合必须逐个相等——全量，不按前缀。**
    ///
    /// 这条门禁此前只锁 `merge-` 前缀（写在 `ipc/distill.rs` 的测试里），于是**每一个新加的
    /// key 都天然不在它的判定面上**：`DISTILL_QUOTA_SHORT` 那句话就是这么在 IPC 里被
    /// `format!` 拼成中文、一路绿灯上线的。前缀白名单的问题不是"漏了一个前缀"，而是它
    /// 把「有没有译」变成了「记不记得回来加前缀」——记不住正是漏译的定义。
    ///
    /// 判定面是**集合相等**而非"en 是 zh 的子集"：只译一边与只写一边同样有害，前者让中文
    /// 用户读到英文，后者让英文用户读到中文，`translate()` 在两种情况下都取得到东西、
    /// 不会有任何东西变红。
    #[test]
    fn both_ftl_bundles_declare_exactly_the_same_message_ids() {
        let ids = |src: &str| {
            src.lines()
                .map(str::trim_end)
                // 注释与续行（缩进行、`.attr`）不是消息 id；只认顶格的 `id = 值`。
                .filter(|line| !line.is_empty() && !line.starts_with(['#', ' ', '\t', '.']))
                .filter_map(|line| line.split_once(" ="))
                .map(|(id, _)| id.trim().to_owned())
                .collect::<std::collections::BTreeSet<_>>()
        };
        let zh = ids(ZH_CN_FTL);
        let en = ids(EN_US_FTL);
        // 空集会让 assert_eq 恒真——解析器写坏时这条门禁会静默失效。
        assert!(zh.len() > 20, "zh-CN.ftl 解析出的 key 太少，解析口径坏了");
        let only_zh: Vec<_> = zh.difference(&en).collect();
        let only_en: Vec<_> = en.difference(&zh).collect();
        assert!(
            only_zh.is_empty() && only_en.is_empty(),
            "两份后端文案的 key 集合必须相等——只有 zh：{only_zh:?}；只有 en：{only_en:?}"
        );
    }

    /// **英文资源里不许有中文**——这是「后端文案走 Fluent」这件事的**全部目的**。
    ///
    /// key 集合相等（上一条）与 key 真的存在（`crate::ipc_error_copy` 那道扫源码的门禁）
    /// 合起来仍漏掉最后一种失效：两边 key 都在、`.ftl` 也解析得动，而 en-US 那一行的**值**
    /// 被顺手抄成了中文。那时英文界面的用户照样读到中文，而三道门禁全绿——正是
    /// `ipc_error_copy` 头注里那句「凡是没打算翻译的，都不会被发现没翻译」的最后一个藏身处。
    ///
    /// 判据面与 `ipc_error_copy::is_cjk` 同族（含 CJK 标点与全角形式）：只锁汉字会让
    /// 一行残留了中文排版的英文串溜过去。
    #[test]
    fn the_english_bundle_contains_no_chinese() {
        let offenders: Vec<&str> = EN_US_FTL
            .lines()
            .map(str::trim_end)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .filter(|line| {
                line.chars().any(|c| {
                    matches!(c,
                        '\u{3000}'..='\u{303F}'
                        | '\u{3400}'..='\u{4DBF}'
                        | '\u{4E00}'..='\u{9FFF}'
                        | '\u{F900}'..='\u{FAFF}'
                        | '\u{FF00}'..='\u{FFEF}')
                })
            })
            .collect();
        // 空跑防护：解析口径坏掉时下面那条断言会恒真。
        assert!(
            EN_US_FTL.lines().filter(|l| l.contains(" = ")).count() > 20,
            "en-US.ftl 解析出的行太少，判据在空跑"
        );
        assert!(
            offenders.is_empty(),
            "en-US.ftl 里有中文——英文界面的用户会读到它，而 key 齐全、bundle 也解析得动：\n{}",
            offenders.join("\n")
        );
    }

    #[test]
    fn i18n_returns_none_for_unknown_key() {
        let i18n = BackendI18n::new("zh-CN");
        assert!(i18n.translate("notifications:does_not_exist").is_none());
    }

    #[test]
    fn i18n_switches_language_at_runtime() {
        let i18n = BackendI18n::new("zh-CN");
        assert_eq!(
            i18n.translate("notifications:strict_local_blocked")
                .unwrap(),
            "严格本地模式已拦截联网任务"
        );
        i18n.set_language("en-US");
        assert_eq!(
            i18n.translate("notifications:strict_local_blocked")
                .unwrap(),
            "Strict local mode blocked a network task"
        );
    }
}
