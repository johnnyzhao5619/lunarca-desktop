import { describe, expect, it } from "vitest";

import { t } from "../i18n/i18n-test-fixture";
import type { UsageEstimateDto } from "../ipc/bindings";
import { estimateLine, shortfallFailureCode } from "./task-estimate";

/**
 * 任务前预估的那一句话（纵切 3 C4，PRD-13-040 ②）：分钟 / credits 由 Rust 换算好，这里只拼句；
 * 每一句都带「约」，最终以服务端计量为准。
 */
describe("预估的那一句话", () => {
  it("转写：约消耗多少分钟，从预付扣时说约多少 credits", () => {
    const estimate: UsageEstimateDto = {
      amounts: [{ unit: "minutes", minutes: 61 }],
      outcome: { state: "prepaid", credits: 2.5 },
    };
    const line = estimateLine(estimate, t, "zh-CN");
    expect(line).toBe(
      t("cloud:estimate.line", {
        consumption: t("cloud:estimate.consumption", {
          amounts: t("cloud:estimate.amount_minutes", { minutes: "61" }),
        }),
        ledger: t("cloud:estimate.from_prepaid", { credits: "2.5" }),
      }),
    );
    expect(line).toContain("约");
  });

  it("文本生成：逐维列出（维度名走用量那套文案），订阅够时说从订阅扣", () => {
    const line = estimateLine(
      {
        amounts: [
          { unit: "dimension", dimension: "tokens_in", amount: 12000 },
          { unit: "dimension", dimension: "tokens_out", amount: 3000 },
        ],
        outcome: { state: "subscription" },
      },
      t,
      "zh-CN",
    );
    expect(line).toContain(t("cloud:usage.dimension.tokens_in"));
    expect(line).toContain(t("cloud:usage.dimension.tokens_out"));
    expect(line).toContain("12,000");
    expect(line).toContain(t("cloud:estimate.from_subscription"));
  });

  it("不够或服务端没列出消耗：这一句不说（不足另有提示）", () => {
    expect(
      estimateLine(
        { amounts: [{ unit: "minutes", minutes: 3 }], outcome: { state: "insufficient", reason: "quota" } },
        t,
        "zh-CN",
      ),
    ).toBeNull();
    expect(estimateLine({ amounts: [], outcome: { state: "subscription" } }, t, "zh-CN")).toBeNull();
  });

  it("不足的原因 → 失败码：修复按钮与失败卡同一张表", () => {
    expect(shortfallFailureCode("quota")).toBe("QUOTA_EXHAUSTED");
    expect(shortfallFailureCode("spend_cap")).toBe("SPEND_CAP_REACHED");
  });
});
