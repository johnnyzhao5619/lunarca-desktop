/**
 * 启动概览的总闸（`appearance.launch_overview`，PRD-07-050）。
 *
 * 键与读法各只有一处：此前 `App.tsx` 有一个读函数、`WorkspaceApp.tsx` 另写一个字面量
 * 去写它——同一个开关两个字符串，改名时只会有一处跟着改。
 */

import type { AppBootstrapDto } from "../../ipc/bindings";

export const LAUNCH_OVERVIEW_KEY = "appearance.launch_overview";

/** 从启动快照里读（不另起一次 IPC）。快照没到 / 没有这一项时按开启处理。 */
export function launchOverviewEnabled(bootstrap: AppBootstrapDto | null): boolean {
  const value = bootstrap?.settings.find((spec) => spec.key === LAUNCH_OVERVIEW_KEY)?.current_value;
  return value?.kind === "bool" ? value.value : true;
}
