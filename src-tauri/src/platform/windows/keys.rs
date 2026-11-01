//! 键位与合成按键序列（ADR-163）。纯数据，任何平台上单测；真正的 `SendInput` 在 `input` 薄层。
//!
//! - [`windows_vk_for_key`]：设置主键词表（`luna_config::shortcut::MAIN_KEYS`）→ Windows 虚拟键码，
//!   供双击手势匹配低层键盘钩子报来的键。
//! - [`chord_strokes`] / [`unicode_strokes`]：合成 Ctrl+C / Ctrl+V 与逐字输入的按键序列。用户手上
//!   还按着的修饰键会先被松开——`SendInput` 与物理键盘共用同一份键盘状态，不松开的话
//!   「Ctrl+Shift+T 唤起后合成 Ctrl+C」实际送出去的是 Ctrl+Shift+C，逐字输入的字母也会变成快捷键。

/// 一次合成按键。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyStroke {
    /// 虚拟键按下 / 松开。
    Vk { vk: u16, down: bool },
    /// `KEYEVENTF_UNICODE`：一个 UTF-16 码元的按下 / 松开。
    Unicode { unit: u16, down: bool },
}

pub const VK_BACK: u16 = 0x08;
pub const VK_TAB: u16 = 0x09;
pub const VK_RETURN: u16 = 0x0D;
pub const VK_CONTROL: u16 = 0x11;
pub const VK_ESCAPE: u16 = 0x1B;
pub const VK_C: u16 = 0x43;
pub const VK_V: u16 = 0x56;
pub const VK_LWIN: u16 = 0x5B;
pub const VK_RWIN: u16 = 0x5C;
pub const VK_LSHIFT: u16 = 0xA0;
pub const VK_RSHIFT: u16 = 0xA1;
pub const VK_LCONTROL: u16 = 0xA2;
pub const VK_RCONTROL: u16 = 0xA3;
pub const VK_LMENU: u16 = 0xA4;
pub const VK_RMENU: u16 = 0xA5;
/// 未分配的虚拟键：松开 Alt / Win 之前先点一下它，系统就不会把「单按一下 Alt」当成激活菜单栏、
/// 「单按一下 Win」当成打开开始菜单（AutoHotkey 的 menu mask 同一做法）。
pub const VK_MASK_KEY: u16 = 0xE8;

/// 左右区分的修饰键（低层钩子与 `GetAsyncKeyState` 报的都是左右键码）。
pub const MODIFIER_VKS: [u16; 8] = [
    VK_LCONTROL,
    VK_RCONTROL,
    VK_LSHIFT,
    VK_RSHIFT,
    VK_LMENU,
    VK_RMENU,
    VK_LWIN,
    VK_RWIN,
];

/// 是不是修饰键（含不分左右的 `VK_SHIFT` / `VK_CONTROL` / `VK_MENU`）。
#[must_use]
pub fn is_modifier_vk(vk: u16) -> bool {
    MODIFIER_VKS.contains(&vk) || matches!(vk, 0x10..=0x12)
}

/// 合成时要带 `KEYEVENTF_EXTENDEDKEY` 的键（右侧 Ctrl / Alt 与 Win 键）；否则右键松开会被当成左键。
#[must_use]
pub fn is_extended_vk(vk: u16) -> bool {
    matches!(vk, VK_RCONTROL | VK_RMENU | VK_LWIN | VK_RWIN)
}

