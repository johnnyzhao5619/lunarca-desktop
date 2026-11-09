import { useTranslation } from "react-i18next";

import { LanguageSelect } from "./LanguageSelect";
import logo from "./lunarca-icon.svg";

export function SignedOutHeader() {
  const { t } = useTranslation("web");
  return (
    <header className="luna-web-header">
      <a className="luna-web-header__home" href="https://lunarca.app/" target="_blank" rel="noopener noreferrer">
        <img className="luna-web-header__logo" src={logo} alt="" />
        {t("signin.home")}
      </a>
      <div className="luna-web-header__end">
        <LanguageSelect />
        <a className="luna-web-button" href={t("signin.downloadUrl")} target="_blank" rel="noopener noreferrer">
          {t("signin.downloadNav")}
        </a>
      </div>
    </header>
  );
}
