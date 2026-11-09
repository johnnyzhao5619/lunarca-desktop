/**
 * 全文搜索的结果（PRD-21-041）：查询语法、命中与片段的切分都在 WASM（与桌面搜索视图同一份），这里只把分段画出来
 * ——命中的字包成 `<mark>`，不在这里算任何偏移。
 */

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { saveErrorText } from "../../lib/error-message";
import type { FileSummary, FulltextHits } from "../../worker/protocol";
import { WorkerError, type WorkerClient } from "../../worker/worker-client";

/** 停止输入多久之后才搜（每一次都要扫一遍内存里的全部正文）。 */
const SEARCH_DEBOUNCE_MS = 200;

export interface FulltextResultsProps {
  client: WorkerClient;
  query: string;
  selectedId: string | null;
  onSelect(file: FileSummary): void;
}

export function FulltextResults({ client, query, selectedId, onSelect }: FulltextResultsProps) {
  const { t, i18n } = useTranslation("web");
  const [hits, setHits] = useState<FulltextHits | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    const timer = window.setTimeout(() => {
      void client.call("search", { query }).then(
        (next) => {
          if (!active) return;
          setHits(next);
          setProblem(null);
        },
        (error: unknown) => {
          if (active) setProblem(error instanceof WorkerError ? error.code : "internal");
        },
      );
    }, SEARCH_DEBOUNCE_MS);
    return () => {
      active = false;
      window.clearTimeout(timer);
    };
  }, [client, query]);

  if (problem) {
    return (
      <p className="luna-web-error" role="alert">
        {saveErrorText(t, problem, undefined, i18n.language)}
      </p>
    );
  }
  if (!hits) return <p className="luna-web-hint">{t("reader.loading")}</p>;
  if (hits.notes.length === 0) return <p className="luna-web-hint">{t("fulltext.empty")}</p>;
  return (
    <div className="luna-web-hits" data-testid="fulltext-results">
      <p className="luna-web-hits__summary" role="status">
        {t("fulltext.summary", { notes: hits.notes.length, matches: hits.total_matches })}
      </p>
      {hits.truncated ? <p className="luna-web-hint">{t("fulltext.truncated")}</p> : null}
      <ul className="luna-web-list">
        {hits.notes.map((note) => {
          const file: FileSummary = { file_id: note.file_id, path: note.path, name: note.name, size: 0, mtime_ms: note.mtime_ms };
          return (
            <li key={note.file_id}>
              <button
                type="button"
                className="luna-web-list__row"
                aria-current={note.file_id === selectedId ? "page" : undefined}
                onClick={() => onSelect(file)}
              >
                <span className="luna-web-list__name">{note.title}</span>
                <span className="luna-web-list__path">{note.path}</span>
                {note.lines.map((line) => (
                  <span key={line.line} className="luna-web-hits__line" title={t("fulltext.line", { line: line.line })}>
                    {line.segments.map((segment, index) =>
                      segment.hit ? (
                        <mark key={index} className="luna-web-hits__mark">
                          {segment.text}
                        </mark>
                      ) : (
                        <span key={index}>{segment.text}</span>
                      ),
                    )}
                  </span>
                ))}
                {note.lines_truncated ? <span className="luna-web-list__path">{t("fulltext.moreLines")}</span> : null}
              </button>
            </li>
          );
        })}
      </ul>
    </div>
  );
}
