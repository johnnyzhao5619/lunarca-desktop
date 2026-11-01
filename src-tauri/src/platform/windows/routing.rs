//! 取选区、注入方式与前台核对的判定（ADR-163）。
//!
//! 全是纯函数：输入是系统调用读出来的事实（UI Automation 读数、前台窗口、窗口类名、输入法开关），
//! 输出是该走哪条路。系统调用在同目录的 `selection` / `injector` / `foreground` 薄层里，
//! 这里在任何平台上单测。

/// UI Automation 对焦点控件选区的读数（`TextPattern.GetSelection`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiaSelection {
    /// 读到选中的文字。
    Text(String),
    /// 控件支持 TextPattern，但此刻没有选中任何文字。
    Empty,
    /// 拿不到：没有焦点控件、控件不支持 TextPattern、调用出错或超时。
    Unavailable,
}

/// 主动取选区该走哪条路（[`selection_route`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectionRoute {
    /// UI Automation 已读到选区，不碰剪贴板。
    Uia(String),
    /// 回落：备份剪贴板 → 合成 Ctrl+C → 读取 → 还原。
    ClipboardCopy,
    /// 判定为没有选区（不碰剪贴板、不合成任何键）。
    NoSelection,
}

/// 合成 Ctrl+C 在这些窗口里是「中断正在运行的程序」而不是复制：控制台宿主、Windows Terminal、
/// Git Bash（mintty）、PuTTY、ConEmu。没读到选区时宁可当作没有选区。
const TERMINAL_WINDOW_CLASSES: &[&str] = &[
    "ConsoleWindowClass",
    "CASCADIA_HOSTING_WINDOW_CLASS",
    "mintty",
    "PuTTY",
    "VirtualConsoleClass",
];

/// 前台窗口类名是不是终端（[`TERMINAL_WINDOW_CLASSES`]）。
#[must_use]
pub fn is_terminal_window_class(class_name: &str) -> bool {
    TERMINAL_WINDOW_CLASSES.contains(&class_name)
}

/// 主动取选区的回落顺序（ADR-163 ②）：
///
/// 1. UI Automation 读到非空白文字 → 用它，零剪贴板接触；
/// 2. 控件明确说「没有选中」→ 就是没有选区，绝不为此去合成复制键；
/// 3. 读不到（控件不暴露 TextPattern）→ 回落剪贴板复制；但前台是终端、或根本没有前台窗口时，
///    合成 Ctrl+C 有副作用（中断进程）或没有对象，判为没有选区。
#[must_use]
pub fn selection_route(uia: UiaSelection, foreground_class: Option<&str>) -> SelectionRoute {
    match uia {
        UiaSelection::Text(text) if !text.trim().is_empty() => SelectionRoute::Uia(text),
        UiaSelection::Text(_) | UiaSelection::Empty => SelectionRoute::NoSelection,
        UiaSelection::Unavailable => match foreground_class {
            Some(class) if !is_terminal_window_class(class) => SelectionRoute::ClipboardCopy,
            _ => SelectionRoute::NoSelection,
        },
    }
}

/// 注入前的前台事实（[`decide_inject_route`] 的输入）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ForegroundFacts {
    /// 此刻的前台窗口（`GetForegroundWindow`，`None` = 没有）。
    pub foreground: Option<isize>,
    /// 前台窗口属于 Lunarca 自己（用户在 HUD / 弹窗的输入框里打过字，前台交给了我们）。
    pub foreground_is_own: bool,
    /// 目标窗口还在（`IsWindow`）。
    pub target_alive: bool,
    /// 目标窗口所属进程权限比我们高（管理员窗口）：系统会静默丢掉我们合成的按键。
    pub target_elevated: bool,
}

/// 为什么不注入、改落剪贴板。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InjectFallback {
    /// 会话开始时没有记下目标窗口。
    NoTarget,
    /// 目标窗口已经关了。
    TargetGone,
    /// 用户切到了别的窗口。
    TargetChanged,
    /// 目标是管理员窗口，合成按键送不进去。
    TargetElevated,
}

/// 注入前核对的结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InjectRoute {
    /// 目标仍在前台：注入。
    Inject,
    /// 前台是 Lunarca 自己的窗口：先把前台还给目标窗口，再核对一次。
    Reactivate,
    /// 不注入，改落剪贴板。
    Fallback(InjectFallback),
}

