//! 附件放在哪、叫什么、正文里的链接怎么写（PRD-06-012 四模式、PRD-06-030 可移植链接；纯逻辑，无 IO）。
//!
//! 桌面「粘贴 / 拖入 / 插入附件」与网页的上传（PRD-21-025，ADR-177 ⑥）用的是这同一份：落盘目录、文件名、
//! 回填的笔记相对链接都在这里推导，调用方只管「这个名字被占了没有」（桌面看磁盘、网页看清单）。
//!
//! 约定：路径均为**库相对、`/` 分隔**；空串 = 库根。

use crate::naming::CivilTime;

/// 拼接两段库内相对路径（任一为空取另一，避免前导 / 多余斜杠）。
#[must_use]
pub fn join_vault_rel(a: &str, b: &str) -> String {
    match (a.is_empty(), b.is_empty()) {
        (true, _) => b.to_owned(),
        (_, true) => a.to_owned(),
        _ => format!("{a}/{b}"),
    }
}

/// 清洗用户配置的库内相对目录：丢弃 `.` / `..` / 空段与前后斜杠，杜绝越出库。
#[must_use]
pub fn sanitize_vault_rel_dir(raw: &str) -> String {
    raw.split('/')
        .filter(|seg| !seg.is_empty() && *seg != "." && *seg != "..")
        .collect::<Vec<_>>()
        .join("/")
}

/// `from_dir`（库内目录）到 `to_path`（库内文件路径）的**相对路径**（含 `../`）：附件位置策略据此
/// 写出可移植的笔记相对链接（PRD-06-030）。
#[must_use]
pub fn relative_vault_path(from_dir: &str, to_path: &str) -> String {
    let from: Vec<&str> = from_dir.split('/').filter(|s| !s.is_empty()).collect();
    let to: Vec<&str> = to_path.split('/').filter(|s| !s.is_empty()).collect();
    let common = from
        .iter()
        .zip(to.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let mut parts: Vec<&str> = vec![".."; from.len() - common];
    parts.extend_from_slice(&to[common..]);
    parts.join("/")
}

/// 附件落点：库内目录与回填到正文的笔记相对链接。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentTarget {
    /// 库内落盘目录（`""` = 库根）。
    pub dir_rel: String,
    /// 回填到正文的**笔记相对**链接（如 `assets/x.png` / `../Attachments/x.png` / `x.png`）。
    pub link: String,
}

/// 附件位置策略（`vault.attachment_location` 四档）→ 落盘目录 + 笔记相对链接。`mode` ∈ `vault_root` /
/// `note_folder` / `note_subfolder` / `specified_folder`，认不出的（含空串）按 `note_subfolder`；`folder`
/// 为空时子目录取 `assets`、指定目录取 `Attachments`（与设置注册表的默认值同一结果）。
#[must_use]
pub fn attachment_target(
    mode: &str,
    folder: &str,
    note_rel: &str,
    filename: &str,
) -> AttachmentTarget {
    let note_dir = crate::links::parent_dir(note_rel);
    let folder = sanitize_vault_rel_dir(folder);
    let dir_rel = match mode {
        "vault_root" => String::new(),
        "note_folder" => note_dir.to_owned(),
        "specified_folder" => {
            if folder.is_empty() {
                "Attachments".to_owned()
            } else {
                folder
            }
        }
        _ => join_vault_rel(note_dir, if folder.is_empty() { "assets" } else { &folder }),
    };
    let file_vault_rel = join_vault_rel(&dir_rel, filename);
    AttachmentTarget {
        link: relative_vault_path(note_dir, &file_vault_rel),
        dir_rel,
    }
}

/// 粘贴物的文件名前缀：图片与其它文件各一个（粘贴物没有名字，只能造一个）。
pub const PASTED_IMAGE_PREFIX: &str = "Pasted-image";
pub const PASTED_FILE_PREFIX: &str = "Pasted-file";
/// 下载远程图片得到的附件。
pub const DOWNLOADED_IMAGE_PREFIX: &str = "Downloaded-image";
/// 拖入 / 挑选的文件名字取不出可用的主干时用它。
pub const UNNAMED_ATTACHMENT_STEM: &str = "Attachment";
/// 找一个没被占的附件名最多试几次。
pub const ASSET_NAME_ATTEMPTS: u32 = 10_000;

