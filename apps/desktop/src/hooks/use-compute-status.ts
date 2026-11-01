/**
 * 「AI 在哪跑」快照的共享订阅。
 *
 * `compute_status` 是一份快照，由两条广播驱动重算：设置改动（隐私档 / 优先落点）与账户
 * 变化（登录了哪台）。多个消费者（算力卡、账户页、状态栏）各自取数各自监听的结果是同一帧
 * 里多次 IPC，且它们可能停在不同的快照上。
 *
 * 订阅收在模块级：首个消费者拉一次并挂监听，最后一个卸载时撤掉。前端零业务（ADR-003）
 * ——这里只做取数与广播，落点判定全在 Rust。
 */

import { useEffect, useState } from "react";

import { commands, events, type ComputeStatusDto } from "../ipc/bindings";

type Listener = (status: ComputeStatusDto | null) => void;

const listeners = new Set<Listener>();
let current: ComputeStatusDto | null = null;
let disposers: Promise<() => void>[] = [];
/** 在飞的那一次取数。三个消费者同帧挂载时共用它，否则首屏仍然是三次 IPC。 */
let inFlight: Promise<void> | null = null;

function fetchNow(): Promise<void> {
  const request = commands
    .computeStatus()
    .then((result) => {
      if (result.status !== "ok") return;
      current = result.data;
      for (const listener of listeners) listener(current);
    })
    .finally(() => {
      if (inFlight === request) inFlight = null;
    });
  inFlight = request;
  return request;
}

/**
 * 取一次快照。
 *
 * `join = true` 允许搭上在飞的那一次（挂载与广播走这条：广播本就发生在写入之后，
 * 搭车拿到的仍是写后的事实）。**写完要立刻回读的那条路不能搭车**——一次先于本次写入
 * 发出的请求会把写前的快照当成结果回给调用方。
 */
function load(join: boolean): Promise<void> {
  return join && inFlight ? inFlight : fetchNow();
}

function subscribe(listener: Listener): () => void {
  listeners.add(listener);
  if (listeners.size === 1) {
    disposers = [
      events.settingsChanged.listen(() => void load(true)),
      events.accountChanged.listen(() => void load(true)),
    ];
  }
  void load(true);
  return () => {
    listeners.delete(listener);
    if (listeners.size > 0) return;
    for (const disposer of disposers) void disposer.then((dispose) => dispose());
    disposers = [];
  };
}

/** 当前落点快照；`null` = 还没取到（调用方据此不渲染，而不是先画一个空壳）。 */
export function useComputeStatus(): ComputeStatusDto | null {
  const [status, setStatus] = useState<ComputeStatusDto | null>(current);
  useEffect(() => subscribe(setStatus), []);
  return status;
}

/** 改动落点之后回读——等的是**写后**那一次的结果，故不搭在飞的车（见 `load`）。 */
export function refreshComputeStatus(): Promise<void> {
  return load(false);
}
