//! 发布渠道（ADR-166）：一期视频「发到哪里」是数据，不是代码。
//!
//! - **内置渠道**随客户端发布（[`BUILTIN_CHANNELS`]）：长视频（YouTube、B 站）、短视频（抖音、
//!   快手、视频号、小红书、YouTube Shorts、TikTok、Instagram Reels）、图文 / 帖子（小红书图文、
//!   公众号、微博、X、LinkedIn）、播客与邮件简报。每个渠道声明形态与约束；**约束只写有出处的
//!   数字**，没核对过的留空（界面写「未核对」，用户可以建一个自定义渠道写自己的上限）。
//! - **自定义渠道**由用户在创作里新建，存在库里（`.luna/creator_channels.json`，随库同步）。
//!
//! 项目主页 `luna.project_channels` 记渠道 id（有序）；判定只经 [`resolve_channels`]。只产出
//! 文字与字幕，不做账号接入与自动上传（PRD-19 §7）。

use serde::{Deserialize, Serialize};

use crate::error::{Result, VaultError};

use crate::vault::Vault;

/// 渠道发什么形态（同一种形态可以发到多个渠道）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelFormat {
    /// 长视频。
    LongVideo,
    /// 短视频（竖屏为主）。
    ShortVideo,
    /// 长图文 / 文章。
    Article,
    /// 帖子 / 推文串。
    Post,
    /// 邮件简报。
    Newsletter,
    /// 音频 / 播客。
    Audio,
}

impl ChannelFormat {
    /// 全部取值（界面分组的顺序）。
    pub const ALL: [Self; 6] = [
        Self::LongVideo,
        Self::ShortVideo,
        Self::Article,
        Self::Post,
        Self::Newsletter,
        Self::Audio,
    ];
}

/// 一个渠道的约束；`None` = 没有上限或还没核对。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelLimits {
    /// 标题最多几个字符。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_max: Option<u32>,
    /// 正文 / 简介 / 文案最多几个字符。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_max: Option<u32>,
    /// 最多几个标签。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags_max: Option<u32>,
    /// 单个标签最多几个字符。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag_max: Option<u32>,
    /// 短视频最长几秒。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip_max_secs: Option<u32>,
}

/// 一个内置渠道。名字随界面语言，由界面按 id 取（`creator.channel.<id>`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuiltinChannel {
    pub id: &'static str,
    pub format: ChannelFormat,
    pub limits: ChannelLimits,
    /// 这个平台对 AI 生成 / 合成的内容有发布时要做的声明（发布须知，界面按 id 取
    /// `creator.channelNotice.<id>`）。国内平台依《人工智能生成合成内容标识办法》（2025-09-01 起）
    /// 第十条要求发布者主动声明；海外按各平台规则（YouTube「已更改或合成的内容」、TikTok 与
    /// Instagram 的 AI 标签、Apple Podcasts 指南 1.11）。
    pub ai_notice: bool,
}

const fn limits(
    title_max: Option<u32>,
    body_max: Option<u32>,
    tags_max: Option<u32>,
    tag_max: Option<u32>,
    clip_max_secs: Option<u32>,
) -> ChannelLimits {
    ChannelLimits {
        title_max,
        body_max,
        tags_max,
        tag_max,
        clip_max_secs,
    }
}

const NONE: ChannelLimits = limits(None, None, None, None, None);

