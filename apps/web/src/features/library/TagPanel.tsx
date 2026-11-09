/**
 * 标签（PRD-21-042）：全库标签与各自的笔记数，点一个列出带它（含子标签）的笔记。标签怎么读、怎么归并与桌面同一份
 * （frontmatter `tags`、`luna.ai_tags` 与正文 `#标签`，在 WASM）；要先有全文索引（同一份正文）。
 */

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { saveErrorText } from "../../lib/error-message";
import type { FileSummary, TagCount } from "../../worker/protocol";
import { WorkerError, type WorkerClient } from "../../worker/worker-client";

export interface TagPanelProps {
  client: WorkerClient;
  selectedId: string | null;
  onSelect(file: FileSummary): void;
}

function codeOf(error: unknown): string {
  return error instanceof WorkerError ? error.code : "internal";
}

export function TagPanel({ client, selectedId, onSelect }: TagPanelProps) {
  const { t, i18n } = useTranslation("web");
  const [tags, setTags] = useState<TagCount[] | null>(null);
  const [tag, setTag] = useState<string | null>(null);
  const [notes, setNotes] = useState<FileSummary[] | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    void client.call("tags", {}).then(
      (next) => {
        if (active) setTags(next);
      },
      (error: unknown) => {
        if (active) setProblem(codeOf(error));
      },
    );
    return () => {
      active = false;
    };
  }, [client]);

  useEffect(() => {
    if (tag === null) return;
    let active = true;
    setNotes(null);
    void client.call("tag_notes", { tag }).then(
      (next) => {
        if (active) setNotes(next);
      },
      (error: unknown) => {
        if (active) setProblem(codeOf(error));
      },
    );
    return () => {
      active = false;
    };
  }, [client, tag]);

  if (problem) {
    return (
      <p className="luna-web-error" role="alert">
        {saveErrorText(t, problem, undefined, i18n.language)}
      </p>
    );
  }
  if (tag !== null) {
    return (
      <section className="luna-web-tags" data-testid="tag-notes">
        <button type="button" className="luna-web-button luna-web-button--quiet luna-web-tags__back" onClick={() => setTag(null)}>
          {t("tags.all")}
        </button>
        <p className="luna-web-tags__heading">{t("tags.notesTitle", { tag })}</p>
        {notes === null ? (
          <p className="luna-web-hint">{t("reader.loading")}</p>
        ) : notes.length === 0 ? (
          <p className="luna-web-hint">{t("tags.none")}</p>
        ) : (
          <ul className="luna-web-list">
            {notes.map((file) => (
              <li key={file.file_id}>
                <button
                  type="button"
                  className="luna-web-list__row"
                  aria-current={file.file_id === selectedId ? "page" : undefined}
                  onClick={() => onSelect(file)}
                >
                  <span className="luna-web-list__name">{file.name}</span>
                  <span className="luna-web-list__path">{file.path}</span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>
    );
  }
  if (tags === null) return <p className="luna-web-hint">{t("reader.loading")}</p>;
  if (tags.length === 0) return <p className="luna-web-hint">{t("tags.empty")}</p>;
  return (
    <ul className="luna-web-tags" data-testid="tag-list">
      {tags.map((item) => (
        <li key={item.tag}>
          <button type="button" className="luna-web-tags__row" onClick={() => setTag(item.tag)}>
            <span className="luna-web-tags__name">#{item.tag}</span>
            {item.ai_only ? (
              <span className="luna-web-tags__badge" title={t("tags.aiOnlyHint")}>
                {t("tags.aiOnly")}
              </span>
            ) : null}
            <span className="luna-web-tags__count">{t("tags.count", { count: item.count })}</span>
          </button>
        </li>
      ))}
    </ul>
  );
}
