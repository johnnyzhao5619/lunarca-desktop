/**
 * 批量整理知识笔记的控制器（PRD-06-044 批量入口 / PRD-06-051 修订 / ADR-024）。
 *
 * **只管"点开始之前"那一段**：预检盘点 → 确认卡 → 批量入队。执行、进度、取消、重试、
 * 断点续跑全部由统一处理队列（PRD-04-055）承担——初版在这里自持了一套进度事件 + 快照，
 * 与队列做的是同一件事，且两套并存会让批量 runner 与队列 worker 同时各跑一件蒸馏、
 * 抢同一个本地引擎。
 *
 * 宿主常驻（WorkspaceShell），故任意目的地的文件夹右键都能唤起确认卡，不必先切到某一页。
 */

import { useCallback, useEffect, useRef, useState } from "react";
import {
  commands,
  events,
  type DistillBatchStartDto,
  type DistillPreflightDto,
} from "../../ipc/bindings";
import {
  DISTILL_BATCH_EVENT,
  DISTILL_NOTES_EVENT,
  MERGE_NOTES_EVENT,
  TAG_MERGE_EVENT,
  type DistillBatchRequestDetail,
  type DistillNotesDetail,
  type MergeNotesDetail,
  type TagMergeDetail,
} from "../../lib/app-events";
import { vaultErrorText } from "../../lib/vault-error";
import { useTaskStartGate } from "../../hooks/use-task-start-gate";

/** 确认卡状态机：关闭 / 预检中 / 预检失败 / 就绪待确认。 */
export type DistillDialogState =
  | { kind: "closed" }
  | { kind: "loading"; scope: string | null }
  | { kind: "error"; message: string; scope: string | null }
  | { kind: "ready"; preflight: DistillPreflightDto; scope: string | null; startError: string | null };

export interface DistillBatchController {
  dialog: DistillDialogState;
  openDialog: (scope?: string | null) => void;
  closeDialog: () => void;
  start: (
    scope: string | null,
    template: string | null,
    fullText: boolean | null,
  ) => Promise<void>;
  /**
   * 刚这一次批量的结果（一句反馈后即清空）；`null` = 无。
   *
   * **带着被挡下的那几篇，不只是一个计数**：只回一个 `queued` 的话，50 篇里挡下 47 篇
   * 时用户拿回的是一句「已加入 3 件」——另外 47 篇既没入队，也没有任何地方说得出为什么。
   */
  outcome: DistillBatchStartDto | null;
  dismissQueued: () => void;
  /**
   * 整批**一件都没发出去**时的原因（库没开、空选择这类命令层拒绝）；`null` = 无。
   *
   * 与 `outcome` 分开：那一栏说的是"进了几件、哪些没进"，这一条说的是"这一批压根没
   * 开始"——把后者伪装成一条逐篇拒绝，会让用户去某一篇上找一个不存在的原因。
   */
  outcomeError: string | null;
}

