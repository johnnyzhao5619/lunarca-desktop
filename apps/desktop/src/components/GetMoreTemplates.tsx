/**
 * 选择面底部的「获取更多模板」（纵切 7b，PRD-18-004）：笔记模板选择器、插入模板与整理模板选择面共用。
 *
 * 只在在线模板目录可用时出现（Rust `template_market` 回非 `null`）：严格本地、未登录或服务端没发布模板目录时不放这个
 * 入口（PRD-18-006「不放不可执行的入口」）。点它去「应用 › 模板」；所在的对话框随之关上（`onNavigate`）。
 */

import { useTranslation } from "react-i18next";

import { useTemplateMarket } from "../hooks/use-template-market";
import { APPS_TEMPLATES_ROUTE } from "../lib/app-route";
import { goTo } from "../lib/navigate";

export function GetMoreTemplates({ onNavigate }: { onNavigate?: () => void }) {
  const { t } = useTranslation("workspace");
  const { market } = useTemplateMarket();
  if (market === null || market === undefined) return null;
  return (
    <button
      type="button"
      className="luna-btn luna-btn--secondary luna-btn--sm"
      onClick={() => {
        onNavigate?.();
        goTo(APPS_TEMPLATES_ROUTE);
      }}
    >
      {t("apps.templates.getMore")}
    </button>
  );
}
