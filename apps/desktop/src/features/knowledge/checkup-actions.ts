/**
 * 体检建议卡的动作映射（PRD-16-032）。纯逻辑，可 headless 测试。
 *
 * **这里没有任何"是不是问题"的判断**——那全在 Rust（`luna_vault::checkup`）。本文件只
 * 回答一个呈现问题：卡上那两个按钮按下去分别通向哪条**既有**流程。去处刻意穷尽：打开那篇
 * 笔记（并跳到那一行）、拉起批量整理确认卡、拉起补齐标签确认卡、掀开记录、在访达里显示一个
 * 文件夹——体检自己不写任何东西（`PRD-16-032`：只出建议、绝不自动修）。
 */

import type { CheckupFindingDto } from "../../ipc/bindings";

export type CheckupAction =
  /** 打开笔记（`line` 非空则跳到那一行）。 */
  | { kind: "note"; path: string; line: number | null }
  /** 拉起批量整理确认卡（待整理积压的去处）。 */
  | { kind: "distill-batch" }
  /** 拉起「全库补齐关系标签」确认卡（还没有标签的那一类的处置）。 */
  | { kind: "rescan" }
  /**
   * 掀开文件树的「隐藏记录」并打开最旧的那一篇草稿（待沉淀积压的处置，PRD-06-055）。
   *
   * **刻意没有"一键全沉淀"**：把草稿判成知识只有用户有资格（PRD-06-053），故这条
   * 动作只负责把他带到现场——树上看得见那几篇、页头有沉淀入口，点不点是他的事。
   */
  | { kind: "show-drafts"; path: string | null }
  /**
   * 在访达里显示一个文件夹（读不了的文件夹的处置，V1b）：改权限是用户在系统里做的事，
   * 体检只把他带到那个文件夹跟前。
   */
  | { kind: "reveal"; path: string };

/**
 * 卡片主按钮的去处。
 *
 * 待整理积压是唯一不落在某一篇笔记上的一类——它指的是"这一堆资料"，故通向批量确认卡
 * （那里有全口径的四桶数字）。其余各类都指名道姓地涉及某一页，主按钮就是把用户带到现场。
 */
export function primaryAction(finding: CheckupFindingDto): CheckupAction | null {
  if (finding.kind === "distill-backlog") {
    return { kind: "distill-batch" };
  }
  if (finding.kind === "untagged") {
    return { kind: "rescan" };
  }
  if (finding.kind === "settle-backlog") {
    // 芯片按「最旧在前」由 Rust 排好（`settle_backlog`），第一项就是该先看的那一篇；
    // 一篇都没有也照常掀开树——"有 N 篇记录待沉淀"这句话本身就还成立。
    return { kind: "show-drafts", path: finding.paths[0] ?? null };
  }
  const path = finding.paths[0];
  if (finding.kind === "unreadable-folder") {
    return path ? { kind: "reveal", path } : null;
  }
  return path ? { kind: "note", path, line: finding.line } : null;
}

/**
 * 「修复经预览」的去处；`null` = 这一类没有确定性修复（判定在 Rust 的 `fix` 字段，
 * 前端不自己决定谁可以修）。
 */
export function fixAction(finding: CheckupFindingDto): CheckupAction | null {
  // 「还没有标签」的确定性处置是跑一次全库补齐——而那张确认卡在 D7 之后会如实列出被
  // 停用的文件夹并给就地取消，于是"为什么没有标签"与"怎么解除"在同一处闭环。
  if (finding.fix === "rescan") {
    return { kind: "rescan" };
  }
  return null;
}

/**
 * 主按钮的文案 key（`workspace:checkup.action.*`）。
 *
 * 逐类给而不是统一"打开"：卡片说的是"这一页没人链它"，按钮却写"打开"，用户要多想一步
 * 才知道点了会发生什么。
 */
export function primaryActionKey(finding: CheckupFindingDto): string {
  switch (finding.kind) {
    case "distill-backlog":
      return "checkup.action.distill";
    case "settle-backlog":
      // 「去看看那几篇」而不是「沉淀」——按钮不该承诺一个它不会做的动作。
      return "checkup.action.showDrafts";
    case "untagged":
      // 这一类没有"某一篇"可打开——主按钮就是去补齐标签（与 fix 同一去处，故只留一个）。
      return "checkup.action.rescan";
    case "broken-link":
      return "checkup.action.openCiting";
    case "unreadable-folder":
      return "checkup.action.reveal";
    default:
      return "checkup.action.open";
  }
}
