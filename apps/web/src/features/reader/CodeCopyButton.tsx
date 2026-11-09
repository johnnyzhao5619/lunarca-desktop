import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

/** 代码块头条的「复制」（网页走浏览器剪贴板；桌面有它自己那一个）。 */
export function CodeCopyButton({ code }: { code: string }) {
  const { t } = useTranslation("web");
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    if (!copied) return;
    const timer = window.setTimeout(() => setCopied(false), 1500);
    return () => window.clearTimeout(timer);
  }, [copied]);

  return (
    <button
      type="button"
      className="luna-web-button luna-web-button--quiet luna-web-copy"
      data-copy={code}
      onClick={() => {
        void navigator.clipboard?.writeText(code).then(() => setCopied(true));
      }}
    >
      {copied ? t("reader.copied") : t("reader.copy")}
    </button>
  );
}
