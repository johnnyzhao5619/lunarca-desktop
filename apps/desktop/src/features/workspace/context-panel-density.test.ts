/**
 * 「上下文」面板的**页签密度**（批次 M，PRD-07-031 加了两格之后的复核）。
 *
 * # 这笔账
 *
 * 面板宽度是**固定 260px**（`.luna-workspace__panel`，不可拖拽），扣掉内距后页签网格
 * 可用约 228px。页签是 `repeat(2, minmax(0, 1fr))`：
 *
 * - **两列不是随便定的，是被宽度逼出来的**。三列每列只剩约 73px，而英文最长的那条
 *   `Backlinks` 配 14px 图标与内距约需 81px——放三列会截断或溢出。`auto-fit` 同样救不了：
 *   容器宽度是死的，永远算不出第三列。
 * - 于是 **N 个页签 = ⌈N/2⌉ 行**。今天 7 个 = 4 行，约 132px，占一个 700px 高面板的两成，
 *   而末行还空着一格。批次 D 之前是 5 个 = 3 行。
 *
 * # 这条用例不是在说「7 个正好」，是在**让第 8 个页签必须先回答这个问题**
 *
 * 加第 8 格 = 加满一行（4 行→4 行）还好，第 9 格就是第 5 行。密度不会一次变糟，它是
 * 一格一格温水化的——而每一格单看都「只多一个」。判据钉住计数与列数，谁要动就得在这里
 * 把账重算一遍，顺带被迫读到上面那两段。
 *
 * 判据**不锁像素、不锁截图**（同 `theme/hud-density-guard.test.ts` 的口径）：锁的是
 * 「几格 × 几列」这两个数，以及「宽度是固定的」这个前提——前提变了（面板可拖拽了），
 * 上面整段推理就得重来，而那时这条也该跟着改。
 */

import { describe, expect, it } from "vitest";

import { CONTEXT_TABS } from "./context-panel-tabs";
import { runtimeStyleCss as css } from "../../theme/style-source";

describe("上下文面板的页签密度（批次 M）", () => {
  it("页签数与列数：7 格 × 2 列 = 4 行，动它就得重算这笔账", () => {
    expect(CONTEXT_TABS.length).toBe(7);
    expect(css).toMatch(
      /\.luna-context-panel__tabs\s*\{[^}]*grid-template-columns:\s*repeat\(2,/s,
    );
  });

  it("面板宽度是固定的——上面那段推理的前提", () => {
    // 这一条单独立，是因为它是**前提**而不是结论：面板哪天变成可拖拽的，
    // 「三列放不下」就不再成立，密度判据也该换一种写法（而不是把这条删掉了事）。
    expect(css).toMatch(/\.luna-workspace__panel\s*\{[^}]*width:\s*260px/s);
  });
});
