/**
 * 官方同步总览的共享订阅（ADR-148、ADR-151；TDD-14 §5）。
 *
 * 状态栏与账户页「云同步」一节同时消费它：各自取数各自监听，两处就可能停在不同的快照上
 * ——状态栏说「已同步」，同屏账户页还写着「同步中」。订阅收在模块级：首个消费者取一次
 * 并挂监听，最后一个卸载时撤掉。
 *
 * `sync://status` 的载荷**就是**完整总览（准入门 + 全部暂停 + 每个已知库一行），故事件到了
 * 直接替换，不回读。前端零业务（ADR-003）——每个库在哪一步全由 Rust 判定。
 */

import { commands, events, type SyncOverviewDto, type VaultSyncDto } from "../ipc/bindings";
import { useStoreSelector, type SelectionEquality } from "./use-store-selector";

type Listener = () => void;

const listeners = new Set<Listener>();
let current: SyncOverviewDto | null = null;
let disposer: Promise<() => void> | null = null;
/** 事件一到就以事件为准：晚回来的那次初始取数不能把更新的快照盖回去。 */
let heardEvent = false;

function publish(overview: SyncOverviewDto) {
  current = overview;
  for (const listener of listeners) listener();
}

function subscribe(listener: Listener): () => void {
  listeners.add(listener);
  if (listeners.size === 1) {
    heardEvent = false;
    disposer = events.syncStatus.listen((event) => {
      heardEvent = true;
      publish(event.payload);
    });
    void commands.syncOverview().then((result) => {
      if (result.status === "ok" && !heardEvent) publish(result.data);
    });
  }
  return () => {
    listeners.delete(listener);
    if (listeners.size > 0) return;
    void disposer?.then((dispose) => dispose());
    disposer = null;
  };
}

function getSnapshot(): SyncOverviewDto | null {
  return current;
}

/** 当前同步总览；`null` = 还没取到（调用方据此不渲染，而不是先画一个「全部未开启」）。 */
export function useSyncOverview(): SyncOverviewDto | null {
  return useSyncOverviewSelector((overview) => overview);
}

/**
 * 只取总览里的一位，选出来的值没变就不重渲染（默认 `Object.is`）。
 *
 * 同步每一轮要发好几次总览（每库阶段、字节数、上次完成时刻都会变），而文件树只关心「当前库
 * 开没开同步」、底栏只关心聚合出来的那一个状态——拿整份总览进 state，每一轮都会把它们连同
 * 子树重渲染一遍。
 */
export function useSyncOverviewSelector<T>(
  select: (overview: SyncOverviewDto | null) => T,
  isEqual?: SelectionEquality<T>,
): T {
  return useStoreSelector(subscribe, getSnapshot, select, isEqual);
}

/**
 * 等用户确认才继续的那几种中止（ADR-148 ⑤⑥）：批量删除熔断拦下了删除，或一侧整份清单
 * 突然为空。两者共用一个确认（`sync_confirm_bulk_delete`），状态栏与账户页都据此给出口。
 */
const AWAITING_CONFIRMATION_ERRORS: ReadonlySet<string> = new Set([
  "sync_local_looks_empty",
  "sync_remote_looks_empty",
]);

/**
 * 额度用尽、只读宽限与失去同步权益都是「还能看、不能写」，对用户是同一句话（状态栏与账户页
 * 共用这一判据；退订后的倒计时另见总览的 `read_only`，PRD-13-015）。
 */
const READ_ONLY_ERRORS: ReadonlySet<string> = new Set([
  "sync_read_only",
  "sync_quota_exceeded",
  "sync_not_entitled",
]);

/**
 * 这台设备在当前账户下选中同步了这个库（开关为开）。没选中与「连在另一个账户下」都不算——后者
 * 只是显示原账户，当前账户既不同步它、也不为它发请求（ADR-151 修订 2026-10-15）。账户页、底栏与
 * 文件树共用这一判据。
 */
export function syncIsSelected(vault: VaultSyncDto): boolean {
  return vault.phase !== "off" && vault.phase !== "other_account";
}

export function syncIsReadOnly(vault: VaultSyncDto): boolean {
  return vault.phase === "error" && vault.last_error !== null && READ_ONLY_ERRORS.has(vault.last_error);
}

export function syncAwaitsConfirmation(vault: VaultSyncDto): boolean {
  return (
    vault.held_deletions > 0 ||
    (vault.last_error !== null && AWAITING_CONFIRMATION_ERRORS.has(vault.last_error))
  );
}
