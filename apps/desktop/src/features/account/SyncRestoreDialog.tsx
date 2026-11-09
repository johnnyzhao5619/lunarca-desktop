/**
 * 整库时间点恢复（PRD-13-007、ADR-148 ⑪）：选一个时刻，把这份库在那一刻的样子恢复到**库旁边的
 * 新文件夹**——从不覆盖当前库。恢复完可以并排打开（副库窗口）对比，再自己决定拿回哪些。
 *
 * 时刻或位置改了就重新问一次「那一刻对应哪次提交、最早能恢复到多早、落到哪个新文件夹」；落点
 * 由 Rust 在所选父文件夹（默认库旁边）里起一个带**所选时刻**的不重名名字，换位置只选父文件夹
 * （不手填路径）。
 */

import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { useDialogBehavior } from "../../hooks/use-dialog-behavior";
import { commands, events, type RestorePointDto, type RestoreReportDto } from "../../ipc/bindings";
import { formatBytes } from "@lunarca/editor-core/format-bytes";
import { formatDateTime } from "../../lib/format-datetime";
import { syncErrorText } from "./sync-error";

/** `datetime-local` 的值（本地时区，到分钟）。 */
function toLocalInput(ms: number): string {
  const d = new Date(ms);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

export function SyncRestoreDialog({
  vaultPath,
  vaultName,
  onClose,
}: {
  vaultPath: string;
  vaultName: string;
  onClose: () => void;
}) {
  const { t, i18n } = useTranslation("cloud");
  const headingRef = useRef<HTMLHeadingElement>(null);
  const [when, setWhen] = useState(() => toLocalInput(Date.now()));
  const [point, setPoint] = useState<RestorePointDto | null>(null);
  /** 用户选的父文件夹；`null` = 库旁边。 */
  const [parent, setParent] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [progress, setProgress] = useState<{ done: number; total: number } | null>(null);
  const [report, setReport] = useState<RestoreReportDto | null>(null);
  const sequence = useRef(0);

  const dialogRef = useDialogBehavior<HTMLDivElement>({
    onClose,
    closeOnEscape: !busy,
    initialFocusRef: headingRef,
  });

  // 时刻或位置一改就重新解析（只取最后一次的回答）。
  useEffect(() => {
    const at = new Date(when).getTime();
    if (!Number.isFinite(at)) return;
    const current = ++sequence.current;
    setError(null);
    void commands.syncRestorePoint(vaultPath, at, parent).then((result) => {
      if (current !== sequence.current) return;
      if (result.status === "error") {
        setPoint(null);
        setError(result.error);
        return;
      }
      setPoint(result.data);
    });
  }, [vaultPath, when, parent]);

  useEffect(() => {
    const disposer = events.syncRestoreProgress.listen((event) => {
      setProgress({ done: event.payload.done ?? 0, total: event.payload.total ?? 0 });
    });
    return () => {
      void disposer.then((dispose) => dispose());
    };
  }, []);

  const chooseParent = async () => {
    const result = await commands.pickFolder(null);
    if (result.status !== "ok" || !result.data) return;
    setParent(result.data);
  };

  const restore = async () => {
    if (!point || busy) return;
    setBusy(true);
    setError(null);
    setProgress(null);
    const result = await commands.syncRestoreToFolder(vaultPath, point.at_seq, point.destination);
    setBusy(false);
    if (result.status === "error") {
      setError(result.error);
      return;
    }
    setReport(result.data);
  };

  const committed = point?.committed_at_ms ?? null;
  return (
    <div className="luna-overlay" role="presentation" onClick={() => (busy ? undefined : onClose())}>
      <div
        ref={dialogRef}
        className="luna-dialog cloud-sync__dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="cloud-sync-restore-title"
        aria-busy={busy}
        onClick={(event) => event.stopPropagation()}
      >
        <header className="luna-dialog__header">
          <h3 id="cloud-sync-restore-title" ref={headingRef} tabIndex={-1}>
            {t("sync.restore.title", { name: vaultName })}
          </h3>
        </header>
        {report ? (
          <div className="cloud-sync__body">
            <p role="status">
              {t("sync.restore.done", {
                count: report.files ?? 0,
                size: formatBytes(report.bytes ?? 0, i18n.language),
                path: report.destination,
              })}
            </p>
            {(report.skipped ?? 0) > 0 ? (
              <p className="cloud-sync__notice" data-tone="warning">
                {t("sync.restore.skipped", { count: report.skipped ?? 0 })}
              </p>
            ) : null}
            <div className="luna-dialog__actions">
              <button type="button" className="luna-btn luna-btn--secondary" onClick={onClose}>
                {t("sync.restore.close")}
              </button>
              <button
                type="button"
                className="luna-btn luna-btn--primary"
                onClick={() => {
                  void commands.openVaultWindow(report.destination);
                  onClose();
                }}
              >
                {t("sync.restore.open_side_by_side")}
              </button>
            </div>
          </div>
        ) : (
          <form
            className="cloud-sync__body"
            onSubmit={(event) => {
              event.preventDefault();
              void restore();
            }}
          >
            <p className="luna-settings-panel__hint">{t("sync.restore.desc")}</p>
            <label className="cloud-sync__label" htmlFor="cloud-sync-restore-when">
              {t("sync.restore.when")}
            </label>
            <input
              id="cloud-sync-restore-when"
              className="luna-input"
              type="datetime-local"
              value={when}
              max={toLocalInput(Date.now())}
              disabled={busy}
              onChange={(event) => setWhen(event.target.value)}
            />
            {point ? (
              <p className="luna-settings-panel__hint" role="status">
                {committed === null
                  ? t("sync.restore.before_first")
                  : t("sync.restore.state_at", { time: formatDateTime(committed, i18n.language) })}{" "}
                {t("sync.restore.earliest", { time: formatDateTime(point.earliest_ms ?? 0, i18n.language) })}
              </p>
            ) : null}
            {point ? (
              <div className="cloud-sync__destination">
                <span className="cloud-sync__label">{t("sync.restore.destination")}</span>
                <span className="cloud-sync__destination-path">{point.destination}</span>
                <button
                  type="button"
                  className="luna-btn luna-btn--secondary luna-btn--sm"
                  disabled={busy}
                  onClick={() => void chooseParent()}
                >
                  {t("sync.restore.change_location")}
                </button>
              </div>
            ) : null}
            {busy && progress ? (
              <p role="status">{t("sync.restore.progress", { done: progress.done, total: progress.total })}</p>
            ) : null}
            {error ? (
              <p className="luna-field-error" role="alert">
                {error === "sync_cursor_expired" ? t("sync.restore.expired") : syncErrorText(error, t)}
              </p>
            ) : null}
            <div className="luna-dialog__actions">
              <button type="button" className="luna-btn luna-btn--secondary" disabled={busy} onClick={onClose}>
                {t("sync.restore.cancel")}
              </button>
              <button type="submit" className="luna-btn luna-btn--primary" disabled={busy || !point}>
                {busy ? t("sync.restore.restoring") : t("sync.restore.submit")}
              </button>
            </div>
          </form>
        )}
      </div>
    </div>
  );
}
