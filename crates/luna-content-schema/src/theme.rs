//! 主题包契约（PRD-12-018 / ADR-133 / ADR-169 ③）：内置、本地导入与市场装的主题是同一个类型、
//! 同一个生成器。
//!
//! 主题包只含展示元数据与两套具名颜色，不接受 CSS、脚本、URL、字体或任意 Token 名。每套颜色前
//! 17 项必填，其余是细调（悬停底、控件边界、代码块、语法高亮……），不填就由那 17 项推导
//! （[`ThemePalette::resolve`]）。推导与校验都在解析出的颜色上做：校验的就是界面上真正画出来的
//! 那一组值，内置主题与导入的主题过的是同一道对比度门槛。样式按主题 id 命名空间生成
//! （[`ThemePackage::css_for`]），于是可以同时装多套。

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{ContentId, PackVersion};

/// v3（ADR-169 ③）：多了可选的细调字段，内置主题由此改写为主题包。尚无发布版本，旧版本的包
/// 不再被接受（ADR-046）。
pub const THEME_PACKAGE_SCHEMA_VERSION: u32 = 3;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemePackage {
    pub schema_version: u32,
    /// 主题 id：样式命名空间、内容层目录名与 `appearance.theme_family` 的取值。
    pub id: ContentId,
    /// 版本：同一个 id 再次导入时据此判断是更新还是不覆盖。
    pub version: PackVersion,
    pub name: String,
    pub author: String,
    pub license: String,
    pub light: ThemePalette,
    pub dark: ThemePalette,
}

/// 一套明暗下的配色（`#RRGGBB`）。前 17 项必填；`Option` 的是细调，默认值写在各字段上。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemePalette {
    pub canvas: String,
    pub surface: String,
    pub panel: String,
    pub text_primary: String,
    pub text_secondary: String,
    pub text_muted: String,
    pub border_subtle: String,
    pub border_default: String,
    pub border_strong: String,
    pub accent: String,
    pub accent_strong: String,
    /// 主按钮上的文字。
    pub accent_text: String,
    pub ai: String,
    pub attention: String,
    pub success: String,
    pub warning: String,
    pub error: String,
    /// 指针悬停的行底色。默认：强调色 10% 混进 `surface`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hover: Option<String>,
    /// 停用控件的底色。默认：弱化文字 5% 混进 `surface`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled: Option<String>,
    /// 次一级的静止底色（引用、标签）。默认：弱化文字 4% 混进 `surface`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub muted: Option<String>,
    /// 输入框、下拉、开关轨道的边界。默认：弱化文字。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border_control: Option<String>,
    /// 焦点环。默认：强调色。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<String>,
    /// 主按钮的底色。默认：强调色。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    /// 「无状态」那一档的状态色。默认：弱化文字。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_muted: Option<String>,
    /// 模态遮罩的颜色（透明度随明暗固定）。默认：深色取 `canvas`，浅色取正文色。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scrim: Option<String>,
    /// 代码块。默认见 [`CodePalette`]。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<CodePalette>,
    /// 语法高亮。默认见 [`SyntaxPalette`]。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub syntax: Option<SyntaxPalette>,
}

/// 代码块的四个颜色。不填整张表时：`surface` 取 `surface`、`header` 取 `canvas`、`text` 取正文色、
/// `hover` 取悬停底。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodePalette {
    pub surface: String,
    pub header: String,
    pub text: String,
    pub hover: String,
}

/// 语法高亮的七类记号。不填整张表时：关键字取 `accent_strong`、字符串取 `success`、数字取
/// `warning`、函数取 `accent`、类型取 `ai`、属性取次要文字、运算符取弱化文字。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyntaxPalette {
    pub keyword: String,
    pub string: String,
    pub number: String,
    pub func: String,
    pub r#type: String,
    pub prop: String,
    pub op: String,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ThemePackageError {
    #[error("invalid theme package: {0}")]
    Parse(String),
    #[error("unsupported theme package schema: {0}")]
    Schema(u32),
    #[error("invalid theme package metadata: {0}")]
    Metadata(&'static str),
    #[error("theme color {field} must use #RRGGBB")]
    Color { field: String },
    #[error("{mode} palette has insufficient contrast for {foreground} on {background}")]
    Contrast {
        mode: &'static str,
        foreground: &'static str,
        background: &'static str,
    },
}

/// 明暗两档。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeMode {
    Light,
    Dark,
}