/// 主键词表名 → Windows 虚拟键码（US 物理位）。覆盖 [`luna_config::shortcut::MAIN_KEYS`] 全集。
#[must_use]
pub fn windows_vk_for_key(token: &str) -> Option<u16> {
    let bytes = token.as_bytes();
    if let [c] = bytes {
        if c.is_ascii_lowercase() {
            return Some(u16::from(c.to_ascii_uppercase()));
        }
        if c.is_ascii_digit() {
            return Some(u16::from(*c));
        }
    }
    if let Some(n) = token.strip_prefix('f').and_then(|n| n.parse::<u16>().ok()) {
        if (1..=24).contains(&n) {
            return Some(0x70 + n - 1);
        }
    }
    let vk = match token {
        "space" => 0x20,
        "enter" => VK_RETURN,
        "tab" => VK_TAB,
        "escape" => VK_ESCAPE,
        "backspace" => VK_BACK,
        "delete" => 0x2E,
        "insert" => 0x2D,
        "home" => 0x24,
        "end" => 0x23,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        "left" => 0x25,
        "up" => 0x26,
        "right" => 0x27,
        "down" => 0x28,
        "semicolon" => 0xBA,
        "equal" => 0xBB,
        "comma" => 0xBC,
        "minus" => 0xBD,
        "period" => 0xBE,
        "slash" => 0xBF,
        "backquote" => 0xC0,
        "bracketleft" => 0xDB,
        "backslash" => 0xDC,
        "bracketright" => 0xDD,
        "quote" => 0xDE,
        _ => return None,
    };
    Some(vk)
}

/// 先松开用户还按着的修饰键。按着 Alt / Win 时先点一下 [`VK_MASK_KEY`]，免得松开那一下
/// 激活菜单栏或开始菜单。松开后**不再按回去**：用户在合成期间松手的话，按回去的键会卡住。
fn release_held(held: &[u16], out: &mut Vec<KeyStroke>) {
    let held: Vec<u16> = held
        .iter()
        .copied()
        .filter(|vk| MODIFIER_VKS.contains(vk))
        .collect();
    if held
        .iter()
        .any(|vk| matches!(*vk, VK_LMENU | VK_RMENU | VK_LWIN | VK_RWIN))
    {
        out.push(KeyStroke::Vk {
            vk: VK_MASK_KEY,
            down: true,
        });
        out.push(KeyStroke::Vk {
            vk: VK_MASK_KEY,
            down: false,
        });
    }
    out.extend(held.into_iter().map(|vk| KeyStroke::Vk { vk, down: false }));
}

/// 合成 `Ctrl+<key>`（复制 / 粘贴）的完整序列：松开手上的修饰键 → Ctrl 按下 → 键按下松开 → Ctrl 松开。
#[must_use]
pub fn chord_strokes(held: &[u16], key: u16) -> Vec<KeyStroke> {
    let mut out = Vec::new();
    release_held(held, &mut out);
    out.extend([
        KeyStroke::Vk {
            vk: VK_CONTROL,
            down: true,
        },
        KeyStroke::Vk {
            vk: key,
            down: true,
        },
        KeyStroke::Vk {
            vk: key,
            down: false,
        },
        KeyStroke::Vk {
            vk: VK_CONTROL,
            down: false,
        },
    ]);
    out
}

