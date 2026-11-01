// 首启引导编排 hook（PRD-14 / TDD-07 / ADR-114 ②）。
//
// 事实源全在 Rust：`setup_status` 给各步顺序与三态、各项真实条件；`onboarding.completed` 决定
// 要不要挡在工作区前面；`onboarding_mark_step` 只记「到访过」。这里做的只有：读、推进、
// 订阅变化。任何"这一步算不算完成"的判断都不在前端。
//
// `setup_status` 的订阅由 App 层的 `useSetupStatus` **唯一持有**（横幅、概览门控与本 hook
// 共用一份），本 hook 只消费它——此前各订阅一份，每次窗口聚焦就跨进程读两遍。

import { useCallback, useEffect, useRef, useState } from "react";
import { commands, type OnboardingStateDto } from "../../ipc/bindings";
import {
  isOnboardingStepId,
  nextStep,
  orderedStepIds,
  resumeStep,
  stepStatesOf,
  type OnboardingStepId,
  type StepStates,
} from "./onboarding-steps";
import type { SetupStatusHandle } from "./use-setup-status";

/**
 * @param initial 启动快照里的引导状态（`app_bootstrap`）；`null` = 快照未到。
 * @param standalone 不会有快照了（快照失败）→ 以 `setup_status` 为准。
 * @param setupStatus App 层唯一的装机状态订阅。
 */
export function useOnboarding(
  initial: OnboardingStateDto | null,
  standalone: boolean,
  setupStatus: SetupStatusHandle,
) {
  const { setup, refresh } = setupStatus;
  const [current, setCurrent] = useState<OnboardingStepId>("welcome");
  // 收场态：快照先给一个答案，`setup_status` 到达后以它为准；「开始使用」后立即置真。
  const [done, setDone] = useState<boolean | null>(null);
  // 续跑落点只在**第一次**拿到状态时定一次；之后用户自己走到哪就是哪，
  // 不能因为某个条件在后台变成 done（模型下载完了）就把他从当前页拽走。
  const resumedRef = useRef(false);

  const states: StepStates = stepStatesOf(setup?.steps ?? []);
  // 步骤顺序的事实源是 Rust 下发的 `steps`，不是前端那份常量（ADR-114 ②）。
  const order = orderedStepIds(setup?.steps ?? []);

  useEffect(() => {
    if (initial !== null) {
      setDone(initial.completed);
    }
  }, [initial]);

  useEffect(() => {
    if (setup === null) {
      return;
    }
    setDone(setup.completed);
    if (!resumedRef.current && !setup.completed) {
      setCurrent(resumeStep(orderedStepIds(setup.steps), stepStatesOf(setup.steps)));
    }
    resumedRef.current = true;
  }, [setup]);

  /**
   * 记「到访过」。**不在这里重读三态**：Rust 写完即广播 `settings://changed`，而
   * `onboarding.completed_steps` 在 `SETUP_SETTING_KEYS` 里，订阅方按那一条重读——
   * 单一重读路径，与设置控制器同一条纪律。另调一次 `refresh()` 只会让每按一次「继续」
   * 都跨进程读两遍装机状态。
   */
  const markStep = useCallback(async (step: OnboardingStepId) => {
    await commands.onboardingMarkStep(step);
  }, []);

  const goNext = useCallback(() => {
    setCurrent((cur) => nextStep(cur, order, states));
  }, [order, states]);

  const goTo = useCallback((step: string) => {
    if (isOnboardingStepId(step)) {
      setCurrent(step);
    }
  }, []);

  /** 覆盖层重入（PRD-14-060）：重读三态并落到指定步骤（缺省第一步）。 */
  const open = useCallback(
    async (step?: string) => {
      await refresh();
      setCurrent(step !== undefined && isOnboardingStepId(step) ? step : "welcome");
    },
    [refresh],
  );

  /**
   * 收场进工作区（「开始使用」与「先进入应用」同一条路）。
   *
   * 重读同样交给广播（理由见 `markStep`）：收场连写 `completed` 与
   * `setup_reminder_dismissed` 两个键、各广播一次，再自带一次 `refresh()` 就是三读。
   */
  const finish = useCallback(async () => {
    const result = await commands.onboardingComplete();
    if (result.status === "ok") {
      setDone(true);
    }
  }, []);

  // 「还不知道该画哪一面」——两种情形：① 快照未到（且不会有快照时以 setup_status 为准）；
  // ② 快照说**未收场**而三态还没到。②此前不算加载中，于是 `shouldShow` 为假、App 落到
  // 工作区那条 return：首帧先画出工作区外壳、发一批注定被丢弃的查询，随后被向导整体替换。
  // 两条命令并发发出，而快照那条注册在前，所以这不是偶发——全新安装每次都走这条路。
  const loading =
    (done === null && !(standalone && setup !== null)) || (done === false && setup === null);
  const resolvedDone = done ?? setup?.completed ?? false;
  // 收场与否在此刻必有答案（由 `loading` 保证），故不再重复判 `setup !== null`。
  const shouldShow = !loading && !resolvedDone;

  return {
    current,
    setup,
    states,
    order,
    done: resolvedDone,
    loading,
    shouldShow,
    markStep,
    goNext,
    goTo,
    open,
    finish,
    refresh,
  };
}

export type OnboardingController = ReturnType<typeof useOnboarding>;
