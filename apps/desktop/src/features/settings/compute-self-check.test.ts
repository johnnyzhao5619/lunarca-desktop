/**
 * 「试跑一次」的两条纪律（ADR-126 第 8 条 + 真机 2026-09-12 的两条补充）。
 *
 * 两条都不是渲染断言够得着的——它们是**接线**：结果句承诺了什么，代码就得真的做什么。
 * 扫源码是这里唯一能证明它的手段（同 `status-visit-metrics.test.ts` 的形态）。
 */

import { describe, expect, it } from "vitest";

const SOURCE = import.meta.glob("./ComputeSelfCheck.tsx", {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

const CODE = Object.values(SOURCE)[0] ?? "";

/** 注释里也会出现这些词，判据只看代码——先把注释整段切掉。 */
function stripComments(source: string): string {
  return source.replace(/\/\*[\s\S]*?\*\//g, "").replace(/^\s*\/\/.*$/gm, "");
}

describe("试跑一次的接线", () => {
  it("跑完通知执行记录重读——结果句承诺的正是「已记入下方」", () => {
    // 真机：那条记录要手动点一次「刷新」才出现，而结果句说它已经在下面了。
    expect(stripComments(CODE)).toContain("refreshUsage()");
  });

  it("落点变了就丢掉上一次的结论——它说的是另一种配置下的事实", () => {
    // 真机：切到仅本机之后屏幕上仍写着「由『Lunarca 云』执行」。一条过期的成功结论
    // 恰好长得像"这条路没问题"，比没有结论更糟。
    const code = stripComments(CODE);
    expect(code).toContain("useComputeStatus()");
    // **身份取「对话这条路」而不是这台机器的落点**：从「服务端优先」切到「本机优先」时
    // `status.location` 一动不动，而执行者已经从云换成了本机（真机同日二次纠正）。
    expect(code, "身份必须取自 chat 那条 route 的位置与理由").toMatch(
      /capability === "chat"[\s\S]*?chatRoute\.location[\s\S]*?chatRoute\.reason/,
    );
    expect(code, "不得拿机器落点当这条路的身份").not.toMatch(
      /routeIdentity[\s\S]{0,80}status\.location/,
    );
    expect(code, "身份变了要把结果清回 idle").toMatch(/useEffect\([\s\S]*?kind: "idle"/);
  });
});
