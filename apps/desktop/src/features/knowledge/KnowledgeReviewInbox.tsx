/**
 * 知识库首页的「待你看」（PRD-16-059 / ADR-136 ③）：后台已为各主题页准备好、仍然有效的整理建议，
 * 按主题分组摆在一处，可以逐页处理，也可以一次接受全部「低风险」的那几页。
 *
 * 判定全在 Rust（`knowledge_review_inbox`）：哪些页有待确认计划、每条是不是低风险（只引用正式
 * 笔记的新增；替换、分歧、转写记载与速记记载都要人看）。**这不是自动写入**：每一页仍由用户点
 * 「应用到本页」或批量接受才写，确认回传的是 Rust 给的操作 ID 与结果指纹，所见即所写。
 *
 * 条目全文直接展开（每页至多 `ITEM_LIMIT` 条，其余去主题页看），批量接受前看得到要写的每一句——
 * 「抽查」不必另做一个随机展开的机关。
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands, events, type KnowledgeReviewInboxDto, type ReviewGroupDto } from "../../ipc/bindings";
import { useVaultReload } from "../../hooks/use-vault-reload";
import { goTo } from "../../lib/navigate";
import { vaultErrorText } from "../../lib/vault-error";
import { rememberApplyUndo } from "./apply-undo-memory";
import { planItemLabelKey } from "./plan-item-label";
import { TOPIC_DIRECTORY_STALE_EVENT, announceTopicDirectoryStale } from "./topic-events";

/** 每页在首页上摆出的条目上限：这是一块提醒，不是第二个主题页。 */
const ITEM_LIMIT = 5;

type Outcome = { kind: "queued"; count: number; failed: number } | { kind: "error"; message: string };

