//! 快捷键值语法、主键词表与冲突判定（**纯逻辑**，事实源 TDD-05 §1.2/§3、PRD-09-003）。
//!
//! 一个 `shortcuts.*` 设置的值有两种语法：普通组合 `cmd_or_ctrl+shift+v`，或双击组合
//! `double_tap:cmd_or_ctrl+c`。解析、归一、主键词表、保留键与冲突判定只在这里一份：
//! `set_setting` 校验、平台注册（热键插件 / 事件 tap 手势表）、设置 UI 三处共用同一口径。
use crate::registry::{SettingControl, SETTINGS};
use crate::settings::Settings;

/// 双击型快捷键值前缀（**唯一事实源**；平台层手势解析与设置 UI 同用此语法）。
pub const DOUBLE_TAP_PREFIX: &str = "double_tap:";

/// 快捷键**主键词表**（唯一事实源）：录制器只会产出这些名字，校验只接受这些名字，
/// 两条注册路径（热键插件 / 双击手势）对每一个名字都必须注册得上——由各自的用例钉住。
///
/// 词表之外的名字（死键、输入法处理中的键、异形键盘的私有键、F20 以上的功能键）在写入
/// 时即被拒绝：那种值过得了"修饰键 + 一个主键"的语法校验，却在注册时悄悄失败，用户
/// 看到的是一行写着快捷键、按下去毫无反应的设置。
pub const MAIN_KEYS: &[&str] = &[
    "a",
    "b",
    "c",
    "d",
    "e",
    "f",
    "g",
    "h",
    "i",
    "j",
    "k",
    "l",
    "m",
    "n",
    "o",
    "p",
    "q",
    "r",
    "s",
    "t",
    "u",
    "v",
    "w",
    "x",
    "y",
    "z", //
    "0",
    "1",
    "2",
    "3",
    "4",
    "5",
    "6",
    "7",
    "8",
    "9", //
    "f1",
    "f2",
    "f3",
    "f4",
    "f5",
    "f6",
    "f7",
    "f8",
    "f9",
    "f10",
    "f11",
    "f12",
    "f13",
    "f14",
    "f15",
    "f16",
    "f17",
    "f18",
    "f19",
    "f20", //
    "space",
    "enter",
    "tab",
    "escape",
    "backspace",
    "delete",
    "up",
    "down",
    "left",
    "right",
    "home",
    "end",
    "pageup",
    "pagedown",
    "insert",
    "minus",
    "equal",
    "bracketleft",
    "bracketright",
    "backslash",
    "semicolon",
    "quote",
    "comma",
    "period",
    "slash",
    "backquote",
];

/// 主键名是否在词表内。
#[must_use]
pub fn is_main_key(name: &str) -> bool {
    MAIN_KEYS.contains(&name)
}

/// 解析失败的原因（校验层据此给出不同的拒绝码）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// 空组合 / 多个主键 / 没有修饰键的裸键。
    Syntax,
    /// 主键不在 [`MAIN_KEYS`] 词表内。
    UnknownKey,
}

/// 一个归一化的键组合（修饰键 + 单个主键）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Combo {
    /// Command / Ctrl（跨平台 `cmd_or_ctrl` 语义）。
    pub cmd_or_ctrl: bool,
    /// Shift。
    pub shift: bool,
    /// Alt / Option。
    pub alt: bool,
    /// 主键（词表内的小写名，如 `"v"` / `"space"` / `"f5"`）。
    pub key: String,
}

impl Combo {
    /// 归一化文本（比较与展示的稳定形式，如 `cmd_or_ctrl+shift+v`）。
    #[must_use]
    pub fn canonical(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        if self.cmd_or_ctrl {
            parts.push("cmd_or_ctrl");
        }
        if self.shift {
            parts.push("shift");
        }
        if self.alt {
            parts.push("alt");
        }
        parts.push(&self.key);
        parts.join("+")
    }

    /// 是否为**保留编辑组合**（`Cmd/Ctrl + C/V/X/A/Z`，无其它修饰键）。
    ///
    /// 作为普通全局热键注册会劫持系统复制/粘贴等基本编辑，故仅允许双击模式
    /// （双击经事件 tap 观测、不拦截原事件）。
    #[must_use]
    pub fn is_reserved_editing(&self) -> bool {
        self.cmd_or_ctrl
            && !self.shift
            && !self.alt
            && matches!(self.key.as_str(), "c" | "v" | "x" | "a" | "z")
    }

