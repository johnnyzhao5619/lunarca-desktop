//! 用户自定义 AI 指令预设（PRD-06-024）：随库存于 `.luna/ai_presets.json`。
//!
//! 与蒸馏模板（`distill_templates`）同一存放形态与同一读写咽喉（`meta`）：预设是「这个库
//! 里的人常用的改写指令」，跟内容走，不是设备偏好。只存名字与指令，绝不含正文。

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::error::Result;

/// 一条预设：稳定 id（Rust 生成）+ 用户起的名字 + 指令原文。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiPreset {
    pub id: String,
    pub name: String,
    pub instruction: String,
}

const META_NAME: &str = "ai_presets";

/// 预设名的长度上限（与浮动菜单里能显示下的一致）。
const MAX_PACK_PRESET_NAME_CHARS: usize = 40;
/// 指令的长度上限：预设是一句改写指令，不是一篇提示词。
const MAX_PACK_PRESET_INSTRUCTION_CHARS: usize = 2000;

/// 解析模板包里的一条 AI 预设（ADR-169 ⑤，TOML：`name` 与 `instruction`）。`id` 由调用方给
/// （包内文件名）；去首尾空白后为空或超长即拒。
///
/// # Errors
/// 不是合规的 TOML、多了未知字段，或名字 / 指令为空、超长时，返回一句英文原因。
pub fn parse_pack_preset(id: &str, source: &str) -> std::result::Result<AiPreset, String> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Raw {
        name: String,
        instruction: String,
    }
    let raw: Raw = toml::from_str(source).map_err(|error| error.to_string())?;
    let name = raw.name.trim().to_owned();
    let instruction = raw.instruction.trim().to_owned();
    if name.is_empty() || name.chars().count() > MAX_PACK_PRESET_NAME_CHARS {
        return Err("preset name must be 1-40 characters".to_owned());
    }
    if instruction.is_empty() || instruction.chars().count() > MAX_PACK_PRESET_INSTRUCTION_CHARS {
        return Err("preset instruction must be 1-2000 characters".to_owned());
    }
    Ok(AiPreset {
        id: id.to_owned(),
        name,
        instruction,
    })
}

/// 读取全部预设（文件不存在 = 空）。
pub fn read_ai_presets(root: &Path) -> Result<Vec<AiPreset>> {
    crate::meta::read_meta(root, META_NAME)
}

/// 整体写回（调用方已做增删，这里只落盘）。
pub fn write_ai_presets(root: &Path, presets: &[AiPreset]) -> Result<()> {
    crate::meta::write_meta(root, META_NAME, presets)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_presets_are_a_name_and_an_instruction_and_nothing_else() {
        let preset = parse_pack_preset(
            "tighten",
            "name = \" 收紧 \"\ninstruction = \"删掉一半的字\"\n",
        )
        .expect("valid");
        assert_eq!(preset.id, "tighten");
        assert_eq!(preset.name, "收紧");
        assert!(parse_pack_preset("x", "name = \"a\"\ninstruction = \" \"\n").is_err());
        assert!(
            parse_pack_preset("x", "name = \"a\"\ninstruction = \"b\"\nmodel = \"c\"\n").is_err()
        );
    }

    #[test]
    fn presets_round_trip_and_start_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_ai_presets(dir.path()).unwrap().is_empty());
        let presets = vec![AiPreset {
            id: "p1".into(),
            name: "改成邮件".into(),
            instruction: "改写成正式邮件".into(),
        }];
        write_ai_presets(dir.path(), &presets).unwrap();
        assert_eq!(read_ai_presets(dir.path()).unwrap(), presets);
    }
}
