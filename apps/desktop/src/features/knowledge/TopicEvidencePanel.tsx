/**
 * 主题阅读视图页头下方的持续整理面板（ADR-134 / TDD-07 §4.1e）。
 *
 * **前端零业务**（ADR-003）：维护状态、候选来源、逐条内容、拒绝原因全部来自 Rust。
 * 本组件只做三件事：按 Rust 下发的维护状态决定给哪个入口、把用户的确认原样带回
 * （计划 ID + 预览指纹 / 升级前后指纹）、渲染每个阶段的结果。
 *
 * - 维护就绪（`ready`）：「检查可添加内容」→ Rust 取候选来源并生成一份**封存计划**；
 *   用户看到的逐条内容就是确认后写入的那一份（同一 operation ID + `result_hash`），
 *   确认只把应用意图写进持久队列，不在前端重算或拼 Markdown。
 * - 旧格式（`needs_upgrade`）：「升级为证据格式」→ 预览影响 → 确认 → 可撤销。
 * - 其他状态（人工改动、坏证据、未知版本……）不给入口：复核出口由页头徽标承担。
 *
 * **迟到响应不串页**：每次请求带自增序号，路径切换或新请求会让旧响应作废
 * （TDD-07 §4.1e「请求绑定选中路径」）。
 */

import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import {
  commands,
  events,
  type PlanRejectionDto,
  type SealedIngestDecisionKindDto,
  type TopicIngestPlanDto,
  type TopicMaintenanceStateDto,
  type TranscriptSourcesDto,
  type TopicUpgradePreviewDto,
  type TopicUpgradeRejectionCodeDto,
} from "../../ipc/bindings";
import { useVaultReload } from "../../hooks/use-vault-reload";
import { reportCitationOutcome } from "../../lib/app-events";
import { announceTopicDirectoryStale } from "./topic-events";
import { isTaskActive } from "../processing/processing-labels";
import { taskFailureText } from "../../lib/task-failure";
import {
  forgetApplyUndo,
  recallApplyUndo,
  rememberApplyUndo,
  type ApplyUndo,
} from "./apply-undo-memory";
import { SparklesIcon } from "@lunarca/editor-core/Icon";
import { planItemLabelKey } from "./plan-item-label";

/** 自动补充已停止、但页头徽标说不清原因的维护状态（人工改正文由徽标的复核态承担）。 */
const PAUSED_STATES: ReadonlySet<TopicMaintenanceStateDto> = new Set([
  "metadata_changed",
  "missing_baseline",
  "unknown_schema",
  "invalid_evidence",
]);

type Flow =
  | { kind: "idle" }
  | { kind: "preparing" }
  /** `operationId` 只在从盘上恢复的计划上有：过期时凭它丢弃载荷。 */
  | {
      kind: "plan";
      plan: TopicIngestPlanDto;
      operationId?: string;
      /** 这份计划上一次应用以失败结束（Rust `apply_phase: failed`）：说明后可再次确认。 */
      applyFailed?: boolean;
    }
  /** `operationId`：本次确认的计划；从盘上恢复的排队态不知道是哪一份，为空。 */
  | { kind: "queued"; operationId?: string }
  /** 排队的应用已落定且写入成功（计划载荷已清理）。`undo`：本次会话里确认的那一份，可撤销。 */
  | { kind: "applied"; undo?: ApplyUndo }
  /** 已恢复成写入前那一版。 */
  | { kind: "undone" }
  /** 没能撤销：写入后又改过，或找不到写入前那一版。 */
  | { kind: "undoBlocked"; reason: "changed" | "unavailable" }
  | { kind: "decided"; decision: SealedIngestDecisionKindDto }
  /** 过期计划被丢弃：只删了载荷、没记抑制，文案不能承诺「不会再推荐」。 */
  | { kind: "discarded" }
  | { kind: "planRejected"; rejection: PlanRejectionDto }
  | { kind: "upgradePreview"; preview: Extract<TopicUpgradePreviewDto, { status: "ready" }> }
  | { kind: "upgraded"; backupId: string; stillReview: boolean }
  | { kind: "restored" }
  | { kind: "upgradeRejected"; code: TopicUpgradeRejectionCodeDto }
  /** `error`：命令失败时带回的原始错误串，只用来取共用的成因句（如登录已失效、库已切换）；
      丢掉它，用户只能读到一句「没有完成」而不知道该做什么。 */
  | { kind: "failed"; error?: string };


