//! 抓取格式选择（纯逻辑，TDD-03 §11.3）。
//!
//! 唯一约束来自下游而非上游：**抓下来的文件必须是 Lunarca 能解码的容器**（`AUDIO/VIDEO_MEDIA_EXTS`
//! 单一来源，PRD-04-058）。站点上"最好"的流常常不满足这一点——YouTube 的最高画质是分离的
//! VP9/AV1 视频流 + opus 音频流，合并需要 ffmpeg，而 opus/webm 我们的 symphonia 档位并不解。
//! 故格式串按**能力分档**给：
//!
//! | 目标 | ffmpeg 在场 | 取法 |
//! | --- | --- | --- |
//! | 仅音频 | 无关 | m4a(AAC) 优先——站点侧几乎恒有，且 symphonia 直解 |
//! | 视频 | 有 | 分离流 + 合并为 mp4，限 1080p |
//! | 视频 | 无 | 站点侧**预合并**的 mp4（通常 ≤720p），诚实降级 |
//!
//! **绝不用 `bestaudio` 兜底到 opus/webm**：那会拿到一个"下载成功却解码失败"的文件，
//! 把三方能力问题伪装成 Lunarca 的 bug。宁可诚实报 `FETCH_NO_COMPATIBLE_FORMAT`。

use serde::{Deserialize, Serialize};

/// 抓取目标：只要音轨，还是连画面一起。
///
/// **随任务走**而非读全局开关（PRD-05-031 的教训：一个全局开关静默改变整批产物的形态）；
/// 设置项 `transcribe.fetch_media` 只提供对话框的**初始值**。
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FetchMediaKind {
    /// 仅音频：体积通常是视频的 1/10 ~ 1/50，转写效果完全等价。
    Audio,
    /// 视频：多花磁盘换"对着画面校对字幕"（同步回放的分屏/剧场，PRD-04-058）。
    Video,
    /// **不保留媒体**：照常下载音频用于转写，转写完即删——笔记只留文稿与来源地址
    /// （PRD-05-006 的省磁盘档，即 PRD-05-004② 的网络版）。
    ///
    /// 转写**必须**有完整音频（whisper 吃 PCM，没有"在线流式转写"这回事），故它与
    /// [`Self::Audio`] 下载同一档；差别只在**转写之后留不留**。
    /// 代价是回放与说话人细分需要按需取回媒体，且**源视频下架后连素材都没了**
    /// ——这是用户的权衡，故不作默认。
    LinkOnly,
}

impl FetchMediaKind {
    /// 稳定序列化名（设置取值 / DTO 契约）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Audio => "audio",
            Self::Video => "video",
            Self::LinkOnly => "link_only",
        }
    }

    /// 转写完成后是否把媒体留在 Vault 里。
    #[must_use]
    pub const fn keeps_media(self) -> bool {
        !matches!(self, Self::LinkOnly)
    }

    /// 由设置值解析；未识别一律回落 `Audio`——省磁盘那一档是安全的默认。
    #[must_use]
    pub fn from_setting(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "video" => Self::Video,
            "link_only" => Self::LinkOnly,
            // 未识别一律回落"留音频"——它是**信息量最保守**的那一档：既能回放，
            // 又不像视频那样吃磁盘，更不像仅存链接那样把素材的存续押在站点上。
            _ => Self::Audio,
        }
    }
}

/// 视频档的分辨率上限：转写用不到 4K，而 4K 是 GB 级的磁盘与带宽。
///
/// > 假设：1080p 上限对"对着画面校对字幕"足够（理由：字幕校对看的是口型与场景，
/// > 不是纹理细节；上限之上的收益递减而成本线性上升。用户要原画请自行下载后导入文件）。
pub const MAX_VIDEO_HEIGHT: u32 = 1080;

