/**
 * 新建主题页的承接对话框（PRD-16-016/017/045/046，ADR-134 证据格式）。
 *
 * **先预览、再确认**：收到 `requestTopicDraft(tag)` 后调用 Rust `prepare_topic_draft`——候选来源、
 * 证据、模型调用、封闭解析与封存全在 Rust；这里逐条展示将写入的内容与出处标题，用户按「创建
 * 主题页」时只把 operation ID 与预览指纹交回 `confirm_topic_draft`，发布的就是看到的那份字节。
 * 标签已有主题页时不另造，直接给「打开」；失效或模型输出不可用时按 Rust 的 `can_replan`
 * 给「重新检查」。
 *
 * 关闭对话框后才返回的结果会被丢弃（Rust 已封存的那份载荷一并清理），不会悄悄写入。
 *
 * 后台起草（PRD-16-060）：打开时先问 Rust `topic_draft_ready`——后台已为这个标签封存了仍然有效的
 * 初稿就直接展示，不再调用模型。那份初稿关掉对话框仍留着（下次还能直接看）；点「放弃」才清掉，
 * 且后台不会为同一批来源再备。没有现成初稿时照旧当场准备，关掉即清。
 */

import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands, type TopicDraftPlanDto } from "../../ipc/bindings";
import { useDialogBehavior } from "../../hooks/use-dialog-behavior";
import { IconButton } from "../../components/ui/IconButton";
import { CloseIcon, SparklesIcon } from "@lunarca/editor-core/Icon";
import { goTo } from "../../lib/navigate";
import { noteStem } from "../../lib/note-path";
import { TOPIC_DRAFT_REQUEST_EVENT, type TopicDraftRequestDetail } from "./topic-draft";
import { announceTopicDirectoryStale } from "./topic-events";

type Stage =
  | { kind: "preparing" }
  /** `prepared`：这份是后台已备好的初稿（关掉不清，「放弃」才清）。 */
  | { kind: "result"; plan: TopicDraftPlanDto; prepared: boolean }
  | { kind: "publishing"; plan: Extract<TopicDraftPlanDto, { status: "planned" }> }
  | { kind: "failed" };

/** 「之后的新内容先给你看、确认才写入」只讲一遍。 */
let taughtThisRun = false;