export interface TopicEvidencePanelProps {
  path: string;
  /** 打开一条出处的来源笔记（分流在壳层闸口）。 */
  onOpenPath: (path: string) => void;
  /** 打开本页的版本历史（宿主持有面板）：自动补充暂停时的修复出口。 */
  onOpenHistory: () => void;
}

export function TopicEvidencePanel({ path, onOpenPath, onOpenHistory }: TopicEvidencePanelProps) {
  const { t } = useTranslation("workspace");
  const [maintenance, setMaintenance] = useState<TopicMaintenanceStateDto | null>(null);
  const [flow, setFlow] = useState<Flow>({ kind: "idle" });
  /** 录音/转写作为来源的现场（Rust 判定是否授权、有几篇可用）。 */
  const [transcripts, setTranscripts] = useState<TranscriptSourcesDto | null>(null);
  /** 正在请用户确认「允许使用录音与转写」（首次启用须说明影响，PRD-16-048）。 */
  const [grantConfirm, setGrantConfirm] = useState(false);
  /** 后台正为本页准备整理建议（Rust `background_preparing`）：不再给会重复调模型的检查按钮。 */
  const [backgroundPreparing, setBackgroundPreparing] = useState(false);
  /** 自增请求序号：只有最新一次请求的响应可以落地。 */
  const requestRef = useRef(0);
  /** 本次会话里确认排队的那一份的撤销凭据；从盘上恢复的排队态不知道是哪一份，为空。 */
  const undoRef = useRef<ApplyUndo | null>(null);

  /**
   * 读 Rust 下发的现场。
   * - `restore`（挂载时）：把仍在盘上的计划或排队中的应用接回来，离开主题去看原文再回来
   *   不必重跑模型。
   * - `settle`（「已加入队列」期间每次队列变化）：应用落定后转成「已写入」，或带失败说明
   *   接回同一份计划。
   * - `refresh`（库变化）：只刷新维护状态。
   */
  const loadStatus = useCallback(
    (mode: "restore" | "settle" | "refresh", queuedId?: string) => {
      const request = requestRef.current;
      void commands.topicCurationStatus(path).then((result) => {
        if (request !== requestRef.current) return;
        if (result.status !== "ok") {
          setMaintenance(null);
          return;
        }
        const status = result.data;
        setMaintenance(status.maintenance);
        setTranscripts(status.transcript_sources);
        setBackgroundPreparing(status.background_preparing);
        if (mode === "refresh") return;
        if (status.apply_phase === "queued") {
          if (mode === "restore") setFlow({ kind: "queued" });
          return;
        }
        // 落定时只有「刚才排队的那一份」回到待确认才算这次失败；盘上别的旧计划不冒充结果。
        const settledOther =
          mode === "settle" &&
          queuedId !== undefined &&
          status.pending_operation_id !== queuedId;
        if (status.pending && !settledOther) {
          setFlow({
            kind: "plan",
            plan: status.pending,
            operationId: status.pending_operation_id ?? undefined,
            applyFailed: status.apply_phase === "failed",
          });
        } else if (mode === "settle") {
          const undo = undoRef.current ?? undefined;
          // 记进会话：离开主题再回来仍能撤销（能不能撤由 Rust 按此刻页面判定）。
          if (undo !== undefined) rememberApplyUndo(path, undo);
          setFlow({ kind: "applied", undo });
        } else {
          const undo = recallApplyUndo(path);
          if (undo !== undefined) setFlow({ kind: "applied", undo });
        }
      });
    },
    [path],
  );

  useEffect(() => {
    requestRef.current += 1;
    setFlow({ kind: "idle" });
    setMaintenance(null);
    setBackgroundPreparing(false);
    loadStatus("restore");
  }, [loadStatus]);

  // 空闲时跟着队列：本页的后台准备开始或结束（队列里那一件的活跃态与上次现场不同）时
  // 重读现场——结束后把准备好的计划接回来，不用离开再回来。只作触发，判定仍以 Rust 为准。
  const idle = flow.kind === "idle";
  useEffect(() => {
    if (!idle) return;
    const unlisten = events.processingQueue.listen((event) => {
      const active = event.payload.items.some(
        (task) => task.kind === "topic_prepare" && task.note_rel === path && isTaskActive(task),
      );
      if (active !== backgroundPreparing) loadStatus("restore");
    });
    return () => {
      void unlisten.then((dispose) => dispose());
    };
  }, [idle, path, backgroundPreparing, loadStatus]);

  // 已加入队列后跟着队列广播落定：成功、失败都不需要用户离开再回来才知道。
  const queued = flow.kind === "queued";
  const queuedId = flow.kind === "queued" ? flow.operationId : undefined;
  useEffect(() => {
    if (!queued) return;
    const unlisten = events.processingQueue.listen(() => loadStatus("settle", queuedId));
    return () => {
      void unlisten.then((dispose) => dispose());
    };
  }, [queued, queuedId, loadStatus]);

  useVaultReload(() => loadStatus("refresh"));

  /** 开始一次新请求并返回「这次响应是否仍然有效」的判据。 */
  const begin = () => {
    requestRef.current += 1;
    const request = requestRef.current;
    return () => request === requestRef.current;
  };

  const prepare = () => {
    const current = begin();
    setFlow({ kind: "preparing" });
    void commands.prepareTopicIngestPlan(path).then((result) => {
      if (!current()) return;
      setFlow(
        result.status === "ok"
          ? { kind: "plan", plan: result.data }
          : { kind: "failed", error: result.error },
      );
    });
  };

  const confirm = (operationId: string, resultHash: string, beforeHash: string) => {
    const current = begin();
    void commands.confirmSealedIngestPlan(operationId, resultHash).then((result) => {
      if (!current()) return;
      if (result.status !== "ok") {
        setFlow({ kind: "failed", error: result.error });
      } else if (result.data.status === "queued") {
        undoRef.current = { before: beforeHash, after: resultHash };
        setFlow({ kind: "queued", operationId });
      } else {
        setFlow({ kind: "planRejected", rejection: result.data.rejection });
      }
    });
  };

  /** 撤销刚才那次写入：Rust 只在这一页仍是写入后那一版时恢复，之后改过就不动。 */
  const undoApply = (undo: ApplyUndo) => {
    const current = begin();
    void commands.undoTopicApply(path, undo.before, undo.after).then((result) => {
      if (!current()) return;
      if (result.status !== "ok") {
        setFlow({ kind: "failed", error: result.error });
      } else if (result.data.status === "restored") {
        undoRef.current = null;
        forgetApplyUndo(path);
        setFlow({ kind: "undone" });
        loadStatus("refresh");
        announceTopicDirectoryStale();
      } else {
        // 之后改过或找不到写入前那一版：这份凭据不会再成立，回来也不再给撤销。
        forgetApplyUndo(path);
        setFlow({ kind: "undoBlocked", reason: result.data.status });
      }
    });
  };

  const decide = (operationId: string, decision: SealedIngestDecisionKindDto) => {
    const current = begin();
    void commands.decideSealedIngestPlan(operationId, decision).then((result) => {
      if (!current()) return;
      if (result.status !== "ok") {
        setFlow({ kind: "failed", error: result.error });
      } else if (result.data.status === "rejected") {
        setFlow({ kind: "planRejected", rejection: result.data.rejection });
      } else if (result.data.status === "discarded") {
        // 计划已过期：Rust 只清理载荷、不记抑制，同一批来源之后仍可能被重新推荐。
        setFlow({ kind: "discarded" });
      } else {
        setFlow({ kind: "decided", decision });
        announceTopicDirectoryStale();
      }
    });
  };

  const previewUpgrade = () => {
    const current = begin();
    void commands.previewTopicUpgrade([path]).then((result) => {
      if (!current()) return;
      const preview = result.status === "ok" ? result.data[0] : undefined;
      if (preview === undefined) {
        setFlow({ kind: "failed" });
      } else if (preview.status === "ready") {
        setFlow({ kind: "upgradePreview", preview });
      } else {
        setFlow({ kind: "upgradeRejected", code: preview.code });
      }
    });
  };

  const applyUpgrade = (preview: Extract<TopicUpgradePreviewDto, { status: "ready" }>) => {
    const current = begin();
    void commands
      .applyTopicUpgrade([
        { path: preview.path, before_hash: preview.before_hash, after_hash: preview.after_hash },
      ])
      .then((result) => {
        if (!current()) return;
        const outcome = result.status === "ok" ? result.data[0] : undefined;
        if (outcome === undefined) {
          setFlow({ kind: "failed" });
        } else if (outcome.status === "upgraded") {
          setFlow({
            kind: "upgraded",
            backupId: outcome.backup_id,
            stillReview: outcome.maintenance !== "ready",
          });
        } else {
          setFlow({ kind: "upgradeRejected", code: outcome.code });
        }
      });
  };

  const restoreUpgrade = (backupId: string) => {
    const current = begin();
    void commands.restoreTopicUpgrade(backupId).then((result) => {
      if (!current()) return;
      if (result.status !== "ok") {
        setFlow({ kind: "failed", error: result.error });
      } else if (result.data.status === "restored") {
        setFlow({ kind: "restored" });
      } else {
        setFlow({ kind: "upgradeRejected", code: result.data.code });
      }
    });
  };

  const reset = () => {
    requestRef.current += 1;
    // 关掉「已写入本页」就是不打算撤销了：回来不再给撤销入口。
    if (flow.kind === "applied") forgetApplyUndo(path);
    setFlow({ kind: "idle" });
    // 回到入口前按当前文件重取维护状态：升级 / 撤销之后入口本身就该变（需升级 ↔ 可检查）。
    loadStatus("refresh");
  };

  const setTranscriptSources = (allowed: boolean) => {
    void commands.setTranscriptSources(allowed ? "allow" : "revoke").then((result) => {
      setGrantConfirm(false);
      if (result.status === "ok") {
        loadStatus("refresh");
        // 候选数随授权变化：目录重取「候选 ≥N」。
        announceTopicDirectoryStale();
      }
    });
  };

  if (flow.kind === "idle") {
    if (maintenance === "ready") {
      return (
        <>
          <div className="luna-topic-evidence" data-tone="idle">
            <SparklesIcon size={13} />
            {backgroundPreparing ? (
              <p className="luna-topic-evidence__text" role="status">
                {t("topicEvidence.backgroundPreparing")}
              </p>
            ) : (
              <>
                <p className="luna-topic-evidence__text">{t("topicEvidence.readyHint")}</p>
                <button type="button" className="luna-btn luna-btn--secondary" onClick={prepare}>
                  {t("topicEvidence.check")}
                </button>
              </>
            )}
          </div>
          {transcripts !== null && (
            <TranscriptSourcesBar
              sources={transcripts}
              confirming={grantConfirm}
              onAsk={() => setGrantConfirm(true)}
              onCancel={() => setGrantConfirm(false)}
              onSet={setTranscriptSources}
            />
          )}
        </>
      );
    }
    if (maintenance !== null && PAUSED_STATES.has(maintenance)) {
      // 自动补充停止的原因如实说出来；不给「解除保护」按钮——TDD-06 §3.9.1 禁止强写
      // 指纹。修复出口是既有的版本历史（这里直接给入口）与「转为我的笔记」（页头徽标菜单）；
      // 未知新版格式的出口是更新 Lunarca，不为旧版本造降级读取（ADR-046）。
      return (
        <div className="luna-topic-evidence" data-tone="idle" role="status">
          <p className="luna-topic-evidence__text">{t(`topicEvidence.paused.${maintenance}`)}</p>
          {maintenance !== "unknown_schema" && (
            <button type="button" className="luna-btn luna-btn--secondary" onClick={onOpenHistory}>
              {t("topicEvidence.openHistory")}
            </button>
          )}
        </div>
      );
    }
    if (maintenance === "needs_upgrade") {
      return (
        <div className="luna-topic-evidence" data-tone="idle">
          <p className="luna-topic-evidence__text">{t("topicEvidence.legacyHint")}</p>
          <button type="button" className="luna-btn luna-btn--secondary" onClick={previewUpgrade}>
            {t("topicEvidence.upgrade")}
          </button>
        </div>
      );
    }
    return null;
  }

  const dismiss = (
    <button type="button" className="luna-btn luna-btn--secondary" onClick={reset}>
      {t("topicEvidence.close")}
    </button>
  );

  switch (flow.kind) {
    case "preparing":
      return (
        <div className="luna-topic-evidence" data-tone="pending" role="status">
          <SparklesIcon size={13} />
          <p className="luna-topic-evidence__text">{t("topicEvidence.preparing")}</p>
        </div>
      );
    case "plan":
      return (
        <PlanResult
          plan={flow.plan}
          staleOperationId={flow.operationId}
          applyFailed={flow.applyFailed === true}
          dismiss={dismiss}
          onOpenPath={onOpenPath}
          onConfirm={confirm}
          onDecide={decide}
          onReplan={prepare}
        />
      );
    case "queued":
      return (
        <Notice text={t("topicEvidence.queued")} dismiss={dismiss} />
      );
    case "applied": {
      const undo = flow.undo;
      return (
        <Notice
          text={t("topicEvidence.applied")}
          dismiss={
            <>
              {undo !== undefined && (
                <button
                  type="button"
                  className="luna-btn luna-btn--secondary"
                  onClick={() => undoApply(undo)}
                >
                  {t("topicEvidence.undoApply")}
                </button>
              )}
              {dismiss}
            </>
          }
        />
      );
    }
    case "undone":
      return <Notice text={t("topicEvidence.undone")} dismiss={dismiss} />;
    case "undoBlocked":
      return (
        <Notice
          text={t(flow.reason === "changed" ? "topicEvidence.undoChanged" : "topicEvidence.undoUnavailable")}
          dismiss={
            <>
              <button type="button" className="luna-btn luna-btn--secondary" onClick={onOpenHistory}>
                {t("topicEvidence.openHistory")}
              </button>
              {dismiss}
            </>
          }
          tone="error"
        />
      );
    case "decided":
      return (
        <Notice
          text={t(flow.decision === "ignored" ? "topicEvidence.ignored" : "topicEvidence.rejectedByUser")}
          dismiss={dismiss}
        />
      );
    case "discarded":
      return <Notice text={t("topicEvidence.discarded")} dismiss={dismiss} />;
    case "planRejected":
      return (
        <Notice
          text={t(`topicEvidence.rejection.${flow.rejection.code}`)}
          dismiss={
            <>
              <ReplanButton rejection={flow.rejection} onReplan={prepare} />
              {dismiss}
            </>
          }
          tone="error"
        />
      );
    case "upgradePreview":
      return (
        <div className="luna-topic-evidence" data-tone="pending">
          <div className="luna-topic-evidence__detail">
            <p className="luna-topic-evidence__text">
              {t("topicEvidence.upgradeImpact", {
                scope: flow.preview.scope_title,
                count: flow.preview.unverified_lines,
              })}
            </p>
            {flow.preview.user_edited && (
              <p className="luna-topic-evidence__text">{t("topicEvidence.upgradeUserEdited")}</p>
            )}
          </div>
          <span className="luna-topic-evidence__actions">
            <button
              type="button"
              className="luna-btn luna-btn--primary"
              onClick={() => applyUpgrade(flow.preview)}
            >
              {t("topicEvidence.upgradeConfirm")}
            </button>
            {dismiss}
          </span>
        </div>
      );
    case "upgraded":
      return (
        <div className="luna-topic-evidence" data-tone="done" role="status">
          <p className="luna-topic-evidence__text">
            {t(flow.stillReview ? "topicEvidence.upgradedReview" : "topicEvidence.upgraded")}
          </p>
          <span className="luna-topic-evidence__actions">
            <button
              type="button"
              className="luna-btn luna-btn--secondary"
              onClick={() => restoreUpgrade(flow.backupId)}
            >
              {t("topicEvidence.upgradeUndo")}
            </button>
            {dismiss}
          </span>
        </div>
      );
    case "restored":
      return <Notice text={t("topicEvidence.upgradeRestored")} dismiss={dismiss} />;
    case "upgradeRejected":
      return (
        <Notice
          text={t(`topicEvidence.upgradeRejection.${flow.code}`)}
          dismiss={dismiss}
          tone="error"
        />
      );
    case "failed":
      return (
        <Notice
          text={
            flow.error === undefined
              ? t("topicEvidence.failed")
              : taskFailureText(flow.error, t, "workspace:topicEvidence.failed")
          }
          dismiss={dismiss}
          tone="error"
        />
      );
  }
}

