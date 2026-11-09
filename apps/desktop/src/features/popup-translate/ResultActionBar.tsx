import { useTranslation } from "react-i18next";
import { CopyIcon, CheckIcon, NotebookIcon, ArrowClockwiseIcon, ReplaceIcon } from "@lunarca/editor-core/Icon";
import { HudActionMenu } from "../../components/ui/HudActionMenu";
import { SpeakButton } from "../tts/SpeakButton";

type ResultActionBarProps = {
  canUseResult: boolean;
  canSaveNote: boolean;
  copied?: boolean;
  onCopy: () => void;
  onRegenerate: () => void;
  onReplace: () => void;
  onSaveNote: () => void;
  showReplace?: boolean;
  /**
   * 「复制」的按钮级别。弹窗翻译里复制**就是**落地（主按钮）；笔记编辑器里落地是
   * 替换 / 插入（由宿主摆主按钮），复制只是次级——同一组件、两种场景，级别由调用方定。
   */
  copyVariant?: "primary" | "secondary";
  /**
   * 「复制」与「朗读」的文案。缺省是弹窗翻译的「复制译文 / 朗读译文」；笔记编辑器里
   * 结果不是译文（优化 / 总结 / 续写都走这条），由调用方给中性文案。
   */
  copyLabel?: string;
  speakLabel?: string;
  /** 译文文本（供「朗读译文」）。 */
  speakText: string;
  /** 译文目标语言（BCP-47，供选嗓音；`null` = 后端检测）。 */
  speakLang: string | null;
  /** 朗读发起标识（区分对比双槽）。 */
  speakScope: string;
};

export function ResultActionBar({
  canUseResult,
  canSaveNote,
  copied = false,
  onCopy,
  onRegenerate,
  onReplace,
  onSaveNote,
  showReplace = true,
  copyVariant = "primary",
  copyLabel,
  speakLabel,
  speakText,
  speakLang,
  speakScope,
}: ResultActionBarProps) {
  const { t } = useTranslation("popup");
  // 一个主动作（ADR-137 V3）：有原文选区时「替换原文」才是用户要的那一下（Bob / Easydict
  // 的口径），复制退为次级；没有选区时复制就是落地。笔记编辑器里的级别由宿主定，不在此改。
  const replaceIsPrimary = showReplace && copyVariant === "primary";
  const copyButton = (
    <button
      type="button"
      className={`luna-btn luna-btn--${replaceIsPrimary ? "secondary" : copyVariant} luna-result-actions__primary`}
      data-copied={copied}
      disabled={!canUseResult}
      onClick={onCopy}
      aria-label={copied ? t("action.copied") : (copyLabel ?? t("action.copy"))}
    >
      {copied ? (
        <>
          <CheckIcon size={14} className="luna-result-actions__copied-icon" />
          <span>{t("action.copied")}</span>
        </>
      ) : (
        <>
          <CopyIcon size={14} />
          <span>{copyLabel ?? t("action.copy")}</span>
        </>
      )}
    </button>
  );
  const replaceButton = showReplace ? (
    <button
      type="button"
      className={`luna-btn ${replaceIsPrimary ? "luna-btn--primary" : "luna-btn--secondary"}`}
      disabled={!canUseResult}
      onClick={onReplace}
    >
      <ReplaceIcon size={14} />
      <span>{t("action.replace")}</span>
    </button>
  ) : null;
  return (
    <div className="luna-result-actions">
      {replaceIsPrimary ? (
        <>
          {replaceButton}
          {copyButton}
        </>
      ) : (
        <>
          {copyButton}
          {replaceButton}
        </>
      )}
      <span className="luna-result-actions__spacer" />
      <SpeakButton
        text={speakText}
        lang={speakLang}
        scope={speakScope}
        className="luna-icon-btn luna-icon-btn--hud"
        title={speakLabel ?? t("action.speak_translation")}
        activeTitle={t("action.speak_stop")}
      />
      <HudActionMenu
        label={t("action.more")}
        items={[
          {
            id: "save-note",
            label: t("action.save_note"),
            icon: <NotebookIcon size={14} />,
            disabled: !canUseResult || !canSaveNote,
            onSelect: onSaveNote,
          },
          {
            id: "regenerate",
            label: t("action.regenerate"),
            icon: <ArrowClockwiseIcon size={14} />,
            onSelect: onRegenerate,
          },
        ]}
      />
    </div>
  );
}