export function TopicDraftHost() {
  const [tag, setTag] = useState<string | null>(null);
  /** 自增请求序号：关掉或换了标签后，迟到的结果不落地（其封存载荷清理掉）。 */
  const requestRef = useRef(0);
  const [stage, setStage] = useState<Stage>({ kind: "preparing" });
  const [teach, setTeach] = useState<string | null>(null);

  const prepare = (target: string) => {
    const request = ++requestRef.current;
    setStage({ kind: "preparing" });
    void commands.prepareTopicDraft(target).then((result) => {
      if (request !== requestRef.current) {
        if (result.status === "ok" && result.data.status === "planned") {
          void commands.discardTopicDraft(result.data.operation_id);
        }
        return;
      }
      setStage(
        result.status === "ok"
          ? { kind: "result", plan: result.data, prepared: false }
          : { kind: "failed" },
      );
    });
  };

  /** 先看后台有没有现成的初稿；没有（或读不出）才当场准备。 */
  const open = (target: string) => {
    const request = ++requestRef.current;
    setStage({ kind: "preparing" });
    void commands.topicDraftReady(target).then((result) => {
      if (request !== requestRef.current) return;
      if (result.status === "ok" && result.data !== null) {
        setStage({ kind: "result", plan: result.data, prepared: true });
      } else {
        prepare(target);
      }
    });
  };

  useEffect(() => {
    const onRequest = (event: Event) => {
      const next = (event as CustomEvent<TopicDraftRequestDetail>).detail.tag;
      setTag(next);
      setTeach(null);
      open(next);
    };
    window.addEventListener(TOPIC_DRAFT_REQUEST_EVENT, onRequest);
    return () => window.removeEventListener(TOPIC_DRAFT_REQUEST_EVENT, onRequest);
  }, []);

  const close = () => {
    requestRef.current += 1;
    if (stage.kind === "result" && !stage.prepared && stage.plan.status === "planned") {
      void commands.discardTopicDraft(stage.plan.operation_id);
    }
    setTag(null);
  };

  const discard = () => {
    requestRef.current += 1;
    if (stage.kind === "result" && stage.plan.status === "planned") {
      void commands.discardTopicDraft(stage.plan.operation_id).then(() => {
        if (stage.prepared) announceTopicDirectoryStale();
      });
    }
    setTag(null);
  };

  const publish = (plan: Extract<TopicDraftPlanDto, { status: "planned" }>) => {
    const request = ++requestRef.current;
    setStage({ kind: "publishing", plan });
    void commands.confirmTopicDraft(plan.operation_id, plan.result_hash).then((result) => {
      if (request !== requestRef.current) return;
      if (result.status !== "ok") {
        setStage({ kind: "failed" });
      } else if (result.data.status === "published") {
        const path = result.data.path;
        goTo(`topics://${path}`);
        setTag(null);
        if (!taughtThisRun) {
          taughtThisRun = true;
          setTeach(path);
        }
      } else {
        setStage({
          kind: "result",
          plan: { status: "rejected", rejection: result.data.rejection },
          prepared: false,
        });
      }
    });
  };

  return (
    <>
      {tag !== null && (
        <TopicDraftDialog
          tag={tag}
          stage={stage}
          onClose={close}
          onDiscard={discard}
          onRetry={() => prepare(tag)}
          onPublish={publish}
        />
      )}
      {teach !== null && <TeachCard path={teach} onDismiss={() => setTeach(null)} />}
    </>
  );
}

function TopicDraftDialog({
  tag,
  stage,
  onClose,
  onDiscard,
  onRetry,
  onPublish,
}: {
  tag: string;
  stage: Stage;
  onClose: () => void;
  onDiscard: () => void;
  onRetry: () => void;
  onPublish: (plan: Extract<TopicDraftPlanDto, { status: "planned" }>) => void;
}) {
  const { t } = useTranslation("workspace");
  const dialogRef = useDialogBehavior<HTMLDivElement>({ onClose, enabled: true });
  return (
    <div className="luna-overlay" role="presentation" onClick={onClose}>
      <div
        ref={dialogRef}
        className="luna-dialog luna-topic-upgrade"
        role="dialog"
        aria-modal="true"
        aria-label={t("topicDraft.title", { tag })}
        onClick={(event) => event.stopPropagation()}
      >
        <header className="luna-topic-upgrade__header">
          <h3>{t("topicDraft.title", { tag })}</h3>
          <IconButton label={t("topicEvidence.close")} icon={<CloseIcon size={14} />} onClick={onClose} />
        </header>
        <DraftBody
          stage={stage}
          onClose={onClose}
          onDiscard={onDiscard}
          onRetry={onRetry}
          onPublish={onPublish}
        />
      </div>
    </div>
  );
}

