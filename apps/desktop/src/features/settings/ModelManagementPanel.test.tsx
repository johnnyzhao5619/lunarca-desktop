import { describe, expect, it } from "vitest";

import { pricingOpen, targetsUsage } from "./UsageExecutionPanel";
import {
  effectiveLabelKey,
  viewForRouteTarget,
  viewsFor,
} from "./ModelManagementPanel";

describe("模型管理渐进披露", () => {
  it("标准视图展示本机模型与运行位置", () => {
    expect(viewsFor()).toEqual(["local", "services"]);
  });

  it("完整设置也不在本地模型页复制分配入口", () => {
    expect(viewsFor()).toEqual(["local", "services"]);
  });

  /**
   * 「执行记录」不是模型库的页签（ADR-122 ⑥）：它回答「跑了什么」，与「装了哪些模型」
   * 不是一件事。它是 AI 页上的独立面板，两档投影都在——藏起来等于把这条状态路径
   * 对普通用户删掉，而它原先所在的「动态」页已经退役。
   */
  it("执行记录不在模型库的页签里", () => {
    for (const mode of ["standard", "advanced"]) {
      expect(viewsFor(), mode).not.toContain("usage");
    }
  });

  /**
   * **深链指名要去某个分配槽时，那一页签必须在**（ADR-126 第 5 条）。
   *
   * 算力卡三格的「更改」落到的正是 `ai.assignment.<slot>`，而标准投影下这个页签不存在
   * ——此前那两颗按钮打开 sheet 后停在「本机模型」，用户要改的那一项根本不在屏幕上，
   * 表现就是「点了打开一个找不到目标的浮层」（真机 2026-09-12 用户圈点）。
   */
  it("标准投影不因深链改变可见页签", () => {
    const standard = viewsFor();
    expect(standard).not.toContain("assignments");
    expect(viewForRouteTarget("ai.assignment.default_llm", standard)).toBeNull();
  });
});

/**
 * `settings://ai_services/usage` 是用量的**唯一**深链（PRD-07-056）。
 *
 * 判定收在一处并 fail closed：未知尾段返回 `null`，由调用方保持当前页签而不是停在空白页。
 */
describe("viewForRouteTarget（settings://local_models/<section> 落位）", () => {
  const standard = viewsFor();
  const advanced = viewsFor();

  /** 两块各认各的尾段：`usage` 是执行记录那张卡，不是模型库的页签。 */
  it("usage 尾段属于执行记录，不切模型库的页签", () => {
    expect(viewForRouteTarget("usage", advanced)).toBeNull();
    expect(viewForRouteTarget("usage", standard)).toBeNull();
    expect(targetsUsage("usage")).toBe(true);
  });

  /** 反过来也要成立：模型库的尾段不属于执行记录。 */
  it("模型库的尾段不属于执行记录", () => {
    for (const target of ["local", "services", "assignments", "ai.assignment.default_llm"]) {
      expect(targetsUsage(target), target).toBe(false);
    }
    expect(targetsUsage(null)).toBe(false);
  });

  /**
   * 两个单价键也归执行记录那张卡，且**必须把那一节展开**——工作台没配单价时给的正是
   * 这条深链，而它承载的输入框默认收在折叠里（ADR-126 第 6/7 条）。
   */
  it("单价键落到执行记录，并展开它承载的那一节", () => {
    for (const key of ["provider.asr_cost_per_min", "provider.llm_cost_per_million_tokens"]) {
      expect(targetsUsage(key), key).toBe(true);
      expect(pricingOpen(key), key).toBe(true);
    }
    // 只是打开这张卡（`usage`）时不强行展开：那一节多数人一次都不会改。
    expect(pricingOpen("usage")).toBe(false);
    expect(pricingOpen(null)).toBe(false);
  });

  it("分配槽归 AI 服务页，不切本地模型页签", () => {
    expect(viewForRouteTarget("ai.assignment.default_llm", advanced)).toBeNull();
    expect(viewForRouteTarget("ai.assignment.default_llm", standard)).toBeNull();
  });

  it("其余子视图按名落位；未知尾段与空尾段一律 null（fail closed）", () => {
    expect(viewForRouteTarget("services", advanced)).toBe("services");
    expect(viewForRouteTarget("local", advanced)).toBe("local");
    expect(viewForRouteTarget("nope", advanced)).toBeNull();
    expect(viewForRouteTarget(null, advanced)).toBeNull();
  });
});

/**
 * 「当前生效」是一句**断言**，只有它为真时才能说。
 *
 * 真机 2026-08-09：打开「仅本地」后转写行的徽标是「能力不匹配」，同一行却写着
 * 「当前生效: 服务端转写」。用户信的是后面那句人话，于是以为音频还在往服务端送，
 * 而实际执行早已回到本机——这正是本仓一路在防的「屏幕上写的 ≠ 字节的去向」。
 */
describe("effectiveLabelKey", () => {
  it("只有 ready 才说「当前生效」", () => {
    expect(effectiveLabelKey("ready", "bge-m3")).toBe("models.effective");
  });

  it("拿不到就绪的那几档一律降级为「已选，当前未生效」", () => {
    for (const readiness of [
      "incompatible",
      "missing",
      "profile_unverified",
    ]) {
      expect(effectiveLabelKey(readiness, "服务端转写")).toBe("models.selected_inactive");
    }
  });

  it("没有解析出目标时不降级——那一档渲染的是「未解析」，本就没在声称什么生效", () => {
    expect(effectiveLabelKey("unresolved", null)).toBe("models.effective");
  });
});
