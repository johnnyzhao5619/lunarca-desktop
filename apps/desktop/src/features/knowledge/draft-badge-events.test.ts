/// <reference types="vite/client" />
// 成熟度事实的**双事件通道**守护（PRD-06-052/053/058；批次 5 验收发现的缺陷，防复发）。
//
// 「记录」徽标与编辑器右键「标记为记录」要在两类写盘后刷新，而这两类走的是**不同的事件**：
// - 编辑器内保存（`write_note`）只发 `note://written`——自写回声在 `vault://changed`
//   一侧被抑制；
// - 外部编辑器改文件与程序化改写（沉淀 / 标记为记录 / 整理列表层级）走 `vault://changed`。
//
// 只听其中一条，另一半场景就静默失效：首版只听 `vaultChanged`，用户在正文里敲下
// `stage` 保存后徽标不出现——恰是它注释第一句承诺的场景。仓库没有 jsdom，
// 组件行为无法渲染验证，故按 parity 守护的先例做源码断言。
//
// 断言的对象自 PRD-06-058 起是**那个 Hook**（`use-note-draft-state.ts`）：订阅逻辑已
// 从徽标里收敛出来，两个消费面（徽标 / 编辑器右键）共用它。守护跟着事实源搬家——否则它
// 会在一个不再负责订阅的文件上恒绿。

import { describe, expect, it } from "vitest";

const sources = import.meta.glob("./{use-note-draft-state.ts,DraftBadge.tsx}", {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

const HOOK = sources["./use-note-draft-state.ts"];
const BADGE = sources["./DraftBadge.tsx"];

describe("成熟度事实：写盘事件的两条通道都在听", () => {
  it("扫描面非空", () => {
    expect(HOOK?.length ?? 0).toBeGreaterThan(200);
    expect(BADGE?.length ?? 0).toBeGreaterThan(200);
  });

  it("同时监听 vaultChanged 与 noteWritten，且都按路径过滤", () => {
    expect(HOOK).toContain("events.vaultChanged.listen");
    expect(HOOK).toContain("events.noteWritten.listen");
    // 两条都是全库信号，必须按本篇路径过滤（批量动作期间不白发判定）。
    expect(HOOK).toContain("event.payload.rel_path === notePath");
    expect(HOOK).toContain("event.payload.path === notePath");
  });

  it("徽标消费那个 Hook，而不是自己再订阅一遍", () => {
    // 各订阅一份的代价不是重复，而是两处会答出不同的话——右键说"能标记为记录"，
    // 徽标却已经显示它是记录态。
    expect(BADGE).toContain("useNoteDraftState");
    expect(BADGE).not.toContain(".listen(");
  });
});
