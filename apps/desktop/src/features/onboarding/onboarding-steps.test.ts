/**
 * 首启引导步骤投影的守护（PRD-14 §3 / ADR-114 ②④）。
 *
 * 步骤表与三态的事实源在 Rust；这里钉两件事：① 前端的 id 清单与 Rust `ONBOARDING_STEP_IDS`
 * 逐项相同（读 Rust 源码比对——两边分叉的表现是某一步在指示器上"消失"，而没有任何编译
 * 错误）；② 三个纯投影（下一步 / 续跑 / 收场）按已完成状态正确跳过、落点正确。
 */

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import type { OnboardingStepStatusDto } from "../../ipc/bindings";
import {
  ONBOARDING_STEP_IDS,
  canFinish,
  nextStep,
  orderedStepIds,
  resumeStep,
  stepStatesOf,
} from "./onboarding-steps";

const RUST = readFileSync(
  resolve(__dirname, "../../../../../src-tauri/src/ipc/onboarding.rs"),
  "utf8",
);

function rustStepIds(): string[] {
  const start = RUST.indexOf("pub(crate) const ONBOARDING_STEP_IDS");
  const block = RUST.slice(start, RUST.indexOf("];", start));
  return [...block.matchAll(/"([a-z_]+)"/g)].map((m) => m[1]);
}

function states(done: string[] = [], skipped: string[] = []) {
  const steps: OnboardingStepStatusDto[] = ONBOARDING_STEP_IDS.map((id) => ({
    id,
    state: done.includes(id) ? "done" : skipped.includes(id) ? "skipped" : "pending",
  }));
  return stepStatesOf(steps);
}

describe("onboarding steps", () => {
  it("前端 id 清单与 Rust ONBOARDING_STEP_IDS 逐项相同", () => {
    expect([...ONBOARDING_STEP_IDS]).toEqual(rustStepIds());
  });

  /**
   * Vault 是唯一必选项，排第二：越早解决用户越早能「先进入应用」（ADR-114 ④）。
   * 「AI 在哪跑」仍在「模型准备」之前——顺序反过来会让用户白下一个 1.6GB 的本机模型。
   */
  it("Vault 第二、AI 在哪跑先于模型准备、第一个任务收尾", () => {
    const order = [...ONBOARDING_STEP_IDS];
    expect(order[1]).toBe("vault");
    expect(order.indexOf("compute")).toBeLessThan(order.indexOf("models"));
    expect(order[order.length - 1]).toBe("first_task");
    expect(order).not.toContain("hardware");
    expect(order).not.toContain("done");
  });

  it("下一步跳过已完成的步骤，跳过态不跳", () => {
    const order = ONBOARDING_STEP_IDS;
    expect(nextStep("welcome", order, states(["vault"]))).toBe("compute");
    expect(nextStep("welcome", order, states([], ["vault"]))).toBe("vault");
    expect(nextStep("permissions", order, states())).toBe("shortcuts");
    expect(nextStep("shortcuts", order, states())).toBe("first_task");
    expect(nextStep("first_task", order, states())).toBe("first_task");
  });

  it("续跑落在第一个未完成步骤；全部完成落在收尾", () => {
    const order = ONBOARDING_STEP_IDS;
    expect(resumeStep(order, states())).toBe("welcome");
    expect(resumeStep(order, states(["welcome", "vault"]))).toBe("compute");
    expect(resumeStep(order, states([...ONBOARDING_STEP_IDS]))).toBe("first_task");
  });

  /**
   * 顺序的事实源是 Rust 下发的 `steps`，不是前端那份常量。
   *
   * 壳层的头注一直写着「只按下发的顺序渲染」，而它此前直接按常量渲染——两句话有一句
   * 是假的，且假的那句不会以任何形式现形：Rust 调一次步序，界面照旧按老顺序画，
   * 编译不红、测试不红。
   */
  it("投影按下发的顺序走，而不是按前端常量", () => {
    const reordered = ["vault", "welcome", "compute"].map((id) => ({
      id,
      state: "pending" as const,
    }));
    const order = orderedStepIds(reordered);
    expect([...order]).toEqual(["vault", "welcome", "compute"]);
    expect(resumeStep(order, states())).toBe("vault");
    expect(nextStep("vault", order, states())).toBe("welcome");
    // 未知 id 丢弃；下发为空时才回落到常量（那一帧壳层根本不画步进条）。
    expect([...orderedStepIds([{ id: "brand_new", state: "pending" }])]).toEqual([
      ...ONBOARDING_STEP_IDS,
    ]);
  });

  it("只要 Vault 完成即可收场（其余由「完成设置」横幅承接）", () => {
    expect(canFinish(states())).toBe(false);
    expect(canFinish(states(["vault"]))).toBe(true);
    expect(canFinish(states([], ["vault"]))).toBe(false);
  });

  it("未知 id（Rust 新加了一步）不会污染投影", () => {
    const withUnknown = stepStatesOf([{ id: "brand_new", state: "pending" }]);
    expect(withUnknown.size).toBe(0);
  });
});