/// 逐字输入的完整序列：松开手上的修饰键 → 每个 UTF-16 码元一按一松。
/// 调用方保证文本不含控制字符（换行等走粘贴，见 `routing::choose_inject_method`）。
#[must_use]
pub fn unicode_strokes(held: &[u16], text: &str) -> Vec<KeyStroke> {
    let mut out = Vec::new();
    release_held(held, &mut out);
    for unit in text.encode_utf16() {
        out.push(KeyStroke::Unicode { unit, down: true });
        out.push(KeyStroke::Unicode { unit, down: false });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 词表里每个名字都有键码且互不重复：校验放行的双击值，Windows 上手势表一定建得出来。
    #[test]
    fn every_main_key_has_a_windows_vk() {
        let missing: Vec<&str> = luna_config::shortcut::MAIN_KEYS
            .iter()
            .copied()
            .filter(|name| windows_vk_for_key(name).is_none())
            .collect();
        assert_eq!(
            missing,
            Vec::<&str>::new(),
            "这些主键在 Windows 上无法作双击手势"
        );
        let mut codes: Vec<u16> = luna_config::shortcut::MAIN_KEYS
            .iter()
            .filter_map(|name| windows_vk_for_key(name))
            .collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(
            codes.len(),
            luna_config::shortcut::MAIN_KEYS.len(),
            "键码有重复"
        );
        assert!(
            codes.iter().all(|vk| !is_modifier_vk(*vk)),
            "主键不能是修饰键"
        );
    }

    #[test]
    fn known_vks() {
        assert_eq!(windows_vk_for_key("c"), Some(VK_C));
        assert_eq!(windows_vk_for_key("v"), Some(VK_V));
        assert_eq!(windows_vk_for_key("7"), Some(0x37));
        assert_eq!(windows_vk_for_key("f1"), Some(0x70));
        assert_eq!(windows_vk_for_key("f12"), Some(0x7B));
        assert_eq!(windows_vk_for_key("space"), Some(0x20));
        assert_eq!(windows_vk_for_key("cmd_or_ctrl"), None);
        assert_eq!(windows_vk_for_key("f0"), None);
        assert!(is_extended_vk(VK_RCONTROL) && is_extended_vk(VK_LWIN));
        assert!(!is_extended_vk(VK_LCONTROL) && !is_extended_vk(VK_C));
    }

    #[test]
    fn chord_with_nothing_held_is_plain_ctrl_key() {
        assert_eq!(
            chord_strokes(&[], VK_C),
            vec![
                KeyStroke::Vk {
                    vk: VK_CONTROL,
                    down: true
                },
                KeyStroke::Vk {
                    vk: VK_C,
                    down: true
                },
                KeyStroke::Vk {
                    vk: VK_C,
                    down: false
                },
                KeyStroke::Vk {
                    vk: VK_CONTROL,
                    down: false
                },
            ]
        );
    }

    #[test]
    fn held_shift_is_released_so_ctrl_c_is_not_ctrl_shift_c() {
        let strokes = chord_strokes(&[VK_LCONTROL, VK_LSHIFT], VK_C);
        assert_eq!(
            &strokes[..2],
            &[
                KeyStroke::Vk {
                    vk: VK_LCONTROL,
                    down: false
                },
                KeyStroke::Vk {
                    vk: VK_LSHIFT,
                    down: false
                },
            ]
        );
        assert!(
            !strokes.contains(&KeyStroke::Vk {
                vk: VK_LSHIFT,
                down: true
            }),
            "松开后不再按回去：用户中途松手会卡键"
        );
    }

    #[test]
    fn releasing_alt_or_win_is_masked_first() {
        let strokes = chord_strokes(&[VK_LMENU], VK_V);
        assert_eq!(
            &strokes[..3],
            &[
                KeyStroke::Vk {
                    vk: VK_MASK_KEY,
                    down: true
                },
                KeyStroke::Vk {
                    vk: VK_MASK_KEY,
                    down: false
                },
                KeyStroke::Vk {
                    vk: VK_LMENU,
                    down: false
                },
            ],
            "单独松开 Alt 会激活菜单栏"
        );
        // 只按着 Ctrl / Shift 不需要遮罩。
        assert!(!chord_strokes(&[VK_RSHIFT], VK_V).contains(&KeyStroke::Vk {
            vk: VK_MASK_KEY,
            down: true
        }));
    }

    #[test]
    fn unicode_strokes_are_one_press_per_utf16_unit() {
        let strokes = unicode_strokes(&[], "你😀");
        let units: Vec<u16> = "你😀".encode_utf16().collect();
        assert_eq!(units.len(), 3, "表情是代理对");
        assert_eq!(strokes.len(), 6);
        assert_eq!(
            strokes[0],
            KeyStroke::Unicode {
                unit: units[0],
                down: true
            }
        );
        assert_eq!(
            strokes[5],
            KeyStroke::Unicode {
                unit: units[2],
                down: false
            }
        );
        // 手上按着 Ctrl 时先松开，否则字母会变成快捷键。
        assert_eq!(
            unicode_strokes(&[VK_RCONTROL], "a")[0],
            KeyStroke::Vk {
                vk: VK_RCONTROL,
                down: false
            }
        );
    }
}