export function useDistillBatch(): DistillBatchController {
  const [dialog, setDialog] = useState<DistillDialogState>({ kind: "closed" });
  const [outcome, setOutcome] = useState<DistillBatchStartDto | null>(null);
  const [outcomeError, setOutcomeError] = useState<string | null>(null);
  // 服务端整理时先问一句够不够（PRD-13-040 ②）；事件订阅只建一次，故经 ref 取最新的那道门。
  const gate = useTaskStartGate();
  const gateRef = useRef(gate);
  gateRef.current = gate;

  const openDialog = useCallback((scope: string | null = null) => {
    setDialog({ kind: "loading", scope });
    void commands.distillPreflight(scope).then((result) => {
      setDialog(
        result.status === "ok"
          ? { kind: "ready", preflight: result.data, scope, startError: null }
          : { kind: "error", message: result.error, scope },
      );
    });
  }, []);

  const closeDialog = useCallback(() => setDialog({ kind: "closed" }), []);

  // 各清单的文件夹右键只派发事件，由本控制器（唯一常驻宿主）唤起确认卡。
  useEffect(() => {
    const onRequest = (event: Event) => {
      openDialog((event as CustomEvent<DistillBatchRequestDetail>).detail.scope);
    };
    window.addEventListener(DISTILL_BATCH_EVENT, onRequest);
    return () => window.removeEventListener(DISTILL_BATCH_EVENT, onRequest);
  }, [openDialog]);

  // **卡上的数字必须随执行刷新**（2026-07-30 真机第二轮的 D8）：它此前是"打开那一刻的
  // 快照"，而队列跑完那一篇之后它仍写着「1 篇待整理」——再点一次就撞上
  // `DISTILL_BATCH_EMPTY: 范围内没有需要整理的资料`，与卡上的数字自相矛盾。
  // 队列一有动静就重取预检：数字与按钮必须描述**同一时刻**的事实。
  useEffect(() => {
    const unlisten = events.processingQueue.listen(() => {
      setDialog((current) => {
        if (current.kind !== "ready") {
          return current;
        }
        void commands.distillPreflight(current.scope).then((result) => {
          if (result.status !== "ok") {
            return;
          }
          setDialog((live) =>
            live.kind === "ready" && live.scope === current.scope
              ? { ...live, preflight: result.data }
              : live,
          );
        });
        return current;
      });
    });
    return () => {
      void unlisten.then((dispose) => dispose());
    };
  }, []);

  const start = useCallback(
    async (scope: string | null, template: string | null, fullText: boolean | null) => {
      const result = await commands.distillBatchStart(scope, template, fullText);
      if (result.status === "ok") {
        // 入队是瞬时动作：关卡片；「已加入 N 件」由 Rust 发 toast（`notify_batch_queued`），
        // 其后的进度归任务面板。这里只留**有篇目被挡下**的结果——它们不是进度，是这一次
        // 动作的另一半，必须说得出是哪几篇、为什么（PRD-06-051）。
        setDialog({ kind: "closed" });
        setOutcome(result.data.rejected.length > 0 ? result.data : null);
        return;
      }
      // 启动失败留在卡上说明原因（"范围内没有需要整理的"等），关掉卡再弹一条 toast
      // 会让用户不知道刚才那个按钮到底做了什么。
      setDialog((current) =>
        current.kind === "ready" ? { ...current, startError: vaultErrorText(result.error) } : current,
      );
    },
    [],
  );

  const dismissQueued = useCallback(() => {
    setOutcome(null);
    setOutcomeError(null);
  }, []);

  // 「把这几篇整理成知识笔记」（右键单篇与多选同一条，PRD-06-044/054）：命令与**结果
  // 呈现**都归这里——派发方只说"整理这几篇"。
  useEffect(() => {
    const start = (paths: string[]) => {
      void commands.distillNotes(paths, null, null).then((result) => {
        if (result.status === "ok") {
          setOutcomeError(null);
          setOutcome(result.data.rejected.length > 0 ? result.data : null);
          return;
        }
        setOutcome(null);
        setOutcomeError(vaultErrorText(result.error));
      });
    };
    const onDistillNotes = (event: Event) => {
      const { paths } = (event as CustomEvent<DistillNotesDetail>).detail;
      // 预估不够时当场说清、一篇都不入队；够、在本机跑、问不到都照常。
      void gateRef.current({ kind: "distill_notes", note_rels: paths, full_text: null }).then((go) => {
        if (go) start(paths);
      });
    };
    window.addEventListener(DISTILL_NOTES_EVENT, onDistillNotes);
    return () => window.removeEventListener(DISTILL_NOTES_EVENT, onDistillNotes);
  }, []);

  // 「把这几条整理成一篇」/「把这个标签汇总成一篇」（PRD-06-047）：两者都只是**入队**
  // ——运行态归处理队列真值（`processing://queue`），完成由知识写入通知承接，这里
  // 与逐篇整理共用同一条「已加入 N 件」反馈。
  // 刻意**不做确认卡**：它只产出一篇新笔记、不覆盖任何东西，代价与风险都不到需要
  // 两步确认的程度——弹一张卡问"要不要整理"只是多一次点击。
  // 也不再另养一条「正在整理成一篇…」的独立状态条：那条条是 2026-09-02 真机上
  // 「处理过程中队列里看不到它」的直接来源——同一件事有两处运行态，第二处永远是漏的。
  useEffect(() => {
    const settle = (result: Awaited<ReturnType<typeof commands.mergeNotes>>) => {
      if (result.status === "ok") {
        // 「已加入队列 1 件」由 Rust 发 toast；这里没有被挡下的篇目要交代。
        setOutcomeError(null);
        setOutcome(null);
      } else {
        setOutcomeError(vaultErrorText(result.error));
      }
    };
    const onMerge = (event: Event) => {
      const { paths } = (event as CustomEvent<MergeNotesDetail>).detail;
      void commands.mergeNotes(paths).then(settle);
    };
    const onTagMerge = (event: Event) => {
      const { tag } = (event as CustomEvent<TagMergeDetail>).detail;
      void commands.distillTagNotes(tag).then(settle);
    };
    window.addEventListener(MERGE_NOTES_EVENT, onMerge);
    window.addEventListener(TAG_MERGE_EVENT, onTagMerge);
    return () => {
      window.removeEventListener(MERGE_NOTES_EVENT, onMerge);
      window.removeEventListener(TAG_MERGE_EVENT, onTagMerge);
    };
  }, []);

  return {
    dialog,
    openDialog,
    closeDialog,
    start,
    outcome,
    outcomeError,
    dismissQueued,
  };
}