/// 内置渠道（顺序即界面上的顺序）。**数字只写查得到出处的**（2026-10-20 核对）：
///
/// - 官方帮助页 / 开发文档：YouTube 标题 100、简介 5000（帮助中心按字符；Data API 按字节，中文约
///   1666 字就到顶）；YouTube Shorts 最长 3 分钟（2024-10-15 起）；X 帖子 280（加权计数，见
///   [`Channel::text_length`]）；LinkedIn 帖子 3000；公众号正文 2 万字（草稿接口）。
/// - 平台上传页 / 一致的第三方来源：B 站标题 80、简介 2000、标签 10 个 × 20 字；TikTok 与
///   Instagram 文案 2200；**Instagram 话题标签 5 个**（2025-12 起，此前 30）；公众号标题 64 字
///   （编辑器；草稿接口为 32）；小红书标题 20、正文 1000；视频号描述 1000；微博正文 2000；
///   Apple Podcasts 单集简介 4000。
/// - 抖音、快手的标题 / 描述上限查不到可靠来源，留空（界面写「未核对」，可建自定义渠道写自己的）。
pub const BUILTIN_CHANNELS: &[BuiltinChannel] = &[
    BuiltinChannel {
        id: "youtube",
        format: ChannelFormat::LongVideo,
        limits: limits(Some(100), Some(5000), None, None, None),
        ai_notice: true,
    },
    BuiltinChannel {
        id: "bilibili",
        format: ChannelFormat::LongVideo,
        limits: limits(Some(80), Some(2000), Some(10), Some(20), None),
        ai_notice: true,
    },
    BuiltinChannel {
        id: "youtube_shorts",
        format: ChannelFormat::ShortVideo,
        limits: limits(Some(100), None, None, None, Some(180)),
        ai_notice: true,
    },
    BuiltinChannel {
        id: "douyin",
        format: ChannelFormat::ShortVideo,
        limits: NONE,
        ai_notice: true,
    },
    BuiltinChannel {
        id: "kuaishou",
        format: ChannelFormat::ShortVideo,
        limits: NONE,
        ai_notice: true,
    },
    BuiltinChannel {
        id: "weixin_channels",
        format: ChannelFormat::ShortVideo,
        limits: limits(None, Some(1000), None, None, None),
        ai_notice: true,
    },
    BuiltinChannel {
        id: "xiaohongshu_video",
        format: ChannelFormat::ShortVideo,
        limits: limits(Some(20), Some(1000), None, None, None),
        ai_notice: true,
    },
    BuiltinChannel {
        id: "tiktok",
        format: ChannelFormat::ShortVideo,
        limits: limits(None, Some(2200), None, None, None),
        ai_notice: true,
    },
    BuiltinChannel {
        id: "instagram_reels",
        format: ChannelFormat::ShortVideo,
        limits: limits(None, Some(2200), Some(5), None, None),
        ai_notice: true,
    },
    BuiltinChannel {
        id: "wechat_article",
        format: ChannelFormat::Article,
        limits: limits(Some(64), Some(20000), None, None, None),
        ai_notice: true,
    },
    BuiltinChannel {
        id: "xiaohongshu_note",
        format: ChannelFormat::Post,
        limits: limits(Some(20), Some(1000), None, None, None),
        ai_notice: true,
    },
    BuiltinChannel {
        id: "weibo",
        format: ChannelFormat::Post,
        limits: limits(None, Some(2000), None, None, None),
        ai_notice: true,
    },
    BuiltinChannel {
        id: "x",
        format: ChannelFormat::Post,
        limits: limits(None, Some(280), None, None, None),
        ai_notice: false,
    },
    BuiltinChannel {
        id: "linkedin",
        format: ChannelFormat::Post,
        limits: limits(None, Some(3000), None, None, None),
        ai_notice: false,
    },
    BuiltinChannel {
        id: "newsletter",
        format: ChannelFormat::Newsletter,
        limits: NONE,
        ai_notice: false,
    },
    BuiltinChannel {
        id: "podcast",
        format: ChannelFormat::Audio,
        limits: limits(None, Some(4000), None, None, None),
        ai_notice: true,
    },
];

/// 没写过渠道的项目发到哪：YouTube 与 B 站（首发面向兼做两边的创作者，PRD-19 §1）。
pub const DEFAULT_CHANNELS: [&str; 2] = ["youtube", "bilibili"];

/// 自定义渠道 id 的前缀（与内置 id 永不相撞）。
pub const CUSTOM_CHANNEL_PREFIX: &str = "custom-";

/// 自定义渠道名的长度上限（字符）。
pub const CUSTOM_CHANNEL_NAME_MAX: usize = 40;

/// 用户新建的一个渠道。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomChannel {
    /// `custom-<8 位十六进制>`。
    pub id: String,
    pub name: String,
    pub format: ChannelFormat,
    #[serde(default)]
    pub limits: ChannelLimits,
    /// 一句渠道风格（生成文案时带上，S4 起用）。
    #[serde(default)]
    pub style: String,
}