    /// 是否被**编辑器**固定占用（[`EDITOR_RESERVED_COMBOS`]）。
    #[must_use]
    pub fn is_editor_reserved(&self) -> bool {
        EDITOR_RESERVED_COMBOS.contains(&self.canonical().as_str())
    }

    /// 是否落在**系统保留键**静态表（PRD-09-003 ②：可检测者诚实警告，不假装能枚举全部）。
    #[must_use]
    pub fn is_reserved_system(&self) -> bool {
        RESERVED_SYSTEM_COMBOS.contains(&self.canonical().as_str())
    }
}

/// 编辑器里固定占用的组合：应用内快捷键不能再绑它们（W3）——工作区快捷键由窗口级监听判定、
/// 编辑器的键由 CodeMirror 自己处理，绑同一个组合时一次按键两边都响。
///
/// 前半与前端 `EDITOR_SHORTCUTS`（`features/command/command-registry.ts`）逐项对齐，前端有一条
/// 读本文件的守护用例；后半是 CodeMirror 自带的查找、撤销重做与缩进。编辑器里的整篇动作
/// （`⌘E` 阅读切换、`⌘J` AI 操作）是可改键的设置（W3b），不在此表——它们之间的撞键由
/// [`conflicting_key`] 管。
pub const EDITOR_RESERVED_COMBOS: &[&str] = &[
    "cmd_or_ctrl+s",
    "cmd_or_ctrl+b",
    "cmd_or_ctrl+i",
    "cmd_or_ctrl+k",
    "cmd_or_ctrl+alt+t",
    "cmd_or_ctrl+0",
    "cmd_or_ctrl+1",
    "cmd_or_ctrl+2",
    "cmd_or_ctrl+3",
    "cmd_or_ctrl+4",
    "cmd_or_ctrl+5",
    "cmd_or_ctrl+6",
    "cmd_or_ctrl+shift+m",
    // CodeMirror 自带。
    "cmd_or_ctrl+f",
    "cmd_or_ctrl+g",
    "cmd_or_ctrl+shift+g",
    "cmd_or_ctrl+shift+z",
    "cmd_or_ctrl+y",
    "cmd_or_ctrl+bracketleft",
    "cmd_or_ctrl+bracketright",
];

/// 该值是否落在编辑器固定占用的组合上（便捷封装；非法语法返回 `false`）。
#[must_use]
pub fn is_editor_reserved_combo(value: &str) -> bool {
    parse_binding(value).is_ok_and(|b| b.combo.is_editor_reserved())
}

/// macOS/Windows 上被系统或窗口管理长期占用的组合（静态表，只列高置信项）。
///
/// 操作系统不提供「枚举他应用全局热键」的接口，第三方占用只能在注册时探测；此表覆盖的是
/// **必然**冲突的系统级键，绑上去等于让用户失去退出/切换窗口等基本操作。
const RESERVED_SYSTEM_COMBOS: &[&str] = &[
    "cmd_or_ctrl+q",
    "cmd_or_ctrl+w",
    "cmd_or_ctrl+h",
    "cmd_or_ctrl+m",
    "cmd_or_ctrl+tab",
    "cmd_or_ctrl+space",
    "cmd_or_ctrl+shift+3",
    "cmd_or_ctrl+shift+4",
    "cmd_or_ctrl+shift+5",
    "cmd_or_ctrl+alt+escape",
];

/// 一个快捷键设置值的解析结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    /// 触发模式：`true` = 双击组合（`double_tap:` 语法）。
    pub double_tap: bool,
    /// 归一化组合。
    pub combo: Combo,
}

/// 解析快捷键值。空值（未绑定）也是 [`ParseError::Syntax`]——调用方在此之前区分空/非空。
///
/// # Errors
/// 语法非法或主键不在词表内。
pub fn parse_binding(value: &str) -> Result<Binding, ParseError> {
    let trimmed = value.trim();
    let (double_tap, combo_src) = match trimmed.strip_prefix(DOUBLE_TAP_PREFIX) {
        Some(rest) => (true, rest),
        None => (false, trimmed),
    };
    parse_combo(combo_src).map(|combo| Binding { double_tap, combo })
}

