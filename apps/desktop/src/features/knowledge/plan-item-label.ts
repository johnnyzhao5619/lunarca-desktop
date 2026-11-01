/**
 * 一条整理建议条目的动作标签键（`workspace:topicEvidence.action.*`）。主题页面板与知识库首页
 * 「待你看」共用：新增若依据是转写或速记，标它的身份而不只是「新增」——那不是已核实的事实。
 */

import type { SealedPlanItemDto } from "../../ipc/bindings";

export function planItemLabelKey(item: SealedPlanItemDto): string {
  if (item.action === "append" && item.transcript_record) return "topicEvidence.action.transcript";
  if (item.action === "append" && item.quick_note_record) return "topicEvidence.action.quickNote";
  return `topicEvidence.action.${item.action}`;
}
