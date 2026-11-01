// @vitest-environment happy-dom
/**
 * 「转写一段示例」——PRD-14-051 的替代成功路径。
 *
 * 它补的是一条真实的死胡同：拒绝了麦克风、又没有本机语言模型的用户，语音输入与弹窗翻译
 * 两张卡同时是灰的，而「第一个任务」是激活漏斗的最后一环。判据落在渲染上：那张卡在不在、
 * 点下去调的是不是**导入那条既有路径**、以及随包示例不在时它必须整张消失（不留一颗
 * 点了必然失败的键——这一步的头注为此专门写过一段）。
 *
 * 反向验证：把 `sampleAvailable` 恒设为 true，「示例不在时不出这张卡」当场红；
 * 把按钮改成不调命令，「点下去走导入路径」当场红。
 */

import { afterEach, describe, expect, it } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { vi } from "vitest";

vi.mock("../../../ipc/bindings", () => import("../../../ipc/bindings-test-double"));

import type { SetupStatusDto } from "../../../ipc/bindings";
import {
  commandMock,
  resetBindingsDouble,
  setCommandReply,
} from "../../../ipc/bindings-test-double";
import { withI18n, t } from "../../../i18n/i18n-test-fixture";
import { ONBOARDING_STEP_IDS } from "../onboarding-steps";
import { FirstTaskStep } from "./FirstTaskStep";

afterEach(() => {
  cleanup();
  resetBindingsDouble();
});

/** 拒绝了麦克风、没有本机语言模型——正是这条路要救的那台机器。 */
const SETUP: SetupStatusDto = {
  completed: false,
  compute_location: "on_device",
  asr_ready: true,
  asr_usable: true,
  llm_ready: false,
  translate_ready: false,
  microphone: "denied",
  accessibility: "denied",
  notification: "unsupported",
  vault_bound: true,
  first_task_done: false,
  steps: ONBOARDING_STEP_IDS.map((id) => ({ id, state: "pending" as const })),
  tracked_ids: [...ONBOARDING_STEP_IDS],
  reminder_dismissed: false,
};

function mount() {
  // `useSettings` 读全量设置；这条用例不关心快捷键，给一份空清单即可。
  setCommandReply("listSettings", { status: "ok", data: [] });
  render(withI18n(<FirstTaskStep setup={SETUP} onRefresh={() => {}} onGoToStep={() => {}} />));
}

describe("第一个任务的第三条路", () => {
  it("随包示例在场时给出那张卡，并带上 CC BY 署名", async () => {
    setCommandReply("onboardingSampleAvailable", true);
    mount();

    await waitFor(() => {
      expect(screen.getByText(t("onboarding:step.first_task.clip_title"))).toBeTruthy();
    });
    // 署名必须在界面上：包里那份 ATTRIBUTION.md 用户永远看不到。
    expect(screen.getByText(t("onboarding:step.first_task.clip_credit"))).toBeTruthy();
  });

  it("点下去走的是既有的导入路径，并就地说明已入队", async () => {
    setCommandReply("onboardingSampleAvailable", true);
    setCommandReply("onboardingTranscribeSample", { status: "ok", data: 1 });
    mount();

    await waitFor(() => {
      expect(screen.getByText(t("onboarding:step.first_task.clip_try"))).toBeTruthy();
    });
    fireEvent.click(screen.getByText(t("onboarding:step.first_task.clip_try")));

    await waitFor(() => {
      expect(commandMock("onboardingTranscribeSample")).toHaveBeenCalled();
    });
    await waitFor(() => {
      expect(screen.getByText(t("onboarding:step.first_task.clip_queued"))).toBeTruthy();
    });
  });

  /**
   * 「推荐档没装」≠「转不了」。随包档落地后这两件事就分家了：高配机器的推荐档是
   * Large v3 Turbo，没装即 `asr_ready=false`，而随包 tiny 一直在。用 `asr_ready` 去判的话，
   * 模型页刚说完「内置模型已就绪，零下载即可开始转写」，这一屏就说「转写模型还没准备好」
   * ——两屏对同一件事说相反的话，而功能上它明明跑得起来（真机 2026-09-11）。
   */
  it("推荐档没装但随包档在场时，示例卡照样能点", async () => {
    setCommandReply("onboardingSampleAvailable", true);
    setCommandReply("listSettings", { status: "ok", data: [] });
    render(
      withI18n(
        <FirstTaskStep
          setup={{ ...SETUP, asr_ready: false, asr_usable: true }}
          onRefresh={() => {}}
          onGoToStep={() => {}}
        />,
      ),
    );

    await waitFor(() => {
      expect(screen.getByText(t("onboarding:step.first_task.clip_try"))).toBeTruthy();
    });
    expect(screen.queryByText(t("onboarding:step.first_task.clip_not_ready"))).toBeNull();
  });

  /**
   * 「没有对话模型」≠「翻不了」：只装了专用翻译模型（Hy-MT2）时，翻译走「翻译与实时档」
   * 照常可用。这张卡若问 `llm_ready`，就会把用户引去下 2.5 GB 的对话模型。
   */
  it("只装了专用翻译模型时，弹窗翻译照样能点", async () => {
    setCommandReply("onboardingSampleAvailable", false);
    setCommandReply("listSettings", { status: "ok", data: [] });
    render(
      withI18n(
        <FirstTaskStep
          setup={{ ...SETUP, llm_ready: false, translate_ready: true }}
          onRefresh={() => {}}
          onGoToStep={() => {}}
        />,
      ),
    );

    await waitFor(() => {
      expect(screen.getByText(t("onboarding:step.first_task.translate_try"))).toBeTruthy();
    });
    expect(screen.queryByText(t("onboarding:step.first_task.translate_not_ready"))).toBeNull();
  });

  it("随包示例不在时整张卡消失，不留一颗必然失败的键", async () => {
    setCommandReply("onboardingSampleAvailable", false);
    mount();

    // 等另外两张卡画完，确认这一帧不是"还没渲染"。
    await waitFor(() => {
      expect(screen.getByText(t("onboarding:step.first_task.voice_title"))).toBeTruthy();
    });
    expect(screen.queryByText(t("onboarding:step.first_task.clip_title"))).toBeNull();
  });
});
