/**
 * 统一处理队列面板（右上角弹出，PRD-04-032/055）。
 *
 * 三段（PRD-04-055 三段修订，2026-09-07）：
 * - **进行中**：运行项显进度条且**可取消**（一路传播到引擎 abort，PRD-05-010）；
 *   排队项显示位次，可上/下调序、可移除；
 * - **未成功**：失败 / 取消项，原因**完整可读**（两行截断、悬停全文），可重试、可逐条移除，
 *   段级「清除未成功」须二次确认；
 * - **已完成**：「打开」跳转对应笔记，「清除已完成」只清本段。
 *
 * 此前只有「进行中 / 已完成」两段，失败项归在「已完成」标题下、原因被 45% 宽 + `nowrap`
 * 截成「任务未能完成。请检查所…」、悬停看到的是诊断码而非人话；「清除已完成」连待重试的
 * 失败一并抹掉（真机 2026-09-07）。
 *
 * 纯展示 + 动作回调；队列状态由后端 `processing://queue` 事件驱动（ADR-003 前端零业务）。
 * Esc / 点外关闭 / 焦点归还由顶栏入口 `ProcessingQueueEntry` 在锚点上用 `useDialogBehavior` 统一给
 * （入口按钮与面板同在锚点内，点入口不算「点到外面」）；本组件只暴露根元素供初始焦点落位。
 */
import { forwardRef } from "react";
import { useTranslation } from "react-i18next";
import type { ProcTaskDto } from "../../ipc/bindings";
import {
  ArrowClockwiseIcon,
  CheckIcon,
  ChevronDownIcon,
  ChevronUpIcon,
  CloseIcon,
  TrashIcon,
} from "../../components/ui/Icon";
import { useConfirm } from "../../components/ConfirmHost";
import { IconButton } from "../../components/ui/IconButton";
import { HudActionMenu, type HudActionMenuItem } from "../../components/ui/HudActionMenu";
import { ProcessingProgress } from "./ProcessingProgress";
import {
  isTaskActive,
  isTaskDone,
  isTaskRetryable as retryable,
  isTaskUnsuccessful,
  procKindLabel,
  procServerQueueLabel,
  procServerQueueTierHint,
} from "./processing-labels";
import { taskFailureText } from "../../lib/task-failure";
import type { StatusVisitMetrics } from "../../hooks/use-status-visit-metrics";

export interface ProcessingQueuePanelProps {
  items: ProcTaskDto[];
  onReorder: (id: string, up: boolean) => void;
  /** 移除排队项或未成功项（运行中 / 已完成不可移除，由 Rust 把关）。 */
  onRemove: (id: string) => void;
  /** 中断运行中的任务（PRD-05-010）。 */
  onCancel: (id: string) => void;
  /** 重跑失败 / 已取消的任务（PRD-05-010）。 */
  onRetry: (id: string) => void;
  /** 清走「已完成」段（只清成功项）。 */
  onClearDone: () => void;
  /** 清走「未成功」段（失败 / 取消）。二次确认由本面板发起。 */
  onClearUnsuccessful: () => void;
  onSelectNote: (noteRel: string) => void;
  onClose: () => void;
  /**
   * 「现在什么在处理」这条状态路径的观测句柄（PRD-07-070 / TDD-12 §2.1）。
   *
   * **观测归持有开关的那一层，不归本面板**：到访的终点是面板关上那一刻，而面板自己
   * 看不到——它是条件渲染的，关闭即卸载。三条关闭路径（关闭钮、`setQueueOpen(false)`、
   * 跳转笔记后收起）都只在 `queueOpen` 上汇合，所以句柄由 `WorkspaceApp` 生产、传进来。
   */
  visit: StatusVisitMetrics;
}

