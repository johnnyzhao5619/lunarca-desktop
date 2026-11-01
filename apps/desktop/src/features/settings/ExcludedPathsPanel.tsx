/**
 * 排除的路径（PRD-06-062，设置「笔记库」页高级层；笔记库对标轮 V4a）。
 *
 * 列出当前排除的文件夹与笔记，可逐条取消。添加入口在文件树右键（那里有路径，不让用户手敲）。
 * 规则与全部过滤都在 Rust（`luna_vault::exclusions`），前端零业务（ADR-003）。
 */

import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useExcludedPaths } from "../../hooks/use-excluded-paths";
import { vaultErrorText } from "../../lib/vault-error";
import { InlineDetails } from "../../components/ui/InlineDetails";

export function ExcludedPathsPanel() {
  const { t } = useTranslation("settings");
  const { rules, setExcluded } = useExcludedPaths();
  const [feedback, setFeedback] = useState<string | null>(null);

  function include(path: string) {
    setFeedback(null);
    void setExcluded(path, false).then((error) => {
      if (error !== null) {
        setFeedback(vaultErrorText(error));
      }
    });
  }

  return (
    <section
      id="settings-section-excluded_paths"
      tabIndex={-1}
      className="luna-settings-panel"
      aria-label={t("excluded_paths.label")}
    >
      <header className="luna-settings-panel__header">
        <h2>{t("excluded_paths.label")}</h2>
      </header>
      <p className="luna-settings-panel__hint">{t("excluded_paths.hint")}</p>
      {/* 作用范围与保护规则按需展开（2026-10-08 W12）：一句常显说清这是什么，细节不压在首屏。 */}
      <InlineDetails summary={t("excluded_paths.scope_summary")}>
        <p>{t("excluded_paths.scope")}</p>
      </InlineDetails>
      {rules.length === 0 ? (
        <p className="luna-settings-panel__hint">{t("excluded_paths.none")}</p>
      ) : (
        <ul className="luna-settings-panel__rows">
          {rules.map((path) => (
            <li key={path} className="luna-settings-panel__row">
              <span className="luna-settings-panel__row-label" title={path}>
                <span className="luna-settings-panel__row-title">{path}</span>
              </span>
              <button
                type="button"
                className="luna-btn luna-btn--secondary"
                aria-label={t("excluded_paths.remove_label", { path })}
                onClick={() => include(path)}
              >
                {t("excluded_paths.remove")}
              </button>
            </li>
          ))}
        </ul>
      )}
      {feedback && (
        <p className="luna-settings-panel__hint" role="status">
          {feedback}
        </p>
      )}
    </section>
  );
}
