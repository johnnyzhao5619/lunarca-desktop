//! 内置主题（ADR-169 ③）：随 Lunarca 发布的主题包（`themes/*.toml`），与导入、市场主题同一个类型、
//! 同一个生成器。
//!
//! 服务端审核也引用这里：市场上架的主题不能用内置主题的 id。随包样式表
//! （[`builtin_stylesheet`]）编进前端的 CSS——每个窗口的第一帧都有配色，不必等 IPC；导入的主题
//! 由应用在运行时注入 [`ThemePackage::css_for`] 的输出。

use std::sync::OnceLock;

use crate::theme::{rule, ThemeMode, ThemePackage};

/// 外观设置对不上任何主题时用它；它的样式不带族选择器，是整套 Token 的底。
pub const FALLBACK_THEME_ID: &str = "luna";

/// 顺序即展示顺序；第一套是 [`FALLBACK_THEME_ID`]。
const SOURCES: [&str; 2] = [
    include_str!("../themes/luna.toml"),
    include_str!("../themes/nebula.toml"),
];

/// 全部内置主题（解析一次）。
///
/// # Panics
/// 随包的主题包不合格时——那是发布前就会被本模块的测试拦下的构建错误。
#[must_use]
pub fn builtin_themes() -> &'static [ThemePackage] {
    static THEMES: OnceLock<Vec<ThemePackage>> = OnceLock::new();
    THEMES.get_or_init(|| {
        SOURCES
            .iter()
            .map(|source| ThemePackage::parse(source).expect("内置主题包必须合格"))
            .collect()
    })
}

/// 这个 id 是不是内置主题的（导入与上架都不能占用）。
#[must_use]
pub fn is_builtin_theme(id: &str) -> bool {
    builtin_themes().iter().any(|theme| theme.id.as_str() == id)
}

/// 前端随包的主题样式表（`packages/styles/src/tokens/themes.generated.css`）。
///
/// 回落主题不带族选择器：`:root` 裸值（外观偏好挂上之前）、`[data-theme]` 两档，以及系统深色而
/// 还没有 `data-theme` 时的兜底——族 id 对不上任何样式时，界面也还是它。其余内置主题挂
/// `[data-theme-family][data-theme]` 双属性（特异性 0,3,0 压过 0,2,0），与运行时注入的同一写法。
#[must_use]
pub fn builtin_stylesheet() -> String {
    let mut out = String::from(
        "/* 生成物（ADR-169 ③），勿手改：由 crates/luna-content-schema/themes/*.toml 经\n \
         * luna_content_schema::builtin_stylesheet 生成。改了主题包之后运行\n \
         * `cargo run -p luna-app --bin export-bindings`；与生成器不一致时 cargo test 判红。 */\n",
    );
    for (index, theme) in builtin_themes().iter().enumerate() {
        out.push_str(&format!("\n/* {} */\n", theme.name));
        let light = theme.declarations(ThemeMode::Light);
        let dark = theme.declarations(ThemeMode::Dark);
        if index == 0 {
            out.push_str(&rule(":root,\n:root[data-theme=\"light\"]", &light, ""));
            out.push_str(&rule(":root[data-theme=\"dark\"]", &dark, ""));
            out.push_str("@media (prefers-color-scheme: dark) {\n");
            out.push_str(&rule(":root:not([data-theme])", &dark, "  "));
            out.push_str("}\n");
        } else {
            for (mode, declarations) in [(ThemeMode::Light, light), (ThemeMode::Dark, dark)] {
                out.push_str(&rule(
                    &format!(
                        ":root[data-theme-family=\"{}\"][data-theme=\"{}\"]",
                        theme.id,
                        mode.as_str()
                    ),
                    &declarations,
                    "",
                ));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_themes_parse_and_the_fallback_comes_first() {
        let themes = builtin_themes();
        assert_eq!(themes.len(), SOURCES.len());
        assert_eq!(themes[0].id.as_str(), FALLBACK_THEME_ID);
        assert!(is_builtin_theme("nebula"));
        assert!(!is_builtin_theme("installed"));
    }

    /// 在线目录里内联的主题包经 `to_toml` 落盘、再经 `parse` 读回：两者必须互逆，否则装上的那套与
    /// 目录里签过的那套不是同一套（可选的细调字段丢了，颜色就悄悄变了）。
    #[test]
    fn every_built_in_theme_round_trips_through_toml() {
        for theme in builtin_themes() {
            assert_eq!(&ThemePackage::parse(&theme.to_toml()).unwrap(), theme);
        }
    }

    #[test]
    fn the_stylesheet_scopes_every_theme_but_the_fallback() {
        let css = builtin_stylesheet();
        assert!(
            css.contains(":root,\n:root[data-theme=\"light\"] {\n  --color-bg-canvas: #efeae0;")
        );
        assert!(css.contains("  :root:not([data-theme]) {\n    --color-bg-canvas: #161418;"));
        assert!(css.contains(":root[data-theme-family=\"nebula\"][data-theme=\"dark\"] {"));
        assert!(!css.contains("data-theme-family=\"luna\""));
    }
}