/**
 * 录音/转写作为来源的授权条（PRD-16-047/048）。只在本页确有可用的权威转写时请求授权，
 * 且先说明影响再确认；已授权时给撤回。授权是本设备对本库的，不随库同步。
 */
function TranscriptSourcesBar({
  sources,
  confirming,
  onAsk,
  onCancel,
  onSet,
}: {
  sources: TranscriptSourcesDto;
  confirming: boolean;
  onAsk: () => void;
  onCancel: () => void;
  onSet: (allowed: boolean) => void;
}) {
  const { t } = useTranslation("workspace");
  if (sources.authorized) {
    return (
      <div className="luna-topic-evidence" data-tone="idle">
        <p className="luna-topic-evidence__hint">{t("topicEvidence.transcripts.enabled")}</p>
        <button type="button" className="luna-btn luna-btn--secondary luna-btn--sm" onClick={() => onSet(false)}>
          {t("topicEvidence.transcripts.revoke")}
        </button>
      </div>
    );
  }
  if (sources.available === 0) {
    return null;
  }
  if (!confirming) {
    return (
      <div className="luna-topic-evidence" data-tone="idle">
        <p className="luna-topic-evidence__text">
          {t("topicEvidence.transcripts.available", { count: sources.available })}
        </p>
        <button type="button" className="luna-btn luna-btn--secondary" onClick={onAsk}>
          {t("topicEvidence.transcripts.ask")}
        </button>
      </div>
    );
  }
  return (
    <div className="luna-topic-evidence" data-tone="pending" role="group" aria-label={t("topicEvidence.transcripts.ask")}>
      <div className="luna-topic-evidence__detail">
        <p className="luna-topic-evidence__text">{t("topicEvidence.transcripts.impact")}</p>
        <p className="luna-topic-evidence__hint">{t("topicEvidence.transcripts.scope")}</p>
      </div>
      <span className="luna-topic-evidence__actions">
        <button type="button" className="luna-btn luna-btn--primary" onClick={() => onSet(true)}>
          {t("topicEvidence.transcripts.allow")}
        </button>
        <button type="button" className="luna-btn luna-btn--secondary" onClick={onCancel}>
          {t("topicEvidence.transcripts.notNow")}
        </button>
      </span>
    </div>
  );
}

