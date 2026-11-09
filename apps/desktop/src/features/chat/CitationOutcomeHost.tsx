/**
 * 出处跳转的失效提示宿主（PRD-16-012「失效显式化」）。
 *
 * **它必须与对话视图解耦**：`open_citation` 在 Rust 内完成导航，主区当场从对话切到那篇
 * 笔记——提示若渲染在消息行里，就渲染在一个用户已经看不见的视图上（真机第七轮实测：
 * `moved` / `heading_only` / `lost` 三种降级终态全是静默的）。故常驻主窗口 shell，
 * 与知识回执同一层。命中原处（`exact`）不打扰。
 */

import { useEffect, useState } from "react";
import { IconButton } from "../../components/ui/IconButton";
import { useTranslation } from "react-i18next";
import { CloseIcon } from "@lunarca/editor-core/Icon";
import { CITATION_OUTCOME_EVENT, type CitationOutcomeDetail } from "../../lib/app-events";

const AUTO_DISMISS_MS = 6_000;

export function CitationOutcomeHost() {
  const { t } = useTranslation("chat");
  const [status, setStatus] = useState<CitationOutcomeDetail | null>(null);

  useEffect(() => {
    function onOutcome(event: Event) {
      const detail = (event as CustomEvent<CitationOutcomeDetail>).detail;
      if (!detail || detail.outcome === "exact") return;
      setStatus(detail);
    }
    window.addEventListener(CITATION_OUTCOME_EVENT, onOutcome);
    return () => window.removeEventListener(CITATION_OUTCOME_EVENT, onOutcome);
  }, []);

  useEffect(() => {
    if (!status) return;
    const timer = window.setTimeout(() => setStatus(null), AUTO_DISMISS_MS);
    return () => window.clearTimeout(timer);
  }, [status]);

  if (!status) return null;
  return (
    <p className="luna-citation-outcome" role="status" aria-live="polite">
      {t(`sources.openOutcome.${status.outcome}`, { title: status.title })}
      <IconButton
        label={t("sources.dismissOutcome")}
        icon={<CloseIcon size={12} />}
        onClick={() => setStatus(null)}
      />
    </p>
  );
}
