// 版本历史面板（PRD-06-040 快照 / PRD-06-041 版本对比）。
//
// 此前只有一块 `<pre>` 全文：要判断"该不该回滚"，用户得肉眼比对两屏文字——等于没有对比。
// 现在默认呈现**统一行差异**（Rust 侧 `luna_vault::diff` 算，前端只渲染），全文作为第二读法。
//
// 回滚是不可逆动作，故改为**面板内两步确认**（原生 `window.confirm` 夺焦、无法主题化、
// 也说不清"回滚前会不会留底"——而它其实会：回滚本身先给当前内容留一份快照）。

import { useEffect, useState } from "react";
import { useTablistNavigation } from "../../hooks/use-tablist-navigation";
import { useTranslation } from "react-i18next";
import { DiffView } from "@lunarca/editor-core/DiffView";
import { formatRelativeTime } from "../../lib/format-relative-time";
import { ClockIcon } from "@lunarca/editor-core/Icon";
import { useVersionHistory, VERSION_VIEW_MODES, type VersionEntry } from "./use-version-history";

export interface VersionHistoryPanelProps {
  path: string | null;
  onClose: () => void;
}

function formatCreated(entry: VersionEntry): string {
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(new Date(entry.createdMs));
}

export function VersionHistoryPanel({ path, onClose }: VersionHistoryPanelProps) {
  const { t, i18n } = useTranslation("editor");
  // 面板打开那一刻的时间基准：相对时间无需逐秒跳动，固定基准也让渲染保持纯粹。
  const [now] = useState(() => Date.now());
  const {
    entries,
    selectedId,
    setSelectedId,
    viewMode,
    setViewMode,
    preview,
    diff,
    error,
    loading,
    restore,
  } = useVersionHistory(path);
  const [confirmingRestore, setConfirmingRestore] = useState(false);
  const modeTabs = useTablistNavigation(VERSION_VIEW_MODES, viewMode, setViewMode);
  const selected = entries.find((entry) => entry.id === selectedId) ?? null;

  // 换版本 / 换读法即作废确认态——否则"确认"会落在用户已经不再看着的那一版上。
  useEffect(() => {
    setConfirmingRestore(false);
  }, [selectedId, viewMode, path]);

  return (
    <aside className="luna-version-history" aria-label={t("version.title")}>
      <header className="luna-version-history__header">
        <h3>{t("version.title")}</h3>
        <button type="button" className="luna-version-history__close" onClick={onClose}>
          {t("version.close")}
        </button>
      </header>
      {error ? (
        <p className="luna-field-error" role="alert">
          {error}
        </p>
      ) : null}
      {loading ? <p className="luna-empty">{t("loading")}</p> : null}
      {!loading && entries.length === 0 ? (
        <div className="luna-version-history__empty">
          <ClockIcon size={24} />
          <p className="luna-empty">{t("version.empty")}</p>
          <p className="luna-version-history__empty-hint">{t("version.emptyHint")}</p>
        </div>
      ) : null}
      {entries.length > 0 ? (
        <div className="luna-version-history__grid">
          <ul className="luna-version-history__list">
            {entries.map((entry) => (
              <li key={entry.id}>
                <button
                  type="button"
                  aria-current={entry.id === selectedId || undefined}
                  onClick={() => {
                    setSelectedId(entry.id);
                  }}
                  title={formatCreated(entry)}
                >
                  <span>{formatRelativeTime(entry.createdMs, now, i18n.language)}</span>
                  <small>{t("version.size_bytes", { count: entry.sizeBytes })}</small>
                </button>
              </li>
            ))}
          </ul>
          <section className="luna-version-history__detail" aria-label={t("version.preview")}>
            <header className="luna-version-history__detail-head">
              <div
                className="luna-version-history__modes"
                role="tablist"
                aria-label={t("version.viewMode")}
              >
                <button
                  ref={modeTabs.register("diff")}
                  type="button"
                  role="tab"
                  aria-selected={viewMode === "diff"}
                  tabIndex={modeTabs.tabIndexOf("diff")}
                  onClick={() => setViewMode("diff")}
                  onKeyDown={modeTabs.onKeyDown}
                >
                  {t("version.viewDiff")}
                </button>
                <button
                  ref={modeTabs.register("full")}
                  type="button"
                  role="tab"
                  aria-selected={viewMode === "full"}
                  tabIndex={modeTabs.tabIndexOf("full")}
                  onClick={() => setViewMode("full")}
                  onKeyDown={modeTabs.onKeyDown}
                >
                  {t("version.viewFull")}
                </button>
              </div>
              {viewMode === "diff" && diff ? (
                <span className="luna-version-history__stat">
                  <span className="luna-version-history__stat-add">+{diff.added}</span>
                  <span className="luna-version-history__stat-del">−{diff.removed}</span>
                </span>
              ) : null}
              {confirmingRestore ? (
                <span className="luna-version-history__confirm">
                  <span>{t("version.restore_confirm")}</span>
                  <button
                    type="button"
                    className="luna-btn luna-btn--primary"
                    onClick={() => {
                      setConfirmingRestore(false);
                      void restore();
                    }}
                  >
                    {t("version.restoreConfirmYes")}
                  </button>
                  <button
                    type="button"
                    className="luna-btn luna-btn--secondary"
                    onClick={() => setConfirmingRestore(false)}
                  >
                    {t("version.restoreConfirmNo")}
                  </button>
                </span>
              ) : (
                <button
                  type="button"
                  className="luna-btn luna-btn--secondary"
                  disabled={!selected}
                  onClick={() => setConfirmingRestore(true)}
                >
                  {t("version.restore")}
                </button>
              )}
            </header>
            {viewMode === "diff" ? (
              <div className="luna-version-history__diff-pane">
                {diff?.degraded ? (
                  <p className="luna-version-history__note">{t("version.diffDegraded")}</p>
                ) : null}
                {diff && diff.added === 0 && diff.removed === 0 ? (
                  <p className="luna-empty">{t("version.diffIdentical")}</p>
                ) : null}
                {diff ? <DiffView diff={diff} /> : null}
              </div>
            ) : (
              <pre className="luna-version-history__full">{preview}</pre>
            )}
          </section>
        </div>
      ) : null}
    </aside>
  );
}