function DraftBody({
  stage,
  onClose,
  onDiscard,
  onRetry,
  onPublish,
}: {
  stage: Stage;
  onClose: () => void;
  onDiscard: () => void;
  onRetry: () => void;
  onPublish: (plan: Extract<TopicDraftPlanDto, { status: "planned" }>) => void;
}) {
  const { t } = useTranslation("workspace");
  const closeButton = (
    <button type="button" className="luna-btn luna-btn--secondary" onClick={onClose}>
      {t("topicEvidence.close")}
    </button>
  );
  if (stage.kind === "preparing" || stage.kind === "publishing") {
    return (
      <p className="luna-topic-upgrade__hint" role="status">
        {t(stage.kind === "preparing" ? "topicDraft.preparing" : "topicDraft.publishing")}
      </p>
    );
  }
  if (stage.kind === "failed") {
    return (
      <>
        <p className="luna-topic-upgrade__hint" role="alert">
          {t("topicDraft.failed")}
        </p>
        <div className="luna-dialog__actions">{closeButton}</div>
      </>
    );
  }
  const plan = stage.plan;
  switch (plan.status) {
    case "exists":
      return (
        <>
          <p className="luna-topic-upgrade__hint">{t("topicDraft.exists")}</p>
          <div className="luna-dialog__actions">
            {closeButton}
            <button
              type="button"
              className="luna-btn luna-btn--primary"
              onClick={() => {
                goTo(`topics://${plan.path}`);
                onClose();
              }}
            >
              {t("topicDraft.openExisting")}
            </button>
          </div>
        </>
      );
    case "no_candidates":
    case "no_evidence":
    case "no_op":
      return (
        <>
          <p className="luna-topic-upgrade__hint" role="status">
            {t(`topicDraft.${plan.status}`)}
          </p>
          <div className="luna-dialog__actions">{closeButton}</div>
        </>
      );
    case "rejected":
      return (
        <>
          <p className="luna-topic-upgrade__hint" role="alert">
            {t(`topicEvidence.rejection.${plan.rejection.code}`)}
          </p>
          <div className="luna-dialog__actions">
            {closeButton}
            {plan.rejection.can_replan && (
              <button type="button" className="luna-btn luna-btn--primary" onClick={onRetry}>
                {t("topicEvidence.replan")}
              </button>
            )}
          </div>
        </>
      );
    case "planned":
      return (
        <>
          <p className="luna-topic-upgrade__hint">
            {t("topicDraft.summary", {
              title: noteStem(plan.target_path),
              count: plan.items.length,
            })}
          </p>
          {!plan.input_complete && (
            <p className="luna-topic-upgrade__hint">{t("topicEvidence.partialCoverage")}</p>
          )}
          {stage.prepared && <p className="luna-topic-upgrade__hint">{t("topicDraft.prepared")}</p>}
          <ul className="luna-topic-upgrade__list">
            {plan.items.map((item, index) => (
              <li
                key={index}
                className="luna-topic-upgrade__item"
                data-transcript={item.transcript_record || item.quick_note_record || undefined}
              >
                {(item.transcript_record || item.quick_note_record) && (
                  <span className="luna-topic-upgrade__page">
                    {t(
                      item.transcript_record
                        ? "topicEvidence.action.transcript"
                        : "topicEvidence.action.quickNote",
                    )}
                  </span>
                )}
                <span className="luna-topic-upgrade__detail">{item.text}</span>
                <span className="luna-topic-upgrade__detail">
                  {t("topicDraft.sources", {
                    titles: item.sources.map((source) => source.title).join(t("topicDraft.sourceSeparator")),
                  })}
                </span>
              </li>
            ))}
          </ul>
          <p className="luna-topic-upgrade__hint">{t("topicDraft.confirmHint")}</p>
          <div className="luna-dialog__actions">
            <button type="button" className="luna-btn luna-btn--secondary" onClick={onDiscard}>
              {t("topicDraft.discard")}
            </button>
            <button type="button" className="luna-btn luna-btn--primary" onClick={() => onPublish(plan)}>
              {t("topicDraft.create")}
            </button>
          </div>
        </>
      );
  }
}

/** 新建后的一次性说明：这一页之后怎样增长。 */
function TeachCard({ path, onDismiss }: { path: string; onDismiss: () => void }) {
  const { t } = useTranslation("workspace");
  return (
    <div className="luna-topic-draft" role="status">
      <div className="luna-topic-draft__row">
        <SparklesIcon size={14} />
        <span>{t("topic.draftDone", { title: noteStem(path) })}</span>
        <div className="luna-topic-draft__actions">
          <IconButton
            density="sm"
            label={t("topic.dismiss")}
            icon={<CloseIcon size={12} />}
            onClick={onDismiss}
          />
        </div>
      </div>
      <p className="luna-topic-draft__teach">{t("topicDraft.teach")}</p>
    </div>
  );
}
