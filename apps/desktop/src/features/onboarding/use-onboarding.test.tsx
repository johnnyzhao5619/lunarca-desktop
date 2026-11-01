// @vitest-environment happy-dom
/**
 * 引导编排 hook 的两条契约（PRD-14-066 / ADR-114 ②）。
 *
 * ① **首帧门**：未收场时，三态没到就等于"还不知道该画哪一面"。缺了这一条，`shouldShow`
 *    为假而 `loading` 也为假，App 会落到工作区那条 return——首帧先画出工作区外壳、发一批
 *    注定被丢弃的查询，随后被向导整体替换。判据必须落在 hook 上：这件事在源码字符串断言
 *    下完全不可见。
 * ② **单一重读路径**：`markStep` / `finish` 只写，不自己重读装机状态——Rust 写完即广播
 *    `settings://changed`，订阅方按那一条重读。`open`（覆盖层重入）是唯一该主动重读的入口，
 *    正向钉住它，免得日后当成"同一类冗余"一并删掉。
 *
 * 反向验证：把 `loading` 改回 `done === null && …`，用例 ①-a 当场红；把 `refresh()` 加回
 * `markStep`，用例 ②-a 当场红。
 */

import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, renderHook, waitFor } from "@testing-library/react";

vi.mock("../../ipc/bindings", () => import("../../ipc/bindings-test-double"));

import type { SetupStatusDto } from "../../ipc/bindings";
import { commandMock, resetBindingsDouble } from "../../ipc/bindings-test-double";
import { ONBOARDING_STEP_IDS } from "./onboarding-steps";
import { useOnboarding } from "./use-onboarding";
import type { SetupStatusHandle } from "./use-setup-status";

afterEach(() => {
  cleanup();
  resetBindingsDouble();
});

function setupDto(overrides: Partial<SetupStatusDto> = {}): SetupStatusDto {
  return {
    completed: false,
    compute_location: "on_device",
    asr_ready: false,
    asr_usable: true,
    llm_ready: false,
    translate_ready: false,
    microphone: "not_determined",
    accessibility: "not_determined",
    notification: "not_determined",
    vault_bound: false,
    first_task_done: false,
    steps: ONBOARDING_STEP_IDS.map((id) => ({ id, state: "pending" as const })),
    tracked_ids: [...ONBOARDING_STEP_IDS],
    reminder_dismissed: false,
    ...overrides,
  };
}

type Props = {
  initial: { completed: boolean; completed_steps: string[] } | null;
  standalone: boolean;
  handle: SetupStatusHandle;
};

function mount(props: Props) {
  return renderHook(
    ({ initial, standalone, handle }: Props) => useOnboarding(initial, standalone, handle),
    { initialProps: props },
  );
}

const noRefresh: SetupStatusHandle["refresh"] = () => Promise.resolve(null);

describe("首帧门", () => {
  it("快照说未收场、三态还没到 → 仍算加载中，不放行工作区", async () => {
    const { result } = mount({
      initial: { completed: false, completed_steps: [] },
      standalone: false,
      handle: { setup: null, refresh: noRefresh },
    });

    await waitFor(() => {
      expect(result.current.done).toBe(false);
    });
    expect(result.current.loading).toBe(true);
    expect(result.current.shouldShow).toBe(false);
  });

  it("三态到达后才画向导", async () => {
    const props: Props = {
      initial: { completed: false, completed_steps: [] },
      standalone: false,
      handle: { setup: null, refresh: noRefresh },
    };
    const { result, rerender } = mount(props);
    rerender({ ...props, handle: { setup: setupDto(), refresh: noRefresh } });

    await waitFor(() => {
      expect(result.current.shouldShow).toBe(true);
    });
    expect(result.current.loading).toBe(false);
  });

  /** 防过度修正：已装机那条快路径不该因为这道门多等一次 `setup_status`。 */
  it("快照说已收场 → 立刻放行工作区，不等三态", () => {
    const { result } = mount({
      initial: { completed: true, completed_steps: [...ONBOARDING_STEP_IDS] },
      standalone: false,
      handle: { setup: null, refresh: noRefresh },
    });

    expect(result.current.loading).toBe(false);
    expect(result.current.shouldShow).toBe(false);
  });

  /** 快照拿不到时以 `setup_status` 为准——这条路不经过快照，故不受新门影响。 */
  it("快照失败（standalone）时按三态放行", async () => {
    const { result } = mount({
      initial: null,
      standalone: true,
      handle: { setup: setupDto(), refresh: noRefresh },
    });

    await waitFor(() => {
      expect(result.current.shouldShow).toBe(true);
    });
    expect(result.current.loading).toBe(false);
  });
});

describe("单一重读路径", () => {
  it("记到访只写不读——重读交给 settings://changed 广播", async () => {
    const refresh = vi.fn(() => Promise.resolve(null));
    const { result } = mount({
      initial: { completed: false, completed_steps: [] },
      standalone: false,
      handle: { setup: setupDto(), refresh },
    });

    await result.current.markStep("welcome");

    expect(commandMock("onboardingMarkStep")).toHaveBeenCalledWith("welcome");
    expect(refresh).not.toHaveBeenCalled();
  });

  it("收场只写不读——它连写两个键、各广播一次，再自带一读就是三读", async () => {
    const refresh = vi.fn(() => Promise.resolve(null));
    const { result } = mount({
      initial: { completed: false, completed_steps: [] },
      standalone: false,
      handle: { setup: setupDto(), refresh },
    });

    await result.current.finish();

    expect(commandMock("onboardingComplete")).toHaveBeenCalled();
    expect(refresh).not.toHaveBeenCalled();
    await waitFor(() => {
      expect(result.current.done).toBe(true);
    });
  });

  it("覆盖层重入仍主动重读（没有设置写入可搭广播的便车）", async () => {
    const refresh = vi.fn(() => Promise.resolve(null));
    const { result } = mount({
      initial: { completed: true, completed_steps: [] },
      standalone: false,
      handle: { setup: setupDto({ completed: true }), refresh },
    });

    await result.current.open("models");

    expect(refresh).toHaveBeenCalledTimes(1);
    await waitFor(() => {
      expect(result.current.current).toBe("models");
    });
  });
});
