// @vitest-environment happy-dom
/**
 * 覆盖层重入（PRD-14-060）与首启是同一个向导、**不是同一种处境**。
 *
 * 重入的人已经在用应用了：最后一步的主按钮对他说「开始使用」不成立。Rust 那侧还有
 * 配套的一半——只有首次收场才把「完成设置」提醒恢复成开（重入时用户多半正是从那条横幅
 * 点进来补设置的），由 `completing_onboarding_restores_the_setup_reminder` 钉住。
 *
 * 反向验证：把标签改回无条件 `action.finish`，第一条当场红。
 */

import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import type { SetupStatusDto } from "../../ipc/bindings";
import { resetBindingsDouble, setCommandReply } from "../../ipc/bindings-test-double";
import { withI18n, t } from "../../i18n/i18n-test-fixture";
import { ONBOARDING_STEP_IDS, orderedStepIds, stepStatesOf } from "./onboarding-steps";
import { OnboardingWizard } from "./OnboardingWizard";
import type { OnboardingController } from "./use-onboarding";

afterEach(() => {
  cleanup();
  resetBindingsDouble();
});

/** Vault 已绑定 + 停在最后一步：这时主按钮才是「收场」那一颗。 */
const SETUP: SetupStatusDto = {
  completed: true,
  compute_location: "on_device",
  asr_ready: true,
  asr_usable: true,
  llm_ready: true,
  translate_ready: true,
  microphone: "granted",
  accessibility: "granted",
  notification: "unsupported",
  vault_bound: true,
  first_task_done: false,
  steps: ONBOARDING_STEP_IDS.map((id) => ({ id, state: "done" as const })),
  tracked_ids: [...ONBOARDING_STEP_IDS],
  reminder_dismissed: false,
};

function controller(): OnboardingController {
  return {
    current: "first_task",
    setup: SETUP,
    states: stepStatesOf(SETUP.steps),
    order: orderedStepIds(SETUP.steps),
    done: false,
    loading: false,
    shouldShow: true,
    markStep: vi.fn(() => Promise.resolve()),
    goNext: vi.fn(),
    goTo: vi.fn(),
    open: vi.fn(() => Promise.resolve()),
    finish: vi.fn(() => Promise.resolve()),
    refresh: vi.fn(() => Promise.resolve(null)),
  };
}

function mount(overlay: boolean) {
  setCommandReply("listSettings", { status: "ok", data: [] });
  setCommandReply("modelManagementSnapshot", {
    status: "ok",
    data: { models: [], assignments: [], storage: null, services: [], remote_compute: null },
  });
  setCommandReply("onboardingSampleAvailable", false);
  render(
    withI18n(<OnboardingWizard controller={controller()} overlay={overlay} onClose={() => {}} />),
  );
}

describe("覆盖层重入", () => {
  it("重入时最后一步说「完成」，不说「开始使用」", () => {
    mount(true);
    expect(screen.getByText(t("onboarding:action.done"))).toBeTruthy();
    expect(screen.queryByText(t("onboarding:action.finish"))).toBeNull();
  });

  it("首启仍然说「开始使用」", () => {
    mount(false);
    expect(screen.getByText(t("onboarding:action.finish"))).toBeTruthy();
    expect(screen.queryByText(t("onboarding:action.done"))).toBeNull();
  });
});
