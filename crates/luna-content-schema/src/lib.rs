//! 内容基座的公共契约（ADR-169 ①，ADR-159 ③，ADR-161）。
//!
//! 主题与模板包是两个商品域，各有自己的类型与校验（PRD-13 §1.8
//! 禁止一张统管一切的条目表）；它们共用的只有两样东西——条目 id 的写法（[`ContentId`]）与包内
//! 相对路径的写法（[`RelPath`]）。
//!
//! **零 IO**：只有类型、解析与纯函数。客户端（`luna-content`、应用装配层）与服务端（`cp-domain`
//! 按路径引用）用的是同一份校验——审核时算出的能力与安装时算出的能力对不上，就拒装
//! （ADR-161 ③）。

mod builtin;
mod id;
pub mod revocation;
pub mod template_archive;
pub mod template_catalog;
pub mod template_pack;
pub mod theme;
pub mod theme_catalog;
mod version;

pub use builtin::{builtin_stylesheet, builtin_themes, is_builtin_theme, FALLBACK_THEME_ID};
pub use id::{ContentId, ContentIdError, RelPath, RelPathError};
pub use revocation::{Revocation, REVOCATION_REASON_MAX_CHARS};
pub use template_archive::{
    sha256_hex, unpack_template_pack, PackArchiveError, UnpackedTemplatePack,
    TEMPLATE_PACK_MANIFEST,
};
pub use template_catalog::{
    validate_template_catalog, validate_template_entry, validate_template_release,
    PackContentCounts, TemplateCatalogEntry, TemplateRelease,
};
pub use template_pack::{
    Capabilities, ContentApiRange, PackAsset, PackAssetKind, Recipe, RecipeInput, RecipeStep,
    TemplatePackError, TemplatePackManifest, HOST_CONTENT_API, MAX_PACK_BYTES, MAX_PACK_FILES,
    TEMPLATE_PACK_SCHEMA_VERSION,
};
pub use theme::{
    CodePalette, ResolvedPalette, Rgb, SyntaxPalette, ThemeMode, ThemePackage, ThemePackageError,
    ThemePalette, THEME_PACKAGE_SCHEMA_VERSION,
};
pub use theme_catalog::{
    starter_theme_entry, starter_theme_release, validate_theme_catalog, validate_theme_entry,
    validate_theme_release, ThemeCatalogEntry, ThemeRelease, THEME_ID_PLACEHOLDER,
};
pub use version::{PackVersion, VersionError};