impl ThemeMode {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    /// 强调色与状态色的半透明底（选中行、徽标）：深色底上要多一点才看得出。
    fn tint_percent(self) -> u8 {
        match self {
            Self::Light => 12,
            Self::Dark => 16,
        }
    }

    fn scrim_alpha(self) -> &'static str {
        match self {
            Self::Light => "0.4",
            Self::Dark => "0.55",
        }
    }
}

/// 一个不透明的 sRGB 颜色。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub [u8; 3]);

impl Rgb {
    /// 只认 `#RRGGBB`。
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let bytes = value.as_bytes();
        if bytes.len() != 7 || bytes[0] != b'#' || !value.is_ascii() {
            return None;
        }
        let pair = |offset| u8::from_str_radix(&value[offset..offset + 2], 16).ok();
        Some(Self([pair(1)?, pair(3)?, pair(5)?]))
    }

    /// `percent`% 的自己混进 `base`（与 CSS `color-mix(in srgb, self p%, base)` 同一算法）。
    #[must_use]
    pub fn mix(self, percent: u8, base: Self) -> Self {
        let weight = f64::from(percent) / 100.0;
        let channel = |i: usize| {
            let value = f64::from(self.0[i]) * weight + f64::from(base.0[i]) * (1.0 - weight);
            // 0..=255 的加权平均，四舍五入后不会越界。
            value.round() as u8
        };
        Self([channel(0), channel(1), channel(2)])
    }

    /// WCAG 2 对比度。
    #[must_use]
    pub fn contrast(self, other: Self) -> f64 {
        let luminance = |color: Self| {
            let channel = |value: u8| {
                let value = f64::from(value) / 255.0;
                if value <= 0.04045 {
                    value / 12.92
                } else {
                    ((value + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * channel(color.0[0])
                + 0.7152 * channel(color.0[1])
                + 0.0722 * channel(color.0[2])
        };
        let (a, b) = (luminance(self), luminance(other));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }
}

impl fmt::Display for Rgb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.0[0], self.0[1], self.0[2])
    }
}

/// 推导完的一套配色：界面上真正画出来的每一个不透明颜色。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedPalette {
    pub canvas: Rgb,
    pub surface: Rgb,
    pub panel: Rgb,
    pub hover: Rgb,
    pub disabled: Rgb,
    pub muted: Rgb,
    pub text_primary: Rgb,
    pub text_secondary: Rgb,
    pub text_muted: Rgb,
    pub border_subtle: Rgb,
    pub border_default: Rgb,
    pub border_strong: Rgb,
    pub border_control: Rgb,
    pub focus: Rgb,
    pub action: Rgb,
    pub action_text: Rgb,
    pub accent: Rgb,
    pub accent_strong: Rgb,
    pub ai: Rgb,
    pub attention: Rgb,
    pub success: Rgb,
    pub warning: Rgb,
    pub error: Rgb,
    pub status_muted: Rgb,
    pub scrim: Rgb,
    pub code_surface: Rgb,
    pub code_header: Rgb,
    pub code_text: Rgb,
    pub code_hover: Rgb,
    pub syntax_keyword: Rgb,
    pub syntax_string: Rgb,
    pub syntax_number: Rgb,
    pub syntax_func: Rgb,
    pub syntax_type: Rgb,
    pub syntax_prop: Rgb,
    pub syntax_op: Rgb,
}