export function KnowledgeReviewInbox() {
  const { t } = useTranslation("workspace");
  const [inbox, setInbox] = useState<KnowledgeReviewInboxDto | null>(null);
  const [busy, setBusy] = useState(false);
  const [outcome, setOutcome] = useState<Outcome | null>(null);
  const requestRef = useRef(0);

  const load = useCallback(() => {
    const request = ++requestRef.current;
    void commands.knowledgeReviewInbox().then((result) => {
      if (request === requestRef.current) {
        setInbox(result.status === "ok" ? result.data : null);
      }
    });
  }, []);

  useEffect(() => {
    load();
    window.addEventListener(TOPIC_DIRECTORY_STALE_EVENT, load);
    // 后台准备好新建议、或排队的应用落定，都经队列广播。
    const unlisten = events.processingQueue.listen(() => load());
    return () => {
      window.removeEventListener(TOPIC_DIRECTORY_STALE_EVENT, load);
      void unlisten.then((dispose) => dispose());
    };
  }, [load]);
  useVaultReload(load);

  /** 确认若干页；成功入队的记下撤销凭据（回到主题页时仍给「撤销这次写入」）。 */
  const confirm = async (groups: ReviewGroupDto[]) => {
    setBusy(true);
    setOutcome(null);
    let queued = 0;
    let failed = 0;
    let lastError: string | null = null;
    for (const group of groups) {
      const result = await commands.confirmSealedIngestPlan(group.operation_id, group.result_hash);
      if (result.status === "ok" && result.data.status === "queued") {
        queued += 1;
        rememberApplyUndo(group.topic_path, { before: group.before_hash, after: group.result_hash });
      } else {
        failed += 1;
        if (result.status === "error") lastError = vaultErrorText(result.error);
      }
    }
    setBusy(false);
    setOutcome(
      queued === 0 && lastError !== null
        ? { kind: "error", message: lastError }
        : { kind: "queued", count: queued, failed },
    );
    announceTopicDirectoryStale();
    load();
  };

  const ignore = async (group: ReviewGroupDto) => {
    setBusy(true);
    setOutcome(null);
    const result = await commands.decideSealedIngestPlan(group.operation_id, "ignored");
    setBusy(false);
    if (result.status === "error") {
      setOutcome({ kind: "error", message: vaultErrorText(result.error) });
    }
    announceTopicDirectoryStale();
    load();
  };

  if (inbox === null || (inbox.groups.length === 0 && outcome === null)) {
    return null;
  }
  const lowRisk = inbox.groups.filter((group) => group.low_risk);
  return (
    <section className="luna-review-inbox" aria-label={t("reviewInbox.title")}>
      <header className="luna-review-inbox__header">
        <h3 className="luna-review-inbox__title">
          {t("reviewInbox.title")}
          <span className="luna-count-badge">{inbox.groups.length + inbox.omitted}</span>
        </h3>
        {lowRisk.length > 0 && (
          <button
            type="button"
            className="luna-btn luna-btn--secondary luna-btn--sm"
            disabled={busy}
            title={t("reviewInbox.acceptLowRiskHint")}
            onClick={() => void confirm(lowRisk)}
          >
            {t("reviewInbox.acceptLowRisk", { count: lowRisk.length })}
          </button>
        )}
      </header>
      {outcome !== null && (
        <p
          className={outcome.kind === "error" ? "luna-field-error" : "luna-review-inbox__notice"}
          role={outcome.kind === "error" ? "alert" : "status"}
        >
          {outcome.kind === "error"
            ? outcome.message
            : outcome.failed > 0
              ? t("reviewInbox.queuedPartly", { count: outcome.count, failed: outcome.failed })
              : t("reviewInbox.queued", { count: outcome.count })}
        </p>
      )}
      <ul className="luna-review-inbox__groups">
        {inbox.groups.map((group) => (
          <li key={group.operation_id} className="luna-review-inbox__group">
            <div className="luna-review-inbox__group-head">
              <button
                type="button"
                className="luna-knowledge-recent__link"
                onClick={() => goTo(`topics://${group.topic_path}`)}
              >
                {group.topic_title}
              </button>
              <span className="luna-review-inbox__meta">
                {t(group.low_risk ? "reviewInbox.itemsLowRisk" : "reviewInbox.items", {
                  count: group.items.length,
                })}
              </span>
            </div>
            <ul className="luna-topic-evidence__items">
              {group.items.slice(0, ITEM_LIMIT).map((item, index) => (
                <li key={index} className="luna-topic-evidence__item" data-action={item.action}>
                  <span className="luna-topic-evidence__action">{t(planItemLabelKey(item))}</span>
                  <span className="luna-topic-evidence__claim">{item.text}</span>
                  {item.related_text !== null && (
                    <span className="luna-topic-evidence__related">
                      {t(
                        item.action === "replace"
                          ? "topicEvidence.replacedText"
                          : "topicEvidence.relatedText",
                        { text: item.related_text },
                      )}
                    </span>
                  )}
                  <span className="luna-topic-evidence__sources">
                    {t("topicDraft.sources", {
                      titles: item.sources
                        .map((source) => source.title)
                        .join(t("topicDraft.sourceSeparator")),
                    })}
                  </span>
                </li>
              ))}
            </ul>
            {group.items.length > ITEM_LIMIT && (
              <p className="luna-review-inbox__meta">
                {t("reviewInbox.more", { count: group.items.length - ITEM_LIMIT })}
              </p>
            )}
            {!group.input_complete && (
              <p className="luna-review-inbox__meta">{t("reviewInbox.partialInput")}</p>
            )}
            <div className="luna-topic-evidence__actions">
              <button
                type="button"
                className="luna-btn luna-btn--secondary luna-btn--sm"
                disabled={busy}
                onClick={() => void confirm([group])}
              >
                {t("reviewInbox.apply")}
              </button>
              <button
                type="button"
                className="luna-btn luna-btn--secondary luna-btn--sm"
                disabled={busy}
                onClick={() => goTo(`topics://${group.topic_path}`)}
              >
                {t("reviewInbox.open")}
              </button>
              <button
                type="button"
                className="luna-btn luna-btn--secondary luna-btn--sm"
                disabled={busy}
                onClick={() => void ignore(group)}
              >
                {t("reviewInbox.ignore")}
              </button>
            </div>
          </li>
        ))}
      </ul>
      {inbox.omitted > 0 && (
        <p className="luna-review-inbox__meta">{t("reviewInbox.omitted", { count: inbox.omitted })}</p>
      )}
    </section>
  );
}
