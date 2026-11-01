/**
 * 顶栏处理队列入口 + 面板（PRD-04-055 / PRD-07-055）。
 *
 * **队列订阅住在这里，不在工作区根**（2026-10-15 真机卡顿）：任务运行期间进度一拍一推，
 * 此前根持有整份队列，于是每一拍都重渲染整个工作区。入口只按选择器取「在跑几条」——角标
 * 数不变就不渲染；面板开着时才取整份队列。
 *
 * **开关本身仍归宿主**：深链 `queue://`、拖入媒体都要从外面把它打开，而到访观测（`visit`）
 * 的终点是开关关上那一刻（PRD-07-070），三条关闭路径只在宿主的开关上汇合。
 */

import { useEffect, useRef, useState, type Dispatch, type SetStateAction } from "react";
import { useTranslation } from "react-i18next";
import { commands, type ProcTaskDto } from "../../ipc/bindings";
import { IconButton } from "../../components/ui/IconButton";
import { CheckIcon, QueueIcon } from "../../components/ui/Icon";
import { useDialogBehavior } from "../../hooks/use-dialog-behavior";
import type { StatusVisitMetrics } from "../../hooks/use-status-visit-metrics";
import { ProcessingQueuePanel } from "./ProcessingQueuePanel";
import { activeCount, useProcessingQueueSelector } from "./use-processing-queue";

/** 面板收着时不取整份队列：同一个空数组，进度推进时选出来的值不变。 */
const NO_ITEMS: ProcTaskDto[] = [];

export interface ProcessingQueueEntryProps {
  open: boolean;
  setOpen: Dispatch<SetStateAction<boolean>>;
  /** 到访观测句柄（由持有开关的宿主生产，见 `useStatusVisitMetrics`）。 */
  visit: StatusVisitMetrics;
  /** 面板里点「打开」：去哪儿由宿主按这篇是什么决定。 */
  onSelectNote: (noteRel: string) => void;
}

export function ProcessingQueueEntry({ open, setOpen, visit, onSelectNote }: ProcessingQueueEntryProps) {
  const { t } = useTranslation("workspace");
  const queueActive = useProcessingQueueSelector((state) => activeCount(state.items));
  const items = useProcessingQueueSelector((state) => (open ? state.items : NO_ITEMS));
  // 队列面板的出口（PRD-04-055 三段修订）：Esc / 点外关闭 / 焦点归还。挂在**锚点**
  // （入口按钮 + 面板）而不是面板上：非模态的点外关闭在 pointerdown 就触发，挂面板上
  // 的话点入口按钮会先关再被 click 重新打开——「点一次关不掉」。初始焦点落面板根。
  const panelRef = useRef<HTMLDivElement | null>(null);
  const anchorRef = useDialogBehavior<HTMLDivElement>({
    onClose: () => setOpen(false),
    enabled: open,
    modal: false,
    initialFocusRef: panelRef,
  });
  // 全部跑完那一瞬入口短暂换成勾（2 秒）：不靠角标消失来表示「好了」。
  const [justFinished, setJustFinished] = useState(false);
  const prevActiveRef = useRef(0);
  useEffect(() => {
    const prev = prevActiveRef.current;
    prevActiveRef.current = queueActive;
    if (!(prev > 0 && queueActive === 0)) return undefined;
    setJustFinished(true);
    const timer = window.setTimeout(() => setJustFinished(false), 2000);
    return () => window.clearTimeout(timer);
  }, [queueActive]);

  return (
    <div className="luna-proc-queue-anchor" ref={anchorRef}>
      {/* 入口字形是托盘（`QueueIcon`），不是旋转箭头——那枚在面板里是「重试」。
          活动中角标呼吸；全部跑完短暂换成勾。 */}
      <IconButton
        className="luna-proc-queue-btn"
        label={t("processing.title")}
        data-just-finished={justFinished}
        icon={
          <>
            {justFinished ? <CheckIcon size={15} /> : <QueueIcon size={15} />}
            {queueActive > 0 ? (
              <span className="luna-proc-queue-btn__badge" data-active="true">
                {queueActive}
              </span>
            ) : null}
          </>
        }
        aria-pressed={open}
        onClick={(event) => {
          // WKWebView 点击不给按钮焦点：先显式聚焦，关闭时焦点才有处可还。
          event.currentTarget.focus({ preventScroll: true });
          setOpen((current) => !current);
        }}
      />
      {open && (
        <ProcessingQueuePanel
          ref={panelRef}
          visit={visit}
          items={items}
          onReorder={(id, up) => void commands.processingQueueReorder(id, up)}
          onRemove={(id) => void commands.processingQueueRemove(id)}
          onCancel={(id) => void commands.processingQueueCancel(id)}
          onRetry={(id) => void commands.processingQueueRetry(id)}
          onClearDone={() => void commands.processingQueueClearDone()}
          onClearUnsuccessful={() => void commands.processingQueueClearUnsuccessful()}
          onSelectNote={onSelectNote}
          onClose={() => setOpen(false)}
        />
      )}
    </div>
  );
}