impl ThemePalette {
    /// 解析每个颜色并补上细调的默认值。
    ///
    /// # Errors
    /// 任何一个颜色不是 `#RRGGBB` 时返回 [`ThemePackageError::Color`]（字段名带明暗前缀）。
    pub fn resolve(&self, mode: ThemeMode) -> Result<ResolvedPalette, ThemePackageError> {
        let color = |field: &str, value: &str| {
            Rgb::parse(value).ok_or_else(|| ThemePackageError::Color {
                field: format!("{}.{field}", mode.as_str()),
            })
        };
        let optional = |field: &str, value: Option<&String>, fallback: Rgb| {
            value.map_or(Ok(fallback), |value| color(field, value))
        };
        let canvas = color("canvas", &self.canvas)?;
        let surface = color("surface", &self.surface)?;
        let text_primary = color("text_primary", &self.text_primary)?;
        let text_secondary = color("text_secondary", &self.text_secondary)?;
        let text_muted = color("text_muted", &self.text_muted)?;
        let accent = color("accent", &self.accent)?;
        let accent_strong = color("accent_strong", &self.accent_strong)?;
        let ai = color("ai", &self.ai)?;
        let success = color("success", &self.success)?;
        let warning = color("warning", &self.warning)?;
        let hover = optional("hover", self.hover.as_ref(), accent.mix(10, surface))?;
        let code = match &self.code {
            Some(code) => (
                color("code.surface", &code.surface)?,
                color("code.header", &code.header)?,
                color("code.text", &code.text)?,
                color("code.hover", &code.hover)?,
            ),
            None => (surface, canvas, text_primary, hover),
        };
        let syntax = match &self.syntax {
            Some(syntax) => [
                color("syntax.keyword", &syntax.keyword)?,
                color("syntax.string", &syntax.string)?,
                color("syntax.number", &syntax.number)?,
                color("syntax.func", &syntax.func)?,
                color("syntax.type", &syntax.r#type)?,
                color("syntax.prop", &syntax.prop)?,
                color("syntax.op", &syntax.op)?,
            ],
            None => [
                accent_strong,
                success,
                warning,
                accent,
                ai,
                text_secondary,
                text_muted,
            ],
        };
        let scrim_fallback = match mode {
            ThemeMode::Light => text_primary,
            ThemeMode::Dark => canvas,
        };
        Ok(ResolvedPalette {
            canvas,
            surface,
            panel: color("panel", &self.panel)?,
            hover,
            disabled: optional(
                "disabled",
                self.disabled.as_ref(),
                text_muted.mix(5, surface),
            )?,
            muted: optional("muted", self.muted.as_ref(), text_muted.mix(4, surface))?,
            text_primary,
            text_secondary,
            text_muted,
            border_subtle: color("border_subtle", &self.border_subtle)?,
            border_default: color("border_default", &self.border_default)?,
            border_strong: color("border_strong", &self.border_strong)?,
            border_control: optional("border_control", self.border_control.as_ref(), text_muted)?,
            focus: optional("focus", self.focus.as_ref(), accent)?,
            action: optional("action", self.action.as_ref(), accent)?,
            action_text: color("accent_text", &self.accent_text)?,
            accent,
            accent_strong,
            ai,
            attention: color("attention", &self.attention)?,
            success,
            warning,
            error: color("error", &self.error)?,
            status_muted: optional("status_muted", self.status_muted.as_ref(), text_muted)?,
            scrim: optional("scrim", self.scrim.as_ref(), scrim_fallback)?,
            code_surface: code.0,
            code_header: code.1,
            code_text: code.2,
            code_hover: code.3,
            syntax_keyword: syntax[0],
            syntax_string: syntax[1],
            syntax_number: syntax[2],
            syntax_func: syntax[3],
            syntax_type: syntax[4],
            syntax_prop: syntax[5],
            syntax_op: syntax[6],
        })
    }
}

impl ResolvedPalette {
    /// 对比度门槛（TDD-15 §3.6；E4 2026-10-06 裁决的现行标准，留了屏幕余量）。四层底含悬停：行在
    /// 指针下时文字照样要读得清。
    fn check_contrast(&self, mode: ThemeMode) -> Result<(), ThemePackageError> {
        let backgrounds = [
            ("canvas", self.canvas),
            ("surface", self.surface),
            ("panel", self.panel),
            ("hover", self.hover),
        ];
        // 正文 AA 4.5；当文字用的强调 / 状态 / 弱化色 4.7（真机截图里 4.51 量出 4.42）；控件边界与
        // 焦点 3.6（WCAG 1.4.11 的 3:1 + macOS 色彩管理压掉的约一成）。
        let rules: [(&'static str, Rgb, f64); 12] = [
            ("text_primary", self.text_primary, 4.5),
            ("text_secondary", self.text_secondary, 4.5),
            ("text_muted", self.text_muted, 4.7),
            ("accent", self.accent, 4.7),
            ("accent_strong", self.accent_strong, 4.7),
            ("ai", self.ai, 4.7),
            ("attention", self.attention, 4.7),
            ("success", self.success, 4.7),
            ("warning", self.warning, 4.7),
            ("error", self.error, 4.7),
            ("border_control", self.border_control, 3.6),
            ("focus", self.focus, 3.6),
        ];
        for (foreground, color, minimum) in rules {
            for (background, base) in backgrounds {
                if color.contrast(base) < minimum {
                    return Err(ThemePackageError::Contrast {
                        mode: mode.as_str(),
                        foreground,
                        background,
                    });
                }
            }
        }
        if self.action_text.contrast(self.action) < 4.5 {
            return Err(ThemePackageError::Contrast {
                mode: mode.as_str(),
                foreground: "accent_text",
                background: "action",
            });
        }
        Ok(())
    }

    /// 这套配色投影到的设计 Token（名字固定、顺序固定；值只可能是校验过的颜色或固定公式）。
    fn declarations(&self, mode: ThemeMode) -> Vec<(&'static str, String)> {
        let tint = |token: &str| {
            format!(
                "color-mix(in srgb, var({token}) {}%, transparent)",
                mode.tint_percent()
            )
        };
        let [r, g, b] = self.scrim.0;
        vec![
            ("--color-bg-canvas", self.canvas.to_string()),
            ("--color-bg-surface", self.surface.to_string()),
            ("--color-bg-panel", self.panel.to_string()),
            ("--color-bg-hover", self.hover.to_string()),
            ("--color-bg-disabled", self.disabled.to_string()),
            ("--color-bg-muted", self.muted.to_string()),
            ("--color-text-primary", self.text_primary.to_string()),
            ("--color-text-secondary", self.text_secondary.to_string()),
            ("--color-text-muted", self.text_muted.to_string()),
            ("--color-border-subtle", self.border_subtle.to_string()),
            ("--color-border-default", self.border_default.to_string()),
            ("--color-border-strong", self.border_strong.to_string()),
            ("--color-border-control", self.border_control.to_string()),
            ("--color-border-focus", self.focus.to_string()),
            ("--color-action-primary", self.action.to_string()),
            ("--color-action-primary-text", self.action_text.to_string()),
            ("--color-accent", self.accent.to_string()),
            ("--color-accent-strong", self.accent_strong.to_string()),
            ("--color-accent-surface", tint("--color-accent")),
            (
                "--color-bg-selected",
                "var(--color-accent-surface)".to_owned(),
            ),
            ("--color-status-ai", self.ai.to_string()),
            ("--color-status-attention", self.attention.to_string()),
            ("--color-status-active", self.success.to_string()),
            ("--color-status-success", self.success.to_string()),
            ("--color-status-warning", self.warning.to_string()),
            ("--color-status-error", self.error.to_string()),
            ("--color-status-muted", self.status_muted.to_string()),
            ("--color-status-ai-surface", tint("--color-status-ai")),
            (
                "--color-status-attention-surface",
                tint("--color-status-attention"),
            ),
            (
                "--color-status-success-surface",
                tint("--color-status-success"),
            ),
            (
                "--color-status-warning-surface",
                tint("--color-status-warning"),
            ),
            ("--color-status-error-surface", tint("--color-status-error")),
            ("--color-status-muted-surface", tint("--color-status-muted")),
            (
                "--color-scrim",
                format!("rgb({r} {g} {b} / {})", mode.scrim_alpha()),
            ),
            ("--code-surface", self.code_surface.to_string()),
            ("--code-surface-header", self.code_header.to_string()),
            ("--code-border", self.border_subtle.to_string()),
            ("--code-text", self.code_text.to_string()),
            ("--code-text-muted", self.text_muted.to_string()),
            ("--code-hover-surface", self.code_hover.to_string()),
            ("--code-hover-text", self.text_primary.to_string()),
            ("--syntax-keyword", self.syntax_keyword.to_string()),
            ("--syntax-string", self.syntax_string.to_string()),
            ("--syntax-number", self.syntax_number.to_string()),
            ("--syntax-func", self.syntax_func.to_string()),
            ("--syntax-type", self.syntax_type.to_string()),
            ("--syntax-prop", self.syntax_prop.to_string()),
            ("--syntax-op", self.syntax_op.to_string()),
        ]
    }
}

/// 一条规则：`selector { --token: value; … }`。`pretty` 给随包样式表用（一行一个声明，读得了、
/// diff 得了），运行时注入的用紧凑写法。
pub(crate) fn rule(selector: &str, declarations: &[(&str, String)], indent: &str) -> String {
    let mut out = format!("{indent}{selector} {{\n");
    for (token, value) in declarations {
        out.push_str(&format!("{indent}  {token}: {value};\n"));
    }
    out.push_str(&format!("{indent}}}\n"));
    out
}

fn compact_rule(selector: &str, declarations: &[(&str, String)]) -> String {
    let body: String = declarations
        .iter()
        .map(|(token, value)| format!("{token}:{value};"))
        .collect();
    format!("{selector}{{{body}}}")
}

impl ThemePackage {
    pub fn parse(source: &str) -> Result<Self, ThemePackageError> {
        let package: Self =
            toml::from_str(source).map_err(|error| ThemePackageError::Parse(error.to_string()))?;
        package.validate()?;
        Ok(package)
    }

