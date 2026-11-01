//! 首帧外观快照（ADR-114 ④ ⑵ / PRD-14-066）。
//!
//! 主窗建窗时把外观设置（明暗 / 配色族 / 语言 / 字号 / 减少动效 / 高对比）经
//! `initialization_script` 注入，脚本在**文档解析之前**跑：把 `data-*` 挂到 `<html>` 上，
//! 并留下 `window.__LUNA_BOOT__` 供 i18n 与 React 挂载前取用。此后 `useAppearance`
//! 读回的是同一份设置，故首帧与稳态一致，不闪。
//!
//! 脚本里那几行「设置 → data-*」与前端 `theme.ts::applyTheme` 是**同一份映射的两个
//! 落点**（一个跑在解析前、一个跑在 React 里），属性名由前端 `boot-appearance-guard`
//! 测试与本文件互相钉死——改一边必须改另一边。
//!
//! 窗口底色取自主题注册表解析出的那一套主题包的 `canvas`（[`canvas_color`]）——与生成
//! `--color-bg-canvas` 的是同一个值，不另存常量。

use luna_config::Settings;
use luna_content::{Layer, Layered};
use luna_content_schema::{Rgb, ThemeMode, ThemePackage};
use serde::Serialize;
use tauri::window::Color;

/// 注入给首帧的外观快照（字段名即 `window.__LUNA_BOOT__` 的键）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BootAppearance {
    /// `system` / `light` / `dark`。
    pub theme: String,
    /// 内置主题 id 或内容层里导入的主题 id（找不到已回落 `luna`，`ipc::themes::resolve_theme`）。
    pub theme_family: String,
    /// 导入的主题要另装的样式（Rust 从校验过的配色生成）；内置主题的样式随包，为空。
    pub installed_theme_css: Option<String>,
    /// 原生窗口首帧底色：所选主题两档的 `canvas`。
    #[serde(skip)]
    canvas_light: Rgb,
    #[serde(skip)]
    canvas_dark: Rgb,
    /// `system` / `zh-CN` / `en-US`——`system` 交给前端按 `navigator.language` 解析。
    pub language: String,
    pub reduce_motion: bool,
    pub high_contrast: bool,
    /// 百分比整数（100 = 1×）。
    pub font_scale: i64,
    /// `compact` / `comfortable`（未知值已回落 `compact`，PRD-12-021）。
    pub density: String,
}

/// 与前端 `resolveDensity` 同一条回落规则。
fn resolve_density(value: &str) -> &'static str {
    match value {
        "comfortable" => "comfortable",
        _ => "compact",
    }
}

impl BootAppearance {
    /// 从设备设置读出快照（只有内置主题）。
    #[must_use]
    #[cfg(test)]
    pub fn from_settings(settings: &Settings) -> Self {
        Self::from_settings_and_registry(settings, &crate::ipc::themes::theme_registry(Vec::new()))
    }

    #[must_use]
    pub fn from_settings_and_registry(
        settings: &Settings,
        registry: &[Layered<ThemePackage>],
    ) -> Self {
        let str_of = |key: &str, fallback: &str| {
            settings
                .get_str(key)
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| fallback.to_owned())
        };
        let entry = crate::ipc::themes::resolve_theme(
            &str_of(crate::ipc::themes::THEME_FAMILY_KEY, ""),
            registry,
        );
        let canvas = |mode| {
            entry
                .item
                .resolved(mode)
                .map_or(Rgb([0, 0, 0]), |palette| palette.canvas)
        };
        Self {
            theme: str_of("appearance.theme", "system"),
            theme_family: entry.item.id.as_str().to_owned(),
            installed_theme_css: (entry.layer != Layer::Builtin)
                .then(|| crate::ipc::themes::runtime_css(entry))
                .flatten(),
            canvas_light: canvas(ThemeMode::Light),
            canvas_dark: canvas(ThemeMode::Dark),
            language: str_of("appearance.language", "system"),
            reduce_motion: settings
                .get_bool("appearance.reduce_motion")
                .unwrap_or(false),
            high_contrast: settings
                .get_bool("appearance.high_contrast")
                .unwrap_or(false),
            font_scale: settings
                .get("appearance.font_scale")
                .and_then(|v| v.as_int())
                .filter(|n| *n > 0)
                .unwrap_or(100),
            density: resolve_density(&str_of("appearance.density", "compact")).to_owned(),
        }
    }

    /// 生成注入脚本。`serde_json` 负责转义——设置值里出现引号或反斜杠也不会破坏脚本。
    #[must_use]
    pub fn script(&self) -> String {
        let json = serde_json::to_string(self).unwrap_or_else(|_| "{}".to_owned());
        format!(
            "(function(){{\
var b={json};window.__LUNA_BOOT__=b;\
try{{var r=document.documentElement;if(!r)return;\
var dark=b.theme==='dark'||(b.theme==='system'&&window.matchMedia('(prefers-color-scheme: dark)').matches);\
r.dataset.theme=dark?'dark':'light';\
r.dataset.themeFamily=b.theme_family;\
if(b.installed_theme_css){{var s=document.createElement('style');s.id='luna-installed-theme';s.textContent=b.installed_theme_css;r.appendChild(s);}}\
r.dataset.contrast=b.high_contrast?'high':'normal';\
r.dataset.reduceMotion=(b.reduce_motion||window.matchMedia('(prefers-reduced-motion: reduce)').matches)?'true':'false';\
r.style.setProperty('--font-scale',String(b.font_scale/100));\
r.dataset.density=b.density;\
}}catch(e){{}}}})();"
        )
    }

    /// 这份快照要不要按深色画底：显式选了就听它的，`system` 由调用方拿窗口的系统主题来定。
    #[must_use]
    pub fn explicit_dark(&self) -> Option<bool> {
        match self.theme.as_str() {
            "dark" => Some(true),
            "light" => Some(false),
            _ => None,
        }
    }
}

