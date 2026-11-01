/**
 * 「本机资源不足」与「服务端额度用尽」必须是两回事（守护）。
 *
 * 此前两者共用 `RESOURCE_EXHAUSTED` 一个码，于是额度用尽的用户被告知
 * 「请关闭其他模型任务或选择更小的模型」——一条与他的处境毫无关系的建议，
 * 照做也不会有任何改善；而真正该做的（看余量、等重置、找管理员加量）无人提示。
 *
 * 这条钉住三件事：两个码各有各的文案、文案不雷同、深链去往不同的地方。
 * 合并回一个码时，编译、lint、其余用例全绿，只有它会红。
 */

import { describe, expect, it } from "vitest";

import zhCommon from "./locales/zh-CN/common.json";
import enCommon from "./locales/en-US/common.json";
import { failureGuidance } from "./failure";

/** 在嵌套字典里找某个 key 的值（错误码文案的挂点各 ns 不同）。 */
function find(node: unknown, key: string): string | undefined {
  if (typeof node !== "object" || node === null) return undefined;
  for (const [k, v] of Object.entries(node as Record<string, unknown>)) {
    if (k === key && typeof v === "string") return v;
    const nested = find(v, key);
    if (nested !== undefined) return nested;
  }
  return undefined;
}

describe("额度用尽 ≠ 本机资源不足", () => {
  // 对话表面自 ADR-111 ④（批 E2）起共用 common 注册表，第二份 chat 文案已删。
  it.each([
    ["zh-CN common", zhCommon, "RESOURCE_EXHAUSTED", "QUOTA_EXHAUSTED"],
    ["en-US common", enCommon, "RESOURCE_EXHAUSTED", "QUOTA_EXHAUSTED"],
  ])("%s：两个码各有文案且不雷同", (_label, bundle, resourceKey, quotaKey) => {
    const resource = find(bundle, resourceKey);
    const quota = find(bundle, quotaKey);
    expect(resource, `缺 ${resourceKey}`).toBeTruthy();
    expect(quota, `缺 ${quotaKey}`).toBeTruthy();
    expect(quota).not.toBe(resource);
  });

  it("额度用尽的文案不得建议「换更小的模型」——那是本机资源不足的动作", () => {
    const quota = find(zhCommon, "QUOTA_EXHAUSTED") ?? "";
    expect(quota).not.toMatch(/更小的模型|释放内存|关闭其他/);
    expect(quota, "要指向可执行的地方：账户 / 用量").toMatch(/账户|用量|额度/);
  });

  it("两个码的深链去往不同的地方", () => {
    const resource = failureGuidance("RESOURCE_EXHAUSTED")?.route;
    const quota = failureGuidance("QUOTA_EXHAUSTED")?.route;
    expect(quota).toBeTruthy();
    expect(quota).not.toBe(resource);
    expect(quota, "额度问题在账户页看，不在模型页").toContain("account");
  });
});
