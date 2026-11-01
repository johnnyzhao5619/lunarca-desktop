/**
 * 统一处理队列的共享订阅（ADR-003 前端零业务，PRD-04-032/055）。
 *
 * 单一订阅 `processing://queue` 事件 + 启动快照。队列的排序/进度/状态机全在 Rust；前端只
 * 渲染 + 发命令（入队 / 调序 / 移除 / 清除历史）。
 *
 * **订阅收在模块级，消费者按需取值**（2026-10-15 真机卡顿）：任务运行期间进度一拍一推，
 * 此前工作区根持有整份队列，于是每一拍都重渲染整个工作区（编辑器、回放、知识库页）。
 * 现在各消费者经选择器只取自己要的那一位——顶栏入口要角标数、回放要这一篇的任务、根只要
 * 「启动时有没有活」——选出来的值没变就不渲染。首个消费者取一次快照并挂监听，最后一个
 * 卸载时撤掉并回到「还没读回来」。
 */

import { commands, events, type ProcTaskDto } from "../../ipc/bindings";
import { sameStructure, useStoreSelector, type SelectionEquality } from "../../hooks/use-store-selector";
import { isTaskActive } from "./processing-labels";

/**
 * 队列 + **快照到没到**。
 *
 * `loaded` 不是可有可无的：空数组既是"队列真的空"也是"还没读回来"，而启动概览的
 * 「有事才弹」判据要区分这两者——判早了就把"还不知道"当成"没事"，于是冷启动时
 * "有任务在跑"这个触发条件基本不会成立（PRD-07-050）。
 */
export interface ProcessingQueueState {
  items: ProcTaskDto[];
  loaded: boolean;
}

const NOT_LOADED: ProcessingQueueState = { items: [], loaded: false };

const listeners = new Set<() => void>();
let current: ProcessingQueueState = NOT_LOADED;
let disposer: Promise<() => void> | null = null;
/** 订阅世代：撤掉之后才回来的那次快照不能写进下一轮订阅。 */
let generation = 0;

function publish(next: ProcessingQueueState) {
  current = next;
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  if (listeners.size === 1) {
    generation += 1;
    const epoch = generation;
    // 事件一到就以事件为准：晚回来的那次快照不能把更新的队列盖回去。
    let heardEvent = false;
    disposer = events.processingQueue.listen((event) => {
      heardEvent = true;
      publish({ items: event.payload.items, loaded: true });
    });
    void commands.processingQueueSnapshot().then((snapshot) => {
      if (epoch === generation && !heardEvent) publish({ items: snapshot, loaded: true });
    });
  }
  return () => {
    listeners.delete(listener);
    if (listeners.size > 0) return;
    void disposer?.then((dispose) => dispose());
    disposer = null;
    generation += 1;
    current = NOT_LOADED;
  };
}

function getSnapshot(): ProcessingQueueState {
  return current;
}

/** 按需取值：选出来的值与上一次相等（默认 `Object.is`）就不重渲染。 */
export function useProcessingQueueSelector<T>(
  select: (state: ProcessingQueueState) => T,
  isEqual?: SelectionEquality<T>,
): T {
  return useStoreSelector(subscribe, getSnapshot, select, isEqual);
}

/**
 * 某一篇笔记的任务（入队序）：驱动回放视图的内联进度 / 排队 / 按钮态。
 *
 * 按内容比较：别的笔记的任务推进时，这一篇的切片内容不变，回放视图不重渲染。
 */
export function useNoteTasks(noteRel: string | null): ProcTaskDto[] {
  return useProcessingQueueSelector(
    (state) => state.items.filter((task) => task.note_rel === noteRel),
    sameStructure,
  );
}

/**
 * 此刻的队列（不订阅）：给「点下去那一刻才要读一次」的地方用，如右键菜单判定这一篇是否
 * 正在处理——为一次点击去订阅进度，等于为它每一拍都重渲染一次。
 */
export function currentProcessingQueue(): ProcTaskDto[] {
  return current.items;
}

/** 非终态（排队/运行中）项数——用于顶栏角标与 spinner 判定。 */
export function activeCount(items: ProcTaskDto[]): number {
  return items.filter(isTaskActive).length;
}
