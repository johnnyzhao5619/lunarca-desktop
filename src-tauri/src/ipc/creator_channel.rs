//! 创作的发布渠道（ADR-166）：渠道目录（内置 + 这个库的自定义渠道）、自定义渠道的增改删、
//! 「我的渠道」（各处选渠道时只列这些，`default` = 新项目默认发，ADR-166 ⑦）。
//!
//! 目录、上限、发布须知与「我的渠道」的归一化都在 `luna_vault::channel`；这里只转 DTO、
//! 检查创作已启用（ADR-003）。

use super::*;
use luna_vault::channel::{Channel, ChannelFormat, ChannelLimits, CustomChannel};

/// 渠道的形态（ADR-166）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChannelFormatDto {
    LongVideo,
    ShortVideo,
    Article,
    Post,
    Newsletter,
    Audio,
}

impl From<ChannelFormat> for ChannelFormatDto {
    fn from(value: ChannelFormat) -> Self {
        match value {
            ChannelFormat::LongVideo => Self::LongVideo,
            ChannelFormat::ShortVideo => Self::ShortVideo,
            ChannelFormat::Article => Self::Article,
            ChannelFormat::Post => Self::Post,
            ChannelFormat::Newsletter => Self::Newsletter,
            ChannelFormat::Audio => Self::Audio,
        }
    }
}

impl From<ChannelFormatDto> for ChannelFormat {
    fn from(value: ChannelFormatDto) -> Self {
        match value {
            ChannelFormatDto::LongVideo => Self::LongVideo,
            ChannelFormatDto::ShortVideo => Self::ShortVideo,
            ChannelFormatDto::Article => Self::Article,
            ChannelFormatDto::Post => Self::Post,
            ChannelFormatDto::Newsletter => Self::Newsletter,
            ChannelFormatDto::Audio => Self::Audio,
        }
    }
}

/// 渠道的约束；`None` = 没有上限或还没核对。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, Type, PartialEq, Eq)]
pub struct ChannelLimitsDto {
    pub title_max: Option<u32>,
    pub body_max: Option<u32>,
    pub tags_max: Option<u32>,
    pub tag_max: Option<u32>,
    pub clip_max_secs: Option<u32>,
}

impl From<ChannelLimits> for ChannelLimitsDto {
    fn from(value: ChannelLimits) -> Self {
        Self {
            title_max: value.title_max,
            body_max: value.body_max,
            tags_max: value.tags_max,
            tag_max: value.tag_max,
            clip_max_secs: value.clip_max_secs,
        }
    }
}

impl From<ChannelLimitsDto> for ChannelLimits {
    fn from(value: ChannelLimitsDto) -> Self {
        // 0 或负数没有意义：当作没有上限。
        let positive = |n: Option<u32>| n.filter(|n| *n > 0);
        Self {
            title_max: positive(value.title_max),
            body_max: positive(value.body_max),
            tags_max: positive(value.tags_max),
            tag_max: positive(value.tag_max),
            clip_max_secs: positive(value.clip_max_secs),
        }
    }
}

/// 渠道目录里的一个渠道（内置或自定义）。
#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
pub struct ChannelDto {
    pub id: String,
    /// 自定义渠道的名字；内置为 `None`（界面按 id 取译名）。
    pub name: Option<String>,
    pub format: ChannelFormatDto,
    pub limits: ChannelLimitsDto,
    /// 自定义渠道的风格说明；内置为 `None`。
    pub style: Option<String>,
    pub builtin: bool,
    /// 发布须知：这个平台要求发布时声明 AI 内容（界面按 id 取 `creator.channelNotice.<id>`）。
    pub ai_notice: bool,
}

impl From<Channel> for ChannelDto {
    fn from(value: Channel) -> Self {
        Self {
            id: value.id,
            name: value.name,
            format: value.format.into(),
            limits: value.limits.into(),
            style: value.style,
            builtin: value.builtin,
            ai_notice: value.ai_notice,
        }
    }
}

/// 「我的渠道」里的一项（ADR-166 ⑦）：选择渠道时只列这些；`default` = 新项目默认发。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ChannelFavoriteDto {
    pub id: String,
    pub default: bool,
}

