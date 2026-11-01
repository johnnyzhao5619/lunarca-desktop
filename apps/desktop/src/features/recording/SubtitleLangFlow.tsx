/**
 * 字幕语言流条（录音 HUD / 实时字幕面板共用，PRD-04-046，Chrome Live Caption 对标）：
 * `[源语言 ▾] → [目标语言 ▾]`。
 *
 * - 源语言 = 转写语种（默认「自动检测」，跟随 `realtime.transcribe_lang` 设置）；auto 模式下
 *   引擎检测锁定后就地回显为「自动检测（中文）」。选项取自设置规格（Rust 注册表单一事实源）。
 * - 目标语言 = 转译目标外语（既有 `TranslateTargetSelect`）；仅转译开启时显示箭头与目标。
 */

import { useTranslation } from "react-i18next";
import { SelectBox } from "../../components/ui/SelectBox";
import {
  langLabelKey,
  TRANSCRIBE_LANG_SETTING,
  useSettingOptions,
} from "../settings/language-options";
import { TranslateTargetSelect } from "./TranslateTargetSelect";

interface SubtitleLangFlowProps {
  /** 转写源语言（"auto" | 语种码）。 */
  transcribeLang: string;
  /** auto 模式下引擎检测锁定的语言码（回显；null = 未检测/显式指定）。 */
  detectedLang: string | null;
  onTranscribeLangChange: (lang: string) => void;
  /** 转写开关关闭时源语言选择禁用。 */
  transcribeDisabled?: boolean;
  /** 转译是否开启：决定是否渲染箭头与目标语言。 */
  translateActive: boolean;
  translateTarget: string;
  onTranslateTargetChange: (lang: string) => void;
  sourceAriaLabel: string;
  targetAriaLabel: string;
}

export function SubtitleLangFlow({
  transcribeLang,
  detectedLang,
  onTranscribeLangChange,
  transcribeDisabled,
  translateActive,
  translateTarget,
  onTranslateTargetChange,
  sourceAriaLabel,
  targetAriaLabel,
}: SubtitleLangFlowProps) {
  const { t } = useTranslation("common");
  const langOptions = useSettingOptions(TRANSCRIBE_LANG_SETTING);
  // 语言码 → 展示名：常用集内走 `common:lang.*`，集外（whisper 可检测 99 语）回退裸码。
  const langDisplay = (code: string) =>
    t(langLabelKey(code).slice("common:".length), { defaultValue: code });
  return (
    <span className="luna-subtitle-langflow">
      <SelectBox
        className="luna-subtitle-target"
        value={transcribeLang}
        disabled={transcribeDisabled}
        onChange={(event) => onTranscribeLangChange(event.currentTarget.value)}
        aria-label={sourceAriaLabel}
      >
        {langOptions.map((lang) => (
          <option key={lang} value={lang}>
            {lang === "auto"
              ? detectedLang
                ? t("lang.autoDetected", { lang: langDisplay(detectedLang) })
                : t("lang.auto")
              : langDisplay(lang)}
          </option>
        ))}
      </SelectBox>
      {translateActive ? (
        <>
          <span className="luna-subtitle-langflow__arrow" aria-hidden="true">
            →
          </span>
          <TranslateTargetSelect
            value={translateTarget}
            onChange={onTranslateTargetChange}
            ariaLabel={targetAriaLabel}
          />
        </>
      ) : null}
    </span>
  );
}
