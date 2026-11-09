/**
 * 最近删除（PRD-21-024，TDD-17 §5.4）：云端保留期内删除的文件，按删除时间倒序分页，显示路径与删除时间。恢复 =
 * 以墓碑 rev 为基线、用那一版的条目与块提交同一个 `file_id` 的新版本（在 Worker / WASM 里）；那一版已过保留期的
 * 只列出、不给恢复。
 */

import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { saveErrorText } from "../../lib/error-message";
import type { DeletedItem, WrittenFile } from "../../worker/protocol";
import { WorkerError, type WorkerClient } from "../../worker/worker-client";

export interface RecentlyDeletedProps {
  client: WorkerClient;
  /** 写入闸关着（只读）时不能恢复。 */
  canRestore: boolean;
  onRestored(file: WrittenFile): void;
}

function codeOf(error: unknown): { code: string; param?: number } {
  return error instanceof WorkerError ? { code: error.code, param: error.param } : { code: "internal" };
}

export function RecentlyDeleted({ client, canRestore, onRestored }: RecentlyDeletedProps) {
  const { t, i18n } = useTranslation("web");
  const [items, setItems] = useState<DeletedItem[] | null>(null);
  const [cursor, setCursor] = useState<number | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const dateFormat = new Intl.DateTimeFormat(i18n.language, { dateStyle: "medium", timeStyle: "short" });

  const load = useCallback(
    async (from: number | null) => {
      try {
        const page = await client.call("deleted", { cursor: from });
        setItems((current) => (from === null ? page.items : [...(current ?? []), ...page.items]));
        setCursor(page.next_cursor);
        setProblem(null);
      } catch (error) {
        const { code, param } = codeOf(error);
        setProblem(saveErrorText(t, code, param, i18n.language));
      }
    },
    [client, i18n.language, t],
  );

  useEffect(() => {
    void load(null);
  }, [load]);

  async function restore(item: DeletedItem) {
    if (item.last_rev === null) return;
    setBusy(item.file_id);
    setNotice(null);
    setProblem(null);
    try {
      const written = await client.call("undelete", { file_id: item.file_id, rev: item.rev, last_rev: item.last_rev });
      setItems((current) => (current ?? []).filter((entry) => entry.file_id !== item.file_id));
      setNotice(t("trash.restored", { path: written.path }));
      onRestored(written);
    } catch (error) {
      const { code, param } = codeOf(error);
      setProblem(saveErrorText(t, code, param, i18n.language));
    } finally {
      setBusy(null);
    }
  }

  return (
    <section className="luna-web-trash" data-testid="recently-deleted">
      <p className="luna-web-hint">{t("trash.hint")}</p>
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
      {items === null ? (
        <p className="luna-web-hint">{t("reader.loading")}</p>
      ) : items.length === 0 ? (
        <p className="luna-web-hint">{t("trash.empty")}</p>
      ) : (
        <ul className="luna-web-trash__list">
          {items.map((item) => (
            <li key={item.file_id} className="luna-web-trash__item">
              <span className="luna-web-list__name">{item.name ?? t("trash.unreadable")}</span>
              {item.path ? <span className="luna-web-list__path">{item.path}</span> : null}
              <span className="luna-web-list__path">{t("trash.deletedAt", { time: dateFormat.format(new Date(item.deleted_at)) })}</span>
              {item.restorable ? (
                <button
                  type="button"
                  className="luna-web-button luna-web-trash__restore"
                  disabled={!canRestore || busy !== null}
                  onClick={() => void restore(item)}
                >
                  {t("trash.restore")}
                </button>
              ) : (
                <span className="luna-web-trash__expired">{t("trash.expired")}</span>
              )}
            </li>
          ))}
        </ul>
      )}
      {cursor !== null ? (
        <button type="button" className="luna-web-button luna-web-button--quiet" onClick={() => void load(cursor)}>
          {t("trash.more")}
        </button>
      ) : null}
    </section>
  );
}
