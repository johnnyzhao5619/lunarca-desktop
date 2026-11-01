/**
 * 账户状态的共享订阅（统一登录，ADR-155）。
 *
 * 账户状态的事实源在 Rust（`luna_cloud::account`）：登录了哪台、登录到哪一步、有没有送达的
 * 链接。前端不推导，只取一次快照、再跟着 `account://changed` 换成 Rust 推来的全量状态。
 *
 * 订阅收在模块级（同 `use-compute-status`）：同一个窗口里账户页、登录组件、引导各自挂载，
 * 共用一条监听与一份快照，不会停在不同的版本上。
 */

import { useEffect, useState } from "react";

import { events } from "../ipc/bindings";
import { commands, type AccountStateDto } from "../ipc/cloud-bindings";

type Listener = (state: AccountStateDto | null) => void;

const listeners = new Set<Listener>();
let current: AccountStateDto | null = null;
let dispose: Promise<() => void> | null = null;

function publish(state: AccountStateDto) {
  current = state;
  for (const listener of listeners) listener(current);
}

/** 重新取一次快照（命令返回之后用：事件可能晚到，界面不等它）。 */
export async function refreshAccount(): Promise<void> {
  const result = await commands.accountState();
  if (result.status === "ok") publish(result.data);
}

function subscribe(listener: Listener): () => void {
  listeners.add(listener);
  if (listeners.size === 1) {
    dispose = events.accountChanged.listen((event) => publish(event.payload.state));
    void refreshAccount();
  }
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0 && dispose) {
      void dispose.then((unlisten) => unlisten());
      dispose = null;
    }
  };
}

/** 账户全量状态；`null` = 首帧还没取回。 */
export function useAccount(): AccountStateDto | null {
  const [state, setState] = useState<AccountStateDto | null>(current);
  useEffect(() => subscribe(setState), []);
  return state;
}