fn favorite_dtos(favorites: Vec<luna_vault::channel::ChannelFavorite>) -> Vec<ChannelFavoriteDto> {
    favorites
        .into_iter()
        .map(|favorite| ChannelFavoriteDto {
            id: favorite.id,
            default: favorite.default,
        })
        .collect()
}

/// 「我的渠道」，按存的顺序（没存过 = YouTube 与 B 站，都是新项目默认）。
///
/// # Errors
/// 库未打开时返回错误。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn creator_channel_favorites(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<ChannelFavoriteDto>, String> {
    state.with_vault(window.label(), |vault| {
        Ok(favorite_dtos(vault.channel_favorites()))
    })
}

/// 存「我的渠道」（管理渠道对话框、入门第一步），返回归一化后存下的那一份。认不出的 id 丢掉；
/// 一个都不剩时不写。
///
/// # Errors
/// 库未打开或写盘失败时返回错误。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn creator_set_channel_favorites(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    favorites: Vec<ChannelFavoriteDto>,
) -> Result<Vec<ChannelFavoriteDto>, String> {
    let favorites: Vec<luna_vault::channel::ChannelFavorite> = favorites
        .into_iter()
        .map(|favorite| luna_vault::channel::ChannelFavorite {
            id: favorite.id,
            default: favorite.default,
        })
        .collect();
    state.with_vault(window.label(), |vault| {
        Ok(favorite_dtos(
            vault.set_channel_favorites(&favorites).map_err(vault_err)?,
        ))
    })
}

/// 新建或修改一个自定义渠道（`id` 为空 = 新建）。
#[derive(Debug, Clone, Deserialize, Type)]
pub struct CustomChannelInputDto {
    pub id: Option<String>,
    pub name: String,
    pub format: ChannelFormatDto,
    pub limits: ChannelLimitsDto,
    pub style: String,
}

/// 渠道目录：内置在前、自定义在后。
///
/// # Errors
/// 库未打开时返回错误。
#[tauri::command]
#[specta::specta]
pub(crate) fn creator_channels(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<ChannelDto>, String> {
    state.with_vault(window.label(), |vault| {
        Ok(vault
            .channel_catalog()
            .into_iter()
            .map(Into::into)
            .collect())
    })
}

/// 新建或修改一个自定义渠道，返回存下的那一份（ADR-166）。
///
/// # Errors
/// 名字为空或超过 40 字、要改的渠道不存在、库未打开或写盘失败时返回错误。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn creator_save_channel(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    channel: CustomChannelInputDto,
) -> Result<ChannelDto, String> {
    super::require_app(&app, &state, luna_config::apps::CREATOR_APP.id)?;
    let name = luna_vault::channel::valid_channel_name(&channel.name)
        .ok_or_else(|| coded_error(&app, "CREATOR_CHANNEL_NAME", "creator-channel-name"))?;
    let saved = state.with_vault(window.label(), |vault| {
        vault
            .save_custom_channel(CustomChannel {
                id: channel.id.unwrap_or_default(),
                name,
                format: channel.format.into(),
                limits: channel.limits.into(),
                style: channel.style,
            })
            .map_err(vault_err)
    })?;
    Ok(ChannelDto {
        id: saved.id,
        name: Some(saved.name),
        format: saved.format.into(),
        limits: saved.limits.into(),
        style: Some(saved.style),
        builtin: false,
        ai_notice: false,
    })
}

/// 删一个自定义渠道（项目主页里记着的 id 不动，内容不丢）。
///
/// # Errors
/// 库未打开或写盘失败时返回错误。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn creator_delete_channel(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    super::require_app(&app, &state, luna_config::apps::CREATOR_APP.id)?;
    state.with_vault(window.label(), |vault| {
        vault.delete_custom_channel(&id).map_err(vault_err)
    })
}

/// 新项目默认发到哪些渠道（「我的渠道」里标了默认的；新建项目对话框的初值）。
///
/// # Errors
/// 库未打开时返回错误。
#[tauri::command]
#[specta::specta]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn creator_default_channels(
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<String>, String> {
    state.with_vault(window.label(), |vault| Ok(vault.default_project_channels()))
}
