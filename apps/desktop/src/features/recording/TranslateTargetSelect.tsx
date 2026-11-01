/**
 * 转译目标外语选择器（录音 HUD / 实时字幕面板共用，PRD-04-041）。
 *
 * 选项取自设置规格（Rust 注册表单一事实源）+ 标签走 `common:lang.*`，杜绝各面板硬编码。
 * 智能双向语义：所选为「与母语互译的外语」——源为母语则译成它，源为它则译回母语。
 */

import { useTranslation } from "react-i18next";
import { SelectBox } from "../../components/ui/SelectBox";
import {
  langLabelKey,
  TRANSLATE_TARGET_SETTING,
  useSettingOptions,
} from "../settings/language-options";

interface TranslateTargetSelectProps {
  value: string;
  onChange: (lang: string) => void;
  disabled?: boolean;
  ariaLabel: string;
}

export function TranslateTargetSelect({
  value,
  onChange,
  disabled,
  ariaLabel,
}: TranslateTargetSelectProps) {
  const { t } = useTranslation("common");
  const options = useSettingOptions(TRANSLATE_TARGET_SETTING);
  return (
    <SelectBox
      className="luna-subtitle-target"
      value={value}
      disabled={disabled}
      onChange={(event) => onChange(event.currentTarget.value)}
      aria-label={ariaLabel}
    >
      {options.map((lang) => (
        <option key={lang} value={lang}>
          {t(langLabelKey(lang).slice("common:".length))}
        </option>
      ))}
    </SelectBox>
  );
}