/// 目录里的一个渠道（内置或自定义）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Channel {
    pub id: String,
    /// 自定义渠道的名字；内置为 `None`（界面按 id 取译名）。
    pub name: Option<String>,
    pub format: ChannelFormat,
    pub limits: ChannelLimits,
    /// 自定义渠道的风格说明；内置为 `None`。
    pub style: Option<String>,
    pub builtin: bool,
    /// 发布须知：这个平台要求声明 AI 内容（自定义渠道恒为 `false`）。
    pub ai_notice: bool,
}

/// 「我的渠道」里的一项（ADR-166 ⑦）：各处选择渠道时只列这些；`default` = 新项目默认发。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelFavorite {
    pub id: String,
    #[serde(default)]
    pub default: bool,
}

const FAVORITES_META: &str = "creator_channel_favorites";

const META_NAME: &str = "creator_channels";

/// 自定义渠道名：去掉首尾空白后不能为空、不超过 [`CUSTOM_CHANNEL_NAME_MAX`] 字；合规时返回
/// 整理后的名字。保存前由调用方先问它（不合规由调用方报自己的错误码）。
#[must_use]
pub fn valid_channel_name(name: &str) -> Option<String> {
    let name = name.trim();
    (!name.is_empty() && name.chars().count() <= CUSTOM_CHANNEL_NAME_MAX).then(|| name.to_owned())
}

/// 按「加权字数」计长度的渠道：X 把中日韩文字等算两个、链接一律算 23（twitter-text v3 的
/// 计数规则，X 发帖框按它判 280）。其余渠道按字符数算。
const WEIGHTED_LENGTH_CHANNELS: &[&str] = &["x"];

/// X 的加权计数里按 1 算的码位区间（twitter-text v3 `ranges`），其余按 2 算。
const LIGHT_RANGES: [(u32, u32); 4] = [(0, 4351), (8192, 8205), (8208, 8223), (8242, 8247)];

/// X 把每个链接都算成这么长（t.co 改写后的长度）。
const LINK_LENGTH: usize = 23;

fn weighted_length(text: &str) -> usize {
    let mut total = 0;
    let mut rest = text;
    while !rest.is_empty() {
        let token_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let token = &rest[..token_end];
        if token.starts_with("https://") || token.starts_with("http://") {
            total += LINK_LENGTH;
        } else {
            total += token
                .chars()
                .map(|c| {
                    let code = u32::from(c);
                    if LIGHT_RANGES
                        .iter()
                        .any(|(lo, hi)| (*lo..=*hi).contains(&code))
                    {
                        1
                    } else {
                        2
                    }
                })
                .sum::<usize>();
        }
        rest = &rest[token_end..];
        let space_end = rest
            .find(|c: char| !c.is_whitespace())
            .unwrap_or(rest.len());
        total += rest[..space_end].chars().count();
        rest = &rest[space_end..];
    }
    total
}

impl Channel {
    /// 这个渠道是不是按加权字数算长度（中日韩文字算两个）——超限提示要说明，否则「155 字超出
    /// 280」让人摸不着头脑。
    #[must_use]
    pub fn counts_weighted(&self) -> bool {
        WEIGHTED_LENGTH_CHANNELS.contains(&self.id.as_str())
    }

    /// 一段文字在这个渠道上算多长（与它的上限比）：X 用加权计数，其余按字符数。
    #[must_use]
    pub fn text_length(&self, text: &str) -> usize {
        if self.counts_weighted() {
            weighted_length(text)
        } else {
            text.chars().count()
        }
    }
}

/// 按 id 找内置渠道。
#[must_use]
pub fn builtin_channel(id: &str) -> Option<&'static BuiltinChannel> {
    BUILTIN_CHANNELS.iter().find(|channel| channel.id == id)
}

