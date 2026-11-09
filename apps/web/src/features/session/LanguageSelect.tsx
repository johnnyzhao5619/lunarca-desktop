/** 未登录页的语言切换（ADR-178 ⑦）：只列网页已有的语言，每种语言用它自己的名字（取自那种语言的文案）。 */

import { useTranslation } from "react-i18next";
import { chooseLanguage, SUPPORTED_LANGUAGES, type SupportedLanguage } from "../../i18n/i18n";

export function LanguageSelect() {
  const { t, i18n } = useTranslation("web");
  return (
    <select
      className="luna-web-input luna-web-input--sm luna-web-language"
      aria-label={t("signin.language")}
      value={i18n.language}
      onChange={(event) => chooseLanguage(event.target.value as SupportedLanguage)}
    >
      {SUPPORTED_LANGUAGES.map((item) => (
        <option key={item} value={item}>
          {i18n.getFixedT(item, "web")("signin.languageName")}
        </option>
      ))}
    </select>
  );
}