/// 粘贴物按 MIME 取前缀。
#[must_use]
pub fn pasted_asset_prefix(mime: &str) -> &'static str {
    if mime.starts_with("image/") {
        PASTED_IMAGE_PREFIX
    } else {
        PASTED_FILE_PREFIX
    }
}

/// 粘贴物的扩展名：常见图片类型给惯用写法，其余 `image/*` 取子类型（只认字母数字），别的一律 `bin`。
#[must_use]
pub fn asset_ext_from_mime(mime: &str) -> String {
    match mime {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        "image/bmp" => "bmp",
        "image/tiff" => "tiff",
        other => other
            .strip_prefix("image/")
            .filter(|s| s.chars().all(|c| c.is_ascii_alphanumeric()) && !s.is_empty())
            .unwrap_or("bin"),
    }
    .to_owned()
}

/// 附件名里的时间戳：`YYYYMMDD-HHMMSS`（本地时刻）。
#[must_use]
pub fn asset_stamp(at: &CivilTime) -> String {
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        at.year, at.month, at.day, at.hour, at.minute, at.second
    )
}

/// 粘贴物的第 `attempt` 个候选名（0 起）：`<前缀>-<时间戳>.<扩展名>`，之后 `<前缀>-<时间戳>-1.<扩展名>`……
#[must_use]
pub fn pasted_asset_name(prefix: &str, stamp: &str, ext: &str, attempt: u32) -> String {
    if attempt == 0 {
        format!("{prefix}-{stamp}.{ext}")
    } else {
        format!("{prefix}-{stamp}-{attempt}.{ext}")
    }
}

/// 可移植的附件文件名主干：空白换成连字符，路径分隔符与控制字符剔除（PRD-06-030）。
#[must_use]
pub fn portable_asset_name(raw: &str) -> String {
    raw.chars()
        .map(|c| if c.is_whitespace() { '-' } else { c })
        .filter(|c| !matches!(c, '/' | '\\' | ':') && !c.is_control())
        .collect::<String>()
        .trim_matches('-')
        .to_owned()
}

/// 用户给了名字的文件（拖入、挑选）的第 `attempt` 个候选名（0 起）：保留它的名字（[`portable_asset_name`]），
/// 扩展名转小写（只认字母数字的扩展名），同名时主干后加 `-1`、`-2`……主干与扩展名按文件名的最后一个 `.`
/// 分（以 `.` 开头、此外没有点的名字整个是主干）。
#[must_use]
pub fn named_asset_name(file_name: &str, attempt: u32) -> String {
    let name = file_name.rsplit(['/', '\\']).next().unwrap_or(file_name);
    let (stem, ext) = match name.rfind('.') {
        Some(dot) if dot > 0 => (&name[..dot], Some(&name[dot + 1..])),
        _ => (name, None),
    };
    let stem = Some(portable_asset_name(stem))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| UNNAMED_ATTACHMENT_STEM.to_owned());
    let ext = ext
        .map(str::to_ascii_lowercase)
        .filter(|e| !e.is_empty() && e.chars().all(|c| c.is_ascii_alphanumeric()));
    let base = if attempt == 0 {
        stem
    } else {
        format!("{stem}-{attempt}")
    };
    match ext {
        Some(ext) => format!("{base}.{ext}"),
        None => base,
    }
}