/// frontmatter 里的渠道 → 认得的渠道 id（按写的顺序、去重）；内置或在 `custom` 里的才算。
/// 一个都没有（没写过，或写的全不认识、自定义渠道被删了）→ [`DEFAULT_CHANNELS`]。**唯一判据。**
#[must_use]
pub fn resolve_channels(raw: &[String], custom: &[CustomChannel]) -> Vec<String> {
    let mut picked: Vec<String> = Vec::new();
    for value in raw {
        let id = value.trim();
        let known = builtin_channel(id).is_some() || custom.iter().any(|channel| channel.id == id);
        if known && !picked.iter().any(|existing| existing == id) {
            picked.push(id.to_owned());
        }
    }
    if picked.is_empty() {
        DEFAULT_CHANNELS.iter().map(|id| (*id).to_owned()).collect()
    } else {
        picked
    }
}

impl Vault {
    /// 用户的自定义渠道（按新建顺序）。
    ///
    /// # Errors
    /// 元数据文件损坏时返回错误。
    pub fn custom_channels(&self) -> Result<Vec<CustomChannel>> {
        crate::meta::read_meta::<CustomChannel>(self.root(), META_NAME)
    }

    /// 渠道目录：内置在前，自定义在后。自定义读不出时只给内置——一个坏掉的元数据文件不该
    /// 让发布页签空掉。
    #[must_use]
    pub fn channel_catalog(&self) -> Vec<Channel> {
        let mut channels: Vec<Channel> = BUILTIN_CHANNELS
            .iter()
            .map(|channel| Channel {
                id: channel.id.to_owned(),
                name: None,
                format: channel.format,
                limits: channel.limits,
                style: None,
                builtin: true,
                ai_notice: channel.ai_notice,
            })
            .collect();
        channels.extend(
            self.custom_channels()
                .unwrap_or_default()
                .into_iter()
                .map(|channel| Channel {
                    id: channel.id,
                    name: Some(channel.name),
                    format: channel.format,
                    limits: channel.limits,
                    style: Some(channel.style),
                    builtin: false,
                    ai_notice: false,
                }),
        );
        channels
    }

    /// 新建（`id` 为空）或改一个自定义渠道，返回存下的那一份。名字要先过
    /// [`valid_channel_name`]；改的是不存在的 id 时报错。
    ///
    /// # Errors
    /// id 不存在或读写失败时返回错误。
    pub fn save_custom_channel(&self, mut channel: CustomChannel) -> Result<CustomChannel> {
        channel.name = channel.name.trim().to_owned();
        channel.style = channel.style.trim().to_owned();
        let mut channels = self.custom_channels()?;
        if channel.id.is_empty() {
            // v7 的前段是时间戳，取末尾 8 位（随机段），同一毫秒建两个也不撞。
            let suffix = uuid::Uuid::now_v7().simple().to_string();
            channel.id = format!("{CUSTOM_CHANNEL_PREFIX}{}", &suffix[suffix.len() - 8..]);
            channels.push(channel.clone());
            // 新建的渠道当然是「我的渠道」：不加进去，建完在选择里找不到它。
            let mut favorites = self.channel_favorites();
            favorites.push(ChannelFavorite {
                id: channel.id.clone(),
                default: false,
            });
            crate::meta::write_meta(self.root(), META_NAME, &channels)?;
            crate::meta::write_meta(self.root(), FAVORITES_META, &favorites)?;
            return Ok(channel);
        } else {
            let existing = channels
                .iter_mut()
                .find(|existing| existing.id == channel.id)
                .ok_or_else(|| VaultError::NotFound(channel.id.clone().into()))?;
            *existing = channel.clone();
        }
        crate::meta::write_meta(self.root(), META_NAME, &channels)?;
        Ok(channel)
    }

    /// 删一个自定义渠道。项目主页里记着它的 id 不动（内容不丢）；判定时认不出就不算，
    /// 一个渠道都不剩的项目回到默认渠道。
    ///
    /// # Errors
    /// 读写失败时返回错误。
    pub fn delete_custom_channel(&self, id: &str) -> Result<()> {
        let mut channels = self.custom_channels()?;
        channels.retain(|channel| channel.id != id);
        crate::meta::write_meta(self.root(), META_NAME, &channels)?;
        let favorites: Vec<ChannelFavorite> = self
            .channel_favorites()
            .into_iter()
            .filter(|favorite| favorite.id != id)
            .collect();
        if !favorites.is_empty() {
            crate::meta::write_meta(self.root(), FAVORITES_META, &favorites)?;
        }
        Ok(())
    }

