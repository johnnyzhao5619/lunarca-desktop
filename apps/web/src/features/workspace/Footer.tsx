import { useTranslation } from "react-i18next";

/** 构建时注入的服务端版本（ADR-177 ⑨：网页与服务端锁步发布，对外显示服务端版本）。 */
declare const __LUNARCA_SERVER_VERSION__: string;

/** 页脚（PRD-21-053、TDD-17 §6.4）：服务端版本与对应源码（AGPL）、服务条款、云隐私政策。 */
export function Footer() {
  const { t } = useTranslation("web");
  return (
    <footer className="luna-web-footer">
      <span>{t("footer.version", { version: __LUNARCA_SERVER_VERSION__ })}</span>
      <a href={t("footer.sourceUrl")} target="_blank" rel="noopener noreferrer">
        {t("footer.source")}
      </a>
      <a href={t("footer.termsUrl")} target="_blank" rel="noopener noreferrer">
        {t("footer.terms")}
      </a>
      <a href={t("footer.privacyUrl")} target="_blank" rel="noopener noreferrer">
        {t("footer.privacy")}
      </a>
    </footer>
  );
}
