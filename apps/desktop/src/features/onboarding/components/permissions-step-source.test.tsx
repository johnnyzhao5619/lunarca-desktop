// @vitest-environment happy-dom
/**
 * 权限步的状态**只有一个来源**：壳层已有的 `setup_status`（PRD-14-063）。
 *
 * 此前这一步另查一遍 `check_permission × 3` 并自挂 focus 监听，而 `useSetupStatus` 挂着
 * 同一个 focus 监听——窗口每聚焦一次就是六次系统权限查询，同一件事问两遍还可能给出两个
 * 答案（两条查询之间用户正好在系统设置里改了）。
 *
 * 判据落在"渲染完一次，`check_permission` 一次都没被调用"上：这是数据源收口的可观察面，
 * 而源码字符串断言证明不了它。反向验证：把组件内的 `checkPermission` 加回去即红。
 */

import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

vi.mock("../../../ipc/bindings", () => import("../../../ipc/bindings-test-double"));

import type { SetupStatusDto } from "../../../ipc/bindings";
import { commandMock, resetBindingsDouble } from "../../../ipc/bindings-test-double";
import { withI18n, t } from "../../../i18n/i18n-test-fixture";
import { ONBOARDING_STEP_IDS } from "../onboarding-steps";
import { PermissionsStep } from "./PermissionsStep";

afterEach(() => {
  cleanup();
  resetBindingsDouble();
});

const SETUP: SetupStatusDto = {
  completed: false,
  compute_location: "on_device",
  asr_ready: false,
  asr_usable: true,
  llm_ready: false,
  translate_ready: false,
  microphone: "granted",
  accessibility: "denied",
  notification: "unsupported",
  vault_bound: true,
  first_task_done: false,
  steps: ONBOARDING_STEP_IDS.map((id) => ({ id, state: "pending" as const })),
  tracked_ids: [...ONBOARDING_STEP_IDS],
  reminder_dismissed: false,
};

describe("权限步的数据源", () => {
  it("三行状态全部来自 setup_status，不再自查 check_permission", () => {
    render(withI18n(<PermissionsStep setup={SETUP} onRefresh={() => {}} />));

    expect(screen.getByText(t("onboarding:step.permissions.status.granted"))).toBeTruthy();
    expect(screen.getByText(t("onboarding:step.permissions.status.denied"))).toBeTruthy();
    expect(screen.getByText(t("onboarding:step.permissions.status.unsupported"))).toBeTruthy();
    expect(commandMock("checkPermission")).not.toHaveBeenCalled();
  });

  it("「重新检测」重读的是同一份装机状态，不另起一条查询", () => {
    const onRefresh = vi.fn();
    render(withI18n(<PermissionsStep setup={SETUP} onRefresh={onRefresh} />));

    fireEvent.click(screen.getByText(t("onboarding:action.recheck")));

    expect(onRefresh).toHaveBeenCalledTimes(1);
    expect(commandMock("checkPermission")).not.toHaveBeenCalled();
  });

  /**
   * 一项都查不到的平台（Windows 上三项皆是）：三行「系统不提供查询」+ 三颗灰按钮读起来
   * 像应用坏了，而实情是这里没有任何需要用户做的事。收成一句陈述。
   */
  it("平台一项都查不到时收成一句话，不摆三行灰按钮", () => {
    const windowsLike: SetupStatusDto = {
      ...SETUP,
      microphone: "unsupported",
      accessibility: "unsupported",
      notification: "unsupported",
    };
    render(withI18n(<PermissionsStep setup={windowsLike} onRefresh={() => {}} />));

    expect(screen.getByText(t("onboarding:step.permissions.none_required"))).toBeTruthy();
    expect(screen.queryByText(t("onboarding:step.permissions.grant"))).toBeNull();
    expect(screen.queryAllByText(t("onboarding:step.permissions.status.unsupported"))).toHaveLength(0);
  });

  /** 状态未到时按「未询问」渲染，而不是空白——三行的位置先占住，值随后到。 */
  it("装机状态未到时不崩、按未询问渲染", () => {
    render(withI18n(<PermissionsStep setup={null} onRefresh={() => {}} />));

    expect(screen.getAllByText(t("onboarding:step.permissions.status.not_determined")).length).toBe(3);
  });
});