    /// 「我的渠道」（ADR-166 ⑦），按存的顺序：认不出的 id 丢掉、重复只留首个；文件没有、读不出
    /// 或一个都不剩时是默认的 YouTube 与 B 站（都是新项目默认）。**唯一判据。**
    #[must_use]
    pub fn channel_favorites(&self) -> Vec<ChannelFavorite> {
        let custom = self.custom_channels().unwrap_or_default();
        let known = |id: &str| builtin_channel(id).is_some() || custom.iter().any(|c| c.id == id);
        let mut out: Vec<ChannelFavorite> = Vec::new();
        for favorite in crate::meta::read_meta::<ChannelFavorite>(self.root(), FAVORITES_META)
            .unwrap_or_default()
        {
            if known(&favorite.id) && !out.iter().any(|existing| existing.id == favorite.id) {
                out.push(favorite);
            }
        }
        if out.is_empty() {
            return DEFAULT_CHANNELS
                .iter()
                .map(|id| ChannelFavorite {
                    id: (*id).to_owned(),
                    default: true,
                })
                .collect();
        }
        out
    }

    /// 存「我的渠道」：按 [`Vault::channel_favorites`] 同一规则归一化后写盘，返回存下的那一份。
    /// 一个都不剩时不写（至少留一个，界面也不让去掉最后一个）。
    ///
    /// # Errors
    /// 写盘失败时返回错误。
    pub fn set_channel_favorites(
        &self,
        favorites: &[ChannelFavorite],
    ) -> Result<Vec<ChannelFavorite>> {
        let custom = self.custom_channels().unwrap_or_default();
        let known = |id: &str| builtin_channel(id).is_some() || custom.iter().any(|c| c.id == id);
        let mut out: Vec<ChannelFavorite> = Vec::new();
        for favorite in favorites {
            if known(&favorite.id) && !out.iter().any(|existing| existing.id == favorite.id) {
                out.push(favorite.clone());
            }
        }
        if !out.is_empty() {
            crate::meta::write_meta(self.root(), FAVORITES_META, &out)?;
        }
        Ok(self.channel_favorites())
    }

