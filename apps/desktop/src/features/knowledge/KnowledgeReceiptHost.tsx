import { useEffect, useState } from "react";
import { IconButton } from "../../components/ui/IconButton";
import { useTranslation } from "react-i18next";
import { CloseIcon } from "@lunarca/editor-core/Icon";
import { events, type KnowledgeReceiptDto } from "../../ipc/bindings";
import { upsertKnowledgeReceipt } from "./knowledge-receipts";
import { openNote } from "../../lib/navigate";

const AUTO_DISMISS_MS = 8_000;



function titleOf(note: { path: string }): string {
  return note.path.split("/").pop()?.replace(/\.md$/i, "") ?? note.path;
}

export function KnowledgeReceiptHost() {
  const { t } = useTranslation("notifications");
  const [items, setItems] = useState<KnowledgeReceiptDto[]>([]);

  useEffect(() => {
    const unlisten = events.knowledgeReceipt.listen((event) => {
      setItems((current) => upsertKnowledgeReceipt(current, event.payload));
    });
    return () => {
      void unlisten.then((dispose) => dispose());
    };
  }, []);

  useEffect(() => {
    const transient = items.filter(
      (item) => item.outcome === "saved" || item.outcome === "queued" || item.outcome === "updated",
    );
    if (transient.length === 0) return;
    const timers = transient.map((item) =>
      window.setTimeout(() => {
        setItems((current) =>
          current.filter((candidate) => candidate.receipt_id !== item.receipt_id),
        );
      }, AUTO_DISMISS_MS),
    );
    return () => timers.forEach((timer) => window.clearTimeout(timer));
  }, [items]);

  if (items.length === 0) return null;
  return (
    <section className="luna-knowledge-receipts" aria-label={t("knowledgeReceipt.hostLabel")}>
      {items.map((item) => {
        const note = item.primary_note;
        const noteTitle = note?.path.split("/").pop()?.replace(/\.md$/i, "") ?? "";
        // 文案跟着终态走：Rust 已经把「更新了哪几页」算成事实，
        // 前端只翻译，绝不把「已排入」写成「已更新」（PRD-16-038）。
        const topicTitles = item.updated_topics
          .map(titleOf)
          .join(t("knowledgeReceipt.stepSeparator"));
        // 知识地图刷新的主体就是地图自己：套用「X 已归入《Y》」会读成
        // 「"知识地图"已归入《知识地图》」——同一句话必须按对象说话。
        const bodyKey =
          item.kind === "knowledge_map" && (item.outcome === "updated" || item.outcome === "queued")
            ? `knowledgeReceipt.body.${item.outcome}_map`
            : `knowledgeReceipt.body.${item.outcome}`;
        const body = t(bodyKey, {
          title: noteTitle,
          topics: topicTitles,
          updated: item.updated_topics.length,
        });
        return (
          <article
            className="luna-notice-card luna-knowledge-receipt"
            data-outcome={item.outcome}
            key={item.receipt_id}
            role="status"
          >
            <header className="luna-notice-card__header">
              <strong>{t(`knowledgeReceipt.outcome.${item.outcome}`)}</strong>
              <IconButton
                label={t("action.dismiss")}
                icon={<CloseIcon size={12} />}
                onClick={() =>
                  setItems((current) =>
                    current.filter((candidate) => candidate.receipt_id !== item.receipt_id),
                  )
                }
              />
            </header>
            <p>{body}</p>
            {item.applied_items !== null ? (
              <small>{t("knowledgeReceipt.appliedItems", { count: item.applied_items })}</small>
            ) : null}
            {item.queued_steps.length > 0 ? (
              <small>
                {t("knowledgeReceipt.queued", {
                  steps: item.queued_steps
                    .map((step) => t(`knowledgeReceipt.step.${step}`))
                    .join(t("knowledgeReceipt.stepSeparator")),
                })}
              </small>
            ) : null}
            {item.degraded_steps.length > 0 ? (
              <small>
                {t("knowledgeReceipt.degraded", {
                  steps: item.degraded_steps
                    .map((step) => t(`knowledgeReceipt.step.${step}`))
                    .join(t("knowledgeReceipt.stepSeparator")),
                })}
              </small>
            ) : null}
            {note && item.primary_action !== null ? (
              <button
                type="button"
                className="luna-notice-card__action"
                onClick={() => openNote(note.path)}
              >
                {t("knowledgeReceipt.open")}
              </button>
            ) : null}
          </article>
        );
      })}
    </section>
  );
}