/// 注入前核对（ADR-163 ③）：目标窗口必须仍在前台，否则不打字——打进别的窗口比不打更糟。
///
/// 唯一的例外是前台落在 Lunarca 自己的窗口上：用户在非激活面板里点了输入框，面板为了收键盘
/// 而被激活过，目标窗口只是暂时让出了前台。此时（且只在 `may_reactivate` 时）先把前台还回去、
/// 由调用方带 `may_reactivate = false` 再核对一次，仍不对就回落。
#[must_use]
pub fn decide_inject_route(
    target: Option<isize>,
    facts: ForegroundFacts,
    may_reactivate: bool,
) -> InjectRoute {
    let Some(target) = target else {
        return InjectRoute::Fallback(InjectFallback::NoTarget);
    };
    if !facts.target_alive {
        return InjectRoute::Fallback(InjectFallback::TargetGone);
    }
    if facts.target_elevated {
        return InjectRoute::Fallback(InjectFallback::TargetElevated);
    }
    if facts.foreground == Some(target) {
        return InjectRoute::Inject;
    }
    if facts.foreground_is_own && may_reactivate {
        return InjectRoute::Reactivate;
    }
    InjectRoute::Fallback(InjectFallback::TargetChanged)
}

/// 注入方式（[`choose_inject_method`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InjectMethod {
    /// `SendInput` + `KEYEVENTF_UNICODE` 逐字打进去：不碰剪贴板。
    UnicodeTyping,
    /// 备份剪贴板 → 写入 → 合成 Ctrl+V → 还原。
    ClipboardPaste,
}

/// 逐字打字的上限（UTF-16 码元）。再长就改走粘贴：逐字打的每个码元都是两次按键事件，
/// 几百字时目标应用要消化好一阵，期间用户一动键盘就会插进中间。
pub const UNICODE_TYPING_MAX_UNITS: usize = 200;

/// 注入方式选择（ADR-163 ③、TDD-05 §2.2）：
///
/// - 输入法开着（中文输入状态）→ 粘贴。逐字打进去的字符可能落进正在组的拼音串；Ctrl+V 会让
///   绝大多数输入法先上屏当前候选再粘贴。跨进程只读得到「开着」，读不到「正在组字」，
///   所以按开着算。
/// - 含控制字符（换行、Tab……）→ 粘贴。逐字打的换行就是回车键：在地址栏、聊天框里会直接提交。
/// - 超过 [`UNICODE_TYPING_MAX_UNITS`] → 粘贴。
/// - 其余逐字打。
#[must_use]
pub fn choose_inject_method(text: &str, ime_open: bool) -> InjectMethod {
    if ime_open
        || text.chars().any(char::is_control)
        || text.encode_utf16().count() > UNICODE_TYPING_MAX_UNITS
    {
        InjectMethod::ClipboardPaste
    } else {
        InjectMethod::UnicodeTyping
    }
}

/// 「替换原文」前对源选区快照的核对结论（[`check_selection_snapshot`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotCheck {
    /// 源控件里选中的仍是触发时那段文字：可以替换。
    Matches,
    /// 选区变了或没了：打进去就不是替换而是插入，改为复制。
    Changed,
    /// 控件不暴露选区（UI Automation 读不到）：无从核对，照常替换。
    Unverifiable,
}

