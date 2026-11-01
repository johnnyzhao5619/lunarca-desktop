//! Windows 剪贴板（ADR-163）：读写文本、`GetClipboardSequenceNumber`（对应 macOS 的 `changeCount`）、
//! 多格式备份与还原。
//!
//! - 备份只收「内存块」格式（`HGLOBAL`）：位图、调色板、图元文件等 GDI 句柄格式与私有格式无法
//!   按字节复制，[`restorable_formats`] 把它们排除；系统由 Unicode 文本合成的 ANSI / OEM 文本也不收。
//! - 备份有时间预算：延迟渲染的应用（Excel 一次复制挂几十种格式）要逐个现算，超出预算就停，
//!   备份记为不完整，还原后如实报「没能完整还原」（[`restore_report`]），不假装成功。
//! - 我们临时写进去的文本带三个登记格式，让系统剪贴板历史（Win+V）、云剪贴板与剪贴板监视工具
//!   跳过它（PRD-01-027 的 Windows 对应物）。
//!
//! 判定是纯函数，在任何平台上单测；系统调用只在 `cfg(target_os = "windows")` 的薄层里。
//! 日志里绝不写剪贴板内容（TDD-12）。

/// 标准剪贴板格式编号（`winuser.h`）。
pub const CF_TEXT: u32 = 1;
pub const CF_BITMAP: u32 = 2;
pub const CF_METAFILEPICT: u32 = 3;
pub const CF_OEMTEXT: u32 = 7;
pub const CF_PALETTE: u32 = 9;
pub const CF_UNICODETEXT: u32 = 13;
pub const CF_ENHMETAFILE: u32 = 14;
pub const CF_OWNERDISPLAY: u32 = 0x0080;
pub const CF_DSPBITMAP: u32 = 0x0082;
pub const CF_DSPMETAFILEPICT: u32 = 0x0083;
pub const CF_DSPENHMETAFILE: u32 = 0x008E;
/// 私有格式与 GDI 对象格式区间：内容是句柄，语义只有放的那个应用知道。
pub const CF_PRIVATE_AND_GDI: std::ops::RangeInclusive<u32> = 0x0200..=0x03FF;

/// 剪贴板里列出的格式中，哪些能按字节备份并原样放回（顺序保持）。
#[must_use]
pub fn restorable_formats(listed: &[u32]) -> Vec<u32> {
    let has_unicode = listed.contains(&CF_UNICODETEXT);
    listed
        .iter()
        .copied()
        .filter(|format| {
            let handle_format = matches!(
                *format,
                CF_BITMAP
                    | CF_METAFILEPICT
                    | CF_PALETTE
                    | CF_ENHMETAFILE
                    | CF_OWNERDISPLAY
                    | CF_DSPBITMAP
                    | CF_DSPMETAFILEPICT
                    | CF_DSPENHMETAFILE
            ) || CF_PRIVATE_AND_GDI.contains(format);
            // 有 Unicode 文本时，ANSI / OEM 文本是系统合成的，放回 Unicode 那份即可。
            let synthesized = has_unicode && matches!(*format, CF_TEXT | CF_OEMTEXT);
            !handle_format && !synthesized
        })
        .collect()
}

/// 一次「备份 → 用剪贴板 → 还原」之后，用户原来的剪贴板怎么样了。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardRestore {
    /// 原样还原。
    Restored,
    /// 还原了，但备份时有格式没取到（超出时间预算或读取失败）：内容可能少了一部分。
    Incomplete,
    /// 没能写回去（剪贴板被别的程序占着）：用户原来的内容已经丢了。
    Failed,
}

impl ClipboardRestore {
    /// 用户原来的剪贴板内容是否完好（界面上据此决定要不要如实提示）。
    #[must_use]
    pub fn is_intact(self) -> bool {
        self == Self::Restored
    }
}

/// 还原结论（纯逻辑）：写回失败优先；写回成功再看备份是否完整。
#[must_use]
pub fn restore_report(backup_complete: bool, restore_ok: bool) -> ClipboardRestore {
    match (restore_ok, backup_complete) {
        (false, _) => ClipboardRestore::Failed,
        (true, false) => ClipboardRestore::Incomplete,
        (true, true) => ClipboardRestore::Restored,
    }
}

/// 剪贴板多格式备份（格式编号 → 原始字节）。
#[derive(Debug, Default)]
pub struct ClipboardBackup {
    formats: Vec<(u32, Vec<u8>)>,
    complete: bool,
}

impl ClipboardBackup {
    /// 备份时每个可还原格式都取到了。
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.complete
    }
}

