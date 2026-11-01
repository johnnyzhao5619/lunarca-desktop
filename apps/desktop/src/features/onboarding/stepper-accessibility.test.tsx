// @vitest-environment happy-dom
/**
 * 步进条的两条可访问性契约。
 *
 * ① **当前步可聚焦**：它这颗按钮确实不该有动作，但用 `disabled` 表达会让它不可聚焦，
 *    于是键盘与读屏用户走过步进条时，唯一读不到的恰好是「我在哪一步」。禁用的是动作，
 *    不是存在——故 `aria-disabled` + onClick 空转。
 * ② **窄窗下仍有进度信息**：紧凑投影与那组带标签的按钮由 `display:none` 互斥，同一时刻
 *    只有一个在无障碍树里。此前两者同时被拿掉（按钮 `display:none` + 紧凑条 `aria-hidden`），
 *    于是窄窗下读屏用户什么都拿不到。
 *
 * 反向验证：把 `disabled={isCurrent}` 加回去，①当场红；把 `aria-hidden` 加回紧凑条，②当场红。
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

const SETUP: SetupStatusDto = {
  completed: false,
  compute_location: "on_device",
  asr_ready: true,
  asr_usable: true,
  llm_ready: true,
  translate_ready: true,
  microphone: "granted",
  accessibility: "granted",
  notification: "unsupported",
  vault_bound: false,
  first_task_done: false,
  steps: ONBOARDING_STEP_IDS.map((id) => ({ id, state: "pending" as const })),
  tracked_ids: [...ONBOARDING_STEP_IDS],
  reminder_dismissed: false,
};

function controller(goTo = vi.fn()): OnboardingController {
  return {
    current: "welcome",
    setup: SETUP,
    states: stepStatesOf(SETUP.steps),
    order: orderedStepIds(SETUP.steps),
    done: false,
    loading: false,
    shouldShow: true,
    markStep: vi.fn(() => Promise.resolve()),
    goNext: vi.fn(),
    goTo,
    open: vi.fn(() => Promise.resolve()),
    finish: vi.fn(() => Promise.resolve()),
    refresh: vi.fn(() => Promise.resolve(null)),
  };
}

function mount(goTo = vi.fn()) {
  setCommandReply("listSettings", { status: "ok", data: [] });
  setCommandReply("modelManagementSnapshot", {
    status: "ok",
    data: { models: [], assignments: [], storage: null, services: [], remote_compute: null },
  });
  setCommandReply("hardwareSummary", { status: "error", error: "not needed" });
  render(withI18n(<OnboardingWizard controller={controller(goTo)} />));
}

describe("步进条可访问性", () => {
  it("当前步可聚焦——禁用的是动作，不是存在", () => {
    mount();
    const current = screen.getByRole("button", { current: "step" });

    expect(current.hasAttribute("disabled")).toBe(false);
    expect(current.getAttribute("aria-disabled")).toBe("true");
    // 七步一个不少地留在无障碍树里。
    const steps = screen
      .getAllByRole("button")
      .filter((b) => b.className.includes("luna-onboarding__stepper-item"));
    expect(steps).toHaveLength(ONBOARDING_STEP_IDS.length);
    expect(steps.every((b) => !b.hasAttribute("disabled"))).toBe(true);
  });

  it("窄窗下的紧凑进度不对读屏隐藏（与按钮组互斥，同时只有一个在树里）", () => {
    mount();
    const compact = screen.getByText(
      t("onboarding:progress_compact", { current: 1, total: ONBOARDING_STEP_IDS.length }),
    );
    expect(compact.closest("[aria-hidden='true']")).toBeNull();
  });
});