    /// 新项目默认发到哪些渠道：「我的渠道」里标了默认的，按存的顺序；一个都没标时取第一个。
    #[must_use]
    pub fn default_project_channels(&self) -> Vec<String> {
        let favorites = self.channel_favorites();
        let picked: Vec<String> = favorites
            .iter()
            .filter(|favorite| favorite.default)
            .map(|favorite| favorite.id.clone())
            .collect();
        if picked.is_empty() {
            favorites
                .into_iter()
                .take(1)
                .map(|favorite| favorite.id)
                .collect()
        } else {
            picked
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn custom(id: &str) -> CustomChannel {
        CustomChannel {
            id: id.to_owned(),
            name: "我的博客".to_owned(),
            format: ChannelFormat::Article,
            limits: ChannelLimits::default(),
            style: String::new(),
        }
    }

    #[test]
    fn channels_resolve_in_written_order_and_default_when_none_is_known() {
        let raw = |items: &[&str]| items.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(resolve_channels(&[], &[]), DEFAULT_CHANNELS);
        assert_eq!(resolve_channels(&raw(&["myspace"]), &[]), DEFAULT_CHANNELS);
        assert_eq!(
            resolve_channels(&raw(&["douyin", " youtube ", "douyin"]), &[]),
            ["douyin", "youtube"]
        );
        assert_eq!(
            resolve_channels(&raw(&["custom-1a2b3c4d"]), &[custom("custom-1a2b3c4d")]),
            ["custom-1a2b3c4d"]
        );
        assert_eq!(
            resolve_channels(&raw(&["custom-1a2b3c4d"]), &[]),
            DEFAULT_CHANNELS,
            "自定义渠道被删了：回到默认"
        );
    }

    #[test]
    fn builtin_ids_are_unique_and_never_look_custom() {
        for (index, channel) in BUILTIN_CHANNELS.iter().enumerate() {
            assert!(!channel.id.starts_with(CUSTOM_CHANNEL_PREFIX));
            assert!(BUILTIN_CHANNELS[..index]
                .iter()
                .all(|other| other.id != channel.id));
        }
    }

    #[test]
    fn custom_channels_are_created_edited_and_deleted_in_the_vault() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        let saved = vault
            .save_custom_channel(CustomChannel {
                name: "  我的博客 ".to_owned(),
                ..custom("")
            })
            .unwrap();
        assert!(saved.id.starts_with(CUSTOM_CHANNEL_PREFIX) && saved.id.len() == 15);
        assert_eq!(saved.name, "我的博客");
        let edited = vault
            .save_custom_channel(CustomChannel {
                limits: ChannelLimits {
                    title_max: Some(30),
                    ..ChannelLimits::default()
                },
                ..saved.clone()
            })
            .unwrap();
        assert_eq!(
            vault.custom_channels().unwrap(),
            std::slice::from_ref(&edited)
        );
        let catalog = vault.channel_catalog();
        assert_eq!(catalog.len(), BUILTIN_CHANNELS.len() + 1);
        assert_eq!(catalog.last().unwrap().name.as_deref(), Some("我的博客"));
        assert!(vault
            .save_custom_channel(custom("custom-missing0"))
            .is_err());
        assert_eq!(
            valid_channel_name("  我的博客 ").as_deref(),
            Some("我的博客")
        );
        assert_eq!(valid_channel_name(" "), None);
        assert_eq!(valid_channel_name(&"长".repeat(41)), None);
        vault.delete_custom_channel(&edited.id).unwrap();
        assert!(vault.custom_channels().unwrap().is_empty());
    }

    #[test]
    fn x_counts_cjk_as_two_and_links_as_twenty_three() {
        let x = Channel {
            id: "x".into(),
            name: None,
            format: ChannelFormat::Post,
            limits: ChannelLimits::default(),
            style: None,
            builtin: true,
            ai_notice: false,
        };
        assert_eq!(x.text_length("hello world"), 11);
        assert_eq!(x.text_length("你好"), 4);
        assert_eq!(
            x.text_length("看 https://example.com/a/very/long/path 吧"),
            2 + 1 + 23 + 1 + 2
        );
        assert_eq!(x.text_length("1/ 开场"), 2 + 1 + 4);
        let weibo = Channel {
            id: "weibo".into(),
            ..x
        };
        assert_eq!(weibo.text_length("你好"), 2, "其它渠道按字符数");
    }

    #[test]
    fn favorites_default_to_youtube_and_bilibili_and_follow_custom_channels() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        assert_eq!(vault.default_project_channels(), DEFAULT_CHANNELS);
        let saved = vault
            .set_channel_favorites(&[
                ChannelFavorite {
                    id: "douyin".into(),
                    default: false,
                },
                ChannelFavorite {
                    id: "myspace".into(),
                    default: true,
                },
                ChannelFavorite {
                    id: "x".into(),
                    default: true,
                },
                ChannelFavorite {
                    id: "douyin".into(),
                    default: true,
                },
            ])
            .unwrap();
        assert_eq!(
            saved,
            [
                ChannelFavorite {
                    id: "douyin".into(),
                    default: false
                },
                ChannelFavorite {
                    id: "x".into(),
                    default: true
                },
            ],
            "认不出的丢掉、重复留首个"
        );
        assert_eq!(vault.default_project_channels(), ["x"]);
        let blog = vault
            .save_custom_channel(CustomChannel {
                id: String::new(),
                ..custom("")
            })
            .unwrap();
        assert!(
            vault
                .channel_favorites()
                .iter()
                .any(|f| f.id == blog.id && !f.default),
            "新建的进我的渠道"
        );
        vault.delete_custom_channel(&blog.id).unwrap();
        assert!(
            !vault.channel_favorites().iter().any(|f| f.id == blog.id),
            "删掉的退出我的渠道"
        );
        vault
            .set_channel_favorites(&[ChannelFavorite {
                id: "douyin".into(),
                default: false,
            }])
            .unwrap();
        assert_eq!(
            vault.default_project_channels(),
            ["douyin"],
            "没标默认就取第一个"
        );
    }
}