/// 依次试 `candidate(0)`、`candidate(1)`……第一个 `taken` 说没被占的名字；都被占了为 `None`。
pub fn free_asset_name(
    mut candidate: impl FnMut(u32) -> String,
    mut taken: impl FnMut(&str) -> bool,
) -> Option<String> {
    (0..ASSET_NAME_ATTEMPTS)
        .map(&mut candidate)
        .find(|name| !taken(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachment_target_covers_four_modes() {
        let note = "Chats/talk.md";
        let t = attachment_target("note_subfolder", "assets", note, "p.png");
        assert_eq!(t.dir_rel, "Chats/assets");
        assert_eq!(t.link, "assets/p.png");
        let t = attachment_target("note_folder", "", note, "p.png");
        assert_eq!(t.dir_rel, "Chats");
        assert_eq!(t.link, "p.png");
        let t = attachment_target("vault_root", "", note, "p.png");
        assert_eq!(t.dir_rel, "");
        assert_eq!(t.link, "../p.png");
        let t = attachment_target("specified_folder", "Attachments", note, "p.png");
        assert_eq!(t.dir_rel, "Attachments");
        assert_eq!(t.link, "../Attachments/p.png");
        // 认不出的模式按子目录；越界段被清洗。
        let t = attachment_target("???", "../evil/sub", note, "p.png");
        assert_eq!(t.dir_rel, "Chats/evil/sub");
        // 没有设置（两项都空）= 注册表默认：笔记同级 assets。
        let t = attachment_target("", "", "root.md", "p.png");
        assert_eq!(t.dir_rel, "assets");
        assert_eq!(t.link, "assets/p.png");
        let t = attachment_target("specified_folder", "", note, "p.png");
        assert_eq!(t.dir_rel, "Attachments");
    }

    #[test]
    fn asset_ext_maps_known_mimes_and_falls_back() {
        assert_eq!(asset_ext_from_mime("image/png"), "png");
        assert_eq!(asset_ext_from_mime("image/jpeg"), "jpg");
        assert_eq!(asset_ext_from_mime("image/svg+xml"), "svg");
        assert_eq!(asset_ext_from_mime("image/heic"), "heic");
        assert_eq!(asset_ext_from_mime("application/pdf"), "bin");
        assert_eq!(asset_ext_from_mime("image/x-icon"), "bin");
        assert_eq!(pasted_asset_prefix("image/png"), PASTED_IMAGE_PREFIX);
        assert_eq!(pasted_asset_prefix("application/pdf"), PASTED_FILE_PREFIX);
    }

    #[test]
    fn pasted_names_carry_the_stamp_and_count_up() {
        let at = CivilTime {
            year: 2026,
            month: 11,
            day: 6,
            hour: 9,
            minute: 5,
            second: 7,
        };
        let stamp = asset_stamp(&at);
        assert_eq!(stamp, "20261106-090507");
        assert_eq!(
            pasted_asset_name(PASTED_IMAGE_PREFIX, &stamp, "png", 0),
            "Pasted-image-20261106-090507.png"
        );
        assert_eq!(
            pasted_asset_name(PASTED_IMAGE_PREFIX, &stamp, "png", 2),
            "Pasted-image-20261106-090507-2.png"
        );
    }

    #[test]
    fn named_assets_keep_a_portable_name() {
        assert_eq!(named_asset_name("diagram.png", 0), "diagram.png");
        assert_eq!(
            named_asset_name("季度 报告 (终).PDF", 0),
            "季度-报告-(终).pdf"
        );
        assert_eq!(named_asset_name("季度 报告.PDF", 1), "季度-报告-1.pdf");
        assert_eq!(named_asset_name("archive.tar.gz", 0), "archive.tar.gz");
        assert_eq!(named_asset_name(".hidden", 0), ".hidden");
        assert_eq!(named_asset_name("   .png", 0), "Attachment.png");
        assert_eq!(named_asset_name("说明.p df", 0), "说明");
        assert_eq!(named_asset_name("dir/a b.txt", 0), "a-b.txt");
        assert_eq!(portable_asset_name(" a:b/c "), "abc");
    }

    #[test]
    fn free_asset_name_skips_taken_names() {
        let taken = ["a.png", "a-1.png"];
        assert_eq!(
            free_asset_name(
                |n| named_asset_name("a.png", n),
                |name| taken.contains(&name)
            ),
            Some("a-2.png".to_owned())
        );
        assert_eq!(free_asset_name(|n| format!("x{n}"), |_| true), None);
    }

    #[test]
    fn relative_path_dotdot_and_descend() {
        assert_eq!(
            relative_vault_path("Chats", "Chats/assets/x.png"),
            "assets/x.png"
        );
        assert_eq!(relative_vault_path("", "x.png"), "x.png");
        assert_eq!(
            relative_vault_path("Chats", "Attachments/x.png"),
            "../Attachments/x.png"
        );
        assert_eq!(relative_vault_path("a/b", "a/c/x.png"), "../c/x.png");
    }

    #[test]
    fn sanitize_strips_traversal_segments() {
        assert_eq!(sanitize_vault_rel_dir("../evil/sub"), "evil/sub");
        assert_eq!(sanitize_vault_rel_dir("/Att/"), "Att");
        assert_eq!(sanitize_vault_rel_dir("a/./b"), "a/b");
        assert_eq!(join_vault_rel("", "b"), "b");
        assert_eq!(join_vault_rel("a", ""), "a");
        assert_eq!(join_vault_rel("a", "b"), "a/b");
    }
}