/** 「目标改变给重算」：是否值得重跑由 Rust 的 `can_replan` 判定，这里只给入口。 */
function ReplanButton({
  rejection,
  onReplan,
}: {
  rejection: PlanRejectionDto;
  onReplan: () => void;
}) {
  const { t } = useTranslation("workspace");
  if (!rejection.can_replan) {
    return null;
  }
  return (
    <button type="button" className="luna-btn luna-btn--secondary" onClick={onReplan}>
      {t("topicEvidence.replan")}
    </button>
  );
}

function Notice({
  text,
  dismiss,
  tone = "done",
}: {
  text: string;
  dismiss: ReactNode;
  tone?: "done" | "error";
}) {
  return (
    <div className="luna-topic-evidence" data-tone={tone} role={tone === "error" ? "alert" : "status"}>
      <p className="luna-topic-evidence__text">{text}</p>
      <span className="luna-topic-evidence__actions">{dismiss}</span>
    </div>
  );
}

function PlanResult({
  plan,
  staleOperationId,
  applyFailed,
  dismiss,
  onOpenPath,
  onConfirm,
  onDecide,
  onReplan,
}: {
  plan: TopicIngestPlanDto;
  /** 从盘上恢复的计划 ID；预览已过期时据此给「丢弃」。 */
  staleOperationId?: string;
  applyFailed: boolean;
  dismiss: ReactNode;
  onOpenPath: (path: string) => void;
  onConfirm: (operationId: string, resultHash: string, beforeHash: string) => void;
  onDecide: (operationId: string, decision: SealedIngestDecisionKindDto) => void;
  onReplan: () => void;
}) {
  const { t } = useTranslation("workspace");
  switch (plan.status) {
    case "no_candidates":
      return <Notice text={t("topicEvidence.noCandidates")} dismiss={dismiss} />;
    case "no_evidence":
      return <Notice text={t("topicEvidence.noEvidence")} dismiss={dismiss} />;
    case "no_op":
      return (
        <Notice
          text={
            // 还有没读到的内容时不能说「没有」：没轮到的来源先看、超长来源接着读（Rust 判定）。
            plan.more_to_read
              ? t("topicEvidence.noOpPartial")
              : !plan.input_complete
                ? t("topicEvidence.noOpClipped")
                : plan.duplicates > 0
                ? t("topicEvidence.noOpDuplicates", { count: plan.duplicates })
                : t("topicEvidence.noOp")
          }
          dismiss={dismiss}
        />
      );
    case "rejected":
      return (
        <Notice
          text={t(`topicEvidence.rejection.${plan.rejection.code}`)}
          dismiss={
            <>
              <ReplanButton rejection={plan.rejection} onReplan={onReplan} />
              {dismiss}
            </>
          }
          tone="error"
        />
      );
    case "planned":
      break;
  }
  const preview = plan.preview;
  if (preview.status === "rejected") {
    // 恢复出来的旧计划已过期：说明原因并给丢弃（只清理载荷，过期计划不记抑制）。
    const discard =
      staleOperationId === undefined ? null : (
        <button
          type="button"
          className="luna-btn luna-btn--secondary"
          onClick={() => onDecide(staleOperationId, "ignored")}
        >
          {t("topicEvidence.discardStale")}
        </button>
      );
    return (
      <Notice
        text={t(`topicEvidence.rejection.${preview.rejection.code}`)}
        dismiss={
          <>
            <ReplanButton rejection={preview.rejection} onReplan={onReplan} />
            {discard}
            {dismiss}
          </>
        }
        tone="error"
      />
    );
  }
  return (
    <section className="luna-topic-evidence" data-tone="pending" aria-label={t("topicEvidence.planTitle")}>
      <div className="luna-topic-evidence__detail">
        {applyFailed && (
          <p className="luna-topic-evidence__text" role="alert">
            {t("topicEvidence.applyFailed")}
          </p>
        )}
        <p className="luna-topic-evidence__text">
          {t("topicEvidence.planSummary", { count: preview.items.length })}
        </p>
        <ul className="luna-topic-evidence__items">
          {preview.items.map((item, index) => (
            <li
              key={index}
              className="luna-topic-evidence__item"
              data-action={item.action}
              data-transcript={item.transcript_record || item.quick_note_record || undefined}
            >
              {/* 转写记载是录音里说过的话、速记记载是还没整理的随手记录，都不是已核实的事实：
                  追加时标它的身份而不只是「新增」。 */}
              <span className="luna-topic-evidence__action">
                {t(planItemLabelKey(item))}
              </span>
              <span className="luna-topic-evidence__claim">{item.text}</span>
              {/* 替换改的是哪一句、分歧与哪一句并列：确认前把原来那条摆出来（Rust 下发）。 */}
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
                {item.sources.map((source) => (
                  <button
                    key={source.path}
                    type="button"
                    className="luna-topic-evidence__source"
                    onClick={() => {
                      // Rust 按封存的定位锚打开并定位到当初引用的那句；计划已失效时退回按路径打开。
                      void commands
                        .openSealedPlanSource(preview.operation_id, source.evidence_index)
                        .then((result) => {
                          if (result.status === "ok") {
                            reportCitationOutcome({ title: source.title, outcome: result.data });
                          } else {
                            onOpenPath(source.path);
                          }
                        });
                    }}
                  >
                    {source.title}
                  </button>
                ))}
              </span>
            </li>
          ))}
        </ul>
        {plan.omitted_sources > 0 ? (
          <p className="luna-topic-evidence__hint">
            {t("topicEvidence.partial", { count: plan.omitted_sources })}
          </p>
        ) : !plan.input_complete ? (
          <p className="luna-topic-evidence__hint">{t("topicEvidence.partialCoverage")}</p>
        ) : null}
        {plan.duplicates > 0 && (
          <p className="luna-topic-evidence__hint">
            {t("topicEvidence.duplicatesSkipped", { count: plan.duplicates })}
          </p>
        )}
        <p className="luna-topic-evidence__hint">{t("topicEvidence.untouched")}</p>
      </div>
      <span className="luna-topic-evidence__actions">
        <button
          type="button"
          className="luna-btn luna-btn--primary"
          onClick={() => onConfirm(preview.operation_id, preview.result_hash, preview.before_hash)}
        >
          {t("topicEvidence.apply")}
        </button>
        <button
          type="button"
          className="luna-btn luna-btn--secondary"
          onClick={() => onDecide(preview.operation_id, "ignored")}
        >
          {t("topicEvidence.ignore")}
        </button>
        <button
          type="button"
          className="luna-btn luna-btn--secondary"
          onClick={() => onDecide(preview.operation_id, "rejected")}
        >
          {t("topicEvidence.reject")}
        </button>
      </span>
    </section>
  );
}
