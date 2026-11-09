/**
 * 一篇的版本历史（PRD-21-026，TDD-17 §5.4）：云端保留期内的版本，新的在前，标出提交设备（网页显示为「网页 · 浏览器名」，
 * 服务端给网页设备记的就是浏览器名）。可以只读查看某一版；「恢复为新版本」= 以那一版的内容提交一个新 rev，现在的内容
 * 仍在历史里。
 */

import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { ReadingView } from "@lunarca/editor-core/ReadingView";
import { saveErrorText } from "../../lib/error-message";
import { webMarkdownHost } from "../../lib/markdown-host";
import type { FileSummary, HistoryRev, HistoryText, WrittenFile } from "../../worker/protocol";
import { WorkerError, type WorkerClient } from "../../worker/worker-client";
import { webReadingVault } from "../reader/web-reading-vault";

export interface HistoryPanelProps {
  client: WorkerClient;
  file: FileSummary;
  /** 写入闸的原因码；`null` = 可写（只读时不给恢复）。 */
  readOnly: string | null;
  /** 恢复之前先把这一篇没存上的改动冲刷掉。 */
  beforeRestore(): Promise<void>;
  onBack(): void;
  onOpenPath(path: string): void;
  onRestored(file: WrittenFile): void;
}

function codeOf(error: unknown): { code: string; param?: number } {
  return error instanceof WorkerError ? { code: error.code, param: error.param } : { code: "internal" };
}

export function HistoryPanel({ client, file, readOnly, beforeRestore, onBack, onOpenPath, onRestored }: HistoryPanelProps) {
  const { t, i18n } = useTranslation("web");
  const [revs, setRevs] = useState<HistoryRev[] | null>(null);
  const [viewing, setViewing] = useState<HistoryText | null>(null);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [generation, setGeneration] = useState(0);
  const vault = useMemo(() => webReadingVault(client), [client]);
  const dateFormat = new Intl.DateTimeFormat(i18n.language, { dateStyle: "medium", timeStyle: "short" });

  function fail(error: unknown) {
    const { code, param } = codeOf(error);
    setProblem(saveErrorText(t, code, param, i18n.language));
  }

  useEffect(() => {
    let active = true;
    setRevs(null);
    void client.call("history", { file_id: file.file_id }).then(
      (next) => {
        if (active) setRevs(next);
      },
      (error: unknown) => {
        if (active) fail(error);
      },
    );
    return () => {
      active = false;
    };
  }, [client, file.file_id, generation]);

  function deviceOf(rev: HistoryRev): string {
    const device = rev.device;
    if (device.kind === "other") return t("history.other");
    if (device.kind === "desktop") return device.name;
    if (device.browser === null) return device.current ? t("history.webUnknownCurrent") : t("history.webUnknown");
    return device.current ? t("history.webCurrent", { browser: device.browser }) : t("history.web", { browser: device.browser });
  }

  async function view(rev: number) {
    setProblem(null);
    try {
      setViewing(await client.call("history_text", { file_id: file.file_id, rev }));
    } catch (error) {
      fail(error);
    }
  }

  async function restore(rev: number) {
    setBusy(true);
    setNotice(null);
    setProblem(null);
    try {
      await beforeRestore();
      const written = await client.call("restore_rev", { file_id: file.file_id, rev });
      setNotice(t("history.restored", { rev }));
      setViewing(null);
      setGeneration((n) => n + 1);
      onRestored(written);
    } catch (error) {
      fail(error);
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="luna-web-history" data-testid="history-panel">
      <header className="luna-web-history__head">
        <button type="button" className="luna-web-button luna-web-button--quiet" onClick={onBack}>
          {t("history.back")}
        </button>
        <h2 className="luna-web-history__title">{t("history.title", { name: file.name })}</h2>
      </header>
      <p className="luna-web-hint">{t("history.hint")}</p>
      {notice ? (
        <p className="luna-web-hint" role="status">
          {notice}
        </p>
      ) : null}
      {problem ? (
        <p className="luna-web-error" role="alert">
          {problem}
        </p>
      ) : null}
      {revs === null ? (
        <p className="luna-web-hint">{t("history.loading")}</p>
      ) : revs.length <= 1 ? (
        <p className="luna-web-hint">{t("history.empty")}</p>
      ) : null}
      {revs && revs.length > 0 ? (
        <ol className="luna-web-history__list">
          {revs.map((rev, index) => (
            <li key={rev.rev} className="luna-web-history__item" data-rev={rev.rev}>
              <span className="luna-web-history__label">
                <span className="luna-web-list__name">{t("history.rev", { rev: rev.rev })}</span>
                {index === 0 ? <span className="luna-web-history__badge">{t("history.current")}</span> : null}
                {rev.deleted ? <span className="luna-web-history__badge">{t("history.deleted")}</span> : null}
              </span>
              <span className="luna-web-list__path">{dateFormat.format(new Date(rev.committed_at))}</span>
              <span className="luna-web-list__path" data-testid="history-device">
                {deviceOf(rev)}
              </span>
              {!rev.deleted ? (
                <span className="luna-web-history__actions">
                  <button type="button" className="luna-web-button luna-web-button--quiet" onClick={() => void view(rev.rev)}>
                    {t("history.view")}
                  </button>
                  {index > 0 ? (
                    <button
                      type="button"
                      className="luna-web-button"
                      disabled={readOnly !== null || busy}
                      onClick={() => void restore(rev.rev)}
                    >
                      {t("history.restore")}
                    </button>
                  ) : null}
                </span>
              ) : null}
            </li>
          ))}
        </ol>
      ) : null}
      {viewing ? (
        <article className="luna-web-history__preview" data-testid="history-preview" aria-label={t("history.viewing", { rev: viewing.rev })}>
          <p className="luna-web-hint">{t("history.viewing", { rev: viewing.rev })}</p>
          {viewing.text === null ? (
            <p className="luna-web-hint">{t("history.notText")}</p>
          ) : (
            <ReadingView host={webMarkdownHost} vault={vault} content={viewing.text} notePath={viewing.path} onOpenPath={onOpenPath} collapseProperties />
          )}
        </article>
      ) : null}
    </section>
  );
}
