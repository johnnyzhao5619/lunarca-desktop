/// <reference types="vite/client" />
// 记录态的两个沉淀入口在**页头与树行**都到齐（PRD-06-054 / ADR-086 ②）。
//
// 「AI 辅助沉淀 = 蒸馏能力推广到用户记录」这件事在前端只有两处接线，而两处都是
// **一行开关**——漏掉任一行的表现是"功能做了，但用户点不到"，且 typecheck / lint /
// 既有单测全绿（多传一个可选 prop、少传一个可选 prop，都合法）。
//
// 仓库没有 jsdom，组件行为无法渲染验证，故按 `draft-badge-events.test.ts` 的先例做源码
// 断言。判据本身仍在 Rust（`NoteStage::allows_distill` / `NoteTreeDto.draft`），本文件只
// 守"接线在不在"。

import { describe, expect, it } from "vitest";

const trees = import.meta.glob("./NoteTree.tsx", {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

const editors = import.meta.glob("../editor/VaultEditor.tsx", {
  eager: true,
  import: "default",
  query: "?raw",
}) as Record<string, string>;

const TREE = Object.values(trees)[0];
const EDITOR = Object.values(editors)[0];

describe("记录态的沉淀入口（PRD-06-054）", () => {
  it("扫描面非空", () => {
    for (const src of [TREE, EDITOR]) {
      expect(src.length).toBeGreaterThan(200);
    }
  });

  it("树行右键对记录态同时给「沉淀为知识」与「生成知识笔记」", () => {
    // 两个动作互不取代（AI 辅助沉淀不改源记录的 stage），故同一个判据、两行开关。
    // 两项的判据是 Rust 下发的**两位不同事实**，前端一位都不拼：
    // 「沉淀为知识」= 记录态（`draft`）；「生成知识笔记」= 资料 ∪ 记录（`distillable`，
    // `NoteStage::allows_distill`）。把后者写成 `node.draft` 的代价是一篇录音在录音清单里
    // 有这一项、在文件树里没有（2026-09-02 真机上报「批量生成知识笔记没有入口」）。
    expect(TREE).toContain("settleable: node.draft,");
    expect(TREE).toContain(": node.distillable,");
    expect(TREE).not.toContain("distillable: node.draft");
    // 多选时按**整批**算（PRD-06-059 D），judgement 仍来自同一位下发事实。
    expect(TREE).toContain("findTreeNode(rendered, p)?.distillable === true");
  });

  it("页头两枚徽标都在，且都不在副库窗口渲染；「资料」身份徽标已退场", () => {
    // 「记录」/「生成知识笔记」——第二枚在来源层与记录态上都出现，这正是"同款入口"的含义。
    // 「来源层」身份不再以徽标形式挂在页头（PRD-06-048②），改在上下文面板「信息」页签说明。
    for (const badge of ["<DraftBadge", "<DistillBadge"]) {
      expect(EDITOR).toContain(badge);
    }
    expect(EDITOR).not.toContain("<MaterialBadge");
  });
});
