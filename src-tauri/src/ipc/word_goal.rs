//! 字数目标（笔记库对标轮 W6b）。存在 `luna.word_goal`（`Vault::set_word_goal`）；这里解析用户
//! 填的数、写盘、让打开着的编辑器跟上。字数本身是编辑器状态栏那一个数，目标只和它比。

use super::*;

/// 目标上限：再大就不是一篇笔记的目标了（多半是多敲了几个零）。
const MAX_WORD_GOAL: u32 = 1_000_000;

/// 设目标的结果（不合法的输入不算错误，界面就地说「请填一个数」）。
#[derive(Debug, Clone, Copy, Serialize, Type, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WordGoalDto {
    Set { goal: u32 },
    Cleared,
    Invalid,
}

/// 用户填的数 → 目标：`Some(None)` = 清掉（填 0），`None` = 不合法。认全角数字与千分位逗号。
fn parse_word_goal(input: &str) -> Option<Option<u32>> {
    let digits: String = input
        .trim()
        .chars()
        .filter(|c| !matches!(c, ',' | '，' | ' ' | '\u{3000}'))
        .map(|c| match c {
            '０'..='９' => char::from_u32(c as u32 - '０' as u32 + '0' as u32).unwrap_or(c),
            _ => c,
        })
        .collect();
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    match digits.parse::<u32>().ok()? {
        0 => Some(None),
        goal if goal <= MAX_WORD_GOAL => Some(Some(goal)),
        _ => None,
    }
}

/// 设 / 清这一篇的字数目标。`input` 是用户填的原文，解析在这里（填 0 = 清掉）。
#[tauri::command]
#[specta::specta]
pub(crate) fn set_word_goal(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: tauri::State<'_, AppState>,
    path: String,
    input: String,
) -> Result<WordGoalDto, String> {
    let Some(goal) = parse_word_goal(&input) else {
        return Ok(WordGoalDto::Invalid);
    };
    let written = state.with_vault(window.label(), |vault| {
        vault.set_word_goal(&path, goal).map_err(vault_err)
    })?;
    if let Some(content) = written {
        // 同文件的编辑器实例据此增量跟进（`luna:` 块多 / 少一行），同「标记为记录」。
        broadcast_note_write(&app, window.label(), &path, content);
    }
    Ok(goal.map_or(WordGoalDto::Cleared, |goal| WordGoalDto::Set { goal }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goals_parse_from_what_people_type() {
        assert_eq!(parse_word_goal("2000"), Some(Some(2000)));
        assert_eq!(parse_word_goal(" 2,000 "), Some(Some(2000)));
        assert_eq!(parse_word_goal("２０００"), Some(Some(2000)));
        assert_eq!(parse_word_goal("0"), Some(None));
        for bad in ["", "两千", "2k", "-5", "1.5", "10000000"] {
            assert_eq!(parse_word_goal(bad), None, "{bad}");
        }
    }
}