/// UTF-16（不含结尾 0）→ 剪贴板文本块的字节：小端 UTF-16 加一个 0 码元。
#[must_use]
pub fn unicode_text_bytes(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// 剪贴板文本块的字节 → 文本：读到第一个 0 码元为止；奇数尾字节丢弃。
#[must_use]
pub fn text_from_unicode_bytes(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .take_while(|unit| *unit != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

#[cfg(target_os = "windows")]
pub use imp::{backup, read_text, restore, sequence_number, write_text, write_text_transient};

#[cfg(target_os = "windows")]
mod imp {
    use super::{
        restorable_formats, text_from_unicode_bytes, unicode_text_bytes, ClipboardBackup,
        CF_UNICODETEXT,
    };
    use std::time::{Duration, Instant};
    use windows::core::w;
    use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData,
        GetClipboardSequenceNumber, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
    };
    use windows::Win32::System::Memory::{
        GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
    };

    /// 打开剪贴板的重试：别的程序正持有它时 `OpenClipboard` 立即失败。
    const OPEN_ATTEMPTS: u32 = 20;
    const OPEN_RETRY: Duration = Duration::from_millis(10);
    /// 备份的时间预算（延迟渲染的格式要现算）。
    const BACKUP_BUDGET: Duration = Duration::from_millis(500);

    /// 持有已打开的剪贴板；drop 即关闭。
    struct OpenGuard;

    impl OpenGuard {
        fn open() -> Option<Self> {
            for _ in 0..OPEN_ATTEMPTS {
                // SAFETY: 不关联窗口地打开剪贴板；成功后由 Drop 关闭。
                if unsafe { OpenClipboard(None) }.is_ok() {
                    return Some(Self);
                }
                std::thread::sleep(OPEN_RETRY);
            }
            None
        }
    }

    impl Drop for OpenGuard {
        fn drop(&mut self) {
            // SAFETY: 只在 open() 成功后存在。
            unsafe {
                let _ = CloseClipboard();
            }
        }
    }

    /// 当前剪贴板序号：每次内容变化自增（对应 macOS `changeCount`）。
    #[must_use]
    pub fn sequence_number() -> u32 {
        // SAFETY: 无参数的只读查询。
        unsafe { GetClipboardSequenceNumber() }
    }

    /// 读一个格式的内存块字节（剪贴板已打开）。
    fn read_format(format: u32) -> Option<Vec<u8>> {
        // SAFETY: 剪贴板已由调用方打开；句柄归剪贴板所有，只在 Lock/Unlock 之间读取、不释放。
        unsafe {
            let handle = GetClipboardData(format).ok()?;
            let global = HGLOBAL(handle.0);
            let size = GlobalSize(global);
            let ptr = GlobalLock(global);
            if ptr.is_null() {
                return None;
            }
            let bytes = std::slice::from_raw_parts(ptr.cast::<u8>(), size).to_vec();
            let _ = GlobalUnlock(global);
            Some(bytes)
        }
    }

    /// 把字节放成一个格式（剪贴板已打开并已清空）。成功后内存归剪贴板所有。
    fn set_format(format: u32, bytes: &[u8]) -> bool {
        // SAFETY: 新分配可移动内存块、拷入字节；SetClipboardData 成功即转交所有权，失败则自行释放。
        unsafe {
            let Ok(global) = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)) else {
                return false;
            };
            let ptr = GlobalLock(global);
            if ptr.is_null() {
                let _ = GlobalFree(Some(global));
                return false;
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr.cast::<u8>(), bytes.len());
            let _ = GlobalUnlock(global);
            if SetClipboardData(format, Some(HANDLE(global.0))).is_ok() {
                true
            } else {
                let _ = GlobalFree(Some(global));
                false
            }
        }
    }

    /// 读剪贴板文本（Unicode）；没有文本或打不开为 `None`。
    #[must_use]
    pub fn read_text() -> Option<String> {
        let _open = OpenGuard::open()?;
        read_format(CF_UNICODETEXT).map(|bytes| text_from_unicode_bytes(&bytes))
    }

    /// 写入文本（持久：用户「复制」的结果，进剪贴板历史）。
    pub fn write_text(text: &str) -> bool {
        write_text_with(text, false)
    }

    /// 写入我们临时放进去、随后就会还原的文本：带上让剪贴板历史 / 云剪贴板 / 监视工具忽略它的标记。
    pub fn write_text_transient(text: &str) -> bool {
        write_text_with(text, true)
    }

    fn write_text_with(text: &str, transient: bool) -> bool {
        let Some(_open) = OpenGuard::open() else {
            return false;
        };
        // SAFETY: 剪贴板已打开。
        if unsafe { EmptyClipboard() }.is_err() {
            return false;
        }
        if !set_format(CF_UNICODETEXT, &unicode_text_bytes(text)) {
            return false;
        }
        if transient {
            let zero = 0u32.to_le_bytes();
            // SAFETY: 登记格式名是静态宽字符串。
            let (exclude, history, cloud) = unsafe {
                (
                    RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing")),
                    RegisterClipboardFormatW(w!("CanIncludeInClipboardHistory")),
                    RegisterClipboardFormatW(w!("CanUploadToCloudClipboard")),
                )
            };
            for format in [exclude, history, cloud] {
                if format != 0 {
                    let _ = set_format(format, &zero);
                }
            }
        }
        true
    }

    /// 备份全部可还原格式（有时间预算，超出即停并记为不完整）。打不开剪贴板时为 `None`。
    #[must_use]
    pub fn backup() -> Option<ClipboardBackup> {
        let _open = OpenGuard::open()?;
        let mut listed = Vec::new();
        let mut format = 0;
        loop {
            // SAFETY: 剪贴板已打开；返回 0 表示列举结束。
            format = unsafe { EnumClipboardFormats(format) };
            if format == 0 {
                break;
            }
            listed.push(format);
        }
        let started = Instant::now();
        let mut out = ClipboardBackup {
            formats: Vec::new(),
            complete: true,
        };
        for format in restorable_formats(&listed) {
            if started.elapsed() > BACKUP_BUDGET {
                out.complete = false;
                break;
            }
            match read_format(format) {
                Some(bytes) => out.formats.push((format, bytes)),
                None => out.complete = false,
            }
        }
        Some(out)
    }

    /// 原样放回备份（空备份 = 原来就是空的，清空即可）。返回是否写回成功。
    pub fn restore(backup: &ClipboardBackup) -> bool {
        let Some(_open) = OpenGuard::open() else {
            return false;
        };
        // SAFETY: 剪贴板已打开。
        if unsafe { EmptyClipboard() }.is_err() {
            return false;
        }
        backup
            .formats
            .iter()
            .fold(true, |ok, (format, bytes)| set_format(*format, bytes) && ok)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gdi_handle_and_private_formats_are_not_byte_copied() {
        let listed = [
            CF_UNICODETEXT,
            CF_BITMAP,
            8, // CF_DIB：内存块，可复制
            CF_ENHMETAFILE,
            15, // CF_HDROP：内存块
            0x0250,
            0x0310,
            0xC123, // 登记格式（如 HTML Format）
        ];
        assert_eq!(
            restorable_formats(&listed),
            vec![CF_UNICODETEXT, 8, 15, 0xC123]
        );
    }

    #[test]
    fn synthesized_ansi_text_is_dropped_only_when_unicode_is_present() {
        assert_eq!(
            restorable_formats(&[CF_UNICODETEXT, CF_TEXT, CF_OEMTEXT, 16]),
            vec![CF_UNICODETEXT, 16]
        );
        assert_eq!(
            restorable_formats(&[CF_TEXT]),
            vec![CF_TEXT],
            "只有 ANSI 文本的老程序：那就是原件"
        );
    }

    #[test]
    fn restore_report_is_honest() {
        assert_eq!(restore_report(true, true), ClipboardRestore::Restored);
        assert!(restore_report(true, true).is_intact());
        assert_eq!(restore_report(false, true), ClipboardRestore::Incomplete);
        assert_eq!(restore_report(true, false), ClipboardRestore::Failed);
        assert_eq!(restore_report(false, false), ClipboardRestore::Failed);
        assert!(!ClipboardRestore::Incomplete.is_intact());
        assert!(ClipboardBackup::default().formats.is_empty());
        assert!(!ClipboardBackup::default().is_complete());
    }

    #[test]
    fn unicode_text_round_trips_and_stops_at_nul() {
        let bytes = unicode_text_bytes("你好 😀");
        assert_eq!(bytes.len() % 2, 0);
        assert_eq!(&bytes[bytes.len() - 2..], &[0, 0], "以 0 码元结尾");
        assert_eq!(text_from_unicode_bytes(&bytes), "你好 😀");
        let mut trailing = unicode_text_bytes("ab");
        trailing.extend(unicode_text_bytes("garbage"));
        assert_eq!(
            text_from_unicode_bytes(&trailing),
            "ab",
            "GlobalSize 可能比文本长"
        );
        assert_eq!(text_from_unicode_bytes(&[0x61]), "", "奇数尾字节丢弃");
    }
}