/// 解析一个纯组合（不含 `double_tap:` 前缀）。
///
/// # Errors
/// 语法非法或主键不在词表内。
pub fn parse_combo(spec: &str) -> Result<Combo, ParseError> {
    let mut combo = Combo {
        cmd_or_ctrl: false,
        shift: false,
        alt: false,
        key: String::new(),
    };
    let mut key: Option<String> = None;
    for raw in spec.split('+') {
        let token = raw.trim().to_ascii_lowercase();
        match token.as_str() {
            "" => return Err(ParseError::Syntax),
            "cmd_or_ctrl" | "cmdorctrl" | "cmd" | "command" | "super" | "meta" | "ctrl"
            | "control" => combo.cmd_or_ctrl = true,
            "shift" => combo.shift = true,
            "alt" | "option" | "opt" => combo.alt = true,
            _ => {
                if key.is_some() {
                    return Err(ParseError::Syntax);
                }
                key = Some(token);
            }
        }
    }
    combo.key = key.ok_or(ParseError::Syntax)?;
    // 无修饰键的裸键（如单个 `v`）作全局热键会吞掉正常输入，一律不接受。
    if !combo.cmd_or_ctrl && !combo.shift && !combo.alt {
        return Err(ParseError::Syntax);
    }
    if !is_main_key(&combo.key) {
        return Err(ParseError::UnknownKey);
    }
    Ok(combo)
}

/// 该值是否为保留编辑组合（便捷封装；非法语法返回 `false`）。
#[must_use]
pub fn is_reserved_editing_combo(value: &str) -> bool {
    parse_binding(value).is_ok_and(|b| b.combo.is_reserved_editing())
}

/// 所有快捷键类设置的 key（控件契约声明为 [`SettingControl::Shortcut`] 者）。
pub fn shortcut_keys() -> impl Iterator<Item = &'static str> {
    SETTINGS
        .iter()
        .filter(|s| matches!(s.control, SettingControl::Shortcut { .. }))
        .map(|s| s.key)
}

