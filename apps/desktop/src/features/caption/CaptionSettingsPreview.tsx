import { useTranslation } from "react-i18next";
import type { SettingSpecDto } from "../../ipc/bindings";
import { cssVariables } from "../../lib/css-variables";
import { LiveSubtitleView } from "../recording/LiveSubtitleView";
import { captionAppearanceFromSettings } from "../recording/use-caption-appearance";

/** 仅展示已确认设置的离线单句，不持有会话、权限、网络或配置写入。 */
export function CaptionSettingsPreview({ specs }: { specs: readonly SettingSpecDto[] }) {
  const { t } = useTranslation("settings");
  const translation = specs.find((spec) => spec.key === "caption.translation_enabled")?.current_value;
  const separate = specs.find((spec) => spec.key === "caption.separate_translation")?.current_value;
  if (translation?.kind !== "bool" || separate?.kind !== "bool") return null;
  const appearance = captionAppearanceFromSettings(specs);
  const split = translation.value && separate.value;
  const showSource = !translation.value || split || appearance.showSource;
  const showTranslation = translation.value && !split;
  const view = {
    cues: [{ startMs: 0, endMs: 0, source: t("preview.caption.source"), translation: t("preview.caption.translation") }],
    hypotheses: [],
  };
  const look = {
    "data-font-size": appearance.fontSize,
    "data-text-color": appearance.textColor,
    style: cssVariables({
      "--caption-bg-alpha": String(appearance.textOpacity / 100),
      "--caption-frame-alpha": String(appearance.frameOpacity / 100),
    }),
  };
  return (
    <details className="luna-settings-panel luna-settings-preview">
      <summary>{t("preview.caption.title")}</summary>
      <p className="luna-settings-panel__hint">{t("preview.caption.hint")}</p>
      <div className="luna-caption-preview__windows">
        <div className="luna-caption-preview__window">
          {split ? <span className="luna-settings-panel__hint">{t("preview.caption.sourceWindow")}</span> : null}
          <div className="luna-caption-preview__sample luna-caption-look" {...look}
            data-order={showSource && showTranslation && appearance.translationFirst ? "translation-first" : "source-first"}>
            <LiveSubtitleView view={view} showSource={showSource} showTranslation={showTranslation}
              translationFirst={showSource && showTranslation && appearance.translationFirst}
              variant="caption" announce={false} emptyLabel={t("preview.caption.source")}
              sourceAriaLabel={t("preview.caption.title")} />
          </div>
        </div>
        {split ? (
          <div className="luna-caption-preview__window">
            <span className="luna-settings-panel__hint">{t("preview.caption.translationWindow")}</span>
            <div className="luna-caption-preview__sample luna-caption-look" {...look}>
              <LiveSubtitleView view={view} showSource={false} showTranslation
                variant="caption" announce={false} emptyLabel={t("preview.caption.translation")}
                sourceAriaLabel={t("preview.caption.translationWindow")} />
            </div>
          </div>
        ) : null}
      </div>
    </details>
  );
}