/// 主窗底色 = 所选主题在该明暗下的 `canvas`（即 `--color-bg-canvas`）。首帧之前 WebView 还没画
/// 任何东西，底色就是用户看到的第一个像素。
#[must_use]
pub fn canvas_color(boot: &BootAppearance, dark: bool) -> Color {
    let Rgb([r, g, b]) = if dark {
        boot.canvas_dark
    } else {
        boot.canvas_light
    };
    Color(r, g, b, 0xff)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_settings_are_empty() {
        let boot = BootAppearance::from_settings(&Settings::new());
        assert_eq!(boot.theme, "system");
        assert_eq!(boot.theme_family, "luna");
        assert_eq!(boot.language, "system");
        assert_eq!(boot.font_scale, 100);
        assert_eq!(boot.explicit_dark(), None);
    }

    #[test]
    fn unknown_family_falls_back_to_luna_and_script_escapes_values() {
        let mut settings = Settings::new();
        settings.set("appearance.theme_family", "garbage");
        settings.set("appearance.theme", "dark\"');alert(1);//");
        let boot = BootAppearance::from_settings(&settings);
        assert_eq!(boot.theme_family, "luna");
        let script = boot.script();
        // JSON 转义后引号成了 \"，脚本文本里不再有能提前闭合字符串的裸引号序列。
        assert!(script.contains("\\\"')"));
        assert!(script.contains("window.__LUNA_BOOT__=b"));
        assert!(script.contains("r.dataset.theme="));
        assert!(script.contains("r.dataset.themeFamily="));
        assert!(script.contains("r.dataset.contrast="));
        assert!(script.contains("r.dataset.reduceMotion="));
        assert!(script.contains("'--font-scale'"));
    }

    #[test]
    fn an_installed_theme_brings_its_css_and_canvas_and_builtins_bring_neither_css() {
        let source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../crates/luna-content-schema/themes/nebula.toml"),
        )
        .unwrap()
        .replace("id = \"nebula\"", "id = \"test\"")
        .replace("canvas = \"#f6f6f6\"", "canvas = \"#fafafa\"");
        let package = ThemePackage::parse(&source).expect("valid test package");
        let registry = crate::ipc::themes::theme_registry(vec![package]);
        let mut settings = Settings::new();
        settings.set("appearance.theme_family", "test");
        let boot = BootAppearance::from_settings_and_registry(&settings, &registry);
        assert_eq!(boot.theme_family, "test");
        assert!(boot
            .installed_theme_css
            .as_deref()
            .is_some_and(|css| css.contains("[data-theme-family=\"test\"]")));
        assert_eq!(canvas_color(&boot, false), Color(0xfa, 0xfa, 0xfa, 0xff));
        assert_eq!(canvas_color(&boot, true), Color(0x1e, 0x1e, 0x1e, 0xff));
        assert!(!boot.script().contains("canvas_light"));

        settings.set("appearance.theme_family", "nebula");
        let boot = BootAppearance::from_settings_and_registry(&settings, &registry);
        assert_eq!(boot.installed_theme_css, None, "内置主题的样式随包");
        assert_eq!(canvas_color(&boot, false), Color(0xf6, 0xf6, 0xf6, 0xff));
    }

    #[test]
    fn the_fallback_canvas_is_lunarca() {
        let boot = BootAppearance::from_settings(&Settings::new());
        assert_eq!(canvas_color(&boot, true), Color(0x16, 0x14, 0x18, 0xff));
        assert_eq!(canvas_color(&boot, false), Color(0xef, 0xea, 0xe0, 0xff));
    }
}