/// 把某个快捷键设为 `value` 时，是否与**另一项**已有绑定撞键（PRD-09-003 ①）。
///
/// 返回被占用的那个设置 key。比较按归一化组合进行，**跨触发模式**——普通组合与
/// 双击组合落在同一物理键上时，按一次触发前者、按两次两者都会响，同样是冲突。
#[must_use]
pub fn conflicting_key(settings: &Settings, key: &str, value: &str) -> Option<&'static str> {
    let combo = parse_binding(value).ok()?.combo.canonical();
    shortcut_keys().filter(|other| *other != key).find(|other| {
        settings
            .get_str(other)
            .and_then(|v| parse_binding(&v).ok())
            .is_some_and(|b| b.combo.canonical() == combo)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_both_syntaxes_and_normalizes() {
        let plain = parse_binding("Cmd+Shift+V").unwrap();
        assert!(!plain.double_tap);
        assert_eq!(plain.combo.canonical(), "cmd_or_ctrl+shift+v");

        let dt = parse_binding("double_tap:cmd_or_ctrl+c").unwrap();
        assert!(dt.double_tap);
        assert_eq!(dt.combo.canonical(), "cmd_or_ctrl+c");
    }

    #[test]
    fn rejects_bare_key_and_multi_key_and_empty() {
        assert_eq!(
            parse_binding("v"),
            Err(ParseError::Syntax),
            "裸键会吞掉正常输入"
        );
        assert_eq!(
            parse_binding("cmd+a+b"),
            Err(ParseError::Syntax),
            "主键只允许一个"
        );
        assert_eq!(parse_binding(""), Err(ParseError::Syntax));
        assert_eq!(parse_binding("cmd+"), Err(ParseError::Syntax));
    }

    /// 词表之外的主键在**写入时**就被拒，而不是到注册那一步静默失败。
    #[test]
    fn rejects_keys_outside_the_vocabulary() {
        assert_eq!(
            parse_binding("cmd_or_ctrl+dead"),
            Err(ParseError::UnknownKey)
        );
        assert_eq!(
            parse_binding("cmd_or_ctrl+f24"),
            Err(ParseError::UnknownKey)
        );
        assert_eq!(
            parse_binding("double_tap:cmd_or_ctrl+shift+ "),
            Err(ParseError::Syntax)
        );
        assert!(parse_binding("cmd_or_ctrl+shift+space").is_ok());
        assert!(parse_binding("double_tap:cmd_or_ctrl+f20").is_ok());
    }

    #[test]
    fn vocabulary_is_lowercase_and_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for name in MAIN_KEYS {
            assert_eq!(*name, name.to_ascii_lowercase(), "{name} 应为小写");
            assert!(seen.insert(*name), "{name} 在词表里重复");
        }
    }

    #[test]
    fn reserved_editing_only_for_bare_cmd_edit_keys() {
        assert!(is_reserved_editing_combo("cmd_or_ctrl+c"));
        assert!(is_reserved_editing_combo("double_tap:cmd_or_ctrl+v"));
        assert!(!is_reserved_editing_combo("cmd_or_ctrl+shift+c"));
        assert!(!is_reserved_editing_combo("cmd_or_ctrl+k"));
    }

    #[test]
    fn reserved_system_table_hits_canonical_form() {
        assert!(parse_combo("Cmd+Q").unwrap().is_reserved_system());
        assert!(parse_combo("cmd_or_ctrl+space")
            .unwrap()
            .is_reserved_system());
        assert!(!parse_combo("cmd_or_ctrl+shift+r")
            .unwrap()
            .is_reserved_system());
    }

    #[test]
    fn conflict_detects_other_binding_across_trigger_modes() {
        let mut settings = Settings::with_defaults();
        settings.set("shortcuts.voice_input", "cmd_or_ctrl+shift+v");
        // 同组合、不同触发模式 → 仍视为冲突（按一次/两次都会响）。
        assert_eq!(
            conflicting_key(&settings, "shortcuts.quick_chat", "double_tap:cmd+shift+v"),
            Some("shortcuts.voice_input")
        );
        // 改自己不算冲突。
        assert_eq!(
            conflicting_key(&settings, "shortcuts.voice_input", "cmd_or_ctrl+shift+v"),
            None
        );
        assert_eq!(
            conflicting_key(&settings, "shortcuts.quick_chat", "cmd_or_ctrl+shift+8"),
            None
        );
    }

    /// **出厂键位不得自相冲突**：默认值撞键会让用户开箱即撞上「与 X 冲突」，
    /// 且其中一个绑定必然静默失效。注册表加新快捷键时由本测试兜住。
    #[test]
    fn default_shortcut_bindings_do_not_collide() {
        let defaults = Settings::with_defaults();
        for key in shortcut_keys() {
            let Some(value) = defaults.get_str(key) else {
                continue;
            };
            if value.trim().is_empty() {
                continue;
            }
            assert_eq!(
                conflicting_key(&defaults, key, &value),
                None,
                "{key} 的出厂键位与另一项冲突"
            );
        }
    }

    /// 出厂键位必须都是合法语法（否则启动即注册失败，且用户看不出哪儿错了）。
    #[test]
    fn default_shortcut_bindings_parse() {
        let defaults = Settings::with_defaults();
        for key in shortcut_keys() {
            let value = defaults.get_str(key).unwrap_or_default();
            if value.trim().is_empty() {
                continue;
            }
            let binding =
                parse_binding(&value).unwrap_or_else(|e| panic!("{key} 出厂键位无法解析：{e:?}"));
            assert!(
                !binding.combo.is_reserved_editing() || binding.double_tap,
                "{key} 的出厂键位是保留编辑组合，必须为双击模式"
            );
        }
    }

    #[test]
    fn shortcut_keys_cover_registry_shortcut_controls() {
        let keys: Vec<&str> = shortcut_keys().collect();
        assert!(keys.contains(&"shortcuts.voice_input"));
        assert!(
            !keys.contains(&"shortcuts.double_tap_window_ms"),
            "时间窗是数值项，不是键绑定"
        );
    }
}
