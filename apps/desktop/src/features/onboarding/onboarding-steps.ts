// 首启引导步骤的**前端投影**（PRD-14 §3 / ADR-114 ②）。
//
// 步骤顺序与三态的事实源在 Rust（`ONBOARDING_STEP_IDS` + `setup_status`）；这里只有
// ① 步骤 id 的类型（供渲染 switch 穷尽），② 在 Rust 下发的有序步骤上做"下一步 / 从哪续跑 /
// 能不能收场"三个纯投影。前端不判"这一步算不算完成"。

import type { OnboardingStepState, OnboardingStepStatusDto } from "../../ipc/bindings";

export type OnboardingStepId =
  | "welcome"
  | "vault"
  | "compute"
  | "models"
  | "permissions"
  | "shortcuts"
  | "first_task";

/**
 * 与 Rust `ONBOARDING_STEP_IDS` 逐项相同（由 `onboarding-steps.test.ts` 读 Rust 源码钉住）。
 *
 * **它是类型来源与回落，不是渲染顺序的事实源**——顺序由 `setup_status` 下发，经
 * [`orderedStepIds`] 取用。此前壳层直接按本常量渲染，而它的头注写着「只按下发的顺序
 * 渲染」：两句话有一句是假的，且假的那句不会以任何形式现形。
 */
export const ONBOARDING_STEP_IDS: readonly OnboardingStepId[] = [
  "welcome",
  "vault",
  "compute",
  "models",
  "permissions",
  "shortcuts",
  "first_task",
];

export function isOnboardingStepId(value: string): value is OnboardingStepId {
  return (ONBOARDING_STEP_IDS as readonly string[]).includes(value);
}

/** 唯一必选项：无 Vault 无法进入工作区（PRD-14 §6 已定）。 */
export const REQUIRED_STEP: OnboardingStepId = "vault";

export type StepStates = ReadonlyMap<OnboardingStepId, OnboardingStepState>;

/** Rust 下发的有序步骤 → 查表用的 Map（未知 id 丢弃：那是 Rust 新加了一步而前端还没接）。 */
export function stepStatesOf(steps: readonly OnboardingStepStatusDto[]): StepStates {
  const map = new Map<OnboardingStepId, OnboardingStepState>();
  for (const step of steps) {
    if (isOnboardingStepId(step.id)) {
      map.set(step.id, step.state);
    }
  }
  return map;
}

/**
 * Rust 下发的**有序** id（未知 id 丢弃：那是 Rust 新加了一步而前端还没接）。
 *
 * 下发为空时回落到本地常量——那只发生在状态还没到的一帧里，而那一帧壳层根本不渲染
 * 步进条（`shouldShow` 要求三态到齐）。
 */
export function orderedStepIds(
  steps: readonly OnboardingStepStatusDto[],
): readonly OnboardingStepId[] {
  const ids = steps.map((step) => step.id).filter(isOnboardingStepId);
  return ids.length > 0 ? ids : ONBOARDING_STEP_IDS;
}

/** `current` 之后第一个尚未 `done` 的步骤；已完成的不重复强制（PRD-14-060）；没有则停在最后一步。 */
export function nextStep(
  current: OnboardingStepId,
  order: readonly OnboardingStepId[],
  states: StepStates,
): OnboardingStepId {
  const index = order.indexOf(current);
  for (let i = index + 1; i < order.length; i += 1) {
    const id = order[i];
    if (states.get(id) !== "done") {
      return id;
    }
  }
  return order[order.length - 1];
}

/** 从第一个尚未 `done` 的步骤续跑；全部完成则落在最后一步（收尾）。 */
export function resumeStep(
  order: readonly OnboardingStepId[],
  states: StepStates,
): OnboardingStepId {
  return order.find((id) => states.get(id) !== "done") ?? order[order.length - 1];
}

/** 能不能收场进工作区：唯一必选项已完成即可（其余由「完成设置」横幅承接，PRD-14-064）。 */
export function canFinish(states: StepStates): boolean {
  return states.get(REQUIRED_STEP) === "done";
}
