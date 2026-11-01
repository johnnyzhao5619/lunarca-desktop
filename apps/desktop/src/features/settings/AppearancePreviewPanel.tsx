import { useTranslation } from "react-i18next";

/** 示例继承已生效的主题、字号与密度，不维护另一份外观状态。 */
export function AppearancePreviewPanel() {
  const { t } = useTranslation("settings");
  return (
    <details className="luna-settings-panel luna-settings-preview">
      <summary>{t("preview.appearance.title")}</summary>
      <p className="luna-settings-panel__hint">{t("preview.appearance.hint")}</p>
      <div className="luna-settings-preview__note">
        <strong className="luna-settings-preview__title">{t("preview.appearance.sampleTitle")}</strong>
        <p>{t("preview.appearance.sampleBody")}</p>
        <div className="luna-settings-preview__meta">
          <span className="luna-count-badge">{t("preview.appearance.sampleTag")}</span>
          <span>{t("preview.appearance.sampleStatus")}</span>
        </div>
      </div>
    </details>
  );
}