    /// 写回 `theme.toml` 的样子（[`Self::parse`] 的逆）：在线目录里内联的主题包装进内容层时，落盘的是与
    /// 本地导入同一种文件，读回时走同一个 `parse`。
    ///
    /// # Panics
    /// 不会：类型里只有字符串、整数与可省略的字段，TOML 都表达得了。
    #[must_use]
    pub fn to_toml(&self) -> String {
        toml::to_string(self).expect("主题包总能写成 TOML")
    }

    pub fn validate(&self) -> Result<(), ThemePackageError> {
        if self.schema_version != THEME_PACKAGE_SCHEMA_VERSION {
            return Err(ThemePackageError::Schema(self.schema_version));
        }
        validate_text(&self.name, 1, 64, "name")?;
        validate_text(&self.author, 1, 64, "author")?;
        validate_text(&self.license, 1, 64, "license")?;
        for mode in [ThemeMode::Light, ThemeMode::Dark] {
            self.palette(mode).resolve(mode)?.check_contrast(mode)?;
        }
        Ok(())
    }

    #[must_use]
    pub fn palette(&self, mode: ThemeMode) -> &ThemePalette {
        match mode {
            ThemeMode::Light => &self.light,
            ThemeMode::Dark => &self.dark,
        }
    }

    /// 推导完的配色；未经 [`Self::validate`] 的包颜色写错时为 `None`。
    #[must_use]
    pub fn resolved(&self, mode: ThemeMode) -> Option<ResolvedPalette> {
        self.palette(mode).resolve(mode).ok()
    }