/// 生成 `-f` 的格式选择串。
///
/// 每档都以**单一预合并流**收尾（`b[ext=mp4]`），保证即使前面的偏好都不可用也不会掉到
/// 需要合并却没有 ffmpeg 的境地。
#[must_use]
pub fn format_selector(kind: FetchMediaKind, ffmpeg: bool) -> String {
    match kind {
        // m4a → 任意 AAC 编码的音频流 → 预合并 mp4（其音轨恒为 AAC，symphonia 直解）。
        // 「不保留媒体」走同一档：转写照样需要完整音频，音频档下得最快、最省带宽。
        FetchMediaKind::Audio | FetchMediaKind::LinkOnly => {
            "ba[ext=m4a]/ba[acodec^=mp4a]/b[ext=mp4]".to_owned()
        }
        FetchMediaKind::Video if ffmpeg => format!(
            "bv*[height<={h}][ext=mp4]+ba[ext=m4a]/b[height<={h}][ext=mp4]/b[ext=mp4]",
            h = MAX_VIDEO_HEIGHT
        ),
        // 无 ffmpeg：只认站点已经合并好的 mp4，绝不选需要 merge 的组合。
        FetchMediaKind::Video => {
            format!("b[height<={MAX_VIDEO_HEIGHT}][ext=mp4]/b[ext=mp4]")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 音频档必须锚定 AAC/m4a——`bestaudio` 在 YouTube 上会给 opus/webm，而那解不了。
    #[test]
    fn audio_selector_pins_aac_family_and_never_falls_back_to_bestaudio() {
        let sel = format_selector(FetchMediaKind::Audio, false);
        assert!(sel.starts_with("ba[ext=m4a]"), "首选 m4a：{sel}");
        assert!(sel.contains("mp4a"), "次选任意 AAC：{sel}");
        assert!(
            !sel.split('/').any(|alt| alt == "ba" || alt == "bestaudio"),
            "裸 bestaudio 会拿到 opus/webm（下载成功但解码失败）：{sel}"
        );
        // ffmpeg 在场与否不改变音频档（单流直取，本就无需合并）。
        assert_eq!(sel, format_selector(FetchMediaKind::Audio, true));
    }

    /// 有 ffmpeg 才允许出现 `+`（合并）；没有则整串不得含它，否则会下到一半才失败。
    #[test]
    fn video_selector_only_merges_when_ffmpeg_present() {
        let with = format_selector(FetchMediaKind::Video, true);
        assert!(with.contains('+'), "有 ffmpeg 应取分离流合并：{with}");
        assert!(with.contains("height<=1080"));

        let without = format_selector(FetchMediaKind::Video, false);
        assert!(
            !without.contains('+'),
            "无 ffmpeg 时选到需合并的组合会在下载末尾才炸：{without}"
        );
        assert!(without.contains("ext=mp4"));
    }

    /// 每一档的最末兜底都必须是**预合并的 mp4**——它是"一定放得出、一定解得开"的那个。
    #[test]
    fn every_tier_ends_with_a_single_playable_container() {
        for (kind, ffmpeg) in [
            (FetchMediaKind::Audio, false),
            (FetchMediaKind::Audio, true),
            (FetchMediaKind::Video, false),
            (FetchMediaKind::Video, true),
            (FetchMediaKind::LinkOnly, false),
            (FetchMediaKind::LinkOnly, true),
        ] {
            let sel = format_selector(kind, ffmpeg);
            let last = sel.rsplit('/').next().unwrap();
            assert_eq!(last, "b[ext=mp4]", "兜底档不对：{sel}");
        }
    }

    /// 「不保留媒体」与「仅音频」下载同一档——转写必须有完整音频，没有"在线流式转写"。
    #[test]
    fn link_only_downloads_the_same_audio_tier_as_audio() {
        assert_eq!(
            format_selector(FetchMediaKind::LinkOnly, false),
            format_selector(FetchMediaKind::Audio, false)
        );
        assert!(!FetchMediaKind::LinkOnly.keeps_media());
        assert!(FetchMediaKind::Audio.keeps_media());
        assert!(FetchMediaKind::Video.keeps_media());
    }

    #[test]
    fn media_kind_setting_round_trip_and_safe_default() {
        assert_eq!(FetchMediaKind::from_setting("video"), FetchMediaKind::Video);
        assert_eq!(FetchMediaKind::from_setting("VIDEO"), FetchMediaKind::Video);
        assert_eq!(FetchMediaKind::from_setting("audio"), FetchMediaKind::Audio);
        // 未识别值回落省磁盘的那一档，而不是替用户下 GB 级视频。
        assert_eq!(FetchMediaKind::from_setting("4k"), FetchMediaKind::Audio);
        assert_eq!(FetchMediaKind::from_setting(""), FetchMediaKind::Audio);
        assert_eq!(FetchMediaKind::Video.as_str(), "video");
        assert_eq!(
            FetchMediaKind::from_setting("link_only"),
            FetchMediaKind::LinkOnly
        );
        assert_eq!(FetchMediaKind::LinkOnly.as_str(), "link_only");
    }
}