/// 换行统一成 `\n` 再比较：快照可能来自剪贴板（`\r\n`），UI Automation 给的可能是 `\r`。
fn normalize_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// 「替换原文」前核对源选区（ADR-163 ③）。
#[must_use]
pub fn check_selection_snapshot(current: &UiaSelection, snapshot_text: &str) -> SnapshotCheck {
    match current {
        UiaSelection::Text(text)
            if normalize_newlines(text).trim() == normalize_newlines(snapshot_text).trim() =>
        {
            SnapshotCheck::Matches
        }
        UiaSelection::Text(_) | UiaSelection::Empty => SnapshotCheck::Changed,
        UiaSelection::Unavailable => SnapshotCheck::Unverifiable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uia_text_wins_and_never_touches_the_clipboard() {
        assert_eq!(
            selection_route(UiaSelection::Text("hello".into()), Some("Notepad")),
            SelectionRoute::Uia("hello".into())
        );
    }

    #[test]
    fn an_explicitly_empty_selection_does_not_synthesize_a_copy() {
        assert_eq!(
            selection_route(UiaSelection::Empty, Some("Notepad")),
            SelectionRoute::NoSelection
        );
        assert_eq!(
            selection_route(UiaSelection::Text("  \n".into()), Some("Notepad")),
            SelectionRoute::NoSelection,
            "只选中了空白等于没有选区"
        );
    }

    #[test]
    fn unreadable_selection_falls_back_to_a_protected_copy_except_in_terminals() {
        assert_eq!(
            selection_route(UiaSelection::Unavailable, Some("WeChatMainWndForPC")),
            SelectionRoute::ClipboardCopy
        );
        for terminal in [
            "ConsoleWindowClass",
            "CASCADIA_HOSTING_WINDOW_CLASS",
            "mintty",
        ] {
            assert_eq!(
                selection_route(UiaSelection::Unavailable, Some(terminal)),
                SelectionRoute::NoSelection,
                "{terminal} 里合成 Ctrl+C 会中断进程"
            );
        }
        assert_eq!(
            selection_route(UiaSelection::Unavailable, None),
            SelectionRoute::NoSelection,
            "没有前台窗口就没有复制对象"
        );
    }

    const TARGET: isize = 0x1234;
    const OTHER: isize = 0x5678;
    const OWN: isize = 0x9abc;

    fn facts(foreground: isize, own: bool) -> ForegroundFacts {
        ForegroundFacts {
            foreground: Some(foreground),
            foreground_is_own: own,
            target_alive: true,
            target_elevated: false,
        }
    }

    #[test]
    fn injects_only_while_the_target_is_still_in_front() {
        assert_eq!(
            decide_inject_route(Some(TARGET), facts(TARGET, false), true),
            InjectRoute::Inject
        );
        assert_eq!(
            decide_inject_route(Some(TARGET), facts(OTHER, false), true),
            InjectRoute::Fallback(InjectFallback::TargetChanged),
            "用户切走了：不能打进别的窗口"
        );
    }

    #[test]
    fn own_window_in_front_hands_focus_back_once() {
        assert_eq!(
            decide_inject_route(Some(TARGET), facts(OWN, true), true),
            InjectRoute::Reactivate
        );
        assert_eq!(
            decide_inject_route(Some(TARGET), facts(OWN, true), false),
            InjectRoute::Fallback(InjectFallback::TargetChanged),
            "还回去之后前台仍不是目标：回落，不再试"
        );
    }

    #[test]
    fn missing_closed_or_elevated_targets_fall_back() {
        assert_eq!(
            decide_inject_route(None, facts(TARGET, false), true),
            InjectRoute::Fallback(InjectFallback::NoTarget)
        );
        let gone = ForegroundFacts {
            target_alive: false,
            ..facts(TARGET, false)
        };
        assert_eq!(
            decide_inject_route(Some(TARGET), gone, true),
            InjectRoute::Fallback(InjectFallback::TargetGone)
        );
        let elevated = ForegroundFacts {
            target_elevated: true,
            ..facts(TARGET, false)
        };
        assert_eq!(
            decide_inject_route(Some(TARGET), elevated, true),
            InjectRoute::Fallback(InjectFallback::TargetElevated),
            "管理员窗口会静默丢掉合成按键，打了等于没打"
        );
    }

    #[test]
    fn short_plain_text_is_typed_everything_else_is_pasted() {
        assert_eq!(
            choose_inject_method("你好，world", false),
            InjectMethod::UnicodeTyping
        );
        assert_eq!(
            choose_inject_method("你好", true),
            InjectMethod::ClipboardPaste,
            "输入法开着：粘贴会先上屏候选"
        );
        assert_eq!(
            choose_inject_method("第一行\n第二行", false),
            InjectMethod::ClipboardPaste,
            "逐字打的换行是回车键"
        );
        assert_eq!(
            choose_inject_method("a\tb", false),
            InjectMethod::ClipboardPaste
        );
        let long = "字".repeat(UNICODE_TYPING_MAX_UNITS + 1);
        assert_eq!(
            choose_inject_method(&long, false),
            InjectMethod::ClipboardPaste
        );
        let edge = "字".repeat(UNICODE_TYPING_MAX_UNITS);
        assert_eq!(
            choose_inject_method(&edge, false),
            InjectMethod::UnicodeTyping
        );
        // 表情是两个 UTF-16 码元：按码元计。
        let emoji = "😀".repeat(UNICODE_TYPING_MAX_UNITS / 2 + 1);
        assert_eq!(
            choose_inject_method(&emoji, false),
            InjectMethod::ClipboardPaste
        );
    }

    #[test]
    fn snapshot_check_tolerates_line_endings_but_not_a_different_selection() {
        assert_eq!(
            check_selection_snapshot(&UiaSelection::Text("a\rb".into()), "a\r\nb"),
            SnapshotCheck::Matches
        );
        assert_eq!(
            check_selection_snapshot(&UiaSelection::Text("other".into()), "a"),
            SnapshotCheck::Changed
        );
        assert_eq!(
            check_selection_snapshot(&UiaSelection::Empty, "a"),
            SnapshotCheck::Changed,
            "选区没了：打进去是插入不是替换"
        );
        assert_eq!(
            check_selection_snapshot(&UiaSelection::Unavailable, "a"),
            SnapshotCheck::Unverifiable
        );
    }
}
