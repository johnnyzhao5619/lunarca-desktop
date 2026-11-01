/**
 * 「完成设置」横幅的投影守护（PRD-14-064）。
 *
 * 「哪些步骤算数」由 Rust 的 `tracked_ids` 给定，前端只按 `steps` 里的三态去数——
 * 故这里的夹具直接给 `tracked_ids`，不再在前端复述一份"谁该计入"的规则。
 */

import { describe, expect, it } from "vitest";
import type { SetupStatusDto } from "../../ipc/bindings";
import { projectSetupBanner } from "./use-setup-status";

const DEFAULT_TRACKED = ["vault", "models", "permissions", "first_task"];

function setup(
  states: Record<string, "pending" | "skipped" | "done">,
  overrides: Partial<SetupStatusDto> = {},
): SetupStatusDto {
  return {
    completed: true,
    compute_location: "on_device",
    asr_ready: false,
    asr_usable: true,
    llm_ready: false,
    translate_ready: false,
    microphone: "not_determined",
    accessibility: "denied",
    notification: "not_determined",
    vault_bound: true,
    first_task_done: false,
    steps: ["welcome", "vault", "compute", "models", "permissions", "first_task"].map((id) => ({
      id,
      state: states[id] ?? "pending",
    })),
    tracked_ids: DEFAULT_TRACKED,
    reminder_dismissed: false,
    ...overrides,
  };
}

describe("完成设置横幅", () => {
  it("已完成数 + 还差条数 恒等于总数——两个数字不自洽时用户只会觉得其中一个是错的", () => {
    // 分母此前只数"真得去做点什么"的四步，而步进条有七步：刚走完向导的人看到「(3/4)」，
    // 屏幕上没有任何地方能解释这个差（真机 2026-09-10）。分母改齐之后这条不变式才成立。
    for (const states of [
      {},
      { welcome: "done", vault: "done" },
      { welcome: "done", vault: "done", compute: "done", models: "skipped" },
      Object.fromEntries(DEFAULT_TRACKED.map((id) => [id, "done"])),
    ] as Parameters<typeof setup>[0][]) {
      const projection = projectSetupBanner(setup(states));
      expect(projection.done + projection.remainingIds.length).toBe(projection.total);
    }
  });

  it("只数 Rust 说算数的那些，第一个未完成的是入口", () => {
    const projection = projectSetupBanner(
      setup({ welcome: "done", vault: "done", compute: "done", models: "skipped" }),
    );
    expect(projection.total).toBe(4);
    expect(projection.done).toBe(1);
    expect(projection.remainingIds).toEqual(["models", "permissions", "first_task"]);
    expect(projection.firstRemaining).toBe("models");
  });

  it("全部完成时没有入口（横幅消失）", () => {
    const projection = projectSetupBanner(
      setup({ vault: "done", models: "done", permissions: "done", first_task: "done" }),
    );
    expect(projection.done).toBe(4);
    expect(projection.firstRemaining).toBeNull();
  });

  it("Rust 把某一步移出计数后前端立刻跟着变（权限全被拒的情形）", () => {
    const projection = projectSetupBanner(
      setup({ vault: "done" }, { tracked_ids: ["vault", "models", "first_task"] }),
    );
    expect(projection.total).toBe(3);
    expect(projection.remainingIds).toEqual(["models", "first_task"]);
  });

  it("关掉提醒后横幅与「有事才弹」一起失效——两者读同一个投影", () => {
    const projection = projectSetupBanner(setup({}, { reminder_dismissed: true }));
    expect(projection.firstRemaining).toBeNull();
    expect(projection.total).toBe(0);
  });
});