    pub(crate) fn declarations(&self, mode: ThemeMode) -> Vec<(&'static str, String)> {
        self.resolved(mode)
            .map(|palette| palette.declarations(mode))
            .unwrap_or_default()
    }

    /// 运行时注入的样式：挂在 `[data-theme-family="<family>"]` 下。包内容不能注入选择器、声明或
    /// URL——值只来自校验过的颜色与固定公式，`family` 是字符白名单内的 [`ContentId`]。未经校验、
    /// 颜色写错的包得到空规则（不会 panic）。
    #[must_use]
    pub fn css_for(&self, family: &ContentId) -> String {
        let rules: String = [ThemeMode::Light, ThemeMode::Dark]
            .into_iter()
            .map(|mode| {
                compact_rule(
                    &format!(
                        ":root[data-theme-family=\"{family}\"][data-theme=\"{}\"]",
                        mode.as_str()
                    ),
                    &self.declarations(mode),
                )
            })
            .collect();
        format!("@layer tokens.family{{{rules}}}")
    }

    /// 卡片上的色块（ADR-169 ③）：`[data-theme-swatch="<id>"]` 上的四个变量，随当前明暗取这套主题
    /// 的那一档——底色、面板、主按钮、正文。
    #[must_use]
    pub fn swatch_css(&self) -> String {
        let id = &self.id;
        let swatch = |mode: ThemeMode| {
            self.resolved(mode)
                .map(|p| {
                    vec![
                        ("--theme-swatch-canvas", p.canvas.to_string()),
                        ("--theme-swatch-panel", p.panel.to_string()),
                        ("--theme-swatch-accent", p.action.to_string()),
                        ("--theme-swatch-text", p.text_primary.to_string()),
                    ]
                })
                .unwrap_or_default()
        };
        format!(
            "@layer tokens.family{{{}{}}}",
            compact_rule(
                &format!("[data-theme-swatch=\"{id}\"]"),
                &swatch(ThemeMode::Light)
            ),
            compact_rule(
                &format!(":root[data-theme=\"dark\"] [data-theme-swatch=\"{id}\"]"),
                &swatch(ThemeMode::Dark)
            ),
        )
    }
}

fn validate_text(
    value: &str,
    min: usize,
    max: usize,
    field: &'static str,
) -> Result<(), ThemePackageError> {
    let len = value.trim().chars().count();
    if len < min || len > max || value.chars().any(char::is_control) {
        return Err(ThemePackageError::Metadata(field));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_source() -> &'static str {
        r##"
schema_version = 3
id = "test.theme"
version = "1.0.0"
name = "Test Theme"
author = "Lunarca Test"
license = "MIT"

[light]
canvas = "#ffffff"
surface = "#ffffff"
panel = "#ffffff"
text_primary = "#111111"
text_secondary = "#333333"
text_muted = "#555555"
border_subtle = "#777777"
border_default = "#666666"
border_strong = "#555555"
accent = "#0055aa"
accent_strong = "#003d7a"
accent_text = "#ffffff"
ai = "#6b258e"
attention = "#765b00"
success = "#08752f"
warning = "#984900"
error = "#b00020"

[dark]
canvas = "#111111"
surface = "#181818"
panel = "#202020"
text_primary = "#ffffff"
text_secondary = "#dddddd"
text_muted = "#bbbbbb"
border_subtle = "#888888"
border_default = "#999999"
border_strong = "#aaaaaa"
accent = "#72b8ff"
accent_strong = "#a4d1ff"
accent_text = "#111111"
ai = "#d7a5ef"
attention = "#f0d56d"
success = "#70dc91"
warning = "#ffad70"
error = "#ff8795"
"##
    }

    #[test]
    fn parses_closed_valid_package_and_generates_only_fixed_selectors() {
        let package = ThemePackage::parse(valid_source()).expect("valid theme");
        assert_eq!(package.id.as_str(), "test.theme");
        let css = package.css_for(&package.id);
        assert!(css.contains("data-theme-family=\"test.theme\"][data-theme=\"light\"]"));
        assert!(css.contains("data-theme-family=\"test.theme\"][data-theme=\"dark\"]"));
        assert!(css.contains("--color-bg-canvas:#ffffff"));
        assert!(!css.contains(package.name.as_str()));
        assert!(package
            .swatch_css()
            .contains("[data-theme-swatch=\"test.theme\"]{--theme-swatch-canvas:#ffffff;"));
    }

    #[test]
    fn omitted_refinements_are_derived_from_the_required_colors() {
        let package = ThemePackage::parse(valid_source()).expect("valid theme");
        let light = package.resolved(ThemeMode::Light).expect("resolved");
        assert_eq!(
            light.hover,
            Rgb::parse("#0055aa").unwrap().mix(10, light.surface)
        );
        assert_eq!(light.hover.to_string(), "#e6eef7");
        assert_eq!(light.focus, light.accent);
        assert_eq!(light.action, light.accent);
        assert_eq!(light.scrim, light.text_primary);
        assert_eq!(light.code_header, light.canvas);
        assert_eq!(light.syntax_type, light.ai);
        let dark = package.resolved(ThemeMode::Dark).expect("resolved");
        assert_eq!(dark.scrim, dark.canvas);
        let css = package.css_for(&package.id);
        assert!(css.contains("--color-scrim:rgb(17 17 17 / 0.55)"));
        assert!(css.contains("var(--color-accent) 12%"));
        assert!(css.contains("var(--color-accent) 16%"));
    }

    #[test]
    fn refinements_override_the_derivation_and_are_validated_too() {
        let refined = valid_source().replace(
            "error = \"#b00020\"\n\n[dark]",
            "error = \"#b00020\"\nfocus = \"#003d7a\"\n\n[light.syntax]\nkeyword = \"#a3512e\"\nstring = \"#3f7d4f\"\nnumber = \"#8a6010\"\nfunc = \"#4a6b8a\"\ntype = \"#9a4a72\"\nprop = \"#57514a\"\nop = \"#8a8378\"\n\n[dark]",
        );
        let package = ThemePackage::parse(&refined).expect("refined theme");
        let light = package.resolved(ThemeMode::Light).expect("resolved");
        assert_eq!(light.focus.to_string(), "#003d7a");
        assert_eq!(light.syntax_type.to_string(), "#9a4a72");
        // 细调同样过对比度：焦点环淡到看不见就拒。
        assert!(matches!(
            ThemePackage::parse(&refined.replace("focus = \"#003d7a\"", "focus = \"#eeeeee\"")),
            Err(ThemePackageError::Contrast {
                foreground: "focus",
                ..
            })
        ));
        // 子表要么整张填、要么不填。
        assert!(ThemePackage::parse(&refined.replace("op = \"#8a8378\"\n", "")).is_err());
    }

    #[test]
    fn rejects_unknown_fields_non_hex_and_low_contrast() {
        assert!(ThemePackage::parse(
            &valid_source().replace("license = \"MIT\"", "license = \"MIT\"\ncss = \"body{}\"")
        )
        .is_err());
        assert!(ThemePackage::parse(&valid_source().replace("#0055aa", "var(--evil)")).is_err());
        assert!(ThemePackage::parse(
            &valid_source().replace("text_primary = \"#111111\"", "text_primary = \"#eeeeee\"")
        )
        .is_err());
        // 悬停底也算一层底：强调色压不住它就拒。
        assert!(matches!(
            ThemePackage::parse(&valid_source().replace(
                "error = \"#b00020\"\n\n[dark]",
                "error = \"#b00020\"\nhover = \"#6b6b6b\"\n\n[dark]"
            )),
            Err(ThemePackageError::Contrast {
                background: "hover",
                ..
            })
        ));
    }

    #[test]
    fn rejects_older_schemas_and_ids_that_could_break_the_selector() {
        for old in [1, 2] {
            assert!(matches!(
                ThemePackage::parse(
                    &valid_source().replace("schema_version = 3", &format!("schema_version = {old}"))
                ),
                Err(ThemePackageError::Schema(found)) if found == old
            ));
        }
        assert!(ThemePackage::parse(&valid_source().replace("id = \"test.theme\"\n", "")).is_err());
        assert!(ThemePackage::parse(
            &valid_source().replace("id = \"test.theme\"", "id = \"x\\\"]{}body{\"")
        )
        .is_err());
    }

    #[test]
    fn garbled_input_never_panics() {
        // 解析器面对的是任意文件：截断与乱改都只能得到 Err，不能 panic。
        let source = valid_source();
        for cut in 0..source.len() {
            if source.is_char_boundary(cut) {
                let _ = ThemePackage::parse(&source[..cut]);
            }
        }
        let mut seed: u32 = 0x9e37_79b9;
        for _ in 0..500 {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let at = (seed as usize) % source.len();
            if !source.is_char_boundary(at) {
                continue;
            }
            let mut garbled = source.to_owned();
            garbled.insert(at, char::from(b'!' + (seed % 90) as u8));
            let _ = ThemePackage::parse(&garbled);
        }
        assert_eq!(Rgb::parse("#ééé"), None);
    }
}
