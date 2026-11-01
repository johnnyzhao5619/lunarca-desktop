/**
 * 工作区底栏的同步指示投影（PRD-07-045；ADR-148、ADR-151）。
 *
 * 只消费 Rust 总览里已裁决的每库阶段与计数，按「最要用户动手的那件事」聚合成一个状态；
 * 一个库都没选中（或严格本地 / 未登录）时不占底栏——那几种状态的去处在账户页。
 */

import type { SyncOverviewDto, VaultSyncDto } from "../../ipc/bindings";
import {
  syncAwaitsConfirmation,
  syncIsReadOnly,
  syncIsSelected,
} from "../../hooks/use-sync-overview";

export type StatusBarSyncKind =
  | "hidden"
  | "awaiting_confirmation"
  | "locked"
  | "unavailable"
  | "read_only"
  | "account_frozen"
  | "offline"
  | "error"
  | "paused"
  | "large_paused_metered"
  | "large_paused_battery"
  | "syncing"
  | "conflicts"
  | "synced";

export interface StatusBarSyncProjection {
  kind: StatusBarSyncKind;
  /** 待确认的删除数 / 冲突副本数 / 已同步的库数；其余为 0。 */
  count: number;
}

const sum = (vaults: VaultSyncDto[], pick: (v: VaultSyncDto) => number) =>
  vaults.reduce((total, v) => total + pick(v), 0);

export function projectStatusBarSync(overview: SyncOverviewDto | null): StatusBarSyncProjection {
  const of = (kind: StatusBarSyncKind, count = 0) => ({ kind, count });
  if (!overview || overview.gate !== "ready") return of("hidden");
  const selected = overview.vaults.filter(syncIsSelected);
  if (selected.length === 0) return of("hidden");

  const waiting = selected.filter(syncAwaitsConfirmation);
  if (waiting.length > 0) return of("awaiting_confirmation", sum(waiting, (v) => v.held_deletions));
  if (selected.some((v) => v.phase === "locked")) return of("locked");
  if (selected.some((v) => v.phase === "unavailable")) return of("unavailable");
  if (selected.some(syncIsReadOnly)) return of("read_only");
  // 账户被冻结（纵切 3 C3）：上传被拒、下载照常——Rust 按服务端这一轮的答复判定。
  if (overview.account_frozen) return of("account_frozen");
  const errors = selected.filter((v) => v.phase === "error").map((v) => v.last_error ?? "");
  if (errors.some((code) => code === "network")) return of("offline");
  if (errors.length > 0) return of("error");
  if (overview.paused || selected.every((v) => v.phase === "paused")) return of("paused");
  // 只停了大文件（ADR-156 ③）：笔记照常，但录音不动——比「同步中」更要说出来。
  if (overview.large_paused === "metered") return of("large_paused_metered");
  if (overview.large_paused === "low_battery") return of("large_paused_battery");
  if (selected.some((v) => v.phase === "syncing")) return of("syncing");
  const conflicts = sum(selected, (v) => v.conflict_copies);
  if (conflicts > 0) return of("conflicts", conflicts);
  return of("synced", selected.length);
}
