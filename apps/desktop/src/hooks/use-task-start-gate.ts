/**
 * 「开始」之前问一句够不够（PRD-13-040 ②，纵切 3 C4）——给**没有确认卡**的入口用的最小形态：
 * 只在不够时弹一张卡说清原因与下一步，够 / 不需要 / 问不到时一声不响地放行。
 *
 * 要不要问、问到的答案全在 Rust（`task_estimate`：本机跑、未登录、严格本地一律不问、零网络）；
 * 本模块只决定**怎么说**。**问不到绝不挡路**——命令抛错也照常开始：预估是善意提醒，服务端预留才是
 * 真正的闸。不足时的修复按钮与失败卡同一取法（额度不够 → 升级或购买 / 打开账户；撞上自设上限 →
 * 调整上限）。
 */

import { useCallback } from "react";

import type { ConfirmFn } from "../components/ConfirmHost";
import { useConfirm } from "../components/ConfirmHost";
import { i18n } from "../i18n/i18n";
import { commands, type EstimateShortfallDto, type PendingTaskDto } from "../ipc/bindings";
import { goTo } from "../lib/navigate";
import { shortfallFailureCode } from "../lib/task-estimate";
import { resolveFailureGuidance } from "./use-failure-guidance";

/** 预估说不够的原因；够、不需要、问不到（含答复读不懂）一律 `null`——放行。 */
async function shortfallOf(task: PendingTaskDto): Promise<EstimateShortfallDto | null> {
  try {
    const estimate = await commands.taskEstimate(task);
    return estimate !== null && estimate.outcome.state === "insufficient" ? estimate.outcome.reason : null;
  } catch {
    return null;
  }
}

/** 能不能开始：`true` = 照常开始；`false` = 预估说不够，已经提示过（用户可能被带去修）。 */
export async function clearToStart(task: PendingTaskDto, ask: ConfirmFn): Promise<boolean> {
  const reason = await shortfallOf(task);
  if (reason === null) return true;
  const guidance = await resolveFailureGuidance(shortfallFailureCode(reason));
  const fix = await ask({
    title: i18n.t(`cloud:estimate.short.${reason}.title`),
    description: i18n.t(`cloud:estimate.short.${reason}.body`),
    confirmLabel: guidance ? i18n.t(guidance.labelKey) : undefined,
    destructive: false,
  });
  if (fix && guidance) goTo(guidance.route);
  return false;
}

/** 组件里用的同一道门（确认宿主取自上下文）。 */
export function useTaskStartGate(): (task: PendingTaskDto) => Promise<boolean> {
  const ask = useConfirm();
  return useCallback((task: PendingTaskDto) => clearToStart(task, ask), [ask]);
}
