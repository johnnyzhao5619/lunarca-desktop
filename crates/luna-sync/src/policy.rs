//! 同步对文件的分类（ADR-156）：哪些算「大文件」、什么时候算「还在写」。
//!
//! 「大文件」只有一个判定，暂停（按流量计费网络 / 电量低）、按需下载与「还在写」都问它，
//! 各写一份的话迟早一处把导入的视频当成小文件在热点上传。

/// 这台设备怎么对待录音（音视频，[`is_media`]）——设备级设置 `sync.recordings_mode`（ADR-156）。
/// 笔记与边车不受它影响，恒同步。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RecordingsMode {
    /// 别的设备的录音只登记为「仅云端」，打开时才下载；本机录的照常上传。默认。
    #[default]
    OnDemand,
    /// 全部下载到本机。
    Full,
    /// 不同步录音：本机新录的不上传，别的设备的只登记为仅云端。
    Off,
}

impl RecordingsMode {
    /// 设置值 → 档位（认不出的按默认）。
    #[must_use]
    pub fn from_setting(value: &str) -> Self {
        match value {
            "full" => Self::Full,
            "off" => Self::Off,
            _ => Self::OnDemand,
        }
    }
}

/// 库是否放在第三方同步盘里（iCloud Drive、Dropbox、OneDrive、Google Drive）。两套同步叠用要用户
/// 自己决定（ADR-148 ⑨），所以登录后不主动询问（ADR-178 ②）。
#[must_use]
pub fn in_third_party_sync_folder(path: &std::path::Path) -> bool {
    let parts: Vec<String> = path
        .components()
        .map(|part| part.as_os_str().to_string_lossy().to_lowercase())
        .collect();
    parts.windows(2).any(|pair| {
        pair[0] == "library" && (pair[1] == "mobile documents" || pair[1] == "cloudstorage")
    }) || parts
        .iter()
        .any(|part| part == "dropbox" || part == "google drive" || part.starts_with("onedrive"))
}

/// 音频 / 视频扩展名（小写）：录音与导入的音视频是同一性质。
const MEDIA_EXTENSIONS: [&str; 12] = [
    "wav", "m4a", "mp3", "aac", "flac", "ogg", "opus", "webm", "mp4", "mov", "mkv", "m4v",
];

/// 不论类型，超过它就算大文件（热点上几秒传不完的量级）。
pub const LARGE_FILE_BYTES: u64 = 20 * 1024 * 1024;

/// 大文件最近这么久之内还在变 → 当作还在写（正在录的 WAV 每秒都在落盘）。
pub const SETTLE_MS: i64 = 60_000;

/// 音频或视频文件（按扩展名）。
#[must_use]
pub fn is_media(path: &str) -> bool {
    path.rsplit_once('.').is_some_and(|(_, ext)| {
        MEDIA_EXTENSIONS
            .iter()
            .any(|known| known.eq_ignore_ascii_case(ext))
    })
}

/// 大文件：音视频，或任何超过 [`LARGE_FILE_BYTES`] 的文件。
#[must_use]
pub fn is_large(path: &str, size: u64) -> bool {
    is_media(path) || size > LARGE_FILE_BYTES
}

/// 这个大文件是不是还在写：最近 [`SETTLE_MS`] 内改过。小文件不等（笔记要即时同步）；
/// 修改时间读不到的不算（说不准就照常同步，最多推一份半截、下一轮再推完整的）。
#[must_use]
pub fn is_settling(path: &str, size: u64, modified_ms: Option<i64>, now_ms: i64) -> bool {
    is_large(path, size) && modified_ms.is_some_and(|m| now_ms.saturating_sub(m) < SETTLE_MS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_and_big_files_are_large_notes_are_not() {
        assert!(is_large("录音/assets/s-p01.wav", 10));
        assert!(is_large("a/B.MOV", 10));
        assert!(is_large("archive.zip", LARGE_FILE_BYTES + 1));
        assert!(!is_large("note.md", 10));
        assert!(!is_large("img.png", 1024));
        assert!(!is_large("wav", 10), "没有扩展名");
    }

    #[test]
    fn only_large_files_that_just_changed_are_settling() {
        let now = 1_000_000;
        assert!(is_settling("r.wav", 1, Some(now - 5_000), now));
        assert!(!is_settling("r.wav", 1, Some(now - SETTLE_MS), now));
        assert!(!is_settling("n.md", 1, Some(now), now), "笔记不等");
        assert!(!is_settling("r.wav", 1, None, now));
    }
}

#[cfg(test)]
mod third_party_tests {
    use super::in_third_party_sync_folder;
    use std::path::Path;

    #[test]
    fn recognizes_the_common_sync_folders_only() {
        for path in [
            "/Users/a/Library/Mobile Documents/com~apple~CloudDocs/Notes",
            "/Users/a/Library/CloudStorage/GoogleDrive-a/Notes",
            "/Users/a/Dropbox/Notes",
            "C:/Users/a/OneDrive - Acme/Notes",
        ] {
            assert!(in_third_party_sync_folder(Path::new(path)), "{path}");
        }
        assert!(!in_third_party_sync_folder(Path::new(
            "/Users/a/Documents/Notes"
        )));
    }
}