export const ProcessingQueuePanel = forwardRef<HTMLDivElement, ProcessingQueuePanelProps>(
  function ProcessingQueuePanel(
    {
      items,
      onReorder,
      onRemove,
      onCancel,
      onRetry,
      onClearDone,
      onClearUnsuccessful,
      onSelectNote,
      onClose,
      visit,
    },
    ref,
  ) {
    const { t } = useTranslation("workspace");
    // 抓取失败的人话在 `transcribe` 命名空间（文案归功能域）。
    const { t: tTranscribe } = useTranslation("transcribe");
    const ask = useConfirm();
    const active = items.filter(isTaskActive);
    const unsuccessful = items.filter(isTaskUnsuccessful);
    const done = items.filter(isTaskDone);
    const queuedIds = active.filter((it) => it.status.state === "queued").map((it) => it.id);

    async function clearUnsuccessful() {
      // 不可撤销且清的是「还打算重试」的东西，故二次确认（与通知历史「清空」同口径）。
      const ok = await ask({
        title: t("processing.clearUnsuccessfulConfirm.title"),
        description: t("processing.clearUnsuccessfulConfirm.description"),
        confirmLabel: t("processing.clearUnsuccessful"),
        destructive: true,
      });
      if (!ok) return;
      visit.markInteracted();
      onClearUnsuccessful();
    }

    /** 任务类型胶囊 + 名称 + 执行者徽标：三段共用的行首。 */
    function identity(item: ProcTaskDto) {
      return (
        <>
          <span className="luna-proc-queue__kind">{procKindLabel(item.kind, t)}</span>
          <span className="luna-proc-queue__item-name" title={item.label}>
            {item.label}
          </span>
          {/* 谁跑的（PRD-17-033）。`null` = 本机——不给本机加徽标：
              本地是默认路径，给它标记会让「没标记」反而变得可疑。
              `leaves_deployment` 由 Rust 判定（ADR-003），前端不按
              kind 字符串自己推「未知算不算离开」。 */}
          {item.executed_by && (
            <span
              className="luna-proc-queue__executor"
              data-leaves={item.executed_by.leaves_deployment}
              title={t("processing.executor.detail", {
                name: item.executed_by.display_name,
                model: item.executed_by.model,
              })}
            >
              {t(`processing.executor.${item.executed_by.kind}`)}
            </span>
          )}
        </>
      );
    }

    /** 逐条移除（未成功项的那一枚；排队项的移除在行菜单里）。 */
    function removeButton(item: ProcTaskDto) {
      return (
        <IconButton
          density="sm"
          className="luna-proc-queue__remove"
          label={t("processing.remove")}
          icon={<TrashIcon size={13} />}
          onClick={() => onRemove(item.id)}
        />
      );
    }

    const headerMenu: HudActionMenuItem[] = [
      ...(done.length > 0
        ? [
            {
              id: "clear-done",
              label: t("processing.clearDone"),
              onSelect: () => {
                visit.markInteracted();
                onClearDone();
              },
            },
          ]
        : []),
      ...(unsuccessful.length > 0
        ? [
            {
              id: "clear-unsuccessful",
              label: t("processing.clearUnsuccessful"),
              tone: "danger" as const,
              onSelect: () => void clearUnsuccessful(),
            },
          ]
        : []),
    ];

    return (
      <div
        ref={ref}
        className="luna-proc-queue"
        role="dialog"
        aria-label={t("processing.title")}
        tabIndex={-1}
      >
        <header className="luna-proc-queue__header">
          <span className="luna-proc-queue__title">{t("processing.title")}</span>
          <div className="luna-proc-queue__header-actions">
            {/* 两枚「清除」是低频整理动作，收进「⋯」（ADR-137 H8）：此前它们与关闭并排、
                比任何一行的动作都显眼。「清除未成功」不可撤销，菜单里标危险色并仍二次确认。 */}
            {headerMenu.length > 0 ? (
              <HudActionMenu
                label={t("processing.more")}
                density="sm"
                side="bottom"
                items={headerMenu}
              />
            ) : null}
            <IconButton
              density="sm"
              label={t("processing.close")}
              icon={<CloseIcon size={14} />}
              onClick={onClose}
            />
          </div>
        </header>

        {items.length === 0 ? (
          <p className="luna-proc-queue__empty">{t("processing.empty")}</p>
        ) : (
          // 观测只挂在**内容**上：顶上那条工具条里的关闭钮不算"把面板用起来了"。
          // 两枚「清除」在工具条里但确实是操作，故各自显式记一笔。
          <div className="luna-proc-queue__body" {...visit.interactionProps}>
            {active.length > 0 && (
              <section className="luna-proc-queue__section">
                <h4 className="luna-proc-queue__section-title">{t("processing.active")}</h4>
                <ul className="luna-proc-queue__list">
                  {active.map((item) => {
                    const queuedPos = queuedIds.indexOf(item.id);
                    return (
                      <li
                        key={item.id}
                        className="luna-proc-queue__item"
                        data-state={item.status.state}
                      >
                        <div className="luna-proc-queue__item-row">
                          {identity(item)}
                          {item.status.state === "queued" && (
                            <span className="luna-proc-queue__item-status">
                              {t("processing.queuedPosition", { position: queuedPos + 1 })}
                            </span>
                          )}
                          <span className="luna-proc-queue__item-actions">
                            {item.status.state === "running" && (
                              <button
                                type="button"
                                className="luna-btn luna-btn--secondary luna-btn--sm"
                                onClick={() => onCancel(item.id)}
                              >
                                {t("processing.cancel")}
                              </button>
                            )}
                            {/* 排队项的三个动作（上移 / 下移 / 移除）收进行菜单（ADR-137 H8：每行只露
                                一个主动作）。上 / 下各一枚**不同**字形：此前两枚都是向下的 chevron，
                                上移靠 CSS 旋转 180° 区分——关闭变换或强制配色下两枚一样。 */}
                            {item.status.state === "queued" && (
                              <HudActionMenu
                                label={t("processing.reorder")}
                                density="sm"
                                side="bottom"
                                items={[
                                  {
                                    id: "up",
                                    label: t("processing.moveUp"),
                                    icon: <ChevronUpIcon size={13} />,
                                    disabled: queuedPos <= 0,
                                    onSelect: () => onReorder(item.id, true),
                                  },
                                  {
                                    id: "down",
                                    label: t("processing.moveDown"),
                                    icon: <ChevronDownIcon size={13} />,
                                    disabled: queuedPos >= queuedIds.length - 1,
                                    onSelect: () => onReorder(item.id, false),
                                  },
                                  {
                                    id: "remove",
                                    label: t("processing.remove"),
                                    icon: <TrashIcon size={13} />,
                                    tone: "danger",
                                    onSelect: () => onRemove(item.id),
                                  },
                                ]}
                              />
                            )}
                          </span>
                        </div>
                        {/* 服务端队列里的排位（ADR-049 ⑧）。位次恒在，等待区间可缺——
                            服务端还没跑过任何一件时给不出样本，那时只报位次。
                            档位的解释挂在 title 上：它回答「为什么我排在后面」。 */}
                        {procServerQueueLabel(item, t) !== null && (
                          <p
                            className="luna-proc-queue__server-queue"
                            title={procServerQueueTierHint(item, t)}
                          >
                            {procServerQueueLabel(item, t)}
                          </p>
                        )}
                        {/* 第二行只写阶段与计数：类型已在行首胶囊里，重复一遍是噪声。 */}
                        {item.status.state === "running" && (
                          <ProcessingProgress task={item} label="phase" />
                        )}
                      </li>
                    );
                  })}
                </ul>
              </section>
            )}

            {unsuccessful.length > 0 && (
              <section className="luna-proc-queue__section">
                <h4 className="luna-proc-queue__section-title">
                  {t("processing.unsuccessful")}
                </h4>
                <ul className="luna-proc-queue__list">
                  {unsuccessful.map((item) => {
                    const reason =
                      item.status.state === "failed"
                        ? taskFailureText(item.status.data.code, tTranscribe)
                        : null;
                    return (
                      <li
                        key={item.id}
                        className="luna-proc-queue__item"
                        data-state={item.status.state}
                      >
                        <div className="luna-proc-queue__item-row">
                          <span className="luna-proc-queue__item-icon">
                            <CloseIcon size={13} />
                          </span>
                          {identity(item)}
                          {item.status.state === "cancelled" && (
                            <span className="luna-proc-queue__item-status">
                              {t("processing.cancelled")}
                            </span>
                          )}
                          {/* 上次会话回填的历史（PRD-04-062）：说清它是历史，且没有重试。 */}
                          {item.restored && (
                            <span className="luna-proc-queue__item-status">
                              {t("processing.previousSession")}
                            </span>
                          )}
                          <span className="luna-proc-queue__item-actions">
                            {/* 「重试」只在**原样重跑可能成功**时出现——判据由 Rust 按错误码
                                下发（站点已下架 / 区域限制 / 登录墙 / 直播重试只会重复失败，
                                给一个注定失败的按钮比不给更糟，PRD-05-006）。取消恒可重试。 */}
                            {retryable(item) && (
                              <button
                                type="button"
                                className="luna-btn luna-btn--secondary luna-btn--sm"
                                onClick={() => onRetry(item.id)}
                              >
                                <ArrowClockwiseIcon size={13} />
                                {/* 这次失败已经计过一次费（结果取不回）：后果写在按钮上，而不是只留在
                                    可能被截断的原因里（C32）。判据由 Rust 下发。 */}
                                <span>
                                  {item.status.state === "failed" &&
                                  item.status.data.retry_charges_again
                                    ? t("processing.retryCharged")
                                    : t("processing.retry")}
                                </span>
                              </button>
                            )}
                            {removeButton(item)}
                          </span>
                        </div>
                        {/* 失败原因**完整可读**：两行截断、悬停全文；诊断码另置小字供排障，
                            不再塞进 title（那让悬停看到的是 `DISTILL_JOB: QUOTA_EXHAUSTED`）。 */}
                        {reason !== null && item.status.state === "failed" && (
                          <p className="luna-proc-queue__reason" title={reason}>
                            {reason}{" "}
                            <code className="luna-proc-queue__code">{item.status.data.code}</code>
                          </p>
                        )}
                      </li>
                    );
                  })}
                </ul>
              </section>
            )}

            {done.length > 0 && (
              <section className="luna-proc-queue__section">
                <h4 className="luna-proc-queue__section-title">{t("processing.doneSection")}</h4>
                <ul className="luna-proc-queue__list">
                  {done.map((item) => {
                    const noteRel = item.status.state === "done" ? item.status.data.note_rel : null;
                    const awaiting =
                      item.status.state === "done" ? item.status.data.awaiting_review : 0;
                    return (
                      <li
                        key={item.id}
                        className="luna-proc-queue__item"
                        data-state={item.status.state}
                      >
                        <div className="luna-proc-queue__item-row">
                          <span className="luna-proc-queue__item-icon">
                            <CheckIcon size={13} />
                          </span>
                          {identity(item)}
                          <span className="luna-proc-queue__item-status">
                            {/* 「完成」在这条队列上有两种意思：东西写好了，或者建议算好了
                                等你确认（归入的建议档）。Rust 用 `awaiting_review` 分开它们
                                ——前端只渲染，不重判（ADR-003）。真机 2026-08-31：用户连点五次
                                「立即归入」看到五条绿勾，而主题页一个字节都没动。 */}
                            {awaiting > 0
                              ? t("processing.awaitingReview", { count: awaiting })
                              : t("processing.done")}
                          </span>
                          {/* 「打开」是一枚真按钮，键盘可达；此前整行 `li` 挂 onClick，Tab 到不了。 */}
                          {noteRel !== null && (
                            <span className="luna-proc-queue__item-actions">
                              <button
                                type="button"
                                className="luna-btn luna-btn--secondary luna-btn--sm"
                                onClick={() => onSelectNote(noteRel)}
                              >
                                {t("processing.open")}
                              </button>
                            </span>
                          )}
                        </div>
                      </li>
                    );
                  })}
                </ul>
              </section>
            )}
          </div>
        )}
      </div>
    );
  },
);
